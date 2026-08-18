//! gatekeeperd — the privileged broker (Phase 4, slice 2).
//!
//! The ONLY merge-capable component. oniond (unprivileged) and shrekctl (operator) connect over a
//! root-owned unix socket and REQUEST layer operations; gatekeeperd independently re-checks each
//! request against the SEALED policy (/usr/lib/shrek/onion-policy) — trusting NOTHING from the caller
//! — enforces the signature/verity gate via `systemd-sysext --image-policy`, and performs the
//! privileged mount+merge. A compromised caller cannot widen the merge (threat-model ADV-8,
//! isolation.md §7, security-model §4/§6). docs/phase4-gatekeeperd.md.
//!
//! Runs as root but scoped to CAP_SYS_ADMIN via the unit's CapabilityBoundingSet= — the merge's real
//! need (mount/overlayfs/dm-verity/loop), not arbitrary root. Long-running + Restart=always
//! (supervised). Dependency-free: the peer-credential check is a raw getsockopt syscall; the audit
//! record is hand-written JSON.
//!
//! Env overrides for the host/container repro (no systemd, no VM): SHREK_ONION_POLICY,
//! SHREK_ONION_STORE, SHREK_ONION_STATE, SHREK_SYSEXT_DIR, SHREK_CONFEXT_DIR, SHREK_BROKER_SOCK,
//! SHREK_BROKER_NOMOUNT=1 (skip the real store mount; treat SHREK_ONION_STORE as already present).

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::{symlink, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::Path;
use std::process::Command;

/// Fixed trust gate (same as slice 1 / Phase 2). Baked in code, never read from the untrusted store.
const IMAGE_POLICY: &str = "root=signed+absent:usr=signed+absent";

struct Layer {
    name: String,
    kind: &'static str,
    present: bool,
    decision: String,
    reason: String,
}

struct Broker {
    policy_path: String,
    store: String,
    state_path: String,
    sysext_dir: String,
    confext_dir: String,
    layers: Vec<Layer>,
    sysext_rc: i32,
    confext_rc: i32,
}

fn env_or(key: &str, default: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| default.to_string())
}

// ---- peer credentials (SO_PEERCRED via raw getsockopt syscall; x86-64) ----

#[repr(C)]
#[derive(Default, Clone, Copy)]
struct Ucred {
    pid: i32,
    uid: u32,
    gid: u32,
}

/// Read the connecting peer's (pid,uid,gid). Dependency-free: SOL_SOCKET=1, SO_PEERCRED=17,
/// getsockopt = syscall 55 on x86-64. struct ucred is 12 bytes, no padding.
fn peer_cred(s: &UnixStream) -> std::io::Result<Ucred> {
    const SYS_GETSOCKOPT: i64 = 55;
    const SOL_SOCKET: i32 = 1;
    const SO_PEERCRED: i32 = 17;
    let mut cred = Ucred::default();
    let mut len: u32 = core::mem::size_of::<Ucred>() as u32;
    let ret: i64;
    unsafe {
        core::arch::asm!(
            "syscall",
            inlateout("rax") SYS_GETSOCKOPT => ret,
            in("rdi") s.as_raw_fd(),
            in("rsi") SOL_SOCKET,
            in("rdx") SO_PEERCRED,
            in("r10") &mut cred as *mut Ucred,
            in("r8")  &mut len as *mut u32,
            in("r9")  0i64,
            lateout("rcx") _,
            lateout("r11") _,
            options(nostack),
        );
    }
    if ret < 0 {
        return Err(std::io::Error::from_raw_os_error(-ret as i32));
    }
    Ok(cred)
}

/// Resolve a username → (uid,gid) from /etc/passwd (colon fields: name:x:uid:gid:…).
fn resolve_user(name: &str) -> Option<(u32, u32)> {
    for line in fs::read_to_string("/etc/passwd").ok()?.lines() {
        let f: Vec<&str> = line.split(':').collect();
        if f.len() >= 4 && f[0] == name {
            return Some((f[2].parse().ok()?, f[3].parse().ok()?));
        }
    }
    None
}

// ---- policy + store ----

fn read_policy(path: &str) -> BTreeSet<String> {
    let mut set = BTreeSet::new();
    let Ok(text) = fs::read_to_string(path) else {
        eprintln!("gatekeeperd: WARN cannot read sealed policy {path} — treating as empty (deny all)");
        return set;
    };
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut parts = line.split_whitespace();
        if parts.next() == Some("enable") {
            if let Some(n) = parts.next() {
                set.insert(n.to_string());
            }
        }
    }
    set
}

fn scan_ddis(dir: &str) -> BTreeMap<String, String> {
    let mut m = BTreeMap::new();
    let Ok(rd) = fs::read_dir(dir) else { return m };
    for ent in rd.flatten() {
        let p = ent.path();
        if let Some(f) = p.file_name().and_then(|s| s.to_str()) {
            if let Some(name) = f.strip_suffix(".raw") {
                m.insert(name.to_string(), p.to_string_lossy().into_owned());
            }
        }
    }
    m
}

/// Clean stale `*.raw` symlinks from a search dir.
fn clean_search(dir: &str) {
    let _ = fs::create_dir_all(dir);
    if let Ok(rd) = fs::read_dir(dir) {
        for ent in rd.flatten() {
            let p = ent.path();
            if p.file_name()
                .and_then(|s| s.to_str())
                .is_some_and(|n| n.ends_with(".raw"))
            {
                let _ = fs::remove_file(&p);
            }
        }
    }
}

fn expose_one(dir: &str, name: &str, target: &str) -> bool {
    let link = format!("{dir}/{name}.raw");
    let _ = fs::remove_file(&link);
    match symlink(target, &link) {
        Ok(()) => true,
        Err(e) => {
            eprintln!("gatekeeperd: WARN symlink {link} -> {target}: {e}");
            false
        }
    }
}

fn run_sysext(bin: &str, verb: &str) -> i32 {
    match Command::new(bin)
        .arg(format!("--image-policy={IMAGE_POLICY}"))
        .arg(verb)
        .status()
    {
        Ok(s) => s.code().unwrap_or(-1),
        Err(e) => {
            eprintln!("gatekeeperd: WARN failed to run {bin} {verb}: {e}");
            -1
        }
    }
}

/// `refresh` re-applies the whole overlay from the current search dirs (systemd 257). Retry once —
/// the unmerge phase can hit a transient EBUSY if something holds /usr open (researcher caveat).
fn refresh(bin: &str) -> i32 {
    let rc = run_sysext(bin, "refresh");
    if rc != 0 {
        return run_sysext(bin, "refresh");
    }
    rc
}

impl Broker {
    /// merge <requested…> — the boot path. Expose exactly (sealed ∩ requested ∩ present); merge; then
    /// classify every store layer for the audit. A requested-but-unsealed layer is refused by the
    /// wall (not-sealed-policy) and never exposed — the ADV-8 invariant.
    fn handle_merge(&mut self, requested: &[String]) -> Vec<String> {
        let sealed = read_policy(&self.policy_path);
        let sysext = scan_ddis(&format!("{}/extensions", self.store));
        let confext = scan_ddis(&format!("{}/confexts", self.store));
        let req: BTreeSet<&String> = requested.iter().collect();

        clean_search(&self.sysext_dir);
        clean_search(&self.confext_dir);
        let mut exposed: BTreeSet<String> = BTreeSet::new();
        for (n, path) in sysext.iter() {
            if sealed.contains(n) && req.contains(n) && expose_one(&self.sysext_dir, n, path) {
                exposed.insert(n.clone());
            }
        }
        for (n, path) in confext.iter() {
            if sealed.contains(n) && req.contains(n) && expose_one(&self.confext_dir, n, path) {
                exposed.insert(n.clone());
            }
        }

        self.sysext_rc = run_sysext("systemd-sysext", "merge");
        self.confext_rc = run_sysext("systemd-confext", "merge");

        let mut layers: Vec<Layer> = Vec::new();
        let classify = |n: &String, rc: i32| -> (String, String) {
            if exposed.contains(n) {
                if rc == 0 {
                    ("merged".into(), String::new())
                } else {
                    ("refused".into(), "image-policy".into())
                }
            } else if req.contains(n) && !sealed.contains(n) {
                ("refused".into(), "not-sealed-policy".into()) // G3: the wall refuses a lying caller
            } else if !req.contains(n) {
                let r = if sealed.contains(n) { "not-requested" } else { "not-enabled" };
                ("omitted".into(), r.into())
            } else {
                ("error".into(), "not-exposed".into())
            }
        };
        for (n, _) in sysext.iter() {
            let (d, r) = classify(n, self.sysext_rc);
            layers.push(Layer { name: n.clone(), kind: "sysext", present: true, decision: d, reason: r });
        }
        for (n, _) in confext.iter() {
            let (d, r) = classify(n, self.confext_rc);
            layers.push(Layer { name: n.clone(), kind: "confext", present: true, decision: d, reason: r });
        }
        for n in requested {
            if !sysext.contains_key(n) && !confext.contains_key(n) {
                layers.push(Layer {
                    name: n.clone(),
                    kind: "unknown",
                    present: false,
                    decision: "absent".into(),
                    reason: "enabled-but-absent".into(),
                });
            }
        }
        layers.sort_by(|a, b| a.name.cmp(&b.name));
        self.layers = layers;
        self.write_state();
        self.response_lines()
    }

    /// activate/deactivate <name> — the runtime API. Same sealed re-check; live overlay via refresh.
    fn handle_activate(&mut self, name: &str, on: bool) -> Vec<String> {
        let sealed = read_policy(&self.policy_path);
        let sysext = scan_ddis(&format!("{}/extensions", self.store));
        let confext = scan_ddis(&format!("{}/confexts", self.store));
        let mut out = Vec::new();

        if !sealed.contains(name) {
            out.push(format!("RESULT {name} unknown refused not-sealed-policy"));
            out.push("END 1 -".into());
            return out;
        }
        let (kind, target, dir, bin) = if let Some(t) = sysext.get(name) {
            ("sysext", t.clone(), self.sysext_dir.clone(), "systemd-sysext")
        } else if let Some(t) = confext.get(name) {
            ("confext", t.clone(), self.confext_dir.clone(), "systemd-confext")
        } else {
            out.push(format!("RESULT {name} unknown absent enabled-but-absent"));
            out.push("END 1 -".into());
            return out;
        };

        let (decision, reason, rc) = if on {
            let _ = expose_one(&dir, name, &target);
            let rc = refresh(bin);
            if rc == 0 { ("merged", String::new(), 0) } else { ("refused", "image-policy".into(), rc) }
        } else {
            let _ = fs::remove_file(format!("{dir}/{name}.raw"));
            let rc = refresh(bin);
            ("omitted", "deactivated".into(), rc)
        };
        self.update_layer(name, kind, &decision.to_string(), &reason);
        self.write_state();
        out.push(format!("RESULT {name} {kind} {decision} {}", if reason.is_empty() { "-" } else { &reason }));
        out.push(format!("END {rc} -"));
        out
    }

    fn handle_status(&self) -> Vec<String> {
        let mut out: Vec<String> = self
            .layers
            .iter()
            .map(|l| {
                format!(
                    "RESULT {} {} {} {}",
                    l.name,
                    l.kind,
                    l.decision,
                    if l.reason.is_empty() { "-" } else { &l.reason }
                )
            })
            .collect();
        out.push(format!("END {} {}", self.sysext_rc, self.confext_rc));
        out
    }

    fn update_layer(&mut self, name: &str, kind: &'static str, decision: &str, reason: &str) {
        if let Some(l) = self.layers.iter_mut().find(|l| l.name == name) {
            l.decision = decision.to_string();
            l.reason = reason.to_string();
        } else {
            self.layers.push(Layer {
                name: name.to_string(),
                kind,
                present: true,
                decision: decision.to_string(),
                reason: reason.to_string(),
            });
            self.layers.sort_by(|a, b| a.name.cmp(&b.name));
        }
    }

    fn response_lines(&self) -> Vec<String> {
        self.handle_status()
    }

    fn write_state(&self) {
        let path = &self.state_path;
        if let Some(p) = Path::new(path).parent() {
            let _ = fs::create_dir_all(p);
        }
        let mut s = String::from("{\n  \"version\": 1,\n");
        s.push_str(&format!("  \"policy\": {},\n", json_str(&self.policy_path)));
        s.push_str(&format!("  \"sysext_merge_rc\": {},\n", self.sysext_rc));
        s.push_str(&format!("  \"confext_merge_rc\": {},\n", self.confext_rc));
        let en: Vec<String> = read_policy(&self.policy_path).iter().map(|e| json_str(e)).collect();
        s.push_str(&format!("  \"enabled\": [{}],\n  \"layers\": [\n", en.join(", ")));
        for (i, l) in self.layers.iter().enumerate() {
            let comma = if i + 1 < self.layers.len() { "," } else { "" };
            s.push_str(&format!(
                "    {{ \"name\": {}, \"kind\": {}, \"present\": {}, \"decision\": {}, \"reason\": {} }}{}\n",
                json_str(&l.name), json_str(l.kind), l.present, json_str(&l.decision), json_str(&l.reason), comma
            ));
        }
        s.push_str("  ]\n}\n");
        if let Err(e) = fs::write(path, s) {
            eprintln!("gatekeeperd: WARN cannot write audit {path}: {e}");
        }
    }
}

fn json_str(s: &str) -> String {
    let mut o = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => o.push_str("\\\""),
            '\\' => o.push_str("\\\\"),
            _ => o.push(c),
        }
    }
    o.push('"');
    o
}

fn handle_conn(b: &mut Broker, stream: UnixStream, allowed: &BTreeSet<u32>) {
    let cred = match peer_cred(&stream) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("gatekeeperd: WARN cannot read peer creds: {e} — dropping");
            return;
        }
    };
    if !allowed.contains(&cred.uid) {
        eprintln!("gatekeeperd: DENY connection uid={} gid={} pid={} (not allowlisted)", cred.uid, cred.gid, cred.pid);
        let mut s = stream;
        let _ = writeln!(s, "END 1 -");
        return;
    }
    let mut reader = BufReader::new(match stream.try_clone() {
        Ok(s) => s,
        Err(_) => return,
    });
    let mut line = String::new();
    if reader.read_line(&mut line).is_err() {
        return;
    }
    let mut tok = line.split_whitespace();
    let verb = tok.next().unwrap_or("");
    let args: Vec<String> = tok.map(String::from).collect();
    eprintln!("gatekeeperd: req uid={} pid={} verb={verb} args={:?}", cred.uid, cred.pid, args);

    let resp = match verb {
        "merge" => b.handle_merge(&args),
        "activate" => b.handle_activate(args.first().map(String::as_str).unwrap_or(""), true),
        "deactivate" => b.handle_activate(args.first().map(String::as_str).unwrap_or(""), false),
        "status" => b.handle_status(),
        other => vec![format!("RESULT - - refused unknown-verb-{other}"), "END 1 -".into()],
    };
    let mut w = stream;
    for l in &resp {
        let _ = writeln!(w, "{l}");
    }
}

fn main() {
    let sock = env_or("SHREK_BROKER_SOCK", "/run/shrek-gk.sock");
    let store = env_or("SHREK_ONION_STORE", "/run/shrek-store");

    // Mount the untrusted layer store read-only (privileged — this is why the broker exists). Skipped
    // for the host repro (SHREK_BROKER_NOMOUNT=1, store points at a plain dir).
    if std::env::var("SHREK_BROKER_NOMOUNT").is_err() {
        let _ = fs::create_dir_all(&store);
        let rc = Command::new("mount")
            .args(["-o", "ro", "/dev/disk/by-label/shrek-layers", &store])
            .status();
        match rc {
            Ok(s) if s.success() => eprintln!("gatekeeperd: mounted layer store at {store}"),
            _ => eprintln!("gatekeeperd: NOTE no layer store mounted (fail-closed: nothing to merge)"),
        }
    }

    // Resolve the unprivileged control-plane user; allow only it + root to connect.
    let mut allowed: BTreeSet<u32> = BTreeSet::from([0]);
    let shrek = resolve_user("shrek");
    if let Some((uid, _)) = shrek {
        allowed.insert(uid);
    } else {
        eprintln!("gatekeeperd: WARN 'shrek' user not found — only root may connect (oniond will fail-closed)");
    }
    // Repro-only: allow extra uids to connect (the host/container smoke runs as a normal user, not
    // root or shrek). Never set in the shipped units.
    if let Ok(extra) = std::env::var("SHREK_BROKER_ALLOW_UID") {
        for tok in extra.split([',', ' ']) {
            if let Ok(u) = tok.trim().parse::<u32>() {
                allowed.insert(u);
            }
        }
    }

    let _ = fs::remove_file(&sock);
    if let Some(p) = Path::new(&sock).parent() {
        let _ = fs::create_dir_all(p);
    }
    let listener = match UnixListener::bind(&sock) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("gatekeeperd: FATAL cannot bind {sock}: {e}");
            std::process::exit(1);
        }
    };
    // The socket is world-connectable (0666), but SO_PEERCRED is the AUTHORITATIVE gate: every
    // connection's peer uid must be in the allowlist (root + shrek) or it is denied + logged. This is
    // an unspoofable per-uid check, stronger than group perms — and it keeps the broker scoped to
    // CAP_SYS_ADMIN alone (a chgrp to the shrek group would require CAP_CHOWN, widening privilege).
    let _ = fs::set_permissions(&sock, fs::Permissions::from_mode(0o666));
    eprintln!("gatekeeperd: listening on {sock} (allowed uids {:?})", allowed);

    let mut broker = Broker {
        policy_path: env_or("SHREK_ONION_POLICY", "/usr/lib/shrek/onion-policy"),
        store,
        state_path: env_or("SHREK_ONION_STATE", "/run/shrek/onion.json"),
        sysext_dir: env_or("SHREK_SYSEXT_DIR", "/run/extensions"),
        confext_dir: env_or("SHREK_CONFEXT_DIR", "/run/confexts"),
        layers: Vec::new(),
        sysext_rc: 0,
        confext_rc: 0,
    };

    for conn in listener.incoming() {
        match conn {
            Ok(stream) => handle_conn(&mut broker, stream, &allowed),
            Err(e) => eprintln!("gatekeeperd: WARN accept: {e}"),
        }
    }
}
