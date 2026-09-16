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

    /// Sync explicit file picks (panel multi-select) — never prunes.
    /// Synchronous + blocking — call off the main thread.
    @discardableResult
    func syncFiles(_ paths: [String]) -> [String: Any]? {
        guard let lib,
              let json = try? JSONSerialization.data(withJSONObject: paths),
              let js = String(data: json, encoding: .utf8),
              let raw = js.withCString({ lyra_lib_sync_files(lib, $0) })
        else { return nil }
        defer { lyra_string_free(raw) }
        return try? JSONSerialization.jsonObject(with: Data(String(cString: raw).utf8)) as? [String: Any]
    }

    /// Remote scan — SFTP walk + header probes into the same DB.
    /// Synchronous + blocking — call off the main thread.
    @discardableResult
    func syncRemote(_ profileJSON: String) -> [String: Any]? {
        guard let lib, let raw = profileJSON.withCString({ lyra_remlib_scan(lib, $0) })
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

    /// SFTP random-access under the block cache — same streaming shape
    /// as torrent playback.
    @discardableResult
    func playRemote(profileJSON: String, remotePath: String) -> Bool {
        guard let e = engine else { return false }
        return remotePath.withCString { rp in
            profileJSON.withCString { lyra_engine_play_remote(e, $0, rp) }
        } == 0
    }

    /// Stream a file out of a torrent — pieces fetch on demand.
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

/// Frame-packed viz snapshot — wraps `lyra_engine_viz_frame` from the
/// viz-core workstream (docs/VIZ-CONTRACT.md). That symbol ships in the
/// parallel Rust build, so it is resolved lazily via dlsym: the Swift shell
/// always links, and callers get nil (→ mock provider) until the real frame
/// producer lands. Once present, one call = one ~4.6KB struct copy.
enum LyraEngine {
    private typealias VizFrameFn =
        @convention(c) (UnsafeRawPointer?, UnsafeMutableRawPointer?) -> UInt64

    /// dlsym on the main executable — static-lib symbols land in the
    /// process image's export table, so this resolves without a header
    /// declaration once viz-core exports the symbol.
    private static let vizFrameSym: VizFrameFn? = {
        guard let p = dlsym(dlopen(nil, RTLD_LAZY), "lyra_engine_viz_frame")
        else { return nil }
        return unsafeBitCast(p, to: VizFrameFn.self)
    }()

    /// Scratch sized past the documented ~4.6KB payload so a struct that
    /// grows with the contract never overflows the buffer.
    private static let vizBufSize = 8192

    /// Latest viz frame from the live engine, or nil when the symbol or the
    /// engine is absent. `seq` unchanged between calls = engine stalled.
    static func vizFrame() -> VizFrame? {
        guard let sym = vizFrameSym, let e = LyraPlayer.shared.enginePtr
        else { return nil }
        var buf = [UInt8](repeating: 0, count: vizBufSize)
        let seq = buf.withUnsafeMutableBytes { sym(e, $0.baseAddress) }
        return VizFrame(cBytes: buf, seq: seq)
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

    /// Remove a torrent from the session. deleteFiles=true also wipes its
    /// downloaded data from disk — the disk-space reclaim path.
    @discardableResult
    func remove(_ id: Int, deleteFiles: Bool) -> Bool {
        lyra_torrent_remove(Int32(id), deleteFiles ? 1 : 0) == 0
    }

    /// Managed torrents [{id,name}] — the session persists across
    /// relaunches, so the UI rebuilds its list from this.
    func list() -> [TorrentInfo] {
        guard let raw = lyra_torrent_list() else { return [] }
        defer { lyra_string_free(raw) }
        let rows = (try? JSONSerialization.jsonObject(with: Data(String(cString: raw).utf8)))
            as? [[String: Any]] ?? []
        return rows.compactMap { r in
            guard let id = r["id"] as? Int, let name = r["name"] as? String
            else { return nil }
            return TorrentInfo(id: id, name: name)
        }
    }

    /// Download-dir entries not owned by any managed torrent —
    /// [(name, bytes)] leftovers from killed sessions.
    func orphans() -> [(name: String, bytes: UInt64)] {
        guard let raw = lyra_torrent_orphans() else { return [] }
        defer { lyra_string_free(raw) }
        let rows = (try? JSONSerialization.jsonObject(with: Data(String(cString: raw).utf8)))
            as? [[String: Any]] ?? []
        return rows.compactMap { r in
            guard let name = r["name"] as? String else { return nil }
            let bytes = (r["bytes"] as? NSNumber)?.uint64Value ?? 0
            return (name, bytes)
        }
    }

    /// Delete every orphan entry → (removed, bytesFreed); nil on failure.
    @discardableResult
    func purgeOrphans() -> (removed: Int, bytes: UInt64)? {
        guard let raw = lyra_torrent_purge_orphans() else { return nil }
        defer { lyra_string_free(raw) }
        guard let d = try? JSONSerialization.jsonObject(with: Data(String(cString: raw).utf8))
                as? [String: Any]
        else { return nil }
        let removed = (d["removed"] as? NSNumber)?.intValue ?? 0
        let bytes = (d["bytes"] as? NSNumber)?.uint64Value ?? 0
        return (removed, bytes)
    }

    /// {progress_bytes,total_bytes,finished}
    func stats(_ id: Int) -> [String: Any]? {
        guard let raw = lyra_torrent_stats(Int32(id)) else { return nil }
        defer { lyra_string_free(raw) }
        return try? JSONSerialization.jsonObject(with: Data(String(cString: raw).utf8)) as? [String: Any]
    }
}


/// Lossless-first torrent discovery — wraps lyra_search_* over the legal
/// indexes (archive.org etree scope, academic torrents). Cheap rows from
/// `search`; `resolve` fetches the full file list + an addable spec.
final class LyraSearch {
    static let shared = LyraSearch()
    private var handle: UnsafeMutableRawPointer?

    private init() {
        let dir = FileManager.default
            .urls(for: .applicationSupportDirectory, in: .userDomainMask)[0]
            .appendingPathComponent("Lyra/search", isDirectory: true)
        try? FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        handle = dir.path.withCString { lyra_search_new($0) }
    }

    deinit { lyra_search_free(handle) }

    /// (results, provider_errors). Blocks on network — call off-main.
    func search(_ q: String) -> (results: [[String: Any]], issues: [[String: Any]]) {
        guard let handle,
              let raw = q.withCString({ lyra_search(handle, $0) }) else { return ([], []) }
        defer { lyra_string_free(raw) }
        guard let d = try? JSONSerialization.jsonObject(with: Data(String(cString: raw).utf8))
                as? [String: Any] else { return ([], []) }
        return (d["results"] as? [[String: Any]] ?? [],
                d["provider_errors"] as? [[String: Any]] ?? [])
    }

    /// Full file list + addable spec for one result row (pass back the
    /// row dict verbatim — the FFI re-parses it as SearchResult).
    /// Blocks on network — call off-main.
    func resolve(_ result: [String: Any]) -> [String: Any]? {
        guard let handle,
              let j = try? JSONSerialization.data(withJSONObject: result),
              let js = String(data: j, encoding: .utf8),
              let raw = js.withCString({ lyra_search_resolve(handle, $0) })
        else { return nil }
        defer { lyra_string_free(raw) }
        return try? JSONSerialization.jsonObject(with: Data(String(cString: raw).utf8))
            as? [String: Any]
    }

    /// ResolvedTorrent.addable → a `lyra_torrent_add` spec string.
    /// magnet/url pass through; torrent_b64 lands as a temp .torrent file.
    static func addableSpec(_ resolved: [String: Any]) -> String? {
        guard let a = resolved["addable"] as? [String: Any],
              let kind = a["kind"] as? String else { return nil }
        switch kind {
        case "magnet": return a["magnet"] as? String
        case "torrent_url": return a["url"] as? String
        case "torrent_b64":
            guard let b64 = a["data"] as? String,
                  let bytes = Data(base64Encoded: b64) else { return nil }
            let f = FileManager.default.temporaryDirectory
                .appendingPathComponent("lyra-\(UUID().uuidString).torrent")
            do { try bytes.write(to: f) } catch { return nil }
            return f.path
        default: return nil
        }
    }
}


/// Agent-native IPC — NDJSON over a unix socket for `lyra` CLI / lyra-mcp.
/// The server lives in Rust (lyra-ipc); this wraps start/stop + the two
/// state edges: publishState (VM→snapshot) and drainCommands (socket→VM).
final class LyraIPC {
    static let shared = LyraIPC()
    private(set) var running = false

    private init() {}

    /// Bind `<container>/Data/Lyra/control.sock` — matches the CLI's
    /// discovery candidate. Application Support is too deep: the path
    /// would exceed the ~103-byte sun_path bind limit.
    @discardableResult
    func start() -> Bool {
        let appSupport = FileManager.default
            .urls(for: .applicationSupportDirectory, in: .userDomainMask)[0]
        let dir = appSupport.deletingLastPathComponent()
            .deletingLastPathComponent() // → Data/ inside the container
            .appendingPathComponent("Lyra", isDirectory: true)
        try? FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        let db = appSupport.appendingPathComponent("Lyra/library.db").path
        let rc = db.withCString { dbp in
            dir.path.withCString { lyra_ipc_start(dbp, $0) }
        }
        running = rc == 0
        if !running {
            NSLog("lyra-ipc: lyra_ipc_start failed rc=%d dir=%@", rc, dir.path)
        }
        return running
    }

    func stop() { lyra_ipc_stop(); running = false }

    /// VM pushes now-playing/queue/mode state — merged into `state.get`.
    func publishState(_ obj: [String: Any]) {
        guard running,
              let data = try? JSONSerialization.data(withJSONObject: obj),
              let js = String(data: data, encoding: .utf8)
        else { return }
        js.withCString { _ = lyra_ipc_publish_state($0) }
    }

    /// UI-bound ops pushed by socket clients — the VM drains + executes.
    func drainCommands() -> [[String: Any]] {
        guard running, let raw = lyra_ipc_drain_commands() else { return [] }
        defer { lyra_string_free(raw) }
        return (try? JSONSerialization.jsonObject(with: Data(String(cString: raw).utf8)) as? [[String: Any]]) ?? []
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
