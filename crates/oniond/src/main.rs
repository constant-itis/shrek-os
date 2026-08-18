//! oniond — Shrek Onion layer policy & orchestration (Phase 4, slice 1).
//!
//! oniond implements NO layering — systemd-sysext / dm-verity / mount / the kernel VFS do the
//! dangerous low-level work (architecture.md §3). oniond owns the POLICY: read a sealed, trusted
//! enable-list; select which signed layers on the untrusted store belong on THIS machine; expose
//! ONLY those to systemd-sysext (symlinks in the tmpfs search dir — the documented selection
//! mechanism, since `merge` has no per-name flag); drive the merge under a FIXED, baked
//! `--image-policy` trust gate; and write a structured audit record. Phase-2's `onion-merge` shell
//! hardcoded all of this — oniond replaces it (docs/phase4-oniond.md).
//!
//! Slice-1 scope: boot-invoked `oniond merge`; no privilege separation (a later slice routes the
//! merge through gatekeeperd); no IPC daemon (shrekctl reads the /run state file). The binary is
//! dependency-free by design (minimal-deps) — the JSON audit record is hand-written.
//!
//! Overridable via env (for the faithful container repro without a VM): SHREK_ONION_POLICY,
//! SHREK_ONION_STORE, SHREK_ONION_STATE, SHREK_SYSEXT_DIR, SHREK_CONFEXT_DIR.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::os::unix::fs::symlink;
use std::path::Path;
use std::process::Command;

/// The trust gate — the SAME fixed policy Phase-2's onion-merge proved. Baked in code (trusted),
/// never read from the untrusted store. `signed` implies Verity + PKCS#7; `+absent` lets the other
/// designator be missing. This is the refusal lever (gate O3): an unsigned/tampered DDI fails to merge.
const IMAGE_POLICY: &str = "root=signed+absent:usr=signed+absent";

struct Layer {
    name: String,
    kind: &'static str, // sysext | confext | unknown
    present: bool,
    decision: &'static str, // merged | omitted | refused | absent | error
    reason: &'static str,
}

fn env_or(key: &str, default: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| default.to_string())
}

fn join(s: &BTreeSet<String>) -> String {
    s.iter().cloned().collect::<Vec<_>>().join(", ")
}

fn main() {
    match std::env::args().nth(1).as_deref() {
        Some("merge") => do_merge(),
        _ => {
            eprintln!("oniond — Shrek Onion layer policy (Phase 4).");
            eprintln!("usage: oniond merge   (select enabled signed layers from the store, then merge)");
            std::process::exit(2);
        }
    }
}

/// Parse the sealed enable-list: lines `enable <name>`; `#` comments and blanks ignored.
fn read_policy(path: &str) -> BTreeSet<String> {
    let mut set = BTreeSet::new();
    let text = match fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("oniond: WARN cannot read policy {path}: {e} — no layers enabled");
            return set;
        }
    };
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut parts = line.split_whitespace();
        match parts.next() {
            Some("enable") => {
                if let Some(name) = parts.next() {
                    set.insert(name.to_string());
                }
            }
            Some(other) => eprintln!("oniond: WARN unknown policy directive '{other}' in {path}"),
            None => {}
        }
    }
    set
}

/// Scan a store subdir for `*.raw` DDIs → map of layer-name (basename sans `.raw`) → path.
fn scan_ddis(dir: &str) -> BTreeMap<String, String> {
    let mut m = BTreeMap::new();
    let rd = match fs::read_dir(dir) {
        Ok(r) => r,
        Err(_) => return m, // missing subtree (e.g. unsigned/tamper stores ship no confexts/) is fine
    };
    for ent in rd.flatten() {
        let p = ent.path();
        if let Some(fname) = p.file_name().and_then(|s| s.to_str()) {
            if let Some(name) = fname.strip_suffix(".raw") {
                m.insert(name.to_string(), p.to_string_lossy().into_owned());
            }
        }
    }
    m
}

/// Selection lever: expose ONLY the enabled DDIs to a clean tmpfs search dir via symlinks.
/// systemd-sysext/-confext then merge exactly what is visible; anything not enabled is never
/// considered. Returns the set actually exposed.
fn expose_enabled(
    search_dir: &str,
    store: &BTreeMap<String, String>,
    enabled: &BTreeSet<String>,
) -> BTreeSet<String> {
    let _ = fs::create_dir_all(search_dir);
    // Remove any stale `*.raw` symlinks from a prior run (a fresh /run makes this a no-op at boot).
    if let Ok(rd) = fs::read_dir(search_dir) {
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
    let mut exposed = BTreeSet::new();
    for (name, path) in store {
        if enabled.contains(name) {
            let link = format!("{search_dir}/{name}.raw");
            match symlink(path, &link) {
                Ok(()) => {
                    exposed.insert(name.clone());
                }
                Err(e) => eprintln!("oniond: WARN symlink {link} -> {path}: {e}"),
            }
        }
    }
    exposed
}

/// Drive the low-level merge under the fixed trust gate. Returns the tool's exit code (-1 on spawn
/// failure). The tool checks each considered DDI's Verity/signature per `--image-policy`.
fn run_merge(bin: &str) -> i32 {
    match Command::new(bin)
        .arg(format!("--image-policy={IMAGE_POLICY}"))
        .arg("merge")
        .status()
    {
        Ok(s) => s.code().unwrap_or(-1),
        Err(e) => {
            eprintln!("oniond: WARN failed to run {bin}: {e}");
            -1
        }
    }
}

/// Coarse per-layer attribution (spike limitation, documented): decision follows the merge exit
/// code. With a single merge-eligible layer per tool — every spike gate — this is exact. With
/// multiple enabled layers of one kind and a non-zero rc, all are marked refused (systemd 257 fails
/// the whole merge rather than skip-and-continue); per-layer `status --json` parsing is a later slice.
fn decide(enabled: bool, exposed: bool, merge_rc: i32) -> (&'static str, &'static str) {
    if !enabled {
        return ("omitted", "not-enabled");
    }
    if !exposed {
        return ("error", "not-exposed"); // enabled + present but the symlink failed
    }
    if merge_rc == 0 {
        ("merged", "")
    } else {
        ("refused", "image-policy")
    }
}

fn do_merge() {
    let policy_path = env_or("SHREK_ONION_POLICY", "/usr/lib/shrek/onion-policy");
    let store = env_or("SHREK_ONION_STORE", "/run/shrek-store");
    let state_path = env_or("SHREK_ONION_STATE", "/run/shrek/onion.json");
    let sysext_dir = env_or("SHREK_SYSEXT_DIR", "/run/extensions");
    let confext_dir = env_or("SHREK_CONFEXT_DIR", "/run/confexts");

    let enabled = read_policy(&policy_path);
    println!("oniond: policy {policy_path} enables [{}]", join(&enabled));

    let sysext_store = scan_ddis(&format!("{store}/extensions"));
    let confext_store = scan_ddis(&format!("{store}/confexts"));

    let sysext_sel = expose_enabled(&sysext_dir, &sysext_store, &enabled);
    let confext_sel = expose_enabled(&confext_dir, &confext_store, &enabled);
    println!(
        "oniond: exposed sysext [{}] confext [{}] to the search dirs",
        join(&sysext_sel),
        join(&confext_sel)
    );

    let sysext_rc = run_merge("systemd-sysext");
    let confext_rc = run_merge("systemd-confext");

    let mut layers: Vec<Layer> = Vec::new();
    for name in sysext_store.keys() {
        let (decision, reason) = decide(enabled.contains(name), sysext_sel.contains(name), sysext_rc);
        layers.push(Layer { name: name.clone(), kind: "sysext", present: true, decision, reason });
    }
    for name in confext_store.keys() {
        let (decision, reason) = decide(enabled.contains(name), confext_sel.contains(name), confext_rc);
        layers.push(Layer { name: name.clone(), kind: "confext", present: true, decision, reason });
    }
    for name in &enabled {
        if !sysext_store.contains_key(name) && !confext_store.contains_key(name) {
            layers.push(Layer {
                name: name.clone(),
                kind: "unknown",
                present: false,
                decision: "absent",
                reason: "enabled-but-absent",
            });
        }
    }
    layers.sort_by(|a, b| a.name.cmp(&b.name));

    for l in &layers {
        if l.reason.is_empty() {
            println!("oniond: {} ({}) -> {}", l.name, l.kind, l.decision);
        } else {
            println!("oniond: {} ({}) -> {} ({})", l.name, l.kind, l.decision, l.reason);
        }
    }
    println!("oniond: sysext merge rc={sysext_rc} confext merge rc={confext_rc}");

    write_state(&state_path, &policy_path, &enabled, sysext_rc, confext_rc, &layers);
    println!("oniond: state written to {state_path}");

    // Human proof on the console (not parsed by the harness).
    let _ = Command::new("systemd-sysext").arg("status").status();

    // NEVER fail the boot — a refused/omitted layer is a survivable condition; the verdict is
    // observed, not fatal (the Phase-2 contract).
    std::process::exit(0);
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

/// Hand-written JSON — one layer object per line so shrekctl can read it back without a JSON parser.
fn write_state(
    path: &str,
    policy: &str,
    enabled: &BTreeSet<String>,
    sysext_rc: i32,
    confext_rc: i32,
    layers: &[Layer],
) {
    if let Some(parent) = Path::new(path).parent() {
        let _ = fs::create_dir_all(parent);
    }
    let mut s = String::new();
    s.push_str("{\n");
    s.push_str("  \"version\": 1,\n");
    s.push_str(&format!("  \"policy\": {},\n", json_str(policy)));
    s.push_str(&format!("  \"sysext_merge_rc\": {sysext_rc},\n"));
    s.push_str(&format!("  \"confext_merge_rc\": {confext_rc},\n"));
    let en: Vec<String> = enabled.iter().map(|e| json_str(e)).collect();
    s.push_str(&format!("  \"enabled\": [{}],\n", en.join(", ")));
    s.push_str("  \"layers\": [\n");
    for (i, l) in layers.iter().enumerate() {
        let comma = if i + 1 < layers.len() { "," } else { "" };
        s.push_str(&format!(
            "    {{ \"name\": {}, \"kind\": {}, \"present\": {}, \"decision\": {}, \"reason\": {} }}{}\n",
            json_str(&l.name),
            json_str(l.kind),
            l.present,
            json_str(l.decision),
            json_str(l.reason),
            comma
        ));
    }
    s.push_str("  ]\n}\n");
    if let Err(e) = fs::write(path, &s) {
        eprintln!("oniond: WARN cannot write state {path}: {e}");
    }
}
