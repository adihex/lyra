import SwiftUI
import UniformTypeIdentifiers

/// One scanned library row — mirrors LibraryTrack's camelCase JSON.
struct Track: Identifiable, Hashable {
    enum Source: Hashable {
        case file
        case torrent(id: Int, fileIdx: Int)
    }
    let id: String // path or torrent://id/idx
    let path: String
    var title: String
    var artist: String
    var album: String
    var albumArtist: String
    var duration: Double
    var format: String
    var codec: String
    var trackNumber: Int
    var artworkHash: String?
    var source: Source = .file

    init(_ d: [String: Any]) {
        path = d["path"] as? String ?? ""
        id = path
        title = d["title"] as? String ?? URL(fileURLWithPath: path).deletingPathExtension().lastPathComponent
        artist = d["artist"] as? String ?? "Unknown Artist"
        album = d["album"] as? String ?? "Unknown Album"
        albumArtist = d["albumArtist"] as? String ?? artist
        duration = d["durationSecs"] as? Double ?? 0
        format = (d["format"] as? String ?? "?").uppercased()
        codec = d["codec"] as? String ?? format
        trackNumber = d["trackNumber"] as? Int ?? 0
        artworkHash = d["artworkHash"] as? String
    }

    /// A file inside a torrent — playable via stream-while-downloading.
    init(torrentId: Int, file d: [String: Any]) {
        let idx = d["index"] as? Int ?? 0
        path = "torrent://\(torrentId)/\(idx)"
        id = path
        let name = d["path"] as? String ?? "file \(idx)"
        title = URL(fileURLWithPath: name).deletingPathExtension().lastPathComponent
        artist = "torrent #\(torrentId)"
        album = ""
        albumArtist = ""
        duration = 0
        format = URL(fileURLWithPath: name).pathExtension.uppercased()
        codec = format
        trackNumber = idx + 1
        source = .torrent(id: torrentId, fileIdx: idx)
    }

    /// Extensions symphonia can actually decode — filters nfo/txt/jpg out
    /// of torrent file lists.
    static let audioExts: Set<String> = [
        "flac", "mp3", "m4a", "aac", "aiff", "aif", "wav", "wave",
        "ogg", "oga", "opus", "wv", "ape", "shn", "alac", "mp4", "mka", "mkv",
    ]
    var isAudio: Bool { Track.audioExts.contains(format.lowercased()) }
}

/// ISO-center EQ bands for the 10-band parametric.
let eqFreqs: [Float] = [31, 62, 125, 250, 500, 1000, 2000, 4000, 8000, 16000]

/// One active torrent in the session — tracked so the UI can offer a
/// remove/purge CTA (downloaded data lives on disk until wiped).
struct TorrentInfo: Identifiable, Hashable {
    let id: Int
    let name: String
}

/// View model. Built with CLT swiftc (no Xcode) — bare `@State` is a
/// SwiftUIMacros convenience macro unavailable here; ObservableObject +
/// @Published + @StateObject are plain property wrappers and work.
final class ViewModel: ObservableObject {
    static let shared = ViewModel() // one VM — window, menu, mini player

    @Published var selection: SidebarItem? = .library
    @Published var tracks: [Track] = []
    @Published var selectedTracks = Set<Track.ID>()
    @Published var sortOrder = [KeyPathComparator(\Track.trackNumber)]
    @Published var columnVis = NavigationSplitViewVisibility.all
    @Published var query = ""
    @Published var scanning = false
    @Published var scanStatus = ""
    @Published var libraryRoot: String?
    @Published var contentID = UUID() // bump → Table skips dataset diffing
    @Published var magnetInput = ""
    @Published var showMagnetEntry = false
    @Published var addingTorrent = false
    @Published var torrents: [TorrentInfo] = []
    @Published var orphans: [(name: String, bytes: UInt64)] = []
    @Published var pairCode: String?
    @Published var pairFp = ""
    @Published var pairedCount = 0
    @Published var pairedDevices: [(id: String, name: String)] = []
    @Published var exclusiveOutput = false

    // now playing
    @Published var hoveredTrack: String?
    @Published var current: Track?
    @Published var lastError: String?
    @Published var playing = false
    @Published var position: Double = 0
    @Published var scrubbing = false
    @Published var displayPosition: Double = 0
    @Published var volume: Float = 1.0 {
        didSet { LyraPlayer.shared.setVolume(volume * volume) } // perceptual taper
    }

    // eq gains per band, dB
    @Published var eq: [Float] = Array(repeating: 0, count: 10)
    @Published var eqCurve: (freqs: [Float], db: [Float]) = ([], [])

    // viz — raw buffer path, no JSON at 60Hz
    @Published var bands: [Float] = Array(repeating: 0, count: 48)
    @Published var clip = false

    // viz surface — frame compositor + mode catalogue state (Viz/).
    // Selected mode persists across launches; rawValue 0 (unset) → Bars.
    let viz = VizRuntime()
    @Published var vizMode: VizMode = VizMode(
        rawValue: UserDefaults.standard.integer(forKey: "vizMode")) ?? .bars {
        didSet { UserDefaults.standard.set(vizMode.rawValue, forKey: "vizMode") }
    }
    /// Which face the Visuals stage shows — flipped from the transport
    /// bar (mini viz → viz, art tile → art) or the pane's face chips.
    @Published var stageFace = StageFace(
        rawValue: UserDefaults.standard.integer(forKey: "stageFace")) ?? .viz {
        didSet { UserDefaults.standard.set(stageFace.rawValue, forKey: "stageFace") }
    }

    private var timer: Timer?
    private var vizBuf: UnsafeMutableBufferPointer<Float>
    private var accessedFolders: [URL] = []

    init() {
        vizBuf = .allocate(capacity: 48)
        restoreBookmarks()
        MediaKeys.shared.hook(
            getState: { (LyraPlayer.shared.isPlaying, LyraPlayer.shared.position) },
            onToggle: { [weak self] in self?.toggle() },
            onNext: { [weak self] in self?.next() },
            onPrev: { [weak self] in self?.prev() },
            onSeek: { pos in LyraPlayer.shared.seek(pos) }
        )
        // Persistent library: rows from the last sync load instantly.
        DispatchQueue.global(qos: .userInitiated).async { [weak self] in
            let ts = LyraLibrary.shared.tracks.map(Track.init)
            DispatchQueue.main.async {
                self?.tracks = ts
                self?.contentID = UUID()
            }
        }
        exclusiveOutput = LyraPlayer.shared.exclusiveOutput
        refreshDevices()
        hookKeys()
        // Session init + restore block on disk/metadata — off the main
        // thread. Restored torrents rebuild their rows + chips.
        DispatchQueue.global(qos: .userInitiated).async { [weak self] in
            self?.refreshTorrents(rebuildRows: true)
            self?.refreshOrphans()
        }
    }
    deinit { vizBuf.deallocate() }

    /// Table sorts via KeyPathComparator — the sortable custom-column
    /// overload only compiles when sortOrder is bound (research-confirmed).
    var sortedTracks: [Track] {
        let base = query.isEmpty ? tracks
            : tracks.filter { t in
                t.title.localizedCaseInsensitiveContains(query)
                || t.artist.localizedCaseInsensitiveContains(query)
                || t.album.localizedCaseInsensitiveContains(query)
            }
        return base.sorted(using: sortOrder)
    }

    var currentIndex: Int? { sortedTracks.firstIndex { $0.id == current?.id } }

    // ── Custom-table selection/nav (system Table replaced — its selection
    // pill can't be tinted; we draw our own square accent selection) ────
    private var lastSelectedIdx: Int?
    private var keyMonitor: Any?

    /// Click-select: plain = replace, ⌘ = toggle, ⇧ = range from last click.
    func selectTrack(_ id: Track.ID) {
        let mods = NSEvent.modifierFlags
        let idx = sortedTracks.firstIndex { $0.id == id }
        if mods.contains(.command) {
            if selectedTracks.contains(id) { selectedTracks.remove(id) }
            else { selectedTracks.insert(id) }
        } else if mods.contains(.shift), let last = lastSelectedIdx, let idx {
            selectedTracks = Set((min(last, idx)...max(last, idx)).map { sortedTracks[$0].id })
        } else {
            selectedTracks = [id]
        }
        lastSelectedIdx = idx
    }

    /// Arrow/return nav — a local key monitor because focusable-view key
    /// routing doesn't reach our custom rows. Skips when a text field
    /// (the filter box) owns the responder chain.
    func hookKeys() {
        keyMonitor = NSEvent.addLocalMonitorForEvents(matching: .keyDown) { [weak self] e in
            guard let self, self.selection == .library,
                  !(NSApp.keyWindow?.firstResponder is NSTextView)
            else { return e }
            switch Int(e.keyCode) {
            case 125: self.moveSelection(1)
            case 126: self.moveSelection(-1)
            case 36: self.playSelected()
            default: return e
            }
            return nil
        }
    }

    func moveSelection(_ d: Int) {
        let rows = sortedTracks
        guard !rows.isEmpty else { return }
        let cur = rows.firstIndex { selectedTracks.contains($0.id) } ?? (d > 0 ? -1 : rows.count)
        let next = min(max(cur + d, 0), rows.count - 1)
        selectedTracks = [rows[next].id]
        lastSelectedIdx = next
    }

    func playSelected() {
        if let t = sortedTracks.first(where: { selectedTracks.contains($0.id) }) { play(t) }
    }

    /// Sort-header toggle — same key flips order, new key resets forward.
    func toggleSort<V: Comparable>(_ kp: KeyPath<Track, V>) {
        if vm_isSorted(kp) {
            let rev = sortOrder.first?.order == .forward
            sortOrder = [KeyPathComparator(kp, order: rev ? .reverse : .forward)]
        } else {
            sortOrder = [KeyPathComparator(kp)]
        }
    }

    private func vm_isSorted<V: Comparable>(_ kp: KeyPath<Track, V>) -> Bool {
        sortOrder.first?.keyPath == kp
    }

    func startPolling() {
        timer?.invalidate()
        timer = Timer.scheduledTimer(withTimeInterval: 1.0 / 30, repeats: true) { [weak self] _ in
            guard let self else { return }
            let p = LyraPlayer.shared
            DispatchQueue.main.async {
                self.playing = p.isPlaying
                if !self.scrubbing {
                    self.position = p.position
                    self.displayPosition = p.position
                }
                _ = p.vizBands(into: self.vizBuf)
                self.bands = Array(self.vizBuf)
                if let v = p.viz { self.clip = v["clip"] as? Bool ?? false }
            }
        }
    }

    func restoreBookmarks() {
        // Persisted security-scoped bookmarks re-grant access to library
        // folders across launches — without this the sandbox denies reads
        // of every persisted path.
        let dict = UserDefaults.standard.dictionary(forKey: "folderBookmarks") as? [String: Data] ?? [:]
        for (path, data) in dict {
            var stale = false
            guard let url = try? URL(resolvingBookmarkData: data,
                                     options: .withSecurityScope,
                                     bookmarkDataIsStale: &stale) else { continue }
            if url.startAccessingSecurityScopedResource() {
                accessedFolders.append(url)
            }
            if stale { storeBookmark(for: URL(fileURLWithPath: path)) }
        }
    }

    private func storeBookmark(for url: URL) {
        guard let data = try? url.bookmarkData(options: .withSecurityScope)
        else { return }
        var dict = UserDefaults.standard.dictionary(forKey: "folderBookmarks") as? [String: Data] ?? [:]
        dict[url.path] = data
        UserDefaults.standard.set(dict, forKey: "folderBookmarks")
    }

    func scanFolder() {
        let panel = NSOpenPanel()
        panel.canChooseFiles = true
        panel.canChooseDirectories = true
        panel.allowsMultipleSelection = true
        panel.message = "Pick folders or audio files"
        guard panel.runModal() == .OK else { return }
        let urls = panel.urls
        guard !urls.isEmpty else { return }
        var dirs: [String] = []
        var files: [String] = []
        for url in urls {
            storeBookmark(for: url)
            if url.hasDirectoryPath { dirs.append(url.path) } else { files.append(url.path) }
        }
        if let dir = dirs.first { libraryRoot = dir }
        scanning = true
        scanStatus = ""
        DispatchQueue.global(qos: .userInitiated).async {
            // Incremental sync into the persistent DB — only mtime-changed
            // files get re-probed; then reload all rows. File picks go through
            // sync_files which never prunes.
            var probed = 0, skipped = 0, pruned = 0
            for dir in dirs {
                if let s = LyraLibrary.shared.syncDir(dir) {
                    probed += s["probed"] as? Int ?? 0
                    skipped += s["skipped"] as? Int ?? 0
                    pruned += s["pruned"] as? Int ?? 0
                }
            }
            if !files.isEmpty, let s = LyraLibrary.shared.syncFiles(files) {
                probed += s["probed"] as? Int ?? 0
                skipped += s["skipped"] as? Int ?? 0
            }
            let ts = LyraLibrary.shared.tracks.map(Track.init)
            DispatchQueue.main.async {
                self.tracks = ts
                self.contentID = UUID() // force no-diff Table rebuild
                self.scanning = false
                self.scanStatus = "\(ts.count) tracks — probed \(probed), skipped \(skipped), pruned \(pruned)"
            }
        }
    }

    /// NSOpenPanel picker for .torrent files — the only sandbox-clean way
    /// to reach e.g. ~/Downloads (typed paths outside the container are
    /// denied; a panel pick grants read for the session).
    func pickTorrentFile() {
        let panel = NSOpenPanel()
        panel.canChooseFiles = true
        panel.canChooseDirectories = false
        panel.allowsMultipleSelection = false
        panel.allowedContentTypes = [UTType(filenameExtension: "torrent") ?? .data]
        guard panel.runModal() == .OK, let url = panel.url else { return }
        magnetInput = url.path
        showMagnetEntry = true
        addTorrent()
    }

    /// Accepts a magnet URI or a local .torrent path. Blocks in the FFI on
    /// magnet metadata resolve — runs on a background queue; audio files in
    /// the torrent land in the table as playable rows.
    func addTorrent() {
        let spec = magnetInput.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !spec.isEmpty, !addingTorrent else { return }
        addingTorrent = true
        lastError = nil
        DispatchQueue.global(qos: .userInitiated).async {
            let id = LyraTorrent.shared.add(spec)
            let rows = id >= 0 ? LyraTorrent.shared.files(id) : []
            let tname = LyraTorrent.shared.list().first { $0.id == id }?.name ?? ""
            DispatchQueue.main.async {
                self.addingTorrent = false
                if id < 0 {
                    self.lastError = "Torrent add failed (\(id)) — bad magnet/file or no peers for metadata"
                    return
                }
                self.magnetInput = ""
                self.showMagnetEntry = false
                var newTracks = rows
                    .map { Track(torrentId: id, file: $0) }
                    .filter(\.isAudio)
                // Sequential numbering — file idx can skip (nfo/jpg filtered),
                // and the torrent name is the closest thing to an album tag.
                for i in newTracks.indices {
                    newTracks[i].trackNumber = i + 1
                    newTracks[i].album = tname
                }
                if newTracks.isEmpty {
                    self.lastError = "Torrent #\(id): no playable audio files (\(rows.count) total)"
                } else {
                    self.tracks.append(contentsOf: newTracks)
                    self.contentID = UUID()
                    self.refreshTorrents()
                    self.scanStatus = "torrent #\(id): \(newTracks.count) playable of \(rows.count) files — streams on demand"
                    self.probeTorrentDurations(newTracks)
                }
            }
        }
    }

    /// Fill in durations for fresh torrent rows — each probe reads the
    /// file's header region only (piece 0 fetches on demand), serially in
    /// the background so a 20-file album doesn't hammer the swarm.
    private func probeTorrentDurations(_ newTracks: [Track]) {
        DispatchQueue.global(qos: .utility).async {
            for t in newTracks {
                guard case .torrent(let tid, let fidx) = t.source else { continue }
                guard let info = LyraTorrent.shared.probe(tid, file: fidx),
                      let dur = info["duration_secs"] as? Double, dur > 0
                else { continue }
                DispatchQueue.main.async {
                    if let i = self.tracks.firstIndex(where: { $0.id == t.id }) {
                        self.tracks[i].duration = dur
                    }
                    if self.current?.id == t.id { self.current?.duration = dur }
                }
            }
        }
    }

    /// Rebuild the torrent list from the session (the source of truth —
    /// rqbit JSON persistence restores torrents across relaunches with
    /// stable ids). rebuildRows also reconstructs table rows for torrents
    /// restored on launch, whose rows were never in this process's RAM.
    /// Safe from any thread — marshals UI updates to main.
    func refreshTorrents(rebuildRows: Bool = false) {
        let work = {
            let list = LyraTorrent.shared.list()
            var newRows: [Track] = []
            if rebuildRows {
                DispatchQueue.main.sync {
                    self.torrents = list
                }
                let known = DispatchQueue.main.sync {
                    Set(self.tracks.compactMap { t -> Int? in
                        guard case .torrent(let tid, _) = t.source else { return nil }
                        return tid
                    })
                }
                for info in list where !known.contains(info.id) {
                    let rows = LyraTorrent.shared.files(info.id)
                    var ts = rows
                        .map { Track(torrentId: info.id, file: $0) }
                        .filter(\.isAudio)
                    for i in ts.indices {
                        ts[i].trackNumber = i + 1
                        ts[i].album = info.name
                    }
                    newRows.append(contentsOf: ts)
                }
            }
            DispatchQueue.main.async {
                self.torrents = list
                if !newRows.isEmpty {
                    self.tracks.append(contentsOf: newRows)
                    self.contentID = UUID()
                    self.probeTorrentDurations(newRows)
                }
            }
        }
        if Thread.isMainThread { DispatchQueue.global(qos: .userInitiated).async(execute: work) }
        else { work() }
    }

    func refreshOrphans() {
        let work = {
            let os = LyraTorrent.shared.orphans()
            DispatchQueue.main.async { self.orphans = os }
        }
        if Thread.isMainThread { DispatchQueue.global(qos: .utility).async(execute: work) }
        else { work() }
    }

    /// Wipe leftover download folders not owned by any managed torrent.
    func purgeOrphans() {
        let work = {
            let res = LyraTorrent.shared.purgeOrphans()
            DispatchQueue.main.async {
                if let r = res {
                    self.orphans = []
                    self.scanStatus = "cleaned \(r.removed) leftover item\(r.removed == 1 ? "" : "s") — freed \(self.fmtBytes(r.bytes))"
                } else {
                    self.lastError = "Orphan cleanup failed"
                }
            }
        }
        if Thread.isMainThread { DispatchQueue.global(qos: .utility).async(execute: work) }
        else { work() }
    }

    func fmtBytes(_ b: UInt64) -> String {
        let gb = Double(b) / 1_073_741_824
        if gb >= 0.1 { return String(format: "%.1f GB", gb) }
        return String(format: "%.0f MB", Double(b) / 1_048_576)
    }

    /// Torrent id embedded in a track path ("torrent://<id>/<idx>").
    private func torrentId(of path: String) -> Int? {
        guard path.hasPrefix("torrent://") else { return nil }
        return Int(path.dropFirst("torrent://".count).split(separator: "/").first ?? "")
    }

    /// Remove a torrent from the session. `deleteFiles` also wipes its
    /// downloaded data from disk. Stops playback if the current track
    /// comes from this torrent.
    func removeTorrent(_ id: Int, deleteFiles: Bool) {
        if let cur = current, case .torrent(let tid, _) = cur.source, tid == id {
            LyraPlayer.shared.stop()
            current = nil
        }
        guard LyraTorrent.shared.remove(id, deleteFiles: deleteFiles) else {
            lastError = "Failed to remove torrent #\(id)"
            return
        }
        let removedIds = Set(tracks.compactMap { t -> String? in
            guard case .torrent(let tid, _) = t.source, tid == id else { return nil }
            return t.id
        })
        tracks.removeAll { removedIds.contains($0.id) }
        selectedTracks.subtract(removedIds)
        torrents.removeAll { $0.id == id }
        contentID = UUID()
        lastError = nil
        scanStatus = deleteFiles
            ? "torrent #\(id) removed — downloaded data deleted"
            : "torrent #\(id) removed — files kept on disk"
        refreshOrphans() // keep-files removal leaves an orphan folder
    }

    /// NSAlert confirm: keep files (cheap) vs delete downloaded data
    /// (reclaim disk space — what we did manually for the 6.3GB Floyd set).
    func confirmRemoveTorrent(_ t: TorrentInfo) {
        let alert = NSAlert()
        alert.messageText = "Remove “\(t.name)”?"
        alert.informativeText = "The torrent leaves the session and its rows disappear from the library. Choose whether to also delete the data already downloaded to disk."
        alert.addButton(withTitle: "Delete Downloaded Files")
        alert.addButton(withTitle: "Keep Files")
        alert.addButton(withTitle: "Cancel")
        alert.alertStyle = .warning
        switch alert.runModal() {
        case .alertFirstButtonReturn: removeTorrent(t.id, deleteFiles: true)
        case .alertSecondButtonReturn: removeTorrent(t.id, deleteFiles: false)
        default: break
        }
    }

    /// Context-menu entry point — resolve the torrent id from a track id.
    func confirmRemoveTorrent(forTrackId trackId: String) {
        guard let tid = torrentId(of: trackId) else { return }
        let info = torrents.first { $0.id == tid } ?? TorrentInfo(id: tid, name: "torrent #\(tid)")
        confirmRemoveTorrent(info)
    }

    func play(_ t: Track) {
        let ok: Bool
        switch t.source {
        case .file:
            ok = LyraPlayer.shared.play(path: t.path)
        case .torrent(let id, let idx):
            ok = LyraPlayer.shared.playTorrent(id, file: idx)
        }
        if ok {
            current = t
            lastError = nil
            position = 0
            pushEQ()
            publishNowPlaying(t)
            // Duration fallback — playback fetches piece 0 anyway, so the
            // header probe rides along on data the swarm already sent.
            if t.duration == 0, case .torrent = t.source {
                probeTorrentDurations([t])
            }
        } else {
            lastError = "Cannot open \(t.title)"
        }
    }

    func playSelection(_ ids: Set<Track.ID>) {
        guard let id = ids.first,
              let t = tracks.first(where: { $0.id == id }) else { return }
        play(t)
    }

    func toggle() {
        let p = LyraPlayer.shared
        if p.isPlaying { p.pause() }
        else if p.canResume { p.resume() }
        else if let t = current ?? sortedTracks.first { play(t) }
        MediaKeys.shared.refreshState()
    }

    func next() { step(1) }
    func prev() { step(-1) }
    private func step(_ d: Int) {
        guard let i = currentIndex, sortedTracks.indices.contains(i + d) else { return }
        play(sortedTracks[i + d])
    }
    func seekBy(_ d: Double) { LyraPlayer.shared.seek(position + d) }

    func scrubEnded() {
        scrubbing = false
        LyraPlayer.shared.seek(displayPosition)
        position = displayPosition
    }

    func applyEQ(_ band: Int) {
        LyraPlayer.shared.setBand(band, freq: eqFreqs[band], q: 0.9,
                                  gainDb: eq[band], peaking: band != 0)
        refreshCurve()
    }
    func pushEQ() {
        for (i, f) in eqFreqs.enumerated() {
            LyraPlayer.shared.setBand(i, freq: f, q: 0.9, gainDb: eq[i],
                                      peaking: i != 0)
        }
        refreshCurve()
    }
    func refreshCurve() {
        // specs update on the worker thread — re-read after it lands
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.08) {
            if let r = LyraPlayer.shared.eqResponse { self.eqCurve = r }
        }
    }

    /// Switch the output path. The FFI swaps in a fresh idle engine — if
    /// music was playing, restart the track and restore position (seek is
    /// channel-ordered behind play, so it lands after the decoder opens).
    func setExclusiveOutput(_ on: Bool) {
        let wasPlaying = LyraPlayer.shared.isPlaying
        let pos = LyraPlayer.shared.position
        let track = current
        if LyraPlayer.shared.setExclusiveOutput(on) {
            exclusiveOutput = on
            UserDefaults.standard.set(on, forKey: "exclusiveOutput")
            lastError = nil
            if wasPlaying, let t = track {
                play(t)
                if pos > 1 { LyraPlayer.shared.seek(pos) }
            }
        } else {
            exclusiveOutput = LyraPlayer.shared.exclusiveOutput
            lastError = on
                ? "Exclusive output unavailable (device busy or not stereo) — staying on shared"
                : "Couldn't return to shared output"
        }
    }

    func refreshDevices() {
        pairedDevices = LyraRemote.shared.devices
        pairedCount = pairedDevices.count
    }

    func revokeDevice(_ id: String) {
        if LyraRemote.shared.revoke(id) { refreshDevices() }
    }

    func publishNowPlaying(_ t: Track) {
        MediaKeys.shared.publish(title: t.title, artist: t.artist,
                                 album: t.album, duration: t.duration,
                                 artworkHash: t.artworkHash)
    }

    func fmt(_ s: Double) -> String {
        let t = Int(s)
        return String(format: "%d:%02d", t / 60, t % 60)
    }
}

/// The focused pane's two faces — the visualizer and the album-art stage
/// are flip sides of the same card.
enum StageFace: Int { case viz, art }

enum SidebarItem: String, CaseIterable, Identifiable {
    case library = "Library"
    case eq = "Equalizer"
    case visuals = "Visuals"
    case remote = "Remote"
    var id: String { rawValue }
    var icon: String {
        switch self {
        case .library: "music.note.list"
        case .eq: "slider.horizontal.3"
        case .visuals: "waveform"
        case .remote: "iphone.radiowaves.left.and.right"
        }
    }
}

struct ContentView: View {
    @ObservedObject private var vm = ViewModel.shared

    var body: some View {
        NavigationSplitView(columnVisibility: $vm.columnVis) {
            // Manual nav — List's selection pill can't be tinted (system
            // accent only); buttons give us the square accent highlight.
            VStack(alignment: .leading, spacing: 2) {
                ForEach(SidebarItem.allCases) { item in
                    Button { vm.selection = item } label: {
                        HStack(spacing: 8) {
                            Image(systemName: item.icon)
                                .frame(width: 18)
                            Text(item.rawValue)
                        }
                        .font(.uiBodyStrong)
                        .foregroundStyle(vm.selection == item ? .white : Ui.ink)
                        .frame(maxWidth: .infinity, alignment: .leading)
                        .padding(.horizontal, Ui.s12)
                        .padding(.vertical, 7)
                        .background(vm.selection == item ? Ui.accent : Color.clear)
                        .contentShape(Rectangle())
                    }
                    .buttonStyle(.plain)
                }
                Spacer()
            }
            .padding(Ui.s8)
            .background(Ui.bg)
            .navigationSplitViewColumnWidth(min: 150, ideal: 190, max: 320)
        } detail: {
            VStack(spacing: 0) {
                detailView
                    .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
                nowPlayingBar
            }
            .frame(maxWidth: .infinity, maxHeight: .infinity)
            .background(Ui.bg)
            .tint(Ui.accent)
            .toolbarBackground(Ui.bg, for: .windowToolbar)
            .toolbarBackground(.visible, for: .windowToolbar)
        }
        .tint(Ui.accent)
        .frame(minWidth: 780, minHeight: 560)
    }

    @ViewBuilder private var detailView: some View {
        switch vm.selection {
        case .library: libraryPane
        case .eq: eqPane
        case .visuals: VisualsPane()
        case .remote: remotePane
        case .none: Text("Select a section").foregroundStyle(Ui.inkSoft)
        }
    }

    // ── Library ──────────────────────────────────────────────────────────
    private var libraryPane: some View {
        VStack(alignment: .leading, spacing: Ui.s16) {
            HStack {
                Text("Library").font(.uiTitle)
                    .foregroundStyle(Ui.ink)
                // explicit search field — .searchable(placement:.toolbar)
                // landed in the collapsed sidebar strip; an inline field is
                // deterministic about where it lives.
                HStack(spacing: 4) {
                    Image(systemName: "magnifyingglass").foregroundStyle(Ui.inkSoft)
                    TextField("Filter…", text: $vm.query)
                        .textFieldStyle(.plain)
                }
                .padding(.horizontal, 10).padding(.vertical, 6)
                .background(Ui.surface)
                .overlay(Rectangle().stroke(Ui.border, lineWidth: 1))
                .frame(minWidth: 90, idealWidth: 200, maxWidth: 260)
                Spacer()
                if let root = vm.libraryRoot {
                    Text(URL(fileURLWithPath: root).lastPathComponent)
                        .foregroundStyle(Ui.inkSoft)
                        .lineLimit(1)
                        .layoutPriority(-1) // truncates first when narrow
                }
                Button {
                    vm.showMagnetEntry.toggle()
                } label: {
                    Label("Add torrent", systemImage: "link.badge.plus")
                }
                .buttonStyle(.sharp)
                .help("Paste a magnet URI or pick up a .torrent file — audio streams on demand")
                Button(vm.scanning ? "Scanning…" : "Scan folder…") { vm.scanFolder() }
                    .disabled(vm.scanning)
                    .buttonStyle(.sharpProminent)
            }
            if vm.showMagnetEntry {
                HStack(spacing: 8) {
                    Image(systemName: "link").foregroundStyle(Ui.inkSoft)
                    TextField("magnet:?xt=… or /path/to/file.torrent", text: $vm.magnetInput)
                        .textFieldStyle(.plain)
                        .padding(.horizontal, 8).padding(.vertical, 5)
                        .background(Ui.bg)
                        .overlay(Rectangle().stroke(Ui.border, lineWidth: 1))
                        .onSubmit { vm.addTorrent() }
                    Button("Add") { vm.addTorrent() }
                        .disabled(vm.magnetInput.isEmpty || vm.addingTorrent)
                        .buttonStyle(.sharpProminent)
                    Button("Browse…") { vm.pickTorrentFile() }
                        .disabled(vm.addingTorrent)
                        .buttonStyle(.sharp)
                    if vm.addingTorrent {
                        ProgressView().controlSize(.small)
                        Text("resolving…").font(.uiCaption).foregroundStyle(Ui.inkSoft)
                    }
                }
                .uiCard()
                .uiElevated()
            }
            if !vm.torrents.isEmpty {
                HStack(spacing: 8) {
                    ForEach(vm.torrents) { t in
                        HStack(spacing: 6) {
                            Image(systemName: "arrow.down.circle.fill")
                                .foregroundStyle(Ui.mint)
                            Text(t.name)
                                .font(.uiCaption)
                                .foregroundStyle(Ui.ink)
                                .lineLimit(1)
                            Button { vm.confirmRemoveTorrent(t) } label: {
                                Image(systemName: "xmark.circle.fill")
                                    .foregroundStyle(Ui.inkSoft)
                            }
                            .buttonStyle(.borderless)
                            .help("Remove torrent — optionally delete downloaded data")
                        }
                        .padding(.horizontal, 10).padding(.vertical, 5)
                        .background(Ui.mint.opacity(0.14))
                        .overlay(Rectangle().stroke(Ui.mint.opacity(0.4), lineWidth: 1))
                    }
                }
            }
            if !vm.orphans.isEmpty {
                HStack(spacing: 8) {
                    Image(systemName: "trash").foregroundStyle(Ui.inkSoft)
                    Text("\(vm.orphans.count) leftover item\(vm.orphans.count == 1 ? "" : "s") · \(vm.fmtBytes(vm.orphans.reduce(0) { $0 + $1.bytes }))")
                        .font(.uiCaption).foregroundStyle(Ui.inkSoft)
                    Button("Clean up") { vm.purgeOrphans() }
                        .buttonStyle(.sharp)
                        .help("Delete download folders left behind by removed torrents")
                }
            }
            if !vm.scanStatus.isEmpty {
                Text(vm.scanStatus).font(.uiCaption).foregroundStyle(Ui.inkSoft)
            }
            if vm.tracks.isEmpty {
                Spacer()
                VStack(spacing: 10) {
                    Text("🪐").font(.system(size: 44))
                    Text("No tunes yet").font(.uiHeadline)
                        .foregroundStyle(Ui.ink)
                    Text("Scan a folder or drop a magnet to fill the sky with music.")
                        .font(.uiCaption)
                        .foregroundStyle(Ui.inkSoft)
                }
                .frame(maxWidth: .infinity)
                Spacer()
            } else {
                trackTable
                    .frame(maxWidth: .infinity, maxHeight: .infinity)
                HStack {
                    Text("\(vm.sortedTracks.count) tracks")
                        .font(.uiCaption).foregroundStyle(Ui.inkSoft)
                    Spacer()
                    Button("Play selected") { vm.playSelection(vm.selectedTracks) }
                        .disabled(vm.selectedTracks.isEmpty)
                        .buttonStyle(.sharp)
                }
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .padding(Ui.s20)
        .onAppear { vm.startPolling() }
    }

    // ── Track table ─────────────────────────────────────────────────────
    // Custom table — the system Table's selection pill renders in the
    // untintable system accent (no Assets.car without Xcode), so we draw
    // our own square terracotta selection. Columns are proportional:
    // #/Time/Codec fixed, Title flexes 2× Artist/Album.
    private var trackTable: some View {
        GeometryReader { geo in
            let w = trackCols(geo.size.width)
            VStack(spacing: 0) {
                trackHeader(w)
                Ui.border.frame(height: 1)
                ScrollView {
                    LazyVStack(spacing: 0) {
                        ForEach(vm.sortedTracks) { t in
                            trackRow(t, widths: w)
                        }
                    }
                }
            }
        }
        .id(vm.contentID)
        .uiCard(padding: 0)
    }

    private func trackCols(_ total: CGFloat) -> [CGFloat] {
        let flex = max(total - 154, 220) // 34 + 56 + 64 fixed
        return [34, flex * 0.5, flex * 0.25, flex * 0.25, 56, 64]
    }

    private func trackHeader(_ w: [CGFloat]) -> some View {
        HStack(spacing: 0) {
            sortCell("#", \.trackNumber, w[0])
            sortCell("Title", \.title, w[1])
            sortCell("Artist", \.artist, w[2])
            sortCell("Album", \.album, w[3])
            sortCell("Time", \.duration, w[4])
            sortCell("Codec", \.codec, w[5])
        }
        .padding(.vertical, 7)
        .background(Ui.surface)
    }

    private func sortCell<V: Comparable>(_ label: String,
                                         _ kp: KeyPath<Track, V>,
                                         _ w: CGFloat) -> some View {
        Button { vm.toggleSort(kp) } label: {
            HStack(spacing: 4) {
                Text(label).font(.uiHeadline)
                if vm.sortOrder.first?.keyPath == kp {
                    Image(systemName: vm.sortOrder.first?.order == .forward
                          ? "chevron.up" : "chevron.down")
                        .font(.system(size: 8, weight: .bold))
                }
            }
            .foregroundStyle(Ui.inkSoft)
            .frame(width: w - 16, alignment: .leading)
            .padding(.horizontal, 8)
        }
        .buttonStyle(.plain)
    }

    private func trackRow(_ t: Track, widths w: [CGFloat]) -> some View {
        let sel = vm.selectedTracks.contains(t.id)
        let soft: Color = sel ? .white.opacity(0.85) : Ui.inkSoft
        return HStack(spacing: 0) {
            cell(t.trackNumber > 0 ? "\(t.trackNumber)" : "—", w[0], .uiCaption,
                 sel ? .white.opacity(0.8) : Ui.inkSoft)
            HStack(spacing: 6) {
                ArtImage(hash: t.artworkHash, label: t.album, size: 18)
                Text(t.title).font(.uiBody).foregroundStyle(sel ? .white : Ui.ink)
                    .lineLimit(1)
            }
            .frame(width: w[1] - 16, alignment: .leading)
            .padding(.horizontal, 8)
            cell(t.artist, w[2], .uiBody, soft)
            cell(t.album, w[3], .uiBody, soft)
            cell(vm.fmt(t.duration), w[4], .uiMono, soft)
            Text(t.codec).font(.uiMicro)
                .foregroundStyle(sel ? .white : Ui.indigo)
                .padding(.horizontal, 6).padding(.vertical, 1)
                .background(sel ? Color.white.opacity(0.16) : Ui.indigo.opacity(0.12))
                .overlay(Rectangle().stroke(sel ? Color.white.opacity(0.5) : Ui.indigo.opacity(0.3), lineWidth: 1))
                .frame(width: w[5], alignment: .center)
        }
        .frame(height: 27)
        .background(sel ? Ui.accent
                    : vm.hoveredTrack == t.id ? Ui.ink.opacity(0.05) : Color.clear)
        .contentShape(Rectangle())
        .onHover { h in
            let id = h ? t.id : nil
            if vm.hoveredTrack != id { vm.hoveredTrack = id }
        }
        // Single tap only — a paired count:2 gesture delays recognition
        // ~300ms to disambiguate. clickCount detects the double-click
        // without the wait.
        .onTapGesture {
            vm.selectTrack(t.id)
            if (NSApp.currentEvent?.clickCount ?? 1) >= 2 { vm.play(t) }
        }
        .contextMenu { trackMenu(t) }
    }

    private func cell(_ s: String, _ w: CGFloat, _ f: Font, _ c: Color) -> some View {
        Text(s).font(f).foregroundStyle(c)
            .lineLimit(1)
            .frame(width: w - 16, alignment: .leading)
            .padding(.horizontal, 8)
    }

    private func trackMenu(_ t: Track) -> some View {
        Group {
            Button("Play") { vm.play(t) }
            Divider()
            if t.id.hasPrefix("torrent://") {
                Button("Remove torrent…") { vm.confirmRemoveTorrent(forTrackId: t.id) }
            } else {
                Button("Reveal in Finder") {
                    NSWorkspace.shared.activateFileViewerSelecting(
                        [URL(fileURLWithPath: t.id)])
                }
            }
        }
    }

    // ── Now playing bar ──────────────────────────────────────────────────
    private var nowPlayingBar: some View {
        VStack(spacing: 6) {
            // seek: engine clock drives; drag owns it until release
            Slider(
                value: $vm.displayPosition,
                in: 0...max(vm.current?.duration ?? 1, 1),
                onEditingChanged: { editing in
                    if editing { vm.scrubbing = true } else { vm.scrubEnded() }
                }
            )
            HStack(spacing: 10) {
                Button { vm.selection = .visuals; vm.stageFace = .art } label: {
                    ArtImage(hash: vm.current?.artworkHash,
                             label: vm.current?.album ?? "", size: 34, px: 256)
                        .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
                .onHover { h in
                    if h { NSCursor.pointingHand.push() } else { NSCursor.pop() }
                }
                .help("Artwork — \(vm.current?.album ?? "nothing playing")")
                VStack(alignment: .leading) {
                    Text(vm.current?.title ?? "Nothing playing").font(.uiHeadline).lineLimit(1)
                        .foregroundStyle(vm.current == nil ? Ui.inkSoft : Ui.ink)
                    Text(vm.lastError ?? [vm.current?.artist, vm.current?.album].compactMap { $0 }.joined(separator: " — "))
                        .font(.uiCaption)
                        .foregroundStyle(vm.lastError != nil ? .red : Ui.inkSoft)
                        .lineLimit(1)
                }
                .frame(minWidth: 60, maxWidth: 200, alignment: .leading)
                Spacer()
                Button { vm.prev() } label: {
                    Image(systemName: "backward.fill").sharpIconBox()
                }
                .buttonStyle(.plain)
                Button { vm.toggle() } label: {
                    Image(systemName: vm.playing ? "pause.fill" : "play.fill")
                        .font(.system(size: 15, weight: .bold))
                        .foregroundStyle(.white)
                        .frame(width: 34, height: 34)
                        .background(Ui.accent)
                }
                .buttonStyle(.plain)
                Button { LyraPlayer.shared.stop() } label: {
                    Image(systemName: "stop.fill").sharpIconBox()
                }
                .buttonStyle(.plain)
                Button { vm.next() } label: {
                    Image(systemName: "forward.fill").sharpIconBox()
                }
                .buttonStyle(.plain)
                Text("\(vm.fmt(vm.displayPosition)) / \(vm.fmt(vm.current?.duration ?? 0))")
                    .font(.uiMono)
                    .foregroundStyle(Ui.inkSoft)
                    .fixedSize()
                Spacer()
                if vm.clip {
                    Text("CLIP").font(.uiMicro.bold()).foregroundStyle(.red)
                }
                spectrumMini // flexes to 0 when the window gets narrow
                Image(systemName: "speaker.wave.2.fill").foregroundStyle(Ui.inkSoft)
                Slider(value: $vm.volume, in: 0...1.42).frame(maxWidth: 100) // 1.42² ≈ 2x gain
            }
            .padding(.horizontal, 10)
            .padding(.bottom, 8)
        }
        .background(Ui.surface)
        .overlay(alignment: .top) { Ui.border.frame(height: 1) }
    }

    /// Transport-bar viz: live thumbnail of the selected mode — clicking
    /// expands into the Visuals pane. This surface is the compositor pump.
    private var spectrumMini: some View {
        Button { vm.selection = .visuals; vm.stageFace = .viz } label: {
            VizSurfaceView(mode: vm.vizMode, compact: true)
                .frame(minWidth: 0, maxWidth: 120)
                .frame(height: 28)
                .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .onHover { h in
            if h { NSCursor.pointingHand.push() } else { NSCursor.pop() }
        }
        .help("Visuals — \(vm.vizMode.displayName)")
    }

    // ── EQ ───────────────────────────────────────────────────────────────
    private var eqPane: some View {
        VStack(alignment: .leading, spacing: Ui.s16) {
            Text("Parametric EQ").font(.uiTitle)
                .foregroundStyle(Ui.ink)
            Text("Curve drawn from the same biquad coefficients the audio path uses — not an approximation.")
                .font(.uiCaption).foregroundStyle(Ui.inkSoft)

            // response curve + analyzer underlay
            eqCurveView
                .frame(maxWidth: .infinity)
                .frame(height: 160)

            // Vertical sliders: rotate a horizontal Slider — the layout
            // frame must be pinned to the ROTATED bounds (20×110), not the
            // unrotated ones, or it overlaps its neighbours.
            HStack(alignment: .top, spacing: 0) {
                ForEach(0..<10, id: \.self) { i in
                    VStack(spacing: 4) {
                        Text(String(format: "%+.0f", vm.eq[i]))
                            .font(.uiMono)
                            .foregroundStyle(Ui.inkSoft)
                            .frame(height: 14)
                        ZStack {
                            // explicit track — the rotated Slider's own
                            // track renders too thin to read in dark mode
                            Rectangle().fill(Ui.border)
                                .frame(width: 4, height: 110)
                            Slider(value: Binding(
                                get: { vm.eq[i] },
                                set: { vm.eq[i] = $0; vm.applyEQ(i) }
                            ), in: -12...12)
                            .rotationEffect(.degrees(-90))
                            .frame(width: 20, height: 110)
                        }
                        Text(freqLabel(eqFreqs[i]))
                            .font(.uiMono)
                            .foregroundStyle(Ui.inkSoft)
                    }
                    .frame(maxWidth: .infinity)
                }
            }
            .frame(height: 150)
            HStack {
                Button("Flat") {
                    for i in 0..<10 { vm.eq[i] = 0; vm.applyEQ(i) }
                }
                .buttonStyle(.sharp)
                Spacer()
                Text("±12dB · 31Hz band is a low shelf")
                    .font(.uiMicro).foregroundStyle(Ui.inkSoft)
            }
            Spacer()
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .padding(Ui.s20)
        .onAppear { vm.refreshCurve() }
    }

    /// EQ response curve + live spectrum underlay. X axis is log-spaced
    /// 20Hz–20kHz, same geometric mapping as lyra-viz's band edges.
    private var eqCurveView: some View {
        Canvas { ctx, size in
            // dB grid: −12…+12
            for db in stride(from: -12, through: 12, by: 6) {
                let y = size.height * CGFloat(1 - (Float(db) + 12) / 24)
                var p = Path()
                p.move(to: CGPoint(x: 0, y: y))
                p.addLine(to: CGPoint(x: size.width, y: y))
                ctx.stroke(p, with: .color(Ui.border.opacity(db == 0 ? 0.9 : 0.5)))
            }
            // spectrum underlay — same log-freq geometry
            if !vm.bands.isEmpty {
                let n = vm.bands.count
                let w = size.width / CGFloat(n)
                for (i, v) in vm.bands.enumerated() {
                    let h = size.height * CGFloat(v) * 0.5
                    ctx.fill(
                        Path(CGRect(x: CGFloat(i) * w, y: size.height - h,
                                    width: w * 0.8, height: h)),
                        with: .color(Ui.mint.opacity(0.25))
                    )
                }
            }
            // curve — Rust computed from the live coefficients
            let (freqs, db) = vm.eqCurve
            if freqs.count > 1 {
                var path = Path()
                for i in 0..<freqs.count {
                    // x: log position of freq in [20, 20000]
                    let lx = log(freqs[i] / 20.0) / log(1000.0)
                    let x = CGFloat(lx) * size.width
                    let y = size.height * CGFloat(1 - (db[i] + 12) / 24)
                    let pt = CGPoint(x: x, y: y)
                    i == 0 ? path.move(to: pt) : path.addLine(to: pt)
                }
                ctx.stroke(path, with: .color(Ui.accent), lineWidth: 2)
            }
            // band markers
            for (i, f) in eqFreqs.enumerated() {
                let lx = log(f / 20.0) / log(1000.0)
                let x = CGFloat(lx) * size.width
                let y = size.height * CGFloat(1 - (vm.eq[i] + 12) / 24)
                let r = CGRect(x: x - 5, y: y - 5, width: 10, height: 10)
                ctx.fill(Path(ellipseIn: r), with: .color(
                    vm.eq[i] == 0 ? Ui.inkSoft.opacity(0.5) : Ui.accent))
            }
        }
        .uiCard(padding: 8)
    }

    private func freqLabel(_ f: Float) -> String {
        f >= 1000 ? String(format: "%gk", f / 1000) : String(format: "%g", f)
    }

    // ── Remote ───────────────────────────────────────────────────────────
    private var remotePane: some View {
        VStack(alignment: .leading, spacing: Ui.s16) {
            Text("Remote Control").font(.uiTitle)
                .foregroundStyle(Ui.ink)
            let r = LyraRemote.shared
            if !r.running {
                Label("Listener failed to start — check log", systemImage: "exclamationmark.triangle")
                    .foregroundStyle(.orange)
            } else {
                Label("Listening on :4777 — Noise XX, pinned devices only", systemImage: "antenna.radiowaves.left.and.right")
                    .foregroundStyle(Ui.inkSoft)
            }
            HStack(spacing: 12) {
                Button(vm.pairCode == nil ? "Pair new device…" : "Rotate code") {
                    if let p = r.openPairing() {
                        vm.pairCode = p.code
                        vm.pairFp = p.fingerprint
                        vm.refreshDevices()
                    }
                }
                .disabled(!r.running)
                .buttonStyle(.sharpProminent)
                Text("\(vm.pairedCount) device\(vm.pairedCount == 1 ? "" : "s") paired")
                    .foregroundStyle(Ui.inkSoft)
            }
            if !vm.pairedDevices.isEmpty {
                VStack(alignment: .leading, spacing: 6) {
                    ForEach(vm.pairedDevices, id: \.id) { d in
                        HStack(spacing: 8) {
                            Image(systemName: "iphone").foregroundStyle(Ui.mint)
                            Text(d.name).lineLimit(1)
                            Text(String(d.id.prefix(10)))
                                .font(.uiMono)
                                .foregroundStyle(Ui.inkSoft)
                            Spacer()
                            Button("Revoke") { vm.revokeDevice(d.id) }
                                .buttonStyle(.sharp)
                        }
                        .uiCard(padding: 8)
                        .uiElevated()
                    }
                }
            }
            if let code = vm.pairCode {
                VStack(alignment: .leading, spacing: 8) {
                    Text("Enter this code on the device — it binds one pairing handshake, it is not a credential:")
                        .font(.uiCaption).foregroundStyle(Ui.inkSoft)
                    Text(code)
                        .font(.system(size: 40, weight: .bold, design: .monospaced))
                        .foregroundStyle(Ui.accent)
                        .textSelection(.enabled)
                    Text("Host key fingerprint: \(vm.pairFp)")
                        .font(.uiMono).foregroundStyle(Ui.inkSoft)
                }
                .uiCard(padding: 16)
                .uiElevated()
            }
            Text("SPAKE2(code) → Noise XXpsk3 → pinned X25519 keys. Reconnects use plain XX — the pinned key is the identity.")
                .font(.uiMicro).foregroundStyle(Ui.inkSoft.opacity(0.7))
            Spacer()
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .padding(Ui.s20)
        .onAppear { vm.refreshDevices() }
    }
}
