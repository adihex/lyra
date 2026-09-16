# Lyra — dino-shred Integration Spec

Date: 2026-09-16. Source: `github.com/adihex/dino-shred` @ HEAD (cloned to
`./dino-shred`; `main.py` 376 LOC, `dino_shred.ipynb` is the same code split into a
tutorial — same constants, same classes, plus teaching notes). Context: Lyra's
SwiftUI shell + Rust core over C FFI; VizFrame pipeline @ ~60 Hz → SwiftUI Canvas
renderers (64 bands, L/R waveform, peak/rms, bass, beat pulse, level, clip);
cliamp-derived viz mode registry (see `cliamp-viz-inventory.md`); `Ui.ink`/`Ui.bg`
sharp-editorial palette; existing NSEvent local key-monitor for table nav.

TL;DR: ship it as a **hidden viz mode** (`dino`) rendered in the existing
VizSurface Canvas, game logic in Swift at a fixed 60 Hz timestep, obstacles
quantized to `beat` and scroll speed driven by `level`/`bass` — the game surfs
the track. ~700 LOC Swift, one new file plus registry/key-monitor/UserDefaults
touch points.

---

## 1. Source mechanics (extracted from `main.py`)

The game is a faithful Chrome-Dino runner skeleton: fixed-x player, world scrolls
left, one-button jump + hold-to-duck, AABB collision, progressive speed.

### 1.1 Constants

| Constant | Value | Notes |
|---|---|---|
| `SCREEN_W` / `SCREEN_H` | 800 / 400 | logical canvas; port keeps this space and letterbox-scales |
| `FPS` | 60 | all physics are **per-frame** constants, not per-second |
| `GROUND_Y` | 310 | horizon line y (origin top-left, +y down) |
| `GRAVITY` | 0.8 px/frame² | applied only while airborne |
| `JUMP_VEL` | −14.0 px/frame | apex ≈ 122 px, airtime ≈ 35 frames (~0.58 s) |
| `BASE_SPEED` / `MAX_SPEED` | 6.0 / 14.0 px/frame | scroll speed |
| `SPEED_UP_PER_POINT` | 0.01 | `speed = min(14, 6 + score·0.01)` → max at score 800 |
| `OBSTACLE_SPAWN_MIN/MAX` | 50 / 120 frames | initial spawn window (0.83–2.0 s) |
| Spawn window shrink | `min: max(25, 50−3·Δsp)`, `max: max(50, 120−6·Δsp)` | `Δsp = int(speed − 6)`; at max speed (Δsp=8) window = 26–72 — the 25/50 clamps never engage |
| Dino `W`/`H`/`DUCK_H`/`X` | 44 / 48 / 28 / 80 | fixed x, hitbox shrinks when ducking |
| Dino hitbox | `(x+4, y+4, W−8, h−4)` | 4 px inset — forgiving |
| Obstacle hitbox | `(x+2, y+2, w−4, h−4)` | 2 px inset |
| Ground dashes | 12 px dash / 6 px gap, y = 312 | `scroll = (scroll + speed) mod 18` |
| Legs anim | 150 ms toggle, ±3 px | `get_ticks()//150 % 2` |

### 1.2 Obstacle variants

| Variant | Size (w×h) | Spawn rule | y |
|---|---|---|---|
| 0 small cactus | 20×40 | always eligible | `GROUND_Y − h` = 270 |
| 1 tall cactus | 24×54 | always eligible | 256 |
| 2 bird high | 42×20 | score ≥ 200 | `GROUND_Y − h − 100` = 190 |
| 3 bird low | 42×20 | score ≥ 500 | `GROUND_Y − h − 60` = 230 |

Variant weights by score: `<200` → uniform {0,1}; `<500` → `[3,3,1]` over {0,1,2};
`≥500` → `[2,2,2,1]` over {0,1,2,3}.

### 1.3 State machine

```
START ──space/↑──▶ PLAYING ──AABB hit──▶ GAME_OVER
                     ▲                      │
                     └────space/↑──_reset()─┘
```

- `update()` early-returns unless `PLAYING`. Per frame, in order:
  1. recompute speed from score; 2. dino physics; 3. ground scroll;
  4. move obstacles, cull `x+w<0`; 5. `spawn_timer++`, spawn on `next_spawn_in`
     then re-draw window (shrunk by speed); 6. `score++`; 7. AABB check → GAME_OVER.
- Input abstraction: `jump()` (only when `on_ground`), `duck(bool)`.
  space/↑ is state-dependent: START→play, GAME_OVER→reset+play, PLAYING→jump.
  ↓ is hold-to-duck (KEYDOWN→true, KEYUP→false).

### 1.4 Honest findings (matters for the port)

- **Scoring is +1 per *frame***, i.e. ~60 pts/s — the code comment "1 point per 6
  frames" is wrong. HI scores reach thousands fast. Port option: keep +1/frame
  internally (faithful) but display `score` as-is; or divide display by 10 for
  Chrome-like numbers. Decision: keep faithful, display raw.
- **"Object pooling" is a Python list** — spawn appends, off-screen culls. No
  reuse pool exists; in Swift a plain `[Obstacle]` + `removeAll(where:)` is a
  faithful port (or a real ring pool — trivial either way, ≤6 live objects).
- **Bird geometry is broken-but-charming**: bird-low bottom edge = 250, standing
  dino top = 266 — birds *never* hit a grounded dino, and ducking dodges nothing.
  Birds only threaten a mid-jump dino. The tutorial's "must duck / must jump"
  claim doesn't hold under the shipped constants. **Port fix**: drop bird-low to
  `GROUND_Y − h − 38` (bottom 272 > dino top 266 → forces a duck *under* it) and
  bird-high to `−70` (punishes jumping, still clearable by staying grounded).
  Keep the fix behind a `tunedGeometry` flag so a "faithful" toggle is possible.
- **No fast-fall**: real Chrome dino drops faster when ↓ is held mid-air; this
  clone's ↓ only toggles the hitbox. Worth adding in the port
  (`if !onGround && ducking { vy += GRAVITY·1.5 }`) — feels better, costs 3 lines.
- Colours: bg `WHITE`, dino/ground `DARK(50,50,50)`, cacti `GREY(83,83,83)`,
  birds `(120,120,120)` — already grayscale; maps 1:1 onto `Ui.ink` at varying
  opacities on `Ui.bg`.
- Render recipe: filled body rect, white eye dot (offset differs standing vs
  ducking), two leg rects toggling ±3 px @150 ms, horizon line + scrolling dashes,
  HUD `Score: %05d` + `HI: %05d` top-right, overlays = 47%-alpha white veil +
  centered title/subtitle.

---

## 2. Where it lives — recommendation: hidden viz mode

**Recommend option C: a 35th viz "mode" (`dino`) inside VizSurface, excluded
from the normal mode cycle, reached by an easter-egg trigger.**

Why, argued from the app's aesthetic:

- The Chrome dino *is* editorial line art — black pixel ink on paper white.
  Rendered as `Ui.ink` on `Ui.bg` it looks like it was designed for Lyra, not
  bolted on. No other game survives that palette this cleanly.
- A viz-mode slot gets VizFrame delivered for free (same pipe every other mode
  consumes) — the music-sync twist in §4 needs zero new plumbing.
- It inherits mode chrome for free: mode cycling, the viz pane's existing
  layout/letterbox, hide-when-stopped behavior. No window, no menu item, no
  settings surface to design.

Rejected:

- **Chrome-style offline/error state**: Lyra is local-first; its "offline" is
  the normal case. A stream-error dead state happens but is rare and
  interrupt-driven — the game would be discovered by accident in the worst
  moment (user's stream just died; now a dinosaur eats the error message).
- **Separate window / Konami-only trigger**: window chrome breaks the single-pane
  editorial layout; a pure Konami is undiscoverable even for an easter egg.

**Trigger design** (pick both, they're ~20 lines):

1. Type `dino` while the viz pane has focus — buffered-key easter egg in the
   existing key monitor (4-key rolling buffer, 1.5 s expiry). Exiting: `Esc` or
   cycling the mode key returns to previous mode.
2. Agent-native hook for free: `viz.mode.set dino` over the control socket +
   `lyra://viz/dino` deep link (per `agent-native-design.md` both surfaces are
   thin adapters on one dispatcher). Lets scripts/agents summon it — very on-brand.

---

## 3. Port architecture — SwiftUI-side, fixed timestep

**Recommendation: game logic + rendering in Swift, inside the viz layer. Do not
put it in the Rust core.**

- Everything the game touches lives Swift-side already: VizFrame snapshots are
  consumed by renderers, the key monitor is Swift, UserDefaults is Swift. The
  only thing Rust would add is letting agents `operation.submit` a jump — cute,
  not worth a per-frame FFI bridge and a second collision of game state with
  engine state. If the agent crowd wants it later, expose *read-only* game state
  (`score`, `state`) in `state.get` — one struct copy, not a logic port.
- Canvas rendering: the game is ~10 rects + 2 text runs + a dashed line — well
  under the per-frame cost of the existing Sand/Geyser ports. No SpriteKit, no
  Metal; `Canvas` + `GraphicsContext` is the same pattern as every viz renderer.

### 3.1 The one real port risk: frame-coupled physics

Every pygame constant is **per-frame at 60 Hz**. A naive `TimelineView` port that
applies `vy += 0.8` per callback will drift on any display that isn't exactly
60 Hz (ProMotion 120 Hz runs callbacks at 120). Required:

```swift
// fixed-timestep accumulator — the viz frame clock already does this pattern
let dt = context.date.timeIntervalSince(lastTick)
acc = min(acc + dt, 4.0/60)            // clamp: max 4 steps, same cap as cliamp's frame clock
while acc >= 1.0/60 { model.step(); acc -= 1.0/60 }
```

`model.step()` applies the per-frame constants verbatim. All constants in §1.1
port 1:1. Coordinate space: keep the logical 800×400 and scale
`canvasScale = min(w/800, h/400)` with letterbox — preserves jump feel at any
pane size.

### 3.2 Structure

```swift
@Observable final class DinoShredModel {   // pure logic, no SwiftUI — unit-testable
    enum Phase { case start, playing, gameOver }
    var dino: Dino; var obstacles: [Obstacle]; var ground: Ground
    var score, highScore, speed, spawnTimer, nextSpawnIn, phase …
    func step(viz: VizFrame?)              // 1/60 s tick, §1.3 order
    func jump(); func duck(_: Bool)        // input actions, not keys
}
struct DinoShredRenderer {                  // Canvas draw, mirrors draw() order
    func draw(_ m: DinoShredModel, in ctx: GraphicsContext, size: CGSize)
}
```

`Obstacle` is a struct (variant, x, size); `Dino` a struct (y, vy, flags).
AABB = `CGRect.intersects` on the padded rects from §1.1/§1.2.

---

## 4. The music-synced twist — concrete VizFrame mapping

Goal: the track *deals* the obstacles; the dino surfs them. Design principle —
**music modulates pacing and texture, never fairness**: every mapping below is
clamped so the game remains playable on silence, noise, or blast-beat metal.

| VizFrame field | Game mapping | Clamp / rule |
|---|---|---|
| `beat` (pulse) | **Spawn quantization.** When `spawnTimer ≥ nextSpawnIn`, set `pendingSpawn`; obstacle appears on the next `beat` rising edge. | Fallback timer: if no beat within 600 ms of eligibility, spawn anyway (ambient/drone tracks still play). Beats during the min-window don't spawn early — quantization delays, never accelerates. |
| `level` + `bass` | **Scroll speed.** `speed = clamp(scoreSpeed, baseFloor, max)` where `scoreSpeed = 6 + score·0.01` (faithful ramp) and `baseFloor = 5 + level·4 + bass·3`. Loud sections literally speed the world up; a quiet bridge is a breather. | Floor capped at 12 so score still owns the endgame; `MAX_SPEED` 14 absolute. Music can *raise* speed above the score curve but never lower it below `BASE_SPEED`. |
| `bands[0..7]` (sub/low) | **Near parallax** — a second dash layer (hills/tufts) scrolling at 0.5× speed, dash heights ∝ low-band energy. | Heights eased with the same 34-up/10-down rates the other renderers use — visual only. |
| `bands[48..63]` (air) | **Far parallax** — sparse cloud/pixel layer at 0.25× speed, density ∝ high-band energy. | Deterministic hash placement (reuse scatterHash-style constant so it's stable frame-to-frame). |
| `beat` | **Landing dust + bird flap.** Dino landing on a beat spawns a 3-particle ink puff; bird wing-flap phase advances on beats (±1 subdivision by frame parity). | Particles capped at 8; skip under Reduce Motion. |
| `clip` | **Ink invert.** While `clip` is true, dino renders `Ui.bg`-on-`Ui.ink` (inverted flash) — reads as "the master is redlining, the dino is glowing". | Max 2 flashes/s; honor Reduce Motion (then it's a static outline change). |
| `rms` vs `peak` | **Score tick color weight** — HUD score brightens with `rms/peak` crest factor. | Cosmetic only. |
| stale/nil VizFrame | **Classic mode** — no playback → `pendingSpawn` uses the timer path every time (beat channel silent), speed = faithful score ramp, parallax static. | This is the deliberate "Chrome offline dino" homage: the game is *more* itself when nothing plays. Detect via `isPlaying` flag on the viz state, not frame staleness. |

**"Shred" combo (the named twist):** clearing an obstacle (its right edge passes
dino x) *within ±150 ms of a beat* increments `combo`; each combo step adds
+0.1× score multiplier and a tick of ink splatter at the dino's feet. Any hit or
non-beat clear resets combo. `combo ≥ 8` → mode title flashes "SHREDDING".
Cheap (~40 LOC), gives the game a skill axis that only exists because of the
music, and justifies the name.

**Fairness invariant:** beat-quantized spawns still respect the post-spawn
minimum window — after each spawn, `nextSpawnIn` is drawn from the same
speed-scaled window as classic, so a 180 BPM track can't machine-gun obstacles
denser than 25 frames apart.

---

## 5. Input — ownership rules

The existing `NSEvent.addLocalMonitorForEvents` table-nav monitor gains a
**capture delegate**: one optional `KeyCapturing` client at a time, checked
before normal dispatch.

```
if let game = keyCaptureOwner, game.handleKey(event) { return nil /* swallowed */ }
// else fall through to existing table nav / transport handling
```

Capture ownership rules (explicit, all required):

- DinoShredModel requests capture when (a) mode == `dino`, (b) window is key,
  (c) viz pane has focus. Releases on any violation + on `Esc`.
- While captured: `space`/`↑` = jump (start/retry by phase), `↓` = duck —
  **including swallowing `space` so it does NOT toggle play/pause.** This is the
  intended conflict resolution: inside the game, space belongs to the dino.
  Play/pause remains reachable via transport bar click, media keys (separate
  `NSEvent` subtype — still global), and `lyra://transport/toggle`.
- `Esc` releases capture and reverts to the previous viz mode. First-run hint in
  the START overlay: `space to run · esc to leave`.
- Monitor must distinguish key-down vs key-up (pygame maps duck to both events);
  auto-repeat `space` key-downs should re-fire `jump()` — harmless since `jump()`
  no-ops airborne, and matches pygame behavior.

Risk callout: a capture delegate that forgets to release is a "spacebar is dead"
bug — gate it behind a single `KeyCapture.owner` property and unit-test the
release paths (Esc, mode change, focus loss, window blur).

---

## 6. State, chrome, coexistence

- **Focus loss**: `NSWindow.didResignKeyNotification` → if `phase == .playing`,
  push a `paused` sub-state (same veil overlay, "paused — click or space").
  TimelineView also stops firing when the pane is occluded; the accumulator's
  4-step clamp already prevents a physics burst on resume — keep that clamp.
- **High score**: `UserDefaults.standard.integer(forKey: "lyra.dino.highScore")`,
  written on GAME_OVER only (not per frame). Also surface `dino.highScore` in
  `state.get` — agents will find it funny, costs one field.
- **Reduce Motion** (`NSWorkspace.accessibilityDisplayShouldReduceMotion` or the
  SwiftUI env value): disables both parallax layers, landing particles, clip
  flash (→ static outline), and the SHREDDING flash. Gameplay unchanged — the
  physics and beat-spawning are information, not decoration.
- **Transport bar**: untouched. The game lives strictly inside the viz rect;
  transport keeps working by mouse and media keys. If the viz pane supports
  full-window takeover, game scales with it (letterboxed 800×400 logical).
- **Idle battery**: TimelineView + Canvas already degrade when hidden; mode only
  ticks while visible. `paused`/non-`dino` → zero cost (the mode isn't even
  allocated until triggered).

---

## 7. Effort estimate

| File | Change | Est. LOC |
|---|---|---|
| `Viz/DinoShred/DinoShredModel.swift` (new) | state machine, physics, spawn/variant tables, AABB, beat-quantized spawn, combo | ~330 |
| `Viz/DinoShred/DinoShredRenderer.swift` (new) | Canvas draw: dino+eye+legs, obstacles, ground, parallax, HUD, overlays | ~220 |
| `Viz/VizMode.swift` (or equivalent registry) | `case dino` + `hidden` flag so cycle skips it | ~15 |
| `Viz/VizSurface.swift` | mode switch branch, TimelineView→`step(viz:)` wiring, capture plumbing | ~60 |
| key monitor file | `KeyCapture` owner/delegate + `dino` trigger buffer | ~70 |
| `state.get` / IPC snapshot | optional `dino` field | ~10 |
| UserDefaults key + reduce-motion reads | | ~15 |
| **Total new Swift** | | **~700** (faithful core ≈ 420; music-sync layer ≈ 200) |

Tests: `DinoShredModel` is pure — port the spawn-window/collision rules as
table-driven tests (jump arc clears cactus, grounded run clears both birds under
tuned geometry, AABB padding, spawn quantization + 600 ms fallback). ~150 LOC.

Risks, ordered:

1. **Key capture leaks** — spacebar dead globally. Mitigate: single owner
   property, release on every exit path, unit test.
2. **Timestep drift** — per-frame constants on non-60 Hz displays. Mitigate:
   fixed-dt accumulator (mandatory, not optional).
3. **Beat-detector starvation** — sparse/misdetected beats stall spawns.
   Mitigate: 600 ms pending-spawn fallback; min-window invariant.
4. **Fairness at high level** — `level` floor pushing speed to 12+ makes spawn
   windows tight. Mitigate: floor cap 12; spawn window already scales with speed.
5. **Scope creep** — sprites, SpriteKit, Rust port, agent-playable API. All
   explicitly out; revisit only if the easter egg lands well.

---

## Appendix A — faithful-port checklist

- [ ] Fixed 60 Hz accumulator, 4-step clamp
- [ ] Constants table §1.1 verbatim (logical 800×400, letterbox scale)
- [ ] Update order: speed → dino → ground → obstacles → cull → spawn → score → AABB
- [ ] Spawn variant gates at score 200 / 500 with the weight tables
- [ ] Spawn window shrink formula + clamps (25/50)
- [ ] Padded hitboxes (dino 4 px, obstacle 2 px)
- [ ] `tunedGeometry` flag: fixed bird offsets (−38 / −70) on by default
- [ ] Hold-to-duck + optional mid-air fast-fall
- [ ] START / GAME_OVER veils, `Score:%05d` + `HI:%05d` top-right
- [ ] Legs toggle on a 150 ms clock, not frames
- [ ] `Ui.ink`-family greys on `Ui.bg`; clip → inverted dino
