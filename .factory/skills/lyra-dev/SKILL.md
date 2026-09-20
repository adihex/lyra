---
name: lyra-dev
description: Develop Lyra (macOS SwiftUI + Rust music player) — build/test/lint loop, FFI safety rules, and where things live.
---

# Lyra dev loop

Lyra has no `.xcodeproj`. The Makefile is the dev loop:
`cargo build -p lyra-ffi` → `swiftc` → `.build/Lyra.app` → ad-hoc codesign.

## Build / test / lint (run in this order, matches `.github/workflows/ci.yml`)

```sh
mise install
make                       # debug .app bundle at .build/Lyra.app
make dev                   # rebuild + relaunch running Lyra (Swift edits land in seconds)
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace     # ~150 tests; device tests self-skip headless
```

Scope to one crate for speed: `cargo test -p lyra-engine`,
`cargo test -p lyra-remote`, `cargo test -p lyra-ipc`.
Release parity check: `make app PROFILE=release CARGO=cargo` (what release.yml runs).

## Where things live

- `crates/lyra-core` — domain vocabulary: `Track`, `AudioFormat`, `PlayerCommand`/`PlayerEvent`.
- `crates/lyra-formats` — probe/decode/tags (symphonia + lofty).
- `crates/lyra-store` — SQLite/WAL library (FTS search, artwork); Rust owns all SQL.
- `crates/lyra-fs` — `ByteSource` seam: local files, SSH exec/SFTP, rsync pin.
- `crates/lyra-engine` — decode → DSP → ring → cpal output; tests prove realtime playback.
- `crates/lyra-hal` — CoreAudio exclusive/hog-mode output via the shared `OutputTap` contract.
- `crates/lyra-dsp` — alloc-free EQ/limiter; `crates/lyra-viz` — draw-ready spectrum data.
- `crates/lyra-remote` — LAN pairing (SPAKE2 → Noise XXpsk3 → pinned X25519, raw TCP).
- `crates/lyra-ipc` — NDJSON unix-socket protocol v2; op table is `OPERATIONS` in `src/protocol.rs`.
- `crates/lyra-cli` — `lyra` CLI + `lyra-mcp` agent control surface over the IPC socket.
- `crates/lyra-ffi` — C ABI (`lyra_*` fns) + `modules/CLyraFFI/lyra.h` header Swift imports.
- `app/Sources/LyraApp` — SwiftUI shell; `docs/runbooks/` — incident runbooks.

## FFI safety rules

1. Every `*const c_char` / `*const f32` crossing the boundary must be null-checked
   on the Rust side before dereference; every `*mut c_char` returned to Swift must
   be freed with `lyra_string_free` — grep for it when adding a string-returning fn.
2. Never let a Rust panic cross `extern "C"`: the IPC layer already catches panics
   at the boundary (`guarded_call` in `lyra-ipc/src/server.rs`) — mirror that pattern
   for new FFI entry points (catch_unwind → error string/code).
3. `lyra.h` and `lyra-ffi/src/lib.rs` change together. `cargo check` will not catch
   drift — only the `swiftc` step in `make` will, so always `make` after FFI edits.
4. No locks, alloc, or I/O on audio RT paths; keep engine callbacks to memcpy/gain.
   Rebuild EQ/DSP state on the worker thread via commands, never by shared mutation.
5. `unsafe` blocks need a one-line `// SAFETY:` comment stating the invariant
   (non-null, bounded len, correct thread) — follow the existing style in
   `crates/lyra-ffi/src/lib.rs` and `coach.rs`.

## Agent verification loop

Prefer driving a live app over scripting the GUI: use the `lyra` CLI
(`cargo run -p lyra-cli -- <cmd>`) against the IPC socket, and
`crates/lyra-remote/examples/remote_client.rs` for the pairing protocol.
After behavior changes, update `docs/api/openapi.yaml` if the method/command
surface moved, and add the matching `area:`/`P0-P3` labels when filing issues.
