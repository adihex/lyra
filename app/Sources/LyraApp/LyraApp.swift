import SwiftUI

@main
struct LyraApp: App {
    @ObservedObject private var vm = ViewModel.shared

    var body: some Scene {
        WindowGroup {
            ContentView()
        }
        .windowStyle(.titleBar)
        .defaultSize(width: 960, height: 640)
        .commands {
            // Space/arrow transport belongs on a *menu* button — putting it
            // on a visible button fights AppKit's focused-button activation.
            CommandMenu("Playback") {
                Button(vm.playing ? "Pause" : "Play") { vm.toggle() }
                    .keyboardShortcut(.space, modifiers: [])
                Button("Next Track") { vm.next() }
                    .keyboardShortcut(.rightArrow, modifiers: .command)
                Button("Previous Track") { vm.prev() }
                    .keyboardShortcut(.leftArrow, modifiers: .command)
                Divider()
                Button("Seek Forward 10s") { vm.seekBy(10) }
                    .keyboardShortcut(.rightArrow, modifiers: .option)
                Button("Seek Back 10s") { vm.seekBy(-10) }
                    .keyboardShortcut(.leftArrow, modifiers: .option)
            }
            CommandGroup(replacing: .newItem) {}
        }

        // Menu-bar mini player — .window style hosts arbitrary SwiftUI
        // (sliders/gestures work; `.menu` style kills them).
        MenuBarExtra("Lyra", systemImage: "music.note") {
            MiniPlayerView()
        }
        .menuBarExtraStyle(.window)
    }
}

/// Compact transport + now-playing for the menu-bar popover.
struct MiniPlayerView: View {
    @ObservedObject private var vm = ViewModel.shared

    var body: some View {
        VStack(spacing: 10) {
            VStack(alignment: .leading, spacing: 2) {
                Text(vm.current?.title ?? "Nothing playing").font(.headline).lineLimit(1)
                Text([vm.current?.artist, vm.current?.album].compactMap { $0 }.joined(separator: " — "))
                    .font(.caption).foregroundStyle(.secondary).lineLimit(1)
            }
            .frame(maxWidth: .infinity, alignment: .leading)
            HStack(spacing: 18) {
                Button { vm.prev() } label: { Image(systemName: "backward.fill") }
                Button { vm.toggle() } label: {
                    Image(systemName: vm.playing ? "pause.fill" : "play.fill")
                        .font(.title2)
                }
                Button { LyraPlayer.shared.stop() } label: { Image(systemName: "stop.fill") }
                Button { vm.next() } label: { Image(systemName: "forward.fill") }
            }
            .buttonStyle(.borderless)
            Slider(value: $vm.volume, in: 0...1.42)
            Text("\(vm.fmt(vm.displayPosition)) / \(vm.fmt(vm.current?.duration ?? 0))")
                .font(.caption.monospaced())
        }
        .padding()
        .frame(width: 240)
        .onAppear { vm.startPolling() }
    }
}
