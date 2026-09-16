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

    /// Pure read for non-pump surfaces — the compositor advances exactly
    /// once per tick via `pump()`, called by the always-mounted mini.
    /// Reading here between pumps returns the same eased frame.
    func frame() -> VizFrame { displayed }

    /// Advance the compositor one step. Fresh seq → ease toward the target
    /// (extra smoothing on top of the Rust ballistics); stalled seq → decay
    /// to rest (cliamp's pause behavior: settle, then freeze).
    func pump() -> VizFrame {
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
        let canvas = TimelineView(.animation(minimumInterval: 1.0 / 60)) { _ in
            Canvas { ctx, size in
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
