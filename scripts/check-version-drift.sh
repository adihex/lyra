#!/usr/bin/env bash
# check-version-drift.sh — fail on dependency version drift.
#
# The workspace shares one Cargo.lock. Blanket-failing on ANY duplicate
# version would be permanently red: 50+ transitive third-party splits
# (e.g. thiserror 1.x required by an old transitive dep alongside our
# 2.x) cannot be unified from this repo. So this gate fails only on
# drift the workspace controls:
#   1. stale lockfile — a manifest changed without updating Cargo.lock;
#   2. duplicate versions of first-party `lyra-*` crates (a path
#      dependency split: two workspace members resolving different
#      versions of the same first-party crate);
#   3. growth in the third-party duplicate count above BASELINE_DUP_NAMES
#      (ratchet: a PR must not introduce NEW version splits; when a
#      `cargo update -p <pkg>` unifies some, lower the baseline here in
#      the same PR that updates Cargo.lock).
#
# Usage: bash scripts/check-version-drift.sh   (runs `cargo metadata`,
# so it needs the workspace checkout + a Rust toolchain, no network
# beyond what cargo itself needs).

set -euo pipefail

cd "$(dirname "$0")/.."

# Baseline grounded 2026-09-20: 658 locked packages, 58 names with >1
# version, all of them transitive third-party (none lyra-*, none
# unifiable from our manifests — verified by inspection, see below).
# Raised 58 → 61 on 2026-09-21: lyra-ui's gtk4-rs/gir macro chain pulls
# proc-macro-crate 1.x/2.x/3.x alongside toml_edit+winnow+heck+toml_datetime
# splits — transitive constraints, not unifiable from our manifests.
BASELINE_DUP_NAMES=61

fail=0
fail_msg() { echo "drift FAIL: $1"; fail=1; }

# 1. Lockfile in sync with manifests. Only cargo's explicit
# "--locked needs update" verdict counts as drift; any other cargo
# failure (e.g. no network for the registry index) is reported verbatim
# so the gate never lies about WHY it is red.
meta_err="$(cargo metadata --locked --format-version 1 2>&1 >/dev/null)" && meta_ok=1 || meta_ok=0
if [ "$meta_ok" -eq 1 ]; then
  echo "lockfile in sync: OK (cargo metadata --locked)"
elif grep -qi "needs to be updated" <<<"$meta_err"; then
  fail_msg "Cargo.lock is stale — run 'cargo update -w' (or 'cargo check') and commit the lockfile."
else
  fail_msg "could not verify lockfile freshness: $meta_err"
fi

# 2+3. Parse Cargo.lock deterministically (equivalent to
# `cargo tree --duplicates`, but stable to parse and offline-safe).
mapfile -t DUP_LINES < <(python3 - <<'EOF'
import collections, re
lock = open('Cargo.lock').read()
pkgs = re.findall(r'\[\[package\]\]\nname = "([^"]+)"\nversion = "([^"]+)"', lock)
by = collections.defaultdict(set)
for name, ver in pkgs:
    by[name].add(ver)
dups = {n: sorted(v) for n, v in by.items() if len(v) > 1}
print(f"TOTAL={len(pkgs)} DUPNAMES={len(dups)}")
for name in sorted(dups):
    print(f"{name} {' '.join(dups[name])}")
EOF
)

summary="${DUP_LINES[0]}"
total="${summary#TOTAL=}"; total="${total%% *}"
dupnames="${summary#*DUPNAMES=}"
echo "locked packages: $total, names with >1 version: $dupnames (baseline: $BASELINE_DUP_NAMES)"

first_party=0
while IFS= read -r line; do
  name="${line%% *}"
  case "$name" in
    lyra-*)
      fail_msg "first-party crate '$line' resolves to multiple versions"
      first_party=1
      ;;
  esac
done < <(printf '%s\n' "${DUP_LINES[@]:1}")

if [ "$first_party" -eq 0 ]; then
  echo "first-party (lyra-*) duplicates: none — OK"
fi

if [ "$dupnames" -gt "$BASELINE_DUP_NAMES" ]; then
  fail_msg "third-party duplicates grew $BASELINE_DUP_NAMES -> $dupnames; unify with 'cargo update -p <pkg>' or justify + raise the baseline."
else
  echo "third-party duplicate budget: OK ($dupnames <= $BASELINE_DUP_NAMES)"
fi

echo "--- duplicate versions (informational) ---"
printf '%s\n' "${DUP_LINES[@]:1}"

# Corroborate with cargo's own duplicate detection (informational only;
# the verdict above comes from the lockfile parse).
echo "--- cargo tree --duplicates (corroboration) ---"
cargo tree --workspace --duplicates --prefix none 2>/dev/null | sort -u | head -70 || true

if [ -n "${GITHUB_STEP_SUMMARY:-}" ]; then
  {
    echo "## Version drift check"
    echo ""
    echo "- Locked packages: \`$total\`"
    echo "- Names with >1 version: \`$dupnames\` (baseline \`$BASELINE_DUP_NAMES\`)"
    echo "- First-party duplicates: $([ "$first_party" -eq 0 ] && echo none || echo FOUND)"
    echo "- Verdict: $([ "$fail" -eq 0 ] && echo PASS || echo FAIL)"
  } >> "$GITHUB_STEP_SUMMARY"
fi

if [ "$fail" -ne 0 ]; then
  echo "version drift detected — see above."
  exit 1
fi
echo "version drift: PASS"
