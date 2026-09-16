import CoreGraphics
import Foundation

/// POSIX drand48 — reimplemented so the star field is bit-identical to
/// scripts/make_icon.swift's `srand48(20260916)` sequence.
private struct Drand48 {
    var state: UInt64
    init(seed: Int64) { state = (UInt64(bitPattern: seed) << 16) | 0x330E }
    mutating func next() -> Double {
        state = (state &* 0x5DEECE66D &+ 0xB) & 0xFF_FFFF_FFFFFF
        return Double(state) / 281474976710656 // 2^48
    }
}

/// Cosmos — "the icon, alive" (docs/research/dock-icon-viz.md §2).
/// Pure motion state shared by the dock-tile CG painter (CosmosPaintCG)
/// and the in-app mode (VizDraw.cosmos). All coordinates are normalized
/// to the unit square, y up, with the same constants make_icon.swift
/// bakes into AppIcon.icns — so the resting frame composes back to the
/// shipped artwork (moon parked at −35°, stars at base alpha).
/// Fixed capacity: one 110-entry star table, one shooter, no per-frame
/// allocation anywhere in drive().
struct CosmosSceneState {
    // geometry — fractions of the scene side (make_icon's S-normalized)
    static let parkAngle: Float = -35 * .pi / 180
    static let planet = SIMD2<Float>(0.5, 0.47)
    static let planetR: Float = 0.215
    static let orbitR: Float = 0.36
    static let moonR: Float = 0.055

    struct Star {
        var x, y: Float   // 0...1, y up — rect ORIGIN like make_icon
        var r: Float      // diameter in icon px (S = 1024)
        var alpha: Float  // base 0.12...0.72
        var mint: Bool
    }

    var stars: [Star]
    var moonAngle = Self.parkAngle
    var omega: Float = 0.12    // rad/s — eased toward 0.12 + 2.8·level
    var ringTilt: Float = 0    // rad — decays 0.92/tick
    var ringKick: Float = 0    // rad — impulse per bass rising edge
    var ringAmt: Float = 0     // smoothed bass → circle↔ring-plane blend
    var pulse: Float = 0       // beat chased by an underdamped spring
    var springV: Float = 0
    var stripePhase: Float = 0 // latitude-band drift, advances on mids
    var glow: Float = 0.28     // eased mint-glow alpha
    var lissAmt: Float = 0     // pane only: circle↔waveform orbit blend
    var mid: Float = 0         // bands[21...48] mean — stripe/glow energy
    var hi: Float = 0          // bands[48...63] mean — star flicker
    var glint: Float = 0       // eased peak — ring glint alpha
    var shootX: Float = 0, shootY: Float = 0
    var shootVX: Float = 0, shootVY: Float = 0
    var shootTTL: Float = 0    // seconds left — 0 = no shooter
    var idle: Float = 0        // seconds level<ε — gates the park slew
    var tick: UInt64 = 0
    var lastLevel: Float = 0, lastBeat: Float = 0
    var prevBass: Float = 0
    var prevClip = false
    var parked = true

    init() {
        // The icon's star field verbatim — five drand48 draws per star in
        // make_icon's order (x, y, r, a, tint), same seed → same sky.
        var rng = Drand48(seed: 20260916)
        var s: [Star] = []
        s.reserveCapacity(110)
        for _ in 0..<110 {
            s.append(Star(x: Float(rng.next()), y: Float(rng.next()),
                          r: Float(rng.next()) * 2.4 + 0.5,
                          alpha: Float(rng.next()) * 0.6 + 0.12,
                          mint: rng.next() < 0.2))
        }
        stars = s
    }

    /// One motion step — the doc's field→motion map. `dt` in seconds; all
    /// per-tick constants are exponent-scaled by `k = dt·60` so the 12 Hz
    /// dock driver and the 60 Hz pane move at the same real-time speed.
    mutating func drive(f: VizFrame, dt: Float, size: CGSize) {
        tick &+= 1
        lastLevel = f.level
        lastBeat = f.beat
        let k = dt * 60
        mid = bandAvg(f.bands, 21, 48)
        hi = bandAvg(f.bands, 48, 64)
        glint += (max(f.peakL, f.peakR) - glint) * (1 - powf(0.8, k))

        // moon — ω eases toward the level target so speed-ups swoop
        omega += ((0.12 + 2.8 * f.level) - omega) * (1 - powf(0.85, k))
        if f.level < 0.01 { idle += dt } else { idle = 0 }
        if idle > 1.0 {
            // stalled ~1s: decelerate and slew home along the shortest
            // arc — a teleport would pop the resting icon.
            var d = (Self.parkAngle - moonAngle)
                .truncatingRemainder(dividingBy: 2 * .pi)
            if d > .pi { d -= 2 * .pi } else if d < -.pi { d += 2 * .pi }
            omega *= powf(0.9, k)
            moonAngle += d * (1 - powf(0.92, k))
            if abs(d) < 0.004 && omega < 0.3 {
                moonAngle = Self.parkAngle
                parked = true
            }
        } else {
            parked = false
            moonAngle = (moonAngle + omega * dt)
                .truncatingRemainder(dividingBy: 2 * .pi)
        }

        // ring — bass rising edges kick the tilt (tickGeyser's edge
        // pattern); the kick integrates into tilt and both relax.
        if f.bass - prevBass > 0.06 && f.bass > 0.15 {
            ringKick += 5 * .pi / 180
        }
        ringKick *= powf(0.86, k)
        ringTilt = ringTilt * powf(0.92, k) + f.bass * ringKick * k
        ringAmt += (f.bass - ringAmt) * (1 - powf(0.8, k))

        // planet — beat through an underdamped spring: the attack lands
        // instantly, the rebound wobbles (secondary overshoot).
        springV += (f.beat - pulse) * 140 * dt
        springV *= expf(-9 * dt)
        pulse += springV * dt
        pulse = min(max(pulse, -0.15), 1.4)

        // mids — latitude drift + glow breathe
        stripePhase += mid * 1.2 * dt
        glow += ((0.28 + 0.17 * mid) - glow) * (1 - powf(0.8, k))
        lissAmt += (min(f.level * 1.5, 1) - lissAmt) * (1 - powf(0.85, k))

        // clip rising edge → one terracotta streak (pane adds beat-edge
        // shooters itself via spawnShooter)
        let clipNow = f.clipL || f.clipR
        if clipNow && !prevClip { spawnShooter() }
        prevClip = clipNow
        if shootTTL > 0 {
            shootTTL -= dt
            let drag = expf(-2.2 * dt) // eases out as it falls
            shootVX *= drag
            shootVY *= drag
            shootX += shootVX * dt
            shootY += shootVY * dt
        }
        prevBass = f.bass
    }

    /// Streak spawn — top-left → planet-ward, ~0.7 s TTL. Jitter comes
    /// off the tick hash so there's no RNG state to carry.
    mutating func spawnShooter(strength: Float = 1) {
        let jx = hashNoise(tick, 911), jy = hashNoise(tick, 977)
        shootX = 0.10 + jx * 0.22
        shootY = 0.86 + jy * 0.10
        shootVX = (0.70 + jy * 0.30) * strength
        shootVY = -(0.35 + jx * 0.20) * strength
        shootTTL = 0.7
    }

    /// The orbit as drawn this tick: the icon's parked circle squashes
    /// into Saturn's ring plane as bass asserts; the pulse spring adds
    /// the beat's 2° tilt kick on top.
    func orbitShape() -> (tilt: Float, squash: Float) {
        let ringSquash = 0.34 + 0.06 * sin(moonAngle)
        let squash = 1 + (ringSquash - 1) * min(ringAmt * 1.6, 1)
        return (ringTilt + pulse * 2 * .pi / 180, squash)
    }

    /// Point on the tilted/squashed orbit ellipse (normalized, y up).
    func orbitPoint(_ angle: Float, tilt: Float, squash: Float) -> SIMD2<Float> {
        let ex = Self.orbitR * cos(angle)
        let ey = Self.orbitR * sin(angle) * squash
        let c = cos(tilt), s = sin(tilt)
        return Self.planet + SIMD2(ex * c - ey * s, ex * s + ey * c)
    }

    /// Scene fully settled: level/beat quiet, moon parked, ring relaxed,
    /// spring still, no shooter. The dock driver stops paying display()
    /// IPC once this holds — the parked frame IS the static icon.
    var isAtRest: Bool {
        parked && lastLevel < 0.01 && lastBeat < 0.02 && shootTTL <= 0
            && abs(ringTilt) < 0.005 && abs(ringKick) < 0.004
            && pulse < 0.03 && abs(springV) < 0.06
    }
}

/// The SHARED scene instance living on VizRuntime (desktop-pet.md §7):
/// the dock tile paints it and the desktop pet adopts it on pop so the
/// transition is continuous — same planet stepping out, not a respawn.
/// `away` marks bodies currently popped out; painters must skip them —
/// while the pet is out the dock icon renders the sky WITHOUT the planet.
/// (The viz pane keeps its own per-VizState CosmosSceneState instead —
/// two mounted surfaces can never share one mutable state.)
final class CosmosScene {
    /// Cast members that can leave the icon for the desktop.
    enum Body: Hashable { case saturn, moon, logo }

    var state = CosmosSceneState()
    var away: Set<Body> = []

    // forwarders — surfaces drive it like the bare state
    var isAtRest: Bool { state.isAtRest }
    var moonAngle: Float { state.moonAngle }
    var ringTilt: Float { state.ringTilt }
    var planetBob: Float { state.pulse }
    var stripePhase: Float { state.stripePhase }
    var tick: UInt64 { state.tick }
    var level: Float { state.lastLevel }
    var mid: Float { state.mid }
    var hi: Float { state.hi }
    var glint: Float { state.glint }
    func drive(_ f: VizFrame, dt: Float) {
        state.drive(f: f, dt: dt, size: .zero)
    }
    func drive(f: VizFrame, dt: Float, size: CGSize = .zero) {
        state.drive(f: f, dt: dt, size: size)
    }
    func spawnShooter(strength: Float = 1) { state.spawnShooter(strength: strength) }
    func orbitShape() -> (tilt: Float, squash: Float) { state.orbitShape() }
    func orbitPoint(_ angle: Float, tilt: Float, squash: Float) -> SIMD2<Float> {
        state.orbitPoint(angle, tilt: tilt, squash: squash)
    }
}
