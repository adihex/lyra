# BitMuse 1.3.33 — reverse-engineering audit

Static analysis of `/Applications/BitMuse.app` (the app Lyra replaces).
Date: 2026-09-16. Method: codesign/entitlement dump, Info.plist, strings
passes on the 17 MB arm64 main binary, Sparkle appcast fetch.

## Architecture

- Single 17 MB arm64 Mach-O, Swift (`BitMuseLib` module) + Sparkle updater.
- `Developer ID Application: SweetEcom OU (6772HZ55AC)`, notarized,
  hardened runtime ON (`flags=0x10000`). Signing hygiene is fine.
- Deps (from `SourcePackages/checkouts` paths): **GRDB.swift** (SQLite,
  FTS5 porter tokenizer), **SwiftNIO** (NIOCore — HTTP+WebSocket server),
  **swift-nio-ssl** (BoringSSL). Bundled codec frameworks: FLAC, ogg,
  opus, vorbis, wavpack (C parsers of untrusted input).
- Remote control: `RemoteServer` — NIO HTTP routes `/api/library/*`
  (albums/artists/tracks/playlists/foryou enumeration), `/api/play/*`,
  `/api/stop-after-current`, plus WebSocket clients ("max connections").
  Advertised as "Play, browse and queue from any phone on the network".
- Licensing: Lemon Squeezy (`api.lemonsqueezy.com/v1/licenses`), paywall
  checkout URL embedded. Metadata/telemetry calls: musicbrainz.org,
  coverartarchive.org, lrclib.net, ws.audioscrobbler.com (Last.fm).
- Updates: Sparkle, `SUFeedURL=https://updates.bitmuse.app/appcast.xml`,
  `SUPublicEDKey` present, edSignature on every enclosure — feed config
  is done correctly.

## Findings

1. **Not sandboxed.** `com.apple.security.app-sandbox` is absent. The
   `files.user-selected.read-write` and `bookmarks.app-scope` entitlements
   are inert without it — the process has full user-level filesystem,
   network, and AppleEvent reach. Any compromised code path (codec bug,
   malicious file, dependency vuln) runs with your full privileges.

2. **Hardcoded secret.** `LastFmApiSecret=1b316315…779db013` (value
   redacted) ships in plaintext in `Info.plist`. Anyone can extract and burn the
   vendor's API quota / abuse the account. Secrets belong in Keychain or
   fetched at runtime — never in the bundle.

3. **Unauthenticated LAN control surface.** The RemoteServer exposes
   library enumeration + playback control with no pairing/auth strings
   anywhere near the remote code path. If it binds `0.0.0.0` (the "any
   phone on the network" pitch implies it does), every device on the LAN —
   or anything that can reach the port — can enumerate your library and
   drive playback. There is no `NSLocalNetworkUsageDescription` in
   Info.plist, so it may not even trigger the local-network prompt.

4. **AppleEvents bridge.** `NSAppleEventsUsageDescription` = "needs
   permission to mount network music volumes" — the app drives Finder/
   system events via AppleScript. In an unsandboxed process that's a
   powerful primitive if any input path is reachable.

5. **URL scheme handler.** `CFBundleURLSchemes` registered
   (`com.bitmuse.app.activation`) — any webpage can invoke it; activation
   flows that parse incoming URLs are a classic injection surface,
   especially unauthenticated.

6. **Memory-unsafe codec parsers.** FLAC/ogg/opus/vorbis/wavpack C
   frameworks decode untrusted files — fuzzing surface; without a
   sandbox there's no containment for a decoder exploit.

7. **Privacy surface.** Library metadata flows to MusicBrainz, Cover Art
   Archive, LRCLib, Last.fm — listening habits leak to third parties
   (whether opt-in is unclear from static analysis).

## How Lyra answers each

| BitMuse gap | Lyra |
|---|---|
| No sandbox | `com.apple.security.app-sandbox` + scoped bookmarks only |
| Plaintext secret in bundle | No secrets in the bundle; Keychain for anything persisted |
| Open LAN remote | SPAKE2 pairing → pinned X25519 → Noise XX; binds after pairing only |
| AppleEvents for mounts | None — rsync/SFTP over ssh2 in `lyra-fs`, no automation entitlements |
| Unauthenticated URL scheme | No URL schemes; agent surface is a 0600 Unix socket (`lyra-ipc`) |
| C codec frameworks in-process | Symphonia decoders in Rust; FFI boundary is a narrow C ABI |
| Third-party metadata leaks | Search hits only legal indexes on explicit user query |
| Notarization (their one good bit) | Same: hardened runtime, signed, sandboxed on top |
