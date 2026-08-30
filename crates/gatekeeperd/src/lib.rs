//! gatekeeperd library surface.
//!
//! The privileged broker ships as the `gatekeeperd` binary (`main.rs`). The reusable, testable
//! internals live here so unit tests, the mount-plane TOCTOU proof (`examples/`), and the binary all
//! share one implementation. Nothing here pulls an external dependency — see `linux_uapi`.

pub mod authority_record;
pub mod bench_plane;
pub mod bench_record;
pub mod ingest_admit;
pub mod linux_uapi;
pub mod mount_plane;
pub mod net_binding;
pub mod net_plane;
pub mod pin_manifest;
pub mod proc_plane;
pub mod provenance_plane;
pub mod sandbox;
pub mod session_view;
pub mod t2_plane;
