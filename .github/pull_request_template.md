# Pull request

## Description

<!-- What does this change and why? Link issues with Closes #N. -->

## Area

<!-- Check the layer(s) this touches. -->

- [ ] Rust core (`crates/…`)
- [ ] FFI boundary (`crates/lyra-ffi`, `modules/CLyraFFI/lyra.h`)
- [ ] SwiftUI shell (`app/`)
- [ ] Sandbox / entitlements (`entitlements.plist`)
- [ ] Build / release (Makefile, `.github/workflows/`)
- [ ] Docs only

## Testing

<!-- How was this verified? Match the CI order: fmt → clippy → tests. -->

```sh
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
# plus: make / make run, device-specific checks, `lyra` CLI probes…
```

- [ ] `cargo fmt --check` clean
- [ ] `cargo clippy --workspace --all-targets -- -D warnings` clean
- [ ] `cargo test --workspace` green (note any device tests that self-skipped)
- [ ] Manual verification: <!-- `make run` smoke, DAC/hog-mode check, pairing e2e… -->

## Context

<!-- Anything reviewers need: BLUEPRINT section, protocol changes
     (did `OPERATIONS`/PlayerCommand move? is docs/api/openapi.yaml updated?),
     screenshots for UI changes, follow-ups filed as issues. -->
