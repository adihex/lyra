import SwiftUI
import UniformTypeIdentifiers

/// One scanned library row — mirrors LibraryTrack's camelCase JSON.
struct Track: Identifiable {
    let id: String // path
    let path: String
    var title: String
    var artist: String
    var album: String
    var albumArtist: String
    var duration: Double
    var format: String
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
        trackNumber = d["trackNumber"] as? Int ?? 0
    }
}

/// ISO-center EQ bands for the 10-band parametric.
let eqFreqs: [Float] = [31, 62, 125, 250, 500, 1000, 2000, 4000, 8000, 16000]

/// View model. NOTE: built with CLT swiftc (no Xcode) — bare `@State` is a
/// SwiftUIMacros convenience macro unavailable here; ObservableObject +
/// @Published + @StateObject are plain property wrappers and work.
final class ViewModel: ObservableObject {
    @Published var selection: SidebarItem? = .library
    @Published var tracks: [Track] = []
    @Published var selectedTrack: Track.ID?
    @Published var scanning = false
    @Published var libraryRoot: String?

    // now playing
    @Published var current: Track?
    @Published var playing = false
    @Published var position: Double = 0
    @Published var draggingSeek = false
    @Published var seekValue: Double = 0
    @Published var volume: Float = 1.0 {
        didSet { LyraPlayer.shared.setVolume(volume) }
    }

    // eq gains per band, dB
    @Published var eq: [Float] = Array(repeating: 0, count: 10)

    // viz
    @Published var bands: [Float] = []
    @Published var clip = false

    private var timer: Timer?

    var currentIndex: Int? { tracks.firstIndex { $0.id == current?.id } }

    func startPolling() {
        timer?.invalidate()
        timer = Timer.scheduledTimer(withTimeInterval: 1.0 / 30, repeats: true) { [weak self] _ in
            guard let self else { return }
            let p = LyraPlayer.shared
            DispatchQueue.main.async {
                self.playing = p.isPlaying
                if !self.draggingSeek { self.position = p.position }
                if let v = p.viz {
                    self.bands = v["bands"] as? [Float] ?? self.bands
                    self.clip = v["clip"] as? Bool ?? false
                }
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
        tracks = []
        DispatchQueue.global(qos: .userInitiated).async {
            let rows = LyraCore.scanDir(path: url.path) ?? []
            let ts = rows.map(Track.init)
            DispatchQueue.main.async {
                self.tracks = ts
                self.scanning = false
            }
        }
    }

    func play(_ t: Track) {
        if LyraPlayer.shared.play(path: t.path) {
            current = t
            position = 0
            // push EQ state into the engine (it rebuilds at track rate)
            for (i, f) in eqFreqs.enumerated() {
                LyraPlayer.shared.setBand(i, freq: f, q: 0.9, gainDb: eq[i],
                                          peaking: i != 0)
            }
        }
    }

    func toggle() {
        let p = LyraPlayer.shared
        if p.isPlaying { p.pause() }
        else if p.canResume { p.resume() }
        else if let t = current ?? tracks.first { play(t) }
    }

    func next() { step(1) }
    func prev() { step(-1) }
    private func step(_ d: Int) {
        guard let i = currentIndex, tracks.indices.contains(i + d) else { return }
        play(tracks[i + d])
    }

    func applyEQ(_ band: Int) {
        LyraPlayer.shared.setBand(band, freq: eqFreqs[band], q: 0.9,
                                  gainDb: eq[band], peaking: band != 0)
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
    @StateObject private var vm = ViewModel()

    var body: some View {
        NavigationSplitView {
            List(SidebarItem.allCases, selection: $vm.selection) { item in
                Label(item.rawValue, systemImage: item.icon).tag(item)
            }
            .navigationSplitViewColumnWidth(min: 140, ideal: 180)
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
                Table(vm.tracks, selection: $vm.selectedTrack) {
                    TableColumn("#") { t in
                        Text("\(t.trackNumber)").frame(width: 28)
                    }.width(36)
                    TableColumn("Title") { t in Text(t.title).lineLimit(1) }
                    TableColumn("Artist") { t in Text(t.artist).lineLimit(1) }.width(140)
                    TableColumn("Album") { t in Text(t.album).lineLimit(1) }.width(160)
                    TableColumn("Time") { t in Text(vm.fmt(t.duration)).monospaced() }.width(52)
                    TableColumn("Fmt") { t in Text(t.format).font(.caption) }.width(48)
                }
                .contextMenu(forSelectionType: Track.ID.self) { _ in
                    if let id = vm.selectedTrack,
                       let t = vm.tracks.first(where: { $0.id == id }) {
                        Button("Play") { vm.play(t) }
                    }
                }
                HStack {
                    Text("\(vm.tracks.count) tracks").font(.caption).foregroundStyle(.secondary)
                    Spacer()
                    Button("Play selected") {
                        if let id = vm.selectedTrack,
                           let t = vm.tracks.first(where: { $0.id == id }) {
                            vm.play(t)
                        }
                    }
                    .disabled(vm.selectedTrack == nil)
                }
            }
        }
        .padding()
        .onAppear { vm.startPolling() }
    }

    // ── Now playing bar ──────────────────────────────────────────────────
    private var nowPlayingBar: some View {
        VStack(spacing: 6) {
            // seek: external clock drives position; local drag owns it while dragging
            Slider(
                value: Binding(
                    get: { vm.draggingSeek ? vm.seekValue : vm.position },
                    set: { vm.seekValue = $0 }
                ),
                in: 0...max(vm.current?.duration ?? 1, 1),
                onEditingChanged: { editing in
                    vm.draggingSeek = editing
                    if !editing { LyraPlayer.shared.seek(vm.seekValue) }
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
                .keyboardShortcut(.space, modifiers: [])
                Button { LyraPlayer.shared.stop() } label: { Image(systemName: "stop.fill") }
                Button { vm.next() } label: { Image(systemName: "forward.fill") }
                Text("\(vm.fmt(vm.position)) / \(vm.fmt(vm.current?.duration ?? 0))")
                    .font(.caption.monospaced())
                Spacer()
                if vm.clip {
                    Text("CLIP").font(.caption2.bold()).foregroundStyle(.red)
                }
                spectrumMini
                Image(systemName: "speaker.wave.2.fill").foregroundStyle(.secondary)
                Slider(value: $vm.volume, in: 0...2).frame(width: 110)
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
            Text("10 bands · peaking (31Hz low-shelf) · gain ±12dB · applied in Rust before the limiter")
                .font(.caption).foregroundStyle(.secondary)
            HStack(alignment: .bottom, spacing: 18) {
                ForEach(0..<10, id: \.self) { i in
                    VStack(spacing: 6) {
                        Text(String(format: "%+.1f", vm.eq[i]))
                            .font(.caption2.monospaced())
                            .frame(width: 44)
                        Slider(value: Binding(
                            get: { vm.eq[i] },
                            set: { vm.eq[i] = $0; vm.applyEQ(i) }
                        ), in: -12...12)
                        .rotationEffect(.degrees(-90))
                        .frame(width: 120)
                        Text(freqLabel(eqFreqs[i]))
                            .font(.caption2.monospaced())
                            .foregroundStyle(.secondary)
                    }
                }
            }
            .frame(height: 180)
            HStack {
                Button("Flat") {
                    for i in 0..<10 { vm.eq[i] = 0; vm.applyEQ(i) }
                }
                Spacer()
            }
            Spacer()
        }
        .padding()
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
