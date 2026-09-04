//! egressd — the desktop-egress SUPERVISOR for the sealed desktop session (ADR-007 S2).
//!
//! S1 baked ONE static deny-by-default `shrek_desktop_egress` nft table with EMPTY named sets and
//! sealed the desktop-egress profile table into `shrek-policy`. This crate is the root supervisor that
//! turns a human's bless into a live allow WITHOUT ever rebuilding that table: it resolves a blessed
//! profile's sealed NAME over a baked DoT client to sealed resolver IPs (never `resolved`/NM/
//! `resolv.conf`/`getaddrinfo` — the `[R1-MF1]`+`[R2-MF-C]` bypass of uid-1000's name-resolution
//! authority, #3121's workaround), then ADDs the resolved IPs to the nft set as ELEMENTS only (never
//! `add rule`, never flush — an apply error leaves the baked deny skeleton exactly in place).
//!
//! Slices (this crate grows one module per slice; the ADR §11.2 decomposition):
//!   * S2a — [`store`]: the durable `/home/.shrek-system/egress` bless/pin state + the world-readable
//!     `/run/shrek/egress/pinned` projection. State only: no nft, no DoT, no socket. ← landed
//!   * S2b — [`apply`]: the element-only nft applier (`nft add/delete element` reconcile against the
//!     LIVE set + fail-closed rollback; the sole browser-cgroup rule insert). ← landed
//!   * S2c — [`dot`]: the sealed DNS-over-TLS re-pin client (sealed resolver IPs + sealed root bundle +
//!     exact expected hostname; never resolved/NM/resolv.conf/getaddrinfo). ← landed
//!   * S2d — [`supervisor`]: the root daemon — bless/unbless/re-pin over a `SO_PEERCRED`-gated,
//!     Tier-B-admitted, rate-limited, journaled uid-1000 socket; flush-free startup reconcile. Uses
//!     [`uapi`] for the one raw syscall (peer creds). ← landed
//!   * S3  — the bless UX backend: [`client`] (`egressd ask` — the unprivileged uid-1000 socket front
//!     door the DMS panel/onboarding exec) + [`store::project_state`] (the legible `/run/shrek/egress/
//!     state` read view). The supervisor's bless is now INTENT-FIRST (a resolve failure leaves the
//!     profile legibly "blessed, pin-deferred", not silently unblessed) and [`supervisor::reconcile`]
//!     re-resolves a blessed-but-pinless profile at boot (the first-run self-heal). ← this slice

pub mod apply;
pub mod client;
pub mod confirmed;
pub mod dot;
pub mod hosts;
pub mod store;
pub mod supervisor;
pub mod uapi;
