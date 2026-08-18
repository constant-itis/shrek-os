//! oniond — sysext layer policy/orchestration (Phase 2/4).
//!
//! Phase-1 scaffold: DISABLED. oniond implements NO layering (systemd-sysext/dm-verity/mount do
//! the low-level work); it is the policy layer that decides which signed layers belong, whether
//! they are compatible/trusted, and which layer to roll back on a bad boot (architecture.md §3).
//! Nothing of that exists yet.

fn main() {
    eprintln!("oniond: Phase-1 disabled scaffold — orchestrates no layers");
    loop {
        std::thread::park();
    }
}
