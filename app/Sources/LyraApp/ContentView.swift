import SwiftUI
import UniformTypeIdentifiers

/// View model. NOTE: this shell is built with CLT swiftc — no full Xcode —
/// so bare `@State` (a SwiftUIMacros convenience macro in the 26 SDK) is
/// unavailable. ObservableObject + @Published + @StateObject are plain
/// property wrappers and compile fine.
final class ViewModel: ObservableObject {
    @Published var selection: SidebarItem? = .library
    @Published var probeResult = "Pick an audio file to probe it via Rust…"
    @Published var remoteOn = false {
        didSet { if remoteOn { LyraCore.startRemote(port: 9966) } }
    }
    // player state — polled from the engine by a view timer
    @Published var playing = false
    @Published var position: Double = 0
    @Published var volume: Float = 1.0 {
        didSet { LyraPlayer.shared.setVolume(volume) }
    }
    @Published var bands: [Float] = []
    @Published var currentFile: String?

    private var timer: Timer?

    func startPolling() {
        timer?.invalidate()
        timer = Timer.scheduledTimer(withTimeInterval: 1.0 / 15, repeats: true) { [weak self] _ in
            guard let self else { return }
            let p = LyraPlayer.shared
            DispatchQueue.main.async {
                self.playing = p.isPlaying
                self.position = p.position
                if let v = p.viz, let b = v["bands"] as? [Float] {
                    self.bands = b
                }
            }
        }
    }

    func pick() {
        let panel = NSOpenPanel()
        panel.allowedContentTypes = [.audio]
        panel.allowsMultipleSelection = false
        guard panel.runModal() == .OK, let url = panel.url else { return }
        currentFile = url.path
        if let result = LyraCore.probe(path: url.path),
           let data = try? JSONSerialization.data(withJSONObject: result, options: .prettyPrinted) {
            probeResult = String(decoding: data, as: UTF8.self)
        }
    }
}

enum SidebarItem: String, CaseIterable, Identifiable {
    case library = "Library"
    case remote = "Remote"
    case dsp = "DSP Chain"
    var id: String { rawValue }
    var icon: String {
        switch self {
        case .library: "music.note.list"
        case .remote: "iphone.radiowaves.left.and.right"
        case .dsp: "slider.horizontal.3"
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
            .navigationSplitViewColumnWidth(min: 160, ideal: 200)
        } detail: {
            switch vm.selection {
            case .library: libraryPane
            case .remote: remotePane
            case .dsp: dspPane
            case .none: Text("Select a section").foregroundStyle(.secondary)
            }
        }
        .frame(minWidth: 720, minHeight: 480)
        .toolbar {
            ToolbarItem(placement: .status) {
                Text("lyra-ffi \(LyraCore.version)")
                    .font(.caption.monospaced())
                    .foregroundStyle(.secondary)
            }
        }
    }

    private var libraryPane: some View {
        VStack(alignment: .leading, spacing: 16) {
            Text("Library").font(.title2)
            HStack(spacing: 12) {
                Button("Open…") { vm.pick() }
                if let f = vm.currentFile {
                    Text(URL(fileURLWithPath: f).lastPathComponent)
                        .foregroundStyle(.secondary).lineLimit(1)
                }
            }
            transportBar
            spectrumView
            ScrollView {
                Text(vm.probeResult)
                    .font(.system(.caption, design: .monospaced))
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .textSelection(.enabled)
            }
            .background(.quaternary.opacity(0.5), in: .rect(cornerRadius: 8))
        }
        .padding()
        .onAppear { vm.startPolling() }
    }

    private var transportBar: some View {
        HStack(spacing: 16) {
            Button(vm.playing ? "Pause" : "Play") {
                let p = LyraPlayer.shared
                if vm.playing { p.pause() }
                else if p.canResume { p.resume() }
                else if let f = vm.currentFile { p.play(path: f) }
            }
            .disabled(vm.currentFile == nil)
            Button("Stop") { LyraPlayer.shared.stop() }
            Text(String(format: "%.1fs", vm.position))
                .font(.caption.monospaced())
                .frame(width: 60)
            Slider(value: $vm.volume, in: 0...2).frame(maxWidth: 160)
            if vm.playing {
                Circle().fill(.green).frame(width: 8, height: 8)
            }
        }
    }

    private var spectrumView: some View {
        Canvas { ctx, size in
            guard !vm.bands.isEmpty else { return }
            let n = vm.bands.count
            let w = size.width / CGFloat(n)
            for (i, v) in vm.bands.enumerated() {
                let h = size.height * CGFloat(v)
                let rect = CGRect(x: CGFloat(i) * w, y: size.height - h,
                                  width: w * 0.8, height: h)
                ctx.fill(Path(rect), with: .color(.green.opacity(0.7)))
            }
        }
        .frame(height: 72)
        .background(.quaternary.opacity(0.5), in: .rect(cornerRadius: 8))
    }

    private var remotePane: some View {
        VStack(alignment: .leading, spacing: 16) {
            Text("Remote Control").font(.title2)
            Toggle("Listen on LAN (port 9966)", isOn: $vm.remoteOn)
            Text("Pairing-based auth lands here — see BLUEPRINT.md § remote.")
                .foregroundStyle(.secondary)
            Spacer()
        }
        .padding()
    }

    private var dspPane: some View {
        VStack(alignment: .leading, spacing: 16) {
            Text("DSP Chain").font(.title2)
            Text("preamp → parametric EQ → crossfeed → limiter → dither")
                .font(.callout.monospaced())
                .foregroundStyle(.secondary)
            Text("lyra-dsp ships Biquad peaking/shelf + safety limiter; engine wiring is the next milestone.")
                .foregroundStyle(.secondary)
            Spacer()
        }
        .padding()
    }

}
