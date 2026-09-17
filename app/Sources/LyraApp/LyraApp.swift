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
    // Observing the store republishes scene content on palette changes.
    @ObservedObject private var theme = LyraTheme.shared

    var body: some Scene {
        WindowGroup(id: "main") {
            ContentView()
                .preferredColorScheme(theme.appearance.colorScheme)
                .tint(theme.color(.tint))
                .background(WindowOpener())
        }
        // Hidden title bar: the lavender chassis runs edge to edge while
        // the native traffic lights and sidebar toolbar stay put.
        .windowStyle(.hiddenTitleBar)
        .defaultSize(width: Bubble.Size.windowWidth,
                     height: Bubble.Size.windowHeight)
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
                .preferredColorScheme(theme.appearance.colorScheme)
                .tint(theme.color(.tint))
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
                .preferredColorScheme(theme.appearance.colorScheme)
                .tint(theme.color(.tint))
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
/// Canvas does NOT render inside MenuBarExtra label slots: the label is
/// extracted as an image, and Canvas produces an empty one. Pre-rendered
/// NSImage bitmaps marked `isTemplate` are the reliable animated-icon
/// pattern — they draw correctly and adapt to the menu bar's tint.
struct MenuBarLabel: View {
    @ObservedObject private var vm = ViewModel.shared
    @ObservedObject private var prefs = Prefs.shared
    @ObservedObject private var ticker = VizTicker.shared

    var body: some View {
        switch (prefs.menuBarMode, vm.playing) {
        case ("spectrum", true):
            Image(nsImage: spectrumImage())
        case ("pulse", true):
            Image(nsImage: pulseImage())
        default:
            // At rest — or in "note" mode — a plain glyph is clearest.
            Image(systemName: "music.note")
        }
    }

    /// 8 bars off the first viz bands — 1.5px stubs at zero so quiet
    /// passages still show a live silhouette rather than a blank item.
    /// pumpIfStale: with every window closed nothing else pumps the
    /// compositor — this label becomes the fallback pump so the bars
    /// actually carry spectrum instead of flatlining.
    private func spectrumImage() -> NSImage {
        let f = vm.viz.pumpIfStale(0.08)
        let img = NSImage(size: NSSize(width: 22, height: 16), flipped: true) { _ in
            NSColor.black.setFill()
            let n = 8, gap: CGFloat = 1.5
            let w = (22 - gap * CGFloat(n - 1)) / CGFloat(n)
            for i in 0..<n {
                let v = i < f.bands.count
                    ? CGFloat(min(max(f.bands[i], 0), 1)) : 0
                NSRect(x: CGFloat(i) * (w + gap), y: 16 - max(16 * v, 1.5),
                       width: w, height: max(16 * v, 1.5)).fill()
            }
            return true
        }
        img.isTemplate = true
        return img
    }

    /// Single level dot — lowest-CPU animated mode.
    private func pulseImage() -> NSImage {
        let f = vm.viz.pumpIfStale(0.08)
        let v = CGFloat(min(max(f.level, 0), 1))
        let r = 2 + v * 6
        let img = NSImage(size: NSSize(width: 18, height: 16), flipped: true) { _ in
            NSColor.black.setFill()
            NSBezierPath(ovalIn: NSRect(x: 9 - r, y: 8 - r,
                                        width: 2 * r, height: 2 * r)).fill()
            return true
        }
        img.isTemplate = true
        return img
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
    @ObservedObject private var theme = LyraTheme.shared

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
            HStack(spacing: Bubble.Space.lg) {
                Button { vm.prev() } label: {
                    Image(systemName: "backward.fill").font(.uiHeadline)
                }
                .buttonStyle(BubbleButtonStyle(
                    shape: .circle, diameter: Bubble.Size.compactKey))
                .accessibilityLabel("Previous track")
                .help("Previous track")
                Button { vm.toggle() } label: {
                    Image(systemName: vm.playing ? "pause.fill" : "play.fill")
                        .font(.bubbleGlyph)
                }
                .buttonStyle(BubbleButtonStyle(
                    prominent: true, shape: .circle,
                    diameter: Bubble.Size.transportKey))
                .accessibilityLabel(vm.playing ? "Pause" : "Play")
                .help(vm.playing ? "Pause" : "Play")
                Button { vm.next() } label: {
                    Image(systemName: "forward.fill").font(.uiHeadline)
                }
                .buttonStyle(BubbleButtonStyle(
                    shape: .circle, diameter: Bubble.Size.compactKey))
                .accessibilityLabel("Next track")
                .help("Next track")
            }

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
        .frame(width: Bubble.Size.inspector)
        .bubbleTray(padding: Bubble.Space.md)
        .background(Color.bubbleChassis)
        .onAppear { vm.startPolling() }
    }
}
