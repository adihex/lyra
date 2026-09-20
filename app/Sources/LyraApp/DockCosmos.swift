import AppKit
import Combine

/// The custom dock-tile content view. draw() is a PURE function of the
/// shared scene + latest frame — the Dock re-renders on resize and
/// magnification, so nothing may mutate here; all motion comes from
/// DockVizDriver's tick. The scene is `VizRuntime.cosmos` — shared with
/// the desktop pet so a pop-out adopts live state instead of respawning.
final class DockIconView: NSView {
    /// Latest eased frame, written by the driver's tick (pure read here).
    /// Named vizFrame — `frame` is NSView's own bounds-in-superview rect.
    var vizFrame = VizFrame.rest
    /// Theme subscription — repaints the tile even while the driver is
    /// idle (nothing playing). Never installs a timer or the tile itself.
    private var themeWatch: AnyCancellable?

    override init(frame frameRect: NSRect) {
        super.init(frame: frameRect)
        themeWatch = LyraTheme.shared.$palette
            .combineLatest(LyraTheme.shared.$appearance)
            .receive(on: RunLoop.main)
            .sink { [weak self] _ in self?.repaintForTheme() }
    }

    required init?(coder: NSCoder) { fatalError() }

    override func viewDidChangeEffectiveAppearance() {
        super.viewDidChangeEffectiveAppearance()
        repaintForTheme()
    }

    /// The Dock only snapshots when the view is its contentView — push
    /// the tile's display() so an idle tile still picks up a theme flip
    /// (NSView.display() only repaints locally; dockTile.display() is
    /// the snapshot IPC that ships pixels to the Dock).
    private func repaintForTheme() {
        needsDisplay = true
        if NSApp.dockTile.contentView === self { NSApp.dockTile.display() }
    }

    override func draw(_ dirty: NSRect) {
        guard let ctx = NSGraphicsContext.current?.cgContext else { return }
        let palette = PetPalette.resolve(LyraTheme.shared.palette,
                                         appearance: effectiveAppearance)
        CosmosPaintCG.draw(ctx, in: bounds,
                           scene: ViewModel.shared.viz.cosmos,
                           frame: vizFrame, palette: palette)
    }
}

/// Drives the dock tile at 12 Hz — display() is a synchronous view
/// snapshot + mach msg to the Dock server, so 12 Hz is the sweet spot
/// (dock-icon-viz §1; ~1.8× Nyquist for the ~150 ms beat decay).
/// The timer stays live once installed so it can observe `playing`; the
/// IPC itself is gated — zero-cost idle is the DockProgress lesson.
///
/// Pref is `Prefs.shared.dockIconMode`: "off" | "hidden" | "playing"
/// (owned by SettingsView — read live each tick, no mirror needed).
final class DockVizDriver {
    private var timer: Timer?
    private let view = DockIconView()
    private var installed = false
    private var pushedRest = false

    private var mode: String { Prefs.shared.dockIconMode }
    private var reduceMotion: Bool {
        NSWorkspace.shared.accessibilityDisplayShouldReduceMotion
    }

    /// Re-evaluate install state — called from the 30 Hz poll (lazy
    /// install on the first playing edge) and keeps enforcing it (pref →
    /// "off" or Reduce Motion tears down at any point). The tile stays
    /// the asset icon until there's music.
    func sync() {
        let blocked = mode == "off" || reduceMotion
        if !installed && !blocked && ViewModel.shared.playing {
            install()
        } else if installed && blocked {
            teardown()
        }
    }

    private func install() {
        view.frame = NSRect(origin: .zero, size: NSApp.dockTile.size)
        view.autoresizingMask = [.width, .height]
        NSApp.dockTile.contentView = view
        installed = true
        pushedRest = false
        let t = Timer(timeInterval: 1.0 / 12, repeats: true) { [weak self] _ in
            self?.tick()
        }
        RunLoop.main.add(t, forMode: .common) // keep ticking under menus
        timer = t
        NSApp.dockTile.display() // land the parked scene immediately
    }

    private func teardown() {
        timer?.invalidate()
        timer = nil
        NSApp.dockTile.contentView = nil
        NSApp.dockTile.display() // back to the asset icon
        installed = false
        pushedRest = false
    }

    // the process dropping the tile also restores the asset icon
    deinit { timer?.invalidate() }

    /// Any visible titled window (the MenuBarExtra panel doesn't count).
    private var windowVisible: Bool {
        NSApp.windows.contains { w in
            w.styleMask.contains(.titled) && w.isVisible
                && !w.isMiniaturized
                && w.occlusionState.contains(.visible)
        }
    }

    private func tick() {
        if reduceMotion {
            teardown() // flipped mid-session — the scene is decorative
            return
        }
        let scene = ViewModel.shared.viz.cosmos
        let playing = ViewModel.shared.playing
        let wantShow = mode != "off"
            && (playing || !scene.isAtRest)
            && (mode == "playing" || !windowVisible)
        // When the tile shouldn't be animating we still drive the scene
        // with a rest frame — it slews to park so the final pushed frame
        // is the true icon composition, not a frozen mid-orbit moment.
        let f = wantShow
            ? ViewModel.shared.viz.pumpIfStale(.milliseconds(80))
            : .rest
        view.vizFrame = f
        scene.drive(f: f, dt: 1.0 / 12)
        if wantShow {
            NSApp.dockTile.display()
            pushedRest = scene.isAtRest
        } else if !pushedRest && scene.isAtRest {
            NSApp.dockTile.display() // land the settled frame, then go silent
            pushedRest = true
        }
    }
}
