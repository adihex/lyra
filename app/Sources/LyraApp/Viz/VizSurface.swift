import SwiftUI
import OSLog

private let vizLog = OSLog(subsystem: "ai.lyra.debug", category: "viz")

/// The single viz metronome. TimelineView proved unreliable here: it
/// mounts but never ticks inside Button labels (the transport mini) and
/// `.animation` sleeps on Canvas content (no animatable attributes) —
/// before the perf pass, the 30 Hz @Published storm was what actually
/// drove these canvases. A Timer-published tick on a dedicated object
/// can't sleep, reaches label slots, and costs nothing at rest because
/// it only runs while playing (or mid-scrub).
final class VizTicker: ObservableObject {
    static let shared = VizTicker()
    /// Read by every ticking surface's body — the publish IS the tick.
    @Published var seq = 0
    private var timer: Timer?
    private init() {}

    /// Idempotent — called every 30 Hz poll. 30 Hz is enough for every
    /// consumer (mini/full viz, menu-bar canvases, seek, EQ underlay);
    /// the full surface still reads at its own frame, not per tick.
    func sync(_ wantLive: Bool) {
        if wantLive && timer == nil {
            let t = Timer(timeInterval: 1.0 / 30, repeats: true) {
                [weak self] _ in self?.seq &+= 1
            }
            RunLoop.main.add(t, forMode: .common) // ticks under menus too
            timer = t
            os_log("viz: ticker started", log: vizLog)
        } else if !wantLive && timer != nil {
            timer?.invalidate()
            timer = nil
            os_log("viz: ticker stopped", log: vizLog)
        }
    }
}

/// Frame compositor: real engine frames when viz-core's symbol exists,
/// the deterministic mock otherwise — same VizFrame type either way.
/// Plain class (not ObservableObject): TimelineView drives the cadence,
/// so nothing here publishes per frame.
final class VizRuntime {
    let state = VizState()
    /// State for secondary surfaces (transport-bar mini). Multiple surfaces
    /// must not share one VizState — each would tick it once per frame.
    let miniState = VizState()
    private let mock = MockVizFrameProvider()
    private(set) var engineLive = false
    private var displayed = VizFrame.rest
    private var lastSeq: UInt64 = 0
    private var pumpCount: Int64 = 0
    private var stalePumpCount: Int64 = 0
    private var miniRenderCount: Int64 = 0

    /// Compact-surface render heartbeat — proves whether the mini's
    /// TimelineView actually ticks its Canvas (vs just being mounted).
    func noteMiniRender() {
        miniRenderCount += 1
        if miniRenderCount % 90 == 1 {
            os_log("viz: miniRender n=%{public}lld", log: vizLog, miniRenderCount)
        }
    }

    /// Full-surface render heartbeat — same proof for the stage canvas.
    private var fullRenderCount: Int64 = 0
    func noteFullRender() {
        fullRenderCount += 1
        if fullRenderCount % 90 == 1 {
            os_log("viz: fullRender n=%{public}lld", log: vizLog, fullRenderCount)
        }
    }

    /// Shared cosmos scene for the dock tile + desktop pet surfaces.
    /// The pet adopts it on pop and hands it back on recall; painters
    /// must skip bodies in `cosmos.away` (the icon literally empties).
    var cosmos = CosmosScene()
    private var lastPump = ContinuousClock.now

    /// Pure read for non-pump surfaces — the compositor advances exactly
    /// once per tick via `pump()`, called by the always-mounted mini.
    /// Reading here between pumps returns the same eased frame.
    func frame() -> VizFrame { displayed }

    /// Advance only when the compositor has gone stale — the dock-tile
    /// driver calls this at 12 Hz: while the window's mini surface pumps
    /// it's a no-op read; when the window is occluded the dock becomes
    /// the pump. Two unconditional pumpers would double-ease the frame.
    func pumpIfStale(_ maxAge: Duration) -> VizFrame {
        if ContinuousClock.now - lastPump > maxAge {
            stalePumpCount += 1
            if stalePumpCount % 60 == 1 {
                os_log("viz: stalePump n=%{public}lld", log: vizLog, stalePumpCount)
            }
            return pump()
        }
        return displayed
    }

    /// Advance the compositor one step. Fresh seq → ease toward the target
    /// (extra smoothing on top of the Rust ballistics); stalled seq → decay
    /// to rest (cliamp's pause behavior: settle, then freeze).
    func pump() -> VizFrame {
        lastPump = .now
        pumpCount += 1
        if pumpCount % 90 == 1 {
            os_log("viz: pump n=%{public}lld seq=%{public}lld live=%{public}@ lvl=%{public}.3f bass=%{public}.3f b10=%{public}.3f wv=%{public}.3f",
                   log: vizLog, pumpCount, lastSeq,
                   engineLive ? "y" : "n",
                   displayed.level, displayed.bass,
                   displayed.bands[10], displayed.waveL[128])
        }
        if let f = LyraEngine.vizFrame() {
            engineLive = true
            if f.seq != lastSeq {
                lastSeq = f.seq
                displayed = displayed.eased(toward: f, 0.55)
            } else {
                displayed = displayed.decayed(0.94)
            }
        } else {
            engineLive = false
            if let f = mock.nextFrame() {
                displayed = displayed.eased(toward: f, 0.55)
                lastSeq = f.seq
            } else {
                displayed = displayed.decayed(0.94)
            }
        }
        return displayed
    }

    /// Seconds shorthand for `pumpIfStale` call sites (`pumpIfStale(0.08)`).
    func pumpIfStale(_ seconds: Double) -> VizFrame {
        pumpIfStale(.seconds(seconds))
    }
}

/// The animated viz surface — one Canvas per mode driven by a 60Hz
/// timeline. `accessibilityReduceMotion`: decorative modes collapse to
/// Bars; meters/scope/wave stay because they convey information.
struct VizSurfaceView: View {
    var mode: VizMode
    /// Transport-bar thumbnail: this surface is the compositor pump (it's
    /// always mounted), uses miniState, hides the mock badge, and takes no
    /// gestures so it can sit inside a Button.
    var compact = false
    @ObservedObject private var vm = ViewModel.shared
    @ObservedObject private var ticker = VizTicker.shared
    /// Palette/scheme changes invalidate the Canvas's cached frame too.
    @ObservedObject private var theme = LyraTheme.shared
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    private var effective: VizMode {
        reduceMotion && mode.decorative ? .bars : mode
    }

    var body: some View {
        // Each TimelineView tick is a full scene transaction — and every
        // transaction piggybacks a main-menu rebuild (makeMainMenu was ~40%
        // of a core when this ran 60 Hz *at rest*). So the metronome only
        // exists while playing; at rest the Canvas draws its decayed frame
        // once, statically. Compact strip ticks at 30 Hz — plenty for a
        // transport-bar mini viz.
        let core = Canvas { ctx, size in
            let m = effective
            let f: VizFrame
            let st: VizState
            if compact {
                vm.viz.noteMiniRender()
                f = vm.viz.pump()
                st = vm.viz.miniState
            } else {
                // Self-healing read: the mini is the designated pump, but
                // it can collapse to 0 width (narrow window) or pause under
                // occlusion. If no pump ran for 80 ms this surface becomes
                // the pump — same pattern the dock tile uses.
                vm.viz.noteFullRender()
                f = vm.viz.pumpIfStale(.milliseconds(80))
                st = vm.viz.state
            }
            st.tick(frame: f, mode: m)
            VizDraw.render(m, &ctx, size, f, st)
            if !compact && !vm.viz.engineLive {
                ctx.draw(
                    ctx.resolve(Text("mock frames").font(.uiMicro)
                                .foregroundColor(Ui.inkSoft.opacity(0.55))),
                    at: CGPoint(x: 6, y: size.height - 8), anchor: .leading)
            }
        }
        // No TimelineView anywhere in this path: `ticker` invalidates the
        // body while playing, the Canvas re-renders, the mini pumps.
        // At rest nothing ticks — the Canvas drew its last frame on the
        // playing edge and stays put. Works inside Button labels.
        let canvas = Group { core }
        if compact {
            canvas
        } else {
            canvas
            .contentShape(Rectangle())
            .gesture(DragGesture(minimumDistance: 0)
            .onChanged { g in
                guard effective == .waveSeek else { return }
                vm.scrubbing = true
                let dur = max(vm.current?.duration ?? 1, 1)
                let w = max(vm.viz.state.seekSize.width, 1)
                vm.displayPosition = Double(min(max(g.location.x / w, 0), 1)) * dur
            }
            .onEnded { _ in
                if effective == .waveSeek { vm.scrubEnded() }
            })
        }
    }
}
