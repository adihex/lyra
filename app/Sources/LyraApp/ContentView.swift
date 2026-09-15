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
            Button("Probe an audio file…") { pickAndProbe() }
            ScrollView {
                Text(vm.probeResult)
                    .font(.system(.caption, design: .monospaced))
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .textSelection(.enabled)
            }
            .background(.quaternary.opacity(0.5), in: .rect(cornerRadius: 8))
        }
        .padding()
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

    private func pickAndProbe() {
        let panel = NSOpenPanel()
        panel.allowedContentTypes = [.audio]
        panel.allowsMultipleSelection = false
        guard panel.runModal() == .OK, let url = panel.url else { return }
        if let result = LyraCore.probe(path: url.path),
           let data = try? JSONSerialization.data(withJSONObject: result, options: .prettyPrinted) {
            vm.probeResult = String(decoding: data, as: UTF8.self)
        } else {
            vm.probeResult = "probe failed"
        }
    }
}
