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
//!   * S2d — the root supervisor daemon: bless/unbless/re-pin verbs over a `SO_PEERCRED`-gated,
//!     rate-limited, journaled uid-1000 socket.

pub mod apply;
pub mod dot;
pub mod store;
