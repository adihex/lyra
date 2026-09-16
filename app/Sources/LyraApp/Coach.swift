import AVFoundation
import Foundation

/// Live input lane — an AVAudioEngine input tap feeds mono f32 into
/// lyra-coach's Session. The tap thread calls lyra_coach_push; the UI
/// polls events/score. All timing uses the tap's sample clock, rebased so
/// session t=0 is the first captured block.
final class LyraCoach {
    static let shared = LyraCoach()

    private var engine: AVAudioEngine?
    private var t0Samples: AVAudioFramePosition?
    private(set) var running = false

    private init() {}

    /// chart: [{t_secs, midi?, policy?}]. config: {latency_offset_s,
    /// wait_for_me} — sample_rate is whatever the tap hands us (48k).
    @discardableResult
    func start(chart: [[String: Any]], config: [String: Any] = [:]) -> Bool {
        guard let cj = json(chart), let cfg = json(config) else { return false }
        let rc = cj.withCString { c in cfg.withCString { lyra_coach_new(c, $0) } }
        guard rc == 0 else { return false }
        return startTap()
    }

    private func startTap() -> Bool {
        let eng = AVAudioEngine()
        let input = eng.inputNode
        let native = input.outputFormat(forBus: 0)
        // Mono float32 at the device's native rate — simplest tap contract.
        guard let fmt = AVAudioFormat(commonFormat: .pcmFormatFloat32,
                                      sampleRate: native.sampleRate,
                                      channels: 1, interleaved: false)
        else { return false }
        t0Samples = nil
        input.installTap(onBus: 0, bufferSize: 1024, format: fmt) { [weak self] buf, when in
            guard let self,
                  let ch = buf.floatChannelData else { return }
            if self.t0Samples == nil { self.t0Samples = when.sampleTime }
            let t0 = self.t0Samples ?? when.sampleTime
            let t = Double(when.sampleTime - t0) / fmt.sampleRate
            _ = lyra_coach_push(ch[0], UInt(buf.frameLength), t)
        }
        do {
            try eng.start()
            engine = eng
            running = true
            return true
        } catch {
            NSLog("lyra-coach: input tap failed: \(error.localizedDescription)")
            lyra_coach_stop()
            return false
        }
    }

    func stop() {
        engine?.inputNode.removeTap(onBus: 0)
        engine?.stop()
        engine = nil
        t0Samples = nil
        running = false
        lyra_coach_stop()
    }

    /// Drain staged events (verdicts, onsets, follower position).
    func events() -> [[String: Any]] {
        guard let raw = lyra_coach_events() else { return [] }
        defer { lyra_string_free(raw) }
        return (try? JSONSerialization.jsonObject(with: Data(String(cString: raw).utf8)) as? [[String: Any]]) ?? []
    }

    func score() -> [String: Any]? {
        guard let raw = lyra_coach_score() else { return nil }
        defer { lyra_string_free(raw) }
        return try? JSONSerialization.jsonObject(with: Data(String(cString: raw).utf8)) as? [String: Any]
    }

    func startCalibration(bpm: Double) { _ = lyra_coach_start_calibration(0, bpm) }

    /// Applies the measured offset when enough hits landed.
    func completeCalibration() -> [String: Any]? {
        guard let raw = lyra_coach_complete_calibration() else { return nil }
        defer { lyra_string_free(raw) }
        return try? JSONSerialization.jsonObject(with: Data(String(cString: raw).utf8)) as? [String: Any]
    }

    func setFeedback(_ mode: String) { mode.withCString { _ = lyra_coach_feedback($0) } }

    private func json(_ v: Any) -> String? {
        (try? JSONSerialization.data(withJSONObject: v))
            .flatMap { String(data: $0, encoding: .utf8) }
    }
}
