# Lyra

A hi-fi music player — SwiftUI shell on macOS, Rust core everywhere.
Built as a security-first reimagining of the BitMuse architecture (see
`BLUEPRINT.md` for the per-layer research and improvement deltas).

## Platforms

| | macOS | Linux |
|---|---|---|
| Rust core (`crates/`) — decode, DSP, store, engine, remote, IPC, CLI | ✔ | ✔ |
| UI | SwiftUI `.app` | `lyra-gui` (GTK4 + libadwaita) + `lyrad` headless host |
| Audio out | cpal (CoreAudio) + `lyra-hal` exclusive IOProc | cpal (ALSA) |

The `.app` and `lyra-hal` are macOS-only — `lyra-hal` compiles to an empty
stub elsewhere and the engine's cpal compat path is the portable driver.
Linux gets a native UI instead of a port of the SwiftUI shell: `lyra-gui`
(GTK4/libadwaita via gtk4-rs) calls the same engine/store crates in-process
and exposes the same IPC socket, so `lyra`/`lyra-mcp` and the E2E harness
treat both apps identically. `lyrad` plays the headless host role (CI,
remote boxes).

## Layout

```
crates/
  lyra-core     domain model — Track, AudioFormat, PlayerCommand/Event
  lyra-formats  probe/decode/tags — symphonia + lofty + ape-decoder + cue-rw
  lyra-store    SQLite/WAL library — FTS search, artwork, track_maps registry
  lyra-fs       byte sources — local + SSH (SFTP or exec fallback), rsync pin
  lyra-torrent  librqbit engine — download→import + stream-while-downloading
  lyra-search   lossless-first search — archive.org etree + Academic Torrents
  lyra-dsp      render-thread-safe DSP — biquad EQ, limiter (alloc-free)
  lyra-viz      spectrum/spectrogram/waveform/meters — draw-ready data
  lyra-engine   decode→DSP→ring→cpal output — plays any ByteSource
  lyra-hal      CoreAudio exclusive/hog output — macOS-only, empty stub elsewhere
  lyra-remote   axum HTTP+WS LAN remote — pairing auth, hashed tokens
  lyra-ipc      NDJSON unix-socket control — the `lyra` CLI / lyra-mcp surface
  lyra-coach    live input lane — onset/pitch/judge/follower + calibration
  lyra-map      offline song maps — grid/sections/chords/notes/tab → .lyramap
  lyra-net      last.fm / musicbrainz / lrclib / cover-art clients
  lyra-cli      `lyra` CLI + `lyra-mcp` — agent-native control of a live app
  lyra-ui       `lyra-gui` — GTK4/libadwaita desktop shell (Linux-native)
  lyra-ffi      staticlib C ABI → Swift (uniffi migration path in blueprint)
                also ships `lyrad`, the headless host binary (Linux/CI)
app/            SwiftUI shell — library, discover, coach, map, EQ, visuals,
                remote panes + menu-bar player + desktop pet
modules/        C module map + header for the FFI boundary
```

## Build

macOS (the `.app`):

```sh
mise install          # project-local rust (mise.toml), no global pollution
make                  # cargo build → swiftc → .build/Lyra.app → ad-hoc codesign
make run              # open it
make check            # cargo check --workspace
cargo test --workspace
```

The Makefile path is the dev loop: no `.xcodeproj` needed. Release packaging
(Developer ID, notarization, Sparkle inside-out signing) is in BLUEPRINT.md §6.

Linux (native GUI + headless host, drives like the app):

```sh
sudo apt install pkg-config libasound2-dev libgtk-4-dev libadwaita-1-dev
cargo build --workspace                      # or `make gui` / `make lyrad`
cargo run -p lyra-ui --bin lyra-gui          # GTK4 desktop app
cargo run -p lyra-ffi --bin lyrad            # headless: engine + IPC + torrents
cargo run -p lyra-ffi --bin lyrad -- --remote-port 9600   # + LAN remote
cargo test --workspace                       # same suite as macOS
make e2e                                     # IPC-driven E2E (see below)
```

Then from another shell: `lyra doctor`, `lyra scan --path ~/Music`,
`lyra play`, `lyra next` — the full CLI surface against either host.

## E2E

`scripts/e2e.sh` asserts the player contract over the IPC socket — scan →
FTS search → queue → transport — against whichever host is live:

- macOS: `make e2e` (builds and opens the real .app)
- Linux: `make e2e` runs it twice — `lyra-gui` under Xvfb and `lyrad`
  headless. Same asserts, same CLI.

CI runs this matrix on every PR (`e2e` job in `.github/workflows/ci.yml`).

## Security posture (deltas vs BitMuse)

- **Sandboxed**: app-sandbox + user-selected read-write + app-scope bookmarks +
  music-library read + network client/server + mic input (coach lane only).
  Nothing else.
- **No AppleEvents**: no `mount volume` AppleScript, no Finder shutdowns.
- **Memory-safe parsers**: decoders/taggers are Rust; the only C is libopus
  behind the official Symphonia adapter.
- **Remote auth is pairing, not a 4-digit PIN**: revocable device tokens,
  SHA-256 hashed at rest, constant-time compare, restart-surviving throttle.
  Credentials never appear in URLs.
- **No secrets in the bundle**: Last.fm keys injected at runtime; license
  validation via Lemon Squeezy key-is-credential API or Ed25519-signed licenses.
