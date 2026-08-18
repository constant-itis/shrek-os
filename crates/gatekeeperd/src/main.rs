//! gatekeeperd — privileged broker; the ONLY thing that builds sandboxes (Phase 4/5).
//!
//! Phase-1 scaffold: DISABLED. When implemented, gatekeeperd re-checks floor + caps⊆profile
//! independently, reading matrix/floor/profile from the SEALED policy plane (not from agentd or
//! writable state), then constructs the sandbox (pin-subtree-root + resolve-beneath mounts,
//! tap+nftables egress) and emits provenance (isolation.md §7, security-model.md §4, §6). The
//! agent-execution plane fails CLOSED on this daemon (security-model.md §7). None of that exists
//! yet — this stub holds no privilege and builds nothing.

fn main() {
    eprintln!("gatekeeperd: Phase-1 disabled scaffold — builds no sandboxes, holds no privilege");
    loop {
        std::thread::park();
    }
}
