import SwiftUI

@main
struct LyraApp: App {
    // The delegate mounts every system surface that needs one: dock
    // menu, notification actions, Spotlight restore (docs §0).
    @NSApplicationDelegateAdaptor(AppDelegate.self) private var appDelegate
    @ObservedObject private var vm = ViewModel.shared
    @ObservedObject private var prefs = Prefs.shared
    // Touching the singleton here starts its policy timer at launch.
    @ObservedObject private var pet = DesktopPet.shared

    var body: some Scene {
        WindowGroup(id: "main") {
            ContentView()
                .preferredColorScheme(.light) // beige theme needs light chrome
                .background(WindowOpener())
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
                // The cosmos cast escapes the dock onto the desktop.
                Picker("Desktop Pet", selection: Binding(
                        get: { pet.policy }, set: { pet.policy = $0 })) {
                    ForEach(DesktopPet.Policy.allCases, id: \.self) {
                        Text($0.title).tag($0)
                    }
                }
                Toggle("Pet is Interactive", isOn: Binding(
                        get: { pet.interactive },
                        set: { pet.interactive = $0 }))
                Button(pet.isOut ? "Recall Pet" : "Summon Pet") {
                    pet.summonOrRecall()
                }
            }
            CommandGroup(replacing: .newItem) {}
            SidebarCommands() // View-menu sidebar toggle + shortcut
        }

        // Cmd-, — prefs home for the system surfaces (docs §10).
        Settings {
            SettingsView()
                .preferredColorScheme(.light)
        }

        // Menu-bar mini player — .window style hosts arbitrary SwiftUI
        // (sliders/gestures work; `.menu` style kills them). isInserted
        // is our own toggle, independent of Tahoe's kill switch.
        // The system commits the current state through this binding on
        // every scene re-eval — writing an @Published there re-published
        // → re-eval → commit → set: a perpetual churn loop. Guarded: a
        // same-value commit is dropped before it can publish, and a real
        // toggle hops a runloop tick out of the in-flight transaction.
        MenuBarExtra(isInserted: Binding(
            get: { prefs.menuBarExtra },
            set: { v in
                guard v != prefs.menuBarExtra else { return }
                DispatchQueue.main.async { prefs.menuBarExtra = v }
            })) {
            MiniPlayerView()
                .preferredColorScheme(.light)
        } label: {
            MenuBarLabel()
        }
        .menuBarExtraStyle(.window)
    }
}

/// openWindow lives only in the view environment — capture it once so
/// the dock menu and notification clicks can reopen the main window.
private struct WindowOpener: View {
    @Environment(\.openWindow) private var openWindow
    var body: some View {
        Color.clear
            .frame(width: 0, height: 0)
            .onAppear { WindowOps.openMain = { openWindow(id: "main") } }
    }
}

/// Live menu-bar label — TimelineView is the documented workaround for
/// the server-level label cache (FB11857447, docs §1). Pure reads off
/// the shared compositor: pump() stays owned by the transport-bar mini
/// surface. At rest (paused engine → decayed frame) the bars flatten to
/// a hairline so the item never vanishes.
struct MenuBarLabel: View {
    @ObservedObject private var vm = ViewModel.shared
    @ObservedObject private var prefs = Prefs.shared

    var body: some View {
        switch prefs.menuBarMode {
        case "note":
            Image(systemName: "music.note")
        case "pulse":
            if vm.playing {
                TimelineView(.animation(minimumInterval: 1.0 / 15)) { _ in
                    pulseCanvas
                }
            } else {
                pulseCanvas
            }
        default:
            if vm.playing {
                TimelineView(.animation(minimumInterval: 1.0 / 15)) { _ in
                    spectrumCanvas
                }
            } else {
                spectrumCanvas
            }
        }
    }

    /// 8 ink bars off the first viz bands — still at rest.
    private var spectrumCanvas: some View {
        Canvas { ctx, size in
            let f = vm.viz.frame()
            let live = vm.playing && f.level > 0.02
            let n = 8
            let gap: CGFloat = 1.5
            let w = (size.width - gap * CGFloat(n - 1)) / CGFloat(n)
            for i in 0..<n {
                let v = live && i < f.bands.count
                    ? CGFloat(min(max(f.bands[i], 0), 1)) : 0
                let h = max(size.height * v, 1.5)
                ctx.fill(
                    Path(CGRect(x: CGFloat(i) * (w + gap),
                                y: size.height - h,
                                width: w, height: h)),
                    with: .color(.primary))
            }
        }
        .frame(width: 22, height: 16)
    }

    /// Single level dot — lowest-CPU animated mode.
    private var pulseCanvas: some View {
        Canvas { ctx, size in
            let f = vm.viz.frame()
            let v = vm.playing ? CGFloat(min(max(f.level, 0), 1)) : 0
            let r = 2 + v * (min(size.width, size.height) / 2 - 2)
            ctx.fill(
                Path(ellipseIn: CGRect(x: size.width / 2 - r,
                                       y: size.height / 2 - r,
                                       width: 2 * r, height: 2 * r)),
                with: .color(.primary))
        }
        .frame(width: 18, height: 16)
    }
}

/// Compact transport + now-playing for the menu-bar popover.
struct MiniPlayerView: View {
    @ObservedObject private var vm = ViewModel.shared
    @ObservedObject private var pet = DesktopPet.shared

    var body: some View {
        VStack(spacing: 10) {
            VStack(alignment: .leading, spacing: 2) {
                Text(vm.current?.title ?? "Nothing playing").font(.uiHeadline)
                    .foregroundStyle(Ui.ink).lineLimit(1)
                Text([vm.current?.artist, vm.current?.album].compactMap { $0 }.joined(separator: " — "))
                    .font(.uiCaption).foregroundStyle(Ui.inkSoft).lineLimit(1)
            }
            .frame(maxWidth: .infinity, alignment: .leading)
            HStack(spacing: 12) {
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
            }
            .tint(Ui.ink)
            Slider(value: $vm.volume, in: 0...1.42)
                .tint(Ui.accent)
            // displayPosition is a plain var — 0.5s cadence while playing,
            // static at rest (see ViewModel for why these aren't @Published).
            Group {
                if vm.playing {
                    TimelineView(.periodic(from: .now, by: 0.5)) { _ in
                        timeText
                    }
                } else {
                    timeText
                }
            }
            // Escape hatch for a pet lost under real windows.
            if pet.isOut || pet.userHidden {
                Button { pet.summonOrRecall() } label: {
                    Text(pet.isOut ? "Recall pet" : "Summon pet")
                        .font(.uiCaption).foregroundStyle(Ui.accent)
                }
                .buttonStyle(.plain)
            }
        }
        .padding()
        .frame(width: 240)
        .background(Ui.surface)
        .onAppear { vm.startPolling() }
    }

    private var timeText: some View {
        Text("\(vm.fmt(vm.displayPosition)) / \(vm.fmt(vm.current?.duration ?? 0))")
            .font(.uiMono)
            .foregroundStyle(Ui.inkSoft)
    }
}
