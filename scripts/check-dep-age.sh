#!/usr/bin/env bash
# Dependency cooling-off gate: every direct workspace dependency must have a
# pinned version at least MIN_DEP_AGE_DAYS old, so a freshly-published
# (possibly compromised) release cannot slip straight into the tree.
#
# Usage: scripts/check-dep-age.sh [MIN_DAYS]   (default: 7, or $MIN_DEP_AGE_DAYS)
#
# Sources: dependency names from the [workspace.dependencies] section of the
# root Cargo.toml, pinned versions from Cargo.lock, release dates from the
# crates.io API (https://crates.io/api/v1/crates/{name}/{version}).
# Workspace-internal lyra-* crates never appear in [workspace.dependencies],
# so there is nothing to exempt. A crates.io request that fails (offline CI,
# rate limit) is a warning, not a failure — the gate judges version age,
# not network weather — but the warning is loud so silent skips get noticed.
set -euo pipefail

MIN_DAYS="${1:-${MIN_DEP_AGE_DAYS:-7}}"
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
NOW="$(date +%s)"
FAIL=0
WARN=0

# Names listed under [workspace.dependencies] in the root Cargo.toml.
mapfile -t DEPS < <(awk '
  /^\[workspace\.dependencies\]/{in_section=1; next}
  /^\[/{in_section=0}
  in_section && /^[A-Za-z0-9_-]+ *=/ { sub(/ *=.*/, ""); print }
' "$ROOT/Cargo.toml")

if [ "${#DEPS[@]}" -eq 0 ]; then
  echo "dep-age: no dependencies found in [workspace.dependencies]; refusing to pass blind."
  exit 1
fi

for name in "${DEPS[@]}"; do
  version="$(awk -v n="$name" '
    /^name = "/{ gsub(/^name = "|\"$/, ""); current=$0; next }
    current == n && /^version = "/{ gsub(/^version = "|\"$/, ""); print; exit }
  ' "$ROOT/Cargo.lock")"
  if [ -z "$version" ]; then
    echo "dep-age: WARN: $name has no pinned version in Cargo.lock; skipping."
    WARN=1
    continue
  fi
  created="$(curl -sS --max-time 20 \
    -H 'User-Agent: lyra-dep-age-check (repository CI gate)' \
    "https://crates.io/api/v1/crates/${name}/${version}" \
    | python3 -c 'import json,sys
try:
    print(json.load(sys.stdin)["version"]["created_at"])
except Exception:
    print("")')"
  if [ -z "$created" ]; then
    echo "dep-age: WARN: could not fetch release date for ${name} ${version}; skipping."
    WARN=1
    continue
  fi
  created_epoch="$(date -j -f '%Y-%m-%dT%H:%M:%S' "${created%%.*}" +%s 2>/dev/null \
    || date -u -d "$created" +%s)"
  age_days=$(( (NOW - created_epoch) / 86400 ))
  if [ "$age_days" -lt "$MIN_DAYS" ]; then
    echo "dep-age: FAIL: ${name} ${version} is ${age_days}d old (< ${MIN_DAYS}d; released ${created})."
    FAIL=1
  else
    echo "dep-age: ok: ${name} ${version} is ${age_days}d old."
  fi
done

if [ "$FAIL" -ne 0 ]; then
  echo "dep-age: FAILED — one or more dependencies are younger than ${MIN_DAYS} days."
  exit 1
fi
if [ "$WARN" -ne 0 ]; then
  echo "dep-age: passed with warnings (some crates skipped; see WARN lines above)."
else
  echo "dep-age: passed — all ${#DEPS[@]} dependencies are at least ${MIN_DAYS} days old."
fi
