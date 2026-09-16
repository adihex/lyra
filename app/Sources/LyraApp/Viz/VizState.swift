import CoreGraphics
import Foundation

/// One pooled particle — value type, preallocated, never grown past cap.
struct VizParticle {
    var x, y, vx, vy, life, maxLife, size: Float
    var kind: UInt8 // 0 spark · 1 flame · 2 petal · 3 burst · 4 bubble · 5 fly · 6 drop
    var seed: Float
    var alive: Bool = false
}

/// Fixed-capacity pool. Spawn reuses dead slots; update is a bounded scan.
struct VizPool {
    var parts: [VizParticle]
    var cursor = 0

    init(cap: Int) {
        parts = [VizParticle](repeating: VizParticle(
            x: 0, y: 0, vx: 0, vy: 0, life: 0, maxLife: 1, size: 1, kind: 0,
            seed: 0, alive: false), count: cap)
    }

    mutating func spawn(_ p: VizParticle) {
        // linear probe from cursor for a dead slot — bounded by capacity
        for _ in 0..<parts.count {
            cursor = (cursor + 1) % parts.count
            if !parts[cursor].alive {
                parts[cursor] = p
                parts[cursor].alive = true
                return
            }
        }
    }
}

/// All Swift-side viz state (contract: particles, CA grids, scrollback
/// rings, LED decay, peak-cap gravity live here, fed by VizFrame).
/// Plain class, main-thread only, mutated from inside Canvas draws which
/// run on the main thread via TimelineView.
final class VizState {
    // peak caps: value + velocity for gravity fall (ClassicPeak, ClassicLED)
    var caps = [Float](repeating: 0, count: 64)
    var capVel = [Float](repeating: 0, count: 64)
    // LED phosphor levels (ClassicLED, Stereo)
    var leds = [Float](repeating: 0, count: 64)
    var stereoL: Float = 0
    var stereoR: Float = 0
    // scrollback rings: 240 cols × 32 rows, flat, head pointer (no alloc)
    static let ringCols = 240
    static let ringRows = 32
    var ring = [Float](repeating: 0, count: ringCols * ringRows)
    var ringHead = 0
    var ringTick: UInt64 = 0
    // heartbeat trace ring (240 samples)
    var ecg = [Float](repeating: 0, count: 240)
    var ecgHead = 0
    // mosaic heat 20×12
    var mosaic = [Float](repeating: 0, count: 20 * 12)
    // sand automaton grid 120×64
    static let sandW = 120
    static let sandH = 64
    var sand = [UInt8](repeating: 0, count: sandW * sandH)
    // particle pools (each ≤512 per contract)
    var spark = VizPool(cap: 256)   // Scatter
    var flame = VizPool(cap: 160)   // Flame
    var petal = VizPool(cap: 128)   // Sakura
    var burst = VizPool(cap: 256)   // Firework
    var bubble = VizPool(cap: 96)   // Bubbles
    var fly = VizPool(cap: 80)      // Firefly
    var geyser = VizPool(cap: 256)  // Geyser
    var drop = VizPool(cap: 256)    // Rain
    // matrix/binary column heads
    var colY = [Float](repeating: 0, count: 48)
    var colSpd = [Float](repeating: 0, count: 48)
    // redsector: starfield (fixed) + tumble angle + per-bar envelope
    var stars: [(x: Float, y: Float, z: Float)] = []
    var tumble: Float = 0
    var rsCeil = [Float](repeating: 0, count: 5)
    var rsFloor = [Float](repeating: 1, count: 5)
    var rsHeight = [Float](repeating: 0.4, count: 5)
    // doom-fire heat field (Flame) — fixed 96×48, rendered scaled
    static let flameW = 96
    static let flameH = 48
    var heat = [Float](repeating: 0, count: flameW * flameH)
    // mosaic cell wiring: which band + ignition threshold per tile
    static let mosW = 20
    static let mosH = 12
    var mosaicBand = [UInt8](repeating: 0, count: mosW * mosH)
    var mosaicThresh = [Float](repeating: 0, count: mosW * mosH)
    // ClassicLED peak caps with hold-then-fall (Winamp PPM feel)
    var ledPeak = [Float](repeating: 0, count: 64)
    var ledHold = [Float](repeating: 0, count: 64)
    // sand explosion phase: grains in ballistic flight (grid is cleared)
    var sandBoom = VizPool(cap: 400)
    var sandBoomTTL = 0
    // ordered-dither value-noise field (Dither) — built once, wraps at edges
    static let noiseSize = 64
    var noise = [Float](repeating: 0, count: noiseSize * noiseSize)
    // shared deterministic LCG for CA/particle spawns (no RNG alloc)
    var rng: UInt64 = 0xF1A3C0DE0BADCAFE
    // edge detection for beat/bass transients
    var prevBass: Float = 0
    var prevBeat: Float = 0
    // cosmos mode + dock tile share this motion state (CosmosScene.swift)
    var cosmos = CosmosSceneState()
    // UI ticks regardless of seq — drives settle/freeze-out animations
    var uiTick: UInt64 = 0
    // WaveSeek peak cache — re-seeded when the track changes; mock peaks
    // until viz-core exports the decoded WaveformPeaks store (see renderer).
    var seekSeed: UInt64 = 0
    var seekPeaks: (min: [Float], max: [Float]) = ([], [])
    var seekSize: CGSize = .zero
    var lastSeq: UInt64 = 0

    init() {
        var s: UInt64 = 0x12345678
        func rnd() -> Float {
            s ^= s << 13; s ^= s >> 7; s ^= s << 17
            return Float((s >> 11) & 0xFFFF) / 65535
        }
        stars.reserveCapacity(120)
        for _ in 0..<120 { stars.append((rnd(), rnd(), 0.2 + 0.8 * rnd())) }
        for i in 0..<48 { colSpd[i] = 0.5 + rnd(); colY[i] = rnd() }
        for i in 0..<80 { // fireflies start distributed
            fly.parts[i] = VizParticle(x: rnd(), y: rnd(), vx: 0, vy: 0,
                                       life: 1, maxLife: 1, size: 2 + rnd() * 2,
                                       kind: 5, seed: rnd() * 10, alive: true)
        }
        // mosaic wiring: band biased by row (top → treble, bottom → bass)
        // + per-cell threshold in [0.04, 0.78] — port of cliamp mosaicDriver.
        for r in 0..<VizState.mosH {
            let base = (VizState.mosH - 1 - r) * 63 / max(VizState.mosH - 1, 1)
            for c in 0..<VizState.mosW {
                let jit = Int(rnd() * 5) - 2
                mosaicBand[r * VizState.mosW + c] =
                    UInt8(min(max(base + jit, 0), 63))
                mosaicThresh[r * VizState.mosW + c] = 0.04 + rnd() * 0.74
            }
        }
        // dither noise field — fixed sequence, same every run (cliamp omarchy)
        var ns: UInt64 = 0x9E3779B97F4A7C15
        for i in 0..<noise.count {
            ns = ns &* 6364136223846793005 &+ 1442695040888963407
            noise[i] = Float((ns >> 33) % 100000) / 100000
        }
    }

    /// LCG draw — deterministic, matches cliamp's rand01 cadence.
    func rand01() -> Float {
        rng = rng &* 6364136223846793005 &+ 1442695040888963407
        return Float((rng >> 33) % 1000) / 1000
    }

    /// Per-tick housekeeping: rings/envelopes push on new seq; pools and
    /// automata advance every UI tick so in-flight particles settle to rest
    /// instead of freezing mid-air (cliamp pauseSettled semantics).
    func tick(frame: VizFrame, mode: VizMode) {
        uiTick &+= 1
        let advanced = frame.seq != lastSeq
        lastSeq = frame.seq
        if advanced {
            ringTick &+= 1
        // downsample 64 bands → 32 rows into the ring head column
        for r in 0..<VizState.ringRows {
            let b = frame.bands[min(r * 2 + 1, 63)] * 0.5
                + frame.bands[min(r * 2, 63)] * 0.5
            ring[ringHead * VizState.ringRows + r] = b
        }
            ringHead = (ringHead + 1) % VizState.ringCols
            // ECG sample: wave envelope + beat spike
            let w = frame.waveL[128]
            ecg[ecgHead] = w * 0.4 + frame.beat * 0.6
            ecgHead = (ecgHead + 1) % ecg.count
        }
        // peak-cap gravity + LED phosphor decay run every tick so caps keep
        // falling to rest while seq is stalled.
        for i in 0..<64 {
            let target = frame.bands[i]
            if caps[i] < target { caps[i] = target; capVel[i] = 0 }
            else {
                capVel[i] += 0.0035 // gravity
                caps[i] = max(target, caps[i] - capVel[i])
            }
            leds[i] = max(target, leds[i] * 0.90)
            // Winamp-style cap: hold 0.45s at apex, then fall 0.55/s
            if target >= ledPeak[i] {
                ledPeak[i] = target; ledHold[i] = 0.45
            } else if ledHold[i] > 0 {
                ledHold[i] = max(0, ledHold[i] - 1.0 / 60)
            } else {
                ledPeak[i] = max(target, ledPeak[i] - 0.55 / 60)
            }
        }
        stereoL = max(frame.peakL, stereoL * 0.92)
        stereoR = max(frame.peakR, stereoR * 0.92)
        tumble += 0.008 + frame.bass * 0.02

        // mode-specific state machines — only the active mode's sim runs
        switch mode {
        case .flame: tickFlame(frame)
        case .mosaic: tickMosaic(frame)
        case .sand: tickSand(frame)
        case .sakura: tickPetals(frame)
        case .firework: tickFireworks(frame)
        case .bubbles: tickBubbles(frame)
        case .geyser: tickGeyser(frame)
        case .redSector: tickRedSector(frame)
        default: break
        }
        prevBass = frame.bass
        prevBeat = frame.beat
    }

    // ── Flame: doom-fire propagation ─────────────────────────────────
    // Bottom row is seeded from the spectrum; heat climbs with lateral
    // wind jitter + random decay — a continuous lapping flame.
    private func tickFlame(_ f: VizFrame) {
        let W = VizState.flameW, H = VizState.flameH
        for x in 0..<W {
            let src = sampleBand(f.bands, pos: Float(x) / Float(W - 1) * 63)
            let sparkle = rand01() * 0.18
            heat[x] = min(1.05, 0.30 + 0.70 * src + sparkle + f.bass * 0.15)
        }
        for y in 1..<H {
            let heightFrac = Float(y) / Float(H - 1)
            let decayBase: Float = 0.010 + 0.028 * heightFrac
            for x in 0..<W {
                let off = Int(rand01() * 3) - 1
                let jitter = rand01() * 0.018
                let sx = min(max(x + off, 0), W - 1)
                heat[y * W + x] = max(0, heat[(y - 1) * W + sx] - decayBase - jitter)
            }
        }
    }

    // ── Mosaic: ignite above threshold, decay in place ───────────────
    private func tickMosaic(_ f: VizFrame) {
        let n = VizState.mosW * VizState.mosH
        for i in 0..<n {
            let level = f.bands[Int(mosaicBand[i])]
            if level > mosaicThresh[i], level > mosaic[i] {
                mosaic[i] = min(level, 1.05)
            }
            mosaic[i] *= 0.88
            if mosaic[i] < 0.001 { mosaic[i] = 0 }
        }
    }

    // ── Sakura: fixed petals pool, spawn target scales with energy ───
    private func tickPetals(_ f: VizFrame) {
        var avg: Float = 0
        for b in f.bands { avg += b }
        avg /= 64
        let target = min(128, 12 + Int(avg * 90))
        var alive = 0
        for i in 0..<petal.parts.count {
            if petal.parts[i].alive {
                alive += 1
                var p = petal.parts[i]
                p.y += p.vy; p.x += p.vx
                p.vx = sin(Float(uiTick) * 0.015 + p.seed * 6.28) * 0.0012
                p.life += 1
                if p.y > 1.05 { p.alive = false }
                petal.parts[i] = p
            }
        }
        var deficit = target - alive
        while deficit > 0 {
            let p = VizParticle(x: rand01(), y: -0.05 - rand01() * 0.2,
                                vx: 0, vy: 0.0008 + rand01() * 0.0018,
                                life: 0, maxLife: 1e9, size: 1 + rand01() * 2,
                                kind: 2, seed: rand01(), alive: true)
            petal.spawn(p)
            deficit -= 1
        }
    }

    // ── Firework: beat-edge launches + ambient cycling ───────────────
    private func tickFireworks(_ f: VizFrame) {
        var avg: Float = 0
        for b in f.bands { avg += b }
        avg /= 64
        // beat rising edge → one burst; loud passages add ambient launches
        let edge = f.beat > 0.55 && prevBeat <= 0.55
        if edge || (avg > 0.25 && hashNoise(uiTick / 30, 7) < avg * 0.35) {
            let cx = 0.15 + rand01() * 0.7, cy = 0.10 + rand01() * 0.30
            let n = 18 + Int(avg * 30)
            for i in 0..<n {
                let a = Float(i) / Float(n) * .pi * 2
                let spd = 0.0018 + rand01() * 0.0032 + avg * 0.002
                burst.spawn(VizParticle(x: cx, y: cy,
                                        vx: cos(a) * spd, vy: sin(a) * spd,
                                        life: 0, maxLife: 55 + rand01() * 40,
                                        size: 1.4 + rand01(),
                                        kind: 3, seed: rand01(), alive: true))
            }
        }
        for i in 0..<burst.parts.count where burst.parts[i].alive {
            var p = burst.parts[i]
            p.x += p.vx; p.y += p.vy
            p.vy += 0.00004 // gravity
            p.vx *= 0.985
            p.life += 1
            if p.life > p.maxLife { p.alive = false }
            burst.parts[i] = p
        }
    }

    // ── Bubbles: fixed population, constant rise + sway ──────────────
    private func tickBubbles(_ f: VizFrame) {
        var avg: Float = 0
        for b in f.bands { avg += b }
        avg /= 64
        for i in 0..<bubble.parts.count {
            if !bubble.parts[i].alive {
                if i < 18 + Int(avg * 20) { // refill to population target
                    bubble.parts[i] = VizParticle(
                        x: rand01(), y: 1.05 + rand01() * 0.15,
                        vx: 0, vy: 0.0006 + rand01() * 0.0014,
                        life: 0, maxLife: 1e9, size: 2.5 + rand01() * 6,
                        kind: 4, seed: rand01(), alive: true)
                }
                continue
            }
            var p = bubble.parts[i]
            p.y -= p.vy * (1 + avg * 1.5)
            p.x += sin(Float(uiTick) * 0.03 + p.seed * 6.28) * 0.0012 * (0.5 + avg)

            p.life += 1
            if p.y < -0.08 { p.alive = false } // popped at the surface
            bubble.parts[i] = p
        }
    }

    // ── Geyser: bass-weighted drizzle + transient jets ───────────────
    private func tickGeyser(_ f: VizFrame) {
        let mid = bandAvg(f.bands, 21, 42), high = bandAvg(f.bands, 42, 64)
        let steady = f.bass * 0.85 + mid * 0.25 + high * 0.08
        for _ in 0..<Int(steady * 6) {
            spawnGeyser(f, vy: 0.006 + steady * 0.016, spread: 0.05)
        }
        let delta = f.bass - prevBass
        if delta > 0.06 && f.bass > 0.15 {
            for _ in 0..<min(60, 20 + Int(delta * 120)) {
                spawnGeyser(f, vy: 0.018 + delta * 0.05 + f.bass * 0.015, spread: 0.10)
            }
        }
        for i in 0..<geyser.parts.count where geyser.parts[i].alive {
            var p = geyser.parts[i]
            p.vy += 0.00030 // gravity pulls back down (y+ is down)
            p.vx *= 0.992
            p.x += p.vx; p.y += p.vy
            p.life += 1
            if p.y > 1.02 || p.life > 200 { p.alive = false }
            geyser.parts[i] = p
        }
    }

    private func spawnGeyser(_ f: VizFrame, vy: Float, spread: Float) {
        let r = rand01()
        var tier: UInt8 = 0
        if r < f.bass { tier = 2 } else if r < f.bass + 0.4 { tier = 1 }
        geyser.spawn(VizParticle(
            x: 0.5 + (rand01() - 0.5) * 2 * spread, y: 1.0,
            vx: (rand01() - 0.5) * (0.002 + vy * 0.15), vy: -vy * (0.6 + rand01() * 0.5),
            life: 0, maxLife: 1e9, size: 1.5 + rand01(),
            kind: 6, seed: Float(tier), alive: true))
    }

    // ── RedSector: per-bar envelope tracking (ceiling/floor relax) ───
    private func tickRedSector(_ f: VizFrame) {
        for i in 0..<5 {
            let level = max(f.bands[i * 2], f.bands[i * 2 + 1])
            if level > rsCeil[i] { rsCeil[i] = level } else { rsCeil[i] -= 0.001 }
            if level < rsFloor[i] { rsFloor[i] = level } else { rsFloor[i] += 0.001 }
            if rsCeil[i] < rsFloor[i] + 0.06 { rsCeil[i] = rsFloor[i] + 0.06 }
            let norm = min(max((level - rsFloor[i]) / (rsCeil[i] - rsFloor[i]), 0), 1)
            let target = 0.40 + norm * 1.9
            let rate: Float = target > rsHeight[i] ? 0.75 : 0.28
            rsHeight[i] += (target - rsHeight[i]) * rate
        }
    }

    // ── Sand: falling-sand CA + bass bumps + overfill explosion ──────
    private func tickSand(_ f: VizFrame) {
        let W = VizState.sandW, H = VizState.sandH
        if sandBoomTTL > 0 || boomAlive() {
            tickSandBoom()
            prevBass = f.bass
            return
        }
        // spawn: each band emits at a column proportional to its index
        for b in 0..<64 {
            let level = f.bands[b]
            if level < 0.10 || rand01() > level * 0.85 { continue }
            let centre = (b * 2 + 1) * W / 128
            let spread = max(W / 128, 1)
            let x = min(max(centre + Int(rand01() * Float(2 * spread)) - spread, 0), W - 1)
            var tier: UInt8 = 1
            if b < 21 { tier = 3 } else if b < 42 { tier = 2 }
            if sand[x] == 0 { sand[x] = tier }
        }
        let delta = f.bass - prevBass
        // explosion: bed >30% full + a kick → everything goes ballistic
        if delta > 0.06 && f.bass > 0.15 {
            var fill = 0
            for g in sand where g != 0 { fill += 1 }
            if Float(fill) / Float(W * H) > 0.30 { startSandBoom(); return }
            // transient bump: vertical lift + lateral spray, deep grains fly most
            let strength = min(delta * 3.5 + f.bass * 0.8, 1.4)
            for y in 0..<H {
                let depth = Float(y) / Float(H - 1)
                let liftProb = min(strength * (0.30 + 0.70 * depth), 0.95)
                let liftMax = 2 + Int(strength * 7 * (0.4 + 0.6 * depth))
                let jit = 1 + Int(strength * 5)
                for x in 0..<W where sand[y * W + x] != 0 {
                    if rand01() > liftProb { continue }
                    let ny = max(y - 1 - Int(rand01() * Float(liftMax)), 0)
                    let nx = min(max(x + Int(rand01() * Float(2 * jit + 1)) - jit, 0), W - 1)
                    if sand[ny * W + nx] == 0 {
                        sand[ny * W + nx] = sand[y * W + x]; sand[y * W + x] = 0
                    }
                }
            }
        }
        // sustained rumble: bottom half churns while bass stays high
        if f.bass > 0.30 {
            let rumble = min((f.bass - 0.30) * 1.8, 0.6)
            for y in (H / 2)..<H {
                let depth = Float(y - H / 2) / Float(H - 1 - H / 2)
                let prob = rumble * (0.15 + 0.55 * depth)
                for x in 0..<W where sand[y * W + x] != 0 {
                    if rand01() > prob { continue }
                    let ny = max(y - 1 - Int(rand01() * 2), 0)
                    let nx = min(max(x + Int(rand01() * 5) - 2, 0), W - 1)
                    if sand[ny * W + nx] == 0 {
                        sand[ny * W + nx] = sand[y * W + x]; sand[y * W + x] = 0
                    }
                }
            }
        }
        // fall pass: bottom-up, alternating scan direction; diag slides
        for y in stride(from: H - 2, through: 0, by: -1) {
            let leftFirst = uiTick % 2 == 0
            for xi in 0..<W {
                let x = leftFirst ? xi : W - 1 - xi
                let g = sand[y * W + x]
                if g == 0 { continue }
                if sand[(y + 1) * W + x] == 0 {
                    sand[(y + 1) * W + x] = g; sand[y * W + x] = 0
                    continue
                }
                let d1: Int = rand01() < 0.5 ? -1 : 1
                for dx in [d1, -d1] {
                    let nx = x + dx
                    if nx >= 0 && nx < W && sand[(y + 1) * W + nx] == 0 {
                        sand[(y + 1) * W + nx] = g; sand[y * W + x] = 0
                        break
                    }
                }
            }
        }
        // floor drifts off so the bed can't pack solid over time
        for x in 0..<W where sand[(H - 1) * W + x] != 0 {
            if rand01() < 0.04 { sand[(H - 1) * W + x] = 0 }
        }
    }

    private func boomAlive() -> Bool {
        for p in sandBoom.parts where p.alive { return true }
        return false
    }

    private func startSandBoom() {
        let W = VizState.sandW, H = VizState.sandH
        var n = 0
        for y in 0..<H {
            let depth = Float(y) / Float(H - 1)
            for x in 0..<W where sand[y * W + x] != 0 {
                let g = sand[y * W + x]; sand[y * W + x] = 0
                if n >= 400 { continue } // particle cap: bed still clears
                sandBoom.spawn(VizParticle(
                    x: Float(x) / Float(W), y: Float(y) / Float(H),
                    vx: (rand01() - 0.5) * 0.03,
                    vy: -(0.008 + rand01() * 0.02 + depth * 0.008),
                    life: 0, maxLife: 1e9, size: 1,
                    kind: 6, seed: Float(g), alive: true))
                n += 1
            }
        }
        sandBoomTTL = 80
    }

    private func tickSandBoom() {
        if sandBoomTTL > 0 { sandBoomTTL -= 1 }
        for i in 0..<sandBoom.parts.count where sandBoom.parts[i].alive {
            var p = sandBoom.parts[i]
            p.vy += 0.0008; p.vx *= 0.985
            p.x += p.vx; p.y += p.vy
            if p.x < 0 || p.x > 1 || p.y < 0 || p.y > 1 { p.alive = false }
            sandBoom.parts[i] = p
        }
    }

    /// Ring column `age` frames back (0 = newest), row 0..31.
    func ringCol(age: Int, row: Int) -> Float {
        let c = (ringHead - 1 - age + VizState.ringCols * 2) % VizState.ringCols
        return ring[c * VizState.ringRows + row]
    }
}

/// Linear-interpolated band lookup at fractional `pos` in band space —
/// the alloc-free counterpart of resampleBands (cliamp sampleBandLinear).
func sampleBand(_ bands: [Float], pos: Float) -> Float {
    guard !bands.isEmpty else { return 0 }
    if bands.count == 1 { return bands[0] }
    let last = Float(bands.count - 1)
    if pos <= 0 { return bands[0] }
    if pos >= last { return bands[bands.count - 1] }
    let i = Int(pos), f = pos - Float(i)
    return bands[i] * (1 - f) + bands[i + 1] * f
}

/// Mean of bands[lo..<hi] — bass/mid/high subbands (cliamp bandAvg).
func bandAvg(_ b: [Float], _ lo: Int, _ hi: Int) -> Float {
    let lo = max(lo, 0), hi = min(hi, b.count)
    guard hi > lo else { return 0 }
    var s: Float = 0
    for i in lo..<hi { s += b[i] }
    return s / Float(hi - lo)
}

/// Average 64 bands down/up to `n` buckets.
func resampleBands(_ bands: [Float], to n: Int) -> [Float] {
    guard n > 0 else { return [] }
    var out = [Float](repeating: 0, count: n)
    for i in 0..<n {
        let lo = i * 64 / n, hi = max((i + 1) * 64 / n, lo + 1)
        var s: Float = 0
        for j in lo..<min(hi, 64) { s += bands[j] }
        out[i] = s / Float(max(hi - lo, 1))
    }
    return out
}

/// Deterministic per-frame hash noise in 0..1 — flicker without RNG state.
func hashNoise(_ a: UInt64, _ b: Int) -> Float {
    var h = a &+ UInt64(b) &* 0x9E3779B97F4A7C15
    h ^= h >> 29; h &*= 0xBF58476D1CE4E5B9
    h ^= h >> 31
    return Float(h & 0xFFFF) / 65535
}
