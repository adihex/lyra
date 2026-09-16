import SwiftUI

/// Bar-family renderers — spectrum columns on a baseline. Ports of cliamp's
/// vis_bars / vis_bars_dot / vis_rain / vis_bars_outline / vis_bricks /
/// vis_columns / vis_classic_peak / vis_mirror / vis_ascii / vis_butterfly.
extension VizDraw {

    /// Bars — smooth fractional bars, one per band, panel-anchored ramp.
    static func bars(_ ctx: inout GraphicsContext, _ size: CGSize, _ f: VizFrame) {
        let n = 64, bw = size.width / 64
        var p = Path()
        for i in 0..<n {
            let h = size.height * CGFloat(f.bands[i])
            guard h > 0.5 else { continue }
            p.addRect(CGRect(x: CGFloat(i) * bw, y: size.height - h,
                             width: max(bw - 1, 1), height: h))
        }
        ctx.fill(p, with: specGradient(size))
    }

    /// BarsDot — halftone stipple: square dots on a 4pt lattice, filled
    /// bottom-up inside each bar (cliamp's braille stipple, real geometry).
    static func barsDot(_ ctx: inout GraphicsContext, _ size: CGSize, _ f: VizFrame) {
        let pitch: CGFloat = 4, dot: CGFloat = 2
        let bw = size.width / 64
        var tiers = [Path(), Path(), Path()]
        for b in 0..<64 {
            let x = CGFloat(b) * bw
            let h = size.height * CGFloat(f.bands[b])
            var y = size.height - pitch
            while y > size.height - h {
                let t = Float(1 - y / size.height)
                let ti = t >= 0.6 ? 2 : (t >= 0.3 ? 1 : 0)
                var dx = x
                while dx + dot <= x + bw - 1 {
                    tiers[ti].addRect(CGRect(x: dx, y: y, width: dot, height: dot))
                    dx += pitch
                }
                y -= pitch
            }
        }
        ctx.fill(tiers[0], with: .color(Ui.mint))
        ctx.fill(tiers[1], with: .color(Ui.indigo))
        ctx.fill(tiers[2], with: .color(Ui.accent))
    }

    /// Rain — falling streaks inside bar silhouettes: head/body/tail,
    /// per-column speed + phase from a position seed (cliamp renderRain).
    static func rain(_ ctx: inout GraphicsContext, _ size: CGSize,
                     _ f: VizFrame, _ st: VizState) {
        let colW: CGFloat = 3
        let cols = Int(size.width / colW)
        let rows = Int(size.height / 5)
        guard cols > 0, rows > 0 else { return }
        var head = Path(), body = Path(), tail = Path()
        for c in 0..<cols {
            let level = sampleBand(f.bands, pos: Float(c) / Float(cols - 1) * 63)
            let activeRows = Int(Float(rows) * level)
            guard activeRows > 1 else { continue }
            // column gate: slow-changing, energy opens more columns
            if hashNoise(st.uiTick / 12, c) > level * 1.6 + 0.1 { continue }
            let speed = 1 + c % 3
            let dropLen = 2 + (c / 7) % 3
            let cycle = rows + dropLen + 3
            let offset = Int(hashNoise(UInt64(c * 13), 0) * Float(cycle))
            let pos = (Int(st.uiTick) / speed + offset) % cycle
            for r in 0..<activeRows {
                let dist = pos - r
                guard dist >= 0 && dist < dropLen else { continue }
                let y = size.height - CGFloat(r + 1) * 5
                let rect = CGRect(x: CGFloat(c) * colW, y: y, width: colW - 1, height: 4)
                if dist == 0 { head.addRect(rect) }
                else if dist == 1 { body.addRect(rect) }
                else { tail.addRect(rect) }
            }
        }
        ctx.fill(tail, with: .color(Ui.mint.opacity(0.5)))
        ctx.fill(body, with: .color(Ui.indigo.opacity(0.8)))
        ctx.fill(head, with: .color(Ui.accent))
    }

    /// Outline — only the top edge of each bar traced as a hairline.
    static func outline(_ ctx: inout GraphicsContext, _ size: CGSize, _ f: VizFrame) {
        let bw = size.width / 64
        var p = Path()
        for i in 0..<64 {
            let h = size.height * CGFloat(f.bands[i])
            guard h > 0.5 else { continue }
            p.addRect(CGRect(x: CGFloat(i) * bw, y: size.height - h,
                             width: max(bw - 1, 1), height: 1.5))
        }
        ctx.fill(p, with: .color(Ui.ink))
    }

    /// Bricks — segmented blocks with visible gaps (cliamp ▄ rows).
    static func bricks(_ ctx: inout GraphicsContext, _ size: CGSize, _ f: VizFrame) {
        let bh: CGFloat = 6, gap: CGFloat = 2
        let rows = Int(size.height / (bh + gap))
        var tiers = [Path(), Path(), Path()]
        for b in 0..<64 {
            let x = CGFloat(b) / 64 * size.width
            let w = max(size.width / 64 - 1, 1)
            let lit = Int(Float(rows) * f.bands[b] + 0.5)
            for r in 0..<min(lit, rows) {
                let y = size.height - CGFloat(r + 1) * (bh + gap)
                let t = Float(r) / Float(max(rows - 1, 1))
                tiers[t >= 0.6 ? 2 : (t >= 0.3 ? 1 : 0)]
                    .addRect(CGRect(x: x, y: y, width: w, height: bh))
            }
        }
        ctx.fill(tiers[0], with: .color(Ui.mint))
        ctx.fill(tiers[1], with: .color(Ui.indigo))
        ctx.fill(tiers[2], with: .color(Ui.accent))
    }

    /// Columns — dense thin columns, linear-interpolated between bands so
    /// neighbours vary slightly (cliamp interpolateBandColumns).
    static func columns(_ ctx: inout GraphicsContext, _ size: CGSize, _ f: VizFrame) {
        let colW: CGFloat = 3
        let cols = Int(size.width / colW)
        guard cols > 1 else { return }
        var p = Path()
        for c in 0..<cols {
            let level = sampleBand(f.bands, pos: Float(c) / Float(cols - 1) * 63)
            let h = size.height * CGFloat(level)
            guard h > 0.5 else { continue }
            p.addRect(CGRect(x: CGFloat(c) * colW, y: size.height - h,
                             width: colW - 1, height: h))
        }
        ctx.fill(p, with: specGradient(size))
    }

    /// ClassicPeak — smooth bars + detached peak caps with gravity fall
    /// (state: st.caps/capVel, advanced in tick).
    static func classicPeak(_ ctx: inout GraphicsContext, _ size: CGSize,
                            _ f: VizFrame, _ st: VizState) {
        let colW: CGFloat = 4
        let cols = Int(size.width / colW)
        guard cols > 1 else { return }
        var p = Path()
        for c in 0..<cols {
            let pos = Float(c) / Float(cols - 1) * 63
            let level = sampleBand(f.bands, pos: pos)
            let h = size.height * CGFloat(level)
            if h > 0.5 {
                p.addRect(CGRect(x: CGFloat(c) * colW, y: size.height - h,
                                 width: colW - 2, height: h))
            }
        }
        ctx.fill(p, with: specGradient(size))
        var caps = Path()
        for c in 0..<cols {
            let pos = Float(c) / Float(cols - 1) * 63
            let cap = sampleBand(st.caps, pos: pos)
            let body = sampleBand(f.bands, pos: pos)
            guard cap > body + 0.02 else { continue }
            caps.addRect(CGRect(x: CGFloat(c) * colW,
                                y: size.height * CGFloat(1 - cap) - 1,
                                width: colW - 2, height: 2))
        }
        ctx.fill(caps, with: .color(Ui.accent))
    }

    /// Mirror — spectrum mirrored on a horizontal axis, wobble-modulated
    /// amplitude per bar (cliamp renderMirror math, real rects).
    static func mirror(_ ctx: inout GraphicsContext, _ size: CGSize,
                       _ f: VizFrame, _ st: VizState) {
        let axisY = size.height / 2
        let cols = max(1, Int(size.width * 0.84 / 4))
        let x0 = (size.width - CGFloat(cols) * 4) / 2
        var env: Float = 0
        for b in f.bands { env += b }
        env /= 64
        let t = Float(st.uiTick) / 60
        var tiers = [Path(), Path(), Path()]
        for i in 0..<cols {
            let half = Float(cols - 1) / 2
            let dist = half > 0 ? abs(Float(i) - half) / half : 0
            let wobble = 0.4 + 0.6 * abs(sin(t * 4.6 + Float(i) * 0.42)
                                         * sin(t * 1.9 - Float(i) * 0.13))
            let amp = Float(size.height) * 0.80 * (1 - dist * 0.55)
                * (0.3 + 0.7 * env) * (0.35 + 0.65 * wobble)
            let band = sampleBand(f.bands, pos: dist * 30)
            let h = CGFloat(max(1, amp * (0.4 + 0.6 * band)))
            let frac = Float(h / (size.height / 2))
            tiers[frac >= 0.6 ? 2 : (frac >= 0.3 ? 1 : 0)].addRect(CGRect(
                x: x0 + CGFloat(i) * 4 + 0.5, y: axisY - h,
                width: 2.5, height: h * 2))
        }
        var axis = Path()
        axis.addRect(CGRect(x: x0, y: axisY - 0.5, width: CGFloat(cols) * 4, height: 1))
        ctx.fill(axis, with: .color(Ui.ink))
        ctx.fill(tiers[0], with: .color(Ui.mint.opacity(0.85)))
        ctx.fill(tiers[1], with: .color(Ui.indigo.opacity(0.85)))
        ctx.fill(tiers[2], with: .color(Ui.accent.opacity(0.9)))
    }

    /// Density — shade-block field: thin columns quantized into █▓▒░-style
    /// opacity tiers (cliamp Ascii port — shade glyphs become alpha steps).
    static func density(_ ctx: inout GraphicsContext, _ size: CGSize, _ f: VizFrame) {
        let colW: CGFloat = 4, rowH: CGFloat = 5
        let cols = Int(size.width / colW), rows = Int(size.height / rowH)
        guard cols > 1, rows > 1 else { return }
        // 4 alpha tiers stand in for the shade glyphs
        var tierP = [Path(), Path(), Path(), Path()]
        for c in 0..<cols {
            let level = sampleBand(f.bands, pos: Float(c) / Float(cols - 1) * 63)
            for r in 0..<rows {
                let rowBot = Float(r) / Float(rows), rowTop = Float(r + 1) / Float(rows)
                guard level > rowBot else { continue }
                let frac = min((level - rowBot) / (rowTop - rowBot), 1)
                let ti = frac >= 0.75 ? 3 : frac >= 0.5 ? 2 : frac >= 0.25 ? 1 : 0
                let y = size.height - CGFloat(r + 1) * rowH
                tierP[ti].addRect(CGRect(x: CGFloat(c) * colW, y: y,
                                         width: colW - 1, height: rowH - 1))
            }
        }
        ctx.fill(tierP[0], with: .color(Ui.mint.opacity(0.35)))
        ctx.fill(tierP[1], with: .color(Ui.indigo.opacity(0.55)))
        ctx.fill(tierP[2], with: .color(Ui.indigo.opacity(0.85)))
        ctx.fill(tierP[3], with: .color(Ui.accent))
    }

    /// Butterfly — mirrored Rorschach: band energy spreads symmetric wings
    /// from a center spine, stippled toward the edges.
    static func butterfly(_ ctx: inout GraphicsContext, _ size: CGSize,
                          _ f: VizFrame, _ st: VizState) {
        let rowH: CGFloat = 4
        let rows = Int(size.height / rowH)
        let cx = size.width / 2
        var p = Path()
        for r in 0..<rows {
            let energy = sampleBand(f.bands, pos: Float(r) / Float(max(rows - 1, 1)) * 63)
            let wob = sin(Float(st.uiTick) * 0.08 + Float(r) * 0.3) * 0.15
            let wing = cx * CGFloat(max(energy + wob, 0)) * 0.9
            let y = CGFloat(r) * rowH
            var dx: CGFloat = 0
            while dx < wing {
                let norm = Float(dx / max(wing, 1))
                var thresh = (1 - norm * norm) * energy
                if norm > 0.6 {
                    thresh *= 0.5 + 0.5 * sin(Float(st.uiTick) * 0.1
                        + Float(r) * 0.5 + Float(dx) * 0.3)
                }
                if hashNoise(st.uiTick / 3, r * 131 + Int(dx)) < thresh {
                    p.addRect(CGRect(x: cx + dx, y: y, width: 2, height: rowH - 1))
                    p.addRect(CGRect(x: cx - dx - 2, y: y, width: 2, height: rowH - 1))
                }
                dx += 2.5
            }
            if energy > 0.05 { // center spine
                p.addRect(CGRect(x: cx - 1, y: y, width: 2, height: rowH - 1))
            }
        }
        ctx.fill(p, with: .color(Ui.indigo.opacity(0.8)))
    }
}
