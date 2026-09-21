# Lyra

A hi-fi music player — SwiftUI shell on macOS, Rust core everywhere.
Built as a security-first reimagining of the BitMuse architecture (see
`BLUEPRINT.md` for the per-layer research and improvement deltas).

## Platforms

| | macOS | Linux |
|---|---|---|
| Rust core (`crates/`) — decode, DSP, store, engine, remote, IPC, CLI | ✔ | ✔ |
| UI | SwiftUI `.app` | `lyrad` headless host + `lyra` CLI |
| Audio out | cpal (CoreAudio) + `lyra-hal` exclusive IOProc | cpal (ALSA) |

The `.app` and `lyra-hal` are macOS-only — `lyra-hal` compiles to an empty
stub elsewhere and the engine's cpal compat path is the portable driver.
On Linux everything else builds and tests identically; `lyrad` plays the
host role the app plays on macOS (engine + IPC socket + torrents + remote),
so the `lyra` CLI and `lyra-mcp` drive a real player.

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

Linux (the headless host — no SwiftUI, drives like the app):

```sh
sudo apt install pkg-config libasound2-dev   # cpal output → ALSA
cargo build --workspace                      # or `make lyrad` for just the host
cargo run -p lyra-ffi --bin lyrad            # engine + IPC + store + torrents
cargo run -p lyra-ffi --bin lyrad -- --remote-port 9600   # + LAN remote
cargo test --workspace                       # same suite as macOS
```

Then from another shell: `lyra doctor`, `lyra scan --path ~/Music`,
`lyra play`, `lyra next` — the full CLI surface against the daemon.

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
