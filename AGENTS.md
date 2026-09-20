# AGENTS.md — Lyra contributor guide for humans and coding agents

Lyra is a macOS hi-fi music player: a SwiftUI shell (`app/`) over a Rust core
(`crates/`). There is no `.xcodeproj`; the Makefile is the dev loop.
Read `README.md` (layout, security posture) and `BLUEPRINT.md` (per-layer
research, packaging, open spikes) before touching architecture.

## Setup

```sh
mise install          # installs project-local rust, cmake, ninja, mr-boxington per mise.toml
```

`mise.toml` pins the toolchain per-project (no global pollution). `cargo` is
shimmed through `mbx` (mr-boxington shared target cache); in CI the workflows
set `CARGO=cargo` so `rust-cache` can cache `target/` as a normal directory.

## Build

```sh
make                  # = make app: cargo build -p lyra-ffi → swiftc → .build/Lyra.app → ad-hoc codesign
make app PROFILE=release CARGO=cargo   # release variant used by .github/workflows/release.yml
make run              # build then open the app
make dev              # rebuild + kill running Lyra + relaunch (pseudo-HMR)
make check            # mise exec -- cargo check --workspace
```

Build details (see Makefile): `swiftc` compiles
`app/Sources/LyraApp/**/*.swift` with `-import-objc-header modules/CLyraFFI/lyra.h`,
links `app/.libs/liblyra_ffi.a`, then `codesign --force --sign -` with
`entitlements.plist`. Requires macOS + `xcrun` (Command Line Tools suffice;
full Xcode is only needed for `Assets.car` compilation — without it the build
warns and keeps going). `make clean` removes `.build`, `app/.libs`, and runs
`cargo clean`.

## Test

```sh
cargo test --workspace            # full suite (~150 unit/integration tests, green on macOS)
cargo test -p lyra-engine         # engine pipeline incl. real-device output tests
cargo test -p lyra-remote         # pairing e2e over loopback (crates/lyra-remote/tests/pairing.rs)
cargo test -p lyra-ipc            # IPC protocol + server/dispatcher tests
```

Device-dependent engine/HAL tests degrade gracefully (skip when no output
device), so the suite stays green on headless runners — this is asserted in
`.github/workflows/ci.yml`, which runs `cargo fmt --check`, then
`cargo clippy --workspace --all-targets -- -D warnings`, then
`cargo test --workspace` on `macos-latest`. Match that order locally:

```sh
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

## Dev workflow

1. Rust changes: edit under `crates/<name>/src`, run `cargo test -p <name>`
   for the fast loop; run the workspace suite before opening a PR.
2. Swift changes: edit `app/Sources/LyraApp/**`, then `make dev` to rebuild
   (Swift-only rebuilds take seconds) and relaunch. `make watch` auto-rebuilds
   on save via `watchexec` (installed through mise).
3. FFI changes: the C ABI lives in `crates/lyra-ffi` with the header at
   `modules/CLyraFFI/lyra.h`. Keep both sides in sync — Swift imports that
   header directly, so a Rust signature change without a header update breaks
   the `swiftc` step, not `cargo check`.
4. Agent-native control: the `lyra` CLI / `lyra-mcp` binaries
   (`crates/lyra-cli`) drive a live app over the `lyra-ipc` unix socket;
   `crates/lyra-remote/examples/remote_client.rs` is the reference client for
   the LAN pairing protocol. Prefer driving the app through these instead of
   GUI scripting when verifying behavior.
5. Release: tags `v*` (or manual dispatch with a `version` input) trigger
   `.github/workflows/release.yml` — version stamp via `plutil` on
   `Info.plist`, `make app PROFILE=release CARGO=cargo`, UDZO DMG via
   `hdiutil`, upload as artifact and optionally attach to a GitHub release.

## Project conventions

- **Rust owns hard state** (DB, engine, network); Swift renders. Never put
  SQL, decoding, or DSP in Swift — expose it through `lyra-ffi` or `lyra-ipc`.
- **Crate layout**: `crates/lyra-<domain>` with `src/lib.rs` as the boundary
  doc; each crate's module docs explain its seam (see `lyra-fs` ByteSource,
  `lyra-engine` decode→DSP→ring→output pipeline).
- **RT discipline** (engine/HAL/DSP): no alloc, lock, I/O, or panic in the
  audio callback — memcpy/gain only. Params cross via atomics/triple-buffer;
  EQ rebuilds happen on the worker via commands.
- **Sandbox posture**: `entitlements.plist` is a deny-list — grow it only when
  a feature demands it. No AppleEvents, no secrets in the bundle, no bearer
  credentials (remote auth is SPAKE2 pairing → pinned X25519 keys).
- **Error taxonomy**: `lyra-ipc` has a stable `ErrorCode` set
  (`crates/lyra-ipc/src/protocol.rs`) — match on `code`, never parse
  `message`. New fallible surfaces should reuse it.
- **Docs**: protocol behavior is specified in code (`OPERATIONS` table in
  `lyra-ipc`, handshake in `lyra-remote/src/lib.rs`) and projected to
  `docs/api/openapi.yaml`; incident runbooks live in `docs/runbooks/`.
  Update them when the behavior changes.
- **Commits/PRs**: follow `.github/pull_request_template.md`; file bugs and
  features with the templates in `.github/ISSUE_TEMPLATE/`.
