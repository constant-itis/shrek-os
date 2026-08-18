//! agentd — agent identity + isolation resolver (Phase 8).
//!
//! Phase-1 scaffold: DISABLED. When implemented, agentd is the UNPRIVILEGED resolver: it maps
//! (trust, caps) -> tier via the matrix + floor and emits a sealed construction request for
//! gatekeeperd to verify and build (isolation.md §5, §7). It resolves; it never constructs a
//! sandbox and never holds privilege. Trust band must be integrity-sourced, unknown => T-hostile
//! (security-model.md §6). None of this exists yet.

fn main() {
    eprintln!("agentd: Phase-1 disabled scaffold — resolves nothing, holds no privilege");
    loop {
        std::thread::park();
    }
}
