import SwiftUI

/// Particle/ambient renderers — vis_scatter / vis_flame / vis_pulse /
/// vis_matrix / vis_binary / vis_sakura / vis_firework / vis_bubbles /
/// vis_logo / vis_firefly / vis_geyser ported to Canvas geometry.
/// Palette stays muted: accent/indigo/mint at partial opacity, no neon.
extension VizDraw {

    /// Scatter — twinkling dot field; per-dot density ∝ band² with a
    /// gravity bias toward the bottom (cliamp scatterHash thresholds).
    static func scatter(_ ctx: inout GraphicsContext, _ size: CGSize,
                        _ f: VizFrame, _ st: VizState) {
        let colW: CGFloat = 6, rowH: CGFloat = 6
        let cols = Int(size.width / colW), rows = Int(size.height / rowH)
        guard cols > 0, rows > 0 else { return }
        var tiers = [Path(), Path(), Path()]
        for c in 0..<cols {
            let level = sampleBand(f.bands, pos: Float(c) / Float(max(cols - 1, 1)) * 63)
            for r in 0..<rows {
                let heightFac = 0.5 + 0.5 * Float(r) / Float(max(rows - 1, 1))
                if hashNoise(st.uiTick + UInt64(r * 3 + c), c) < level * level * heightFac {
                    let y = size.height - CGFloat(r) * rowH - 3
                    let t = Float(r) / Float(rows)
                    tiers[t >= 0.6 ? 2 : (t >= 0.3 ? 1 : 0)]
                        .addRect(CGRect(x: CGFloat(c) * colW, y: y, width: 2, height: 2))
                }
            }
        }
        ctx.fill(tiers[0], with: .color(Ui.mint.opacity(0.7)))
        ctx.fill(tiers[1], with: .color(Ui.indigo.opacity(0.7)))
        ctx.fill(tiers[2], with: .color(Ui.accent.opacity(0.8)))
    }

    /// Flame — doom-fire heat field (state tick) rendered as cells;
    /// hot accent core, terracotta body, wispy stippled tips.
    static func flame(_ ctx: inout GraphicsContext, _ size: CGSize,
                      _ f: VizFrame, _ st: VizState) {
        let W = VizState.flameW, H = VizState.flameH
        let cw = size.width / CGFloat(W), ch = size.height / CGFloat(H)
        var core = Path(), body = Path(), tips = Path()
        for y in 0..<H {
            for x in 0..<W {
                let h = st.heat[y * W + x] // row 0 = bottom source
                guard h > 0.10 else { continue }
                // wispy tip stipple: low heat breaks into scattered cells
                if h < 0.25 && hashNoise(st.uiTick, y * W + x) > h * 4 { continue }
                let rect = CGRect(x: CGFloat(x) * cw,
                                  y: size.height - CGFloat(y + 1) * ch,
                                  width: cw, height: ch)
                if h >= 0.55 { core.addRect(rect) } else { body.addRect(rect) }
                if h < 0.25 { tips.addRect(rect) }
            }
        }
        ctx.fill(body, with: .color(Ui.accent.opacity(0.55)))
        ctx.fill(core, with: .color(Ui.accent.opacity(0.95)))
        ctx.fill(tips, with: .color(Ui.indigo.opacity(0.35)))
    }

    /// Pulse — pulsating disc: per-angle band energy deforms the radius,
    /// whole shape surges on level; shockwave ring on transients.
    static func pulse(_ ctx: inout GraphicsContext, _ size: CGSize,
                      _ f: VizFrame, _ st: VizState) {
        let cx = size.width / 2, cy = size.height / 2
        let maxR = min(cx, cy) - 4
        var avg: Float = 0
        for b in f.bands { avg += b }
        avg /= 64
        let breath = sin(Float(st.uiTick) * 0.05) * 0.02
        let rot = Float(st.uiTick) * (0.015 + avg * 0.04)
        var blob = Path()
        for i in 0..<64 {
            let a = Float(i) / 64 * .pi * 2 + rot
            let energy = sampleBand(f.bands, pos: Float(i))
            let blended = energy * 0.6 + avg * 0.4
            let r = CGFloat(maxR) * CGFloat(0.08 + breath + 0.92 * blended * blended)
            let pt = CGPoint(x: cx + cos(CGFloat(a)) * r,
                             y: cy + sin(CGFloat(a)) * r)
            i == 0 ? blob.move(to: pt) : blob.addLine(to: pt)
        }
        blob.closeSubpath()
        ctx.fill(blob, with: .radialGradient(
            Gradient(colors: [Ui.accent.opacity(0.9), Ui.indigo.opacity(0.55)]),
            center: CGPoint(x: cx, y: cy), startRadius: 0,
            endRadius: maxR))
        ctx.stroke(blob, with: .color(Ui.ink.opacity(0.7)), lineWidth: 1)
        // shockwave ring: expands + fades with the beat
        if f.beat > 0.05 {
            let phase = (Float(st.uiTick) * 0.10).truncatingRemainder(dividingBy: 1)
            let r = maxR * CGFloat(0.3 + 0.7 * phase)
            let strength = CGFloat(f.beat * f.beat) * (1 - CGFloat(phase * phase))
            ctx.stroke(Path(ellipseIn: CGRect(x: cx - r, y: cy - r,
                                              width: r * 2, height: r * 2)),
                       with: .color(Ui.mint.opacity(min(strength, 0.9))),
                       lineWidth: 1 + strength * 2)
        }
    }

    /// Matrix — katakana glyph rain; per-column fixed speed, trail with
    /// bright head, density gated by band energy (cliamp renderMatrix).
    static func matrix(_ ctx: inout GraphicsContext, _ size: CGSize,
                       _ f: VizFrame, _ st: VizState) {
        let glyphs = Array("ｦｧｨｩｪｫｬｭｮｯｰｱｲｳｴｵｶｷｸｹｺｻｼｽｾｿﾀﾁﾂﾃﾄ0123456789")
        let colW: CGFloat = 12, rowH: CGFloat = 11
        let cols = Int(size.width / colW), rows = Int(size.height / rowH)
        guard cols > 0, rows > 0 else { return }
        for c in 0..<cols {
            let energy = sampleBand(f.bands, pos: Float(c) / Float(max(cols - 1, 1)) * 63)
            // stable gate flips every ~20 ticks; energy opens more columns
            if hashNoise(st.uiTick / 20, c * 7) > energy * 1.5 + 0.1 { continue }
            let speed = 2 + c % 3
            let trail = 3 + (c / 7) % 3
            let cycle = rows + trail + 4
            let offset = Int(hashNoise(UInt64(c * 13), 1) * Float(cycle))
            let pos = (Int(st.uiTick) / speed + offset) % cycle
            for r in 0..<rows {
                let dist = pos - r
                guard dist >= 0 && dist <= trail else { continue }
                // glyph mutates slowly (~every 4 ticks)
                let gi = Int(hashNoise(st.uiTick / 4, c * 131 + r * 31)
                             * Float(glyphs.count - 1))
                let resolved = ctx.resolve(
                    Text(String(glyphs[gi]))
                        .font(.system(size: 10, weight: dist == 0 ? .bold : .regular,
                                      design: .monospaced))
                        .foregroundColor(dist == 0 ? Ui.accent
                                         : dist <= 2 ? Ui.mint : Ui.mint.opacity(0.4)))
                ctx.draw(resolved, at: CGPoint(x: CGFloat(c) * colW + colW / 2,
                                               y: CGFloat(r) * rowH + rowH / 2),
                         anchor: .center)
            }
        }
    }

    /// Binary — streaming 0/1 field; scroll speed + 1-density ∝ energy.
    static func binary(_ ctx: inout GraphicsContext, _ size: CGSize,
                       _ f: VizFrame, _ st: VizState) {
        let colW: CGFloat = 9, rowH: CGFloat = 10
        let cols = Int(size.width / colW), rows = Int(size.height / rowH)
        guard cols > 0, rows > 0 else { return }
        let oneHot = ctx.resolve(Text("1").font(.system(size: 9, weight: .bold,
                                                        design: .monospaced))
                                 .foregroundColor(Ui.accent))
        let one = ctx.resolve(Text("1").font(.system(size: 9, design: .monospaced))
                              .foregroundColor(Ui.indigo))
        let zero = ctx.resolve(Text("0").font(.system(size: 9, design: .monospaced))
                               .foregroundColor(Ui.inkSoft.opacity(0.35)))
        for c in 0..<cols {
            let energy = sampleBand(f.bands, pos: Float(c) / Float(max(cols - 1, 1)) * 63)
            let speed = max(1, 4 - Int(energy * 3))
            let scroll = Int(st.uiTick) / speed
            for r in 0..<rows {
                let h = hashNoise(0, c * 197 + (r + scroll) * 31)
                let isOne = h < energy * 0.6 + 0.15
                let t = isOne ? (energy > 0.4 ? oneHot : one) : zero
                ctx.draw(t, at: CGPoint(x: CGFloat(c) * colW + colW / 2,
                                        y: CGFloat(r) * rowH + rowH / 2),
                         anchor: .center)
            }
        }
    }

    /// Sakura — petals from the state pool; teardrop ellipses rotated by
    /// per-petal phase, terracotta-muted (no pink in this palette).
    static func sakura(_ ctx: inout GraphicsContext, _ size: CGSize,
                       _ f: VizFrame, _ st: VizState) {
        for p in st.petal.parts where p.alive {
            let x = CGFloat(p.x) * size.width
            let y = CGFloat(p.y) * size.height
            let s = CGFloat(2 + p.size * 2)
            let rot = CGFloat(sin(Float(st.uiTick) * 0.02 + p.seed * 6.28)) * 0.8
            var path = Path(ellipseIn: CGRect(x: -s / 2, y: -s / 4,
                                              width: s, height: s / 2))
            path = path.applying(CGAffineTransform(rotationAngle: rot)
                                 .concatenating(CGAffineTransform(translationX: x, y: y)))
            ctx.fill(path, with: .color(Ui.accent.opacity(0.55 + 0.3 * CGFloat(p.seed))))
        }
        _ = f
    }

    /// Firework — state-pool bursts: radial particles + gravity + fade.
    static func firework(_ ctx: inout GraphicsContext, _ size: CGSize,
                         _ f: VizFrame, _ st: VizState) {
        var p = Path()
        for pt in st.burst.parts where pt.alive {
            let fade = 1 - pt.life / pt.maxLife
            let x = CGFloat(pt.x) * size.width
            let y = CGFloat(pt.y) * size.height
            let s = CGFloat(pt.size) * (0.5 + CGFloat(fade))
            p.addRect(CGRect(x: x - s / 2, y: y - s / 2, width: s, height: s))
        }
        ctx.fill(p, with: .color(Ui.accent.opacity(0.85)))
        _ = f
    }

    /// Bubbles — hollow rising rings w/ specular nick (state pool).
    static func bubbles(_ ctx: inout GraphicsContext, _ size: CGSize,
                        _ f: VizFrame, _ st: VizState) {
        for p in st.bubble.parts where p.alive {
            let x = CGFloat(p.x) * size.width
            let y = CGFloat(p.y) * size.height
            let r = CGFloat(p.size)
            // pop fade near the top
            let fade = min(max(y / (size.height * 0.15), 0.15), 1)
            let ring = Path(ellipseIn: CGRect(x: x - r, y: y - r,
                                              width: r * 2, height: r * 2))
            ctx.stroke(ring, with: .color(Ui.indigo.opacity(0.6 * fade)),
                       lineWidth: 1)
            // specular highlight, upper-left
            ctx.fill(Path(ellipseIn: CGRect(x: x - r * 0.45 - 1, y: y - r * 0.45 - 1,
                                            width: 2.5, height: 2.5)),
                     with: .color(Ui.mint.opacity(0.8 * fade)))
        }
        _ = f
    }

    /// Logo — LYRA block wordmark; per-block visibility gated by the
    /// letter's band energy (cliamp logoGlyphs → 5×7 bitmaps, LYRA set).
    static func logo(_ ctx: inout GraphicsContext, _ size: CGSize,
                     _ f: VizFrame, _ st: VizState) {
        // 5×7 bitmaps, bit 4 = leftmost pixel
        let glyphs: [[UInt8]] = [
            [0x10, 0x10, 0x10, 0x10, 0x10, 0x10, 0x1F], // L
            [0x11, 0x11, 0x0A, 0x04, 0x04, 0x04, 0x04], // Y
            [0x1E, 0x11, 0x11, 0x1E, 0x14, 0x12, 0x11], // R
            [0x0E, 0x11, 0x11, 0x1F, 0x11, 0x11, 0x11], // A
        ]
        let bandFor = [4, 20, 36, 52] // spread letters across the spectrum
        let totalW = 4 * 5 + 3 * 2 // 26 pixel cols
        let cell = min(size.width / CGFloat(totalW + 4),
                       size.height / CGFloat(7 + 4))
        let ox = (size.width - CGFloat(totalW) * cell) / 2
        let oy = (size.height - 7 * cell) / 2
        var p = Path()
        for (li, g) in glyphs.enumerated() {
            let energy = f.bands[bandFor[li]]
            let wob = sin(Float(st.uiTick) * 0.06 + Float(li) * 0.9) * 1.5
            let bounce = CGFloat(energy) * cell * 1.2 + CGFloat(wob)
            let fill = energy * energy * 0.75 + 0.15
            for py in 0..<7 {
                for px in 0..<5 where (g[py] & UInt8(1 << (4 - px))) != 0 {
                    if hashNoise(st.uiTick, li * 977 + py * 31 + px) > fill { continue }
                    let x = ox + CGFloat(li * 7 + px) * cell
                    let y = oy + CGFloat(py) * cell - bounce
                    p.addRect(CGRect(x: x, y: y, width: cell - 1, height: cell - 1))
                }
            }
        }
        ctx.fill(p, with: .color(Ui.accent))
    }

    /// Firefly — Lissajous-wandering lights over a grass silhouette;
    /// high bands raise blink rate, bass tilts a side wind (stateless,
    /// like cliamp — positions are pure functions of tick).
    static func firefly(_ ctx: inout GraphicsContext, _ size: CGSize,
                        _ f: VizFrame, _ st: VizState) {
        let bass = bandAvg(f.bands, 0, 21), high = bandAvg(f.bands, 42, 64)
        // grass silhouette: ragged bottom edge
        var grass = Path()
        var x: CGFloat = 0
        while x < size.width {
            let h = 4 + 3 * sin(x * 0.41) + 2.5 * sin(x * 0.17 + 2.3)
            grass.addRect(CGRect(x: x, y: size.height - max(2, h),
                                 width: 3, height: max(2, h)))
            x += 3
        }
        ctx.fill(grass, with: .color(Ui.mint.opacity(0.35)))
        let t = Float(st.uiTick)
        let wind = bass * 14
        var bright = Path(), dim = Path()
        for i in 0..<26 {
            let seed = Float(i) * 22.46822519 + 11
            let fx = 0.012 + (seed.truncatingRemainder(dividingBy: 17)) / 3500
            let fy = 0.018 + (Float(Int(seed) >> 4 % 19)) / 2900
            let phx = (seed.truncatingRemainder(dividingBy: 1000)) / 1000 * .pi * 2
            let phy = (Float(Int(seed) >> 8 % 1000)) / 1000 * .pi * 2
            let bx = size.width / 2 + CGFloat(cos(t * fx + phx)) * (size.width - 24) * 0.45
            let by = size.height * 0.5 + CGFloat(sin(t * fy + phy)) * (size.height - 24) * 0.4
            let px = bx + CGFloat(wind) * CGFloat(sin(t * 0.02 + phx))
            let on = sin(t * 0.18 + Float(i) * 1.31) * 0.5 + 0.5 + high * 0.4 > 0.55
            if on {
                bright.addRect(CGRect(x: px - 1.5, y: by - 1.5, width: 3, height: 3))
                // glow cross
                dim.addRect(CGRect(x: px - 3.5, y: by - 0.5, width: 7, height: 1))
                dim.addRect(CGRect(x: px - 0.5, y: by - 3.5, width: 1, height: 7))
            } else {
                dim.addRect(CGRect(x: px - 0.5, y: by - 0.5, width: 1.5, height: 1.5))
            }
        }
        ctx.fill(dim, with: .color(Ui.mint.opacity(0.3)))
        ctx.fill(bright, with: .color(Ui.accent.opacity(0.95)))
    }

    /// Geyser — bass fountain: steady drizzle + transient jets (state pool).
    static func geyser(_ ctx: inout GraphicsContext, _ size: CGSize,
                       _ f: VizFrame, _ st: VizState) {
        var tiers = [Path(), Path(), Path()]
        for p in st.geyser.parts where p.alive {
            let x = CGFloat(p.x) * size.width
            let y = CGFloat(p.y) * size.height
            let s = CGFloat(p.size)
            tiers[Int(p.seed)].addRect(CGRect(x: x - s / 2, y: y - s / 2,
                                             width: s, height: s))
        }
        ctx.fill(tiers[0], with: .color(Ui.mint.opacity(0.7)))
        ctx.fill(tiers[1], with: .color(Ui.indigo.opacity(0.8)))
        ctx.fill(tiers[2], with: .color(Ui.accent.opacity(0.9)))
        _ = f
    }
}
