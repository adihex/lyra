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

/// Live menu-bar label — VizTicker supplies the cadence while playing.
/// Pure reads off the shared compositor: pump() stays owned by the
/// transport-bar mini surface. At rest the label is a static note glyph —
/// flattened hairline bars rendered as an invisible item.
struct MenuBarLabel: View {
    @ObservedObject private var vm = ViewModel.shared
    @ObservedObject private var prefs = Prefs.shared
    // VizTicker drives the cadence while playing; at rest the canvas
    // renders once, statically. TimelineView can't live in a
    // MenuBarExtra label slot — it never ticks there.
    @ObservedObject private var ticker = VizTicker.shared

    /// Animated modes only differ while playing; at rest every mode shows
    /// the same note glyph — hairline bars rendered as an invisible item.
    private var live: Bool {
        vm.playing && vm.viz.frame().level > 0.02
    }

    var body: some View {
        if !live {
            Image(systemName: "music.note")
        } else {
            switch prefs.menuBarMode {
            case "note":
                Image(systemName: "music.note")
            case "pulse":
                pulseCanvas
            default:
                spectrumCanvas
            }
        }
    }

    /// 8 ink bars off the first viz bands — still at rest.
    private var spectrumCanvas: some View {
        Canvas { ctx, size in
            let f = vm.viz.frame()
            let n = 8
            let gap: CGFloat = 1.5
            let w = (size.width - gap * CGFloat(n - 1)) / CGFloat(n)
            for i in 0..<n {
                let v = i < f.bands.count
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
            let v = CGFloat(min(max(f.level, 0), 1))
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
/// The slider row is a *seek* bar — elapsed/remaining labels flank it so
/// it can't be mistaken for volume. Volume gets its own speaker-labelled
/// row below the transport.
struct MiniPlayerView: View {
    @ObservedObject private var vm = ViewModel.shared
    @ObservedObject private var pet = DesktopPet.shared
    @ObservedObject private var ticker = VizTicker.shared

    var body: some View {
        VStack(spacing: 10) {
            VStack(alignment: .leading, spacing: 2) {
                Text(vm.current?.title ?? "Nothing playing").font(.uiHeadline)
                    .foregroundStyle(Ui.ink).lineLimit(1)
                Text([vm.current?.artist, vm.current?.album].compactMap { $0 }.joined(separator: " — "))
                    .font(.uiCaption).foregroundStyle(Ui.inkSoft).lineLimit(1)
            }
            .frame(maxWidth: .infinity, alignment: .leading)

            // Seek — same binding pattern as the main window's seekSlider:
            // engine clock drives, drag owns it until release.
            VStack(spacing: 0) {
                Slider(
                    value: Binding(
                        get: { vm.displayPosition },
                        set: { vm.displayPosition = $0 }),
                    in: 0...max(vm.current?.duration ?? 1, 1),
                    onEditingChanged: { editing in
                        if editing { vm.scrubbing = true } else { vm.scrubEnded() }
                    }
                )
                .tint(Ui.accent)
                .disabled(vm.current == nil)
                HStack {
                    Text(vm.fmt(vm.displayPosition))
                    Spacer()
                    Text("-\(vm.fmt(max((vm.current?.duration ?? 0) - vm.displayPosition, 0)))")
                }
                .font(.uiMono)
                .foregroundStyle(Ui.inkSoft)
            }

            // Transport — prev / play-pause / next. Pause already covers
            // stop's job here; a fourth button read as clutter.
            HStack(spacing: 16) {
                Button { vm.prev() } label: {
                    Image(systemName: "backward.fill")
                        .font(.system(size: 13, weight: .semibold))
                        .sharpIconBox(32)
                }
                .buttonStyle(.plain)
                .help("Previous track")
                Button { vm.toggle() } label: {
                    Image(systemName: vm.playing ? "pause.fill" : "play.fill")
                        .font(.system(size: 16, weight: .bold))
                        .foregroundStyle(.white)
                        .frame(width: 40, height: 40)
                        .background(Ui.accent)
                }
                .buttonStyle(.plain)
                .help(vm.playing ? "Pause" : "Play")
                Button { vm.next() } label: {
                    Image(systemName: "forward.fill")
                        .font(.system(size: 13, weight: .semibold))
                        .sharpIconBox(32)
                }
                .buttonStyle(.plain)
                .help("Next track")
            }
            .tint(Ui.ink)

            // Volume — speaker glyphs make the control unmistakable.
            HStack(spacing: 8) {
                Image(systemName: "speaker.fill")
                    .font(.system(size: 9))
                    .foregroundStyle(Ui.inkSoft)
                Slider(value: $vm.volume, in: 0...1.42)
                    .tint(Ui.accent)
                Image(systemName: "speaker.wave.3.fill")
                    .font(.system(size: 9))
                    .foregroundStyle(Ui.inkSoft)
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
}
