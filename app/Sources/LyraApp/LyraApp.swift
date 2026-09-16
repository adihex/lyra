import SwiftUI

@main
struct LyraApp: App {
    @ObservedObject private var vm = ViewModel.shared

    var body: some Scene {
        WindowGroup {
            ContentView()
                .preferredColorScheme(.light) // beige theme needs light chrome
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
                Divider()
                // Exclusive HAL output: hog mode + IOProc. Engine swaps at
                // runtime; choice persists to the next launch.
                Toggle("Exclusive Output (HAL)",
                       isOn: Binding(
                           get: { vm.exclusiveOutput },
                           set: { vm.setExclusiveOutput($0) }))
            }
            CommandGroup(replacing: .newItem) {}
        }

        // Menu-bar mini player — .window style hosts arbitrary SwiftUI
        // (sliders/gestures work; `.menu` style kills them).
        MenuBarExtra("Lyra", systemImage: "music.note") {
            MiniPlayerView()
                .preferredColorScheme(.light)
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
                Text(vm.current?.title ?? "Nothing playing").font(.cozy(.headline))
                    .foregroundStyle(Cozy.ink).lineLimit(1)
                Text([vm.current?.artist, vm.current?.album].compactMap { $0 }.joined(separator: " — "))
                    .font(.caption).foregroundStyle(Cozy.inkSoft).lineLimit(1)
            }
            .frame(maxWidth: .infinity, alignment: .leading)
            HStack(spacing: 18) {
                Button { vm.prev() } label: { Image(systemName: "backward.fill") }
                Button { vm.toggle() } label: {
                    Image(systemName: vm.playing ? "pause.fill" : "play.fill")
                        .font(.system(size: 16, weight: .bold))
                        .foregroundStyle(.white)
                        .frame(width: 34, height: 34)
                        .background(Cozy.accent, in: Circle())
                }
                .buttonStyle(.plain)
                Button { LyraPlayer.shared.stop() } label: { Image(systemName: "stop.fill") }
                Button { vm.next() } label: { Image(systemName: "forward.fill") }
            }
            .buttonStyle(.borderless)
            .tint(Cozy.ink)
            Slider(value: $vm.volume, in: 0...1.42)
                .tint(Cozy.accent)
            Text("\(vm.fmt(vm.displayPosition)) / \(vm.fmt(vm.current?.duration ?? 0))")
                .font(.cozy(.caption, weight: .medium).monospacedDigit())
                .foregroundStyle(Cozy.inkSoft)
        }
        .padding()
        .frame(width: 240)
        .background(Cozy.surface)
        .onAppear { vm.startPolling() }
    }
}
