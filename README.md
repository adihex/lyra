# Lyra

A macOS hi-fi music player — SwiftUI shell, Rust core. Built as a
security-first reimagining of the BitMuse architecture (see `BLUEPRINT.md`
for the per-layer research and improvement deltas).

## Layout

```
crates/
  lyra-core     domain model — Track, AudioFormat, PlayerCommand/Event
  lyra-formats  probe/decode/tags — symphonia + lofty + ape-decoder + cue-rw
  lyra-dsp      render-thread-safe DSP — biquad EQ, limiter (alloc-free)
  lyra-remote   axum HTTP+WS LAN remote — pairing auth, hashed tokens
  lyra-net      last.fm / musicbrainz / lrclib / cover-art clients
  lyra-ffi      staticlib C ABI → Swift (uniffi migration path in blueprint)
app/            SwiftUI shell (NSOpenPanel probe demo + remote toggle)
modules/        C module map + header for the FFI boundary
```

## Build

```sh
mise install          # project-local rust (mise.toml), no global pollution
make                  # cargo build → swiftc → .build/Lyra.app → ad-hoc codesign
make run              # open it
make check            # cargo check --workspace
```

The Makefile path is the dev loop: no `.xcodeproj` needed. Release packaging
(Developer ID, notarization, Sparkle inside-out signing) is in BLUEPRINT.md §6.

## Security posture (deltas vs BitMuse)

- **Sandboxed**: app-sandbox + user-selected read-write + app-scope bookmarks +
  network client/server. Nothing else.
- **No AppleEvents**: no `mount volume` AppleScript, no Finder shutdowns.
- **Memory-safe parsers**: decoders/taggers are Rust; the only C is libopus
  behind the official Symphonia adapter.
- **Remote auth is pairing, not a 4-digit PIN**: revocable device tokens,
  SHA-256 hashed at rest, constant-time compare, restart-surviving throttle.
  Credentials never appear in URLs.
- **No secrets in the bundle**: Last.fm keys injected at runtime; license
  validation via Lemon Squeezy key-is-credential API or Ed25519-signed licenses.
