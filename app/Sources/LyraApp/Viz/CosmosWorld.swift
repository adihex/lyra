import CoreGraphics
import Foundation

/// Wander FSM — which behavior currently owns the cast's heading.
/// `.perch` carries the CGWindowID of the window whose top edge we're on.
enum WanderState: Hashable {
    case walkBottom         // patrol the bottom edge of visibleFrame
    case eight              // lazy Lissajous across the mid band — the dance
    case perch(CGWindowID)  // sit on a large window's top edge
    case sleep              // seq stall / menu Sleep — converge, dim 30%
}

/// World-space layer over CosmosScene (desktop-pet.md §7): positions,
/// velocities, and the shimeji-style wander FSM the scene doesn't have.
/// Only the pet surface owns one; dock/pane run the scene with fixed
/// origins. Pure value type — the driver feeds VizFrames + bounds in,
/// panel center positions come out.
struct CosmosWorld {
    typealias Body = CosmosScene.Body

    var bodies: [Body: CGPoint]
    var vel: [Body: CGVector]
    var fsm: WanderState = .walkBottom
    var speed: Float = 40      // eased roam speed, pt/s
    var dimness: Float = 0     // 0…1 → painter dims ~30% while asleep
    var logoAngle: Float = 0   // logo tumble (mid-band spin rate)
    var dashT: Float = 0       // clip comet-dash TTL (0.7 s)
    var dashDir = CGVector.zero
    /// Set when the pop lands — the first hop waits for a beat edge.
    var awaitingBeat = false

    private var target = CGPoint.zero
    private var groundY: CGFloat = 0
    private var stateT: Float = 0
    private var stateDur: Float = 9
    private var walkDir: CGFloat = 1
    private var idleT: Float = 0
    private var eightT: Float = 0
    private var hopVel: CGFloat = 0
    private var airborne = false
    private var stallT: Float = 0
    private var lastSeq: UInt64 = 0
    private var autoSlept = false
    private var dockK: Float = 0       // 0…1 → moon docks onto the planet
    private var moonKick = CGVector.zero // post-drag slingshot offset
    private var orbitOff = CGVector.zero
    private var sleepAnchor = CGPoint.zero
    private var prevBeat: Float = 0
    private var prevClip = false
    private var dragBody: Body?
    private var grabOff = CGVector.zero
    private var lastBounds = CGRect.zero
    private var rng: UInt64 = 0x5DEECE66D

    init(bounds: CGRect) {
        let s = CGPoint(x: bounds.midX, y: bounds.minY + 76)
        bodies = [.saturn: s,
                  .moon: CGPoint(x: s.x + 36, y: s.y - 21), // parked −35°
                  .logo: CGPoint(x: s.x - 82, y: s.y + 8)]
        vel = [.saturn: .zero, .moon: .zero, .logo: .zero]
        target = s
        groundY = s.y
        sleepAnchor = s
        lastBounds = bounds
    }

    /// Fully converged + dimmed — the driver may stop paying window moves.
    var settled: Bool {
        fsm == .sleep && dimness > 0.9 && !airborne && dashT <= 0
    }

    /// Menu Sleep — stays out, converges and dims.
    mutating func sleep() {
        if fsm != .sleep { enter(.sleep); autoSlept = false }
    }

    mutating func wake() {
        if fsm == .sleep { enter(.walkBottom); autoSlept = false }
    }

    // ── pet-mode dragging: saturn carries the system, the moon
    //    slingshots back into orbit on release ─────────────────────
    mutating func beginDrag(_ b: Body, at p: CGPoint) {
        guard let pos = bodies[b] else { return }
        dragBody = b
        grabOff = CGVector(dx: pos.x - p.x, dy: pos.y - p.y)
        if b == .saturn { airborne = false }
    }

    mutating func drag(to p: CGPoint) {
        guard let b = dragBody, let pos = bodies[b] else { return }
        let q = CGPoint(x: p.x + grabOff.dx, y: p.y + grabOff.dy)
        if b == .saturn {
            let d = CGVector(dx: q.x - pos.x, dy: q.y - pos.y)
            for k in bodies.keys { bodies[k]!.x += d.dx; bodies[k]!.y += d.dy }
            target.x += d.dx; target.y += d.dy
            sleepAnchor.x += d.dx; sleepAnchor.y += d.dy
        } else {
            bodies[b] = q
        }
    }

    mutating func endDrag() {
        if dragBody == .moon, let m = bodies[.moon], let s = bodies[.saturn] {
            // slingshot: leftover displacement decays back into the orbit
            moonKick = CGVector(dx: m.x - s.x - orbitOff.dx,
                                dy: m.y - s.y - orbitOff.dy)
        }
        if let b = dragBody { vel[b] = .zero }
        dragBody = nil
    }

    /// One roam step. `perches` maps window IDs → Cocoa-space frames of
    /// large on-screen windows (enumerated by the driver on a slow timer).
    mutating func roam(_ f: VizFrame, _ scene: CosmosScene, dt: Float,
                       bounds: CGRect, perches: [CGWindowID: CGRect]) {
        lastBounds = bounds
        stateT += dt
        speed += ((40 + 260 * f.level) - speed) * 0.1 // swoops, not steps

        // seq stall → sleep; fresh seq while auto-asleep → wake
        if f.seq == lastSeq { stallT += dt } else { stallT = 0; lastSeq = f.seq }
        if stallT > 4 && fsm != .sleep { enter(.sleep); autoSlept = true }
        if fsm == .sleep && autoSlept && stallT == 0 { enter(.walkBottom); autoSlept = false }

        // clip edge → comet dash: the whole cast darts ~200 pt, ease-out
        let clip = f.clipL || f.clipR
        if clip && !prevClip && fsm != .sleep {
            dashT = 0.7
            let a = rnd() * 2 * .pi
            dashDir = CGVector(dx: cos(CGFloat(a)), dy: sin(CGFloat(a)))
        }
        prevClip = clip
        if dashT > 0 { dashT = max(0, dashT - dt) }

        // beat rising edge → hop (grounded walk/perch only; the first
        // hop after the pop lands waits for this edge — it's on the music)
        let edge = f.beat > 0.55 && prevBeat <= 0.55
        prevBeat = f.beat
        if awaitingBeat {
            if edge { awaitingBeat = false; airborne = true; hopVel = 320 }
        } else if edge && !airborne && (fsm == .walkBottom || isPerch) {
            airborne = true
            hopVel = 300 + 160 * CGFloat(f.level)
        }

        // dim toward sleep / back to bright
        let dimT: Float = fsm == .sleep ? 1 : 0
        dimness += (dimT - dimness) * min(1, 2.5 * dt)
        dockK += ((fsm == .sleep ? 1 : 0) - dockK) * min(1, 2.5 * dt)

        // ── FSM: pick targets + ground lines ──
        switch fsm {
        case .walkBottom:
            groundY = bounds.minY + 76
            if idleT > 0 {
                idleT -= dt
            } else {
                target.x += walkDir * CGFloat(speed) * CGFloat(dt)
                target.y = groundY
                if target.x < bounds.minX + 70 { target.x = bounds.minX + 70; walkDir = 1 }
                if target.x > bounds.maxX - 70 { target.x = bounds.maxX - 70; walkDir = -1 }
                if rnd() < 0.09 * dt { idleT = 0.8 + 1.8 * rnd() } // pause to bob
            }
            if stateT > stateDur { pickNext(bounds: bounds, perches: perches) }
        case .eight:
            groundY = -1e9 // no floor while drifting
            eightT += dt
            let w = CGFloat(0.45 + 0.8 * f.level)
            target = CGPoint(
                x: bounds.midX + sin(CGFloat(eightT) * w) * bounds.width * 0.30,
                y: bounds.minY + bounds.height * 0.42
                   + sin(CGFloat(eightT) * w * 2 + 1.3) * bounds.height * 0.16)
            if stateT > stateDur { pickNext(bounds: bounds, perches: perches) }
        case .perch(let id):
            if let r = perches[id] {
                groundY = r.maxY + 46 // sit on the title bar
                target = CGPoint(x: r.midX, y: groundY)
            } else {
                enter(.walkBottom) // perch vanished — back to the floor
            }
            if stateT > stateDur { pickNext(bounds: bounds, perches: perches) }
        case .sleep:
            groundY = bounds.minY + 76
            target = sleepAnchor
        }

        // ── saturn follows the FSM target; x always tracks so beat
        //    hops chain into a bouncy walk across the screen edge ──
        if let p = bodies[.saturn], dragBody != .saturn {
            var q = p
            let step = CGFloat(speed) * CGFloat(dt) * (fsm == .sleep ? 0.7 : 1)
            q.x += max(-step, min(step, target.x - q.x))
            if airborne {
                q.y += hopVel * CGFloat(dt)
                hopVel -= 1500 * CGFloat(dt)
                if q.y <= groundY { q.y = groundY; airborne = false }
            } else {
                q.y += max(-step, min(step, target.y - q.y))
            }
            if dt > 0 {
                vel[.saturn] = CGVector(dx: (q.x - p.x) / CGFloat(dt),
                                        dy: (q.y - p.y) / CGFloat(dt))
            }
            bodies[.saturn] = q
        }
        let sPos = bodies[.saturn] ?? target

        // ── moon: Lissajous of waveL/waveR around saturn (the moon
        //    surfs the waveform in world space) + a base circle at
        //    moonAngle so quiet bars don't collapse it onto the planet ──
        let r = 44 + 18 * CGFloat(f.bass)
        let ma = scene.state.moonAngle
        let ai = ((Int(ma / (2 * .pi) * 256) % 256) + 256) % 256
        var orb = CGVector(
            dx: (CGFloat(f.waveL[ai]) + 0.28 * cos(CGFloat(ma))) * r,
            dy: (CGFloat(f.waveR[ai]) + 0.28 * sin(CGFloat(ma))) * r)
        orb.dx *= CGFloat(1 - dockK); orb.dy *= CGFloat(1 - dockK)
        orbitOff = orb
        if dragBody != .moon {
            bodies[.moon] = CGPoint(x: sPos.x + orb.dx + moonKick.dx,
                                    y: sPos.y + orb.dy + moonKick.dy)
        }
        moonKick.dx *= pow(0.02, CGFloat(dt))
        moonKick.dy *= pow(0.02, CGFloat(dt))

        // ── logo: the playful one — spring-follows behind saturn,
        //    bounce amplitude + tumble rate off the mid bands ──
        let mid = bandAvg(f.bands, 21, 48)
        logoAngle += (0.6 + 5.0 * mid) * dt
        if dragBody != .logo, var v = vel[.logo], var lp = bodies[.logo] {
            let home: CGPoint = fsm == .sleep
                ? CGPoint(x: sPos.x - 74, y: sPos.y - 18) // parks beside
                : CGPoint(x: sPos.x - 80 + 12 * sin(CGFloat(logoAngle) * 0.6),
                          y: sPos.y + 6 + (8 + 30 * CGFloat(mid))
                              * sin(CGFloat(logoAngle) * 1.7))
            v.dx += (home.x - lp.x) * 10 * CGFloat(dt)
            v.dy += (home.y - lp.y) * 10 * CGFloat(dt)
            let damp = pow(CGFloat(0.10), CGFloat(dt))
            v.dx *= damp; v.dy *= damp
            lp.x += v.dx * CGFloat(dt); lp.y += v.dy * CGFloat(dt)
            vel[.logo] = v
            bodies[.logo] = lp
        }

        // ── comet dash applies to every free body ──
        if dashT > 0 {
            let v = 560 * CGFloat(dashT) / 0.7 * CGFloat(dt)
            for b in bodies.keys where b != dragBody {
                bodies[b]!.x += dashDir.dx * v
                bodies[b]!.y += dashDir.dy * v
            }
        }

        // never leave the screen — the cast is single-display by design
        let inset = bounds.insetBy(dx: 24, dy: 24)
        for b in bodies.keys where b != dragBody {
            bodies[b]!.x = min(max(bodies[b]!.x, inset.minX), inset.maxX)
            bodies[b]!.y = min(max(bodies[b]!.y, inset.minY), inset.maxY)
        }
    }

    private var isPerch: Bool {
        if case .perch = fsm { return true }
        return false
    }

    private mutating func enter(_ s: WanderState) {
        fsm = s
        stateT = 0
        switch s {
        case .walkBottom:
            stateDur = 8 + 8 * rnd()
            if let p = bodies[.saturn] {
                target = CGPoint(x: min(max(p.x, lastBounds.minX + 70),
                                        lastBounds.maxX - 70),
                                 y: lastBounds.minY + 76)
                walkDir = p.x > lastBounds.midX ? -1 : 1
            }
        case .eight:
            stateDur = 7 + 7 * rnd()
        case .perch:
            stateDur = 6 + 8 * rnd()
        case .sleep:
            stateDur = .infinity
            if let p = bodies[.saturn] {
                sleepAnchor = CGPoint(x: min(max(p.x, lastBounds.minX + 70),
                                             lastBounds.maxX - 70),
                                      y: lastBounds.minY + 76)
            }
        }
    }

    /// Weighted pick among the *other* states — perch only when a big
    /// on-screen window is available.
    private mutating func pickNext(bounds: CGRect, perches: [CGWindowID: CGRect]) {
        var opts: [WanderState] = [.walkBottom, .eight]
        if let id = largestPerch(perches, bounds: bounds) { opts.append(.perch(id)) }
        var i = Int(rnd() * Float(opts.count)) % opts.count
        if opts[i] == fsm { i = (i + 1) % opts.count }
        enter(opts[i])
    }

    private func largestPerch(_ perches: [CGWindowID: CGRect],
                              bounds: CGRect) -> CGWindowID? {
        perches.filter { $0.value.intersects(bounds) }
            .max { $0.value.width * $0.value.height
                   < $1.value.width * $1.value.height }?.key
    }

    private mutating func rnd() -> Float {
        rng ^= rng << 13; rng ^= rng >> 7; rng ^= rng << 17
        return Float((rng >> 11) & 0xFFFF) / 65535
    }
}
