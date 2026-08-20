//! net_plane — the per-workload egress plane. Phase-5 slice-3 (checkpoint #2563, oracle-proven in
//! scripts/egress-plane-repro.sh, scenario C4).
//!
//! `construct()` gives EVERY sandbox `--private-network`, so nspawn OWNS a fresh netns with only
//! loopback — a no-net cell reaches nothing, and the historical host-netns hole is closed. A C-net
//! cell then gets egress by the CNI-style model this module implements: gatekeeperd (privileged,
//! parent of the nspawn) discovers the container's netns leader, injects a veth + addressing + a
//! SEALED-profile-derived nftables allow-list INTO that netns, and only THEN releases the workload
//! past a ready-barrier. The invariant chain (isolation.md, security-model.md §7):
//!
//!   * rules-before-usable — the veth carries no traffic until the default-deny + allow rules are
//!     installed; the workload blocks on the barrier until injection completes.
//!   * fail-closed — pre-resolution, leader discovery, or ANY injection step failing tears the whole
//!     setup down and aborts the workload. There is NO host-network fallback, ever.
//!   * sealed destinations — agentd names a profile; the `host:proto:port` set comes ONLY from the
//!     compiled-in `shrek_policy::egress` table, resolved to PINNED IPv4 A-records here. AAAA-only /
//!     unresolvable ⇒ fail closed (IPv4-only).
//!
//! Like the rest of gatekeeperd it is dependency-free: it shells to the sealed `ip`/`nft` binaries
//! (as the onion broker shells to `mount`/`systemd-sysext`) and reads `/proc` directly.

use shrek_policy::egress::{EgressProfile, Proto};
use std::io::{self, Write};
use std::net::{Ipv4Addr, ToSocketAddrs};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// The private supernet egress sandboxes are numbered within — one `/30` per sandbox, host `.1`,
/// container `.2`. Chosen to be unlikely to collide with a real LAN; documented so an operator can
/// steer clear. Sandbox identity → `/30` is a pure hash (see [`SandboxNet::for_id`]).
const EGRESS_SUPERNET: [u8; 2] = [10, 66];

/// A resolved egress destination: a single pinned IPv4 endpoint. The profile's DNS `host` has been
/// resolved to concrete A-records HERE (the name is sealed policy; the IP is the runtime pin).
#[derive(Clone, Copy, Debug)]
pub struct Endpoint {
    pub ip: Ipv4Addr,
    pub proto: Proto,
    pub port: u16,
}

/// The per-sandbox network identity: deterministic names + `/30` derived purely from the sandbox id,
/// so setup and teardown agree without any shared state. Interface names are kept within Linux's
/// 15-char `IFNAMSIZ` limit by using the hashed index, not the raw id.
pub struct SandboxNet {
    pub ns: String,        // /run/netns name we bind the container's netns to
    pub host_if: String,   // root-side veth end
    pub cont_if: String,   // container-side veth end (lives in `ns`)
    pub table: String,     // per-sandbox nft table
    pub host_ip: Ipv4Addr, // .1 on the /30
    pub cont_ip: Ipv4Addr, // .2 on the /30
}

fn djb2(s: &str) -> u32 {
    let mut h: u32 = 5381;
    for b in s.bytes() {
        h = h.wrapping_mul(33).wrapping_add(b as u32);
    }
    h
}

/// Sanitize an id into an nft identifier (alnum + `_`); nft table names are less restrictive than
/// interface names, but keep it conservative.
fn sanitize(id: &str) -> String {
    id.chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '_' }).collect()
}

impl SandboxNet {
    /// Pure id → network identity. 14 bits of hash pick a `/30` inside the `/16` supernet (16384
    /// slots); interface names use the same index so they are unique per-`/30` and ≤ 15 chars.
    pub fn for_id(id: &str) -> SandboxNet {
        let idx = djb2(id) & 0x3FFF; // 0..16383 → one /30 each within 10.66.0.0/16
        let base = idx << 2; // /30-aligned host part (0,4,8,…)
        let o3 = (base >> 8) as u8;
        let o4 = (base & 0xFF) as u8;
        let [a, b] = EGRESS_SUPERNET;
        SandboxNet {
            ns: format!("shrek-{}", sanitize(id)),
            host_if: format!("skh{idx:04x}"),
            cont_if: format!("skc{idx:04x}"),
            table: format!("shrek_egress_{}", sanitize(id)),
            host_ip: Ipv4Addr::new(a, b, o3, o4 + 1),
            cont_ip: Ipv4Addr::new(a, b, o3, o4 + 2),
        }
    }

    /// Build the sealed nft ruleset for this sandbox: a PER-SANDBOX table with a `policy accept`
    /// forward chain that (1) allows exactly the resolved endpoints from this container's IP, then
    /// (2) `drop`s everything else FROM this container's IP. Scoping every rule to `cont_ip` means
    /// the table is default-deny for THIS sandbox without touching the host's global forward hook
    /// (a `policy drop` base chain would). Return replies match neither allow nor the scoped drop
    /// (their saddr is the remote), so `policy accept` + conntrack un-NAT delivers them.
    fn ruleset(&self, endpoints: &[Endpoint]) -> String {
        let mut allows = String::new();
        for e in endpoints {
            allows.push_str(&format!(
                "    ip saddr {} ip daddr {} {} dport {} accept\n",
                self.cont_ip, e.ip, e.proto.label(), e.port
            ));
        }
        format!(
            "table ip {t} {{\n\
             \x20 chain forward {{\n\
             \x20   type filter hook forward priority 0; policy accept;\n\
             {allows}\
             \x20   ip saddr {c} drop\n\
             \x20 }}\n\
             \x20 chain postrouting {{\n\
             \x20   type nat hook postrouting priority srcnat; policy accept;\n\
             \x20   ip saddr {c} masquerade\n\
             \x20 }}\n\
             }}\n",
            t = self.table,
            c = self.cont_ip,
        )
    }

    /// Inject the egress plumbing into the running container's netns (identified by `leader` pid),
    /// then install the sealed nft rules. Every step is fail-closed: the first error propagates and
    /// the caller tears everything down. Ordered rules-before-usable — the nft table (default-deny)
    /// is applied LAST here but still BEFORE the barrier is released by the caller.
    pub fn inject(&self, leader: u32, endpoints: &[Endpoint]) -> io::Result<()> {
        ip(&["netns", "attach", &self.ns, &leader.to_string()])?;
        ip(&["link", "add", &self.host_if, "type", "veth", "peer", "name", &self.cont_if])?;
        ip(&["link", "set", &self.cont_if, "netns", &self.ns])?;
        ip(&["addr", "add", &format!("{}/30", self.host_ip), "dev", &self.host_if])?;
        ip(&["link", "set", &self.host_if, "up"])?;
        ip(&["-n", &self.ns, "addr", "add", &format!("{}/30", self.cont_ip), "dev", &self.cont_if])?;
        ip(&["-n", &self.ns, "link", "set", &self.cont_if, "up"])?;
        ip(&["-n", &self.ns, "link", "set", "lo", "up"])?;
        ip(&["-n", &self.ns, "route", "add", "default", "via", &self.host_ip.to_string()])?;
        // Host must forward for masquerade to work. Best-effort write (already-1 on most hosts).
        let _ = std::fs::write("/proc/sys/net/ipv4/ip_forward", b"1");
        nft_apply(&self.ruleset(endpoints))
    }

    /// The filesystem path `ip netns add` binds this netns at — the value a T2/runsc OCI `network`
    /// namespace `path` must point to so the gVisor sentry joins THIS netns and programs netstack
    /// from the veth we provisioned in it. (`ip netns` mounts under `/var/run/netns`, which is
    /// `/run/netns` on a merged-/run distro; runsc opens the path verbatim.)
    pub fn ns_path(&self) -> String {
        format!("/run/netns/{}", self.ns)
    }

    /// PRE-SPAWN egress plumbing for a constructor that OWNS the netns lifecycle. Phase-6 slice-1b:
    /// unlike nspawn (T1) — where `--private-users` cannot join a host-owned netns (EPERM, #2563/C2),
    /// forcing the [`inject`](Self::inject) late-attach — `runsc` (T2) joins a host-created netns via
    /// the OCI `network` namespace path, so there is NO leader to discover and NO post-start barrier
    /// race. Create the netns, wire veth + addressing + route, and install the sealed nft allow-list
    /// ALL BEFORE the sandbox boots, so gVisor's netstack initializes from a fully-provisioned
    /// interface. Fail-closed: any step errors and the caller tears the whole thing down. Every step
    /// after `netns add` mirrors [`inject`](Self::inject) exactly — same veth, same addressing, same
    /// `ruleset` — so the egress BOUNDARY (host-side veth peer + per-sandbox nft) is identical to T1
    /// and independent of the guest's internal stack (netstack vs a kernel stack).
    pub fn create_and_inject(&self, endpoints: &[Endpoint]) -> io::Result<()> {
        ip(&["netns", "add", &self.ns])?;
        ip(&["link", "add", &self.host_if, "type", "veth", "peer", "name", &self.cont_if])?;
        ip(&["link", "set", &self.cont_if, "netns", &self.ns])?;
        ip(&["addr", "add", &format!("{}/30", self.host_ip), "dev", &self.host_if])?;
        ip(&["link", "set", &self.host_if, "up"])?;
        ip(&["-n", &self.ns, "addr", "add", &format!("{}/30", self.cont_ip), "dev", &self.cont_if])?;
        ip(&["-n", &self.ns, "link", "set", &self.cont_if, "up"])?;
        ip(&["-n", &self.ns, "link", "set", "lo", "up"])?;
        ip(&["-n", &self.ns, "route", "add", "default", "via", &self.host_ip.to_string()])?;
        // Host must forward for masquerade to work. Best-effort write (already-1 on most hosts).
        let _ = std::fs::write("/proc/sys/net/ipv4/ip_forward", b"1");
        nft_apply(&self.ruleset(endpoints))
    }

    /// Best-effort, idempotent teardown — remove the nft table, the veth pair (deleting the host end
    /// removes both), and our netns bind-name. Safe to call after a partial inject or a clean run.
    /// Leaves NO residual egress plumbing (fail-closed default is "no network").
    pub fn teardown(&self) {
        let _ = nft(&["delete", "table", "ip", &self.table]);
        let _ = ip(&["link", "del", &self.host_if]);
        let _ = ip(&["netns", "del", &self.ns]);
    }
}

/// A sealed profile resolved to concrete IPv4: the nft `endpoints` (every A-record, so the allow-list
/// covers all of a CDN's addresses) plus the `hosts` mapping (one pinned IP per name) that gets
/// written into the sandbox's `/etc/hosts` so the workload resolves WITHOUT any DNS egress.
pub struct Resolved {
    pub endpoints: Vec<Endpoint>,
    pub hosts: Vec<(&'static str, Ipv4Addr)>,
}

/// Resolve a sealed profile's `host:proto:port` rules to PINNED IPv4. Fail-closed: a rule whose host
/// yields NO A-record (AAAA-only, NXDOMAIN, resolver error) aborts the whole construct — egress is
/// IPv4-only and we never silently drop a destination the policy intended to allow.
pub fn resolve_profile_v4(profile: &EgressProfile) -> io::Result<Resolved> {
    let mut endpoints: Vec<Endpoint> = Vec::new();
    let mut hosts: Vec<(&'static str, Ipv4Addr)> = Vec::new();
    for r in profile.rules {
        let mut first: Option<Ipv4Addr> = None;
        // getaddrinfo via std; the `:port` makes it a SocketAddr iterator.
        for sa in (r.host, r.port).to_socket_addrs()? {
            if let std::net::SocketAddr::V4(v4) = sa {
                let ip = *v4.ip();
                first.get_or_insert(ip);
                let dup = endpoints.iter().any(|o| {
                    o.ip == ip && o.port == r.port
                        && matches!((o.proto, r.proto), (Proto::Tcp, Proto::Tcp) | (Proto::Udp, Proto::Udp))
                });
                if !dup {
                    endpoints.push(Endpoint { ip, proto: r.proto, port: r.port });
                }
            }
        }
        match first {
            // One pinned IP per name for /etc/hosts (nft still allows every resolved A-record).
            Some(ip) => hosts.push((r.host, ip)),
            None => {
                return Err(io::Error::new(
                    io::ErrorKind::Other,
                    format!("egress host {:?} has no IPv4 (A) record — fail closed (IPv4-only)", r.host),
                ))
            }
        }
    }
    Ok(Resolved { endpoints, hosts })
}

/// Render the pinned host→IP map as an `/etc/hosts` body (localhost + one line per sealed host). The
/// workload resolves profile names through THIS, never a DNS query that would need egress.
pub fn etc_hosts(hosts: &[(&'static str, Ipv4Addr)]) -> String {
    let mut s = String::from("127.0.0.1 localhost\n");
    for (h, ip) in hosts {
        s.push_str(&format!("{ip} {h}\n"));
    }
    s
}

/// Discover the container's netns leader: a process descending from `nspawn_pid` that lives in a
/// DIFFERENT net namespace than us. nspawn buries the netns holder several forks down, so we walk
/// full ancestry (the oracle showed `pgrep -P` two levels was not enough). Polls until `timeout`.
pub fn discover_leader(nspawn_pid: u32, timeout: Duration) -> io::Result<u32> {
    let host_net = std::fs::read_link("/proc/self/ns/net")?;
    let deadline = Instant::now() + timeout;
    loop {
        for ent in std::fs::read_dir("/proc")?.flatten() {
            let Some(name) = ent.file_name().to_str().map(str::to_owned) else { continue };
            let Ok(pid) = name.parse::<u32>() else { continue };
            let Ok(net) = std::fs::read_link(format!("/proc/{pid}/ns/net")) else { continue };
            if net == host_net {
                continue;
            }
            if is_descendant(pid, nspawn_pid) {
                return Ok(pid);
            }
        }
        if Instant::now() >= deadline {
            return Err(io::Error::new(io::ErrorKind::TimedOut, "container netns leader not found"));
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// Walk PPid up from `pid`; true if `ancestor` is on the chain (bounded to avoid a cycle hang).
fn is_descendant(mut pid: u32, ancestor: u32) -> bool {
    for _ in 0..32 {
        let Ok(status) = std::fs::read_to_string(format!("/proc/{pid}/status")) else { return false };
        let ppid = status
            .lines()
            .find_map(|l| l.strip_prefix("PPid:").map(|v| v.trim().parse::<u32>().ok()))
            .flatten();
        match ppid {
            Some(p) if p == ancestor => return true,
            Some(p) if p > 1 => pid = p,
            _ => return false,
        }
    }
    false
}

// ---- shelling to the sealed ip/nft binaries ------------------------------------------------------

fn ip(args: &[&str]) -> io::Result<()> {
    let st = Command::new("ip").args(args).status()?;
    if st.success() {
        Ok(())
    } else {
        Err(io::Error::new(io::ErrorKind::Other, format!("ip {:?} failed rc={:?}", args, st.code())))
    }
}

fn nft(args: &[&str]) -> io::Result<()> {
    let st = Command::new("nft").args(args).status()?;
    if st.success() {
        Ok(())
    } else {
        Err(io::Error::new(io::ErrorKind::Other, format!("nft {:?} failed rc={:?}", args, st.code())))
    }
}

/// Apply a ruleset atomically via `nft -f -`.
fn nft_apply(ruleset: &str) -> io::Result<()> {
    let mut child = Command::new("nft").arg("-f").arg("-").stdin(Stdio::piped()).spawn()?;
    child
        .stdin
        .take()
        .ok_or_else(|| io::Error::new(io::ErrorKind::Other, "nft: no stdin"))?
        .write_all(ruleset.as_bytes())?;
    let st = child.wait()?;
    if st.success() {
        Ok(())
    } else {
        Err(io::Error::new(io::ErrorKind::Other, format!("nft -f - failed rc={:?}", st.code())))
    }
}

// ---- the ready-barrier (host side of the rendezvous bound into the sandbox at /rv) ---------------

/// Files the barrier-wrapped workload polls (see `sandbox::EGRESS_BARRIER`). The host writes `go`
/// once injection succeeds, or `abort` on any failure (fail-closed — the workload never egresses).
pub struct Barrier {
    pub dir: PathBuf,
}

impl Barrier {
    pub fn go(&self) -> io::Result<()> {
        std::fs::write(self.dir.join("go"), b"1")
    }
    pub fn abort(&self) {
        let _ = std::fs::write(self.dir.join("abort"), b"1");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn subnet_and_names_are_pure_and_bounded() {
        let a = SandboxNet::for_id("s0");
        let b = SandboxNet::for_id("s0");
        // deterministic
        assert_eq!(a.host_ip, b.host_ip);
        assert_eq!(a.cont_if, b.cont_if);
        // host/container are .1/.2 of the same /30
        let h = a.host_ip.octets();
        let c = a.cont_ip.octets();
        assert_eq!([h[0], h[1]], EGRESS_SUPERNET);
        assert_eq!(h[3] + 1, c[3]);
        assert_eq!(h[3] % 4, 1); // .1 within a /30 block
        // interface names fit IFNAMSIZ (15 usable chars)
        assert!(a.host_if.len() <= 15 && a.cont_if.len() <= 15);
    }

    #[test]
    fn distinct_ids_generally_get_distinct_subnets() {
        let x = SandboxNet::for_id("build-a");
        let y = SandboxNet::for_id("build-b");
        assert_ne!(x.cont_ip, y.cont_ip);
        assert_ne!(x.host_if, y.host_if);
    }

    #[test]
    fn ruleset_scopes_every_rule_to_the_container_ip_and_denies_by_default() {
        let net = SandboxNet::for_id("t1");
        let eps = [
            Endpoint { ip: Ipv4Addr::new(140, 82, 112, 3), proto: Proto::Tcp, port: 443 },
            Endpoint { ip: Ipv4Addr::new(140, 82, 113, 3), proto: Proto::Tcp, port: 443 },
        ];
        let rs = net.ruleset(&eps);
        let cip = net.cont_ip.to_string();
        // an allow line per endpoint, each scoped to the container source ip
        assert_eq!(rs.matches(" accept\n").count(), 2);
        assert!(rs.contains(&format!("ip saddr {cip} ip daddr 140.82.112.3 tcp dport 443 accept")));
        // default-deny for this sandbox + masquerade, both scoped to the container ip
        assert!(rs.contains(&format!("ip saddr {cip} drop")));
        assert!(rs.contains(&format!("ip saddr {cip} masquerade")));
        // policy is ACCEPT (does not hijack the host's global forward hook)
        assert!(rs.contains("policy accept"));
        assert!(!rs.contains("policy drop"));
    }

    #[test]
    fn sanitize_makes_safe_identifiers() {
        assert_eq!(sanitize("g-net-1"), "g_net_1");
        assert_eq!(sanitize("ok_9"), "ok_9");
    }
}
