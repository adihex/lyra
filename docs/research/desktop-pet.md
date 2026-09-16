# Lyra Desktop Pet — the Cosmos scene escapes the Dock

**Escalation:** the celestial scene doesn't just animate inside the dock icon — on command (or on a beat drop) it *pops out* and lives on the desktop as a pet: Saturn with its rings, the Moon orbiting it, the Lyra logo as a third body — a little solar system that roams the screen and dances to the music even when Lyra's window is minimized. Think OpenAI's Codex `/pet` (floats above every window, draggable, doubles as a status surface) crossed with shimeji's wander behaviors.

**Verdict: YES, and it's easier than the dock tile.** A desktop pet on macOS is just a borderless, transparent, click-through `NSPanel` — no API archaeology, no IPC-per-frame like `dockTile.display()`, no rate ceiling worth respecting. The hard parts are behavioral (the pop transition, where the dock tile actually is, permissions for the fancy tricks), not rendering.

---

## 1. Mechanism — the pet window recipe (verified against real implementations)

Every native desktop-pet / overlay project converges on the same `NSPanel` configuration. Sources: R-Pets' SPEC (a deliberate OpenPets-Electron→AppKit mapping), Lowtech's `OSDWindow` (FuzzyIdeas, production code), the notch-Dynamic-Island writeups, and shimeji-on-Mac ports (ShimeTomo, shimeji4mac's JNA path, codex-pet).

```swift
let panel = NSPanel(
    contentRect: rect,
    styleMask: [.borderless, .nonactivatingPanel], // nonactivating: never steals focus
    backing: .buffered, defer: false)
panel.isOpaque = false
panel.backgroundColor = .clear
panel.hasShadow = false
panel.level = .floating                    // above normal windows; .statusBar to go higher
panel.ignoresMouseEvents = true            // full click-through (see §5 for interactive mode)
panel.collectionBehavior = [.canJoinAllSpaces, .stationary, .fullScreenAuxiliary]
panel.hidesOnDeactivate = false
panel.isReleasedWhenClosed = false         // pesty issue: a stray close() nils window forever
```

| Requirement | Flag | Notes |
|---|---|---|
| Floats above everything | `level = .floating` | `.statusBar` hovers over nearly all UI incl. during fullscreen-adjacent contexts; use as "always on top" pref |
| Survives Space switches | `.canJoinAllSpaces + .stationary` | `.fullScreenAuxiliary` keeps it visible over fullscreen apps — the Codex pet behavior |
| Click-through | `ignoresMouseEvents = true` | Whole window, zero code. Per-pixel click-through is possible (borderless + non-layered contentView + hitTest override) but is a known-quirky path — only needed for the draggable mode, where simplest is `ignoresMouseEvents = false` and let the whole small window be the hit target |
| No focus steal | `.nonactivatingPanel` | Also keeps Lyra's window key while pet is dragged |
| Never dies accidentally | `isReleasedWhenClosed = false`, `hidesOnDeactivate = false` | Production-gotcha fixes from pesty/Lowtech |
| Screenshots/screen-share | `sharingType = .none` hides it | **Default: visible.** A pet people can't screenshot defeats the point; offer "hide from captures" as a pref |
| Multi-monitor | `NSScreen.screens` + `didChangeScreenParametersNotification` | Re-clamp positions on display change |

**Sandbox:** all of this is plain AppKit — Lyra's existing `app-sandbox` + hardened runtime covers it, zero new entitlements. It is, literally, just a window. App Review-safe.

**Drive loop:** `CADisplayLink` (macOS 14+) or a 30 Hz `Timer` calling `setFrameOrigin` per body — window moves are ordinary WindowServer transactions; shimeji/oneko clones have done exactly this for decades.

---

## 2. Architecture — moving windows (chosen) vs fullscreen overlay

**Option A — per-body moving windows (the shimeji standard):** each cast member is its own ~48–160 pt panel that roams via `setFrameOrigin`. The Moon *physically orbits* the planet in world space — its window position = planet's window position + orbit offset. Dragging Saturn drags the system; the Moon follows because the coupling is in the world model, not the renderer.

**Option B — one fullscreen transparent overlay per screen**, bodies drawn inside. Trivial multi-body math and the easiest pop transition, but: a full-screen layer composites every frame (real GPU/IOSurface cost at 1440p+, the "measure-decide" cost), every pixel must click-through (pet can never be draggable without mode-flipping `ignoresMouseEvents`), and it shows up weirdly in Exposé/captures.

**Pick A.** Per-body windows. Rationale:
- Cheaper: ~3 small surfaces vs one always-on fullscreen layer. Idle cost ≈ zero.
- The coupling trick (moon window slaved to planet position + orbit offset) gives genuine orbital mechanics in world space — more charming than bodies in one box, and impossible in a single-window scene.
- Hit regions are per-body for free, enabling the draggable "Pet" mode per member (grab the planet, the system follows).
- The pop-out is *per-body* anyway (planet bursts, moon chases) — moving windows express that directly; an overlay would fake it.

Cast (initial): **Saturn** ~140 pt panel (planet + ring plane drawn inside), **Moon** ~56 pt panel, **Logo** ~72 pt panel (the third body — sees below). A "troupe" mode (one 320 pt container, all bodies inside, tighter choreography) is a fallback if window-juggling annoys.

---

## 3. The Pop — launching from the Dock

`NSDockTile` exposes `size` but **no screen-frame API**. Two routes to the tile's real rect:

**A. Heuristic (no permission, default):**
1. **Dock edge + thickness** from `NSScreen.frame` vs `NSScreen.visibleFrame` deltas — bottom dock steals height, left/right steal width. Autohide → no delta → fall back to bottom edge nearest last-known dock config (`defaults read com.apple.dock orientation`).
2. **Slot along the dock:** `NSApp.dockTile.size` gives current icon pitch. Ordinal position ≈ count icons before ours: `defaults read com.apple.dock persistent-apps` (ordered bundle IDs) + `persistent-others`, then running-not-pinned apps appended in `NSRunningApplication` launch order. Not pixel-exact under magnification, but lands within an icon-width or two.
3. That estimate is enough because of the perceptual trick: **at the pop instant, the dock tile itself plays a "crouch + launch" frame** (planet scales down as if jumping out — driven by the same contentView at its normal 12 Hz). The eye binds the two events; a few points of origin error are invisible. While the pet is out, the dock icon renders the *sky without the planet* — empty star field, faint orbit circle — and the planet pops back in on recall. That's the money detail: the icon literally empties.

**B. AX (exact, permission-gated):** `AXUIElementCreateApplication` on `com.apple.dock`'s PID → walk `AXChildren` of the dock list → match `AXTitle == "Lyra"`/`AXURL` → read `AXPosition`/`AXSize` — exact tile rect including magnification. Requires Accessibility trust (TCC prompt; works under sandbox — it's user consent, not an entitlement). **Policy: use it only if `AXIsProcessTrusted()` is already true; never prompt for it.** A pet isn't worth a permission dialog.

**Burst animation (~600 ms):**
1. Tick 0: dock icon scene plays crouch (planet squash 0.8×) — one display() frame.
2. Pet panels appear at the estimated tile rect, scale 0.2 → 1.15 → 1.0 spring, translate up-and-out on a ballistic arc toward a landing point on `visibleFrame`'s bottom edge.
3. Moon panel spawns mid-arc already orbiting; logo tumbles out last (it's the clown of the cast).
4. Land → squash → first hop syncs to next `beat` rising edge, so the very first move is on the music.

**Recall:** reverse arc into the tile rect, panels fade at the edge, dock scene regains its planet (scale 0→1 with a beat-synced bump if timing aligns).

---

## 4. Behavior — the cast dances to `VizFrame` in world space

The field→motion map from `dock-icon-viz.md` lifts to world coordinates; `level`/`beat`/`bass` now move windows, not just pixels:

| VizFrame | World-space motion | Easing |
|---|---|---|
| `level` | Roam speed: wander velocity `40 + 260·level` pt/s, turn rate, hop frequency | `speed += (target−speed)·0.1` — swoops, not steps |
| `beat` | Hop: vertical impulse per rising edge (`beat` crosses 0.55, same edge pattern as `tickFireworks`); land with squash `sy=1−0.12·beat` | beat's own ~150 ms decay is the hop envelope |
| `bass` | Ring flip: Saturn's ring plane rotates a full 360° when `bass−prevBass > 0.06` kicks while `bass > 0.3` — a bass drop flips the planet | spring-integrated tilt, relaxes to rest plane |
| `bands[48…63]` | Twinkle aura: each body emits 2–4 star sparks inside its own panel, alpha from high-band mean — the pet carries a little weather of stars | per-star `hashNoise` phase |
| `bands[21…47]` | Logo body's bounce amplitude + spin rate (the logo is the playful one — it tumbles, the planet glides) | direct |
| `waveL/waveR` | Moon's orbit = Lissajous of L/R (same as cosmos pane mode — the moon surfs the waveform in world space, radius `44 + 18·bass` pt) | — |
| `peak` | Glint dot riding Saturn's ring | PPM ballistics already applied |
| `clip` | Comet dash: the whole cast darts ~200 pt in a random direction trailing a terracotta streak, then resumes — error energy becomes slapstick | 0.7 s TTL, ease-out |
| `seq` stall | Sleep: bodies converge — moon docks onto planet, logo parks beside, cast settles onto the nearest perch or bottom edge, dimmed 30% | exponential ease to rest, then drive loop stops |

**Wander FSM (shimeji-idiomatic, `WorldState`):**
- **Walk the bottom:** patrol `visibleFrame`'s bottom edge (which already excludes the dock — free collision), pauses to idle-bob.
- **Lazy figure-eights:** Lissajous drift across the middle band of the screen — doubles as the "dancing" motion while music plays.
- **Perch:** sit on a window title bar. **Correction to the brief: this does NOT need AX.** `CGWindowListCopyWindowInfo(kCGWindowListOptionOnScreenOnly)` returns on-screen windows' bounds + owner names — public metadata, no TCC prompt, sandbox-fine. Perch targets = top edge of the largest visible window (excluding our own). AX is only needed if we want to *follow* a specific window's lifecycle — skip that.
- **Beat-hop chains:** on sustained `beat`, hops chain into a bouncy walk — the cast literally dances across the screen edge.

---

## 5. Lifecycle + UX

- **Toggle:** "Desktop Pet" in the Playback menu + VisualsPane — Off / While playing / Always. Default **While playing**: the pet is a viz, sleeping (or absent) when the music stops. "Always" gives the shimeji-companion experience.
- **Policy on app state:** minimize/hide Lyra → pet persists (the point). Lyra quits → panels close with the app — honest, no LSUIElement agent trickery (`.accessory` activation policy would kill the dock icon entirely — don't).
- **Click-through vs pettable:** two modes.
  - *Ghost* (default while playing): `ignoresMouseEvents = true` — pure ambiance, zero accident surface.
  - *Pet*: `ignoresMouseEvents = false` — whole panel is the hit target: drag a body (dragging Saturn drags the system; the Moon slingshots when released — orbit offset preserved), double-click to send it home, right-click for a tiny menu (Sleep / Recall / Hide). Trade-off: a draggable window can be lost under real windows or accidentally grabbed — solve with "shake to wake" reset and a menu-bar "Recall pet" item.
- **Multi-display:** the cast lives on the screen containing Lyra's window (fallback: `NSScreen.main`); display changes → re-clamp to new `visibleFrame`. Optional "follow Lyra's window" vs "pinned display" pref. Do NOT split the cast across displays — the orbit coupling reads broken.
- **Reduce Motion:** pet never spawns (or spawns parked, motionless). Consistent with the decorative-mode contract.
- **Reduce Transparency / autohide-dock:** pop falls back to bottom-edge spawn; perching unaffected.

---

## 6. Frame budget

- Per-body windows: 3 × ~140 pt surfaces at 30–60 Hz via `CADisplayLink`/`Timer` + `setFrameOrigin` — WindowServer moves of small windows are cheap; decades of oneko/shimeji clones prove it. Draw each body with the same CG painter as the dock tile (vector, no per-frame alloc). Budget: <1 % CPU, negligible GPU.
- Had we chosen the fullscreen overlay: one per-screen layer at 1440p+ compositing every frame — real cost (est. 3–8 % GPU continuously), which is exactly why we didn't. If a future "scene takeover" mode wants overlay scale, measure `CADisplayLink` frame time + WindowServer CPU first.
- The pet drive loop reuses `VizRuntime.pumpIfStale(0.033)` — when Lyra's window is open the mini surface pumps; when minimized, the pet becomes the pumper. One compositor, N consumers.

---

## 7. One scene graph, three surfaces — shared type design

The dock doc already split motion (`CosmosScene`) from painting. The pet adds a third *surface* and a new *world-space* layer — same scene, new dimension:

```swift
/// Motion driven by VizFrame — unchanged from dock-icon-viz.md.
struct CosmosScene {
    var moonAngle, moonOmega, ringTilt, kick: Float
    var starSeeds: [Star]            // srand48(20260916) — same sky everywhere
    var comet: Comet?                // clip → shooting star / comet dash
    mutating func drive(_ f: VizFrame, dt: Float)   // field→motion map
    var isAtRest: Bool
}

/// NEW: world-space layer — positions + wander FSM + body coupling.
/// Only exists for the pet; dock/pane run CosmosScene with fixed origins.
struct CosmosWorld {
    var bodies: [Body: CGPoint]      // .saturn, .moon, .logo in screen coords
    var vel: [Body: CGVector]
    var fsm: WanderState             // .walkBottom, .eight, .perch(CGWindowID), .sleep
    mutating func roam(_ f: VizFrame, dt: Float, bounds: CGRect) {
        // moon.x = saturn.x + orbitOffset(scene.moonAngle, waveL/waveR lissajous)
        // bodies.saturn follows fsm target at speed(level)
    }
}

/// One painter, three looks — all CGContext so every surface shares it.
enum CosmosPaint {
    static func draw(_ ctx: CGContext, _ r: CGRect, _ s: CosmosScene, look: Look)
    enum Look {
        case dock        // squircle-inset card, full star field (rests = AppIcon)
        case pane        // VizDraw.cosmos — full-bleed, Lissajous orbit trace, ring grains
        case pet(Body)   // single body + local twinkle aura, transparent bg
    }
}
```

Surface table:

| Surface | Scene | World | Painter | Drive |
|---|---|---|---|---|
| Dock tile (`DockIconView`) | `CosmosScene` (empties while pet is out) | — | `.dock` | 12 Hz `Timer` + `display()` |
| Viz pane (`cosmos`, mode 35) | `CosmosScene` | — | `.pane` | existing `TimelineView` 60 Hz |
| Desktop pet (3 `NSPanel`s) | `CosmosScene` | `CosmosWorld` | `.pet(body)` | 30 Hz `Timer`/`CADisplayLink` + `setFrameOrigin` |

Handoff detail: on pop, the pet's `CosmosScene` *adopts* the dock scene's state (moon angle, ring tilt, comet) so the transition is continuous — the same planet stepping out, not a respawn. On recall, state hands back.

---

## 8. Prior art + risks

- **Codex `/pet`** (OpenAI desktop app, 2026): floats above every window, draggable, doubles as progress surface — proof the UX pattern lands and that users accept a floating companion from a serious app. Note its open bug: sprite layer fails to render on multi-monitor (openai/codex#21088) — test the pet across display changes early.
- **codex-pet** (fzx2666-fz): open-source Swift pet — draggable circular window, hover-expand, saved position. Closest code reference.
- **ShimeTomo** (a35hie): SwiftUI shimeji for macOS 26; **shimeji4mac** (nonowarn): the original Mac shimeji via JNA; **R-Pets SPEC**: the flag-by-flag Electron→AppKit mapping used in §1; **Lowtech OSDWindow**: production overlay-window hygiene (`isReleasedWhenClosed`, `sharingType`, collectionBehavior).
- **Risks:** pop-origin estimate is approximate without AX (mitigated by the icon-crouch trick); draggable mode invites "lost pet" complaints (menu-bar recall item); `CGWindowList` perch targets shift as windows move — re-enumerate on a slow timer, not per frame; three moving windows can look jittery under WindowServer load — cap at 30 Hz and prefer one coupled update pass per tick.
