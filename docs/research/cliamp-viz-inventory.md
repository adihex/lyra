# cliamp visualizer — implementation spec

Source: `github.com/bjarneo/cliamp` @ `81ec636` (2026-09-15). Files: `ui/visualizer.go`,
`ui/fft.go`, `ui/tick.go`, `ui/vis_*.go`, `ui/model/tick.go`, `player/tap.go`, `ui/styles.go`.

Terminology used throughout:

- `W` = panel width in terminal cells (`PanelWidth`, = terminal width − 2·`PaddingH`; `PaddingH` = 3,
  so 74 cells on an 80-col terminal). `H` = `v.Rows`, panel height in terminal rows
  (default `DefaultVisRows` = 7; config `vis_rows` 1–40; compact layout = 5; full-vis = height−8).
- **Dot grid** = `W*2` wide × `H*4` tall (Braille sub-cells). `dotCols = W*2`, `dotRows = H*4`.
- **Braille cell**: rune `U+2800 | bits`; bit map (row,col)→bit:
  `{{0x01,0x08},{0x02,0x10},{0x04,0x20},{0x40,0x80}}` — standard dot numbering (1,2,3,7 | 4,5,6,8).
- **Tier/color**: `specTag(n)`: `n ≥ 0.6 → 2` (SpectrumHigh), `≥ 0.3 → 1` (SpectrumMid), else `0` (SpectrumLow);
  `-1` = unstyled. Default ANSI: Low=10 (bright green), Mid=11 (bright yellow), High=9 (bright red).
  Themed: `t.Green`/`t.Yellow`/`t.Red`. `ColorDim` = ANSI 7 / `t.FG`.
- `bands` = last `Analyze()` output (`v.bands`); `smoothed` = `v.SmoothedBands()` (per-frame eased copy).
- `frame` = `v.frame`, a tick-count clock — **not** wall time (see §1.6). All hash-driven modes are
  pure functions of `(bands, frame, geometry)` — no hidden RNG state.

---

## 1. ANALYSIS PIPELINE

### 1.1 Sample tap (`player/tap.go`)

- Stereo ring buffer `[][2]float64`, size `max(4096, speakerFrames + 4096)`.
  `speakerFrames = sr * BufferMs/1000`; `BufferMs` default 250 (range 50–5000) ⇒ ~15k frames @44.1k.
- Tap sits **before volume control** (pre-volume amplitude), post-EQ/decode.
- `SamplesInto(dst)`: mono mixdown `(L+R)/2` of the last `len(dst)` frames **ending at the newest
  written frame** — i.e., ahead of the audible position by up to `speakerFrames` (~250 ms default:
  band modes effectively "predict" the near future). Used by all band modes.
- `WaveformSamplesInto(dst)`: same mixdown but the window ends at the **audible position**:
  `end = written − speakerFrames + elapsedFrames`, where `elapsedFrames` is wall-clock time since the
  last backend refill × sample rate; clamped to `[0, written]`. Advances smoothly between backend
  bursts. Used by `BandCount==0` modes (Wave/Scope/Heartbeat) **and ClassicPeak**.
- `StereoSamplesInto(dst)`: last N raw `[2]float64` stereo frames ending at write pos (latest-written,
  not audible-aligned). Used only by Stereo.
- `visVolumeLinked` (config `vis_volume_linked`, **default true**): samples scaled by
  `gain = 10^(volumeDb/20)`, `volumeDb ∈ [volMin≈−50, +6]`, applied in the model's Analyze/Stereo
  wrappers. Mono mode: stereo pairs replaced by their mid before gain.

### 1.2 `Analyze(samples, spec)` (`visualizer.go:680`)

Spec = `{BandCount, FFTSize}`; normalized: `BandCount<0→0`, `FFTSize≤0→2048`.

1. `waveBuf = copy(samples)` — always, even for band modes; `nil` input ⇒ empty.
2. **Silence gate**: `len==0` or `maxAbs < 1e-5` ⇒ `bands[b] = prev[b]*0.8` per call; return. No FFT.
3. Window: **symmetric Hann** `w[i] = 0.5·(1 − cos(2πi/(N−1)))` (denominator N−1, not N).
   Zero-pad to N; only first `min(len,N)` samples used.
4. FFT: radix-2 in-place Cooley-Tukey (`fft.go`), twiddles `w[k]=e^{−2πik/N}`.
5. Power spectrum `P[k] = re²+im²` for bins `1..N/2−1`; `P[0]=0` (DC dropped).
6. `binHz = sr/N` (21.5 Hz @44.1k/2048; 10.8 Hz @44.1k/4096).

### 1.3 Band edges (`buildSpectrumEdges`)

Anchors `A = [20, 100, 200, 400, 800, 1600, 3200, 6400, 12800, 16000, 20000]` Hz (11 anchors).

```
for i in 0..count:  pos = i*10/count  (anchor-space position)
  idx = floor(pos); frac = pos - idx
  if idx >= 10:        edge[i] = 20000
  elif frac == 0:      edge[i] = A[idx]
  else:                edge[i] = 10^( log10(A[idx])·(1-frac) + log10(A[idx+1])·frac )   # log interp
```

`count=10` reproduces the anchors exactly. Other counts (ClassicPeak=64, ClassicLED≈25) get
geometric interpolation *between* anchors — piecewise-log curve, not a single log span.

### 1.4 Band level (`averageSpectrumRangeLinear` + normalization)

```
lo = clamp(edge[b]  /binHz, 1, N/2-1)          # bin 1 min: DC excluded
hi = clamp(edge[b+1]/binHz, lo, N/2-1)
n  = clamp(ceil((hi-lo)*2), 4, 32)              # 4..32 samples per band
avgP = mean over i of lerp(P, lo + (i+0.5)/n * (hi-lo))   # linear-interp'd power bins
level = clamp( (10*log10(avgP) + 10) / 50, 0, 1 )         # −10 dB → 0, +40 dB → 1 (power dB)
```

**In-Analyze temporal smoothing** (per spec, runs at analysis cadence ~30 Hz):

```
if new > prev:  bands[b] = new*0.6  + prev*0.4    # fast attack
else:           bands[b] = new*0.25 + prev*0.75   # slow decay
prev[b] = bands[b]
```

`prev` is per-spec (`prevBySpec` map) so ClassicPeak's 64-band history doesn't alias the 10-band one.

### 1.5 `smoothedBands` — second-stage per-frame easing

Every `BandCount>0` driver tick calls `advanceSmoothing(now)`:

```
smoothed[i] = classicPeakStep(smoothed[i], bands[i], dt)
  = cur + (target-cur) * (1 - exp(-rate*dt))
  rate = 34.0 rising / 10.0 falling          # classicPeakBarRiseRate / FallRate
dt = real elapsed; clamped to ≤160ms (10×TickAnim); default 16ms
```

On spec/mode change `smoothed` snaps to `bands` once. All `renderOnlyDriver` modes and the
field/CA drivers consume `smoothed`; ClassicPeak, ClassicLED and Stereo do their own ballistics on
`v.bands`/raw samples instead.

### 1.6 Cadences, frame clock, pause/overlay

Tick intervals (`ui/tick.go`):

| const | value | use |
|---|---|---|
| `TickAnim` | 16 ms (~60 FPS) | bar/wave/scope drivers; UI tick when `--visualizer-60fps` or raw-sample mode |
| `TickWave` | = `TickAnim` | Wave/Scope/Heartbeat requested cadence |
| `TickFast` | 50 ms (20 FPS) | default playing cadence; all `newRenderOnlyDriver` modes |
| `TickAnalyze` | 33 ms (~30 Hz) | min spacing between FFT runs / stereo samples |
| `TickSlow` | 200 ms | overlay active, stopped, VisNone |
| `TickIdle` | 1500 ms | fully idle |
| `TickLowPowerPlaying` | 500 ms | low-power mode while playing |

- **FFT decoupled from render**: `defaultDriverTick` runs `Analyze` only when `BandCount==0` (every
  tick — refreshes `waveBuf`, no FFT) or `now − lastAnalyze ≥ TickAnalyze`.
- **`v.frame` clock**: `frame += animationSteps(now, driverInterval)`. If interval ≥ `TickFast` ⇒ +1/tick.
  If < `TickFast` (16 ms drivers) ⇒ `steps = elapsed/interval`, **capped at 4 steps/tick**, remainder
  carried. So at 20 FPS UI, 16 ms-clock animations still run at full speed (~3 steps/tick).
- **Actual UI cadence while playing** (`model/tick.go`): `TickAnim` if `visualizer60FPS` flag or
  `UsesRawSamples()` (BandCount==0, incl. Stereo); `min(driverInterval, TickFast)` for ClassicPeak;
  else `TickFast`.
- **Paused**: `frame` does **not** advance (returns before `frame +=`). Model feeds `Analyze(nil)` →
  bands decay ×0.8/analysis; raw modes' `waveBuf` cleared. Ticks continue at `TickFast` until
  `pausedSettled` (all `bands` and `smoothedBands` < 0.01, `waveBuf` empty, `visPauseSettler` OK:
  Sand needs `explosionTTL==0 && no particles`; Geyser needs no particles), then `Suspend()`.
- **Overlay**: drivers early-return; `lastAnalyzeAt`/`lastSmoothTick`/frame timing reset → next tick
  after dismiss analyzes immediately with a 1-frame dt.
- **Frame fitting**: each line clipped at `W` cols (ANSI sequences still emitted after clip), padded
  with spaces to `W`, truncated to `H` lines (`fitVisualizerFrame`).

### 1.7 Shared helpers

- `visBandWidth(b)`: per-band cell width. `visible=min(bands,W)`; `gaps=min(visible−1, max(0,W−visible))`;
  `bandCols=W−gaps`; `base=bandCols/visible`, first `bandCols%visible` bands get +1. At W=74,B=10:
  widths 7,7,7,7,7,6,6,6,6,6 + 9 gaps.
- `interpolateBandColumns(bands, widths)`: per-column level = `lerp(band[b], band[b+1], c/width[b])`.
- `resampleBandsLinear(bands, M)`: `out[c] = lerpSample(bands, c/(M−1)·(N−1))` (endpoint-exact).
- `fracBlock(level, rowBottom, rowTop)`: `≥rowTop→'█'`; `>rowBottom→ barBlocks[int(frac*8)]` over
  `" ▁▂▃▄▅▆▇█"` — **frac<1/8 renders ' '** (slivers vanish); `≤rowBottom→' '`.
- `shadeBlock` (Ascii): `≥rowTop→'█'`; frac `≥0.75→'▓'`, `≥0.5→'▒'`, `≥0.25→'░'`, else `' '`.
- `scatterHash(band,row,col,frame)`: `f=(frame+row*3+col)/3`; `h=band*7919+row*6271+col*3037+f*104729`;
  `h^=h>>16; h*=0x45d9f3b37197344b; h^=h>>16; → (h%10000)/10000` — deterministic "twinkle" PRNG;
  effective randomness changes every 3 frames.
- `rng64(state)`: LCG `s = s*6364136223846793005 + 1442695040888963407`, out `(s>>33)%1000/1000`.
- `bandAvg(b,lo,hi)`: mean of slice range.
- `brailleGrid`: `int8` dot buffer, `set()` keeps **max** tier per dot; `ensure()` **clears** when
  dims match (reused canvas). `render()` packs 4×2 → braille, cell wears max dot tier.
- `specWrap(rowBottomNorm, line)` / `flushStyleRun`: wrap a run in the cached ANSI pair for its tier.

---

## 2. MODE REGISTRY (`visModes`, cycle order)

32 entries + `None` (no-op) + `VisCount` sentinel; Lua plugins append after `VisCount` (spec {10,2048},
default cadence, renderer callback receiving **raw `v.bands`, not smoothed**). Order:

```
Bars BarsDot Rain BarsOutline Bricks Columns ClassicPeak Wave Scatter Flame Retro Pulse
Matrix Binary Sakura Firework Bubbles Logo Terrain Scope Heartbeat Butterfly Ascii Firefly
Mosaic Sand Geyser ClassicLED Stereo Mirror Omarchy RedSector None
```

Driver classes:
- **`newFastRenderOnlyDriver(spec, TickAnim/TickWave, fn)`** — stateless; requests 16 ms while playing:
  Bars, BarsDot, BarsOutline, Bricks, Columns, Ascii, Mirror, Omarchy (TickAnim); Wave, Scope,
  Heartbeat (TickWave, spec BandCount 0).
- **`newRenderOnlyDriver(spec, fn)`** — stateless; `TickFast` while playing:
  Rain, Scatter, Retro, Pulse, Matrix, Binary, Sakura, Firework, Bubbles, Logo, Butterfly, Firefly.
- **Custom drivers** (own `Tick`/state): ClassicPeak, Flame, Terrain, Mosaic, Sand, Geyser,
  ClassicLED, Stereo, RedSector.

Per-mode below: **In** = inputs; **Spec** = `{bands, FFT}`; **State** = persistent state;
**Algo** = core loop; **Color** = tier mapping; **Diff** = port difficulty.

### Bars — `vis_bars.go`
- **In**: smoothed (10). **Spec**: {10, 2048}. **Tick**: TickAnim. **State**: none.
- **Algo**: per row r (0=top): `rowBottom=(H−1−r)/H`, `rowTop=(H−r)/H`; per band emit
  `fracBlock(level,rowBottom,rowTop)` × `visBandWidth(b)` cells + 1-space gap.
- **Color**: `specWrap(rowBottom)` — bottom rows green → top rows red (tier = row position).
- **Diff**: trivial. Fractional block = 1/8-row steps; port as `fillRect` to fractional height.

### BarsDot — `vis_bars_dot.go`
- **In**: smoothed. **Spec**: {10, 2048}. **Tick**: TickAnim. **State**: none.
- **Algo**: braille stipple bars: dot `(dr,dc)` in cell lit iff `dotY=(dotRows−1−(row*4+dr))/dotRows < band`.
  i.e., all dots below the fractional level lit — solid stipple, not scattered.
- **Color**: `specTag(rowBottom)` per run; gaps styled.
- **Diff**: trivial — or draw dithered-fill rects natively.

### Rain — `vis_rain.go`
- **In**: smoothed. **Spec**: {10, 2048}. **Tick**: TickFast. **State**: none (all from `frame`).
- **Algo** (per column c inside band b's span, only where `rowNorm < level`):
  ```
  gate:  scatterHash(b,0,c, frame/12) > level*1.6+0.1  ⇒ column dark (activity ∝ level, re-gated ~every 12 frames)
  seed   = c*7919 + 104729
  speed  = 1 + seed%3            # frames per row-step
  dropLen= 2 + (seed/7)%3        # 2..4 rows
  cycle  = H + dropLen + 3; pos = (frame/speed + (seed/13)%cycle) % cycle
  dist = pos − row:  0→'┃'(tag2), 1→'│'(tag1), ≥2 & <dropLen→':'(tag0)
  ```
- **Diff**: medium — deterministic cell lookup; port as per-column drop sprites.

### BarsOutline — `vis_bars_outline.go`
- **In**: smoothed. **Tick**: TickAnim. **State**: none.
- **Algo**: emit `'─'`×width only on the row where `rowBottom < level < rowTop` (the bar's top edge);
  spaces elsewhere + gap.
- **Diff**: trivial — a 1px line at each bar's peak.

### Bricks — `vis_bricks.go`
- **In**: smoothed. **Tick**: TickAnim. **State**: none.
- **Algo**: per row, threshold `t=(H−1−r)/H`; band cell = `'▄'`×width if `level>t` else spaces; +gap.
  Half-height blocks ⇒ solid bars with ½-row gaps.
- **Diff**: trivial — integer-row-quantized bars.

### Columns — `vis_columns.go`
- **In**: smoothed. **Tick**: TickAnim. **State**: none.
- **Algo**: `cols = interpolateBandColumns(bands, visBandWidth per band)`; each column
  `fracBlock(colLevel, rowBottom, rowTop)`, 1-cell gap between band groups.
- **Trick**: within a band's columns, level lerps toward the *next* band — smooth dense spectrum.
- **Diff**: trivial.

### ClassicPeak — `vis_classic_peak.go` (custom driver)
- **In**: `v.bands` raw (own ballistics; not smoothed). **Spec**: **{64, 4096}** and
  `WaveformSamplesInto` (audible-aligned window).
- **Geometry**: `cols = (W+1)/2` bars, 1 cell wide + 1 gap. `levels = classicPeakBands(bands,cols)`:
  when shrinking 64→cols, **area-weighted average** of overlapping bands; when expanding, linear resample.
- **State** (per column): `barPos`, `peakPos`, `peakVel`, `peakHold`; `lastTick`, `bandsAt`.
- **Tick**: if playing, analyze when `now−bandsAt ≥ analysisInterval = max(frameInterval, max(20ms, fftWindow/2))`
  (≈46 ms @44.1k). If not playing, feed silence. Then `sync` + `advance`.
- **Ballistics** per frame (dt = real elapsed, clamp ≤10·(1/60)s, default 1/60 s):
  ```
  barPos += classicPeakStep(barPos, level, dt)            # exp ease, 34 up / 10 down
  if landed && level > peakPos:                            # new launch
      peakPos = level; peakVel = min(1.7, 0.8 + 1.4*(level−peakPos)); peakHold = 0
  if peakHold > 0: peakHold −= dt; skip while >0           # apex hold 0.08 s
  else: peakPos += peakVel·dt; peakVel −= 9.5·dt           # gravity
        clamp peakPos ≤ 1.0
        if vel crossed +→− and peakPos > barPos+0.01: vel=0, peakHold=0.08   # apex
        if peakPos ≤ barPos: peakPos=barPos; vel=0; hold=0                    # landed
  ```
- **Render**: body = `fracBlock`; cap drawn only if `peak > level + max(0.01, 0.5/dotRows)` at
  `dotY = round((1−peak)·(dotRows−1))` → row `dotY/4`, glyph `⎺⎻⎼⎽[dotY%4]` (horizontal sub-row ticks).
  Bars are **right-aligned**: `rowPad = max(0, W−(2·cols−1))` spaces precede them (ClassicLED same).
- **Frame interval**: `fps = 1.7·(H·4)` clamp [24,60] ⇒ ~47.6 FPS @H=7 (~21 ms); model caps at ≤50 ms.
- **Diff**: medium-hard — the cap physics is the point; everything else is a bar render.

### Wave — `vis_wave.go`
- **In**: `waveBuf` (mono, ≤2048 samples, audible-aligned). **Spec**: {0, 2048} — **no FFT**.
  **Tick**: TickWave(16 ms) + `UsesRawSamples` ⇒ UI really ticks 16 ms.
- **Algo**: per dot column x: `s = samples[x·n/dotCols]`; `y = (1−s)·(dotRows−1)/2` clamp;
  fill vertical run between `y[x]` and `y[x−1]` (connected trace). Pack braille.
- **Color**: `specWrap` per terminal row (top red → bottom green).
- **Diff**: trivial-medium — a polyline through per-column sample decimation.

### Scatter — `vis_scatter.go`
- **In**: smoothed. **Tick**: TickFast. **State**: none.
- **Algo**: per dot (row,dr,c,dc) in band b's span: `h=scatterHash(b,dotRow,dotCol,frame)`;
  `threshold = bands[b]² · (0.5 + 0.5·dotRow/(dotRows−1))`; lit iff `h < threshold`.
  Density ∝ energy², gravity-biased toward bottom.
- **Diff**: trivial-medium — per-pixel hash gate; keep the twinkle cadence (`/3` inside hash).

### Flame — `vis_flame.go` (custom driver)
- **In**: smoothed. **Spec**: {10,2048}. **Tick**: TickFast. **State**: `heat[] float64`
  (dotRows×dotCols, y=0 = bottom source), LCG `rng`.
- **Tick**:
  ```
  source row y=0 per x:  src = sampleBandLinear(smoothed, x/(dotCols−1)·9)
                         heat[0,x] = min(1.05, 0.30 + 0.70·src + rand01·0.18)
  propagate for y = dotRows−1 down to 1:                    # top→down so row y−1 is fresh
      decayBase = 0.010 + 0.028·(y/(dotRows−1))
      heat[y,x] = max(0, heat[y−1, x+rand{−1,0,1}] − decayBase − rand01·0.018)
  ```
- **Render**: `heatY = dotRows−1−y` flip; skip `h<0.10`; `h<0.25` lit stochastically iff
  `scatterHash(0,y,x,frame) ≤ h*4` (wispy tips); `h≥0.55→tag1 (yellow core)` else `tag2 (red)`.
  Cell tag = max of its dots.
- **Diff**: hard-ish — cellular field, must keep buffer + iteration order.

### Retro — `vis_retro.go`
- **In**: smoothed + `frame`. **Tick**: TickFast. **State**: none.
- **Algo** (byte grid dotRows×dotCols, values 0/1=grid/2=wave/3=sun):
  ```
  horizon = max(dotRows*2/5, 2); floor = dotRows−horizon; cx=(dotCols−1)/2
  SUN: semicircle r=horizon·0.85 centered at horizon; stripe gaps on lower half
       (rowDist < r*0.5 → skip rows where (int(rowDist)/max(1,int(r*0.15)))%2==1)
  HORIZON line at dy=horizon
  V-LINES: 19 rays from vanishing point: for dy>horizon, t=(dy−horizon)/(floor−1),
           x = cx + (bottomX_i − cx)·t  (bottomX_i = i·(dotCols−1)/18)
  H-LINES: 10 lines, z=(i+scroll)/10 wrapped; scroll=frame·0.08 mod 1;
           dy = horizon+1 + z²·(floor−2)         # quadratic perspective
  WAVE: per dx, bandF = dx/(dotCols−1)·9 → cosine-lerp between bands; level=max(0.03,·);
        wy = horizon − level·horizon·0.85; mark column run between consecutive wy
  ```
- **Color**: per cell priority wave(2/red) > sun(1/yellow) > grid(0/green).
- **Diff**: medium — static scene + scroll; the wave is the only audio-reactive part.

### Pulse — `vis_pulse.go`
- **In**: smoothed + `frame`. **Tick**: TickFast. **State**: cached polar coords (dist,angle per dot).
- **Algo** per dot (x-aspect-corrected: `dx·=(centerY/centerX)`):
  ```
  avg = mean(bands); breath = sin(frame·0.05)·0.02
  rotAngle = angle + frame·(0.015 + avg·0.04)  mod 2π        # whole field rotates
  bandPos = rotAngle·(10/2π); energy = cosineLerp(bands, bandPos)
  punch = (0.6·energy + 0.4·avg)²
  r = maxR·(0.08 + breath + 0.92·punch)                      # maxR = centerY−1
  lit if dist ≤ r; AA edge: r < dist < r+1.5 → lit iff scatterHash < (1−(dist−r)/1.5)·0.7
  shockwave: phase=frame·0.10 mod 1; R=maxR·(0.3+0.7·phase); strength=avg²·(1−phase²);
             lit if |dist−R| < 0.6+1.5·strength and fade>0.4
  cell norm = max(dist/r) over dots (shock dots bump ≥0.65, AA ≥0.9) → specTag = radial gradient
  ```
- **Diff**: medium — polar distance field; precompute per-pixel (dist,angle) like the original.

### Matrix — `vis_matrix.go`
- **In**: smoothed. **Tick**: TickFast. **State**: none.
- **Algo** per column c in band b:
  ```
  gate: scatterHash(b,0,c,frame/20) > energy·1.5+0.1 ⇒ dark column
  seed = c·7919+104729; speed = 2+seed%3 (frames/row); trail = 3+(seed/7)%3; cycle = H+trail+4
  pos = (frame/speed + (seed/13)%cycle) % cycle; dist = pos−row
  if 0 ≤ dist ≤ trail: char = matrixChars[(seed ^ (row·31 + (frame/4)·17)) % 41]
     tag: dist==0→2, ≤2→1, else 0
  ```
- `matrixChars` = 31 half-width katakana + '0'..'9' (41 total).
- **Diff**: medium — same skeleton as Rain, plus glyph table.

### Binary — `vis_binary.go`
- **In**: smoothed. **Tick**: TickFast. **State**: none.
- **Algo** per column c in band b: `speed = max(1, 4−int(energy·3))` (louder → faster scroll);
  `bit = scatterHash(b, row + frame/speed, c, 0) < energy·0.6+0.15 → '1' else '0'`.
- **Color**: `'1' && energy>0.4`→2; `'1' || energy>0.3`→1; else 0.
- **Diff**: trivial-medium.

### Sakura — `vis_sakura.go`
- **In**: smoothed. **Tick**: TickFast. **State**: none.
- **Algo**: `numPetals = 12 + int(avgEnergy·16)`; petal p: `seed=p·104729+7919`;
  `shape = sakuraShapes[seed·4391 % 9]` (3 large 6-dot, 3 medium 4-dot, 3 small 2–3-dot silhouettes);
  `fallSpeed = shapeIdx≥6 ? 2 : 1`; `x = seed%dotCols + sin(frame·0.015 + (seed%1000)/1000·2π)·3`;
  `y = ((seed·3037)%(dotRows+10) + frame·fallSpeed/8) % (dotRows+10) − 5`; stamp shape dots.
- **Color**: row gradient (top red → bottom green).
- **Diff**: medium — sprite stamps on dot grid; needs the 9 shape bitmaps.

### Firework — `vis_firework.go`
- **In**: smoothed. **Tick**: TickFast. **State**: none.
- **Algo**: `bursts = 5 + int(avg·9)`; per burst i: `cycle=(frame+i·7)/48`; `seed=cycle·104729+i·7919`;
  `offset = i·48/bursts + (seed/3)%5`; `local = (frame+offset)%48`; launch = 10 frames.
  ```
  local<10: trail — 4 dots at x=cx rising from bottom to cy: ty = dotRows−1 − (dotRows−1−cy)·local/10 +dy
  else:     burstT=(local−10)/38; R=(3+energy·8)·min(burstT·3,1); grav=burstT²·5; fade=1−1.3·burstT
            particles = 18+int(energy·18); pSeed=seed+p·2909; speed=0.6+(pSeed%400)/1000
            px=cx+cos(θ)·R·speed; py=cy+sin(θ)·R·speed+grav; draw iff scatterHash(…,frame) ≤ fade
  cx=(seed·6271)%dotCols; cy=(seed·4391)%(dotRows/2)+dotRows/8; bandIdx=seed%10; energy=bands[bandIdx]
  ```
- **Diff**: medium — deterministic per-cycle bursts; the rising trail is easy to forget.

### Bubbles — `vis_bubbles.go`
- **In**: smoothed. **Tick**: TickFast. **State**: none.
- **Algo**: 18 fixed bubbles. Per bubble i: `seed=i·104729+7919`; `radius=1.5+(seed%100)/100·2.5`;
  `speedDiv=3+int(radius)` (bigger rises slower); `wrapH=dotRows+2r+8`;
  `y=wrapH−1−((seed·3037%wrapH + frame/speedDiv)%wrapH)−r−2`; `x=seed%dotCols + sin(frame·0.03+phase)·(1.5+avg·2.5)`.
  Ring: dots where `r−0.9 ≤ dist ≤ r`. Pop zone `y < r+3`: `popFade=y/popZone`, cull dots by stable
  `scatterHash(i,dy,dx,0)>popFade`. Specular: if r≥2 && popFade>0.5, 3-dot cluster at (−0.45r,−0.45r).
- **Diff**: medium — hollow rings + highlight; all deterministic.

### Logo — `vis_logo.go`
- **In**: smoothed. **Tick**: TickFast. **State**: none.
- **Algo**: "CLIAMP" as 6× 5×7 bitmaps (`logoGlyphs`), gap 2 px → 40×7 glyph field.
  `scaleX=max(1,dotCols/40)`, `scaleY=max(1,(dotRows·3/4)/7)`; centered.
  Letter→band map `{0,2,4,5,7,9}`. Per letter: `wave=sin(frame·0.06+li·0.9)·1.5`;
  `bounce=int(energy·baseOffsetY·0.3 + wave)`; `letterY = baseOffsetY − bounce` (rises with energy).
  Per glyph pixel → scaleX×scaleY dot block; dot drawn iff `scatterHash(li, py·scaleY+sy, px·scaleX+sx, frame) ≤ fill`,
  `fill = energy²·0.75 + 0.15` (silence dissolves text to sparse pixels).
- **Diff**: medium — needs glyph bitmaps (5×7 each, included in source).

### Terrain — `vis_terrain.go` (custom driver)
- **In**: smoothed. **Tick**: TickFast. **State**: `buf[] float64` len dotCols (persists).
- **Tick**: scroll `buf` left by 2 dots; new right columns =
  `min(1, mean(smoothed) + scatterHash(0,0,{0,1},frame)·0.12)`.
- **Render**: per dot column fill from `topDot = dotRows−1−int(h·(dotRows−1))` down.
- **Color**: row gradient (peaks red → valleys green).
- **Diff**: medium — ring-buffer heightfield, trivially portable.

### Scope — `vis_scope.go`
- **In**: `waveBuf` + `frame`. **Spec**: {0,2048}, no FFT. **Tick**: TickWave (real 16 ms).
- **Algo**: `delay = clamp(n/4 + sin(frame·0.02)·(n/8), 1, n−1)`;
  `step = max(1, (n−delay)/512)` (integer div ⇒ ≥512 XY points, every sample when n−delay≤512);
  `x=samples[i]→dot`, `y=samples[i+delay]→dot`; linear-interpolate between consecutive dots
  (only if `max(|dx|,|dy|) < 30`).
- **Diff**: medium — XY scatter + DDA line fill. Mono ⇒ delayed self as Y (pseudo-Lissajous).

### Heartbeat — `vis_heartbeat.go`
- **In**: `waveBuf`. **Spec**: {0,2048}, no FFT. **Tick**: TickWave. **State**: none.
- **Algo**: per dot column x: `s=samples[x·n/dotCols]`; `shaped = s·|s|` (sign-preserving square —
  sharpens peaks into QRS-like spikes); `y = center − shaped·(dotRows·0.45)`; connected vertical runs.
  Baseline: dashed center line (6 on / 4 off) where trace absent.
- **Color**: trace cells → tag 2 (red); baseline → tag 0 (green). Cell w/ any non-baseline dot = red.
- **Diff**: trivial-medium — same polyline as Wave + sign² shaping + dashed baseline.

### Butterfly — `vis_butterfly.go`
- **In**: smoothed + `frame`. **Tick**: TickFast. **State**: none.
- **Algo** per dot row dy: `energy = lerp(bands, dy/(dotRows−1)·9)` — **band 0 (bass) at TOP,
  treble at bottom**; `wobble = sin(frame·0.08 + dy·0.3)·0.15`; `wingWidth = centerX·(energy+wobble)·0.9`;
  per dx < wingWidth: `norm=dx/wingWidth`; `threshold=(1−norm²)·energy`, and at `norm>0.6` ×=
  `(0.5+0.5·sin(frame·0.1+dy·0.5+dx·0.3))`; lit iff `scatterHash(bi,dy,dx,frame/3) < threshold` →
  draw at `centerX+dx` **and** `centerX−1−dx` (mirror). Spine: 2 center dots when `energy>0.05`.
- **Color**: `specWrap(row/(H−1))` — **inverted vs other modes: top=green, bottom=red**.
- **Diff**: medium — mirrored stochastic field.

### Ascii — `vis_ascii.go`
- **In**: smoothed. **Tick**: TickAnim. **State**: none.
- **Algo**: `cols=(W+1)/2`; `levels=resampleBandsLinear(bands,cols)`; per col `shadeBlock(level,…)`
  + 1-space gap. (The website-style dense bars.)
- **Diff**: trivial.

### Firefly — `vis_firefly.go`
- **In**: smoothed. **Tick**: TickFast. **State**: none.
- **Algo**: `bass=bandAvg(0..n/3)`, `high=bandAvg(2n/3..n)`; `wind=bass·1.5`.
  Grass silhouette: per x, `h = 1 + int(2.5 + 1.5·sin(x·0.41) + 1.0·sin(x·0.17+2.3))` dots up from bottom.
  26 fireflies: `fx=0.012+(seed%17)/3500`, `fy=0.018+((seed>>4)%19)/2900`, phases from seed;
  `x = dotCols/2 + cos(t·fx+phx)·(dotCols−6)·0.45 + wind·sin(t·0.02+phx)`;
  `y = (dotRows−4)·0.5 + sin(t·fy+phy)·(dotRows−6)·0.4`; skip if OOB or inside grass.
  `on = sin(t·0.18 + i·1.31)·0.5 + 0.5 + high·0.4 > 0.55`; lit → bright dot + 4-neighbor dim halo;
  unlit → dim dot only.
- **Color**: bright=2, dim halo=1, grass=0 (cell = max tier present).
- **Diff**: medium — Lissajous drift + blink gate; pure function of frame+bands.

### Mosaic — `vis_mosaic.go` (custom driver)
- **In**: smoothed. **Tick**: TickFast. **State**: `cells[] {bandIdx, threshold, value}`,
  size `H × tiles`, `tiles=(W+1)/3` (2-wide tiles + 1 gap). Regenerated on every OnEnter (reshuffle).
- **Init** (LCG `0xC1AB1A1015D5`): per cell `band = clamp(baseBand + jitter(−2..2))`,
  `baseBand = (H−1−r)·9/(H−1)` — **top row listens to treble, bottom to bass**;
  `threshold = 0.04 + rand·0.74`.
- **Tick** per cell: `lvl=bands[bandIdx]`; if `lvl > threshold` → `value = max(value, min(lvl,1.05))`;
  then `value *= 0.88`, `<0.001 → 0`.
- **Render**: `value<0.05→' '`; `≥0.05→'░'`; `≥0.15→'▒'`; `≥0.28→'▓'`; `≥0.45→'█'`(tag0);
  `≥0.65→'█'`(tag1); `≥0.85→'█'`(tag2). Tile = 2 glyph cells + 1 space.
- **Diff**: medium — stateful per-cell decay; trivially native.

### Sand — `vis_sand.go` (custom driver)
- **In**: smoothed. **Tick**: TickFast. **State**: `grid int8[dotRows×dotCols]` (0 empty, 1/2/3 tier),
  `prevBass`, LCG rng, `particles[]`, `explosionTTL`. `pauseSettled` = no explosion/particles.
- **Tick** (in order):
  ```
  bass = bandAvg(0..n/3); delta = bass − prevBass
  EXPLOSION MODE (TTL>0 or particles): vy+=0.5; vx*=0.985; x+=vx; y+=vy; offscreen→die;
      grid rebuilt from particles each tick; TTL 80 cap; then return.
  SPAWN: per band b with level≥0.10 and rand01 ≤ level·0.85:
      centre=(2b+1)·dotCols/(2·bandCount), spread=dotCols/(2·bandCount); x = centre±spread;
      tier = b<n/3→3(red,bass) / b<2n/3→2(yellow) / else 1(green); place at TOP row if empty.
  TRIGGER: if delta>0.06 && bass>0.15 && fillRatio>0.30 → every grain → ballistic particle:
      vy=−(2+rand·5+depthFrac·2), vx=(rand−0.5)·8 → enter explosion mode, return.
  TRANSIENT BUMP (same trigger, fill≤30%): strength=min(1.4, delta·3.5+bass·0.8);
      per grain top→down: p= strength·(0.30+0.70·depthFrac) ≤0.95 → lift 1..(2+strength·7·(0.4+0.6·depth)),
      x-jitter ±(1+strength·5); move if target empty.
  RUMBLE (bass>0.30): rumble=min(0.6,(bass−0.3)·1.8); bottom half only;
      p=rumble·(0.15+0.55·depthFrac) → lift 1..2, jitter ±2.
  FALL: y=dotRows−2→0, scan dir alternates by frame parity; down if empty else diagonal
      (rand picks side order); bottom row evaporates p=0.04.
  ```
- **Render**: brailleGrid-style, cell = max tier.
- **Diff**: hard — falling-sand CA + ballistic sub-phase + bass-transient detection.

### Geyser — `vis_geyser.go` (custom driver)
- **In**: smoothed. **Tick**: TickFast. **State**: `particles[] {x,y,vx,vy,tier,life}`, brailleGrid,
  `prevBass`, LCG rng `0xFEED5EED`. `pauseSettled` = no particles.
- **Tick**:
  ```
  bass/mid/high = bandAvg thirds; delta = bass − prevBass
  steady = bass·0.85 + mid·0.25 + high·0.08
  spawn int(steady·6) drops at (dotCols/2, bottom), spread=max(2,dotCols/16), vy=−(1.5+steady·4.5)·(0.6..1.1)
  kick (delta>0.06 && bass>0.15): burst = 40+int(delta·180), spread×2, vy=−(4.5+delta·10+bass·4)·(0.6..1.1)
  spawn lateral: vx = (rand01−0.5)·(1 + |vy|·0.4)
  per particle: vy+=0.30; vx*=0.992; x+=vx; y+=vy; life++
      die if iy≥dotRows or ix OOB or life>200; **iy<0 clamps to row 0** (jets smear the top edge)
  tier at spawn: r<bass→3, r<bass+mid→2, else 1
  ```
- **Diff**: hard-ish — real particle system (unbounded pool, gravity, drag).

### ClassicLED — `vis_classic_led.go` (custom driver)
- **In**: `v.bands` raw (own ballistics). **Spec**: **{(W+1)/3, 2048}** — band count is a function of
  panel width (≈25 @ W=74); FFT 2048.
- **State** per bar (2-cell wide + 1 gap): `body`, `peak`, `hold`; `bandsAt`, `lastTick`.
- **Tick**: analyze at `TickAnalyze` (33 ms) while playing; silence otherwise. `advance` with dt
  (real elapsed, clamp ≤10·frame, frame=1/30 s ⇒ **30 FPS** requested cadence):
  ```
  body += (target−body)·(1−e^{−rate·dt}),  rate = 60 up / 16 down
  peak:  body≥peak → peak=body, hold=0.45s
         hold>0 → hold−=dt
         else  peak=max(body, peak − 0.55·dt)          # linear fall, not ballistic
  ```
- **Render**: per row rfb (0=bottom): `lit=floor(body·H)`; `peakSeg=floor(peak·H)` clamp H−1;
  `showPeak = peak>body+0.5/H && peakSeg≥lit`; cell `rfb<lit→'▄'`, `rfb==peakSeg&&showPeak→'▀'`, else ' '.
- **Diff**: medium — quantized meters + simple peak ballistics; `▄/▀` = per-row LED look.

### Stereo — `vis_stereo.go` (custom driver)
- **In**: **stereo frames via `StereoSamplesInto`** — 2048 `[2]float64` latest-written samples,
  sampled ≤ every 33 ms (`TickAnalyze`). **Spec**: {0, 2048} — no FFT, no waveBuf.
  **Tick**: TickAnim while playing/animating.
- **Metrics** per channel: `level = dB(RMS)`, `peak = dB(max|s|)`, `dB(x)=clamp((20·log10 x −(−48))/48, 0,1)`
  (floor −48 dB).
- **Ballistics** (dt real, clamp ≤160 ms, default 16 ms): `level` exp-ease rate 36 up / 10 down;
  `peak`: new max snaps + hold 450 ms, then linear fall 0.65/s, never below level.
- **Render**: L meter on top `H/2` rows, R on bottom (odd H → 1 blank row between); label `L `/`R `
  centered. Per meter: `cells=W−2`; `lit=round(level·cells)` → `'▮'`; unlit `'·'` (unstyled);
  `peakCell=round(peak·cells)−1` → `'■'`; `specTag(cell/(cells−1))` position gradient green→red.
- **Diff**: trivial-medium — needs true stereo tap (only mode using it).

### Mirror — `vis_mirror.go`
- **In**: smoothed — **but only the mean**: `env = mean(clamp01(bands))`. + `frame`.
  **Tick**: TickAnim. **State**: reused brailleGrid (cleared per render).
- **Algo**: `span = 84%·dotCols` even; `barCount=span/2`; axis at `axisY=dotRows/2`. Axis line drawn
  (tier 1). Per bar i at `x = x0+i·2+1` (every other dot column):
  ```
  dist  = |i − (barCount−1)/2| / ((barCount−1)/2)          # 0 center → 1 edges
  wobble= 0.4 + 0.6·|sin(t·4.6+i·0.42)·sin(t·1.9−i·0.13)|   t = frame·0.016
  amp   = dotRows·0.80·(1−dist·0.55)·(0.3+0.7·env)·(0.35+0.65·wobble)
  radius= clamp(round(amp), 1, maxRadius); fill column axisY±radius
      tier 3 (red) where |y−axisY|/radius ≥ 0.75, else 2 (yellow)
  ```
- **NOTE**: per-bar heights are NOT band values — it's an envelope-modulated sine-interference pattern
  that only *looks* like a spectrum.
- **Diff**: medium — trivially native (vertical symmetric bars); keep the dual-sine wobble.

### Omarchy — `vis_omarchy.go`
- **In**: smoothed + `frame`. **Tick**: TickAnim. **State**: none (noise field is a global const).
- **Geometry**: **half-block pixels, not braille** — `pxRows=H·2`, `pxCols=W`; cell = `▀/▄/█` of 2 stacked px.
- **Mark**: wordmark art (81×20 px, decoded from half-block art) preferred, else 15×15 square spiral;
  integer scale 1–3, 2-px margin; neither fits → no mark.
- **Algo** per pixel (pr,pc); `t=frame·0.03`; `amp=mean(bands)`:
  ```
  levelAt(pc): side = min(1, |pc+0.5−half|/half)              # 0 center → 1 edges
               pos = (1−side)·n − 0.5; lerp bands             # MIRRORED: bass at outer edges
               level = max(0, (raw−0.06)/0.94)
  mark pixel → lit, tier = specTag(0.34 + levelAt·0.35 + amp·0.45)
  shade = (min(1, distToMark/6))³                              # clear zone around mark
  spec: if up=pxRows−1−pr < tall=level·pxRows·0.95 → level·(1−up/tall)^0.85   # column fill from bottom
  base = 0.6·noise(u+t·0.14, w−t·0.055) + 0.4·noise(u·0.55−t·0.08, w·0.55+t·0.06)   # drifting value noise, u=pc/6, w=pr/6
  tw   = 0.5+0.5·sin(t·1.1 + jitter(pr,pc)·2π)                 # per-pixel blink
  lum  = shade·(0.30 + 0.52·base² + 0.18·tw + amp·0.22)·0.34 + spec·0.72·min(1,shade·3)
  threshold = 0.55·(bayer8x8[pr&7][pc&7]+0.5)/64 + 0.45·jitter(pr+7,pc+13)
  lit iff lum > threshold; tier = specTag(spec·0.55 + amp·0.12)
  ```
- `noise`: 64×64 LCG-seeded value noise, bilinear + smoothstep, wrap edges. `jitter`: hash like scatterHash.
- **Diff**: hard — Bayer dither + value noise + mirrored spectrum + embedded wordmark; but it's a
  pure per-pixel function → straightforward Canvas shader-style port once constants are copied.

### RedSector — `vis_red_sector.go` (custom driver)
- **In**: smoothed (10 → 5 bars via band pairs). **Tick**: TickFast. **State**: `ceiling[5]`,
  `floor[5]`, `heights[5]` (adaptive envelope), `redSectorGrid` (7-tag dot raster).
- **Envelope** per bar i (per tick): `level = max(bands[2i], bands[2i+1])`;
  `ceiling = max(level, ceiling−0.001)`; `floor = min(level, floor+0.001)`; keep `ceiling ≥ floor+0.06`;
  `norm=(level−floor)/(ceiling−floor)`; `target = 0.40 + norm·1.90` (world units);
  `heights += (target−heights)·rate`, `rate = 0.75 rising / 0.28 falling` — **per-tick lerp, not dt-scaled**.
  ⇒ Bars show *relative* position within each band's recent range, not absolute loudness.
- **Render** (per frame): starfield then 5 tumbling wireframe boxes:
  ```
  stars: count = clamp(dotRows·dotCols/130, 6, 90); star i: splitmix64-hash positions;
         x = (hash0·dotCols − frame·speed_i) mod dotCols (3 speed lanes 0.10/0.19/0.28);
         y = hash1·dotRows; tag = 1 + hash2·4   (tags 1..4 = dim colors)
  bars:  spinY=frame·0.105, spinX=frame·0.073 rad; camZ = 6.0 + sin(frame·0.011)·1.5
         8 corners: x=baseX±0.26, z=±0.26, y=groundY(−1.05) or +height
         rotate Y then X; perspective f = 3.2/max(0.35, z+camZ); px=x·f, py=y·f
         backface cull: face normal·viewDir > 0 (faces wound so cross points inward)
         draw 4 edges per visible face (DDA, ≤512 steps); tag by height: shown≥0.6→7, ≥0.3→6, else 5
  fit:   project never-exceeded hull (±hullX, ground..maxH, ±depth); zoom=0.55+0.30·(0.5+0.5·sin(frame·0.011));
         fitY=(dotRows−1)/spanY·zoom; stretch=min(2, 40/dotRows); fitX=min(fitY·stretch, (dotCols−1)/spanX)
  ```
- **Tags/colors**: 1=ColorDim, 2..4=dim(SpecLow/Mid/High) — ANSI bright (8–15)→−8, hex→RGB·0.6;
  5..7=SpecLow/Mid/High. Cell wears max tag ⇒ bars always occlude stars.
- **Diff**: hardest — 3D projection, backface culling, adaptive envelope, custom 7-tier rasterizer.

---

## 3. PORT DIFFICULTY SUMMARY

| Tier | Modes | Notes |
|---|---|---|
| **Trivial** (bands→rects/glyphs) | Bars, BarsDot, BarsOutline, Bricks, Columns, Ascii | Direct level→height mapping. `fracBlock`/`shadeBlock` quantization for fidelity; or draw real rects and skip the ⅛-sliver quirk. |
| **Trivial-medium** | Wave, Heartbeat | Sample decimation + polyline; Heartbeat adds `s·|s|` shaping + dashed baseline. Need the **audible-aligned** waveform tap. |
| **Medium** (deterministic field/sprite, pure f(frame,bands)) | Rain, Scatter, Matrix, Binary, Sakura, Firework, Bubbles, Logo, Butterfly, Firefly, Pulse, Retro, Mirror, Scope | All stateless — port scatterHash + frame clock verbatim and they pixel-match. Retro/Pulse are static scenes w/ distance fields. Mirror is envelope-only (not per-band!). Scope needs waveBuf. |
| **Medium** (light state) | Terrain (heightfield ring), Mosaic (cell decay), ClassicLED (body/peak ballistics @30 FPS), Stereo (RMS/peak dB meters, stereo tap) | Stateful but simple update rules. |
| **Medium-hard** | ClassicPeak | Own spec {64 bands, FFT 4096, audible-aligned} + ballistic peak caps (launch/hold/gravity). |
| **Hard** | Flame (heat-field CA), Sand (falling-sand CA + explosion particles + bass transients), Geyser (particle system), Omarchy (value-noise + Bayer dither + wordmark), RedSector (3D wireframe + adaptive envelope + 7-tag rasterizer) | Keep buffers, iteration order, and LCGs for parity. |

### Terminal→native reinterpretation notes
- **Braille (2×4 dots/cell)**: Wave, Scatter, Flame, Retro, Pulse, Sakura, Firework, Bubbles, Logo,
  Terrain, Scope, Heartbeat, Butterfly, Firefly, Sand, Geyser, Mirror, RedSector, BarsDot.
  In SwiftUI, treat as a pixel canvas `W*2 × H*4` — or render at true pixel resolution and drop the
  packing entirely.
- **Half-block (2 px/cell)**: Omarchy (`▀▄█`); ClassicLED uses `▄`/`▀` for LED/peak glyphs.
- **8-step block ramp** ` ▁▂▃▄▅▆▇█`: Bars, Columns, ClassicPeak body (+ `⎺⎻⎼⎽` sub-row peak ticks).
- **Shade ramp** ` ░▒▓█`: Ascii, Mosaic.
- **Glyph tables**: Matrix's 41 katakana/digits; Logo's 6× (5×7) bitmaps; Omarchy wordmark (81×20) +
  square (15×15); Sakura's 9 petal shapes; Rain `┃│:`; Binary `01`.
- **Color tiers** are positional/semantic, not per-value: bars/terrain/scope color by *row*
  (top red); Sand/Geyser color by *band tier* (bass red → treble green — inverted vs the row gradient);
  RedSector has 7 tags; Mosaic intensity-tiered; Stereo position-gradient. Tier `-1` = terminal default.
- **`v.frame` is a tick counter, not time** — reproduce via a monotonically increasing frame int
  stepped at the cadences above (16 ms clock modes advance ~3 frames per 50 ms UI tick, cap 4/tick).
- Every hash/LCG is reproducible: `scatterHash`, `rng64` (NR LCG), `redSectorStarHash` (splitmix64
  finalizer), Omarchy's fixed 64×64 noise field — copy constants for pixel-identical output.
- Two taps exist: **latest-written** (`SamplesInto`, all band modes — runs ~speaker-buffer ahead of
  audible) vs **audible-aligned** (`WaveformSamplesInto`: Wave/Scope/Heartbeat/ClassicPeak).
  For a native port just use the freshest samples unless you also buffer output audio.
