# Lyra Dock Icon Visualization — Feasibility & Design

**Question:** can Lyra's Dock icon (and adjacent surfaces) animate in sync with the music?

**Verdict: YES — with public API, sandboxed, hardened-runtime, App Store legal.** `NSDockTile.contentView` has been public since Mac OS X 10.5 and is unchanged on macOS 26 Tahoe (nothing in the 26.x release notes touches it). The catch: it's a pull model — the Dock does not live-render your view; every frame costs one `dockTile.display()` call, which snapshots the view and ships the image to the Dock server over a synchronous mach message. That makes ~12 fps the sweet spot, not 60. At icon size (48–128 pt), 12 fps is indistinguishable from smooth, and Lyra's beat pulse already decays over ~150 ms — the fastest meaningful feature in a `VizFrame` — so nothing musical is lost.

The design recommendation below is **not a meter stuck on the icon**. Lyra's own icon (`scripts/make_icon.swift`) is already a tiny celestial scene — an indigo gas giant, a mint moon parked at −35° on a faint orbit circle, a seeded star field, a mint glow. The dock viz should be **that icon, alive**: the moon orbits, the planet bobs on the beat, the orbit tilts with bass like a Saturn ring, stars twinkle off the high bands, and a clip event sends a shooting star across the sky. At rest it composes back to *exactly the static icon* — the moon eases home to −35° and the scene becomes the artwork again. Best possible identity story: you can't tell it's animated until the music starts.

---

## 1. Mechanism table

| Surface | API | Live? | Rate ceiling | Sandbox-safe | Notes |
|---|---|---|---|---|---|
| **Dock icon** | `NSApp.dockTile.contentView = NSView` + `dockTile.display()` | Yes — pull model | Practically ~10–15 fps sustained; 30 fps demonstrated smooth but wasteful | Yes — public AppKit, no entitlement | Replaces whole icon incl. shape; `badgeLabel` still overlays |
| **Dock icon (alt)** | `NSApp.applicationIconImage = NSImage` | Per-frame `NSImage` swap | Same IPC cost per update | Yes | Flickers if combined with `contentView` — pick one; `contentView` wins for animation |
| **Dock badge** | `dockTile.badgeLabel` | Per-change | Cheap but still a mach msg — state transitions only | Yes | Red notification-style pill; misleading for "♪/BPM" text — see §5 |
| **Menu bar item** | `MenuBarExtra` label = arbitrary SwiftUI view (Lyra already ships one, `.window` style) | Yes — in-proc compositing | No real ceiling; 15–30 fps fine | Yes | Best ambient surface: visible even when Dock autohides |
| **Now Playing** | `MPNowPlayingInfoCenter` / `MPMediaItemArtwork` | Static image only | — | Yes | `MPMediaItemAnimatedArtwork` exists (macOS 26 / iOS 19) but wants a *local video file per media item* — pre-rendered loop, not a live feed. Technically possible, pointless for sync. Skip |
| **Control Center** | Now Playing widget only | No | — | — | No custom surface |
| **Stage Manager thumbnail** | — | No | — | — | No public API; WindowServer-captured; even private SkyLight capture paths broke on macOS 26.5 (alt-tab #5748) |
| **Touch Bar** | — | Dead | — | — | Skip |
| **Lock Screen** | — | Nothing | — | — | Skip |
| **Persistent tile while app quit** | `NSDockTilePlugin` (dylib in `Contents/PlugIns`, runs inside Dock process) | Static custom tile | — | **Not Mac App Store legal** | Unneeded: only animate while running; on quit the tile reverts to the asset icon automatically |

### `display()` cost — the real constraint

`dockTile.display()` is not a redraw flag; it performs a synchronous snapshot of `contentView` and a `mach_msg` to the Dock server on the calling (main) thread. Measured consequences in the wild:

- bad-dock (wondertwins/bad-dock) ran a `Timer` at **30 fps** driving a physics bouncing ball — smooth. Streaming Bad Apple video ran at **12 fps** (chosen for memory, not because the Dock choked).
- sindresorhus/DockProgress (the canonical MAS-safe dock-progress lib) calls `display()` from a display-link observer — but **stops the observer the moment the animated value converges**. That's the discipline to copy: run the clock only while something is moving.
- Counter-example: BasedHardware/omi issue #6194 — `NSDockTile.display()` → `CoreDockUpdateWindow` → synchronous `mach_msg` blocked the main thread 2 s+ when the Dock server was busy (that path was window-tile invalidation driven by SwiftUI re-render churn, i.e. *uncontrolled* call rate — the lesson is call it on your own cadence, never as a side effect of view invalidation, and never faster than needed).
- 100 Hz (`Timer` at 0.01 s) "heavily increases CPU" (mtynior, measured) — every call is a full view snapshot at backing scale + IPC + Dock compositing.

**Rate recommendation: 12 Hz** (`Timer` on `.main`). Rationale: 150 ms beat decay ≈ 6.7 Hz feature rate → 12 Hz is ~1.8× Nyquist for anything the viz produces; each frame is a ~256×256 px snapshot — trivial. Treat 30 fps as proven-but-wasteful headroom, never a target.

### Rounded-tile masking — you can draw edge to edge

macOS does **not** re-clip a custom `contentView` to the squircle. The tile view bounds are the full square; standard icons just inset their artwork into the inscribed squircle (~10–12 % margin). bad-dock measured this: icon art drawn at 1.2× fills the entire square tile; beyond that, clipped. Cyberduck famously used `applicationIconImage` to escape the squircle entirely. **Recommendation: conform.** Draw the scene into the same inset the asset icon would occupy (scale the make_icon geometry ~0.83, centered) so Lyra sits naturally among other icons — the whimsy is the motion, not a jailbreak. Note the Tahoe caveat below: icon styles don't apply to custom tiles.

### Retina / resizing

`dockTile.size` reports the tile in screen points and changes with Dock size/magnification; the system may redraw on resize. The view must be width+height resizable and must draw **fully proportional** — all scene coordinates normalized to the tile rect, vector drawing only (CGContext/NSBezierPath), which automatically renders crisp at the tile's backing scale.

### Sandbox / hardened runtime / App Review

- `contentView`, `display()`, `badgeLabel`, `applicationIconImage`: plain public AppKit. **No entitlement, no sandbox exception.** DockProgress is shipped inside sandboxed Mac App Store apps; HandBrake's dock progress is the canonical precedent.
- Nothing private anywhere in this design. App Review risk ≈ zero. The only banned piece in this API family is `NSDockTilePlugin`, which we're not using.
- One rule: never set both `applicationIconImage` and `dockTile.contentView` — they fight and flicker (bad-dock). `contentView` only.

---

## 2. The design: "the icon, alive" (working name: Cosmos)

Reuse the exact composition `make_icon.swift` bakes into `AppIcon.icns` — same constants, same seeded star field — so the resting frame is pixel-faithful to the shipped icon:

| Element (icon coords, S = tile side) | Static | Driven |
|---|---|---|
| Space bg | near-black → indigo diagonal gradient | static |
| Star field, 110 pts, `srand48(20260916)` | alpha 0.12–0.72, 20 % mint | twinkle off high bands |
| Mint radial glow | alpha 0.28 @ r 0.44 S | breathes with `rms` |
| Orbit circle | hairline, alpha 0.16, r 0.36 S | tilts/squashes with `bass` → reads as Saturn's ring plane |
| Planet | indigo radial gradient, r 0.215 S @ (0.5, 0.47), 3 latitude bands, 2 craters | bobs + squashes on `beat`; band stripes drift on mids |
| Moon | mint gradient, r 0.055 S, parked −35° | **orbits**; speed = `level`; parks home on pause |
| Shooting star | — | `clip` edge → terracotta streak |

### Field → motion map (concrete)

Let `dt` = tick delta, all motion driven off the eased compositor frame (`VizRuntime.frame()`), so Rust ballistics + Swift ease(0.55) already smooth the inputs:

| VizFrame | Scene element | Motion | Easing |
|---|---|---|---|
| `beat` (0–1, ~150 ms decay) | Planet bob & squash | `y += beat·0.020·S` upward; squash `sx = 1+0.05·beat`, `sy = 1−0.07·beat` (volume-preserving); ring/orbit gets `beat·2°` tilt kick | direct — the Rust ~150 ms decay IS the animation; add one spring in scene state for secondary overshoot |
| `level` (0–1) | Moon orbit speed | `ω = 0.12 + 2.8·level` rad/s (stately drift → excited spin); moon scale `mr·(1+0.10·level)` | `ω` itself eases: `ω += (target−ω)·0.15` per tick so speed-ups swoop instead of step |
| `bass` (0–1) | Orbit/ring wobble | ellipse y-squash `0.34 + 0.06·sin(φ)`; tilt `θ = θ·0.92 + bass·Δθ`; ring line alpha `0.16 + 0.30·bass` — bass makes the "ring" assert itself | tilt integrates a kick per bass rising-edge (`bass−prevBass > 0.06` → `Δθ += 5°`) then relaxes — same edge pattern as `tickGeyser` |
| `bands[48…63]` | Star twinkle | star *i* alpha `= base_i + hi·flicker_i(t)`; `hi` = mean of top 16 bands; `flicker_i` = `hashNoise(uiTick/6, i)` per-star phase | none needed — flicker is already per-star decorrelated |
| `bands[21…47]` | Planet latitude bands | stripe phase drifts `0.02·mid` per tick; glow alpha `0.28→0.45` with mid energy | direct |
| `peak[0..1]` | Ring glint | one accent-tinted dot rides the orbit opposite the moon, alpha = `max(peakL,peakR)` | peak already has PPM ballistics |
| `waveL/waveR` | *icon: unused*; **pane mode: comet trail** | at pane scale the orbit path is a Lissajous of `waveL×waveR` — the moon literally traces the waveform | — |
| `clip` | Shooting star | clip rising edge → streak spawns top-left, ~0.7 s TTL, terracotta `Ui.accent`, eases out | edge-detect like `prevBeat` in `tickFireworks` |
| `seq` stall | global decay | `pump()` already feeds `decayed(0.94)`; when `level < 0.01` for ~1 s → moon slews to −35° park, stars settle to base alpha | slew = exponential ease `θ += (park−θ)·0.08`, never a teleport |

### Idle / paused / stopped

The contract's rule — "seq stalls → decay to rest, then freeze" — becomes narrative here:

- **Paused (seq stalled):** planet settles to center, squash relaxes, ring tilt relaxes, moon decelerates and *eases back to −35°* — its parked position in the shipped icon. Stars hold at base alpha. End state is indistinguishable from the static app icon. No pause glyph: the scene going still IS the paused mark. (A red `badgeLabel` "❚❚" would read as an unread count — skip it.)
- **Stopped / nothing loaded:** same rest state. Optionally draw one hairline hairline-baseline… no — keep it identical to the icon. Stillness is the signal.
- **Clip:** one terracotta shooting star, then gone. The only "loud" moment in a quiet scene — appropriate for a sticky error flag.
- **Reduce Motion:** the scene is decorative → honor `accessibilityReduceMotion` by never animating the tile (leave the static icon; the in-app mode falls back per the decorative taxonomy).

### Reads at 48 pt?

At 48 pt the scene is: a ~20 px planet, a ~5 px moon on a ~35 px orbit, ~40 stars. Bob, orbit, and ring-tilt all read. Twinkle and latitude drift don't — they're for the 128 pt/pane render; gate the fine detail on `size.width` (stars < 1 px and crater spots drop out below 64 pt — same trick icon designers use). Ship the simplified layer unconditionally at first; add the detail gate only if it looks mushy.

---

## 3. Should it reuse a VizMode renderer, or a dedicated painter?

**Dedicated painter, shared scene.** The blocker is real: `VizDraw.render` takes `inout GraphicsContext` — that's the SwiftUI `Canvas` API, which does not exist inside an `NSView.draw(_:)`. Two honest options:

- (a) `NSHostingView` wrapping a `Canvas`, pushed a new frame each tick, `display()` snapshots it. Works (the clock-icon demos all do this), but you pay a SwiftUI layout+render pass per frame and inherit hosting-view quirks for a view that lives offscreen.
- (b) **`DockIconView: NSView` + a plain CGContext painter.** make_icon.swift is *already* a CGContext painter of this exact scene — the dock painter is that script with a `drive:` parameter. ~120 lines, zero framework impedance, resolution-independent for free.

Recommend (b), factored so the *motion math* is shared:

```
CosmosScene            // pure state: moonAngle, ω, ringTilt, kickVel, starPhase, shootingStar TTL
  .drive(f: VizFrame, dt: Float, size: CGSize)   // field→motion map above; only state that mutates
CosmosPaintCG          // NSView draw — normalized coords → CGContext (dock tile)
VizDraw.cosmos         // Canvas renderer — same CosmosScene → GraphicsContext (in-app mode 35)
```

Two thin painters, one brain. The dock tile never links SwiftUI-Canvas code; the pane mode never touches AppKit.

---

## 4. Also an in-app viz mode? — yes, `cosmos` = mode 35

Same scene, two scales — do both, they share `CosmosScene`:

- `VizMode`: `case cosmos = 35`, `displayName "Cosmos"`, `inputs "bands · beat · level · wave"`, `decorative: true` (it conveys rhythm, not data → falls back to Bars under Reduce Motion, consistent with the contract's taxonomy).
- New file `RenderCosmos.swift` in `app/Sources/LyraApp/Viz/`; scene state hangs off `VizState` (add `var cosmos = CosmosSceneState()` — orbit angle, ring tilt, star phases, shooting-star TTL, a `VizPool(cap: 64)` of `kind: 7` ring-grain particles if you want the ring to granulate).
- **What the pane version gets that the icon can't:** the orbit path drawn as the live Lissajous of `waveL/waveR` (the moon surfs the actual waveform — the mode becomes a cosmic `scope`); all 110+ stars plus drifting depth; ring granulation — `bands[0…31]` mapped around the ring ellipse as particle grains (the ring is secretly a circular spectrum); planet latitude stripes pulsing per mid band; shooting stars on clip AND optionally on big beat edges.
- Contract compliance: `CosmosScene` is fixed-capacity, no per-frame alloc, pools ≤512 — it slots into the existing ownership map as pure viz-ui work.

This also gives you the answer to "is the dock icon a gimmick": the pane mode is where the scene gets to breathe; the dock tile is the same friend in miniature.

---

## 5. Implementation sketch (against real types in ~/lyra-src)

```swift
// app/Sources/LyraApp/Viz/CosmosScene.swift — shared motion state (see §3)
// app/Sources/LyraApp/Viz/RenderCosmos.swift — VizDraw.cosmos + VizMode.cosmos = 35
// app/Sources/LyraApp/DockCosmos.swift — the AppKit side:

final class DockIconView: NSView {
    let scene = CosmosScene()           // own instance — never share VizState across surfaces
    override func draw(_ dirty: NSRect) {
        guard let ctx = NSGraphicsContext.current?.cgContext else { return }
        CosmosPaintCG.draw(ctx, bounds, scene)   // make_icon.swift geometry + drive offsets
    }
}

final class DockVizDriver {             // owned by ViewModel (single VM already: ViewModel.shared)
    private var timer: Timer?
    private let view = DockIconView()
    private var installed = false

    func setEnabled(_ on: Bool) {       // pref: "Dock icon dances"
        if on && !installed {
            view.autoresizingMask = [.width, .height]
            NSApp.dockTile.contentView = view
            installed = true
            timer = Timer.scheduledTimer(withTimeInterval: 1.0/12, repeats: true) { [weak self] _ in
                self?.tick()
            }
        } else if !on && installed {
            timer?.invalidate(); timer = nil
            NSApp.dockTile.contentView = nil
            NSApp.dockTile.display()    // restore asset icon
            installed = false
        }
    }

    private func tick() {
        let f = ViewModel.shared.viz.pumpIfStale(0.080)  // NEW on VizRuntime — see below
        view.scene.drive(f, dt: 1.0/12, size: view.bounds.size)
        let settled = view.scene.isAtRest              // level<ε, beat<ε, moon parked
        if !settled || ViewModel.shared.playing {
            NSApp.dockTile.display()                    // 12 Hz while alive; silent when asleep
        } else {
            // scene at rest → stop paying IPC; next pump (play) restarts via playing flag
        }
    }
}
```

Hook points in existing code:

- **`VizRuntime` needs `pumpIfStale(_ maxAge:)`** — today `pump()` is owned by the always-mounted compact `VizSurfaceView` (transport bar). If the window is closed/occluded, nothing pumps and `frame()` goes stale. Add `private var lastPump = ContinuousClock.now`, stamp it in `pump()`, and expose `pumpIfStale` that only advances when `now − lastPump > maxAge`. The dock driver calls it at 12 Hz: when the window is up it's a no-op read; when the window's gone it becomes the pump. (Don't just call `pump()` unconditionally — two pumpers double-ease the compositor.)
- **Install site:** Lyra is pure-SwiftUI `@main` with no AppDelegate — add `@NSApplicationDelegateAdaptor(AppDelegate.self)` in `LyraApp.swift` and install in `applicationDidFinishLaunching`, *or* lazily on first `playing == true` edge in `startPolling()` (ContentView.swift:239 — it already polls at 30 Hz and owns `playing`). Lazy install is fine and keeps the tile untouched until there's music.
- **Driver lifetime:** one per app, on `ViewModel` next to `let viz = VizRuntime()` (ContentView.swift:120).
- **Prefs (VisualsPane.swift):** "Animate Dock icon: Off / When window hidden / While playing". Recommend default **While playing** — the hidden-only variant saves little and loses the delight; expose it anyway.
- **`isAtRest` gate:** once the scene reports rest AND `!playing`, stop calling `display()` entirely (keep the timer — it must still observe `playing` — or fold the whole driver into the existing 30 Hz poll and only allocate the tile view when playing). Zero-cost idle is the DockProgress lesson.

Lifecycle rules:

1. `playing && !settled` → `display()` at 12 Hz.
2. `!playing` → frame decays (existing `decayed(0.94)` in `pump()`) → scene eases to icon-rest → moon parks −35° → `display()` once final, then **stop**.
3. Window occluded/hidden/minimized → keep animating (that's the *point* — the tile is the status surface when the window isn't visible). App terminated → system drops the custom tile automatically.
4. `accessibilityReduceMotion` → driver never installs (or immediately tears down).
5. `display()` on main thread only; `draw(_:)` must not mutate scene state — all motion lives in `scene.drive`, called from the tick. draw stays a pure function of scene state (re-entrant safe for Dock resize redraws).

---

## 6. Adjacent animated surfaces — ranked by value

1. **Menu bar label (do this second, it's nearly free).** Lyra already ships `MenuBarExtra("Lyra", systemImage: "music.note")` with `.window` style — the label accepts an arbitrary SwiftUI `View`, so swap the static label for a `TimelineView(.animation(minimumInterval: 1/15))` + tiny `Canvas` drawing a mini-Cosmos (planet dot + orbiting moon dot, ~18 pt) or a 5-band ink micro-spectrum, reading `vm.viz.frame()` — no `display()` IPC at all, in-proc compositing, no rate ceiling worth respecting beyond 15 fps politeness. This is the better ambient surface in practice (Dock autohide is common). iStat Menus is the eternal precedent for living menu-bar graphics; Peekaboo ships an animated status icon today.
2. **Now Playing artwork:** static `MPMediaItemArtwork` only for live data. `MPMediaItemAnimatedArtwork` exists in the Tahoe-era SDKs (macOS 26 / iOS 19) but it vends a **local video file per media item** fetched on demand — you could pre-render a looping Cosmos mp4 per track and attach it; it still wouldn't be synced to the audio, and it would only appear where the OS shows animated artwork. Interesting party trick, wrong tool.
3. **`badgeLabel`:** works over a custom view ("the application Dock tile may be badged with a short custom string") but renders as the red notification pill — a live "♪128" reads as an unread count and every change is another mach message. Verdict: skip, or at most a static "♪" toggled once on play/pause as a cheap state marker (off by default).
4. **Confirmed dead ends:** Stage Manager thumbnails (no API, private capture paths broke on 26.5), Control Center (Now Playing widget only), Touch Bar (dead), window proxy icon (not a viz surface), persistent-offline tile via `NSDockTilePlugin` (MAS-banned, unnecessary).

---

## 7. Prior art

| App/project | What it does | Relevance |
|---|---|---|
| **DockArt** (2007, iTunes plugin) | Replaced iTunes' dock icon with album art, optional progress overlay | The original "music → dock icon" idea; died with iTunes visualizer plugins (~12.6). Proof the concept has 18 years of nostalgia behind it |
| **Dock Party** (Mac App Store) | Music-synced visualizers, progress bar, album art along the Dock edge | Ships on the MAS today — proves Apple allows live music graphics at the Dock (it draws overlay windows adjacent to the Dock rather than a tile, but reviewers saw the end result) |
| **bad-dock** (2025, wondertwins) | 12 fps video + 30 fps physics ball inside the dock tile, public API only, built with bare `swiftc` | The rate-limit evidence + squircle measurements + the `applicationIconImage`/`contentView` flicker gotcha |
| **DockProgress** (sindresorhus) | Animated dock-icon progress; display-link-driven `display()`, stops when converged | The engineering pattern to copy; ships inside sandboxed MAS apps |
| **HandBrake / Chrome** | Progress bar + badge in dock tile | Canonical precedent for dock tiles as status surface |
| **Neil Sardesai / mtynior** (2021) | Animated watch-face clock, CPU monitor dock tiles | NSHostingView path; "not designed for real-time — 100 Hz kills CPU" measurement |
| **Cyberduck** | `applicationIconImage` to escape the squircle | Proof edge-to-edge works; Lyra shouldn't take the escape — conforming looks better next to neighbors |
| **omi desktop** | `display()` storm → 2 s main-thread hangs via `mach_msg` to a busy Dock | The cautionary tale: own the cadence, never let view invalidation trigger tile updates |

**App Review risk: none.** Every API here is public since 10.5/10.6; no private symbols, no entitlement, sandbox-compatible. The only adjacent thing that's banned (`NSDockTilePlugin`) isn't needed.

---

## 8. Risks & honest trade-offs

- **`display()` is a synchronous main-thread mach msg.** If the Dock server is wedged, a tick can block. At 12 Hz the exposure is small; still, never call it from `drawRect`, never let SwiftUI invalidation trigger it, and coalesce (skip the call when `scene` is unchanged — a `isAtRest`/`dirty` flag).
- **Tahoe icon styles:** a custom tile ignores the user's Default/Dark/Clear/Tinted setting — a "Clear icons" user sees Lyra's opaque night card. Mitigation: conform to the squircle inset (not full-bleed) so it reads as a normal icon that happens to move; accept the mismatch — every app with a custom tile has it.
- **Battery:** each frame = view snapshot (256² px @2×) + IPC + Dock composite, forever while playing. Trivial but perpetual — the `isAtRest` hard-stop and the window-hidden pref are the cost controls. Optionally drop to 6 Hz on battery.
- **Double-pump hazard:** calling `pump()` from the dock tick while the window's mini surface also pumps double-eases the compositor. Solved by `pumpIfStale` — do not skip that change.
- **Don't mix icon APIs:** `applicationIconImage` + `contentView` together flicker (bad-dock). `contentView` only.
- **Resist scope creep:** no NSDockTilePlugin (MAS-banned, pointless), no badgeLabel ticker, no per-frame `applicationIconImage`. The whole feature is ~250 lines: one scene struct, one CG painter, one NSView, one 12 Hz driver, one enum case + Canvas painter for the pane mode.

---

*See also `desktop-pet.md` — same Cosmos scene popped out of the Dock onto the desktop as a roaming pet (borderless NSPanel per body, the icon literally empties while it's out).*
