import SwiftUI

/// Trace/meter renderers — vis_wave, vis_scope, vis_heartbeat, vis_retro,
/// vis_stereo, vis_classic_led ported to real geometry.
extension VizDraw {

    /// Wave — mono oscilloscope trace of the L channel with continuity.
    static func wave(_ ctx: inout GraphicsContext, _ size: CGSize, _ f: VizFrame) {
        var base = Path()
        base.move(to: CGPoint(x: 0, y: size.height / 2))
        base.addLine(to: CGPoint(x: size.width, y: size.height / 2))
        ctx.stroke(base, with: .color(Ui.border.opacity(0.7)),
                   style: StrokeStyle(lineWidth: 1, dash: [5, 4]))
        var p = Path()
        let n = f.waveL.count
        for i in 0..<n {
            let x = CGFloat(i) / CGFloat(n - 1) * size.width
            let y = (1 - CGFloat(f.waveL[i])) * 0.5 * size.height
            i == 0 ? p.move(to: CGPoint(x: x, y: y)) : p.addLine(to: CGPoint(x: x, y: y))
        }
        ctx.stroke(p, with: .color(Ui.indigo), lineWidth: 1.5)
    }

    /// Scope — Lissajous XY from waveL vs waveR (true stereo pair from the
    /// frame; cliamp phase-delays mono, we get real L/R).
    static func scope(_ ctx: inout GraphicsContext, _ size: CGSize, _ f: VizFrame) {
        var p = Path()
        let n = min(f.waveL.count, f.waveR.count)
        for i in 0..<n {
            let x = (CGFloat(f.waveL[i]) + 1) * 0.5 * size.width
            let y = (1 - CGFloat(f.waveR[i])) * 0.5 * size.height
            i == 0 ? p.move(to: CGPoint(x: x, y: y)) : p.addLine(to: CGPoint(x: x, y: y))
        }
        ctx.stroke(p, with: .color(Ui.mint), lineWidth: 1)
        // frame crosshair
        var cross = Path()
        cross.move(to: CGPoint(x: size.width / 2, y: 0))
        cross.addLine(to: CGPoint(x: size.width / 2, y: size.height))
        cross.move(to: CGPoint(x: 0, y: size.height / 2))
        cross.addLine(to: CGPoint(x: size.width, y: size.height / 2))
        ctx.stroke(cross, with: .color(Ui.border.opacity(0.5)), lineWidth: 0.5)
    }

    /// Heartbeat — scrolling ECG trace off the state's ring; squared-sample
    /// shaping happens at push time (w·|w| flavor via beat spike).
    static func heartbeat(_ ctx: inout GraphicsContext, _ size: CGSize,
                          _ f: VizFrame, _ st: VizState) {
        let mid = size.height / 2
        var base = Path()
        for x in stride(from: 0, to: size.width, by: 10) {
            base.addRect(CGRect(x: x, y: mid - 0.5, width: 5, height: 1))
        }
        ctx.fill(base, with: .color(Ui.mint.opacity(0.4)))
        var p = Path()
        let n = st.ecg.count
        for i in 0..<n {
            // oldest → newest left→right
            let idx = (st.ecgHead + i) % n
            let s = st.ecg[idx]
            let shaped = s * abs(s) * 2.2 // square magnitude, keep sign — ECG sharpen
            let x = CGFloat(i) / CGFloat(n - 1) * size.width
            let y = mid - CGFloat(shaped) * size.height * 0.45
            i == 0 ? p.move(to: CGPoint(x: x, y: y)) : p.addLine(to: CGPoint(x: x, y: y))
        }
        ctx.stroke(p, with: .color(Ui.accent), lineWidth: 1.5)
    }

    /// Retro — synthwave scene: striped sun, perspective grid floor
    /// scrolling toward the viewer, spectrum silhouette on the horizon.
    static func retro(_ ctx: inout GraphicsContext, _ size: CGSize,
                      _ f: VizFrame, _ st: VizState) {
        let horizon = size.height * 0.4
        let cx = size.width / 2
        // ── sun: semicircle with stripe gaps in the lower half ──
        let sunR = horizon * 0.85
        var sun = Path()
        var dy: CGFloat = 0
        while dy < sunR {
            let rowDist = sunR - dy // distance above sun base (= horizon)
            let halfW = sqrt(max(0, sunR * sunR - rowDist * rowDist))
            // stripes in the bottom half: skip alternate bands
            let stripeOn = rowDist >= sunR * 0.5
                || Int(rowDist / max(1, sunR * 0.15)) % 2 == 0
            if stripeOn && halfW > 0.5 {
                sun.addRect(CGRect(x: cx - halfW, y: horizon - rowDist - 1.5,
                                   width: halfW * 2, height: 2))
            }
            dy += 2
        }
        ctx.fill(sun, with: .color(Ui.accent.opacity(0.9)))
        // ── grid floor ──
        var grid = Path()
        let vLines = 18
        for i in 0...vLines {
            let bx = CGFloat(i) / CGFloat(vLines) * size.width
            grid.move(to: CGPoint(x: bx, y: size.height))
            grid.addLine(to: CGPoint(x: cx, y: horizon))
        }
        let scroll = (Float(st.uiTick) * 0.08).truncatingRemainder(dividingBy: 1)
        let floorDepth = size.height - horizon - 2
        for i in 0..<10 {
            var z = (Float(i) + scroll) / 10
            if z > 1 { z -= 1 }
            let y = horizon + 1 + CGFloat(z * z) * floorDepth
            grid.addRect(CGRect(x: 0, y: y, width: size.width, height: 1))
        }
        ctx.stroke(grid, with: .color(Ui.indigo.opacity(0.55)), lineWidth: 1)
        // horizon hairline
        ctx.fill(Path(CGRect(x: 0, y: horizon - 0.5, width: size.width, height: 1)),
                 with: .color(Ui.inkSoft))
        // ── wave silhouette riding the horizon ──
        var w = Path()
        let maxWave = horizon * 0.85
        let step = max(size.width / 128, 2)
        var x: CGFloat = 0
        var first = true
        while x <= size.width {
            let pos = Float(x / size.width) * 63
            var level = sampleBand(f.bands, pos: pos)
            level = max(0.03, level)
            let y = horizon - CGFloat(level) * maxWave
            let pt = CGPoint(x: x, y: y)
            first ? w.move(to: pt) : w.addLine(to: pt)
            first = false
            x += step
        }
        ctx.stroke(w, with: .color(Ui.mint), lineWidth: 1.5)
    }

    /// ClassicLED — segmented LED bars: quantized lit cells, phosphor decay
    /// body (st.leds), hold-then-fall peak caps (st.ledPeak).
    static func classicLED(_ ctx: inout GraphicsContext, _ size: CGSize,
                           _ f: VizFrame, _ st: VizState) {
        let barW: CGFloat = 8, gap: CGFloat = 3
        let bars = max(1, Int((size.width + gap) / (barW + gap)))
        let segH: CGFloat = 5, segGap: CGFloat = 2
        let segs = max(1, Int(size.height / (segH + segGap)))
        var tiers = [Path(), Path(), Path()]
        var caps = Path(), unlit = Path()
        for b in 0..<bars {
            let pos = Float(b) / Float(bars - 1) * 63
            let body = sampleBand(st.leds, pos: pos)
            let lit = Int(body * Float(segs) + 0.5)
            let cap = Int(sampleBand(st.ledPeak, pos: pos) * Float(segs))
            let x = CGFloat(b) * (barW + gap)
            for r in 0..<segs {
                let y = size.height - CGFloat(r + 1) * (segH + segGap)
                let rect = CGRect(x: x, y: y, width: barW, height: segH)
                if r < lit {
                    let t = Float(r) / Float(segs)
                    tiers[t >= 0.6 ? 2 : (t >= 0.3 ? 1 : 0)].addRect(rect)
                } else {
                    unlit.addRect(rect)
                }
            }
            if cap >= lit && cap < segs {
                caps.addRect(CGRect(x: x, y: size.height - CGFloat(cap + 1) * (segH + segGap),
                                    width: barW, height: segH))
            }
        }
        ctx.fill(unlit, with: .color(Ui.ink.opacity(0.06)))
        ctx.fill(tiers[0], with: .color(Ui.mint))
        ctx.fill(tiers[1], with: .color(Ui.indigo))
        ctx.fill(tiers[2], with: .color(Ui.accent))
        ctx.fill(caps, with: .color(Ui.accent.opacity(0.9)))
    }

    /// Stereo — L/R horizontal LED meters with PPM hold ticks
    /// (cliamp stereoDriver; inputs peak/rms from the frame).
    static func stereo(_ ctx: inout GraphicsContext, _ size: CGSize,
                       _ f: VizFrame, _ st: VizState) {
        let segW: CGFloat = 6, gap: CGFloat = 2
        let rowH = size.height / 2
        let levels = [st.stereoL, st.stereoR]
        let rms = [f.rmsL, f.rmsR]
        let labels = ["L", "R"]
        for ch in 0..<2 {
            let y0 = CGFloat(ch) * rowH + 4
            let h = rowH - 10
            let cells = max(1, Int((size.width - 26) / (segW + gap)))
            var p = Path(), peakP = Path(), dark = Path()
            let lit = Int(levels[ch] * Float(cells) + 0.5)
            // RMS fill (dimmer body) + peak edge — two-layer meter
            let rmsLit = Int(rms[ch] * Float(cells) + 0.5)
            for i in 0..<cells {
                let x = 22 + CGFloat(i) * (segW + gap)
                let rect = CGRect(x: x, y: y0, width: segW, height: h)
                if i < lit {
                    p.addRect(rect)
                } else {
                    dark.addRect(rect)
                }
                if i == lit - 1 && lit > 0 {
                    peakP.addRect(CGRect(x: x, y: y0 - 2, width: segW, height: 2))
                }
            }
            ctx.fill(dark, with: .color(Ui.ink.opacity(0.07)))
            ctx.fill(p, with: .linearGradient(
                Gradient(colors: [Ui.mint, Ui.indigo, Ui.accent]),
                startPoint: CGPoint(x: 22, y: 0),
                endPoint: CGPoint(x: size.width, y: 0)))
            ctx.fill(peakP, with: .color(Ui.ink))
            // RMS ghost underline
            var rp = Path()
            if rmsLit > 0 {
                rp.addRect(CGRect(x: 22, y: y0 + h + 2,
                                  width: CGFloat(rmsLit) * (segW + gap) - gap, height: 2))
            }
            ctx.fill(rp, with: .color(Ui.inkSoft.opacity(0.6)))
            ctx.draw(Text(labels[ch]).font(.uiMono).foregroundColor(Ui.inkSoft),
                     at: CGPoint(x: 10, y: y0 + h / 2), anchor: .center)
        }
    }
}
