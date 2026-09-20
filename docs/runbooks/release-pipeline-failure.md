# Runbook: release-pipeline failure (`.github/workflows/release.yml`)

Covers the tag-push / manual-dispatch pipeline that stamps `Info.plist`,
runs `make app PROFILE=release CARGO=cargo`, builds a UDZO DMG with
`hdiutil`, and uploads/attaches it as a GitHub release.

## Symptoms

- GitHub Actions run for `release` is red on a `v*` tag push or a
  `workflow_dispatch` run.
- `gh release view vX.Y.Z` shows no DMG, or the `Lyra-X.Y.Z-dmg` artifact
  is missing / `if-no-files-found: error` tripped.

## Triage (in order)

### 1. Identify the failed step

```sh
gh run list --workflow release.yml --limit 5
gh run view <RUN_ID> --log-failed
```

### 2. Version-stamp failures (`plutil` step)

`plutil -replace CFBundleShortVersionString -string "$V" Info.plist` fails
when `Info.plist` is malformed or the `version` dispatch input is empty.

- Check the input: manual dispatches require `version` (e.g. `0.2.0`).
  For tag pushes the version derives from `GITHUB_REF_NAME#v`.
- Validate locally: `plutil -lint Info.plist`.
- The stamp mutates `Info.plist` in the runner only — never commit the
  stamped file; the workflow checks out clean each run.

### 3. Build failures (`make app PROFILE=release CARGO=cargo`)

- Reproduce on a Mac runner-equivalent host:
  `make app PROFILE=release CARGO=cargo`.
- Common causes:
  - Toolchain drift: the workflow installs via `jdx/mise-action@v3` from
    `mise.toml` (rust stable, cmake, ninja). A new stable rustc lint can
    break the build — pin or fix forward, never downgrade silently.
  - `Swatinem/rust-cache@v2` poisoning after a `Cargo.lock` change: clear
    the cache from the Actions UI and re-run.
  - Missing full Xcode: `actool`/`Assets.car` absence is a **warning**, not
    a failure (Makefile skips it). If the step fails elsewhere, look at the
    `swiftc` invocation and `codesign` lines, not the accent-color warning.
- `CARGO=cargo` bypasses the local `mbx` shim — do not "fix" CI by
  hardcoding `mbx`; the shim is dev-only by design.

### 4. DMG packaging failures (`hdiutil` step)

- `hdiutil create` fails if a previous `.build/dmg` layout leaked into the
  runner (it shouldn't — fresh checkout) or if `.build/Lyra.app` is absent
  because the build step silently produced nothing. Verify the `cp -R
  .build/Lyra.app .build/dmg/` source exists in the failed-run logs.
- `shasum` mismatch on re-runs: expected — each build re-signs ad-hoc.

### 5. GitHub release attach failures (`gh release create/upload`)

- Runs under `secrets.GITHUB_TOKEN` with `permissions: contents: write`.
  A 403 means the permission block was edited — restore it.
- `gh release create` failing with "already exists" on re-runs is handled
  by the `|| gh release upload --clobber` fallback; if both fail, check
  whether the tag was deleted/recreated (`git ls-remote --tags origin`).

## Recovery

1. Fix forward on `master` (docs-only fix) or fix the workflow input.
2. Re-run: `gh run rerun <RUN_ID>` for transient (cache/runner) failures;
   re-push the tag (`git tag -d` + `git push origin :refs/tags/vX.Y.Z`
   only if the release never published) or re-dispatch with
   `gh workflow run release.yml -f version=X.Y.Z -f create_release=true`
   for real failures.
3. Confirm: artifact present, `shasum -a 256` printed in the log, release
   page shows the DMG.

## Known limitations (not incidents)

- Ad-hoc signing means Gatekeeper shows "unidentified developer" on first
  open (right-click → Open). Developer ID + notarization needs Apple
  secrets — see BLUEPRINT.md §6. Do not treat the Gatekeeper warning as a
  pipeline failure.
