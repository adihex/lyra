import SwiftUI

/// Cosmos — the dock-icon scene at pane scale (dock-icon-viz §4). Shares
/// CosmosSceneState with the tile painter; the extras the icon can't
/// show live here: the orbit path is the live Lissajous of waveL×waveR
/// (the moon literally surfs the waveform), the ring granulates off
/// bands[0…31] into a circular spectrum, and big beat edges spawn
/// shooting stars alongside clip edges.
extension VizDraw {

    /// Semantic theme colors only — the Cosmos cast wears the same
    /// PetPalette roles as the dock tile and desktop pet (sky =
    /// tray→chassis, planet = accent→tint→bodyDark, moon =
    /// companion→secondaryInk). Dynamic providers resolve per
    /// appearance; alpha ramps unchanged.

    static func cosmos(_ ctx: inout GraphicsContext, _ size: CGSize,
                       _ f: VizFrame, _ st: VizState) {
        st.cosmos.drive(f: f, dt: 1.0 / 60, size: size)
        let s = st.cosmos
        // pane-only extra: big beat edges also send a streak
        if f.beat > 0.8 && st.prevBeat <= 0.8 { st.cosmos.spawnShooter(strength: 0.7) }

        // Canvas is y-down; scene math is y-up — flip once, then the
        // geometry is identical to CosmosPaintCG.
        ctx.translateBy(x: 0, y: size.height)
        ctx.scaleBy(x: 1, y: -1)

        let W = size.width, H = size.height
        let S = min(W, H)
        let ox = (W - S) / 2, oy = (H - S) / 2
        func P(_ v: SIMD2<Float>) -> CGPoint {
            CGPoint(x: ox + CGFloat(v.x) * S, y: oy + CGFloat(v.y) * S)
        }

        // ── background: full-bleed space ──
        ctx.fill(Path(CGRect(origin: .zero, size: size)),
                 with: .color(Color.bubbleChassis))
        ctx.fill(Path(CGRect(origin: .zero, size: size)),
                 with: .linearGradient(
                    Gradient(colors: [Color.bubbleTray,
                                      Color.bubbleChassis]),
                    startPoint: CGPoint(x: W * 0.3, y: H),
                    endPoint: CGPoint(x: W * 0.7, y: 0)))

        // ── star field across the whole pane + slow depth drift ──
        let hi = bandAvg(f.bands, 48, 64)
        for i in 0..<s.stars.count {
            let st_ = s.stars[i]
            let drift = Float(s.tick) * (0.00002 + 0.00006 * st_.r)
            let x = CGFloat((st_.x + drift).truncatingRemainder(dividingBy: 1)) * W
            let y = CGFloat(st_.y) * H
            let d = CGFloat(st_.r) * S / 1024
            let a = Double(st_.alpha) + Double(hi * hashNoise(s.tick / 6, i)) * 0.5
            ctx.fill(Path(ellipseIn: CGRect(x: x, y: y, width: d, height: d)),
                     with: .color((st_.mint ? Color.bubbleAccent
                                            : Color.bubbleKeycap)
                                  .opacity(min(a, 1))))
        }
        // twinkle pluses — the icon's two
        for (sx, sy, sr) in [(0.78, 0.82, 0.016), (0.2, 0.68, 0.012)]
            as [(CGFloat, CGFloat, CGFloat)] {
            let cx = ox + sx * S, cy = oy + sy * S, r = sr * S
            ctx.fill(Path(ellipseIn: CGRect(x: cx - r * 0.35, y: cy - r * 0.35,
                                            width: r * 0.7, height: r * 0.7)),
                     with: .color(Color.bubbleKeycap.opacity(0.9)))
            var plus = Path()
            plus.move(to: CGPoint(x: cx - r, y: cy))
            plus.addLine(to: CGPoint(x: cx + r, y: cy))
            plus.move(to: CGPoint(x: cx, y: cy - r))
            plus.addLine(to: CGPoint(x: cx, y: cy + r))
            ctx.stroke(plus, with: .color(Color.bubbleKeycap.opacity(0.55)),
                       style: StrokeStyle(lineWidth: r * 0.16, lineCap: .round))
        }

        let mid = bandAvg(f.bands, 21, 48)
        let (tilt, squash) = s.orbitShape()
        let center = CosmosSceneState.planet
        let bobY = max(s.pulse, 0) * 0.020
        let pc = P(center + SIMD2(0, bobY))
        let pr = S * CGFloat(CosmosSceneState.planetR)

        // ── companion glow ──
        ctx.fill(Path(ellipseIn: CGRect(x: pc.x - S * 0.44, y: pc.y - S * 0.44,
                                        width: S * 0.88, height: S * 0.88)),
                 with: .radialGradient(
                    Gradient(colors: [Color.bubbleCompanion.opacity(Double(s.glow)),
                                      Color.bubbleCompanion.opacity(0)]),
                    center: pc, startRadius: 0, endRadius: S * 0.44))

        // ── orbit: circle ↔ live Lissajous of waveL×waveR (the moon
        //    surfs the waveform); silent wave parks it back on-circle ──
        var orbit = Path()
        let n = min(f.waveL.count, f.waveR.count)
        for i in 0..<n {
            let a = Float(i) / Float(n) * 2 * .pi
            let c = s.orbitPoint(a, tilt: tilt, squash: squash)
            let l = center + SIMD2(f.waveL[i], f.waveR[i])
                * CosmosSceneState.orbitR * 0.85
            let pt = P(c + (l - c) * s.lissAmt)
            i == 0 ? orbit.move(to: pt) : orbit.addLine(to: pt)
        }
        orbit.closeSubpath()
        ctx.stroke(orbit,
                   with: .color(Color.bubbleCompanion.opacity(
                        min(0.16 + 0.30 * Double(f.bass), 1))),
                   lineWidth: max(S * 0.003, 0.8))

        // ring granulation — bands[0…31] as grains around the ellipse:
        // the ring is secretly a circular spectrum
        var grains = [Path(), Path(), Path()]
        for i in 0..<32 {
            let b = f.bands[i]
            guard b > 0.04 else { continue }
            let gp = P(s.orbitPoint(Float(i) / 32 * 2 * .pi,
                                    tilt: tilt, squash: squash))
            let gr = (1.5 + 4 * CGFloat(b)) * max(S / 600, 0.7)
            grains[b >= 0.6 ? 2 : (b >= 0.3 ? 1 : 0)]
                .addEllipse(in: CGRect(x: gp.x - gr, y: gp.y - gr,
                                       width: gr * 2, height: gr * 2))
        }
        ctx.fill(grains[0], with: .color(Ui.mint.opacity(0.75)))
        ctx.fill(grains[1], with: .color(Ui.indigo.opacity(0.8)))
        ctx.fill(grains[2], with: .color(Ui.accent.opacity(0.85)))

        // ring glint opposite the moon
        let glintA = max(f.peakL, f.peakR)
        if glintA > 0.05 {
            let gp = P(s.orbitPoint(s.moonAngle + .pi,
                                    tilt: tilt, squash: squash))
            let gr = S * 0.014
            ctx.fill(Path(ellipseIn: CGRect(x: gp.x - gr, y: gp.y - gr,
                                            width: gr * 2, height: gr * 2)),
                     with: .color(Color.bubbleAccent.opacity(Double(glintA) * 0.9)))
        }

        // ── planet: bob + squash, drifting latitude bands, craters ──
        let sx = 1 + 0.05 * s.pulse, sy = 1 - 0.07 * s.pulse
        var planetCtx = ctx
        planetCtx.translateBy(x: pc.x, y: pc.y)
        planetCtx.scaleBy(x: CGFloat(sx), y: CGFloat(sy))
        let prect = CGRect(x: -pr, y: -pr, width: pr * 2, height: pr * 2)
        planetCtx.clip(to: Path(ellipseIn: prect))
        planetCtx.fill(Path(prect),
                       with: .radialGradient(
                        Gradient(colors: [Color.bubbleAccent,
                                          Color.bubbleTint,
                                          Color.bubbleBodyDark]),
                        center: CGPoint(x: -pr * 0.45, y: pr * 0.5),
                        startRadius: pr * 0.1, endRadius: pr * 1.6))
        for k in [-1, 0, 1] as [CGFloat] {
            let y = k * pr * 0.42
            let drift = CGFloat(sin(s.stripePhase + Float(k) * 1.7)) * pr * 0.12
            var band = Path()
            band.move(to: CGPoint(x: -pr * 0.75 + drift, y: y))
            band.addCurve(to: CGPoint(x: pr * 0.75 + drift, y: y - pr * 0.10),
                          control1: CGPoint(x: -pr * 0.2 + drift, y: y + pr * 0.12),
                          control2: CGPoint(x: pr * 0.25 + drift, y: y - pr * 0.18))
            planetCtx.stroke(band,
                             with: .color(Color.bubbleCompanion.opacity(
                                    min(0.16 + Double(mid) * 0.3, 1))),
                             style: StrokeStyle(lineWidth: pr * 0.09 * (1 + CGFloat(mid) * 0.5),
                                                lineCap: .round))
        }
        planetCtx.fill(Path(ellipseIn: CGRect(x: pr * 0.28, y: -pr * 0.30,
                                              width: pr * 0.34, height: pr * 0.34)),
                       with: .color(Color.bubbleLegend.opacity(0.55)))
        planetCtx.fill(Path(ellipseIn: CGRect(x: -pr * 0.55, y: -pr * 0.05,
                                              width: pr * 0.22, height: pr * 0.22)),
                       with: .color(Color.bubbleLegend.opacity(0.55)))

        // ── moon on the blended orbit ──
        let mi = Int(((s.moonAngle.truncatingRemainder(dividingBy: 2 * .pi)
                       + 2 * .pi).truncatingRemainder(dividingBy: 2 * .pi))
                     / (2 * .pi) * Float(n - 1))
        let mc = s.orbitPoint(s.moonAngle, tilt: tilt, squash: squash)
        let ml = center + SIMD2(f.waveL[mi], f.waveR[mi])
            * CosmosSceneState.orbitR * 0.85
        let mp = P(mc + (ml - mc) * s.lissAmt)
        let mr = S * CGFloat(CosmosSceneState.moonR) * (1 + 0.10 * CGFloat(f.level))
        var moonCtx = ctx
        moonCtx.clip(to: Path(ellipseIn: CGRect(x: mp.x - mr, y: mp.y - mr,
                                                width: mr * 2, height: mr * 2)))
        moonCtx.fill(Path(CGRect(x: mp.x - mr, y: mp.y - mr,
                                 width: mr * 2, height: mr * 2)),
                     with: .radialGradient(
                        Gradient(colors: [Color.bubbleCompanion,
                                          Color.bubbleSecondaryInk]),
                        center: CGPoint(x: mp.x - mr * 0.4, y: mp.y + mr * 0.4),
                        startRadius: 0, endRadius: mr * 2.2))
        moonCtx.fill(Path(ellipseIn: CGRect(x: mp.x - mr * 0.15, y: mp.y - mr * 0.3,
                                            width: mr * 0.5, height: mr * 0.5)),
                     with: .color(Color.bubbleLegend.opacity(0.5)))

        // ── shooting star ──
        if s.shootTTL > 0 {
            let t = max(s.shootTTL / 0.7, 0)
            let head = P([s.shootX, s.shootY])
            let tail = P([s.shootX - s.shootVX * 0.16,
                          s.shootY - s.shootVY * 0.16])
            var streak = Path()
            streak.move(to: tail); streak.addLine(to: head)
            ctx.stroke(streak, with: .color(Ui.accent.opacity(Double(t * t) * 0.35)),
                       style: StrokeStyle(lineWidth: S * 0.012, lineCap: .round))
            var tip = Path()
            tip.move(to: P([s.shootX - s.shootVX * 0.07,
                            s.shootY - s.shootVY * 0.07]))
            tip.addLine(to: head)
            ctx.stroke(tip, with: .color(Ui.accent.opacity(Double(t * t))),
                       style: StrokeStyle(lineWidth: S * 0.005, lineCap: .round))
        }
    }
}
