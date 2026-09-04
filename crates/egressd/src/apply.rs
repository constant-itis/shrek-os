//! apply — the ELEMENT-ONLY nft applier for the baked desktop egress table (ADR-007 S2b).
//!
//! S1 baked ONE static `inet shrek_desktop_egress` table with every rule present and EMPTY named sets.
//! This module is the only thing that mutates it at runtime, and it does so under a hard constraint
//! (`[R2-MF-B]`): it ONLY ever `add element` / `delete element` on a named set, plus the single
//! sanctioned browser-cgroup rule insert (Q7). It NEVER `add rule` (other than that one), NEVER `add
//! table`, NEVER `flush`, NEVER `delete table`. So a mistake or a failed apply can only ever leave the
//! deny-by-default skeleton exactly in place — fail-closed by construction, not by cleanup.
//!
//! Two enforcement shapes:
//!   * PINNED profiles (`weather`): the supervisor resolves the sealed name (S2c) and this applier
//!     reconciles `@<profile>_pinned` to exactly the desired IPv4 set — diffing against the LIVE set
//!     (the real kernel truth), adding the new, deleting the stale. On any nft error it ROLLS BACK the
//!     elements it added this call, so a partial apply never half-opens an allow.
//!   * BROAD profile (`web-browsing`): unpinnable, so instead of a set element the supervisor inserts a
//!     cgroup-scoped accept pair ABOVE rule 0 (the stub-drop), matched on `shrek-browser.slice`. nft
//!     resolves the cgroup PATH to an id at load time, so this rule can only be inserted once the slice
//!     EXISTS (at browser launch) — its live matcher is validated in the sealed-VM dogfood, not a bare
//!     netns. Removed by handle on teardown. It is the SOLE runtime rule insert.
//!
//! Dep-free: shells `nft` via [`std::process::Command`] (the [`NftExec`] trait; a recording double in
//! tests asserts the exact element-only argv without root). Every command is built here as typed argv,
//! so "never flush / never add-rule" is auditable in one file.

use std::collections::BTreeSet;
use std::net::Ipv4Addr;
use std::path::Path;
use std::process::Command;

use crate::store;
use shrek_policy::egress::Proto;

/// The single baked table this applier mutates. Never created/flushed/deleted here — only its named
/// set elements (+ the one browser rule) are touched.
pub const TABLE: &str = "inet shrek_desktop_egress";

/// The baked concatenated set for the advanced RAW tier (S4). Type `ipv4_addr . inet_proto .
/// inet_service`, so ONE set encodes per-element (ip, proto, port) with ZERO runtime rules — the
/// element-only invariant `[R2-MF-B]` holds for raw exactly as for the pinned profiles. The single baked
/// rule `ip daddr . meta l4proto . th dport @raw_pinned accept` matches it.
pub const RAW_SET: &str = "raw_pinned";

/// Map a sealed profile to the baked nft set the applier pins into. `Some` ONLY for a uid-1000
/// user-blessed PINNABLE profile: NOT baseline (system-uid egress, declarative), NOT broad (cgroup-
/// scoped), NOT pre-pinned (`desktop-ntp` is sealed into `@ntp_pinned` at bake time and never touched).
/// Single-sourced with the baked table — adding a pinnable profile means baking its set AND a match arm
/// here. Fail-closed default `None` (the caller then refuses / records an unknown-profile fault, and NO
/// element is written).
pub fn set_name(profile: &str) -> Option<&'static str> {
    match profile {
        "weather" => Some("weather_pinned"),
        _ => None,
    }
}

// ---- typed nft commands -------------------------------------------------------------------------

/// One `nft` invocation's argv (without the leading `nft`). Kept typed so the whole command surface is
/// auditable and unit-testable — every mutating command this crate can emit is built by a fn below.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NftCmd(pub Vec<String>);

impl NftCmd {
    fn of(parts: &[&str]) -> Self {
        NftCmd(parts.iter().map(|s| s.to_string()).collect())
    }
    /// True iff this command only reads (`list`) — used to assert a dry run performs no mutation.
    pub fn is_read_only(&self) -> bool {
        matches!(self.0.first().map(String::as_str), Some("list") | Some("-a"))
    }
}

fn table_parts() -> Vec<&'static str> {
    // "inet shrek_desktop_egress" as two argv tokens (nft joins argv with spaces before lexing).
    TABLE.split(' ').collect()
}

/// `nft add element inet shrek_desktop_egress <set> { <addr> }` — one address per command (idempotent;
/// trivial rollback). NEVER a rule.
pub fn add_element(set: &str, addr: Ipv4Addr) -> NftCmd {
    let addr = addr.to_string();
    let mut v = vec!["add", "element"];
    v.extend(table_parts());
    v.push(set);
    v.push("{");
    v.push(&addr);
    v.push("}");
    NftCmd::of(&v)
}

/// `nft delete element inet shrek_desktop_egress <set> { <addr> }`.
pub fn del_element(set: &str, addr: Ipv4Addr) -> NftCmd {
    let addr = addr.to_string();
    let mut v = vec!["delete", "element"];
    v.extend(table_parts());
    v.push(set);
    v.push("{");
    v.push(&addr);
    v.push("}");
    NftCmd::of(&v)
}

/// `nft list set inet shrek_desktop_egress <set>` — read the LIVE elements to reconcile against.
pub fn list_set(set: &str) -> NftCmd {
    let mut v = vec!["list", "set"];
    v.extend(table_parts());
    v.push(set);
    NftCmd::of(&v)
}

/// One concatenated `@raw_pinned` element: `{ <ip> . <proto> . <port> }` (S4). nft joins argv with
/// spaces, so the `.` concat separators are their own tokens. Idempotent; one element per command.
pub fn add_raw_element(addr: Ipv4Addr, proto: Proto, port: u16) -> NftCmd {
    raw_element_cmd("add", addr, proto, port)
}

/// `nft delete element inet shrek_desktop_egress raw_pinned { <ip> . <proto> . <port> }`.
pub fn del_raw_element(addr: Ipv4Addr, proto: Proto, port: u16) -> NftCmd {
    raw_element_cmd("delete", addr, proto, port)
}

fn raw_element_cmd(op: &str, addr: Ipv4Addr, proto: Proto, port: u16) -> NftCmd {
    let addr = addr.to_string();
    let proto = proto.label().to_string();
    let port = port.to_string();
    let mut v = vec![op, "element"];
    v.extend(table_parts());
    v.push(RAW_SET);
    v.extend(["{", &addr, ".", &proto, ".", &port, "}"]);
    NftCmd::of(&v)
}

/// `nft -a list chain inet shrek_desktop_egress output` — read rules WITH handles (to find rule 0's
/// handle for the browser insert, and to find the browser rules' handles for teardown).
pub fn list_chain() -> NftCmd {
    let mut v = vec!["-a", "list", "chain"];
    v.extend(table_parts());
    v.push("output");
    NftCmd::of(&v)
}

/// The cgroup-scoped browser rule pair, inserted ABOVE rule 0 by handle (`[Q7]`). `path` is the
/// `shrek-browser.slice` cgroupv2 path as it exists at launch; `level` is its ancestor level. Two rules:
///   1. stub-ACCEPT — lets the browser cgroup reach the resolved stubs (so it resolves names normally),
///      inserted above rule 0's stub-DROP so it wins for browser-scoped traffic only.
///   2. broad ACCEPT — the browser cgroup reaches arbitrary hosts (web-browsing is unpinnable).
/// Both are `insert ... handle <rule0>`, so both land above the stub-drop; nft's `insert` places each
/// immediately before the handle, so pass them in the order you want them to end up (stub-accept last
/// inserted ⇒ topmost is fine either way since both precede rule 0). Live cgroup-path validation
/// requires the slice to exist (S6 dogfood), so these are argv-tested here, not netns-loaded.
pub fn browser_stub_accept(rule0_handle: u32, path: &str, level: u32) -> NftCmd {
    let h = rule0_handle.to_string();
    let lvl = level.to_string();
    let mut v = vec!["insert", "rule"];
    v.extend(table_parts());
    v.extend(["output", "handle", &h]);
    v.extend(["meta", "skuid", "1000", "socket", "cgroupv2", "level", &lvl, path]);
    v.extend(["ip", "daddr", "{", "127.0.0.53,", "127.0.0.54", "}", "th", "dport", "53", "accept"]);
    NftCmd::of(&v)
}

pub fn browser_broad_accept(rule0_handle: u32, path: &str, level: u32) -> NftCmd {
    let h = rule0_handle.to_string();
    let lvl = level.to_string();
    let mut v = vec!["insert", "rule"];
    v.extend(table_parts());
    v.extend(["output", "handle", &h]);
    v.extend(["meta", "skuid", "1000", "socket", "cgroupv2", "level", &lvl, path, "accept"]);
    NftCmd::of(&v)
}

/// `nft delete rule inet shrek_desktop_egress output handle <h>` — remove a browser rule on teardown.
pub fn delete_rule(handle: u32) -> NftCmd {
    let h = handle.to_string();
    let mut v = vec!["delete", "rule"];
    v.extend(table_parts());
    v.extend(["output", "handle", &h]);
    NftCmd::of(&v)
}

// ---- parsers ------------------------------------------------------------------------------------

/// Parse the IPv4 elements out of `nft list set …` output. Tolerant of single- or multi-line
/// `elements = { a, b }`. Non-IPv4 tokens are ignored (the set is typed `ipv4_addr`, so nothing else
/// should appear; ignoring is safe and keeps the diff conservative).
pub fn parse_set_elements(listing: &str) -> BTreeSet<Ipv4Addr> {
    let mut out = BTreeSet::new();
    if let Some(open) = listing.find("elements") {
        let tail = &listing[open..];
        if let (Some(lb), Some(rb)) = (tail.find('{'), tail.find('}')) {
            if lb < rb {
                for tok in tail[lb + 1..rb].split([',', ' ', '\n', '\t']) {
                    if let Ok(a) = tok.trim().parse::<Ipv4Addr>() {
                        out.insert(a);
                    }
                }
            }
        }
    }
    out
}

/// Parse the concatenated `(ip, proto, port)` elements out of `nft list set … raw_pinned` output. nft
/// prints a concat element as `1.2.3.4 . tcp . 8443` — the separator is ` . ` (space-dot-space), NOT a
/// bare `.` (an IPv4 literal contains dots), so we split each element on " . ". Malformed tuples are
/// ignored (conservative — keeps the diff from spuriously deleting a well-formed live element).
pub fn parse_raw_set_elements(listing: &str) -> BTreeSet<(Ipv4Addr, Proto, u16)> {
    let mut out = BTreeSet::new();
    let Some(open) = listing.find("elements") else { return out };
    let tail = &listing[open..];
    let (Some(lb), Some(rb)) = (tail.find('{'), tail.find('}')) else { return out };
    if lb >= rb {
        return out;
    }
    for elem in tail[lb + 1..rb].split(',') {
        let parts: Vec<&str> = elem.trim().split(" . ").map(|s| s.trim()).collect();
        if parts.len() != 3 {
            continue;
        }
        let (Ok(ip), proto, Ok(port)) = (
            parts[0].parse::<Ipv4Addr>(),
            match parts[1] {
                "tcp" => Some(Proto::Tcp),
                "udp" => Some(Proto::Udp),
                _ => None,
            },
            parts[2].parse::<u16>(),
        ) else {
            continue;
        };
        if let Some(proto) = proto {
            out.insert((ip, proto, port));
        }
    }
    out
}

/// Find every browser-cgroup rule handle (the `socket cgroupv2 … shrek-browser.slice` accepts inserted
/// above rule 0) so teardown (`confirmed-unbless web-browsing`) can `delete rule` each by handle.
pub fn parse_browser_handles(chain_listing: &str) -> Vec<u32> {
    let mut out = Vec::new();
    for line in chain_listing.lines() {
        if line.contains("cgroupv2") && line.contains("shrek-browser.slice") {
            if let Some(idx) = line.find("# handle ") {
                if let Ok(h) = line[idx + "# handle ".len()..].trim().parse::<u32>() {
                    out.push(h);
                }
            }
        }
    }
    out
}

/// Find rule 0's handle: the `meta skuid 1000 … th dport 53 drop` stub-drop line's `# handle N`. The
/// browser rules insert ABOVE this. `None` if absent (the applier then refuses the browser insert —
/// without the anchor it will not blindly insert at the top).
pub fn parse_rule0_handle(chain_listing: &str) -> Option<u32> {
    for line in chain_listing.lines() {
        if line.contains("skuid 1000")
            && line.contains("th dport 53 drop")
            && line.contains("127.0.0.53")
        {
            if let Some(idx) = line.find("# handle ") {
                return line[idx + "# handle ".len()..].trim().parse().ok();
            }
        }
    }
    None
}

// ---- executor -----------------------------------------------------------------------------------

/// Runs an [`NftCmd`]. Abstracted so tests drive a recording double (assert exact argv, script
/// failures) without root, and the sealed-VM/oracle uses the real `nft`.
pub trait NftExec {
    /// Run a command. `Ok(stdout)` on exit 0; `Err(message)` on nonzero/spawn failure.
    fn run(&mut self, cmd: &NftCmd) -> Result<String, String>;
}

/// Production executor: shells the sealed `/usr/sbin/nft`.
pub struct ShellNft;

impl NftExec for ShellNft {
    fn run(&mut self, cmd: &NftCmd) -> Result<String, String> {
        let out = Command::new("nft")
            .args(&cmd.0)
            .output()
            .map_err(|e| format!("spawn nft: {e}"))?;
        if out.status.success() {
            Ok(String::from_utf8_lossy(&out.stdout).into_owned())
        } else {
            Err(format!(
                "nft {:?} exit {:?}: {}",
                cmd.0,
                out.status.code(),
                String::from_utf8_lossy(&out.stderr).trim()
            ))
        }
    }
}

/// Why an apply did not (fully) take effect. The caller maps these to a [`store::FaultKind`] and parks
/// a fault — NO element is left half-applied for [`ApplyError::Nft`] on the add path (rolled back).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ApplyError {
    /// Profile is not a pinnable set-managed profile (unknown / baseline / broad / pre-pinned). The
    /// caller records `unknown-profile` and installs NO element.
    Unmanaged(String),
    /// An `nft` command failed. The add path rolled back; the deny skeleton stands.
    Nft(String),
}

// ---- orchestration ------------------------------------------------------------------------------

/// Reconcile `@<profile>_pinned` to exactly `desired`, element-only, diffing against the LIVE set.
///
/// Fail-closed guarantees:
///   * an UNMANAGED profile ⇒ `Err(Unmanaged)`, ZERO nft commands run (never touches the table);
///   * on an nft error while ADDING, every element added THIS call is deleted (rollback) before
///     returning `Err(Nft)`, so a bless can never half-open;
///   * the `.applied` audit marker is written ONLY after a clean reconcile.
///
/// Returns the addresses actually present after the call (for the caller's log / projection).
pub fn apply_pins(
    store_dir: &Path,
    exec: &mut dyn NftExec,
    profile: &str,
    desired: &[Ipv4Addr],
) -> Result<Vec<Ipv4Addr>, ApplyError> {
    let set = set_name(profile).ok_or_else(|| ApplyError::Unmanaged(profile.to_string()))?;

    let want: BTreeSet<Ipv4Addr> = desired.iter().copied().collect();
    let live = parse_set_elements(&exec.run(&list_set(set)).map_err(ApplyError::Nft)?);

    let to_add: Vec<Ipv4Addr> = want.difference(&live).copied().collect();
    let to_del: Vec<Ipv4Addr> = live.difference(&want).copied().collect();

    // Add first (open the new pins), rolling back on any failure so we never half-open.
    let mut added: Vec<Ipv4Addr> = Vec::new();
    for addr in &to_add {
        if let Err(e) = exec.run(&add_element(set, *addr)) {
            for done in &added {
                let _ = exec.run(&del_element(set, *done)); // best-effort rollback to prior state
            }
            return Err(ApplyError::Nft(format!("add {addr}: {e}")));
        }
        added.push(*addr);
    }
    // Then delete the stale pins. A delete failure leaves a (bounded, previously-blessed) stale allow;
    // report it but do NOT roll back the adds (those are the desired, correct state).
    for addr in &to_del {
        if let Err(e) = exec.run(&del_element(set, *addr)) {
            return Err(ApplyError::Nft(format!("delete {addr}: {e}")));
        }
    }

    let final_set: Vec<Ipv4Addr> = want.iter().copied().collect();
    let _ = store::write_applied(store_dir, profile, &final_set);
    Ok(final_set)
}

/// Drop a profile's pins entirely (unbless): reconcile the set to empty. Element-only.
pub fn unapply(store_dir: &Path, exec: &mut dyn NftExec, profile: &str) -> Result<(), ApplyError> {
    apply_pins(store_dir, exec, profile, &[])?;
    let _ = store::clear_applied(store_dir, profile);
    Ok(())
}

/// Install the browser-cgroup rule pair above rule 0 (at browser launch). Looks up rule 0's handle
/// live; refuses (no insert) if the anchor is absent. Returns the two commands' order for teardown by
/// handle later. NOTE: live cgroup-path validity is the kernel's to check at insert time (the slice
/// must exist) — see module docs.
pub fn install_browser_rules(
    exec: &mut dyn NftExec,
    cgroup_path: &str,
    level: u32,
) -> Result<(), ApplyError> {
    let listing = exec.run(&list_chain()).map_err(ApplyError::Nft)?;
    let handle = parse_rule0_handle(&listing)
        .ok_or_else(|| ApplyError::Nft("rule-0 stub-drop anchor not found; refusing insert".into()))?;
    exec.run(&browser_broad_accept(handle, cgroup_path, level))
        .map_err(ApplyError::Nft)?;
    exec.run(&browser_stub_accept(handle, cgroup_path, level))
        .map_err(ApplyError::Nft)?;
    Ok(())
}

/// Remove ALL browser-cgroup rules currently in the chain (web-browsing teardown / `confirmed-unbless
/// web-browsing`). Idempotent: zero handles ⇒ zero commands ⇒ Ok. Deletes by handle so it removes both
/// the stub-accept and broad-accept of the pair (MF-5: the panel must not read "Disabled" while the
/// browser keeps broad egress). A delete error surfaces so the caller parks a fault + keeps the record.
pub fn uninstall_browser_rules(exec: &mut dyn NftExec) -> Result<usize, ApplyError> {
    let listing = exec.run(&list_chain()).map_err(ApplyError::Nft)?;
    let handles = parse_browser_handles(&listing);
    let mut removed = 0usize;
    for h in handles {
        exec.run(&delete_rule(h)).map_err(ApplyError::Nft)?;
        removed += 1;
    }
    Ok(removed)
}

/// Reconcile the concatenated `@raw_pinned` set to EXACTLY `desired` (the UNION over all blessed raw
/// entries of their resolved `(ip, proto, port)` tuples — MF-5: computed as a whole set, so removing one
/// raw destination can never drop another's element when the two share a tuple). Element-only, diffing
/// against the LIVE set, with add-rollback on any nft error — identical fail-closed discipline to
/// [`apply_pins`]. Returns the tuples present after the call.
pub fn apply_raw(
    exec: &mut dyn NftExec,
    desired: &[(Ipv4Addr, Proto, u16)],
) -> Result<Vec<(Ipv4Addr, Proto, u16)>, ApplyError> {
    let want: BTreeSet<(Ipv4Addr, Proto, u16)> = desired.iter().copied().collect();
    let live = parse_raw_set_elements(&exec.run(&list_set(RAW_SET)).map_err(ApplyError::Nft)?);

    let to_add: Vec<_> = want.difference(&live).copied().collect();
    let to_del: Vec<_> = live.difference(&want).copied().collect();

    let mut added: Vec<(Ipv4Addr, Proto, u16)> = Vec::new();
    for &(ip, proto, port) in &to_add {
        if let Err(e) = exec.run(&add_raw_element(ip, proto, port)) {
            for &(a, p, pt) in &added {
                let _ = exec.run(&del_raw_element(a, p, pt)); // best-effort rollback
            }
            return Err(ApplyError::Nft(format!("add raw {ip}.{}.{port}: {e}", proto.label())));
        }
        added.push((ip, proto, port));
    }
    for &(ip, proto, port) in &to_del {
        if let Err(e) = exec.run(&del_raw_element(ip, proto, port)) {
            return Err(ApplyError::Nft(format!("delete raw {ip}.{}.{port}: {e}", proto.label())));
        }
    }
    Ok(want.into_iter().collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Recording executor: canned `list`/`list set`/`list chain` replies, records every command, and
    /// can be scripted to fail the Nth mutating command (to exercise rollback).
    struct Rec {
        live: String,        // reply to `list set`
        chain: String,       // reply to `list chain`
        cmds: Vec<NftCmd>,   // everything run, in order
        fail_on: Option<usize>, // 1-based index of the mutating command to fail
        mut_count: usize,
    }
    impl Rec {
        fn new() -> Self {
            Rec { live: String::new(), chain: String::new(), cmds: vec![], fail_on: None, mut_count: 0 }
        }
        fn mutations(&self) -> Vec<&NftCmd> {
            self.cmds.iter().filter(|c| !c.is_read_only()).collect()
        }
    }
    impl NftExec for Rec {
        fn run(&mut self, cmd: &NftCmd) -> Result<String, String> {
            self.cmds.push(cmd.clone());
            let head = cmd.0.first().map(String::as_str);
            if head == Some("-a") {
                return Ok(self.chain.clone());
            }
            if cmd.0.first().map(String::as_str) == Some("list") {
                return Ok(self.live.clone());
            }
            // a mutation
            self.mut_count += 1;
            if Some(self.mut_count) == self.fail_on {
                return Err(format!("scripted failure on mutation {}", self.mut_count));
            }
            Ok(String::new())
        }
    }

    fn store_tmp() -> std::path::PathBuf {
        use std::sync::atomic::{AtomicU64, Ordering};
        static N: AtomicU64 = AtomicU64::new(0);
        let base = std::env::var("CARGO_TARGET_TMPDIR").unwrap_or_else(|_| "/tmp".into());
        let d = std::path::PathBuf::from(base)
            .join(format!("apply-{}-{}", std::process::id(), N.fetch_add(1, Ordering::Relaxed)));
        let _ = std::fs::remove_dir_all(&d);
        store::ensure_store(&d).unwrap();
        d
    }

    #[test]
    fn unmanaged_profile_runs_zero_commands() {
        let d = store_tmp();
        let mut rec = Rec::new();
        for p in ["web-browsing", "desktop-ntp", "desktop-updates", "evil"] {
            let e = apply_pins(&d, &mut rec, p, &[Ipv4Addr::new(1, 2, 3, 4)]).unwrap_err();
            assert!(matches!(e, ApplyError::Unmanaged(_)), "{p} should be unmanaged");
        }
        assert!(rec.cmds.is_empty(), "an unmanaged profile must not touch nft at all");
    }

    #[test]
    fn bless_from_empty_adds_only_the_desired_elements() {
        let d = store_tmp();
        let mut rec = Rec::new(); // empty live set
        let a = Ipv4Addr::new(104, 16, 1, 1);
        let b = Ipv4Addr::new(104, 16, 2, 2);
        let got = apply_pins(&d, &mut rec, "weather", &[a, b]).unwrap();
        assert_eq!(got, vec![a, b]);
        let muts = rec.mutations();
        assert_eq!(muts.len(), 2);
        assert_eq!(*muts[0], add_element("weather_pinned", a));
        assert_eq!(*muts[1], add_element("weather_pinned", b));
        // never a rule / flush / table op
        for c in &rec.cmds {
            assert!(!c.0.iter().any(|t| t == "flush" || t == "table" || t == "rule"));
        }
        // audit marker reflects the applied set
        assert_eq!(store::load_applied(&d, "weather"), vec![a, b]);
    }

    #[test]
    fn reconcile_adds_new_and_deletes_stale() {
        let d = store_tmp();
        let mut rec = Rec::new();
        rec.live = "set weather_pinned {\n type ipv4_addr\n elements = { 104.16.1.1, 9.9.9.9 }\n}".into();
        let a = Ipv4Addr::new(104, 16, 1, 1); // keep
        let c = Ipv4Addr::new(104, 16, 3, 3); // add
        // desired = {a, c}; live = {a, 9.9.9.9} ⇒ add c, delete 9.9.9.9
        apply_pins(&d, &mut rec, "weather", &[a, c]).unwrap();
        let muts = rec.mutations();
        assert!(muts.contains(&&add_element("weather_pinned", c)));
        assert!(muts.contains(&&del_element("weather_pinned", Ipv4Addr::new(9, 9, 9, 9))));
        assert!(!muts.contains(&&add_element("weather_pinned", a)), "a already live, no re-add");
    }

    #[test]
    fn add_failure_rolls_back_and_reports_nft() {
        let d = store_tmp();
        let mut rec = Rec::new();
        rec.fail_on = Some(2); // fail the SECOND add
        let a = Ipv4Addr::new(104, 16, 1, 1);
        let b = Ipv4Addr::new(104, 16, 2, 2);
        let e = apply_pins(&d, &mut rec, "weather", &[a, b]).unwrap_err();
        assert!(matches!(e, ApplyError::Nft(_)));
        let muts = rec.mutations();
        // add a (ok), add b (fail) ⇒ rollback delete a. Net: no element remains ⇒ fail-closed.
        assert_eq!(*muts[0], add_element("weather_pinned", a));
        assert_eq!(*muts[1], add_element("weather_pinned", b));
        assert_eq!(*muts[2], del_element("weather_pinned", a));
        // marker NOT written on failure
        assert_eq!(store::load_applied(&d, "weather"), Vec::<Ipv4Addr>::new());
    }

    #[test]
    fn parse_set_elements_multiline() {
        let s = "table inet shrek_desktop_egress {\n set weather_pinned {\n type ipv4_addr\n elements = { 104.16.1.1,\n 104.16.2.2 }\n }\n}";
        let got = parse_set_elements(s);
        assert!(got.contains(&Ipv4Addr::new(104, 16, 1, 1)));
        assert!(got.contains(&Ipv4Addr::new(104, 16, 2, 2)));
        assert_eq!(got.len(), 2);
        assert!(parse_set_elements("no elements here").is_empty());
    }

    #[test]
    fn parse_rule0_handle_from_real_listing() {
        // shape from the live netns probe
        let listing = "\tchain output { # handle 1\n\t\tmeta skuid 1000 ip daddr { 127.0.0.53, 127.0.0.54 } th dport 53 drop # handle 6\n\t\tmeta skuid 1000 oif \"lo\" accept # handle 7\n\t\tmeta skuid 1000 drop # handle 9\n\t}";
        assert_eq!(parse_rule0_handle(listing), Some(6));
        assert_eq!(parse_rule0_handle("no stub drop here"), None);
    }

    #[test]
    fn browser_rules_insert_above_rule0_by_handle_never_flush() {
        let mut rec = Rec::new();
        rec.chain = "meta skuid 1000 ip daddr { 127.0.0.53, 127.0.0.54 } th dport 53 drop # handle 6".into();
        install_browser_rules(&mut rec, "user.slice/user-1000.slice/shrek-browser.slice", 2).unwrap();
        let muts = rec.mutations();
        assert_eq!(muts.len(), 2);
        for c in &muts {
            assert_eq!(c.0[0], "insert");
            assert_eq!(c.0[1], "rule");
            assert!(c.0.contains(&"handle".to_string()) && c.0.contains(&"6".to_string()));
            assert!(c.0.contains(&"cgroupv2".to_string()));
            assert!(!c.0.iter().any(|t| t == "flush"));
        }
    }

    #[test]
    fn browser_insert_refuses_without_anchor() {
        let mut rec = Rec::new();
        rec.chain = "no rule-0 here".into();
        let e = install_browser_rules(&mut rec, "x/shrek-browser.slice", 2).unwrap_err();
        assert!(matches!(e, ApplyError::Nft(_)));
        assert!(rec.mutations().is_empty(), "no anchor ⇒ no rule inserted");
    }

    // ---- S4 raw concat set ----------------------------------------------------------------------

    #[test]
    fn raw_element_cmd_is_a_concat_add_element_never_a_rule() {
        let c = add_raw_element("203.0.113.7".parse().unwrap(), Proto::Tcp, 8443);
        assert_eq!(c.0[0], "add");
        assert_eq!(c.0[1], "element");
        assert!(c.0.contains(&"raw_pinned".to_string()));
        // the concat tokens `{ 203.0.113.7 . tcp . 8443 }`
        let joined = c.0.join(" ");
        assert!(joined.contains("{ 203.0.113.7 . tcp . 8443 }"), "{joined}");
        assert!(!c.0.iter().any(|t| t == "rule" || t == "flush"), "never a rule/flush");
        // delete mirror + udp/port variety.
        let d = del_raw_element("198.51.100.9".parse().unwrap(), Proto::Udp, 51820);
        assert_eq!(d.0[0], "delete");
        assert!(d.0.join(" ").contains("{ 198.51.100.9 . udp . 51820 }"));
    }

    #[test]
    fn parse_raw_set_elements_splits_on_space_dot_space_not_ip_dots() {
        // nft prints a concat set with ` . ` separators; the IP's own dots must NOT confuse the parse.
        let listing = "table inet shrek_desktop_egress {\n  set raw_pinned {\n    type ipv4_addr . inet_proto . inet_service\n    elements = { 203.0.113.7 . tcp . 8443, 198.51.100.9 . udp . 51820 }\n  }\n}";
        let got = parse_raw_set_elements(listing);
        assert!(got.contains(&("203.0.113.7".parse().unwrap(), Proto::Tcp, 8443)));
        assert!(got.contains(&("198.51.100.9".parse().unwrap(), Proto::Udp, 51820)));
        assert_eq!(got.len(), 2);
        // empty set ⇒ empty (inert, fail-closed), never a wildcard.
        assert!(parse_raw_set_elements("set raw_pinned { type ipv4_addr . inet_proto . inet_service }").is_empty());
    }

    #[test]
    fn apply_raw_reconciles_union_and_rolls_back_on_error() {
        // Live set has one stale tuple; desired swaps it for two — add both, delete the stale, element-only.
        let mut rec = Rec::new();
        rec.live = "elements = { 10.0.0.9 . tcp . 443 }".into();
        let desired = vec![
            ("203.0.113.7".parse().unwrap(), Proto::Tcp, 8443),
            ("198.51.100.9".parse().unwrap(), Proto::Udp, 53),
        ];
        let present = apply_raw(&mut rec, &desired).unwrap();
        assert_eq!(present.len(), 2);
        let muts = rec.mutations();
        // 2 adds + 1 delete, no rule/flush anywhere.
        assert_eq!(muts.iter().filter(|c| c.0[0] == "add").count(), 2);
        assert_eq!(muts.iter().filter(|c| c.0[0] == "delete").count(), 1);
        assert!(!muts.iter().any(|c| c.0.iter().any(|t| t == "rule" || t == "flush")));

        // On an add failure, every add THIS call is rolled back (never half-open).
        let mut rec2 = Rec::new();
        rec2.live = String::new();
        rec2.fail_on = Some(2); // fail the 2nd add
        let e = apply_raw(&mut rec2, &desired).unwrap_err();
        assert!(matches!(e, ApplyError::Nft(_)));
        let adds = rec2.mutations().iter().filter(|c| c.0[0] == "add").count();
        let dels = rec2.mutations().iter().filter(|c| c.0[0] == "delete").count();
        assert_eq!(adds, 2, "attempted both adds");
        assert_eq!(dels, 1, "rolled back the one successful add");
    }

    #[test]
    fn parse_browser_handles_and_uninstall_by_handle() {
        let listing = "\tchain output { # handle 1\n\t\tmeta skuid 1000 socket cgroupv2 level 2 \"user.slice/user-1000.slice/shrek-browser.slice\" accept # handle 11\n\t\tmeta skuid 1000 socket cgroupv2 level 2 \"user.slice/user-1000.slice/shrek-browser.slice\" ip daddr { 127.0.0.53, 127.0.0.54 } th dport 53 accept # handle 12\n\t\tmeta skuid 1000 ip daddr { 127.0.0.53, 127.0.0.54 } th dport 53 drop # handle 6\n\t}";
        assert_eq!(parse_browser_handles(listing), vec![11, 12]);
        let mut rec = Rec::new();
        rec.chain = listing.into();
        let removed = uninstall_browser_rules(&mut rec).unwrap();
        assert_eq!(removed, 2);
        let muts = rec.mutations();
        assert!(muts.iter().all(|c| c.0[0] == "delete" && c.0.contains(&"rule".to_string())));
        // idempotent: no browser rules ⇒ zero deletes.
        let mut empty = Rec::new();
        empty.chain = "meta skuid 1000 drop # handle 6".into();
        assert_eq!(uninstall_browser_rules(&mut empty).unwrap(), 0);
    }
}
