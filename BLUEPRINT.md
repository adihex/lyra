# Lyra — Blueprint

Build a BitMuse-class macOS hi-fi player, fixing every layer's weakness.
Shell: **SwiftUI**. Core: **Rust**. Research date: 2026-09-15 (6 parallel
primary-source passes: formats, audio-out/DSP, remote security,
sandbox/packaging, data/ML, competitive UX).

## Principles

1. **Memory-unsafe code lives behind seams or not at all.** All decoders/taggers
   are Rust; the only C in the tree is libopus (adapter seam) and bundled DST
   (optional feature).
2. **Sandboxed by default** — the incumbent wasn't. Entitlements are a deny-list
   we grow only when a feature demands it.
3. **No bearer secrets, ever.** Remote auth is pairing → pinned keys → mutually
   authenticated encrypted transport. Nothing reusable to sniff or replay.
4. **No secrets in the bundle.** License API keys are the credential themselves;
   Last.fm keys arrive at runtime; offline licensing = Ed25519 verify-only pubkey.
5. **Rust owns the hard state** (DB, engine, network). Swift renders. One schema
   authority, one migration system.

## Layer-by-layer stack

| Layer | BitMuse (incumbent) | Lyra | Delta |
|---|---|---|---|
| Language split | Swift monolith + C/C++ parsers in-process | SwiftUI shell / Rust core via FFI | parsers can't corrupt the process |
| Container | **no sandbox** | app-sandbox + bookmarks + net client/server | parser bug ≠ full user compromise |
| Decode | FLAC/ogg/opus/vorbis/wavpack C frameworks | symphonia 0.6 (+wavpack crate, opus adapter) | safe Rust; same coverage minus APE/DSF |
| APE/DSF | bundled | ape-decoder · dsd-reader/ndsd-read (+dst-decoder opt) | new crates — validate on real library early |
| Tags R/W | TagLib (C++, static) | lofty 0.24 (+audex for DSF/DFF until lofty#638) | memory-safe tag writes |
| CUE | in-house | cue-rw + encoding_rs | strict+permissive modes, encoding handling |
| Loudness | in-house scanner | ebur128 (EBU Tech 3341/3342-verified) | standard-conformant |
| Audio out | CoreAudio via Swift | coreaudio (IOProc/hog/phys-format) or coreaudio-rs+objc2; cpal fallback path | bpplay is the mechanical port target |
| DSP | in-house | biquad + bs2b + rubato + M/S + TPDF dither + limiter | proven building blocks |
| Remote | plain HTTP/WS, 4-digit PIN | SPAKE2 pairing → pinned Ed25519 → Noise XX (`snow`) over WS/TCP; mDNS via mdns-sd | see threat table |
| Throttle | per-IP, in-memory | persistent (redb/SQLite), global bucket + IP, exponential backoff, survives restart | brute-force actually stops |
| Persistence | GRDB (Swift-side) | rusqlite bundled (FTS5 built in) + rusqlite_migration, Rust-owned | single writer, single schema |
| Similarity | OpenL3 + PANNs .mlpackage (CoreML) | ort + ONNX: Discogs-EffNet (similarity), EfficientAT mn10 (tagging); CLAP for text-queries later; sqlite-vec KNN in same .db | better models, one file |
| Metadata net | URLSession calls | reqwest+rustls; MusicBrainz 1rps governor; LRCLIB get-cached bulk path; CAA thumbnail endpoints + disk cache | correct rate-limit citizenship |
| Licensing | Lemon Squeezy, key in Keychain | same API (no secret needed — key is credential) + optional Ed25519-signed offline licenses | verify-only pubkey in binary |
| Updates | Sparkle 2.9 + EdDSA | Sparkle 2.x sandboxed: `SUEnableInstallerLauncherService` + `-spks`/`-spki` mach-lookup | sandboxed updater |
| FFI | n/a (all Swift) | hand C ABI now → uniffi (proc-macro) for the broad API | async + callback interfaces |
| Build | Xcode + DerivedData | Makefile (no xcodeproj dev loop) → XcodeGen for release | reproducible, CLT-only |

## § Formats — coverage map

| Format | Decode | Tag R/W | Note |
|---|---|---|---|
| FLAC / WAV / AIFF | symphonia | lofty | done |
| MP3 | symphonia (gapless) | lofty | done |
| M4A AAC/ALAC | symphonia isomp4 | lofty | **verify gapless**: isomp4 isn't marked gapless — honor iTunSMPB/edit-list |
| Ogg Vorbis | symphonia | lofty | done |
| Opus | symphonia ogg demux + libopus adapter | lofty | no mature pure-Rust decoder; `opus-rs` watch |
| WavPack | symphonia-codec-wavpack 0.1.1 | lofty | new; no compressed-DSD/.wvc yet |
| APE | ape-decoder 0.3.2 | lofty / ape-decoder | newest decoder — fuzz real files early |
| DSF/DFF | dsd-reader / ndsd-read (+dst-decoder) | audex (lofty#638 pending) | DoP pack ~30 lines, hand-roll |
| CUE | — | cue-rw | encoding_rs for Shift-JIS/GBK sheets |

## § Audio output & DSP

Pipeline: **decode thread → DSP thread → HeapRb → RT callback = memcpy only.**

- Output driver trait `AudioDevice` with two impls:
  - *Audiophile*: `coreaudio` 0.2.2 (new, single-author — pin + wrap) or
    `coreaudio-rs` helpers + raw `objc2-core-audio` IOProc. Hog mode
    (toggle semantics), device rate switching (async — wait for RateListener),
    integer physical format, DoP passthrough (all DSP bypassed, bit-exact).
  - *Compat*: cpal shared-mode f32 at device rate (rubato always on).
- RT rules: no alloc/lock/IO/panic in callback (`assert_no_alloc` in debug),
  params via `triple_buffer`/atomics, aux threads via `audio_thread_priority`.
- Prior art to clone: **bpplay** (single-file C — hog, integer format, IOProc
  memcpy, mlock'd track RAM, DoP256). MPD's OSXOutputPlugin for DoP/hog quirks.
- Rate changes between tracks only; gapless within same rate — same
  constraint bpplay/MPD have.
- DSP chain order: gain(RG/preamp) → parametric EQ (biquad, DF1 retune) →
  bs2b crossfeed → M/S widen → rubato (bypass if rate matches) → limiter →
  TPDF dither (integer path only) → int pack / DoP pack.
- Sandbox + hog mode: works (Colibri/YM Pro ship sandboxed on MAS) — spike first.

## § Remote control — the fix

Threat model vs incumbent:

| Their weakness | Our fix |
|---|---|
| 4-digit PIN bearer | SPAKE2 (RFC 9382) pairing — one online guess per session, nothing offline |
| plaintext HTTP/WS | Noise_XX_25519_ChaChaPoly_SHA256 (`snow`), forward secrecy |
| PIN in `?pin=` URLs | secrets never on the wire; URLs carry no credentials |
| per-IP memory throttle | PAKE 1-guess + persistent counters + global bucket + backoff |
| no MITM check | host pubkey fingerprint in QR + optional 8-char SAS |
| no revocation | device table: name, pinned key, paired_at, revoke |
| replayed frames | transport nonces + command timestamps |

Flow: mDNS `_lyra._tcp` advertises `id`+pubkey-hash → phone scans QR (or types
6-digit code shown on Mac) → SPAKE2 → encrypted channel → exchange Ed25519
device keys → pin. Reconnects = Noise XX against pinned statics. Revocation =
delete key. Discovery is an untrusted hint; identity comes from keys.

Crates: axum 0.8 (ws) · snow 0.10 · spake2 0.4 · mdns-sd 0.17 · redb (counters
+ devices) · governor (coarse DoS only) · ed25519-dalek · hkdf · subtle · zeroize.

macOS notes: bind `[::]`; Sequoia+ Local Network permission needs
`NSLocalNetworkUsageDescription`; app firewall keys on signing DR — ad-hoc
re-prompts each build, dev identity signing fixes. Web-page client can't do
Noise/mTLS — if we want a browser remote, SPAKE2→session-token on WSS instead.

## § Sandbox & packaging

Entitlements (in `entitlements.plist`): app-sandbox · files.user-selected.rw ·
bookmarks.app-scope · network.client · network.server. Later, when Sparkle
lands: `SUEnableInstallerLauncherService` + mach-lookup `-spks`/`-spki`.

Deliberately absent: automation.apple-events (NSWorkspace.selectFile covers
reveal-in-Finder; bookmarks cover network mounts — no `mount volume`
AppleScript, no Finder restarts).

- Bookmarks without `.securityScopeAllowOnlyReadAccess` for tag-write folders;
  handle staleness; counted start/stopAccessing pairs.
- XPC parser isolation: optional. Rust kills the corruption class; XPC adds
  crash isolation only (fd-passing pattern if ever needed).
- FFI: hand C ABI now; uniffi (proc-macro, sync callbacks — async foreign
  traits broken under Swift 6 mode, uniffi#2929) when the surface broadens.
- Release: XcodeGen → inside-out Sparkle signing (never `--deep`) →
  notarytool → stapler → DMG → EdDSA `sign_update`.
- Dev loop: this Makefile — works on CLT alone, produces sandboxed .app.

## § Remote libraries — SSH (lyra-fs)

Files live on remote boxes (adi-linux, jiopc — tailnet sshd, no extra daemons).
Design: **pull bytes over SSH, decode locally** — the decoded stream is
bit-exact with the source file. No transcode, no macFUSE/sshfs, no SMB setup.

```text
remote path ──ByteSource──▶ CachingSource ──MediaSource──▶ symphonia ─▶ DSP
  (ssh dd / sftp / russh)    1MiB LRU + read-ahead          (Read+Seek)
```

- `ByteSource` seam: `LocalFile` now; `SshExecFile` (v0: `ssh host stat/dd`,
  works with config aliases today); next: `openssh-sftp-client` multiplexed
  channel (dev path — inherits `~/.ssh/config`), then `russh` (feature-gated)
  for the sandboxed build — a bookmark-granted key file, since a spawned
  /usr/bin/ssh can't read `~/.ssh` inside the sandbox.
- `CachingSource`: 1 MiB blocks, 256 MiB LRU, sequential read-ahead — a track
  is fetched once, gapless prefetch rides the same mechanism (pre-open next
  track's blocks while current plays).
- `scan(host, root)`: `ssh host find -printf` enumeration — remote-side walk,
  avoids per-file SFTP stat storms; lazy tag reads via ranged fetch (lofty
  only needs head/tail chunks).
- `pin(host, dir)`: rsync `-a --partial` to local cache — offline copies with
  delta/resume/checksums. rsync is the *offline* primitive, not playback.
- Later: `lyra-agent` static binary on the remote → scan+tag+serve in one
  protocol (turns each box into a library server; also fixes jiopc-class
  constrained hosts where dd-per-block is wasteful).

Security notes: russh path means no `~/.ssh` read inside the sandbox (key
bytes come via bookmarked file); exec path relies on user's ssh config
(ProxyJump/ports just work); host-key verification still applies (russh
`check_server_key` — pin per-host on first connect, TOFU).

## § Torrent sources (lyra-torrent)

Same seam as SSH: a torrent file is a `ByteSource`. `librqbit` 8.x (pure
Rust — DHT, magnets, per-file selection, streaming `FileStream`).

- **Download → import** (default): completes into a managed dir, normal
  scanner imports as local files. Per-file selection for multi-file album
  torrents.
- **Stream-while-downloading**: `TorrentFileSource` wraps rqbit's
  `FileStream` (AsyncRead+AsyncSeek) — pieces fetch on demand in read
  order, `CachingSource` absorbs seek latency. Play starts in seconds,
  not when the swarm finishes.
- **Seeding policy**: seed-while-downloading default; ratio/time caps and
  private-tracker seeding are settings, not defaults.
- **Legality/privacy**: intended for netlabels, etree-style taper archives,
  CC/PD and artist-distributed content. DHT exposes the swarm IP — offer a
  tracker-only mode; note it in first-run copy.

## § Data / ML / integrations

- **rusqlite `bundled`** (SQLite 3.53, FTS5 built in) + `rusqlite_migration`;
  one writer conn + read conns, WAL. Swift never writes SQL — it queries via
  FFI and subscribes to update-hook change events.
- **sqlite-vec** `vec0` virtual table for embeddings — ~68ms brute-force KNN at
  100k×384d; exact results, same file. `hnsw_rs` behind a trait if outgrown.
- **Models** (`ort` + CoreML EP): Discogs-EffNet ONNX for "similar tracks"
  (contrastive, purpose-built); EfficientAT mn10 for tagging (PANNs successor);
  `larger_clap_music` if we add text/vibe queries. Shared mel-spectrogram stage.
- **Last.fm**: token→browser→`auth.getSession` (md5 api_sig — still the spec);
  session key in Keychain; batch ≤50 scrobbles; >30s & ≥half-track rules.
- **MusicBrainz**: hard 1 rps/IP — shared `governor` limiter, real UA string,
  cache MBIDs. **CAA**: 307→IA, use `front-500`, disk-cache forever.
  **LRCLIB**: `/api/get-cached` for bulk scans, honor 429 `Retry-After`.
- **Licensing**: Lemon Squeezy key-is-credential (verify `store_id`/`product_id`
  in meta), instance_id persisted, offline-grace cache. Optional tier-B:
  Ed25519-signed license JSON from a tiny endpoint — pubkey in binary verifies
  offline (same pattern as Sparkle's SUPublicEDKey).

## § UX — what the competitive scan says to build

Parity assumed for the incumbent's feature set. Ranked improvements:

1. **Flagship remote**: full library browse + queue edit + DSP presets on the
   phone; NowPlaying Remote Media Sessions (WWDC26) → lock-screen integration.
2. **Parallel resumable indexer**: per-folder progress, FSEvents watch, mtime
   incremental, graceful offline-volume state. (Audirvāna's 13h serial rescan
   is the anti-benchmark; Roon does ~5min parallel.)
3. **Signal-path transparency UI**: live chain diagram file→decode→DSP→out,
   colored integrity badge, true-resolution/upsample detection.
4. **Safe tag editing**: atomic writes, verify-after-write, preserve unknown
   blocks/embedded CUE, regex replace, tags-from-path, undo.
5. **Platform pack**: MPRemoteCommandCenter media keys, App
   Intents/Shortcuts/Siri, menu-bar mini-player, Now Playing widget.
6. **Discovery modes**: Guest-DJ-style radios, A→B sonic journey, on-device NL
   smart playlists (Petrichor proves it works).
7. **Zones**: AirPlay 2 sender, per-device DSP/EQ profiles + remembered settings.
8. **Audio Units hosting** (convolution/room-correction) — power-user moat.
9. **Migration onboarding**: Music.app import (play counts/loved/smart rules),
   missing-file reconciliation.
10. Cheap format closes: Opus, SACD ISO, WMA, tracker MODs.

Do NOT copy: subscription-gating core features (VOX), serial rescans &
reinstall-requiring DBs (Audirvāna), mDNS-only discovery + port-forwarding
(Roon), phone-UI-on-desktop (Plexamp Mac), config-maximalism (fb2k), MQA,
iPod sync.

## § Footprint — memory & binary size

**Memory.** The incumbent parks whole decoded tracks in RAM (memoryPlayback
up to 256MB/track, artwork cache unbounded). Lyra's rules:

- **Stream, don't stage**: the `CachingSource` block cache is bounded across
  the *library* (256 MiB default), not per track; DSD/hi-res can't balloon it.
- **mimalloc** global allocator in lyra-ffi — less fragmentation/RSS for the
  small-block + stream-buffer alloc pattern.
- **Bounded viz state**: FFT scratch allocated once, spectrogram/scope rings
  fixed-capacity, waveform peaks stored as O(width) rows in SQLite, never
  per-sample.
- **Lazy models**: the `ort` session loads on first "For You"/tagging use and
  unloads on idle — ~20-40MB not resident at rest.
- **Artwork**: decode off main thread, downscale to display size, LRU disk
  cache (the incumbent caches full-res PNGs).

**Binary size.** `opt-level="s"` workspace-wide with explicit `opt-level=3`
per-package overrides for the decode/DSP hot path (symphonia bundles, ape,
opusic-sys, rustfft, lyra-dsp/viz/fs). LTO + codegen-units=1 + strip already
on. The big lever vs the incumbent's ~51MB: **models ship as downloads** —
EffNet/EfficientAT ONNX fetched on first use (optional "download at install"
pref), not bundled .mlpackages. `cargo bloat` in CI to keep it honest.

## § Visualizations (lyra-viz)

Computed in Rust on the DSP tap; UI gets draw-ready values only.

| Viz | Mechanism | Cost |
|---|---|---|
| Spectrum bars | Hann FFT (rustfft, SIMD) → geometric log bands → attack/decay ballistics | one 4k FFT per frame, ~µs |
| Spectrogram | bounded ring of normalized spectrum rows (waterfall) | same FFT, O(width×bands) |
| Waveform seekbar | min/max per bucket at **import time** → SQLite row | O(n) once per track |
| VU/peak + clip latch | per-channel peak w/ PPM decay + block RMS dBFS | trivial |
| Oscilloscope/Lissajous | strided decimation ring, L+R traces | bounded |

The wire format to Swift is just `Vec<f32>` bars / `(peak,rms)` pairs — the
SwiftUI layer draws with Canvas; nothing crosses the boundary per-sample.

## § Build

- Toolchain pinned per-project: `mise.toml` → rust stable, cmake+ninja
  (libopus), mr-boxington (`mbx` = shared/pruned target cache — project-local
  only, no global cargo wrapper).
- `make` → cargo build → swiftc (CLT suffices; SwiftUI *macros* like bare
  `@State` need Xcode — the shell avoids them) → .app → ad-hoc codesign.
- `make check` → `mbx check`/`cargo check --workspace`.

## Open spikes (validate early)

1. Hog mode + rate switching + integer format under app-sandbox on macOS 26 —
   works on MAS apps, verify on Developer-ID direct distribution.
2. `mdns-sd` coexisting with mDNSResponder on UDP 5353.
3. Gapless M4A/AAC via isomp4 (iTunSMPB/edit-list honoring).
4. `ape-decoder` + `symphonia-codec-wavpack` maturity on a real library.
5. `coreaudio` 0.2.2 vs raw `objc2-core-audio` for the IOProc driver.
6. Browser remote fallback: SPAKE2→WSS session tokens if a web client is wanted.
7. BitMuse library.sqlite import path (read-only open, map rows).
