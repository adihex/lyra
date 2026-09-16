import AppKit
import CoreGraphics

/// CGContext painter for the Cosmos scene — the dock tile's renderer.
/// This is scripts/make_icon.swift's drawing plus drive offsets, in
/// normalized unit coords (y up) scaled into the centered squircle at
/// ~0.83 of the tile — macOS does NOT re-clip a custom contentView, so
/// the artwork conforms to the asset icon's inset instead of going
/// edge-to-edge. Pure function of (scene, frame): safe for the Dock's
/// resize/magnification redraws — all mutation lives in scene.drive.
///
/// Bodies in `scene.away` are skipped — when the desktop pet pops out,
/// the icon literally empties to sky + faint orbit (desktop-pet.md §3).
enum CosmosPaintCG {
    private static func col(_ r: CGFloat, _ g: CGFloat, _ b: CGFloat,
                            _ a: CGFloat = 1) -> CGColor {
        CGColor(srgbRed: r, green: g, blue: b, alpha: a)
    }

    // gradients/colors are immutable — shared across every frame
    private static let bgGrad = CGGradient(
        colorsSpace: CGColorSpaceCreateDeviceRGB(),
        colors: [col(0.14, 0.10, 0.36), col(0.02, 0.03, 0.09)] as CFArray,
        locations: [0, 1])!
    private static let glowGrad = CGGradient(
        colorsSpace: CGColorSpaceCreateDeviceRGB(),
        colors: [col(0.25, 0.85, 0.75, 0.28), col(0.25, 0.85, 0.75, 0)] as CFArray,
        locations: [0, 1])!
    private static let planetGrad = CGGradient(
        colorsSpace: CGColorSpaceCreateDeviceRGB(),
        colors: [col(0.45, 0.55, 0.95), col(0.16, 0.22, 0.55),
                 col(0.07, 0.10, 0.28)] as CFArray,
        locations: [0, 0.55, 1])!
    private static let moonGrad = CGGradient(
        colorsSpace: CGColorSpaceCreateDeviceRGB(),
        colors: [col(0.85, 1.0, 0.95), col(0.45, 0.75, 0.72)] as CFArray,
        locations: [0, 1])!
    private static let starWhite = col(0.85, 0.9, 1)
    private static let starMint = col(0.6, 0.95, 0.85)
    private static let plusCol = col(0.9, 0.97, 1)
    private static let orbitCol = col(0.7, 0.85, 1)
    private static let stripeCol = col(0.65, 0.75, 1)
    private static let craterCol = col(0.10, 0.16, 0.42, 0.55)
    private static let moonSpotCol = col(0.3, 0.55, 0.55, 0.5)
    private static let accentCol = col(0.780, 0.447, 0.278) // Ui.accent

    static func draw(_ ctx: CGContext, in bounds: CGRect,
                     scene: CosmosScene, frame f: VizFrame) {
        let s = scene.state
        let saturnAway = scene.away.contains(.saturn)
        let moonAway = scene.away.contains(.moon)
        let boundsSide = min(bounds.width, bounds.height)
        guard boundsSide > 8 else { return }
        let S = boundsSide * 0.83
        let tile = CGRect(x: bounds.midX - S / 2, y: bounds.midY - S / 2,
                          width: S, height: S)
        let devPx = abs(ctx.ctm.a) // pt→px, for the <1px detail gate
        func P(_ v: SIMD2<Float>) -> CGPoint {
            CGPoint(x: tile.minX + CGFloat(v.x) * S,
                    y: tile.minY + CGFloat(v.y) * S)
        }

        // continuous-corner squircle ≈ roundrect at 22.5% — the icon's
        // own mask shape, applied by hand since tiles get none.
        ctx.saveGState()
        ctx.addPath(CGPath(roundedRect: tile, cornerWidth: S * 0.225,
                           cornerHeight: S * 0.225, transform: nil))
        ctx.clip()

        // ── background: base fill then the indigo diagonal ──
        ctx.setFillColor(col(0.02, 0.03, 0.09))
        ctx.fill(tile)
        ctx.drawLinearGradient(bgGrad, start: P([0.3, 1]),
                               end: P([0.7, 0]), options: [])

        // ── star field: alpha = base + hi-band flicker; <1px dropped ──
        let hi = s.hi
        for i in 0..<s.stars.count {
            let star = s.stars[i]
            let d = CGFloat(star.r) * S / 1024
            if d * devPx < 1 { continue }
            ctx.setFillColor(star.mint ? starMint : starWhite)
            ctx.setAlpha(min(CGFloat(star.alpha)
                             + CGFloat(hi * hashNoise(s.tick / 6, i)) * 0.5, 1))
            ctx.fillEllipse(in: CGRect(x: tile.minX + CGFloat(star.x) * S,
                                       y: tile.minY + CGFloat(star.y) * S,
                                       width: d, height: d))
        }
        ctx.setAlpha(1)
        // the two twinkle pluses, same as the icon
        for (sx, sy, sr) in [(0.78, 0.82, 0.016), (0.2, 0.68, 0.012)]
            as [(CGFloat, CGFloat, CGFloat)] {
            let cx = tile.minX + sx * S, cy = tile.minY + sy * S, r = sr * S
            ctx.setFillColor(plusCol)
            ctx.setAlpha(0.9)
            ctx.fillEllipse(in: CGRect(x: cx - r * 0.35, y: cy - r * 0.35,
                                       width: r * 0.7, height: r * 0.7))
            ctx.setStrokeColor(plusCol)
            ctx.setAlpha(0.55)
            ctx.setLineWidth(r * 0.16)
            ctx.setLineCap(.round)
            ctx.move(to: CGPoint(x: cx - r, y: cy))
            ctx.addLine(to: CGPoint(x: cx + r, y: cy))
            ctx.move(to: CGPoint(x: cx, y: cy - r))
            ctx.addLine(to: CGPoint(x: cx, y: cy + r))
            ctx.strokePath()
        }
        ctx.setAlpha(1)

        let mid = s.mid
        // Saturn gone → the orbit falls back to the icon's flat hairline
        // (no ring plane, no glint, no glow — those belong to the planet)
        let (tilt, squash) = saturnAway ? (Float(0), Float(1)) : s.orbitShape()
        let center = CosmosSceneState.planet
        let bobY = saturnAway ? 0 : max(s.pulse, 0) * 0.020
        let pc = P(center + SIMD2(0, bobY))

        // ── mint glow behind the planet — breathes with mids ──
        if !saturnAway {
            ctx.saveGState()
            ctx.setAlpha(CGFloat(s.glow / 0.28))
            ctx.drawRadialGradient(
                glowGrad, startCenter: pc, startRadius: 0,
                endCenter: pc, endRadius: S * 0.44,
                options: [.drawsAfterEndLocation])
            ctx.restoreGState()
        }

        // ── orbit: the icon's hairline circle tilts/squashes into a
        //    ring plane off bass ──
        let orbitPx = S * CGFloat(CosmosSceneState.orbitR)
        ctx.saveGState()
        ctx.translateBy(x: P(center).x, y: P(center).y)
        ctx.rotate(by: CGFloat(tilt))
        ctx.scaleBy(x: 1, y: CGFloat(squash))
        ctx.setStrokeColor(orbitCol)
        ctx.setAlpha(saturnAway ? 0.16 : min(0.16 + 0.30 * CGFloat(f.bass), 1))
        ctx.setLineWidth(S * 0.003)
        ctx.strokeEllipse(in: CGRect(x: -orbitPx, y: -orbitPx,
                                     width: orbitPx * 2, height: orbitPx * 2))
        ctx.restoreGState()

        // ring glint — one accent dot opposite the moon, PPM-ballistic
        let glintA = s.glint
        if !saturnAway && glintA > 0.05 {
            let gp = P(s.orbitPoint(s.moonAngle + .pi,
                                    tilt: tilt, squash: squash))
            let gr = S * 0.014
            ctx.setFillColor(accentCol)
            ctx.setAlpha(CGFloat(glintA) * 0.9)
            ctx.fillEllipse(in: CGRect(x: gp.x - gr, y: gp.y - gr,
                                       width: gr * 2, height: gr * 2))
            ctx.setAlpha(1)
        }

        // ── planet: bob + volume-preserving squash on the beat ──
        if !saturnAway {
            let pr = S * CGFloat(CosmosSceneState.planetR)
            let sx = 1 + 0.05 * s.pulse, sy = 1 - 0.07 * s.pulse
            ctx.saveGState()
            ctx.translateBy(x: pc.x, y: pc.y)
            ctx.scaleBy(x: CGFloat(sx), y: CGFloat(sy))
            ctx.addEllipse(in: CGRect(x: -pr, y: -pr, width: pr * 2, height: pr * 2))
            ctx.clip()
            ctx.drawRadialGradient(
                planetGrad,
                startCenter: CGPoint(x: -pr * 0.45, y: pr * 0.5),
                startRadius: pr * 0.1,
                endCenter: .zero, endRadius: pr * 1.6,
                options: [.drawsAfterEndLocation])
            // latitude bands — drift on mids, same three curves as the icon
            ctx.setStrokeColor(stripeCol)
            ctx.setAlpha(min(0.16 + CGFloat(mid) * 0.3, 1))
            ctx.setLineWidth(pr * 0.09 * (1 + CGFloat(mid) * 0.5))
            ctx.setLineCap(.round)
            for k in [-1, 0, 1] as [CGFloat] {
                let y = k * pr * 0.42
                let drift = CGFloat(sin(s.stripePhase + Float(k) * 1.7)) * pr * 0.12
                ctx.move(to: CGPoint(x: -pr * 0.75 + drift, y: y))
                ctx.addCurve(to: CGPoint(x: pr * 0.75 + drift, y: y - pr * 0.10),
                             control1: CGPoint(x: -pr * 0.2 + drift, y: y + pr * 0.12),
                             control2: CGPoint(x: pr * 0.25 + drift, y: y - pr * 0.18))
                ctx.strokePath()
            }
            // crater spots drop out below 64pt — icon-design detail gate
            if S >= 64 {
                ctx.setFillColor(craterCol)
                ctx.setAlpha(1)
                ctx.fillEllipse(in: CGRect(x: pr * 0.28, y: -pr * 0.30,
                                           width: pr * 0.34, height: pr * 0.34))
                ctx.fillEllipse(in: CGRect(x: -pr * 0.55, y: -pr * 0.05,
                                           width: pr * 0.22, height: pr * 0.22))
            }
            ctx.restoreGState()
        }

        // ── the moon — rides the (possibly squashed) orbit ──
        if !moonAway {
            let mp = P(s.orbitPoint(s.moonAngle, tilt: tilt, squash: squash))
            let mr = S * CGFloat(CosmosSceneState.moonR) * (1 + 0.10 * CGFloat(f.level))
            ctx.saveGState()
            ctx.addEllipse(in: CGRect(x: mp.x - mr, y: mp.y - mr,
                                      width: mr * 2, height: mr * 2))
            ctx.clip()
            ctx.drawRadialGradient(
                moonGrad,
                startCenter: CGPoint(x: mp.x - mr * 0.4, y: mp.y + mr * 0.4),
                startRadius: 0,
                endCenter: mp, endRadius: mr * 2.2,
                options: [.drawsAfterEndLocation])
            ctx.setFillColor(moonSpotCol)
            ctx.fillEllipse(in: CGRect(x: mp.x - mr * 0.15, y: mp.y - mr * 0.3,
                                       width: mr * 0.5, height: mr * 0.5))
            ctx.restoreGState()
        }

        // ── shooting star — the one loud moment (clip edge) ──
        if s.shootTTL > 0 {
            let t = max(s.shootTTL / 0.7, 0)
            let head = P([s.shootX, s.shootY])
            let tail = P([s.shootX - s.shootVX * 0.16,
                          s.shootY - s.shootVY * 0.16])
            ctx.setStrokeColor(accentCol)
            ctx.setLineCap(.round)
            ctx.setAlpha(CGFloat(t * t * 0.35))
            ctx.setLineWidth(S * 0.012)
            ctx.move(to: tail); ctx.addLine(to: head); ctx.strokePath()
            ctx.setAlpha(CGFloat(t * t))
            ctx.setLineWidth(S * 0.005)
            let mid = P([s.shootX - s.shootVX * 0.07,
                         s.shootY - s.shootVY * 0.07])
            ctx.move(to: mid); ctx.addLine(to: head); ctx.strokePath()
            ctx.setAlpha(1)
        }

        ctx.restoreGState()
    }
}
