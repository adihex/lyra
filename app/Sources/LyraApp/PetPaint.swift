import AppKit
import CoreGraphics

/// Per-body painter for the desktop pet — one CGContext draw per panel,
/// transparent background, make_icon.swift geometry + colors scaled to
/// the local rect. Pure function of scene + fx so `setNeedsDisplay`
/// redraws stay re-entrant.
enum PetPaint {
    /// Per-body transform/effects driven by the pet driver. The band
    /// means CosmosSceneState doesn't store ride along here.
    struct FX {
        var scale: CGFloat = 1        // pop spring 0.2 → 1.15 → 1.0
        var alpha: CGFloat = 1        // pop/recall fade
        var dim: CGFloat = 0          // sleep dimness 0…1 (~30%)
        var spin: CGFloat = 0         // logo tumble angle
        var dashDir = CGVector.zero   // clip comet-dash direction
        var dashT: CGFloat = 0        // remaining TTL, of 0.7
        var hi: CGFloat = 0           // bands[48…63] mean → twinkle aura
        var mid: CGFloat = 0          // bands[21…47] mean → stripes
        var glint: CGFloat = 0        // max(peakL, peakR) → ring glint
    }

    static func draw(_ ctx: CGContext, _ r: CGRect, _ body: CosmosScene.Body,
                     _ s: CosmosScene, fx: FX) {
        ctx.saveGState()
        ctx.setAlpha(fx.alpha * (1 - 0.3 * fx.dim)) // sleep dims 30%
        let c = CGPoint(x: r.midX, y: r.midY)
        ctx.translateBy(x: c.x, y: c.y)
        ctx.scaleBy(x: fx.scale, y: fx.scale)
        ctx.translateBy(x: -c.x, y: -c.y)

        if fx.dashT > 0 { drawDash(ctx, r, fx) }
        drawAura(ctx, r, body, s, fx)

        switch body {
        case .saturn: drawSaturn(ctx, r, s, fx)
        case .moon: drawMoon(ctx, r, s)
        case .logo: drawLogo(ctx, r, s, spin: fx.spin)
        }
        ctx.restoreGState()
    }

    // ── palette (make_icon.swift / Ui tokens) ──
    private static func col(_ r: CGFloat, _ g: CGFloat, _ b: CGFloat,
                            _ a: CGFloat = 1) -> CGColor {
        CGColor(srgbRed: r, green: g, blue: b, alpha: a)
    }

    /// Clip comet dash — terracotta streak trailing behind the body.
    private static func drawDash(_ ctx: CGContext, _ r: CGRect, _ fx: FX) {
        let c = CGPoint(x: r.midX, y: r.midY)
        let s = min(r.width, r.height)
        let a = 0.4 * (fx.dashT / 0.7)
        ctx.setStrokeColor(col(0.78, 0.45, 0.28, a))
        ctx.setLineCap(.round)
        for (len, w) in [(s * 0.55, s * 0.05), (s * 0.32, s * 0.028)] as [(CGFloat, CGFloat)] {
            ctx.setLineWidth(w)
            ctx.move(to: c)
            ctx.addLine(to: CGPoint(x: c.x - fx.dashDir.dx * len,
                                    y: c.y - fx.dashDir.dy * len))
            ctx.strokePath()
        }
    }

    /// Local star twinkle — each body carries 2–4 sparks, alpha off the
    /// high bands (bands[48…63] mean).
    private static func drawAura(_ ctx: CGContext, _ r: CGRect,
                                 _ body: CosmosScene.Body, _ s: CosmosScene,
                                 _ fx: FX) {
        guard fx.hi > 0.02 else { return }
        let c = CGPoint(x: r.midX, y: r.midY)
        let n = body == .saturn ? 4 : 2
        let rad = min(r.width, r.height) * 0.46
        for i in 0..<n {
            let th = CGFloat(i) / CGFloat(n) * 2 * .pi + 0.7
            let p = CGPoint(x: c.x + rad * cos(th), y: c.y + rad * sin(th))
            let a = fx.hi * (0.15 + 0.85 * CGFloat(hashNoise(s.state.tick / 16, 40 + i * 13)))
            let sz = 1.0 + 1.6 * CGFloat(hashNoise(s.state.tick / 16, 80 + i * 7))
            twinkle(ctx, p, sz, a)
        }
    }

    private static func twinkle(_ ctx: CGContext, _ p: CGPoint,
                                _ r: CGFloat, _ a: CGFloat) {
        ctx.setFillColor(col(0.9, 0.97, 1, a * 0.9))
        ctx.fillEllipse(in: CGRect(x: p.x - r * 0.35, y: p.y - r * 0.35,
                                   width: r * 0.7, height: r * 0.7))
        ctx.setStrokeColor(col(0.9, 0.97, 1, a * 0.55))
        ctx.setLineWidth(max(0.6, r * 0.16))
        ctx.setLineCap(.round)
        ctx.move(to: CGPoint(x: p.x - r, y: p.y))
        ctx.addLine(to: CGPoint(x: p.x + r, y: p.y))
        ctx.move(to: CGPoint(x: p.x, y: p.y - r))
        ctx.addLine(to: CGPoint(x: p.x, y: p.y + r))
        ctx.strokePath()
    }

    // ── Saturn: indigo gas giant + tilted ring (bass kicks flip it) ──
    private static func drawSaturn(_ ctx: CGContext, _ r: CGRect,
                                   _ s: CosmosScene, _ fx: FX) {
        let c = CGPoint(x: r.midX, y: r.midY)
        let S = min(r.width, r.height)
        let pr = S * 0.26
        let tilt = -0.30 + CGFloat(s.state.orbitShape().tilt)

        // mint glow behind — eased by the scene's glow channel
        let glow = CGGradient(
            colorsSpace: CGColorSpaceCreateDeviceRGB(),
            colors: [col(0.25, 0.85, 0.75, CGFloat(s.state.glow)),
                     col(0.25, 0.85, 0.75, 0)] as CFArray,
            locations: [0, 1])!
        ctx.drawRadialGradient(glow, startCenter: c, startRadius: 0,
                               endCenter: c, endRadius: S * 0.5,
                               options: [.drawsAfterEndLocation])

        ctx.saveGState()
        // beat spring → bob/squash (pulse can dip slightly negative)
        let pulse = CGFloat(s.state.pulse)
        ctx.translateBy(x: c.x, y: c.y)
        ctx.scaleBy(x: 1 + 0.05 * pulse, y: 1 - 0.12 * pulse)
        ctx.translateBy(x: -c.x, y: -c.y)

        // ring, back half (top of the tilted plane, behind the planet)
        let rr = CGRect(x: -pr * 1.7, y: -pr * 0.52,
                        width: pr * 3.4, height: pr * 1.04)
        ctx.saveGState()
        ctx.translateBy(x: c.x, y: c.y)
        ctx.rotate(by: tilt)
        ctx.clip(to: CGRect(x: -pr * 2.2, y: 0, width: pr * 4.4, height: pr * 2.2))
        ctx.setStrokeColor(col(0.70, 0.82, 1, 0.45))
        ctx.setLineWidth(pr * 0.18)
        ctx.strokeEllipse(in: rr)
        ctx.setStrokeColor(col(0.85, 0.92, 1, 0.35))
        ctx.setLineWidth(pr * 0.07)
        ctx.strokeEllipse(in: rr.insetBy(dx: -pr * 0.10, dy: -pr * 0.10))
        ctx.restoreGState()

        // planet body — gradient + lazy stripes + friendly craters
        ctx.saveGState()
        let prect = CGRect(x: c.x - pr, y: c.y - pr, width: pr * 2, height: pr * 2)
        ctx.addEllipse(in: prect)
        ctx.clip()
        let planet = CGGradient(
            colorsSpace: CGColorSpaceCreateDeviceRGB(),
            colors: [col(0.45, 0.55, 0.95), col(0.16, 0.22, 0.55),
                     col(0.07, 0.10, 0.28)] as CFArray,
            locations: [0, 0.55, 1])!
        ctx.drawRadialGradient(
            planet,
            startCenter: CGPoint(x: c.x - pr * 0.45, y: c.y + pr * 0.5),
            startRadius: pr * 0.1,
            endCenter: c, endRadius: pr * 1.6,
            options: [.drawsAfterEndLocation])
        ctx.setStrokeColor(col(0.65, 0.75, 1, 0.16 + 0.15 * fx.mid))
        ctx.setLineWidth(pr * 0.09)
        ctx.setLineCap(.round)
        let drift = pr * 0.15 * sin(CGFloat(s.state.stripePhase) * 6)
        for k in [-1, 0, 1] as [CGFloat] {
            let y = c.y + k * pr * 0.42
            ctx.move(to: CGPoint(x: c.x - pr * 0.75, y: y))
            ctx.addCurve(to: CGPoint(x: c.x + pr * 0.75, y: y - pr * 0.10),
                         control1: CGPoint(x: c.x - pr * 0.2 + drift,
                                           y: y + pr * 0.12),
                         control2: CGPoint(x: c.x + pr * 0.25 + drift,
                                           y: y - pr * 0.18))
            ctx.strokePath()
        }
        ctx.setFillColor(col(0.10, 0.16, 0.42, 0.55))
        ctx.fillEllipse(in: CGRect(x: c.x + pr * 0.28, y: c.y - pr * 0.30,
                                   width: pr * 0.34, height: pr * 0.34))
        ctx.fillEllipse(in: CGRect(x: c.x - pr * 0.55, y: c.y - pr * 0.05,
                                   width: pr * 0.22, height: pr * 0.22))
        ctx.restoreGState()

        // ring, front half — crosses below the planet for the wrap look
        ctx.saveGState()
        ctx.translateBy(x: c.x, y: c.y)
        ctx.rotate(by: tilt)
        ctx.clip(to: CGRect(x: -pr * 2.2, y: -pr * 2.2, width: pr * 4.4, height: pr * 2.2))
        ctx.setStrokeColor(col(0.70, 0.82, 1, 0.55))
        ctx.setLineWidth(pr * 0.18)
        ctx.strokeEllipse(in: rr)
        ctx.setStrokeColor(col(0.85, 0.92, 1, 0.4))
        ctx.setLineWidth(pr * 0.07)
        ctx.strokeEllipse(in: rr.insetBy(dx: -pr * 0.10, dy: -pr * 0.10))
        ctx.restoreGState()

        // glint dot riding the front of the ring (PPM ballistics
        // already applied upstream in the peak channels)
        if fx.glint > 0.05 {
            let th = CGFloat(s.state.tick % 2048) / 2048 * .pi + .pi // param on front half
            let g = CGPoint(x: c.x + cos(tilt) * (pr * 1.7) * cos(th)
                                 - sin(tilt) * (pr * 0.52) * sin(th),
                            y: c.y + sin(tilt) * (pr * 1.7) * cos(th)
                                 + cos(tilt) * (pr * 0.52) * sin(th))
            ctx.setFillColor(col(1, 1, 1, fx.glint * 0.9))
            ctx.fillEllipse(in: CGRect(x: g.x - 1.8, y: g.y - 1.8,
                                       width: 3.6, height: 3.6))
        }
        ctx.restoreGState()
    }

    // ── Moon: mint gradient + one spot ──
    private static func drawMoon(_ ctx: CGContext, _ r: CGRect,
                                 _ s: CosmosScene) {
        let c = CGPoint(x: r.midX, y: r.midY)
        let mr = min(r.width, r.height) * 0.32
        let bob = 1 - 0.06 * CGFloat(s.state.pulse)
        ctx.saveGState()
        ctx.translateBy(x: c.x, y: c.y)
        ctx.scaleBy(x: bob, y: bob)
        ctx.translateBy(x: -c.x, y: -c.y)
        ctx.addEllipse(in: CGRect(x: c.x - mr, y: c.y - mr,
                                  width: mr * 2, height: mr * 2))
        ctx.clip()
        let g = CGGradient(
            colorsSpace: CGColorSpaceCreateDeviceRGB(),
            colors: [col(0.85, 1.0, 0.95), col(0.45, 0.75, 0.72)] as CFArray,
            locations: [0, 1])!
        ctx.drawRadialGradient(
            g, startCenter: CGPoint(x: c.x - mr * 0.4, y: c.y + mr * 0.4),
            startRadius: 0, endCenter: c, endRadius: mr * 2.2,
            options: [.drawsAfterEndLocation])
        ctx.setFillColor(col(0.3, 0.55, 0.55, 0.5))
        ctx.fillEllipse(in: CGRect(x: c.x - mr * 0.15, y: c.y - mr * 0.3,
                                   width: mr * 0.5, height: mr * 0.5))
        ctx.restoreGState()
    }

    // ── Logo: the playful one — a terracotta note that tumbles ──
    private static func drawLogo(_ ctx: CGContext, _ r: CGRect,
                                 _ s: CosmosScene, spin: CGFloat) {
        let c = CGPoint(x: r.midX, y: r.midY)
        let S = min(r.width, r.height)
        ctx.saveGState()
        ctx.translateBy(x: c.x, y: c.y)
        ctx.rotate(by: sin(spin) * 0.55)          // rocking tumble
        let pulse = CGFloat(s.state.pulse)
        ctx.scaleBy(x: 1 + 0.05 * pulse, y: 1 - 0.08 * pulse)
        ctx.setFillColor(col(0.78, 0.45, 0.28, 0.95)) // Ui.accent
        // double eighth note: two heads, two stems, one beam
        for hx in [-S * 0.16, S * 0.14] as [CGFloat] {
            ctx.saveGState()
            ctx.translateBy(x: hx, y: -S * 0.22)
            ctx.rotate(by: -0.35)
            ctx.fillEllipse(in: CGRect(x: -S * 0.15, y: -S * 0.11,
                                       width: S * 0.30, height: S * 0.22))
            ctx.restoreGState()
        }
        ctx.fill(CGRect(x: S * 0.02, y: -S * 0.22,
                        width: S * 0.05, height: S * 0.52))
        ctx.fill(CGRect(x: S * 0.32, y: -S * 0.28,
                        width: S * 0.05, height: S * 0.58))
        ctx.saveGState()
        ctx.translateBy(x: S * 0.02, y: S * 0.30)
        ctx.rotate(by: -0.18)
        ctx.fill(CGRect(x: 0, y: -S * 0.09, width: S * 0.36, height: S * 0.12))
        ctx.restoreGState()
        ctx.restoreGState()
    }
}
