# Lyra Visualization Contract

Single source of truth for the viz subsystem. Two workstreams build against this:
`viz-core` (Rust: analyzers + FFI) and `viz-ui` (SwiftUI: renderers + controls).
Do not change the ABI shape without updating this file first.

## Data flow

```
PCM (post-DSP, pre-output) -> VizTap (engine, audio thread, no alloc)
  -> LyraVizFrame (C struct, one call per UI frame)
    -> Swift polling at ~60 Hz -> Canvas renderers
```

Rust ships **features**, not pixels. All particle systems, cellular automata,
scrollback rings (terrain/spectrogram), LED phosphor decay, and peak-cap
gravity live in the Swift layer, fed by `LyraVizFrame`. Rust state must be
bounded: FFT scratch preallocated, rings fixed-capacity, zero alloc on the
audio callback path.

## FFI payload (`modules/CLyraFFI/lyra.h`)

```c
typedef struct {
    float    bands[64];    /* normalized 0..1, log-spaced 20 Hz-20 kHz,
                              attack/decay ballistics applied           */
    float    wave_l[256];  /* strided-decimated recent PCM, newest last */
    float    wave_r[256];  /*   -1..1                                    */
    float    peak[2];      /* 0..1, PPM ballistics, L/R                  */
    float    rms[2];       /* 0..1, dBFS normalized (-60..0 -> 0..1)     */
    float    bass;         /* 0..1, mean energy of lowest ~4 bands       */
    float    beat;         /* 0..1, onset/transient pulse, ~150 ms decay */
    float    level;        /* 0..1, overall loudness (pulse modes)       */
    uint32_t clip;         /* sticky clip bitmask: bit0 L, bit1 R        */
    uint64_t seq;          /* monotonically increasing frame counter     */
} LyraVizFrame;            /* ~4.6 KB — one copy per UI frame            */

uint64_t lyra_engine_viz_frame(const void *engine, LyraVizFrame *out);
/* returns seq; Swift skips redraw when seq is unchanged */
```

Keep the existing `lyra_engine_viz`/`lyra_engine_viz_bands` working — the
transport-bar mini spectrum still uses them.

## Mode registry (34 modes; names mirror cliamp where applicable)

| # | Mode | Inputs | Notes |
|---|------|--------|-------|
| 1 | Bars | bands | smooth fractional bars |
| 2 | BarsDot | bands | halftone/stipple fill |
| 3 | Rain | bands,beat | droplets inside bar silhouettes |
| 4 | Outline | bands | top-edge trace only |
| 5 | Bricks | bands | segmented blocks w/ gaps |
| 6 | Columns | bands | many thin columns |
| 7 | ClassicPeak | bands | falling peak caps (UI state) |
| 8 | Wave | wave_l/r | mono oscilloscope trace |
| 9 | Scatter | bands | particle sparkle field |
| 10 | Flame | bands,bass | rising flame tendrils |
| 11 | Retro | bands,wave | synthwave perspective grid |
| 12 | Pulse | level,beat | pulsating ring/disc |
| 13 | Matrix | bands,beat | falling glyph rain |
| 14 | Binary | bands | streaming 0/1 field |
| 15 | Sakura | bands | falling petals |
| 16 | Firework | bands,beat | beat-triggered bursts |
| 17 | Bubbles | bands | rising hollow rings |
| 18 | Logo | bands | LYRA block wordmark reacts |
| 19 | Terrain | bands | scrolling ridge heightfield (UI ring) |
| 20 | Scope | wave_l,wave_r | Lissajous XY |
| 21 | Heartbeat | wave_l,level | ECG envelope trace |
| 22 | Butterfly | bands | mirrored Rorschach spectrum |
| 23 | Density | bands | shade-block field (cliamp Ascii) |
| 24 | Firefly | bands,bass | drifting fireflies |
| 25 | Mosaic | bands | flickering heatmap tiles (UI state) |
| 26 | Sand | bands | falling-sand automaton (UI state) |
| 27 | Geyser | bands,bass,beat | bass particle fountain |
| 28 | ClassicLED | bands,peak | LED matrix + peak caps (Winamp) |
| 29 | Stereo | peak,rms | L/R horizontal LED meters |
| 30 | Mirror | bands | spectrum mirrored on axis |
| 31 | Dither | bands | ordered-dither pixel field (cliamp Omarchy, no brand mark) |
| 32 | RedSector | bands | tumbling wireframe EQ + starfield |
| 33 | Spectrogram | bands ring | scrolling frequency heatmap (UI ring) |
| 34 | WaveSeek | WaveformPeaks | min/max seekbar, replaces plain Slider |

Modes are metadata-driven: `enum VizMode: Int, CaseIterable` with display name,
input needs, and a `Canvas` renderer each. `v`/`V` style cycling isn't needed —
use a proper picker (dropdown or chip strip).

## Ownership map

- **viz-core** owns: `crates/lyra-viz/**`, viz sections of
  `crates/lyra-engine/src/lib.rs`, `crates/lyra-ffi/src/lib.rs`,
  `modules/CLyraFFI/lyra.h`, viz tests. No Swift files.
- **viz-ui** owns: `app/Sources/LyraApp/Viz/**` (new dir — all renderers,
  mode enum, VizSurfaceView, mode picker, mock-frame provider),
  viz additions to `Bridge.swift`, viz-pane integration + elevation/shadow
  tokens in `ContentView.swift`/`Theme.swift`. No Rust files.
- Shared-file rule: lyra.h → viz-core only; Bridge/ContentView/Theme → viz-ui
  only. Zero overlap.

## Swift bridge shape

```swift
struct VizFrame {           // mirrors LyraVizFrame, value semantics
    var bands: [Float]      // 64
    var waveL: [Float]      // 256
    var waveR: [Float]
    var peak: (Float, Float)
    var rms: (Float, Float)
    var bass, beat, level: Float
    var clipLR: (Bool, Bool)
    var seq: UInt64
}
// LyraEngine.vizFrame() -> VizFrame  — wraps lyra_engine_viz_frame
// MockVizFrameProvider: deterministic synthetic frames (sine sweep, beat
// pattern) so UI work proceeds before FFI lands; same VizFrame type.
```

## Budgets & rules

- FFT scratch + all rings: allocated once at engine init.
- `lyra_engine_viz_frame`: short lock, memcpy out, no math under lock beyond
  what's already computed in the audio tap.
- Swift renderers: `Canvas` only; fixed-capacity particle pools (<= 512);
  no per-frame heap alloc in draw; honor `accessibilityReduceMotion`
  (decorative particle modes fall back to Bars; meters/scope/wave stay —
  they convey information).
- Pause behavior: when seq stops advancing, renderers ease to rest
  (cliamp does this — decay to rest then freeze).
- Visual style: sharp-editorial (square, hairline borders, Ui palette).
  Particle modes get muted variants of terracotta/mint/indigo — not neon.
- Tests (viz-core): sine->correct band index; silence->all zeros; white
  noise->broadband; stereo phase->scope traces differ; impulse->beat spike;
  ring capacity bounds; no NaN/inf under edge inputs.

## Elevation/shadow design token (viz-ui adds to Theme.swift)

bored.com-style hard offset shadow on interactive cards:
`.shadow(color: Ui.ink.opacity(0.15), radius: 0, x: 3, y: 3)` at rest;
hover lift = `offset(y: -2)` + `x:5, y:5` shadow, 120 ms easeOut.
Apply to clickable cards/pane surfaces, not static text.
