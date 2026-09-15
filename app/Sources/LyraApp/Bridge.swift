import Foundation

/// Thin Swift face over the lyra-ffi C ABI.
/// Contract: lyra_* strings are caller-freed via lyra_string_free.
enum LyraCore {

    static var version: String {
        guard let p = lyra_version() else { return "?" }
        return String(cString: p)
    }

    /// Probe an audio file → decoded JSON dictionary.
    /// Access is via security-scoped user selection (sandbox-safe).
    static func probe(path: String) -> [String: Any]? {
        guard let raw = path.withCString({ lyra_probe($0) }) else { return nil }
        defer { lyra_string_free(raw) }
        let json = String(cString: raw)
        return try? JSONSerialization.jsonObject(with: Data(json.utf8)) as? [String: Any]
    }

    /// Start the LAN remote server. Returns true if it launched.
    /// Security model is in lyra-remote; see BLUEPRINT.md § remote.
    @discardableResult
    static func startRemote(port: UInt16) -> Bool {
        lyra_remote_start(port) == 0
    }
}

/// Playback engine handle — wraps the lyra_engine_* C API.
/// Lazy-init: created on first use (grabs the default output device).
final class LyraPlayer {
    static let shared = LyraPlayer()
    private var engine: UnsafeMutableRawPointer?

    private init() {
        engine = lyra_engine_new()
    }

    deinit { lyra_engine_free(engine) }

    @discardableResult
    func play(path: String) -> Bool {
        guard let e = engine else { return false }
        return path.withCString { lyra_engine_play_file(e, $0) } == 0
    }
    func pause() { lyra_engine_pause(engine) }
    func resume() { lyra_engine_resume(engine) }
    func stop() { lyra_engine_stop(engine) }
    func seek(_ secs: Double) { lyra_engine_seek(engine, secs) }
    func setVolume(_ v: Float) { lyra_engine_set_volume(engine, v) }

    var position: Double { lyra_engine_position(engine) }
    var isPlaying: Bool { lyra_engine_is_playing(engine) != 0 }
    var canResume: Bool { lyra_engine_can_resume(engine) != 0 }

    /// Draw-ready viz JSON: {"bands":[…], "peak":[l,r], "clip":bool}.
    var viz: [String: Any]? {
        guard let raw = lyra_engine_viz(engine) else { return nil }
        defer { lyra_string_free(raw) }
        let json = String(cString: raw)
        return try? JSONSerialization.jsonObject(with: Data(json.utf8)) as? [String: Any]
    }
}
