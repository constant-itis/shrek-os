# Phase-1 S8 — rollback proof (automatic boot assessment)

> S8 gate (final Phase-1 milestone): *deliberately break an update → the system automatically
> returns to the last-good version, with no operator action.*
> This closes the base acceptance test: S6 proved a sealed image boots, S7 proved it updates,
> S8 proves a **bad** update is survivable. Together they break the ADR-001 tie toward **stay on
> Debian**.

## The mechanism: systemd Automatic Boot Assessment

We do not add a bespoke rollback engine — the whole loop is native to `systemd-boot` + `systemd`
on trixie (257), the same stack S7 already uses. Three cooperating pieces:

1. **Boot counting (the loader).** New UKIs are installed into the ESP with a try counter in the
   filename — `shrek_2_x86-64+3-0.efi` (`+<tries-left>-<tries-done>`). systemd-boot **decrements the
   left counter each time it launches that entry**, and prioritises entries with a non-zero counter.
   When an entry reaches `+0-N` it is marked *bad* and ordered after all non-bad entries. Our v1 UKI
   is written **without** a counter (`shrek_1_x86-64.efi`) → it is a permanent known-good entry, so
   it is the fallback the loader lands on when v2 goes bad. (Set by S7's `20-uki.transfer`:
   `TriesLeft=3`.)

2. **The success marker (blessing).** On a counted boot, `systemd-bless-boot-generator` sees the
   `LoaderBootCountPath` EFI variable and pulls in `systemd-bless-boot.service`. That service is
   `Requires=` + `After=boot-complete.target`, so it only runs — and only then strips the counter
   from the filename, making the update *permanent* — **once `boot-complete.target` is reached**.

3. **The health gate (ours).** `shrek-boot-health.service` sits in front of the success marker:

   ```ini
   [Unit]
   Before=boot-complete.target
   FailureAction=reboot
   [Install]
   RequiredBy=boot-complete.target      # baked as a .requires symlink in the sealed /usr tree
   ```

   Because it is ordered `Before` the target and the target `Requires` it, a **non-zero exit fails
   `boot-complete.target`'s job** → `systemd-bless-boot` never runs → the counter is never stripped.
   `FailureAction=reboot` then reboots the box so the loader decrements again on the next attempt.
   After the tries are exhausted, the loader falls back to v1. This is exactly the wiring of
   systemd's own `systemd-boot-check-no-failures.service`, plus greenboot's reboot-on-failure.

   The gate runs **only on counted boots** (`boot-complete.target` is not pulled into a non-counted
   boot), so the good v1 fallback boots straight through without the gate firing.

> In production `boot-health-check` is where Shrek's real greenboot-style checks live (control-plane
> liveness, verity intact, reachability…). For the spike the sole check is a **poison marker**.

## Breaking an update on purpose

`BREAK=1 scripts/build-in-container.sh <v>` stages `/usr/lib/shrek/boot-poison` into that version's
sealed root. `boot-health-check` exits non-zero whenever the marker is present, so the version is
unhealthy *by construction*. The marker is gitignored and cleared on every normal build, so an
ordinary `build-in-container.sh <v>` is always a healthy build. (No kernel/init sabotage needed, and
nothing about the *good* path is special-cased — a real broken update reaches the same gate by
failing a real check or by never reaching `boot-complete.target` at all, e.g. a panic/hang.)

## The proof (`scripts/rollback-proof.sh`)

```
1. build GOOD v1                     → out/shrek_1_x86-64.raw   (UKI shrek_1_x86-64.efi, NO counter)
2. BREAK=1 build BROKEN v2           → out/shrek_2_x86-64.*     (sealed root carries boot-poison)
3. offline A/B update v1's disk → v2 → out/shrek-updated.raw    (v2 in slot B + shrek_2_x86-64+3-0.efi)
4. boot out/shrek-updated.raw        → the rollback runs itself:
```

```
boot: systemd-boot picks v2 (newest, +3-0 → +2-1)  →  IMAGE_VERSION=2
      shrek-boot-health: POISON marker present — boot UNHEALTHY  →  FailureAction=reboot
   ↻ v2 (+2-1 → +1-2) → unhealthy → reboot
   ↻ v2 (+1-2 → +0-3) → unhealthy → reboot ; v2 now BAD
      systemd-boot falls back to v1 (non-counted, good)          →  IMAGE_VERSION=1  ← settles here
```

**Pass** = the final `/etc/issue` banner on the serial log reads `IMAGE_VERSION=1` (and the boot
carries v1's roothash): the deliberately broken v2 rolled back to v1 automatically. The script
prints the verdict from `out/vm-console.log`.

## Run it

```
scripts/rollback-proof.sh              # full: build v1 + broken v2 + update + boot + verdict
REBUILD=0 scripts/rollback-proof.sh    # reuse existing out/ artifacts, just update+boot+verdict
BUDGET=600 scripts/rollback-proof.sh   # widen the VM wall-clock for the multi-reboot cycle
```

## Notes / gotchas

- **Counting only applies under systemd-boot.** Booting a UKI directly (no loader) leaves
  `LoaderBootCountPath` unset → no counting, no blessing, no rollback. The VM boots through
  systemd-boot, so the EFI variable is present.
- **`/etc` is from the sealed root**, `/var` is volatile (`systemd.volatile=state`, S7). The
  `RequiredBy=boot-complete.target` edge is therefore baked as a `.requires` symlink under
  `/usr/lib/systemd/system/` rather than enabled into `/etc`, so it survives on the read-only image
  without a build-time `systemctl enable`.
- **`systemd-boot-check-no-failures.service`** (ships in `systemd-boot`, disabled by default) is the
  generic "any failed unit ⇒ unhealthy" gate and is worth enabling in the product; the spike uses the
  single deterministic Shrek gate to keep the proof unambiguous.
- The boot-assessment components need no extra package — `systemd` + `systemd-boot` +
  `systemd-boot-efi` + `systemd-container` (already installed) cover generator, bless service,
  `boot-complete.target`, and `bootctl`.
