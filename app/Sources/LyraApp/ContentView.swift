import SwiftUI
import UniformTypeIdentifiers

/// One scanned library row — mirrors LibraryTrack's camelCase JSON.
struct Track: Identifiable, Hashable {
    let id: String // path
    let path: String
    var title: String
    var artist: String
    var album: String
    var albumArtist: String
    var duration: Double
    var format: String
    var codec: String
    var trackNumber: Int

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
    }
}

/// ISO-center EQ bands for the 10-band parametric.
let eqFreqs: [Float] = [31, 62, 125, 250, 500, 1000, 2000, 4000, 8000, 16000]

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
    @Published var libraryRoot: String?
    @Published var contentID = UUID() // bump → Table skips dataset diffing

    // now playing
    @Published var current: Track?
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

    private var timer: Timer?
    private var vizBuf: UnsafeMutableBufferPointer<Float>

    init() {
        vizBuf = .allocate(capacity: 48)
        MediaKeys.shared.hook(
            getState: { (LyraPlayer.shared.isPlaying, LyraPlayer.shared.position) },
            onToggle: { [weak self] in self?.toggle() },
            onNext: { [weak self] in self?.next() },
            onPrev: { [weak self] in self?.prev() },
            onSeek: { pos in LyraPlayer.shared.seek(pos) }
        )
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

    func scanFolder() {
        let panel = NSOpenPanel()
        panel.canChooseFiles = false
        panel.canChooseDirectories = true
        guard panel.runModal() == .OK, let url = panel.url else { return }
        libraryRoot = url.path
        scanning = true
        DispatchQueue.global(qos: .userInitiated).async {
            let rows = LyraCore.scanDir(path: url.path) ?? []
            let ts = rows.map(Track.init)
            DispatchQueue.main.async {
                self.tracks = ts
                self.contentID = UUID() // force no-diff Table rebuild
                self.scanning = false
            }
        }
    }

    func play(_ t: Track) {
        if LyraPlayer.shared.play(path: t.path) {
            current = t
            position = 0
            pushEQ()
            publishNowPlaying(t)
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

    func publishNowPlaying(_ t: Track) {
        MediaKeys.shared.publish(title: t.title, artist: t.artist,
                                 album: t.album, duration: t.duration)
    }

    func fmt(_ s: Double) -> String {
        let t = Int(s)
        return String(format: "%d:%02d", t / 60, t % 60)
    }
}

enum SidebarItem: String, CaseIterable, Identifiable {
    case library = "Library"
    case eq = "Equalizer"
    case remote = "Remote"
    var id: String { rawValue }
    var icon: String {
        switch self {
        case .library: "music.note.list"
        case .eq: "slider.horizontal.3"
        case .remote: "iphone.radiowaves.left.and.right"
        }
    }
}

struct ContentView: View {
    @ObservedObject private var vm = ViewModel.shared

    var body: some View {
        NavigationSplitView(columnVisibility: $vm.columnVis) {
            List(SidebarItem.allCases, selection: $vm.selection) { item in
                Label(item.rawValue, systemImage: item.icon).tag(item)
            }
            .navigationSplitViewColumnWidth(min: 170, ideal: 190, max: 240)
        } detail: {
            VStack(spacing: 0) {
                detailView
                nowPlayingBar
            }
        }
        .frame(minWidth: 780, minHeight: 560)
        .toolbar {
            ToolbarItem(placement: .status) {
                Text("lyra-ffi \(LyraCore.version)")
                    .font(.caption.monospaced())
                    .foregroundStyle(.secondary)
            }
        }
    }

    @ViewBuilder private var detailView: some View {
        switch vm.selection {
        case .library: libraryPane
        case .eq: eqPane
        case .remote: remotePane
        case .none: Text("Select a section").foregroundStyle(.secondary)
        }
    }

    // ── Library ──────────────────────────────────────────────────────────
    private var libraryPane: some View {
        VStack(alignment: .leading, spacing: 12) {
            HStack {
                Text("Library").font(.title2)
                // explicit search field — .searchable(placement:.toolbar)
                // landed in the collapsed sidebar strip; an inline field is
                // deterministic about where it lives.
                HStack(spacing: 4) {
                    Image(systemName: "magnifyingglass").foregroundStyle(.secondary)
                    TextField("Filter…", text: $vm.query)
                        .textFieldStyle(.plain)
                }
                .padding(.horizontal, 8).padding(.vertical, 4)
                .background(.quaternary, in: RoundedRectangle(cornerRadius: 6))
                .frame(width: 200)
                Spacer()
                if let root = vm.libraryRoot {
                    Text(URL(fileURLWithPath: root).lastPathComponent)
                        .foregroundStyle(.secondary)
                }
                Button(vm.scanning ? "Scanning…" : "Scan folder…") { vm.scanFolder() }
                    .disabled(vm.scanning)
            }
            if vm.tracks.isEmpty {
                Spacer()
                Text("Scan a folder to build the library.")
                    .foregroundStyle(.secondary)
                    .frame(maxWidth: .infinity)
                Spacer()
            } else {
                Table(vm.sortedTracks, selection: $vm.selectedTracks,
                      sortOrder: $vm.sortOrder) {
                    TableColumn("#", value: \.trackNumber) { t in
                        Text("\(t.trackNumber)")
                    }.width(30)
                    TableColumn("Title", value: \.title)
                    TableColumn("Artist", value: \.artist).width(140)
                    TableColumn("Album", value: \.album).width(150)
                    TableColumn("Time", value: \.duration) { t in
                        Text(vm.fmt(t.duration)).monospaced()
                    }.width(50)
                    TableColumn("Codec", value: \.codec) { t in
                        Text(t.codec).font(.caption2.weight(.semibold))
                            .padding(.horizontal, 5).padding(.vertical, 1)
                            .background(.quaternary, in: .capsule)
                    }.width(56)
                }
                .id(vm.contentID) // force rebuild — 100k-row diffing stalls
                .contextMenu(forSelectionType: Track.ID.self) { items in
                    Button("Play") { vm.playSelection(items) }
                    Divider()
                    Button("Reveal in Finder") {
                        if let id = items.first {
                            NSWorkspace.shared.activateFileViewerSelecting(
                                [URL(fileURLWithPath: id)])
                        }
                    }
                } primaryAction: { items in
                    vm.playSelection(items) // double-click
                }
                HStack {
                    Text("\(vm.sortedTracks.count) tracks")
                        .font(.caption).foregroundStyle(.secondary)
                    Spacer()
                    Button("Play selected") { vm.playSelection(vm.selectedTracks) }
                        .disabled(vm.selectedTracks.isEmpty)
                }
            }
        }
        .padding()
        .onAppear { vm.startPolling() }
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
            HStack(spacing: 14) {
                VStack(alignment: .leading) {
                    Text(vm.current?.title ?? "—").font(.headline).lineLimit(1)
                    Text([vm.current?.artist, vm.current?.album].compactMap { $0 }.joined(separator: " — "))
                        .font(.caption).foregroundStyle(.secondary).lineLimit(1)
                }
                .frame(minWidth: 160, alignment: .leading)
                Spacer()
                Button { vm.prev() } label: { Image(systemName: "backward.fill") }
                Button { vm.toggle() } label: {
                    Image(systemName: vm.playing ? "pause.fill" : "play.fill")
                }
                Button { LyraPlayer.shared.stop() } label: { Image(systemName: "stop.fill") }
                Button { vm.next() } label: { Image(systemName: "forward.fill") }
                Text("\(vm.fmt(vm.displayPosition)) / \(vm.fmt(vm.current?.duration ?? 0))")
                    .font(.caption.monospaced())
                    .fixedSize()
                Spacer()
                if vm.clip {
                    Text("CLIP").font(.caption2.bold()).foregroundStyle(.red)
                }
                spectrumMini
                Image(systemName: "speaker.wave.2.fill").foregroundStyle(.secondary)
                Slider(value: $vm.volume, in: 0...1.42).frame(width: 110) // 1.42² ≈ 2x gain
            }
            .padding(.horizontal, 12)
            .padding(.bottom, 8)
        }
        .background(.bar)
    }

    private var spectrumMini: some View {
        Canvas { ctx, size in
            guard !vm.bands.isEmpty else { return }
            let n = vm.bands.count
            let w = size.width / CGFloat(n)
            for (i, v) in vm.bands.enumerated() {
                let h = size.height * CGFloat(v)
                ctx.fill(
                    Path(CGRect(x: CGFloat(i) * w, y: size.height - h,
                                width: w * 0.75, height: h)),
                    with: .color(.green.opacity(0.75))
                )
            }
        }
        .frame(width: 120, height: 28)
    }

    // ── EQ ───────────────────────────────────────────────────────────────
    private var eqPane: some View {
        VStack(alignment: .leading, spacing: 16) {
            Text("Parametric EQ").font(.title2)
            Text("Curve drawn from the same biquad coefficients the audio path uses — not an approximation.")
                .font(.caption).foregroundStyle(.secondary)

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
                            .font(.caption2.monospaced())
                            .frame(height: 14)
                        ZStack {
                            // explicit track — the rotated Slider's own
                            // track renders too thin to read in dark mode
                            Capsule().fill(.quaternary)
                                .frame(width: 4, height: 110)
                            Slider(value: Binding(
                                get: { vm.eq[i] },
                                set: { vm.eq[i] = $0; vm.applyEQ(i) }
                            ), in: -12...12)
                            .rotationEffect(.degrees(-90))
                            .frame(width: 20, height: 110)
                        }
                        Text(freqLabel(eqFreqs[i]))
                            .font(.caption2.monospaced())
                            .foregroundStyle(.secondary)
                    }
                    .frame(maxWidth: .infinity)
                }
            }
            .frame(height: 150)
            HStack {
                Button("Flat") {
                    for i in 0..<10 { vm.eq[i] = 0; vm.applyEQ(i) }
                }
                Spacer()
                Text("±12dB · 31Hz band is a low shelf")
                    .font(.caption2).foregroundStyle(.secondary)
            }
            Spacer()
        }
        .padding()
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
                ctx.stroke(p, with: .color(.gray.opacity(db == 0 ? 0.5 : 0.2)))
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
                        with: .color(.green.opacity(0.2))
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
                ctx.stroke(path, with: .color(.accentColor), lineWidth: 2)
            }
            // band markers
            for (i, f) in eqFreqs.enumerated() {
                let lx = log(f / 20.0) / log(1000.0)
                let x = CGFloat(lx) * size.width
                let y = size.height * CGFloat(1 - (vm.eq[i] + 12) / 24)
                let r = CGRect(x: x - 5, y: y - 5, width: 10, height: 10)
                ctx.fill(Path(ellipseIn: r), with: .color(
                    vm.eq[i] == 0 ? .gray.opacity(0.5) : .accentColor))
            }
        }
        .background(.quaternary.opacity(0.3), in: RoundedRectangle(cornerRadius: 8))
    }

    private func freqLabel(_ f: Float) -> String {
        f >= 1000 ? String(format: "%gk", f / 1000) : String(format: "%g", f)
    }

    // ── Remote ───────────────────────────────────────────────────────────
    private var remotePane: some View {
        VStack(alignment: .leading, spacing: 16) {
            Text("Remote Control").font(.title2)
            Text("Pairing-based auth (SPAKE2 → pinned keys → Noise XX) lands before this is exposed — see BLUEPRINT.md § remote.")
                .foregroundStyle(.secondary)
            Spacer()
        }
        .padding()
    }
}
