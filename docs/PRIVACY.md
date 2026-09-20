# Lyra Privacy — PII surface, handling, and posture

All claims below are grounded in the current tree. File paths are given so
each statement can be re-verified with `grep`/read. Nothing here describes
tooling that does not exist in the repo.

## 1. What personal / potentially-identifying data exists in the code

### 1a. Music library and listening-adjacent data (local SQLite)

`crates/lyra-store/src/lib.rs` (`lyra-store: the library database.
Rust-owned single writer (rusqlite bundled …), WAL, schema via
rusqlite_migration`):

- `tracks` table rows: file `path`, `title`, `artist`, `album`,
  `album_artist`, `track_no`, `mtime`, audio `format`, `artwork_hash`,
  `audio_hash`. Full-text index `tracks_fts(title, artist, album)` kept in
  sync by `tracks_ai/ad/au` triggers.
- `sources` table: scan roots that were added.
- `settings` table: opaque `key → value` app settings
  (`set_setting` / `clear_setting`).
- `waveform_peaks` table: per-path min/max buckets
  (`path PRIMARY KEY REFERENCES tracks(path) ON DELETE CASCADE`).
- `artwork`, `artwork_fetch`, `track_maps` tables: content hashes and
  per-track map state (`audio_hash` joins tracks to their map).
- The DB file is `library.db` inside the app container / Application Support
  dir (`app/Sources/LyraApp/Bridge.swift`: `dbDir` —
  `library.db`, `maps/`, IPC socket live there; `Lyra/library.db` under
  Application Support).

This is library/listening data in the everyday sense: what tracks you own,
where they live on disk, and derived fingerprints. There is no analytics
SDK, telemetry uploader, or account system in the tree.

### 1b. Credentials and remote-access secrets (macOS Keychain, never prefs)

- `app/Sources/LyraApp/RemoteSources.swift`:
  - `enum Keychain` wraps `SecItemAdd` / `SecItemCopyMatching` /
    `SecItemDelete` with `kSecClassGenericPassword`,
    `kSecAttrService = "app.lyra.player.remote"`,
    `kSecAttrAccount = <profile UUID>`.
  - SSH/SFTP passwords live only in that Keychain service
    (`ContentView.swift`: `remotePassword` form field —
    `SecureField("Password (optional — Keychain)")`,
    comment: `passwords live in the Keychain`).
  - Indexer API keys (Jackett/Prowlarr Torznab feed keys) likewise:
    `SettingsView.swift` states `The key stays in Keychain`, and
    `RemoteSources.swift` persists only the endpoint model in UserDefaults
    while `Keychain.set/get/remove` holds the key.
  - Deletion path: clearing a password/API field calls `Keychain.remove`;
    deleting a profile calls `Keychain.remove(profile id)`.
- `crates/lyra-net/src/lib.rs` documents the Last.fm session-key rule:
  `we exchange for a session key kept in Keychain — never prefs`.
  `LastFm::new(api_key, secret)` takes both `injected at runtime; NOT
  compiled in`, and the module header states `no secrets baked into the
  binary` / keys `arrive via config/env at first-run (or the user's own
  Last.fm app), never shipped in Info.plist`. `README.md` repeats
  `No secrets in the bundle`.
- `BLUEPRINT.md` confirms the same design: `Last.fm keys arrive at runtime`,
  `session key in Keychain`.

Non-secret remote profile fields (host, port, user, key_path, root_path —
`crates/lyra-fs/src/config.rs`, `RemoteProfile`) ARE persisted in
UserDefaults (`RemoteSources.swift`: `UserDefaults.standard.data/defaultsKey`
round-trip) plus folder security-scoped bookmarks
(`ContentView.swift`: `folderBookmarks`). Folder bookmarks and UI prefs
(`Prefs.swift`: `Everything persists to UserDefaults` — notify/menu-bar/dock/
viz/pet settings) are explicitly non-credential storage; secrets are never
written there.

### 1c. Network recipients (what leaves the device, and when)

Outbound integrations implemented in `crates/lyra-net/src/lib.rs`:

| Destination | Code evidence | Payload sent |
|---|---|---|
| Last.fm (`https://www.last.fm/api/auth/?api_key=…`) | `LastFm::auth_url`, `Scrobble { artist, track, album, timestamp_unix, duration_secs }` | Browser auth URL opened for explicit user approval; scrobble shape defined for later submission. The `api_sig` signer + actual POST are marked `TODO … implemented when scrobbling lands`; current tree builds the shape, not the transmit path. |
| MusicBrainz (`musicbrainz.org/ws/2/release/{mbid}`, UA `Lyra/0.1.0 (https://github.com/lyra-player)`) | `MusicBrainz::release` | MBID / release lookup; policy comment notes max 1 req/s via a shared rate-limiter (TODO governor crate). |
| Cover Art Archive (`coverartarchive.org/release/{mbid}/front-500`) | `coverart_url` (+ `lyra-search/src/cover_art.rs` query builder) | MBID-based artwork fetch. |
| LRCLIB (`lrclib.net/api/get`) | `LrcLib::get` | `track_name`, `artist_name`, `album_name`, `duration` for synced/plain lyrics. |

Sandbox posture (`entitlements.plist`): `com.apple.security.app-sandbox =
true`, `network.client = true` with comment
`outbound: last.fm/musicbrainz/lrclib/coverart/license/appcast`,
`network.server = true` for the LAN remote-control listener.
`docs/BITMUSE-AUDIT.md` independently lists the same three third parties
(`coverartarchive.org, lrclib.net, ws.audioscrobbler.com`) and flags that
`listening habits leak to third parties` whenever those lookups run —
i.e. track/artist/album metadata is inherently disclosed to whichever
service answers the query.

### 1d. Masking / redaction already present

- `crates/lyra-fs/src/config.rs`: custom `Debug for AuthMethod` —
  `Password(_) → "Password(<redacted>)"`,
  `PasswordCallback(_) → "PasswordCallback(<redacted>)"`.
  Unit test `auth_debug_redacts` asserts the formatted value equals
  `Password(<redacted>)` and does NOT contain the input secret.
  Only `Agent` and `KeyFile(path)` (path only, passphrase excluded by `..`)
  print anything identifying.
- Secrets are excluded from `Debug`/`Serialize` surfaces by construction:
  `RemoteProfile` (the serializable struct) carries no secret field at all;
  the secret travels via `AuthMethod::Password` (short-lived) or
  `PasswordCallback`, both redacted above.
- UI uses `SecureField` for password entry (`ContentView.swift`).

## 2. Handling procedures (as implemented)

1. **Secrets live in Keychain.** Service `app.lyra.player.remote`,
   per-profile accounts; Last.fm session key likewise (never UserDefaults,
   never the bundle). Verified in §1b.
2. **Runtime injection, no baked-in keys.** Last.fm `api_key`/`secret`
   arrive via config/env at first run or the user's own Last.fm app
   (`lyra-net` header); nothing secret is compiled in or shipped in
   `Info.plist`.
3. **Log redaction.** Any `{:?}` formatting of `AuthMethod` emits
   `<redacted>` for password variants (test-enforced). Do not `Display`/
   log raw secrets elsewhere; new auth variants must extend the manual
   `Debug` impl, not derive it.
4. **Local-first storage.** Library data stays in the on-device
   `library.db` (WAL, single Rust writer). Remote scan rows are namespaced
   by `sftp://host…` source URIs (`RemoteProfile::source_uri`) and
   `prune_prefix` scoping exists so `a remote scan must never delete local
   rows (or another host's)`.
5. **Sandbox + bookmarks.** App sandbox is on; music-folder access rides
   user-selected read-write + app-scope bookmarks, `~/Music` read-only
   entitlement, audio-input only for the coach lane (`entitlements.plist`).
6. **Credential lifecycle.** Emptying a secret field removes the Keychain
   item; deleting a profile/endpoint removes its item. No code path writes
   a secret to UserDefaults, logs, or the DB.

## 3. Privacy posture: minimization, retention/deletion, consent

### 3a. Data minimization

- Stored library fields are those needed to browse, search (FTS), display
  art, and map tracks (`tracks` + triggers, `artwork_hash '' = "checked,
  none (don't re-probe)"` so artwork is not re-fetched).
- Waveform data is `O(width)` min/max buckets, not audio.
- Lyrics are fetched on demand from LRCLIB, not bulk-stored.
- No account, no analytics, no crash-report uploader in the tree.

### 3b. Retention and deletion (local SQLite, user-controlled)

- Retention = the local `library.db` lives as long as the install/library
  does. There is no server copy and no retention timer in code.
- User-driven deletion that exists today:
  - Rescan prunes vanished files: `Library::prune_missing` /
    `prune_prefix` execute `DELETE FROM tracks WHERE path=?1`
    (waveform rows cascade; FTS triggers clean the index).
  - `clear_setting(key)` executes `DELETE FROM settings WHERE key=?1`.
  - Removing a remote root + rescanning prunes its `sftp://…` rows via the
    scoped `prune_prefix`.
  - Removing a profile/endpoint deletes its Keychain secret (§1b).
  - Uninstalling the app removes the container holding `library.db`.
- What does NOT exist: a one-click "erase my library", an export-my-data
  button, or timed auto-expiry. Those are proposed below, not claimed.

### 3c. Last.fm sharing is user opt-in (with an honesty note)

Evidence for opt-in:

- The documented flow requires the user to approve in the browser
  (`auth_url` → `auth.getSession` token→session exchange per
  `BLUEPRINT.md` and the `lyra-net` doc comment), and the session key is
  then kept in Keychain.
- Nothing in the current tree auto-enrolls the user: there is no stored
  Last.fm session key default, no background scrobble sender (the POST +
  `api_sig` signer are explicit `TODO`s), and no Settings default that
  enables sharing. `docs/BITMUSE-AUDIT.md` notes `whether opt-in is
  unclear from static analysis` for a *different* (audited) codebase — for
  Lyra itself the flow cannot complete without the user's browser approval
  step, but the in-app consent toggle has not landed yet (see next steps).

### 3d. GDPR / request handling — what exists vs. what does not

A repo-wide search for `gdpr|data subject|right to|erasure|export.*data|
delet.*account|DSR` over `*.rs`/`*.swift` returns no request-handling
endpoints. That is expected: Lyra is a single-user local app with no
server-side personal-data store, so there is no controller-side portal to
build. What exists instead:

- **Access:** the user already holds the data — `library.db` is a standard
  SQLite file in their container; any SQLite client can read/export it.
- **Rectification:** rescan/re-import corrects metadata rows.
- **Erasure:** rescan-prune + profile removal + Keychain removal + uninstall
  (§3b). No remote copy needs erasing because none of the integrations
  stores a Lyra-side account record.
- **Portability:** SQLite file itself is portable, but no one-click JSON/CSV
  export exists.

### 3e. Concrete next steps (not yet implemented — do not treat as done)

1. Add an explicit Last.fm consent toggle in Settings (default off),
   persisted outside Keychain with the grant timestamp; gate ALL of
   `auth_url`, session-key storage, now-playing, and scrobble-queue on it.
2. Make the scrobble queue visible (pending items, destination, retry) and
   add "scrobble only on Wi-Fi / never metered" if applicable.
3. Add "Export my library (JSON)" and "Erase library data" actions calling
   the existing `prune_*`/`clear_setting`/Keychain-remove paths plus file
   removal, with confirmation UI.
4. Extend the `AuthMethod`-style redaction rule to any new secret-bearing
   type (manual `Debug`, test like `auth_debug_redacts`) and keep secrets
   out of crash logs.
5. Document third-party disclosures in-app (Settings → Privacy) reusing the
   §1c table, since MusicBrainz/CoverArt/LRCLIB/Last.fm inherently receive
   track metadata per query.
