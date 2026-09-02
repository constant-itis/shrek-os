#!/usr/bin/env bash
# Host proof for owner-account provisioning (#2939 — the first-boot owner-credential + display-name slice).
#
# Two gates, both WITHOUT the mkosi/docker image build and WITHOUT root or a VM:
#
#   A. SEED CORE — run the real helper (/usr/lib/shrek/shrek-owner-provision) via its SHREK_PROVISION_SEED_ONLY
#      seam against a synthetic /etc/shadow in a scratch dir, and assert the security-critical invariants:
#        - the spliced dev: line carries a VALID $6$ (SHA-512) crypt of the seeded passphrase (salt round-trip);
#        - every OTHER shadow line (root/shrek/swamp/polkitd) is byte-for-byte preserved (must-fix 5);
#        - the store dir is 0700 and the shadow file 0640 (must-fix 1 shape);
#        - the helper NEVER chowns the identity store to uid 1000 — the load-bearing property that stops a
#          uid-1000 unlink-replace of /etc/shadow (must-fix 1; contrast hosts-seed, which DOES chown to 1000);
#        - a re-run is idempotent (seed-once; the baked placeholder never clobbers a provisioned credential).
#
#   B. VARIANT MATRIX — via the SHREK_STAGE_ONLY seam in build-in-container.sh, assert the wizard is enabled
#      iff INSTALLABLE/DOGFOOD, off for LIVE_INSTALLER/plain, and non-interactive (baked seed) iff DOGFOOD.
#
# The bind-mount delivery over /etc/shadow and the runtime root:root/root:shadow chowns need root+a real fs;
# they are proven by the sealed-VM dogfood. This is the fast, root-free, build-free unit gate.
set -euo pipefail
REPO_ROOT="$(git rev-parse --show-toplevel)"; cd "$REPO_ROOT"

HELPER="image/overlay/usr/lib/shrek/shrek-owner-provision"
SERVICE="image/overlay/usr/lib/systemd/system/shrek-owner-provision.service"
PROFILE="image/overlay/etc/profile.d/50-shrek-owner.sh"
AUTOLOGIN="image/overlay/usr/lib/systemd/system/getty@tty1.service.d/autologin.conf"
TEMPLATE="image/owner-provision/getty-owner-provision.conf"
OP_DROPIN="image/overlay/etc/systemd/system/getty@tty1.service.d/50-owner-provision.conf"
OP_ENV="image/overlay/etc/shrek/owner-provision.env"
OP_SEED="image/overlay/etc/shrek/owner-seed"

PASS=0; FAIL=0
ok()  { echo "  PASS $*"; PASS=$((PASS+1)); }
bad() { echo "  FAIL $*"; FAIL=$((FAIL+1)); }

# The generated variant artifacts are gitignored build inputs; clear them so each stage starts clean and the
# proof leaves the tree byte-clean (mirrors nopasswd-variant-proof.sh).
clean() { rm -f "$OP_DROPIN" "$OP_ENV" "$OP_SEED"; return 0; }
trap clean EXIT

SCRATCH="$(mktemp -d)"; trap 'rm -rf "$SCRATCH"; clean' EXIT

# ======================================================================================================
echo "=== A. seed core (real helper, SHREK_PROVISION_SEED_ONLY, synthetic shadow, non-root) ==="

# A synthetic baked /etc/shadow mirroring image/mkosi.postinst's append order + a $6$ dev placeholder.
SRC="$SCRATCH/shadow.src"
cat > "$SRC" <<'EOF'
root:$6$rootsalt0$abcdefgROOThash0123456789abcdefABCDEF0123456789abcdefABCDEF01234:19700:0:99999:7:::
shrek:!*:19723:0:99999:7:::
dev:$6$Ylf7qC855/ESKbuC$5K7c15ydx9s3vgGnYOtXg5nfDi0NaUuSBfgIEJdN5ilgPab9h/i3jXUpJbHFPla6Leq9S5ZLrvfM2TnRzrECh.:20000:0:99999:7:::
swamp:!*:20000:0:99999:7:::
polkitd:!*:20000:0:99999:7:::
EOF

PW='oracle-test-passphrase'; NAME='Test Owner'
SEEDF="$SCRATCH/seed"; printf '%s\n%s\n' "$PW" "$NAME" > "$SEEDF"
IDENT="$SCRATCH/ident"

seed() {
  env SHREK_IDENTITY_DIR="$IDENT" SHREK_SOURCE_SHADOW="$SRC" \
      SHREK_PROVISION_MODE=noninteractive SHREK_PROVISION_SEED_FILE="$SEEDF" \
      SHREK_PROVISION_SEED_ONLY=1 sh "$HELPER" >/dev/null 2>&1
}

if seed; then ok "helper seed-core ran clean (seed-only, non-root)"; else bad "helper seed-core errored"; fi
STORE="$IDENT/shadow"

# $6$ crypt validity — extract the salt from the spliced dev hash and re-derive; must match exactly.
if [ -f "$STORE" ]; then
  H=$(grep '^dev:' "$STORE" | cut -d: -f2)
  case "$H" in
    '$6$'*)
      SALT=$(printf '%s' "$H" | cut -d'$' -f3)
      R=$(printf '%s' "$PW" | openssl passwd -6 -salt "$SALT" -stdin)
      [ "$R" = "$H" ] && ok "dev: line carries a valid \$6\$ crypt of the passphrase" \
                       || bad "dev: crypt does not verify against the passphrase"
      ;;
    *) bad "dev: line is not a \$6\$ hash (got: ${H:0:8}…)" ;;
  esac
else
  bad "store $STORE was not written"
fi

# Byte-preservation: every non-dev line identical between source and store.
if [ -f "$STORE" ]; then
  if diff <(grep -vE '^dev:' "$SRC") <(grep -vE '^dev:' "$STORE") >/dev/null; then
    ok "root/shrek/swamp/polkitd lines byte-for-byte preserved"
  else
    bad "non-dev shadow lines were altered"
  fi
  # And the dev line actually CHANGED (not a no-op).
  if [ "$(grep '^dev:' "$SRC")" != "$(grep '^dev:' "$STORE")" ]; then
    ok "dev: line was re-credentialed (differs from the baked placeholder)"
  else
    bad "dev: line unchanged — placeholder not replaced"
  fi
fi

# Store dir/file modes (must-fix 1 shape). Ownership is root:root/root:shadow at runtime (root-only, proven
# by the dogfood); here we assert the modes the helper sets as a non-root user.
if [ -d "$IDENT" ]; then
  m=$(stat -c '%a' "$IDENT");  [ "$m" = "700" ] && ok "store dir mode 0700" || bad "store dir mode=$m (want 700)"
fi
if [ -f "$STORE" ]; then
  m=$(stat -c '%a' "$STORE"); [ "$m" = "640" ] && ok "store shadow mode 0640" || bad "store shadow mode=$m (want 640)"
fi
[ -f "$IDENT/owner" ] && [ "$(head -n1 "$IDENT/owner")" = "$NAME" ] && ok "display name recorded in owner file" || bad "display name not recorded"
[ -f "$IDENT/provisioned" ] && ok "provisioned marker written (login gate keys on it)" || bad "provisioned marker missing"

# ADR-005 §5 display-name PRE-FILL (from the gate's validated owner_display_name):
VDIR="$SCRATCH/validated"; mkdir -p "$VDIR"; printf 'Manifest Owner' > "$VDIR/owner_display_name"
# (i) a BLANK seed display-name line adopts the validated manifest name.
SEED_BLANK="$SCRATCH/seed.blank"; printf '%s\n\n' "$PW" > "$SEED_BLANK"; IDENT_PF="$SCRATCH/ident.prefill"
env SHREK_IDENTITY_DIR="$IDENT_PF" SHREK_SOURCE_SHADOW="$SRC" SHREK_PROVISION_MODE=noninteractive \
    SHREK_PROVISION_SEED_FILE="$SEED_BLANK" SHREK_PROVISION_VALIDATED_DIR="$VDIR" SHREK_PROVISION_SEED_ONLY=1 \
    sh "$HELPER" >/dev/null 2>&1 || true
[ "$(head -n1 "$IDENT_PF/owner" 2>/dev/null)" = "Manifest Owner" ] && ok "pre-fill: blank seed name adopts the gate-validated owner_display_name" || bad "pre-fill from validated manifest name failed"
# (ii) an EXPLICIT seed name overrides the pre-fill.
IDENT_OV="$SCRATCH/ident.override"
env SHREK_IDENTITY_DIR="$IDENT_OV" SHREK_SOURCE_SHADOW="$SRC" SHREK_PROVISION_MODE=noninteractive \
    SHREK_PROVISION_SEED_FILE="$SEEDF" SHREK_PROVISION_VALIDATED_DIR="$VDIR" SHREK_PROVISION_SEED_ONLY=1 \
    sh "$HELPER" >/dev/null 2>&1 || true
[ "$(head -n1 "$IDENT_OV/owner" 2>/dev/null)" = "$NAME" ] && ok "pre-fill: an explicit seed name overrides the manifest default" || bad "explicit seed name did not override the pre-fill"
# (iii) NO manifest name + blank seed => falls back to the account name (dev), not empty.
IDENT_FB="$SCRATCH/ident.fallback"
env SHREK_IDENTITY_DIR="$IDENT_FB" SHREK_SOURCE_SHADOW="$SRC" SHREK_PROVISION_MODE=noninteractive \
    SHREK_PROVISION_SEED_FILE="$SEED_BLANK" SHREK_PROVISION_VALIDATED_DIR="$SCRATCH/nonexistent" SHREK_PROVISION_SEED_ONLY=1 \
    sh "$HELPER" >/dev/null 2>&1 || true
[ "$(head -n1 "$IDENT_FB/owner" 2>/dev/null)" = "dev" ] && ok "pre-fill: no manifest name falls back to the account name" || bad "no-manifest fallback is not the account name"

# NEGATIVE case (must-fix 1) — a uid-1000 unlink-replace of the store must be impossible. Non-root, we prove
# this as the invariant that makes it so: the helper sets the store dir root:root 0700 at runtime and NEVER
# chowns it to the owner/uid-1000 (unlike hosts-seed). A root:root 0700 dir denies uid≠0 all write, so a
# non-owner cannot unlink the shadow file within it.
if grep -Eq 'chown[[:space:]]+0:0[[:space:]]+"\$IDENT_DIR"' "$HELPER"; then
  ok "helper chowns the store dir to root:root (0:0) at runtime"
else
  bad "helper does not chown the store dir to root:root"
fi
# No chown of the identity store to a non-root owner ANYWHERE (the hosts-seed anti-pattern that would open
# the escalation). Strip full-line comments first so prose mentioning "chown"/"chowns" is not miscounted;
# every real chown invocation must target root (0:0 or 0:42=root:shadow).
CHOWN_CMDS=$(grep -vE '^[[:space:]]*#' "$HELPER" | grep -nE '\bchown\b' || true)
if printf '%s\n' "$CHOWN_CMDS" | grep -Evq 'chown[[:space:]]+0:(0|42)\b' && [ -n "$CHOWN_CMDS" ]; then
  echo "    offending chown line(s):"; printf '%s\n' "$CHOWN_CMDS" | grep -Ev 'chown[[:space:]]+0:(0|42)\b' | sed 's/^/      /'
  bad "helper chowns the store to a non-root owner (uid-1000 escalation risk)"
else
  ok "helper NEVER chowns the store to a non-root owner (no uid-1000 unlink-replace path)"
fi
# Live EACCES check when the oracle happens to run as root (bonus; skipped in the normal non-root run).
if [ "$(id -u)" = 0 ]; then
  chown 0:0 "$IDENT"; chmod 0700 "$IDENT"
  if setpriv --reuid 1000 --regid 1000 --clear-groups sh -c "rm -f '$STORE'" 2>/dev/null; then
    bad "LIVE: uid-1000 unlinked the store (root:root 0700 not enforced?)"
  else
    ok "LIVE: uid-1000 unlink of the store denied by root:root 0700"
  fi
fi

# Idempotence: a second run (marker now present) must not re-seed — the store content is unchanged.
BEFORE=$(cat "$STORE")
seed || true
AFTER=$(cat "$STORE")
[ "$BEFORE" = "$AFTER" ] && ok "re-run is idempotent (seed-once; content unchanged)" || bad "re-run mutated the store (not seed-once)"

# ======================================================================================================
echo
echo "=== B. variant matrix (wizard enabled iff INSTALLABLE/DOGFOOD; non-interactive seed iff DOGFOOD) ==="

stage() { clean; env "$@" SHREK_STAGE_ONLY=1 bash scripts/build-in-container.sh 1 >/dev/null 2>&1 || return 1; }
present() { [ -e "$1" ] || [ -L "$1" ]; }

echo "--- LIVE_INSTALLER=1 ---"
stage LIVE_INSTALLER=1 || bad "LIVE_INSTALLER staging errored"
present "$OP_DROPIN" && bad "LIVE_INSTALLER: owner-provision must be OFF (ownerless live session keeps public shrek)" || ok "LIVE_INSTALLER: no owner-provision drop-in"
present "$OP_ENV"    && bad "LIVE_INSTALLER: no owner-provision env expected" || ok "LIVE_INSTALLER: no env"
present "$OP_SEED"   && bad "LIVE_INSTALLER: no baked seed expected"          || ok "LIVE_INSTALLER: no seed"

echo "--- DOGFOOD=1 ---"
stage DOGFOOD=1 || bad "DOGFOOD staging errored"
present "$OP_DROPIN" && ok "DOGFOOD: owner-provision drop-in staged" || bad "DOGFOOD: owner-provision drop-in MISSING"
present "$OP_DROPIN" && grep -q '^Requires=shrek-owner-provision.service' "$OP_DROPIN" && ok "DOGFOOD: getty Requires= the oneshot (fail-closed)" || bad "DOGFOOD: getty drop-in missing Requires="
present "$OP_ENV" && grep -q '^SHREK_PROVISION_MODE=noninteractive' "$OP_ENV" && ok "DOGFOOD: env = non-interactive seed" || bad "DOGFOOD: env not non-interactive"
if present "$OP_SEED"; then
  ok "DOGFOOD: baked test seed present"
  [ "$(sed -n 1p "$OP_SEED" | wc -c)" -ge 9 ] && ok "DOGFOOD: seed passphrase is >=8 chars" || bad "DOGFOOD: seed passphrase too short"
  [ -n "$(sed -n 2p "$OP_SEED")" ] && ok "DOGFOOD: seed carries a display name" || bad "DOGFOOD: seed has no display name"
else
  bad "DOGFOOD: baked test seed MISSING"
fi

echo "--- INSTALLABLE=1 (the product) ---"
stage INSTALLABLE=1 || bad "INSTALLABLE staging errored"
present "$OP_DROPIN" && ok "INSTALLABLE: owner-provision drop-in staged (blocking wizard before desktop)" || bad "INSTALLABLE: owner-provision drop-in MISSING"
present "$OP_ENV" && grep -q '^SHREK_PROVISION_MODE=interactive' "$OP_ENV" && ok "INSTALLABLE: env = interactive wizard" || bad "INSTALLABLE: env not interactive"
present "$OP_SEED" && bad "INSTALLABLE: MUST NOT bake a seed (no public owner passphrase in the product)" || ok "INSTALLABLE: no baked seed"

echo "--- plain (no flags / desktop-sealed-proof default) ---"
stage || bad "plain staging errored"
present "$OP_DROPIN" && bad "plain: owner-provision must be OFF (no writable /home)" || ok "plain: no owner-provision drop-in"
present "$OP_ENV"  && bad "plain: no env expected"  || ok "plain: no env"
present "$OP_SEED" && bad "plain: no seed expected" || ok "plain: no seed"

# ======================================================================================================
echo
echo "=== C. structural + regression guards ==="
sh -n "$HELPER" && ok "helper parses (sh -n)" || bad "helper failed sh -n"
# The helper is a systemd ExecStart target — a non-executable mode fails the unit at start (getty then
# fails its Requires= and the whole dev session cascade fails). Guard the exec bit here, cheaply.
[ -x "$HELPER" ] && ok "helper is executable (ExecStart can run it)" || bad "helper is NOT executable — ExecStart will fail the unit"

# Service: shipped, inert-by-default (no [Install]), ordered before getty, mount ns NOT isolated.
present "$SERVICE" && ok "oneshot unit ships in the overlay" || bad "oneshot unit missing"
grep -q '^\[Install\]' "$SERVICE" && bad "oneshot must have NO [Install] (enablement is the getty drop-in only)" || ok "oneshot has no [Install] (variant-gated via getty Requires=)"
grep -q '^Before=getty@tty1.service' "$SERVICE" && ok "oneshot ordered Before=getty@tty1.service (wizard owns tty1 first)" || bad "oneshot not ordered before getty@tty1"
grep -qE '^(PrivateMounts|MountFlags)=' "$SERVICE" && bad "oneshot isolates its mount ns — the /etc/shadow bind would not reach PAM" || ok "oneshot does not isolate the mount ns (bind reaches PAM)"

# must-fix 9: the autologin drop-in no longer claims dev's password is locked.
grep -q 'password is locked' "$AUTOLOGIN" && bad "autologin.conf still says 'password is locked' (stale — it is UNLOCKED)" || ok "autologin.conf stale 'password is locked' comment fixed"

# ADR-005 §5: the display-name-in-PS1 behavior is REMOVED — bash PS1 does command substitution
# (promptvars default-on) and the name is not shell-safe (the sanitizer strips control/CSI/colon but not
# $ ` \), so a name like $(cmd) would execute at every prompt. The name renders in Quickshell ONLY now.
[ ! -e "$PROFILE" ] && ok "PS1 owner-name injection removed (50-shrek-owner.sh gone)" || bad "50-shrek-owner.sh still present (PS1 injection vector)"
# Belt-and-braces: no profile.d script interpolates the owner name into a prompt.
if grep -rl 'shrek-identity/owner' image/overlay/etc/profile.d/ 2>/dev/null | while IFS= read -r f; do grep -q 'PS1' "$f" && echo "$f"; done | grep -q .; then
  bad "a profile.d script still injects the owner display name into PS1"
else
  ok "no profile.d script interpolates the owner name into PS1"
fi

# Template + payload gate + base package.
visudo_ok=1; present "$TEMPLATE" && grep -q 'Requires=shrek-owner-provision.service' "$TEMPLATE" && ok "getty drop-in template Requires= the oneshot" || bad "drop-in template missing/incomplete"
grep -q 'owner-provision-not-enabled' scripts/build-installer-payload.sh && ok "payload gate asserts the product ships the wizard enabled" || bad "payload gate has no owner-provision assertion"
grep -q 'dogfood-owner-seed-PRESENT' scripts/build-installer-payload.sh && ok "payload gate rejects a DOGFOOD seed in the product" || bad "payload gate does not reject a baked seed"
grep -qE '^\s*openssl\b' image/mkosi.conf && ok "openssl added to the base image (runtime \$6\$ crypt)" || bad "openssl not in base mkosi.conf"

echo
echo "=== owner-provision proof: PASS=$PASS FAIL=$FAIL ==="
[ "$FAIL" -eq 0 ]
