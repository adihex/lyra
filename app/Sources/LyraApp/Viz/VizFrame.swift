import Foundation

/// Value-semantics mirror of the C `LyraVizFrame` (see docs/VIZ-CONTRACT.md).
/// bands: 64 log-spaced 0..1 · waveL/R: 256 samples -1..1 · peak/rms: L/R 0..1.
struct VizFrame {
    var bands: [Float]      // 64
    var waveL: [Float]      // 256
    var waveR: [Float]      // 256
    var peakL, peakR: Float
    var rmsL, rmsR: Float
    var bass, beat, level: Float
    var clipL, clipR: Bool
    var seq: UInt64

    init(bands: [Float], waveL: [Float], waveR: [Float],
         peakL: Float, peakR: Float, rmsL: Float, rmsR: Float,
         bass: Float, beat: Float, level: Float,
         clipL: Bool, clipR: Bool, seq: UInt64) {
        self.bands = bands; self.waveL = waveL; self.waveR = waveR
        self.peakL = peakL; self.peakR = peakR
        self.rmsL = rmsL; self.rmsR = rmsR
        self.bass = bass; self.beat = beat; self.level = level
        self.clipL = clipL; self.clipR = clipR; self.seq = seq
    }

    static var rest: VizFrame {
        VizFrame(bands: Array(repeating: 0, count: 64),
                 waveL: Array(repeating: 0, count: 256),
                 waveR: Array(repeating: 0, count: 256),
                 peakL: 0, peakR: 0, rmsL: 0, rmsR: 0,
                 bass: 0, beat: 0, level: 0,
                 clipL: false, clipR: false, seq: 0)
    }

    /// Parse the raw C struct bytes written by `lyra_engine_viz_frame`.
    /// Layout: 583 floats (bands64, waveL256, waveR256, peak2, rms2,
    /// bass, beat, level), then u32 clip bitmask, then u64 seq.
    init(cBytes bytes: [UInt8], seq: UInt64) {
        func f(_ i: Int) -> Float {
            let off = i * 4
            guard off + 4 <= bytes.count else { return 0 }
            var v: Float = 0
            withUnsafeMutableBytes(of: &v) { dst in
                bytes.withUnsafeBytes { src in
                    dst.copyMemory(from: UnsafeRawBufferPointer(
                        start: src.baseAddress!.advanced(by: off), count: 4))
                }
            }
            return v.isFinite ? v : 0
        }
        var bands: [Float] = []
        bands.reserveCapacity(64)
        for i in 0..<64 { bands.append(min(max(f(i), 0), 1)) }
        var waveL: [Float] = []
        waveL.reserveCapacity(256)
        for i in 0..<256 { waveL.append(min(max(f(64 + i), -1), 1)) }
        var waveR: [Float] = []
        waveR.reserveCapacity(256)
        for i in 0..<256 { waveR.append(min(max(f(320 + i), -1), 1)) }
        let clipOff = 583 * 4
        var clipRaw: UInt32 = 0
        if clipOff + 4 <= bytes.count {
            withUnsafeMutableBytes(of: &clipRaw) { dst in
                bytes.withUnsafeBytes { src in
                    dst.copyMemory(from: UnsafeRawBufferPointer(
                        start: src.baseAddress!.advanced(by: clipOff), count: 4))
                }
            }
        }
        self.init(bands: bands, waveL: waveL, waveR: waveR,
                  peakL: min(max(f(576), 0), 1), peakR: min(max(f(577), 0), 1),
                  rmsL: min(max(f(578), 0), 1), rmsR: min(max(f(579), 0), 1),
                  bass: min(max(f(580), 0), 1), beat: min(max(f(581), 0), 1),
                  level: min(max(f(582), 0), 1),
                  clipL: clipRaw & 1 == 1, clipR: clipRaw & 2 == 2, seq: seq)
    }

    /// Exponential ease toward a fresh frame; call once per UI tick.
    func eased(toward target: VizFrame, _ k: Float) -> VizFrame {
        var out = self
        for i in 0..<64 { out.bands[i] += (target.bands[i] - out.bands[i]) * k }
        for i in 0..<256 {
            out.waveL[i] += (target.waveL[i] - out.waveL[i]) * k
            out.waveR[i] += (target.waveR[i] - out.waveR[i]) * k
        }
        out.peakL += (target.peakL - out.peakL) * k
        out.peakR += (target.peakR - out.peakR) * k
        out.rmsL += (target.rmsL - out.rmsL) * k
        out.rmsR += (target.rmsR - out.rmsR) * k
        out.bass += (target.bass - out.bass) * k
        out.beat += (target.beat - out.beat) * k
        out.level += (target.level - out.level) * k
        out.clipL = target.clipL
        out.clipR = target.clipR
        out.seq = target.seq
        return out
    }

    /// Decay toward silence — used when seq stalls (paused engine).
    func decayed(_ k: Float = 0.94) -> VizFrame {
        var out = self
        for i in 0..<64 { out.bands[i] *= k }
        for i in 0..<256 { out.waveL[i] *= k; out.waveR[i] *= k }
        out.peakL *= k; out.peakR *= k; out.rmsL *= k; out.rmsR *= k
        out.bass *= k; out.beat *= k; out.level *= k
        out.clipL = false; out.clipR = false
        return out
    }
}

/// One viz-frame source. The real engine source when viz-core lands,
/// the mock until then — the surface treats them identically.
protocol VizFrameSource {
    /// Next frame, or nil when the source has nothing (engine missing).
    func nextFrame() -> VizFrame?
}

/// Deterministic synthetic frames so UI work proceeds before FFI lands:
/// rotating sine-sweep bands, periodic beat pulse, L/R sine waves with a
/// phase offset, occasional sticky clip. Pure function of the tick —
/// same tick always yields the same frame.
final class MockVizFrameProvider: VizFrameSource {
    private var t: UInt64 = 0

    func nextFrame() -> VizFrame? {
        t &+= 1
        return MockVizFrameProvider.frame(at: t)
    }

    func reset() { t = 0 }

    static func frame(at t: UInt64) -> VizFrame {
        let tt = Double(t) * 0.055
        var bands = [Float](repeating: 0, count: 64)
        for i in 0..<64 {
            let d = Double(i)
            var v = 0.34
                + 0.30 * sin(tt * 1.7 - d * 0.27)
                + 0.18 * sin(tt * 0.63 + d * 0.11)
                + 0.10 * sin(tt * 3.1 + d * 0.53)
            v *= 1.0 - (d / 64.0) * 0.45 // gentle high-end rolloff
            bands[i] = Float(min(max(v, 0), 1))
        }
        // periodic beat pulse: sharp attack, ~150ms-ish decay at 60Hz
        let phase = Double(t % 42) / 42.0
        let beatPulse = phase < 0.18 ? Float(1.0 - phase / 0.18) : 0
        let bass = (bands[0] + bands[1] + bands[2] + bands[3]) / 4
        let beat = min(max(beatPulse * 0.7 + bass * 0.5, 0), 1)
        var waveL = [Float](repeating: 0, count: 256)
        var waveR = [Float](repeating: 0, count: 256)
        let env = 0.25 + 0.75 * Double(min(max(bass * 1.4, 0), 1))
        for j in 0..<256 {
            let ph = Double(j) / 256.0 * .pi * 6.0
            waveL[j] = Float(sin(ph + tt * 2.6) * 0.62 * env
                             + 0.18 * sin(ph * 2.7 + tt * 5.1) * env)
            waveR[j] = Float(sin(ph * 0.94 + tt * 2.6 + 0.9) * 0.62 * env
                             + 0.18 * sin(ph * 2.5 + tt * 4.7 + 0.4) * env)
        }
        let peakL = min(max(bands[8] * 0.9 + beat * 0.3, 0), 1)
        let peakR = min(max(bands[10] * 0.9 + beat * 0.25, 0), 1)
        let level = min(max(bands.reduce(0, +) / 64 * 1.6, 0), 1)
        let clipping = (t % 613) < 6 // occasional sticky clip blip
        return VizFrame(bands: bands, waveL: waveL, waveR: waveR,
                        peakL: peakL, peakR: peakR,
                        rmsL: peakL * 0.7, rmsR: peakR * 0.7,
                        bass: bass, beat: beat, level: level,
                        clipL: clipping, clipR: clipping && (t % 2 == 0),
                        seq: t)
    }

    /// Stable per-track pseudo peaks for WaveSeek until a real peak store
    /// lands (TODO: wire to the decoder peak path when viz-core ships it).
    static func peaks(seed: UInt64, count: Int) -> (min: [Float], max: [Float]) {
        var lo = [Float](repeating: 0, count: count)
        var hi = [Float](repeating: 0, count: count)
        var s = seed &+ 0x9E3779B97F4A7C15
        for i in 0..<count {
            s ^= s << 13; s ^= s >> 7; s ^= s << 17 // xorshift64*
            let r0 = Float((s >> 11) & 0xFFFF) / 65535
            s ^= s << 13; s ^= s >> 7; s ^= s << 17
            let r1 = Float((s >> 11) & 0xFFFF) / 65535
            let swell = 0.45 + 0.55 * abs(sin(Double(i) * 0.05 + Double(seed % 97)))
            hi[i] = min(max((0.25 + 0.75 * r0) * Float(swell), 0.04), 1)
            lo[i] = -min(max((0.25 + 0.75 * r1) * Float(swell), 0.04), 1)
        }
        return (lo, hi)
    }
}
