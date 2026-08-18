//! swampd — filesystem-intelligence daemon (Phase 6+).
//!
//! Phase-1 scaffold: DISABLED. It indexes nothing, opens no sockets, holds no privilege.
//! When implemented, swampd is a *subject* of the wall — Landlocked DEFAULT-DENY to an explicit
//! allow-set so protected bytes never enter its address space (architecture.md §5,
//! security-model.md §5). Nothing of that exists yet; this is a placeholder so the shape is real
//! and the critical-failure test (architecture.md §9) is meaningful: stopping it changes nothing.

fn main() {
    eprintln!("swampd: Phase-1 disabled scaffold — no indexing, no sockets, no privilege");
    // A real daemon stays resident; there is nothing to do yet.
    loop {
        std::thread::park();
    }
}
