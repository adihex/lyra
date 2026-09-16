# Lyra — macOS System-Integration Surfaces

Date: 2026-09-16. Grounded against the real tree at `~/lyra-src`:
`app/Sources/LyraApp/{LyraApp,ContentView,Bridge,MediaKeys,Viz/*}.swift`,
`Info.plist`, `entitlements.plist`, `Makefile`, `BLUEPRINT.md`,
`docs/research/{agent-native-design,album-art-design…}.md`. API status claims
verified against Apple docs + macOS 26 (Tahoe) field reports — the status-item
hosting change and MenuBarExtra label bug are load-bearing, don't skip them.

## 0. What already exists (don't re-scope)

| Surface | State | Where |
|---|---|---|
| Now Playing + media keys | **Done** — `MPRemoteCommandCenter` + `MPNowPlayingInfoCenter`, `playbackState` set correctly | `MediaKeys.swift` |
| Menu-bar extra | **Exists** — `MenuBarExtra(.window)` + `MiniPlayerView` (title, transport, volume, time) | `LyraApp.swift:43` |
| Noise-XX LAN remote w/ pairing | Done, port 4777, mDNS `_lyra._tcp` | `Bridge.swift` `LyraRemote` |
| `public.audio` Viewer doc type | Declared ("Open With" works for system audio UTIs) | `Info.plist` |
| VizFrame @60Hz (bands/wave/beat/bass/level) | Contract stable, dlsym-resolved, mock provider in place | `VizFrame.swift` |

What does **not** exist yet, and gates several designs below: no queue
(`queue.*` is designed in agent-native, unimplemented), no rating/love column
(`lyra-store` has tracks/sources/settings/waveform_peaks only), no album art
(pipeline designed, landing separately), no `NSApplicationDelegateAdaptor`
(everything below that needs a delegate — dock menu, `continueUserActivity`,
UNUserNotificationCenterDelegate — hangs off adding one, ~20 lines), no
`CFBundleURLTypes` (the `lyra://` scheme is another workstream's deliverable).

Build constraint that shapes everything: **CLT `swiftc` Makefile, no
xcodeproj.** `@State`/SwiftUIMacros are absent under CLT — all view state must
live in `ObservableObject`s (already the pattern). App extensions (QL, widget,
importer) are hand-buildable (bundle + swiftc + inside-out codesign) but the
planned XcodeGen migration for Sparkle is their natural home.

---

## 1. Menu bar extra — animated icon + popover

**macOS 26 status.** Third-party status items are now *hosted by the
ControlCenter process* (`NSSceneStatusItem` in SDK 26; same API surface).
System Settings → Menu Bar gets a per-app "Allow in the Menu Bar" kill switch,
and the ledger keys on bundle ID — reported dev churn (agents/IDEs launching
your binary) can mis-attribute and silently block the item. Two consequences:

1. A `MenuBarExtra`-only app *exits* when the user disables it (the scene can't
   mount). Lyra has a `WindowGroup`, so it survives — but the extra just
   silently never appears. Detect with `NSStatusItem.isVisible` if we migrate
   (below); with `MenuBarExtra` there's no visibility hook — mitigate by
   treating the menu bar as optional UI, never the only path to transport.
2. **MenuBarExtra's label is cached at the menu-bar-server level until hover**
   (FB11857447, still real) — an animated label via `TimelineView` inside the
   `MenuBarExtra` label is the documented workaround and is *flaky* (breaks
   under test, stutters). Every animated-icon app that ships (Ice, Stats,
   Multi.app) uses `NSStatusItem` + `NSHostingView` inside `statusItem.button`.

**Recommendation: migrate to `NSStatusItem` + `NSHostingView` + `NSPopover`.**
Reasons specific to Lyra: (a) we want a VizFrame-driven animated icon —
`bands`/`level`/`beat` at ~30Hz in a 22-pt strip — and there is no API rate cap
on status-item redraw (unlike `NSDockTile`), but under Tahoe hosting each frame
is effectively an XPC to ControlCenter, so cap at 30Hz and make it opt-in;
(b) `isVisible` tells us when Tahoe has hidden us; (c) `NSPopover` gives real
control of anchoring/detach-into-window behavior vs `.menuBarExtraStyle(.window)`.
Cost: we lose CLT-cleanliness? No — `NSStatusItem`/`NSHostingView`/`NSPopover`
are plain AppKit+SwiftUI, all CLT-safe. ~1 day.

**Icon content (three opt-in modes, Settings-controlled):**
- Static note glyph (default; Tahoe-era template image)
- Mini spectrum: 8–12 of the 64 bands, eased, 30Hz — reads at a glance "Lyra is
  playing"; freeze to rest on pause (VizState already has `decayed()` semantics)
- Beat pulse: single dot/`level` meter — lowest CPU

**Popover content (grow `MiniPlayerView` to ~280pt):** album-art thumb (when the
art pipeline lands — art cache is content-addressed in the container, so the
path resolves by hash), title/artist/album (exists), seek bar using
`displayPosition` scrub-state (reuse the main-window pattern — never seek on
drag), transport row (exists) + shuffle/repeat once queue lands, volume (exists),
a 1-row queue peek ("Up next: …"), output-device submenu (engine already
enumerates devices for HAL), and a footer: Love (when ratings exist) · Open Lyra
· Settings · Quit. Keep the `.window` behavior — sliders die under `.menu`
style (already encoded in the blueprint).

Sandbox: fully fine, no entitlement. Priority **P0** (extends existing code).

---

## 2. Notifications — UNUserNotificationCenter

Works sandboxed, no entitlement; runtime permission via
`requestAuthorization(options: [.alert, .sound])` — ask lazily on first enable,
never at launch. Add `UNUserNotificationCenterDelegate` on the app delegate;
`userNotificationCenter(_:willPresent:)` returning `.banner` decides the
frontmost-suppression below (foreground delivery is suppressed by default —
exactly what we want).

**Artwork**: `UNNotificationAttachment` (macOS 10.14+) takes a file URL — write
the art thumb into `tmp/` inside the container and attach; lands free once the
art pipeline ships. Until then, no attachment (icon-only notification).

**Actions** (`UNNotificationCategory` "TRACK", declared at launch):
- **Next** — always
- **Love ♡** — *gated*: needs a rating column in `lyra-store`; ship the category
  with just Next today, add Love when ratings land
- Body click → open main window to the track (`lyra://play?track=` when the
  scheme lands)
- NOT "add to queue" — the track is already playing; meaningless here.

**Anti-spam policy — the actual design problem.** Every-track notifications are
hostile. Ship three modes (Settings): **Off / Album change (default) / Every
track**, all subject to: suppress when Lyra is frontmost (user is looking at the
player — a banner is pure noise), suppress when the track change was
user-initiated inside the app (they clicked — they know), dedupe identical
track-change bursts (gapless album transitions → one banner), `interruptionLevel
= .passive` (never breaks Focus/DND, no sound by default). `threadIdentifier =
album` so banners coalesce per album in Notification Center. This is the
iTunes-classic behavior done deliberately; Focus/DND respect is automatic via
the system — we don't detect it.

Effort ~1 day + art hookup. Priority **P0**.

---

## 3. Dock right-click menu

`applicationDockMenu(_:) -> NSMenu?` on the app delegate — returns a fresh NSMenu
on each right-click, so now-playing state is always current. Contents (iTunes
pattern): a disabled header item showing `Title — Artist` (or "Not playing"),
separator, Play/Pause (stateful), Next, Previous, separator, Love (gated),
"Show Lyra". All actions call the existing `vm` methods. ~2 hours, sandbox-free,
no entitlement. **P0** — cheapest real integration win in the document.

(`NSDockTilePlugIn` — menu while *not* running — exists but is a separate plugin
bundle living in a system process; overkill for a player. Skip. Dock *tile*
itself is the other workstream — don't touch.)

---

## 4. Spotlight indexing — CSSearchableIndex

`CSSearchableIndex.default()` works fully sandboxed, no entitlement. Index the
library: `CSSearchableItem(uniqueIdentifier: track.id, domainIdentifier:
"tracks", attributeSet)` with `CSSearchableItemAttributeSet` contentType
`.audio`, title, `artist`, `album`, `duration`, `thumbnailData` (art pipeline),
keywords. Drive it from the library change stream — `sync_dir`/`sync_files`
already return stats; add index-diff on import and `deleteSearchableItems` on
prune. Batch via `beginIndexBatch`/`endIndexBatch`. Also implement
`CSSearchableIndexDelegate` (reindexAll/reindexItems — Spotlight can ask us to
rebuild).

**Restore path**: tapping a result calls
`application(_:continue:restorationHandler:)` with activityType
`CSSearchableItemActionType` ("com.apple.corespotlightitem") and the
identifier in `userInfo[CSSearchableItemActivityIdentifier]` — route it into
select-and-play. This does **not** need `lyra://` to ship first, but should
share the dispatcher when it lands (agent-native doc §3.1 — same target space:
`lyra://play?track=<id>`).

**The other half — `.mdimporter`**: `CSImportExtension` is iOS-only; on macOS,
Finder-level file metadata (Get Info artist/album fields, Finder search by
tag) still runs on legacy CFPlugin `.mdimporter` bundles in
`Contents/Library/Spotlight` — old tech, still works (Foxglove ships one for
MCAP). Ours would feed `lofty` tag reads for `.dsf/.ape/.wv/.mod` — files
Spotlight understands zero about today. Medium effort, clearly gated behind
the QL extension proving the appex packaging path. **P2.**

Library index itself: ~1.5 days. **P0/P1** — for a *library* player this is the
highest-leverage system surface after menu bar; users will find Lyra tracks by
typing in Spotlight, which no competitor remote-app does well.

---

## 5. Quick Look extension — the differentiator

**Correction to the brief**: FLAC previews natively on modern macOS (CoreAudio
FLAC since 10.13 — Finder spacebar plays it, albeit with a bare play button and
zero metadata). The actual gap — and it's big for a lossless-first player — is
**APE, DSF/DFF, WavPack, Opus, tracker MODs**: files Finder treats as blank
documents. A QL Preview extension (`QLPreviewingController`, view-based —
`preparePreviewOfFile(at:completionHandler:)`) inside `Lyra.app` claims them.

What the preview shows: embedded art + full `lofty` tag sheet (our tags render
richer than anything Finder shows — codec, bitrate, sample rate, channels, RG
gain, duration), a static waveform strip from `waveform_peaks`-style decoding,
and **in-preview playback**: the appex links the same Rust staticlib → decode
via symphonia → `AVAudioEngine`/AUHAL out. QLCodec (OSS, runs on Tahoe) proves
non-native-format audio playback inside a QL preview is shippable. Playback in
the appex is the stretch goal — v1 = metadata+art+waveform + "Open in Lyra"
button (deep link), v2 = play button.

Companion: **`QLThumbnailProvider`** extension — album art as the Finder icon
thumbnail for all our UTIs (the system never thumbs audio files). Cheap once
the appex machinery exists; same metadata seam.

Sandbox reality: the appex is sandboxed by definition; the system grants it
read access to *the previewed file only* — **no sibling `cover.jpg` access**.
Embedded art (pipeline source #1) is what's available; folder-art fallback is
impossible inside the extension. Requires declaring our own UTIs
(`UTExportedTypeDeclarations` for `.dsf`, `.dff`, `.ape`, `.wv`, `.mod`, etc.)
in the app `Info.plist` first — that plist work is ~free and also fixes
"Open With" for non-system formats. Do it now regardless.

Effort: appex packaging is the real cost — needs the build to produce a nested
signed bundle; natural fit for the XcodeGen migration. Code itself ~2–3 days
(the tag/decode code already exists in Rust). **P1** — genuinely
differentiating; this is the "Colibri-class" platform polish a hi-fi player
should have and nobody under MAS does for DSF.

---

## 6. App Intents / Shortcuts — brief

Owned by the agent-native workstream (§3.2 there: `AppShortcutsProvider`, ~1
week, sandbox-clean, no entitlements; intents ride the same command dispatcher
as the socket). Overlap note: **Controls (below) and Focus filters both consume
AppIntents** — land the intent set once and all three surfaces light up.
`lyra://` + intents + socket are three adapters on one dispatcher; don't fork
semantics.

---

## 7. Share & Services

- **`NSSharingServicePicker`** — "Share" on a track/now-playing row: shares the
  file URL (Finder target), or a `♪ Title — Artist` string/`lyra://` link into
  Mail/Messages/Mastodon. `show(relativeTo:of:preferredEdge:)`. Half-day. **P2** —
  nice, not load-bearing for a local player.
- **Services menu** (`NSServices` in `Info.plist`, `NSApp.servicesProvider`) —
  "Play in Lyra" / "Enqueue in Lyra" appearing in Finder right-click → Services
  for `NSSendFileTypes` = our audio UTIs. Requires the same UTI declarations as
  QL. ~half-day once UTIs exist. **P2**, cheap ecosystem presence.

---

## 8. Handoff / Continuity — honest take

`NSUserActivity` `isEligibleForHandoff` works sandboxed, no entitlement — but
Handoff only fires between apps signed with the **same Team ID** on devices
sharing an iCloud account, and Lyra today is ad-hoc signed (no team ID) with a
LAN-remote companion that isn't an iCloud app. Even with a same-team iOS build,
what would it carry? Not playback — the phone can't decode the Mac's local
files — at best "open the remote UI at the current queue position," which the
Noise session already does better over LAN without iCloud. **P2/skip** — revisit
only if a real iOS companion ships under a paid team. (The *other* continuity
win — WWDC26 NowPlaying Remote Media Sessions for phone lock-screen control —
belongs to the remote workstream, not this one.)

---

## 9. Widgets — honest take

A desktop Now Playing widget via WidgetKit is possible but structurally weak:
the widget extension renders timeline snapshots with a **~40–70 reload/day
budget** — no continuous animation, no VizFrame feed, ever. The one trick that
works: `Text(timerInterval:)` ticks per-second inside a rendered entry, so an
accurate *position readout* survives between reloads; reload on track change
(`WidgetCenter.reloadTimelines`) is a legitimate, budget-appropriate use. So
the widget = static-ish art+title+position that never animates — strictly worse
than the menu-bar popover it duplicates. Verdict: **P2**, build only after
XcodeGen makes appex targets free, and only if users ask. Spend the viz effort
on the menu bar icon instead.

New in macOS 26 and different from widgets: **Controls**
(`ControlWidgetButton`/`ControlWidgetToggle` via WidgetKit+AppIntents) — users
can place app controls in **Control Center *and* the menu bar**. A Play/Pause
toggle + Next button is a genuinely good fit: tiny, state-driven
(`ControlValueProvider` reads current playback state), free ride once AppIntents
exist. **P1**, sequenced behind the intents workstream. Caveat for both: the
extension needs shared state → `com.apple.security.application-groups`, which
**requires real signing — app groups don't work under ad-hoc signing.** That's
a distribution-stage gate, not a design blocker.

---

## 10. Grab-bag

| Surface | Design | Effort | Pri |
|---|---|---|---|
| **Siri/media intents** | `IN*MediaIntent` is iOS/HomePod-only — macOS Siri runs Shortcuts → covered by App Intents. Nothing extra. | — | — |
| **Focus filters** | `SetFocusFilterIntent` (AppIntents, in-process, sandbox-ok): per-Focus playback profile — e.g. Work focus → suppress track banners + crossfeed preset + resume a named playlist. Cute, marginal. | 1 d | P2 |
| **AirPlay picker** | `AVRoutePickerView` (AVKit, current, sandbox-ok) via `NSViewRepresentable` in the transport bar. **Conflict honesty**: it routes *system/app* output — meaningless under Exclusive HAL mode where the engine owns the device. Show only in compat mode, or hide+tooltip under hog. AirPlay is lossy anyway; a hi-fi player offers it for convenience, not fidelity. | 1 d | P2 |
| **Launch at login** | `SMAppService.mainApp.register()` — macOS 13+, sandbox-ok, reads `.status` (`.enabled` only; `.requiresApproval` → deep-link System Settings). Toggle in Settings. | 2 h | P1 |
| **Settings window** | SwiftUI `Settings { }` scene — CLT-safe, gives Cmd-,. Home for: notification policy (§2), menu-bar mode (§1), launch-at-login, exclusive-output toggle (move from Playback menu), folders/bookmarks, remote pairing. **Everything above needs this home — build early.** | 1 d | P0 |
| **MPRemoteCommandCenter extras** | `likeCommand`/`bookmarkCommand` (Control Center popover hearts), `skipForward/Backward` 15s — extend `MediaKeys.hook`. Like is gated on ratings. | 2 h | P1 |
| **Now Playing artwork** | `MPMediaItemPropertyArtwork` in `publish()` — one field once art lands. | 1 h | P1 |
| **Idle-sleep discipline** | `ProcessInfo.beginActivity(.userInitiated)` while playing so App Nap/idle sleep can't stall a long session; release on pause/stop. | 10 lines | P2 |
| **Global hotkey** | Carbon `RegisterEventHotKey` works *inside the sandbox* without Accessibility (unlike `NSEvent` global key monitors, which need Input Monitoring). Cmd-Opt-Space toggle, Cmd-Opt-L love. | 1 d | P2 |

---

## 11. Anything else — and what to NOT build

- **Screen saver**: skip. Third-party `.saver` bundles run under
  `legacyScreenSaver.appex`, which Apple broke further in Sonoma→Sequoia→Tahoe
  (double-instance preview bugs, no `stopAnimation`, no public API for the new
  engine). The viz engine's soul, better spent: an in-app **idle/fullscreen viz
  mode** — same pixels, zero rotting API.
- **Lock Screen**: macOS exposes no third-party lock-screen surface; Now Playing
  already shows there via MediaPlayer (done).
- **Touch Bar**: dead hardware, skip.
- **AppleScript `.sdef`**: agent-native owns it (their §3.3, P2) — `osascript`
  scriptability; being scriptable needs no apple-events entitlement.
- **`lyra://` URL scheme**: agent-native owns it; this doc only *consumes* it
  (QL "Open in Lyra", share links, Spotlight restore).
- **App Nap / power**: covered above.
- **macOS 26 Tahoe menu-bar ledger bug**: ad-hoc signing churn during dev can
  wedge bundle IDs in `group.com.apple.controlcenter` `trackedApplications`;
  document the fix (System Settings → Reset Control Center) in dev docs. Also:
  keep the MenuBarExtra *scene* `isInserted:`-bindable for our own toggle —
  distinct from the system kill switch.

---

## 12. Entitlement matrix

Current `entitlements.plist`: `app-sandbox`, `files.user-selected.read-write`,
`files.bookmarks.app-scope`, `assets.music.read-only`, `network.client`,
`network.server`. Deltas:

| Feature | Sandbox-ok? | New entitlement needed |
|---|---|---|
| Menu bar (NSStatusItem/MenuBarExtra) | ✅ | none |
| Notifications (UNCenter, actions, attachments) | ✅ | none — runtime auth prompt |
| Dock menu (`applicationDockMenu`) | ✅ | none |
| Spotlight index (`CSSearchableIndex`) | ✅ | none |
| QL preview/thumbnail appex | ✅ | none — appex is sandboxed by definition; reads *only* the previewed file |
| `.mdimporter` | ✅ | none (CFPlugin, loaded by mds) |
| App Intents / Focus filter | ✅ | none |
| Widget ext / Controls ext | ✅ | **`com.apple.security.application-groups`** (shared state app↔appex) — needs paid-team signing |
| Handoff | ✅ | none (needs same-Team-ID iOS app + iCloud) |
| AVRoutePickerView | ✅ | none |
| SMAppService login item | ✅ | none |
| Services (`NSServices`) | ✅ | none |
| `RegisterEventHotKey` | ✅ | none |
| Still deliberately absent | — | `automation.apple-events` stays out (blueprint §sandbox) |

**One new entitlement total** (`application-groups`), and only when the widget/
controls extensions ship — which transitively means "needs Developer ID or MAS
signing, not ad-hoc."

## 13. Surface table

| Surface | API | Sandbox | Entitlement Δ | Effort | Priority |
|---|---|---|---|---|---|
| Menu bar icon + popover | NSStatusItem+NSHostingView (migrate from MenuBarExtra) | ✅ | none | 1 d | **P0** (extend existing) |
| Notifications | UNUserNotificationCenter | ✅ | none | 1 d + art | **P0** |
| Dock menu | applicationDockMenu | ✅ | none | 2 h | **P0** |
| Settings scene | SwiftUI Settings | ✅ | none | 1 d | **P0** (blocks nothing but houses everything) |
| Launch at login | SMAppService.mainApp | ✅ | none | 2 h | **P0/P1** |
| Spotlight library index | CSSearchableIndex | ✅ | none | 1.5 d | **P1** |
| UTI/doc-type expansion | Info.plist + UTExportedTypeDeclarations | ✅ | none | 2 h | **P1** (gates QL/Services) |
| Now Playing extras | MPArtwork, like/bookmark cmds | ✅ | none | 3 h | **P1** (art/ratings gated) |
| Quick Look preview + thumbs | QLPreviewingController + QLThumbnailProvider appex | ✅ | none | 2–3 d + appex build | **P1** (differentiator) |
| Control Center controls | ControlWidget via AppIntents | ✅ | app-groups* | 1 d post-intents | **P1** |
| Services menu | NSServices | ✅ | none | 0.5 d | P2 |
| Share picker | NSSharingServicePicker | ✅ | none | 0.5 d | P2 |
| Focus filter | SetFocusFilterIntent | ✅ | none | 1 d | P2 |
| AirPlay picker | AVRoutePickerView | ✅ | none | 1 d | P2 |
| Desktop widget | WidgetKit ext | ✅ | app-groups* | 1–2 d post-XcodeGen | P2 (weak vs menu bar) |
| `.mdimporter` | CFPlugin bundle | ✅ | none | 2 d | P2 |
| Idle sleep prevention | ProcessInfo.beginActivity | ✅ | none | 10 ln | P2 |
| Global hotkey | RegisterEventHotKey | ✅ | none | 1 d | P2 |
| Handoff | NSUserActivity | ✅ | none | — | **skip** (Noise remote wins) |
| Screen saver | .saver | — | — | — | **skip** (Tahoe-rotted) |
| App Intents / `lyra://` / sdef | — | ✅ | none | — | other workstream |

\* app-groups ⇒ real signing required; no-op under ad-hoc.

## 14. Recommended build order

Tied to what exists in the tree — each step lands on already-shipped code:

1. **`NSApplicationDelegateAdaptor` + Settings scene** — the delegate is the
   mounting point for dock menu, notifications, Spotlight restore; the Settings
   scene houses every preference below. (P0, ~1.5 d)
2. **Dock menu** — pure win, hours. (P0)
3. **Menu-bar migration** `MenuBarExtra → NSStatusItem+NSHostingView+NSPopover`
   — unblocks the animated VizFrame icon (30Hz, opt-in) and gives `isVisible`
   detection under Tahoe's hosting. Grow the popover per §1. (P0, ~1 d)
4. **Notifications** — policy engine (off/album/all × frontmost &
   user-initiated suppression) + Next action; attachment slot ready for art.
   (P0, ~1 d)
5. **UTI declarations** — `UTExportedTypeDeclarations` for dsf/dff/ape/wv/mod/
   wv/etc. in `Info.plist` — unblocks Open-With, QL, Services. (P1, hours)
6. **Spotlight index** — library `CSSearchableIndex` + `continue` restore into
   select-and-play; converge with `lyra://` dispatcher when it lands. (P1)
7. **Now Playing polish** — artwork property + like/bookmark/skip commands as
   art & ratings land. (P1)
8. **Login item + hotkey + idle-sleep** — the cheap comforts batch. (P1/P2)
9. **At the XcodeGen migration**: QL preview+thumbnail appex (the killer
   feature), then Controls, then widget-if-users-ask. `.mdimporter` after QL
   proves the metadata seam. (P1→P2, signing-gated)
10. **Later/maybe**: share picker, Services entries, Focus filter, AirPlay
    picker — each ≤1 day, value-judged against roadmap appetite. (P2)

Explicitly not on the list: Handoff, screen saver, Touch Bar,
`NSDockTilePlugIn`, apple-events entitlement — reasons inline.
