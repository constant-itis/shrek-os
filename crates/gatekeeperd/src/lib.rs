//! gatekeeperd library surface.
//!
//! The privileged broker ships as the `gatekeeperd` binary (`main.rs`). The reusable, testable
//! internals live here so unit tests, the mount-plane TOCTOU proof (`examples/`), and the binary all
//! share one implementation. Nothing here pulls an external dependency — see `linux_uapi`.

pub mod linux_uapi;
pub mod mount_plane;
pub mod net_plane;
pub mod pin_manifest;
pub mod proc_plane;
pub mod provenance_plane;
pub mod sandbox;
pub mod t2_plane;
