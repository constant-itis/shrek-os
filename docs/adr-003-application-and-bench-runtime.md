# ADR-003 — Application delivery & the Bench/Workshop container runtime

**Status:** 🟢 Proposed — **Fable-reviewed GO-WITH-FIXES (2026-08-30, corrections folded);
Part 2 runtime feasibility (MVP step 2) VERIFIED GREEN on the sealed VM (2026-08-30);
Part 1 baseline-app + browser Onions BUILT + MERGE-PROVEN + RENDER-PROVEN on the sealed
boot (2026-08-30, dogfood PASS=81/0); Part 2 MVP step 3 (the `prjquota` Bench storage pool)
BUILT + PROVEN on the sealed boot (2026-08-30, dogfood PASS=83/0 — project quota EDQUOT-enforced
on the growable `/home`); Part 2 MVP step 4 (the `bench_plane` lifecycle supervisor + `shrek bench`
CLI + the persistent state model) BUILT + PROVEN (2026-08-30, dogfood PASS=88/0 — `gatekeeperd bench`
create→run→quota→destroy end-to-end on the sealed image, plus a 14/14 host oracle);
Part 2 MVP step 5 (route FS + egress grants through the existing Gatekeeper — rule 3, the
security-critical core) BUILT + PROVEN (2026-08-30 — a 30/30 host oracle runs REAL rootless podman: an
FS grant round-trips writes to `dev`, a `--ro` grant denies writes, and a networked `run` late-attaches a
veth + the sealed nft allow-list into the bench netns; plus the sealed-boot BENCH-GRANT stage proving
grant materialization + boot re-issuance on the real prjquota `/home`);
Part 2 MVP step 6 (ship one offline Scratch seed) BUILT + PROVEN (2026-08-30 — the delivery mechanism was
decided EMPIRICALLY: `podman load` of the sysext OCI-archive runs green on native rootless overlay, so
`additionalimagestores` is NOT used; the seed is a real `alpine`+`coreutils`+`ffmpeg`+`exit42` base
(~52M, `podman load`ed on demand by `bench_plane`'s digest-keyed `ensure_seed`); host oracle 33/0 and the
sealed-boot BENCH/BENCH-SUP stages prove the seed runs + `ffmpeg` is present + the loader re-materializes
the image from the archive);
Part 2 MVP step 7 (constrained `.desktop` export) BUILT + PROVEN (2026-08-30 — a Bench app exports as a
`.desktop` whose `Exec` is only `/usr/bin/shrek-bench-run <bench> <key>`: the fixed-baked-key discipline,
so the untrusted `.desktop` carries a KEY and the command lives in the root-owned record; Fable-reviewed
GO-WITH-FIXES with all five must-fixes folded — env-override feature-gate, write-`.desktop`-as-dev,
argv-preserving encoding, recorded filenames, and a dot-leading-path grant denylist; host oracle 45/0 +
the sealed-boot BENCH-EXPORT stage).**
Decides *how a daily-driver Shrek OS gets its everyday apps* and *how a user installs
anything beyond the baked baseline*, on a sealed immutable image. Builds on ADR-001
(Deployment / A-B) and ADR-002 (environment vocabulary — this ADR uses those nouns verbatim:
**Application**, **Onion**, **Bench**, **Workshop**, **Job**, **User Tool**).

> **Bench-0 runtime proof — GREEN on the real sealed boot (dogfood oracle, PASS=74 FAIL=0).**
> Rootless Podman/crun runs on the sealed dm-verity image with all six assertions passing as
> `dev`: (i) sealed unprivileged userns (`unshare -U` — the AppArmor risk Fable flagged is
> clear on this base), (ii) **native** rootless `overlay` (not vfs/fuse), (iii) subuid maps
> the full 65536 range (setuid `newuidmap` survives the sysext merge), (iv) a real compiled
> ELF execs off the `noexec` `/home` pool → rc 42 (rule-2 confirmed — fresh-superblock exec
> works), (v) negative control: direct exec from the `noexec` pool is blocked, (vi) the
> container lands under `dev`'s delegated systemd `user-1000.slice`. Artifacts:
> `layers/shrek-bench/`, `scripts/build-bench-layer.sh`, the `image/mkosi.postinst` bakes, and
> the Bench-0 stage in the dogfood probe/oracle. **Integration findings the proof surfaced
> (all fixed) — see "Consequences → the sealed-`/etc` footprint" below.**

**Owner decisions locked (2026-08-30):**
1. Baseline apps are **baked** (immutable ⇒ no post-install apt escape hatch; the
   Silverblue/SteamOS/Bazzite rule, philosophy #2905).
2. The **web browser is its own signed Onion** (Firefox ESR), *separate* from the rest
   of the baseline app set, so it updates independently of everything else.
3. The "everything-else" install plane is **Podman/crun-based** and is **ADR-first** — no
   AppImage. This ADR is that first step; the runtime is proven *before* any user-facing
   `shrek bench` code lands. (Deliberately rejects AppImage-in-`~/Apps`: unconfined,
   no portal/secret/grant story — a personal daily-driver deserves the real plane.)

## Context

Today's *installed* Shrek system (as opposed to the installer USB, which is app-rich but
throwaway) has a complete shell — Sway + DMS + Quickshell, foot, PipeWire, grim/slurp,
udisks2/NetworkManager/BlueZ/UPower backends — and **almost no applications**: no
browser, file manager, image/PDF/media viewer, archive manager, or GUI editor; a thin
font set (DejaVu-core + Material Symbols only); and even the everyday CLI utilities
(`curl`/`wget`/`git`/`unzip`/`openssh`/`jq`/`rg`) live only in the installer/dev sysexts,
never on the disk you boot. On a normal distro these are a `apt install` away. On a
sealed dm-verity `/usr` they are not — a missing app requires a full image rebuild the
end user cannot perform. So the inclusion rule flips: **bundle broadly.**

Two things must be decided together, because "what apps exist" (the package manifest) and
"how you add more" (the mutable plane) are the two halves of a usable OS.

## Decision

### Part 1 — Baseline Applications are delivered as Onions

The everyday apps are **Application**-kind software (ADR-002) shipped as signed dm-verity
**Onion** sysexts, merged by `oniond` under the sealed `onion-policy`. This satisfies both
the *functional-out-of-the-box* goal (#2905) and immutability: an Onion is a separate
signed partition, **not** baked into the sealed root image, and is independently
rebuildable/updatable — which is precisely why the browser gets its own layer.

> **Part 1 proof — BUILT + GREEN on the real sealed boot (dogfood oracle, PASS=81 FAIL=0).**
> Both Onions are built (`shrek-browser` 377 M, `shrek-apps` 1.2 G) and both halves are proven:
> (a) **MERGE** — the dogfood probe asserts, on the merged sealed `/usr`, that `firefox-esr` +
> its `.desktop` landed, the app set (nautilus/loupe/papers/mpv/file-roller+backends/
> gnome-text-editor) landed with entries, the real font set landed (Noto **emoji** + **CJK** —
> the thin-font fix), and the everyday CLI utils (`curl`/`git`/`jq`/`rg`/`fd`/…) are now on the
> installed disk (they existed only in the installer/dev sysexts before). (b) **RENDER** —
> `scripts/apps-render-proof.sh` launches firefox-esr + a GTK app under headless
> sway (`WLR_RENDERER=pixman`) + `grim` and asserts a real pixel frame (unique-colour count
> ≫ a blank frame), because a virtio-vga guest without virgl scanout-captures GTK as bare window
> *outlines* — a capture artifact, not a render failure (#2923). Artifacts: `layers/shrek-browser/`,
> `layers/shrek-apps/`, `scripts/build-{browser,apps}-layer.sh`, the `onion-policy` enables, the
> `build-layers.sh` `INCLUDE_BROWSER`/`INCLUDE_APPS` staging, the APPS stage in the dogfood probe/
> oracle, and `scripts/apps-render-proof.sh`.

**Two new Onion layers** (same build shape as `shrek-desktop`: `mkosi` `Format=sysext`,
`--base-tree <sealed-base closure> --overlay`, `enable` line in
`image/overlay/usr/lib/shrek/onion-policy`, staged by `scripts/build-layers.sh`):

| Layer | Kind | Contents |
|---|---|---|
| **`shrek-browser`** | Onion | `firefox-esr` (own layer, updates independently — owner decision 2) |
| **`shrek-apps`** | Onion | file manager, image viewer, PDF viewer, media player, archive manager, GUI text editor, a real font set, and the everyday CLI utilities |

Concrete `shrek-apps` candidate manifest (all Debian `main`, Wayland-native where it
matters; final closure resolved + logged at build per the `# VERIFY` discipline):

- **File manager:** `nautilus` *(GTK4, Wayland-clean, GVfs/udisks2-native trash+mount)* —
  alternative `nemo` or `pcmanfm-qt` if closure is too heavy.
- **Image viewer:** `loupe` *(GNOME's GTK4 viewer)* — alt `eog`.
- **PDF/document viewer:** `papers` *(GNOME's GTK4 Evince successor)* — alt `evince`.
- **Media/video player:** `mpv` *(minimal deps, Wayland/GPU-accelerated, no DE pull-in)*.
- **Archive manager:** `file-roller` + backends `unzip zip p7zip-full xz-utils`.
- **GUI text editor:** `gnome-text-editor` *(GTK4)* — alt `gedit`.
- **Fonts (the thin-font fix):** `fonts-noto-core fonts-noto-color-emoji
  fonts-noto-cjk fonts-liberation2 fonts-dejavu` — a UI face, emoji (kills tofu boxes),
  and CJK coverage.
- **Everyday CLI utils (promoted from the installer sysext into the installed OS):**
  `curl wget git openssh-client jq ripgrep fd-find unzip zip ca-certificates less
  htop rsync` + document/format helpers.

`.desktop` entries land in `/usr/share/applications`; the DMS launcher + `shrek-menu`
apps provider already surface host `DesktopEntries` (#2827), so no launcher wiring is
needed beyond the entries themselves.

**Size posture:** these are the fat sysext plane, *not* the fixed 2 G A/B root slot, so
they do not pressure root geometry (same separation that let the ~400 MB firmware batch
fit, #2909). Firefox is the single largest closure and its own layer, so its size is
isolated and its updates are decoupled.

### Part 2 — The Bench/Workshop plane runs on Podman/crun inside an Onion

Everything a user installs beyond the baked baseline runs in the **Bench** plane
(ADR-002: the mutable "mess-with-a-door" — `apt`/`pip`/experiments without touching sealed
`/usr`; promotes to a reproducible **Workshop**). Its runtime substrate is **rootless
Podman + crun**, delivered as its own Onion:

| Layer | Kind | Contents |
|---|---|---|
| **`shrek-bench`** | Onion | `podman crun conmon fuse-overlayfs uidmap catatonit` + baked configs at `/usr/share/containers/{storage,containers}.conf` (graphroot/runroot→`/home`, `image_copy_tmp_dir`→`/home`) |

**What is deliberately NOT in the layer** (Fable corrections):
- **`subuid`/`subgid` do not go in a sysext** — a sysext extends `/usr` only, and runtime
  `/etc` is sealed read-only (dm-verity); this is the same constraint that forces `dev`/
  `swamp`/`polkitd` to be created in `image/mkosi.postinst`. Bake `dev:100000:65536` into
  `/etc/subuid`+`/etc/subgid` in `mkosi.postinst` (or ship via the `shrek-conf` **confext**,
  the only layer kind that may touch `/etc`).
- **`passt`/`slirp4netns` are dropped from the granted path** — their egress emerges from
  the user's host stack as `dev`'s own traffic, so there is no per-bench source address for
  `net_plane`'s nft rules to key on (they'd make `shrek bench network` unenforceable). Bench
  egress uses `net_plane`'s late-attach path instead (rule 3). `--network=none` by default.
- **`conmon` is mandatory** (podman won't run without it); the baked `containers.conf`/
  `storage.conf` at `/usr/share/containers/` is podman's native config path — reading there
  sidesteps the read-only-`/etc` problem for everything except subuid.

**Four load-bearing design rules (the parts #2829 said must be nailed):**

1. **Storage lives on the persistent `/home` plane, not `/var`, not swamp-state.**
   *(Fable-verified: `/var` is volatile tmpfs via `systemd.volatile=state`; `/home` is the
   only durable plane — ext4 label `shrek-data`, `home.mount`.)* `/var` is volatile and
   swamp-state is the 128 M control-plane; container storage is neither. Benches get a
   dedicated pool under `/home` — e.g. `/home/.shrek/benches/` — with explicit quotas
   (needs `prjquota` on the `shrek-data` fs → a `dogfood-data-disk.sh` + installer-format +
   `home.mount` options change) + GC. Podman's `graphroot`/`runroot` **and**
   `image_copy_tmp_dir` all point there via the baked `/usr/share/containers/*.conf` —
   *both* redirects matter: podman's default `image_copy_tmp_dir=/var/tmp` is tmpfs here, so
   pulling a large image would OOM. `loginctl enable-linger` state also lives in volatile
   `/var/lib/systemd/linger`; MVP posture = **benches die on logout** (defensible; stated
   explicitly rather than papered over).
2. **`noexec` on the host pool — exec works in-container by fresh-superblock, not "the
   container's internal view."** *(Fable correction — the original mechanism was wrong.)*
   `noexec` is a **vfsmount** property. The overlay/fuse-overlayfs merged rootfs podman runs
   is a **fresh superblock** mounted without `MS_NOEXEC`, so it does not inherit `noexec`
   from the pool's backing mount — that is *why* exec works in the container even on a
   `noexec` pool. Three consequences the rule must carry: **(a)** the `vfs` fallback driver
   is **forbidden** — it binds a plain dir as rootfs, and a bind *inherits* `noexec`, so
   container init can't exec; **(b)** proof requires a **real compiled ELF**, never a shell
   script (the repo has already watched a script-based test false-pass where a real ELF
   failed — `mount_plane.rs:171-177`, the same class that killed gVisor-on-noexec); **(c)**
   `noexec` on the pool buys host-side safety only (`dev`'s shell can't exec a binary dropped
   into the graphroot), not container containment. **Pre-approved fallback if the proof
   fails:** exec on the graphroot, `noexec` only on *granted data* mounts (which
   `relocate_rw` already enforces) — so a failed proof re-scopes instead of stalling.
   (Note: `home.mount` is `nosuid,nodev` but **not** `noexec` today — the noexec pool is a
   new sub-mount to build, it does not exist yet.)
3. **Grants route through the existing Gatekeeper — no parallel security system, no
   loosening of T2.** *(Fable-verified structurally honest: `t2_plane.rs:28-31` only
   *imports* `mount_plane`/`net_plane` as libraries; a sibling can too.)* The Bench is a
   **new user-compute plane** that *reuses* the grant primitives in `gatekeeperd`
   (`mount_plane.rs` `pin_beneath`/`relocate_rw` — ns-agnostic, reusable; `net_plane.rs`
   egress) — **not** an extension of the T2/`runsc` constructor. T2 = narrowly-constructed,
   *agent*-authority, ephemeral; Bench = persistent, *user*-authority. **Egress uses the
   late-attach shape only:** benches run `--network=none`, and on grant, root `gatekeeperd`
   drives `net_plane.rs:149` `inject()` against the container's netns leader (root holds
   caps in every userns). The pre-spawn `create_and_inject()` path is **structurally
   unavailable** to rootless podman (joining a root-created netns needs `CAP_SYS_ADMIN` in
   the netns's owning userns). Because a rootless netns dies on every container stop, grants
   are **re-issued per container start and at boot** — so bench starts must be brokered
   through the `shrek bench` CLI (or a `podman events` watcher).
4. **MVP grants no secrets.** A Bench starts with access to *nothing* — no `~/.ssh`
   mount, no inherited host env, no token copy. Git etc. come later via device-code / a
   host-brokered one-shot credential helper (#2829). A malicious `npm postinstall` finds
   only the files explicitly granted to that Bench.

**`bench_plane.rs` is a lifecycle supervisor, not a thin CLI shim** *(Fable blocker 4)*.
Unlike T2's grant mounts — which live in a per-request private ns of a `gatekeeperd` child
and die with the process — a persistent Bench needs its grant mounts visible where `dev`'s
podman can see them (host ns, under `/run/shrek/bench/<id>/grants/`). That forces a named
**persistent-grant state model**, budgeted as the real work of Part 2: durable grant records
on `/home` (template = `net_binding.rs`'s record pattern), boot-time re-issuance (`/run` is
volatile), survival across `gatekeeperd` restarts, uid-mapping so root-relocated mounts are
traversable by `dev`'s subuid range (`--userns=keep-id` vs idmapped mounts), and teardown on
`destroy`.

**`shrek bench` verbs** (ADR-002 already reserves the `shrek bench …` namespace):
`create <name>` · `enter <name>` · `run <name> -- <cmd>` · `grant <name> <path> --rw` ·
`network <name> <policy>` · `export <name> <key> -- <cmd>` · `run-export <name> <key>` ·
`unexport <name> <key>` · `reset <name>` · `quota <name>` · `destroy <name>` · and the
ADR-002 promote path `promote <name> → Workshop`.

### Workshop seeds & the `debian-apt` egress profile — ✅ apt workshop GREEN (2026-09-01)

A Bench runs from a SEALED **seed catalog** (bench_plane `SEED_CATALOG`), not a user-chosen
image — the same fail-closed discipline as an egress profile name. Two seeds ship:

- **`scratch`** — the tiny Alpine/media proof seed (musl; ffmpeg north-star).
- **`debian`** — the **apt + pip workshop** seed (glibc): `debian:trixie` + `ca-certificates`
  **+ `python3` / `python3-venv` / `python3-pip`** (`--no-install-recommends`, so NO build
  toolchain rides along), with deb822 runtime sources pointing EVERY suite (`trixie`,
  `-updates`, `-security`) at **`https://deb.debian.org`** and `/var/lib/apt/lists` purged. Built
  by `scripts/build-workshop-seed.sh` (~76M archive baked beside `scratch.tar` in the shrek-bench
  sysext), loaded on demand by `ensure_seed`, selected per-bench at
  `shrek bench create <name> --seed debian` (recorded as a `seed` line in the durable record;
  a pre-seed record defaults to `scratch`). A dedicated `python` seed was considered and rejected:
  the debian seed already carries apt, so `apt` **and** `pip` in ONE bench is the natural workshop
  base (and the only way to `pip install` an sdist needing an apt-installed compiler, since a
  `--rm` run makes apt state per-session).

This realizes ADR-002's "mess-with-a-door": `apt-get install` real tooling in a bench without
touching the sealed `/usr`. Egress is the sealed **`debian-apt`** profile — **ONE host**,
`deb.debian.org:tcp:443`. deb.debian.org is a direct Fastly-CDN service fronting the security
archive too, so one pin covers every suite; `security.debian.org` (a separately-rotating
round-robin) is deliberately NOT reachable. HTTPS/443 only: on a shared-CDN IP allow-list a
plaintext `:80` `Host:` header would reach any Fastly customer. apt validates the cert against
the SNI name, never the spawn-time-pinned IP (identical to `github-https`). Proven LIVE in the
host oracle (`bench-plane-proof.sh`): a `debian` bench granted only `debian-apt` runs
`apt-get update` + `apt-get install sl` reaching deb.debian.org through the injected veth+nft,
while `apt-get changelog` (an unlisted host) fails closed and the host stays sealed. The sealed
VM proves the shipped seed bakes + loads + selects, offline.

> **SEALED-EGRESS INVARIANT (Fable):** a Bench holding `debian-apt` — or ANY internet egress
> profile — must **never receive a secret** via grant, env, or export. The shared-CDN aperture
> (any Fastly site is reachable to a workload crafting its own TLS) is contained ONLY by this
> no-secrets rule, not by the network layer. Already enforced: the dot-path grant denylist
> blocks `~/.ssh`/`~/.config`; secrets stay host-side via the broker pattern. The real hardening
> — a host-side apt broker (same shape as `model-proxy`/`swamp-broker`, enforcing `Host`/SNI =
> deb.debian.org) — is post-MVP.

### PyPI / pip workshop & the repeatable `network` verb — ✅ GREEN (2026-09-01)

The pip fast-follow adds a sibling egress profile and makes a Bench hold a **set** of profiles so
`apt` and `pip` compose on one bench.

- **`pypi-https` sealed profile** (shrek-policy `egress.rs`) — **TWO hosts**: `pypi.org:443`
  (the Simple/JSON index + pip's own version self-check) and `files.pythonhosted.org:443` (the
  separate CDN serving wheels/sdists + PEP 658 `.metadata`). Both Fastly-fronted — the SAME
  shared-CDN aperture as `debian-apt`/`github-https`, so https/443 only and the **SEALED-EGRESS
  INVARIANT above applies verbatim** (a pip bench must hold no secret). `pypi.python.org` (a 301
  relic) and `test.pypi.org` are deliberately unlisted; pip vendors its own certifi bundle, so
  there is no CA-fetch host.

- **Repeatable `network` verb — a DECLARATIVE SET.** `shrek bench network <name> <profile...>`
  now sets the bench's egress to EXACTLY the listed set (it **replaces** the prior set; `network
  <name> none` revokes all). So `network web debian-apt pypi-https` composes both; `run_networked`
  resolves the **union** via `net_plane::resolve_profiles_v4` (endpoints deduped by
  `(ip,proto,port)`, `/etc/hosts` by name — a single-element set is byte-for-byte the legacy
  result). The record simply holds multiple `net <profile>` lines. Declarative-set was chosen over
  an additive `--add/--remove` flag because it is **consent-superior**: the ceremony always shows
  the COMPLETE resulting reachability (one diff row per profile), and the approved tuple binds the
  absolute set — a concurrent record change between prompt and commit is overwritten by exactly
  what the human saw, never composed with unseen state. The ceremony-free `none` revoke is routed
  by EXACT arity (`<name> none` with nothing after) so no `network <name> none <profile>` can
  silently drop args into a revoke.

- **NOEXEC `/work` shapes the pip UX.** The bench data pool is mounted `noexec`, so a venv's
  entry-point **scripts** (`/work/venv/bin/pip`) cannot `execve`. The blessed invocation is
  `python3 -m venv /work/venv` then **`/work/venv/bin/python3 -m pip install <pkg>`** — `python3`
  is a symlink resolving to the on-exec `/usr/bin/python3`, so it runs; pip is imported as a
  module, never exec'd as a script. The persistent `/work` venv is therefore **pure-python only**
  (a native-extension wheel needs `PROT_EXEC` to `dlopen`, so it belongs in an ephemeral in-overlay
  venv within one session — consistent with apt's per-session semantics). The oracle pins this as a
  tested invariant: `/work/venv/bin/pip --version` MUST fail (script on noexec) while `python3 -m
  pip` succeeds — a guard against a future pool-loses-noexec regression.

Proven LIVE in the host oracle: a `debian` bench granted `debian-apt pypi-https` runs
`apt-get install` AND a `python3 -m pip install` reaching PyPI through the injected veth+nft; each
profile ALONE fails closed for the other's host (cross-profile isolation is enforced at name
resolution — the bench `/etc/hosts` holds only the granted profiles' pinned hosts; IP-level
overlap on the shared Fastly CDN is the documented aperture, not asserted by IP). The sealed VM
proves, offline, that the seed bakes python3/pip/venv and that the repeatable verb records the
composed set.

**Two follow-ups still on the proven step-5 path — do NOT bolt on blind:**
1. **Egress-before-workload.** `run_networked` late-attaches egress AFTER the container starts
   (the rootless constraint), so a naive `apt-get update` fails during the no-egress window and
   the container exits before inject (netns-drift guard, fail-closed). A workload that survives
   the window (retry-until-egress) works today; the product fix is a **holder+exec** model
   (start a `sleep`-holder → inject → `exec` the workload), mirroring interactive `enter`.
2. **Persistence.** `run` is `--rm`, so an `apt`/`pip install` lands in the EPHEMERAL container
   layer, not the persistent `/work` — durable installs are the ADR-002 **`promote`** path (bench
   → signed Workshop layer), still a stub. (PyPI `pypi-https` + the repeatable `network` verb —
   the third fast-follow named here — is now ✅ done, see the subsection above.)

### MVP sequence (build order, #2829 — each gated, don't skip ahead)

1. **This ADR** (defines Host / Onion / T2 / Bench authority + the four rules above).
2. **The one-script feasibility proof — ✅ DONE & GREEN (2026-08-30, sealed VM, PASS=74/0).**
   The smallest experiment that retires five unknowns at once (Fable's "smallest next proof").
   Built the real `shrek-bench` sysext (not a throwaway) + baked `subuid`/`subgid` via
   `mkosi.postinst`, mounted an ext4 pool at `/home/.shrek/benches` with `noexec,nosuid,nodev`,
   hand-stage a throwaway podman sysext, mount an ext4 pool at `/home/.shrek/benches` with
   `noexec,nosuid,nodev`, then as `dev` on the **sealed boot** assert, in order:
   (i) `unshare -U true` → rc 0 (kernel + AppArmor userns posture — don't trust the Debian
   default); (ii) `podman unshare cat /proc/self/uid_map` → full 65536 range (subuid bake +
   setuid `newuidmap` surviving the sysext merge); (iii) `podman load` a seeded image and
   `podman run` a **compiled static ELF that exits 42** (not `true`, not a script) → rc 42
   and `podman info` shows native `overlay`; (iv) negative control: exec that same ELF
   *directly from the graphroot path on the host* → must fail `EACCES` (proves `noexec` is
   real); (v) `systemd-cgls /user.slice/user-1000.slice` shows the container under `dev`'s
   delegated slice. Pass → rules 1+2 and open-Qs 1/2/4 retired. Fail at (iii) → the
   pre-approved fallback posture (rule 2) activates, no redesign. **Must not alter
   `t2_plane.rs`.**
3. **Dedicated growable Bench storage pool** on `/home` with `prjquota` quotas (rule 1) —
   productionize the throwaway pool from step 2 (installer-format + `home.mount` + `dogfood-
   data-disk.sh`). **✅ DONE & GREEN (2026-08-30, sealed VM, dogfood PASS=83/0).** shrek-data is
   formatted `-O quota,project` (installer + `dogfood-data-disk.sh`) and mounted `prjquota` at the
   INITIAL mount — ext4 only engages quota there, a later `remount,prjquota` leaves "Quota mode:
   none" (proven). `shrek-home-quota-prep.service` runs BEFORE `home.mount` and retrofits the
   feature onto any pre-step-3 disk (`e2fsck` + `tune2fs -O quota,project`) so the `prjquota` mount
   can never brick. The Bench pool itself is a **noexec,nosuid,nodev bind sub-mount** on that growable
   fs (`shrek-bench-pool.service` — rule 2's "the noexec pool is a new sub-mount to build"), replacing
   the step-2 loopback stopgap so benches share `/home`'s space AND get project quotas. The dogfood
   probe proves EDQUOT enforcement end-to-end: a `dev` (non-root — root is quota-exempt) `conv=fsync`
   write past a 1 MiB project cap fails "Disk quota exceeded" at exactly the cap. bench_plane (step 4)
   caps each Bench with `setquota -P`.
4. **`bench_plane.rs` (lifecycle supervisor) + `shrek bench` CLI** (create/enter/run/reset/
   quota/destroy) + the persistent-grant state model above. **✅ DONE & GREEN (2026-08-30, dogfood
   PASS=88/0 + host oracle 14/14).** `crates/gatekeeperd/src/bench_record.rs` is the durable state
   model (the `net_binding.rs` record shape, but on the persistent `/home` not volatile `/run`):
   `SHREK-BENCH 1` line-text records under `/home/.shrek/records` — the ROOT-owned `/home/.shrek`
   anchor, deliberately NOT inside the `dev`-owned `0700` pool `/home/.shrek/benches` (a records dir
   there would have a `dev`-owned parent that `dev` could `rename(2)` aside to substitute forged records;
   mycelium #2982 hole 2). Atomic temp+rename, fail-closed parse, `next_project_id` allocation from a base. `bench_plane.rs` is the supervisor —
   a SIBLING of `t2_plane` (imports `mount_plane`/`net_plane` as libs like `t2_plane.rs:28-31`,
   **never touches `t2_plane.rs`**): `create` allocates an ext4 project id + caps it (`chattr -p +P`
   + `setquota -P`) + writes the record; `run`/`enter` drop to `dev`'s rootless podman
   (`--network=none --no-hosts`, `/work` = the quota-scoped data dir); `reset` wipes data but keeps
   identity+quota; `quota` re-caps; `destroy` frees the id + removes everything; `reissue` re-applies
   quotas at boot (the records are the source of truth `/run` is rebuilt from). `grant`/`network`
   are explicit stubs (step 5); `promote` is later. The `gatekeeperd bench` verb (a new `main.rs`
   dispatch arm) is the privileged supervisor; `shrek bench` forwards to it (mirrors `shrek run` →
   `gatekeeperd sandbox`). Proven: 10 unit tests, `scripts/bench-plane-proof.sh` (14/14 in a
   privileged container — record + project-quota EDQUOT enforcement for a non-root writer + id
   reuse), and a sealed-boot BENCH-SUP stage (create→run exit42→quota-enforce→destroy).
5. **Route FS + egress grants through the existing Gatekeeper** (rule 3 — the security-critical core).
   **✅ DONE & GREEN (2026-08-30, host oracle 30/0 with REAL rootless podman + the sealed-boot BENCH-GRANT
   stage).** `bench_plane.rs` wires the `grant`/`network` verbs (no `t2_plane.rs` edit — it imports
   `mount_plane`/`net_plane` as libs, `t2_plane.rs:28-31`).
   - **FS grants** (`grant <name> <dir> --rw|--ro`): pin the dir TOCTOU-safely beneath the `/home/<dev>`
     anchor (`open_anchor`+`pin_beneath`), then `relocate_rw`/`relocate_ro` it into the HOST mount ns at
     `/run/shrek/bench/<id>/grants/<leaf>` (rw/ro, always `noexec,nodev,nosuid`) — NOT a private ns like
     T2, because `dev`'s rootless podman is a separate process tree that must SEE the mount. Podman binds
     it at `/grants/<leaf>`. **USERNS = default rootless mapping (container-root ⇔ host-`dev`), not
     `keep-id`** — a grant dir is `dev`-owned by construction so a container-root workload reads/writes it
     and writes land back as `dev`; `keep-id` was empirically WRONG (it makes container-root a subuid that
     cannot write the `dev`-owned grant). An arbitrary image with a non-root `USER` needs an idmapped `-v`
     (deferred with the arbitrary-image story). **PROPAGATION INVARIANT (proven):** the relocate bind is
     only visible to podman's persistent pause mount-ns if `/` is `rshared` (real systemd default) — so
     the boot `reissue` unit must NOT `PrivateMounts`/`MountFlags=slave`. **Redirect-safe grant dir
     (mycelium #2982 hole 3):** the per-bench `<id>` and `grants` dirs are `root:dev 0710` — root owns
     them so `dev` can neither plant a symlink leaf inside `grants` nor `rename(2)` `grants`/`<id>` aside
     to redirect the root relocate bind onto a system target (e.g. `/etc`); `dev` (group `dev`) keeps the
     `--x` traverse its `podman -v` needs, and `other ---` preserves Fable fix-1 (no other unprivileged
     service follows the bind into `dev`'s home). Grants persist in the record and are re-materialized at
     boot by `shrek-bench-reissue.service` (`/run` is volatile).
   - **Egress grants** (`network <name> <profile>`): the profile is validated against sealed
     `shrek_policy::egress` (default-deny; `none` revokes) and recorded. Benches run `--network=none`; a
     networked `run` starts DETACHED, discovers the netns leader (`podman inspect`), verifies it is in a
     DISTINCT netns (pid-recycle guard, Fable fix 4), then root `gatekeeperd` `inject()`s the veth + sealed
     nft allow-list and re-verifies identity. The pre-spawn `create_and_inject()` is structurally
     unavailable to rootless podman. The start→inject window is fail-SAFE (zero egress until injection,
     never more than granted), so no rendezvous barrier is needed for a user-authority Bench. A networked
     bench binds its own sealed-profile `/etc/hosts` (coexists with `--no-hosts`; Shrek's `/etc/hosts`
     symlink #2816 is avoided). **net_plane hardening (Fable fix 3, shared with T1/T2):** a per-sandbox
     `input` hook drops all veth-sourced traffic so a bench cannot reach host-local listeners.
6. **Ship one offline Scratch seed** — a base image loaded via `podman load` from a tarball
   in the sysext (safer than `additionalimagestores` under merged `/usr`, which risks the
   kernel's overlay stacking-depth-2 limit — VERIFY before choosing).
   **✅ DONE & GREEN (2026-08-30, host oracle 33/0 + the sealed-boot BENCH/BENCH-SUP stages).**
   - **DELIVERY decided empirically** (`scratchpad/seed-derisk.sh`, real rootless podman 5.4.2): `podman
     load` of the sysext archive runs green end-to-end on the NATIVE `overlay` driver (`exit42` → rc 42,
     `ffmpeg -version` → rc 0); `additionalimagestores` under the already-overlayed merged `/usr` could
     not be cleanly validated (its failure mode — overlay-on-overlay stacking depth — is structural, not
     environmental) and is a version-coupled containers-storage layout, not a stable interchange format.
     `podman load` wins; `additionalimagestores`/composefs is filed as a later disk-dedup optimization for
     when per-user layer duplication actually hurts (it is NOT a launch gate).
   - **The seed is a real base**, not the `exit42` stand-in: `alpine`+`coreutils`+`ffmpeg` (the step-8
     media north-star) + the `exit42` helper baked in (so the seed IS the rule-2 proof image). Built by
     `scripts/build-bench-seed.sh` — base pinned by digest, every apk pinned by version, `podman save
     --format oci-archive` (~52M, vs ~133M docker-archive). The seed tar + its `.digest` sidecar are
     GITIGNORED build products (rebuilt on demand, the quickshell-staging posture — a 52M artifact that
     churns does not belong in git history); `exit42.tar` is removed, `exit42.elf` is retained (the
     committed rule-2 ELF + the host-side `noexec` negative-control artifact + the seed helper source).
   - **The product loader** is `bench_plane`'s `ensure_seed()` (called from `run`): `podman load`s the
     sysext archive into `dev`'s rootless store iff the image is absent OR stale. Staleness is DIGEST-keyed
     (the `.digest` sidecar records the built image Id; a mutable `localhost/scratch` tag would otherwise
     pin a user to the old image after an OS-shipped seed update). Best-effort + fail-open (no baked tar ⇒
     no-op, the oracle path). This is the one Rust change in step 6 → it bumps the system-index baseline.
7. **Constrained `.desktop` export** from a Bench via a `shrek-bench-run` wrapper
   (steal distrobox's export UX + the fixed-baked-key discipline from `shrek-menu`, never
   a path/command — same rule as the menu provider).
   **✅ DONE & GREEN (2026-08-30, host oracle 45/0 + the sealed-boot BENCH-EXPORT stage; Fable
   GO-WITH-FIXES, all 5 must-fixes folded).**
   - **The discipline:** `shrek bench export <name> <key> [--label L] [--icon I] -- <cmd…>` records the
     key→workload map in the **root-owned** record (only the privileged supervisor writes it) and writes a
     `.desktop` whose `Exec` is only `/usr/bin/shrek-bench-run <name> <key>` — two charset-validated tokens,
     **no command, no field codes**. `shrek-bench-run` (a compiled, baked wrapper) `env_clear`s and forwards
     just those tokens to `gatekeeperd bench run-export`, which resolves the key **server-side** against the
     record and runs it via the normal `run` path (grants apply). A forged/tampered `.desktop` can carry
     only a key — an unregistered one is refused — so it can inject no host command. `unexport`/`destroy`
     sweep the `.desktop`; the launcher (DMS + shrek-menu apps provider, #2827) surfaces it from
     `~/.local/share/applications`.
   - **Fable must-fixes folded:** (1) the `SHREK_BENCH_*` path overrides — the Bench trust anchor — are
     compiled OUT of the shipped image (a new `oracle-env` cargo feature; the sealed build ignores the
     environment entirely, so a redirected env crossing the sudo boundary can't point root `gatekeeperd` at
     a dev-writable records/anchor dir); (2) the `.desktop` is written/removed **as dev** (runuser), never
     root, so root never creates a file in a dev-controlled dir (symlink-redirect gadget); (3) the workload
     is stored argv-faithfully (`%`-escaped per arg, not space-joined); (4) the exact `.desktop` filename is
     recorded so sweeps delete precisely it (no `name`/`key` `-`-collision) and a cross-bench filename
     collision is refused; (5) a grant whose anchor-relative path has a **dot-leading component** (`~/.local`,
     `~/.config`, `~/.ssh`, …) is refused — else a workload could plant an *un*constrained `.desktop`.
8. **Prove a Media workflow E2E** (the north-star acceptance below).
   **✅ DONE & GREEN (2026-08-30, host oracle 55/0 + the sealed-boot BENCH-MEDIA stage). No Rust — the
   `run` verb already forwards an arbitrary workload; step 8 is an integration proof, not new code, so no
   `system-index` bump.**

### North-star acceptance

On a vanilla **offline** Shrek install, a user (or a dispatched agent) converts a video
inside a bundled Scratch/Media Bench after granting access to **only** the input +
destination dirs; the host stays sealed; execution is attributable to that Bench;
`destroy` removes all its tooling + mutable state. If that works, the Shrek interaction
model is proven — not just a container launcher.

**Proven — the exact flow.** `shrek bench create media` → `grant media <in> --ro` +
`grant media <out> --rw` → `run media -- ffmpeg -i /grants/<in>/clip.mp4 -c:v libvpx …
/grants/<out>/out.webm` → `destroy media`. What each proof surface asserts (both use the
**real shipped Scratch seed's** ffmpeg — no stand-in — and a real rootless-podman transcode,
the step-6 gotcha being a ROOT podman-in-docker nested-cgroup artifact, not the rootless path):
- **A real transcode runs *inside* the Bench** (rc 0) reading the **read-only** input grant and
  writing the **read-write** dest grant; the output **round-trips to the host owned by `dev`**.
- **It is a real video, not a husk** — the output is probed (`ffprobe`) to a decodable **VP8/webm**
  video stream.
- **The seed is loaded on demand** — the run's `ensure_seed` re-loads the offline seed from the
  sysext archive (the image is dropped first to force the vanilla-boot path).
- **The host stays sealed** — a write to the **read-only** input grant from inside the Bench is
  denied; only the explicitly-granted rw dest is mutable.
- **`destroy` removes the Bench's tooling + mutable state** (record + data dir + `/run` bench dir)
  **yet the delivered output persists on the host** — the user keeps what they asked for, the Bench
  keeps nothing.

The endpoint-free sealed VM proves the **pure offline path** on the actual image + baked seed (no
build, no network); the host oracle proves the same flow fast against the shipped seed tar (skipped
with a loud notice if the gitignored seed has not been built). With this, the Shrek interaction model
is proven end-to-end — **the ADR-003 Part 2 MVP is complete.**

## Consequences

- `onion-policy` gains `enable shrek-browser` and `enable shrek-apps` (Part 1 — **added,
  built, and merge-proven**, dogfood PASS=81/0); `enable shrek-bench` is **already added and
  proven** (Part 2 step 2 green). All are the same signed-sysext trust gate as `shrek-desktop`.
- **The sealed-`/etc` footprint (Bench-0 proof finding).** The proof empirically confirmed
  this ADR's core wrinkle: a sysext extends `/usr` only, so the container runtime's **entire
  `/etc` footprint must be baked into the sealed base** (`image/mkosi.postinst`), because
  runtime `/etc` is read-only and the sysext strips it. Baking, in order of what the proof
  hit: (1) `/etc/subuid` + `/etc/subgid` (rootless uid range); (2) `/etc/containers/policy.json`
  — **mandatory**, podman refuses every image op without a signature policy (MVP =
  `insecureAcceptAnything` for offline seeds; real pull-signature enforcement is a later
  egress/trust decision); (3) `/etc/containers/registries.conf` with
  `unqualified-search-registries = []` (offline seeds use fully-qualified `localhost/` names;
  matches default-deny egress). The runtime's `storage.conf`/`containers.conf` ride the sysext
  at `/usr/share/containers/` (podman reads there natively — no `/etc` needed for those).
- **Podman vs Shrek's `/etc/hosts` symlink (Bench-0 proof finding).** Shrek's `/etc/hosts` is
  a baked symlink → `/home/.shrek-system/hosts` (the `shrek-connect` design, #2816), which
  podman cannot read/rewrite when synthesizing a container hosts file → it aborts. Offline
  benches run `--network=none --no-hosts` (a hosts file is meaningless with no network). A
  **networked** bench (later, via `net_plane` late-attach) will need an explicit hosts answer
  (`--add-host` / a bench-owned hosts file) rather than Shrek's `/home`-symlinked one —
  flagged for the egress-grant slice.
- **cgroup manager:** the container lands under `dev`'s delegated systemd `user-1000.slice`
  (`Delegate=yes`) when a user session bus is present — proven. The Bench runtime keeps the
  default systemd manager; the graphical bench session always has the bus. (The proof's
  headless probe had to start `dbus.socket` explicitly; a real `shrek bench` launch won't.)
- The installer sysext's app duplication (firefox-esr, gparted, CLI utils) is *not*
  removed — the installer is a distinct throwaway environment; the point is those apps now
  also exist in the installed OS.
- **Deliverable-4 provability splits:** browser + a second app (Part 1) are **PROVEN this
  cycle** — a sealed-boot merge check (dogfood PASS=81/0) plus the grim-container render check
  (firefox-esr + gnome-text-editor draw real frames; VM screendump lies about GTK render, #2923).
  "Install one app via the chosen mechanism" (Part 2) is provable only when the `shrek-bench`
  runtime lands (MVP step 2+), by design — the owner chose ADR-first over a same-day AppImage demo.
- No system-index bump for Part 1 (packaging/config/manifest, zero Rust), nor for Part 2 step 3
  (config/systemd/shell). Part 2 step 4's `bench_plane.rs` + `bench_record.rs` + CLI **are** Rust, so
  they **DO** bump the graph baseline; Part 2 step 5 also touches Rust (`bench_plane.rs` grant/egress
  wiring + the `net_plane.rs` host-local input-drop), so it bumps too (refreshed post-merge). Part 2 step 6
  is mostly seed/config (a new `build-bench-seed.sh`, gitignored artifacts, repointed proofs) but adds a
  small `bench_plane.rs` `ensure_seed()` loader, so it bumps as well. Part 2 step 7 adds the export verbs +
  the `Export` model + a new `shrek-bench-run` crate + the `oracle-env` feature-gate + the dot-path grant
  denylist — all Rust, so it bumps too.

## Open questions — resolved by Fable review (2026-08-30)

1. **Rootless Podman on a sealed image:** ~~subuid/subgid location~~ **resolved** — bake into
   `/etc/subuid`+`/etc/subgid` via `mkosi.postinst` (sysext can't touch `/etc`); add `uidmap`
   (setuid `newuidmap`/`newgidmap`) to the layer. Kernel/cgroup posture *probably* fine
   (trixie defaults `unprivileged_userns_clone=1`; `libpam-systemd` + `dbus-user-session`
   give cgroup delegation) but **must be asserted** on the sealed boot (step 2.i), because
   `apparmor_restrict_unprivileged_userns` is one distro decision from breaking it.
2. **Storage driver:** **resolved → native rootless `overlay`** (trixie's 6.12 kernel does
   unprivileged overlay in userns); keep `fuse-overlayfs` in the layer strictly as fallback;
   `vfs` **forbidden** (inherits `noexec`, rule 2a). Assert `podman info` shows `overlay`
   (step 2.iii).
3. **The `bench_plane.rs` seam:** **resolved as feasible without touching T2**, but re-scoped
   from "sibling issuer" to "lifecycle supervisor" (see the paragraph after the four rules) —
   the real Part-2 work is the persistent-grant state model, not a CLI shim.
4. **`noexec` feasibility:** **resolved** — works *iff* the rootfs is a fresh overlay/
   fuse-overlayfs superblock (rule 2), proven with a real ELF + a host-side negative control
   (step 2.iii–iv). If it fails, the pre-approved fallback (`noexec` on granted data mounts
   only) activates — this does **not** concede a *global* host exec mount (#2829), only the
   graphroot.

### Still genuinely open (not blockers, flagged for their step)
- ~~Offline seed delivery: `podman load` tarball vs `additionalimagestores` (overlay
  stacking-depth-2 risk) — VERIFY at step 6.~~ **RESOLVED at step 6 (2026-08-30): `podman load` (proven
  green on native rootless overlay); `additionalimagestores` rejected (structural overlay-stacking-depth
  risk under merged `/usr`).** Backlog: `additionalimagestores`/composefs as a disk-dedup optimization
  (the shipped seed adds ~52M to every signed sysext `.raw` + every update download — the eventual
  argument to revive a shared read-only store), and pull-signature enforcement on the seed (MVP =
  `insecureAcceptAnything`).
- Linger/logout: benches-die-on-logout is the MVP posture; durable linger needs another
  `/home` redirect — revisit only if it bites.
- ~~`prjquota` on `shrek-data`: touches the installer format path — plan at step 3.~~ **RESOLVED at
  step 3 (2026-08-30):** `mkfs -O quota,project` in the installer + `home.mount prjquota` at initial
  mount + a pre-mount `tune2fs` retrofit for old disks (brick-safe). Proven EDQUOT-enforcing (PASS=83/0).
