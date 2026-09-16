import Foundation

/// Thin Swift face over the lyra-ffi C ABI.
/// Contract: lyra_* strings are caller-freed via lyra_string_free.
enum LyraCore {

    static var version: String {
        guard let p = lyra_version() else { return "?" }
        return String(cString: p)
    }

    /// Probe an audio file → decoded JSON dictionary.
    static func probe(path: String) -> [String: Any]? {
        guard let raw = path.withCString({ lyra_probe($0) }) else { return nil }
        defer { lyra_string_free(raw) }
        let json = String(cString: raw)
        return try? JSONSerialization.jsonObject(with: Data(json.utf8)) as? [String: Any]
    }

    /// Scan a folder recursively → [[String:Any]] track rows (LibraryTrack).
    /// Synchronous + CPU-bound — call off the main thread.
    static func scanDir(path: String) -> [[String: Any]]? {
        guard let raw = path.withCString({ lyra_scan_dir($0) }) else { return nil }
        defer { lyra_string_free(raw) }
        let json = String(cString: raw)
        return try? JSONSerialization.jsonObject(with: Data(json.utf8)) as? [[String: Any]]
    }

    /// Start the LAN remote server. Returns true if it launched.
    @discardableResult
    static func startRemote(port: UInt16) -> Bool {
        lyra_remote_start(port) == 0
    }
}

/// Persistent library DB — wraps lyra_lib_*. Open once at app start;
/// rows survive relaunch, rescan only re-probes mtime-changed files.
final class LyraLibrary {
    static let shared = LyraLibrary()
    private var lib: UnsafeMutableRawPointer?

    private init() {
        let dir = FileManager.default
            .urls(for: .applicationSupportDirectory, in: .userDomainMask)[0]
            .appendingPathComponent("Lyra", isDirectory: true)
        try? FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        lib = dir.appendingPathComponent("library.db").path
            .withCString { lyra_lib_open($0) }
    }

    deinit { lyra_lib_free(lib) }

    /// Incremental sync — returns SyncStats dict (walked/probed/skipped/pruned).
    /// Synchronous + blocking — call off the main thread.
    @discardableResult
    func syncDir(_ path: String) -> [String: Any]? {
        guard let lib, let raw = path.withCString({ lyra_lib_sync_dir(lib, $0) })
        else { return nil }
        defer { lyra_string_free(raw) }
        return try? JSONSerialization.jsonObject(with: Data(String(cString: raw).utf8)) as? [String: Any]
    }

    /// All library rows (LibraryTrack JSON dicts).
    var tracks: [[String: Any]] {
        guard let lib, let raw = lyra_lib_tracks(lib) else { return [] }
        defer { lyra_string_free(raw) }
        return (try? JSONSerialization.jsonObject(with: Data(String(cString: raw).utf8)) as? [[String: Any]]) ?? []
    }

    /// FTS search — prefix terms over title/artist/album.
    func search(_ q: String) -> [[String: Any]] {
        guard let lib, let raw = q.withCString({ lyra_lib_search(lib, $0) }) else { return [] }
        defer { lyra_string_free(raw) }
        return (try? JSONSerialization.jsonObject(with: Data(String(cString: raw).utf8)) as? [[String: Any]]) ?? []
    }
}

/// Playback engine handle — wraps the lyra_engine_* C API.
/// The engine is hot-swapped on output-mode changes, so the pointer is
/// never cached: every call re-reads lyra_engine_current().
final class LyraPlayer {
    static let shared = LyraPlayer()
    private var engine: UnsafeMutableRawPointer? { lyra_engine_current() }
    /// The remote server routes commands into this engine.
    var enginePtr: UnsafeMutableRawPointer? { engine }

    private init() {
        // Persisted choice; fall back to compat if exclusive is unavailable
        // (device went multichannel, hog denied) — never leave the app silent.
        let wanted: Int32 = UserDefaults.standard.bool(forKey: "exclusiveOutput") ? 1 : 0
        if lyra_engine_new_mode(wanted) == nil, wanted == 1 {
            _ = lyra_engine_new_mode(0)
        }
    }

    deinit { lyra_engine_free(engine) }

    /// Switch output path at runtime. The engine comes back idle — the
    /// caller decides whether to restart the current track.
    @discardableResult
    func setExclusiveOutput(_ on: Bool) -> Bool {
        lyra_engine_set_output_mode(on ? 1 : 0) == 0
    }
    var exclusiveOutput: Bool { lyra_engine_output_mode() == 1 }

    @discardableResult
    func play(path: String) -> Bool {
        guard let e = engine else { return false }
        return path.withCString { lyra_engine_play_file(e, $0) } == 0
    }

    /// Stream a file out of a torrent — pieces fetch on demand.
    @discardableResult
    func playTorrent(_ torrentId: Int, file fileIdx: Int) -> Bool {
        guard let e = engine else { return false }
        return lyra_engine_play_torrent(e, Int32(torrentId), Int32(fileIdx)) == 0
    }
    func pause() { lyra_engine_pause(engine) }
    func resume() { lyra_engine_resume(engine) }
    func stop() { lyra_engine_stop(engine) }
    func seek(_ secs: Double) { lyra_engine_seek(engine, secs) }
    func setVolume(_ v: Float) { lyra_engine_set_volume(engine, v) }
    func setBand(_ band: Int, freq: Float, q: Float, gainDb: Float, peaking: Bool) {
        lyra_engine_set_band(engine, Int32(band), freq, q, gainDb, peaking ? 1 : 0)
    }

    var position: Double { lyra_engine_position(engine) }
    var isPlaying: Bool { lyra_engine_is_playing(engine) != 0 }
    var canResume: Bool { lyra_engine_can_resume(engine) != 0 }

    /// Raw normalized bands — the 60Hz path (no JSON, no alloc).
    func vizBands(into buf: UnsafeMutableBufferPointer<Float>) -> Int {
        guard let e = engine, let base = buf.baseAddress else { return 0 }
        return Int(lyra_engine_viz_bands(e, base, UInt(buf.count)))
    }

    /// EQ response curve: {"freqs":[…], "db":[…]} — same coefficients as audio.
    var eqResponse: (freqs: [Float], db: [Float])? {
        guard let raw = lyra_engine_eq_response(engine) else { return nil }
        defer { lyra_string_free(raw) }
        let json = String(cString: raw)
        guard let d = try? JSONSerialization.jsonObject(with: Data(json.utf8)) as? [String: Any],
              let f = d["freqs"] as? [Float], let db = d["db"] as? [Float]
        else { return nil }
        return (f, db)
    }

    /// Draw-ready viz JSON: {"bands":[…], "peak":[l,r], "clip":bool}.
    var viz: [String: Any]? {
        guard let raw = lyra_engine_viz(engine) else { return nil }
        defer { lyra_string_free(raw) }
        let json = String(cString: raw)
        return try? JSONSerialization.jsonObject(with: Data(json.utf8)) as? [String: Any]
    }
}

/// Torrent session — one rqbit process-wide; downloads land inside the app
/// container so the sandbox permits reads/writes.
final class LyraTorrent {
    static let shared = LyraTorrent()

    private init() {
        let dir = FileManager.default
            .urls(for: .applicationSupportDirectory, in: .userDomainMask)[0]
            .appendingPathComponent("Lyra/torrents", isDirectory: true)
        try? FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        if lyra_torrent_init(dir.path) != 0 {
            NSLog("lyra: torrent engine init failed")
        }
    }

    /// Magnet URI or .torrent path → torrent id (≥0). Blocks on magnet
    /// metadata resolve — call off the main thread.
    func add(_ spec: String) -> Int {
        Int(spec.withCString { lyra_torrent_add($0) })
    }

    /// [{index,path,len}] for a resolved torrent.
    func files(_ id: Int) -> [[String: Any]] {
        guard let raw = lyra_torrent_files(Int32(id)) else { return [] }
        defer { lyra_string_free(raw) }
        return (try? JSONSerialization.jsonObject(with: Data(String(cString: raw).utf8)))
            as? [[String: Any]] ?? []
    }

    /// {duration_secs,codec,sample_rate,channels} for a torrent file —
    /// reads only the header region (piece 0 fetches on demand).
    /// Call off the main thread; nil = keep the row without metadata.
    func probe(_ id: Int, file idx: Int) -> [String: Any]? {
        guard let raw = lyra_torrent_probe(Int32(id), Int32(idx)) else { return nil }
        defer { lyra_string_free(raw) }
        return try? JSONSerialization.jsonObject(with: Data(String(cString: raw).utf8))
            as? [String: Any]
    }

    /// {progress_bytes,total_bytes,finished}
    func stats(_ id: Int) -> [String: Any]? {
        guard let raw = lyra_torrent_stats(Int32(id)) else { return nil }
        defer { lyra_string_free(raw) }
        return try? JSONSerialization.jsonObject(with: Data(String(cString: raw).utf8)) as? [String: Any]
    }
}


/// LAN remote — SPAKE2 pairing → pinned X25519 keys → Noise XX.
/// The listener binds 0.0.0.0; security lives in the handshake.
final class LyraRemote {
    static let shared = LyraRemote()
    private(set) var running = false

    private init() {
        let dir = FileManager.default
            .urls(for: .applicationSupportDirectory, in: .userDomainMask)[0]
            .appendingPathComponent("Lyra", isDirectory: true)
        try? FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        let key = dir.appendingPathComponent("remote-key.bin").path
        guard let e = LyraPlayer.shared.enginePtr else { return }
        if key.withCString({ lyra_remote_init(e, $0) }) == 0,
           lyra_remote_start(4777) == 0 {
            running = true
        } else {
            NSLog("lyra: remote init/listen failed")
        }
    }

    /// Open a pairing window → (code, host fingerprint) or nil.
    func openPairing() -> (code: String, fingerprint: String)? {
        guard let raw = lyra_remote_open_pairing() else { return nil }
        defer { lyra_string_free(raw) }
        guard let d = try? JSONSerialization.jsonObject(with: Data(String(cString: raw).utf8))
                as? [String: Any],
              let code = d["code"] as? String, let fp = d["fp"] as? String
        else { return nil }
        return (code, fp)
    }

    var pairedCount: Int { Int(lyra_remote_paired_count()) }

    /// Persisted paired devices — [(pinned-key hash hex, display name)].
    var devices: [(id: String, name: String)] {
        guard let raw = lyra_remote_devices() else { return [] }
        defer { lyra_string_free(raw) }
        let rows = (try? JSONSerialization.jsonObject(with: Data(String(cString: raw).utf8)))
            as? [[String: Any]] ?? []
        return rows.compactMap { r in
            guard let id = r["id"] as? String, let name = r["name"] as? String
            else { return nil }
            return (id, name)
        }
    }

    /// Remove a paired device — it must re-pair to connect again.
    @discardableResult
    func revoke(_ id: String) -> Bool {
        id.withCString { lyra_remote_revoke($0) } == 0
    }
}
