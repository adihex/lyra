import SwiftUI

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
        ContinuousClock.now - lastPump > maxAge ? pump() : displayed
    }

    /// Advance the compositor one step. Fresh seq → ease toward the target
    /// (extra smoothing on top of the Rust ballistics); stalled seq → decay
    /// to rest (cliamp's pause behavior: settle, then freeze).
    func pump() -> VizFrame {
        lastPump = .now
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
                f = vm.viz.pump()
                st = vm.viz.miniState
            } else {
                f = vm.viz.frame()
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
        let canvas = Group {
            if vm.playing {
                TimelineView(.animation(
                    minimumInterval: compact ? 1.0 / 30 : 1.0 / 60)) { _ in
                    core
                }
            } else {
                core
            }
        }
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
