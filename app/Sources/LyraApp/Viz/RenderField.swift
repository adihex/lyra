import SwiftUI

/// Stateful field renderers — vis_terrain / vis_mosaic / vis_sand /
/// vis_omarchy (→Dither, no brand mark) / vis_red_sector / spectrogram
/// ring / WaveSeek waveform seekbar.
extension VizDraw {

    /// Terrain — scrolling ridge heightfield: ring history → silhouette,
    /// newest at the right edge (cliamp terrainDriver).
    static func terrain(_ ctx: inout GraphicsContext, _ size: CGSize,
                        _ f: VizFrame, _ st: VizState) {
        let cols = min(VizState.ringCols, Int(size.width / 2))
        guard cols > 1 else { return }
        let step = size.width / CGFloat(cols - 1)
        var ridge = Path()
        for i in 0..<cols {
            let age = cols - 1 - i
            var h: Float = 0
            for r in 0..<VizState.ringRows { h += st.ringCol(age: age, row: r) }
            h /= Float(VizState.ringRows)
            let x = CGFloat(i) * step
            let y = size.height - CGFloat(h) * size.height * 0.92
            i == 0 ? ridge.move(to: CGPoint(x: x, y: y))
                   : ridge.addLine(to: CGPoint(x: x, y: y))
        }
        var fill = ridge
        fill.addLine(to: CGPoint(x: size.width, y: size.height))
        fill.addLine(to: CGPoint(x: 0, y: size.height))
        fill.closeSubpath()
        ctx.fill(fill, with: .linearGradient(
            Gradient(colors: [Ui.indigo.opacity(0.10), Ui.indigo.opacity(0.4)]),
            startPoint: .zero, endPoint: CGPoint(x: 0, y: size.height)))
        ctx.stroke(ridge, with: .color(Ui.accent), lineWidth: 1.5)
        _ = f
    }

    /// Mosaic — fixed-tile heatmap: cells ignite over their threshold,
    /// decay in place; hot tiles promote to accent (cliamp mosaicDriver).
    static func mosaic(_ ctx: inout GraphicsContext, _ size: CGSize,
                       _ f: VizFrame, _ st: VizState) {
        let W = VizState.mosW, H = VizState.mosH
        let cw = size.width / CGFloat(W), ch = size.height / CGFloat(H)
        var cool = Path(), mid = Path(), hot = Path()
        for r in 0..<H {
            for c in 0..<W {
                let v = st.mosaic[r * W + c]
                guard v > 0.05 else { continue }
                let rect = CGRect(x: CGFloat(c) * cw + 0.5, y: CGFloat(r) * ch + 0.5,
                                  width: cw - 1, height: ch - 1)
                // cliamp tiers: ≥0.85 overdrive · ≥0.65 hot · else shade levels
                if v >= 0.85 { hot.addRect(rect) }
                else if v >= 0.65 { mid.addRect(rect) }
                else { cool.addRect(rect) }
            }
        }
        ctx.fill(cool, with: .color(Ui.mint.opacity(0.5)))
        ctx.fill(mid, with: .color(Ui.indigo.opacity(0.75)))
        ctx.fill(hot, with: .color(Ui.accent))
        _ = f
    }

    /// Sand — falling-sand CA grid (state tick): grains tinted by the band
    /// tier that spawned them; boom particles during the explosion phase.
    static func sand(_ ctx: inout GraphicsContext, _ size: CGSize,
                     _ f: VizFrame, _ st: VizState) {
        let W = VizState.sandW, H = VizState.sandH
        let cw = size.width / CGFloat(W), ch = size.height / CGFloat(H)
        if st.sandBoomTTL > 0 || st.sandBoom.parts.contains(where: { $0.alive }) {
            var tiers = [Path(), Path(), Path()]
            for p in st.sandBoom.parts where p.alive {
                tiers[max(0, min(2, Int(p.seed) - 1))].addRect(CGRect(
                    x: CGFloat(p.x) * size.width - 1, y: CGFloat(p.y) * size.height - 1,
                    width: 2, height: 2))
            }
            ctx.fill(tiers[0], with: .color(Ui.mint.opacity(0.8)))
            ctx.fill(tiers[1], with: .color(Ui.indigo.opacity(0.85)))
            ctx.fill(tiers[2], with: .color(Ui.accent.opacity(0.9)))
            return
        }
        var tiers = [Path(), Path(), Path()]
        for y in 0..<H {
            for x in 0..<W where st.sand[y * W + x] != 0 {
                let g = st.sand[y * W + x]
                tiers[Int(g) - 1].addRect(CGRect(x: CGFloat(x) * cw,
                                                 y: CGFloat(y) * ch,
                                                 width: max(cw, 1), height: max(ch, 1)))
            }
        }
        ctx.fill(tiers[0], with: .color(Ui.mint.opacity(0.75)))
        ctx.fill(tiers[1], with: .color(Ui.indigo.opacity(0.8)))
        ctx.fill(tiers[2], with: .color(Ui.accent.opacity(0.85)))
        _ = f
    }

    /// Dither — ordered-dither pixel field (cliamp Omarchy minus the brand
    /// mark): drifting value noise thresholded through an 8×8 Bayer matrix
    /// + per-pixel jitter; spectrum thickens columns from the bottom,
    /// bass at the outer edges.
    static let bayer8: [UInt8] = [
        0, 32, 8, 40, 2, 34, 10, 42,
        48, 16, 56, 24, 50, 18, 58, 26,
        12, 44, 4, 36, 14, 46, 6, 38,
        60, 28, 52, 20, 62, 30, 54, 22,
        3, 35, 11, 43, 1, 33, 9, 41,
        51, 19, 59, 27, 49, 17, 57, 25,
        15, 47, 7, 39, 13, 45, 5, 37,
        63, 31, 55, 23, 61, 29, 53, 21,
    ]

    static func dither(_ ctx: inout GraphicsContext, _ size: CGSize,
                       _ f: VizFrame, _ st: VizState) {
        let px: CGFloat = 4
        let cols = Int(size.width / px), rows = Int(size.height / px)
        guard cols > 0, rows > 0 else { return }
        let t = Float(st.uiTick) * 0.03
        var amp: Float = 0
        for b in f.bands { amp += b }
        amp /= 64
        let NS = Float(VizState.noiseSize)
        func noiseAt(_ u: Float, _ v: Float) -> Float {
            let uu = u - floor(u / NS) * NS, vv = v - floor(v / NS) * NS
            let x0 = Int(uu), y0 = Int(vv)
            let x1 = (x0 + 1) % VizState.noiseSize, y1 = (y0 + 1) % VizState.noiseSize
            var fx = uu - Float(x0), fy = vv - Float(y0)
            fx = fx * fx * (3 - 2 * fx); fy = fy * fy * (3 - 2 * fy)
            let n = VizState.noiseSize
            let a = st.noise[y0 * n + x0], b = st.noise[y0 * n + x1]
            let c = st.noise[y1 * n + x0], d = st.noise[y1 * n + x1]
            return (a + (b - a) * fx) * (1 - fy) + (c + (d - c) * fx) * fy
        }
        var cool = Path(), mid = Path(), hot = Path()
        for r in 0..<rows {
            for c in 0..<cols {
                // mirrored band lookup — bass at the outer edges
                let half = Float(cols) / 2
                let side = min(abs(Float(c) + 0.5 - half) / max(half, 1), 1)
                let raw = max(sampleBand(f.bands, pos: (1 - side) * 63 - 0.5) - 0.06, 0) / 0.94
                // spectrum column: lit from bottom, eases off toward the top
                var specE: Float = 0
                if raw > 0 {
                    let up = Float(rows - 1 - r)
                    let tall = raw * Float(rows) * 0.95
                    if up < tall { specE = raw * pow(1 - up / tall, 0.85) }
                }
                let u = Float(c) / 6, v = Float(r) / 6
                let base = 0.6 * noiseAt(u + t * 0.14, v - t * 0.055)
                    + 0.4 * noiseAt(u * 0.55 - t * 0.08, v * 0.55 + t * 0.06)
                let jit = hashNoise(0, r * 197 + c * 31 + 7)
                let tw = 0.5 + 0.5 * sin(t * 1.1 + jit * 6.28)
                let lum = (0.30 + 0.52 * base * base + 0.18 * tw + amp * 0.22) * 0.34
                    + specE * 0.72
                let threshold = 0.55 * (Float(bayer8[(r & 7) * 8 + (c & 7)]) + 0.5) / 64
                    + 0.45 * jit
                guard lum > threshold else { continue }
                let rect = CGRect(x: CGFloat(c) * px, y: CGFloat(r) * px,
                                  width: px - 0.5, height: px - 0.5)
                let heat = specE * 0.55 + amp * 0.12
                if heat > 0.45 { hot.addRect(rect) }
                else if heat > 0.2 { mid.addRect(rect) }
                else { cool.addRect(rect) }
            }
        }
        ctx.fill(cool, with: .color(Ui.inkSoft.opacity(0.4)))
        ctx.fill(mid, with: .color(Ui.indigo.opacity(0.75)))
        ctx.fill(hot, with: .color(Ui.accent))
    }

    /// RedSector — RSI Megademo wireframe EQ: five hollow bars tumbling as
    /// a rigid body over a drifting starfield; backface-culled edges.
    static func redSector(_ ctx: inout GraphicsContext, _ size: CGSize,
                          _ f: VizFrame, _ st: VizState) {
        // starfield: three speed lanes drifting left
        var stars = Path()
        let count = max(6, min(90, Int(size.width * size.height / 130) / 8))
        for i in 1...count {
            let speed = 0.10 + Float(i % 3) * 0.09
            var x = hashNoise(0, i * 4 + 0) * Float(size.width)
            x = x - Float(st.uiTick) * speed
            x = x.truncatingRemainder(dividingBy: Float(size.width))
            if x < 0 { x += Float(size.width) }
            let y = hashNoise(0, i * 4 + 1) * Float(size.height)
            stars.addRect(CGRect(x: CGFloat(x), y: CGFloat(y), width: 1.5, height: 1.5))
        }
        ctx.fill(stars, with: .color(Ui.inkSoft.opacity(0.5)))

        // ── tumbling wireframe bars (port of redSectorPlace/drawBars) ──
        let frame = Double(st.tumble) * 60
        let cosY = cos(frame * 0.105), sinY = sin(frame * 0.105)
        let cosX = cos(frame * 0.073), sinX = sin(frame * 0.073)
        let camZ = 6.0 + sin(frame * 0.011) * 1.5
        let focal = 3.2, groundY = -1.05
        func place(_ x: Double, _ y: Double, _ z: Double) -> (x: Double, y: Double, z: Double, px: Double, py: Double) {
            let x1 = x * cosY + z * sinY
            let z1 = -x * sinY + z * cosY
            let y1 = y * cosX - z1 * sinX
            let z2 = y * sinX + z1 * cosX
            let depth = max(0.35, z2 + camZ)
            let s = focal / depth
            return (x1, y1, z2, x1 * s, y1 * s)
        }
        // fit against the max hull so quiet bars don't zoom the picture
        let hullX = Double(4) / 2 * 0.78 + 0.26
        var minX = Double.infinity, maxX = -Double.infinity
        var minY = Double.infinity, maxY = -Double.infinity
        for sx in [-1.0, 1.0] {
            for y in [groundY, groundY + 2.30] {
                for sz in [-1.0, 1.0] {
                    let r = place(sx * hullX, y, sz * 0.26)
                    minX = min(minX, r.px); maxX = max(maxX, r.px)
                    minY = min(minY, r.py); maxY = max(maxY, r.py)
                }
            }
        }
        let zoom = 0.55 + 0.30 * (0.5 + 0.5 * sin(frame * 0.011))
        let fitY = Double(size.height - 1) / max(maxY - minY, 0.001) * zoom
        let stretch = min(2.0, max(1, 40 / Double(size.height)))
        let fitX = min(fitY * stretch, Double(size.width - 1) / max(maxX - minX, 0.001))
        let cx = Double(size.width) / 2 - (minX + maxX) / 2 * fitX
        let cy = Double(size.height) / 2 + (minY + maxY) / 2 * fitY

        let cornerX: [Double] = [-1, 1, 1, -1, -1, 1, 1, -1]
        let cornerZ: [Double] = [-1, -1, -1, -1, 1, 1, 1, 1]
        let cornerTop = [false, false, true, true, false, false, true, true]
        let faces: [[Int]] = [
            [0, 1, 2, 3], [5, 4, 7, 6], [0, 3, 7, 4],
            [1, 5, 6, 2], [3, 2, 6, 7], [0, 4, 5, 1],
        ]
        var low = Path(), midP = Path(), high = Path()
        for bar in 0..<5 {
            let baseX = (Double(bar) - 2) * 0.78
            let topY = groundY + Double(st.rsHeight[bar])
            var rx = [Double](repeating: 0, count: 8)
            var ry = rx, rz = rx, px = rx, py = rx
            for c in 0..<8 {
                let r = place(baseX + cornerX[c] * 0.26,
                              cornerTop[c] ? topY : groundY,
                              cornerZ[c] * 0.26)
                rx[c] = r.x; ry[c] = r.y; rz[c] = r.z
                px[c] = cx + r.px * fitX; py[c] = cy - r.py * fitY
            }
            let shown = (st.rsHeight[bar] - 0.40) / 1.9
            for face in faces {
                let (i1, i2, i3) = (face[0], face[1], face[2])
                // backface cull: normal·view > 0 (inward-wound faces)
                let ux = rx[i2] - rx[i1], uy = ry[i2] - ry[i1], uz = rz[i2] - rz[i1]
                let vx = rx[i3] - rx[i2], vy = ry[i3] - ry[i2], vz = rz[i3] - rz[i2]
                let nx = uy * vz - uz * vy, ny = uz * vx - ux * vz, nz = ux * vy - uy * vx
                let facing = nx * rx[i1] + ny * ry[i1] + nz * (rz[i1] + camZ)
                guard facing > 0 else { continue }
                for e in 0..<4 {
                    let (a, b) = (face[e], face[(e + 1) % 4])
                    var seg = Path()
                    seg.move(to: CGPoint(x: px[a], y: py[a]))
                    seg.addLine(to: CGPoint(x: px[b], y: py[b]))
                    if shown >= 0.6 { high.addPath(seg) }
                    else if shown >= 0.3 { midP.addPath(seg) }
                    else { low.addPath(seg) }
                }
            }
        }
        ctx.stroke(low, with: .color(Ui.mint.opacity(0.9)), lineWidth: 1)
        ctx.stroke(midP, with: .color(Ui.indigo.opacity(0.9)), lineWidth: 1)
        ctx.stroke(high, with: .color(Ui.accent), lineWidth: 1)
        _ = f
    }

    /// Spectrogram — scrolling frequency heatmap off the state ring
    /// (240 cols × 32 rows; newest column at the right edge).
    static func spectrogram(_ ctx: inout GraphicsContext, _ size: CGSize,
                            _ f: VizFrame, _ st: VizState) {
        let cols = min(VizState.ringCols, Int(size.width / 2))
        let rows = VizState.ringRows
        let cw = size.width / CGFloat(cols), ch = size.height / CGFloat(rows)
        // quantize into 6 heat levels — mint dim → accent hot
        var tiers = [Path(), Path(), Path(), Path(), Path(), Path()]
        for c in 0..<cols {
            let age = cols - 1 - c
            for r in 0..<rows {
                let v = st.ringCol(age: age, row: r)
                guard v > 0.03 else { continue }
                let q = min(5, Int(v * 7))
                // low freq at the bottom
                let y = size.height - CGFloat(r + 1) * ch
                tiers[q].addRect(CGRect(x: CGFloat(c) * cw, y: y,
                                        width: max(cw, 1), height: max(ch - 0.5, 1)))
            }
        }
        let cols0: [Color] = [
            Ui.inkSoft.opacity(0.25), Ui.mint.opacity(0.4), Ui.mint.opacity(0.65),
            Ui.indigo.opacity(0.6), Ui.indigo.opacity(0.85), Ui.accent,
        ]
        for i in 0..<6 { ctx.fill(tiers[i], with: .color(cols0[i])) }
        _ = f
    }

    /// WaveSeek — waveform seekbar from stored peaks. Peaks are mock-seeded
    /// by the track id until viz-core exports the decoded-peak store.
    /// TODO(viz-core): swap MockVizFrameProvider.peaks for the FFI peak
    /// path once `WaveformPeaks` is exposed (lyra_track_peaks or similar).
    static func waveSeek(_ ctx: inout GraphicsContext, _ size: CGSize,
                         _ f: VizFrame, _ st: VizState) {
        let vm = ViewModel.shared
        st.seekSize = size
        let seed = vm.current.map { UInt64(bitPattern: Int64($0.id.hashValue)) } ?? 42
        let n = max(24, Int(size.width / 3))
        if st.seekSeed != seed || st.seekPeaks.min.count != n {
            st.seekSeed = seed
            st.seekPeaks = MockVizFrameProvider.peaks(seed: seed, count: n)
        }
        let (lo, hi) = st.seekPeaks
        let dur = max(vm.current?.duration ?? 1, 1)
        let played = Float(min(max(vm.displayPosition / dur, 0), 1))
        let mid = size.height / 2
        var done = Path(), rest = Path()
        let step = size.width / CGFloat(n)
        for i in 0..<n {
            let x = CGFloat(i) * step
            let top = mid - CGFloat(hi[i]) * mid * 0.92
            let bot = mid - CGFloat(lo[i]) * mid * 0.92
            let rect = CGRect(x: x, y: top, width: max(step - 1, 1), height: max(bot - top, 1))
            if Float(i) / Float(n) <= played { done.addRect(rect) }
            else { rest.addRect(rect) }
        }
        ctx.fill(rest, with: .color(Ui.inkSoft.opacity(0.5)))
        ctx.fill(done, with: .color(Ui.accent))
        // playhead hairline
        let px = CGFloat(played) * size.width
        ctx.fill(Path(CGRect(x: px - 0.5, y: 0, width: 1, height: size.height)),
                 with: .color(Ui.ink))
        // live level tickles the head of the played region
        if f.level > 0.02 {
            ctx.fill(Path(CGRect(x: px - 1, y: mid - 8 * CGFloat(f.level),
                                 width: 2, height: 16 * CGFloat(f.level))),
                     with: .color(Ui.accent))
        }
    }
}
