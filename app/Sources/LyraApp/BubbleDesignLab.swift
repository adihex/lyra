import SwiftUI

/// Design Lab — the Bubblegum design system, live inside Lyra.
/// Two surfaces:
///   * Layout Canvas — an interactive mock key matrix that exercises
///     the controls end-to-end with purely local state (no audio, no
///     ViewModel, no Rust core).
///   * Components — a diagnostic catalog that renders every Bubblegum
///     control in Normal / Pressed / Hovered / Disabled via the
///     `bubblePreviewState` environment snapshot, plus a live row of
///     real controls for genuine pointer/keyboard testing.
///
/// State is held in ObservableObject models — object-backed state
/// stays stable across view recomposition. The lab owns the canvas
/// model so switching tabs doesn't reset the layout.

/// Top-level tabs for the Design Lab page.
enum BubbleLabTab: String, CaseIterable, Identifiable {
    case canvas = "Layout Canvas"
    case components = "Components"
    var id: String { rawValue }
    var title: String { rawValue }
}

/// Which lab surface is showing — persisted for the window's lifetime.
final class BubbleLabModel: ObservableObject {
    @Published var tab: BubbleLabTab = .canvas
}

struct BubbleDesignLabView: View {
    @StateObject private var lab = BubbleLabModel()
    /// Owned here, passed down as @ObservedObject — tab switches must
    /// not reset the demo layout.
    @StateObject private var canvas = BubbleCanvasState()
    @ObservedObject private var theme = LyraTheme.shared

    var body: some View {
        VStack(alignment: .leading, spacing: Bubble.Space.lg) {
            VStack(alignment: .leading, spacing: Bubble.Space.xs) {
                Text("Hardware, softened.")
                    .font(.uiTitle)
                    .foregroundStyle(Color.bubbleLegend)
                    .accessibilityAddTraits(.isHeader)
                Text("Lyra's Bubblegum keycaps — press the mock deck, then audit every control state in the component catalog.")
                    .font(.uiCaption)
                    .foregroundStyle(Ui.inkSoft)
            }
            // Compact theme controls live here too so the lab can be QA'd
            // without a trip to Settings — same shared store bindings.
            ViewThatFits(in: .horizontal) {
                HStack(spacing: Bubble.Space.lg) { labPicker; themePickers }
                VStack(alignment: .leading, spacing: Bubble.Space.sm) {
                    labPicker
                    themePickers
                }
            }

            switch lab.tab {
            case .canvas:
                BubbleMatrixCanvasView(model: canvas)
            case .components:
                ComponentPlaygroundView()
            }
        }
        .padding(Bubble.Space.page)
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
        .background(Color.bubbleChassis)
    }

    private var labPicker: some View {
        Picker("Design Lab section", selection: $lab.tab) {
            ForEach(BubbleLabTab.allCases) { tab in
                Text(tab.title).tag(tab)
            }
        }
        .pickerStyle(.segmented)
        .tint(Color.bubbleNativeSelectionTint)
        .labelsHidden()
        .frame(maxWidth: Bubble.Size.sidebarMax)
    }

    private var themePickers: some View {
        HStack(spacing: Bubble.Space.md) {
            Picker("App appearance", selection: $theme.appearance) {
                ForEach(LyraAppearance.allCases) { a in
                    Text(a.title).tag(a)
                }
            }
            .pickerStyle(.segmented)
            .tint(Color.bubbleNativeSelectionTint)
            .fixedSize()
            Picker("Palette", selection: $theme.palette) {
                ForEach(LyraPalette.allCases) { p in
                    HStack {
                        PaletteSwatch(palette: p)
                        Text(p.title)
                    }
                    .tag(p)
                }
            }
            .pickerStyle(.menu)
            .fixedSize()
        }
    }
}

/// Small palette dot for pickers — accent over keycap with the
/// palette's border rim; resolves in the view's own appearance.
struct PaletteSwatch: View {
    let palette: LyraPalette
    @Environment(\.colorScheme) private var scheme

    var body: some View {
        let c = palette.colors(dark: scheme == .dark)
        ZStack {
            Circle().fill(Color(bubbleHex: c.keycap))
            Circle().fill(Color(bubbleHex: c.accent))
                .padding(Bubble.Space.xs)
        }
        .frame(width: Bubble.Size.swatch, height: Bubble.Size.swatch)
        .overlay(Circle().strokeBorder(Color(bubbleHex: c.border),
                                       lineWidth: Bubble.Stroke.fine))
        .accessibilityHidden(true)
    }
}

// MARK: - Layout canvas

/// Interactive mock deck: a 3×3 key matrix in a tray, a wide "space
/// bar" sequence key, and an inspector tray of layout controls. Wide
/// windows get matrix | inspector side by side; narrow windows stack.
struct BubbleMatrixCanvasView: View {
    @ObservedObject var model: BubbleCanvasState
    @ObservedObject private var theme = LyraTheme.shared

    var body: some View {
        ScrollView(.vertical) {
            ViewThatFits(in: .horizontal) {
                // Fixed minimums only on the horizontal candidate so
                // ViewThatFits can genuinely choose the fallback.
                HStack(alignment: .top, spacing: Bubble.Space.xl) {
                    matrixTray
                        .frame(minWidth: Bubble.Size.canvasMin)
                    inspectorTray
                        .frame(width: Bubble.Size.inspector)
                }
                VStack(alignment: .leading, spacing: Bubble.Space.xl) {
                    matrixTray
                    inspectorTray
                }
            }
            .frame(maxWidth: .infinity, alignment: .leading)
        }
    }

    private var matrixTray: some View {
        VStack(alignment: .leading, spacing: Bubble.Space.md) {
            Text(model.name.trimmingCharacters(in: .whitespaces).isEmpty
                 ? "Untitled matrix" : model.name)
                .font(.uiHeadline)
                .foregroundStyle(Color.bubbleLegend)
                .accessibilityAddTraits(.isHeader)
            Text(model.selectedTitle)
                .font(.uiCaption)
                .foregroundStyle(Ui.inkSoft)

            LazyVGrid(
                columns: Array(
                    repeating: GridItem(.fixed(Bubble.Size.key), spacing: model.density.gap),
                    count: Bubble.Demo.columns),
                spacing: model.density.gap
            ) {
                ForEach(model.pads) { pad in
                    VStack(spacing: Bubble.Space.xs) {
                        Button { model.activate(pad.id) } label: {
                            Image(systemName: pad.symbol)
                                .font(.bubbleGlyph)
                        }
                        .buttonStyle(BubbleButtonStyle(
                            shape: .circle, selected: model.selectedID == pad.id))
                        .disabled(!model.controlsEnabled)
                        .accessibilityLabel(pad.title)
                        .accessibilityValue(model.selectedID == pad.id
                                            ? "Selected" : "Not selected")
                        .help(pad.title)
                        if model.showLegends {
                            Text(pad.title)
                                .font(.uiCaption)
                                .foregroundStyle(Ui.inkSoft)
                        }
                    }
                }
            }

            // The "space bar" — wide prominent key. No .keyboardShortcut
            // here: Lyra's Playback menu owns Space globally, and a
            // focused button already activates on Space via macOS.
            Button { model.toggleRunning() } label: {
                Label(model.running ? "Pause sequence" : "Start sequence",
                      systemImage: model.running ? "pause.fill" : "play.fill")
                    .frame(maxWidth: .infinity)
                    .frame(minHeight: Bubble.Size.spaceBarHeight)
            }
            .buttonStyle(.bubbleProminent)
            .disabled(!model.controlsEnabled)

            // Honest footer — this deck is a layout toy, not an instrument.
            Text(model.running ? "Sequence active" : "Sequence idle")
                .font(.uiCaption)
                .foregroundStyle(Color.bubbleLegend)
            Text("Local interaction demo — no audio is triggered.")
                .font(.uiMicro)
                .foregroundStyle(Ui.inkSoft)
        }
        .bubbleTray()
    }

    private var inspectorTray: some View {
        VStack(alignment: .leading, spacing: Bubble.Space.md) {
            Text("Layout controls")
                .font(.uiHeadline)
                .foregroundStyle(Color.bubbleLegend)
                .accessibilityAddTraits(.isHeader)

            TextField("Matrix name", text: $model.name)
                .textFieldStyle(.bubble)

            Picker("Key spacing", selection: $model.density) {
                ForEach(BubbleDensity.allCases) { d in
                    Text(d.rawValue).tag(d)
                }
            }
            .pickerStyle(.menu)

            Group {
                Toggle("Show legends", isOn: $model.showLegends)
                // Always enabled — it's the way back from a disabled deck.
                Toggle("Enable keys", isOn: $model.controlsEnabled)
            }
            .toggleStyle(.bubble)

            HStack(spacing: Bubble.Space.sm) {
                Button("Move left") { model.move(-1) }
                    .buttonStyle(.bubble)
                    .disabled(!model.canMove(-1))
                Button("Move right") { model.move(1) }
                    .buttonStyle(.bubble)
                    .disabled(!model.canMove(1))
            }

            Text("Selected: \(model.selectedTitle)")
                .font(.uiCaption)
                .foregroundStyle(Ui.inkSoft)

            // Demo-only level — local slider value, wired to nothing.
            Slider(value: $model.level, in: Bubble.Demo.levelRange) {
                Text("Demo level")
            }
            .tint(Color.bubbleLegend)
            Text("Demo level \(model.level, format: .percent.precision(.fractionLength(0))) — local value only")
                .font(.uiCaption)
                .foregroundStyle(Ui.inkSoft)

            Button("Reset layout") { model.reset() }
                .buttonStyle(.bubble)
        }
        .bubbleTray()
    }
}

// MARK: - Component catalog

/// Live-controls model for the bottom of the catalog — real events
/// (press count, toggle, text) so pointer/keyboard/disabled behavior
/// can be verified by hand, not just rendered.
final class BubbleLiveModel: ObservableObject {
    @Published var presses = 0
    @Published var enabled = true
    @Published var on = false
    @Published var text = "Lyra"
}

/// Diagnostic grid: every Bubblegum control rendered in each
/// BubblePreviewState. Specimen cells are inert — `.allowsHitTesting(false)`
/// + `.accessibilityHidden(true)` — with a text description carrying
/// the semantics so a snapshot can't masquerade as a working control.
/// The environment override simulates state visuals; it is NOT actual
/// event injection. The "Live controls" row beneath the grid holds the
/// real, interactive counterparts.
struct ComponentPlaygroundView: View {
    @StateObject private var live = BubbleLiveModel()
    @ObservedObject private var theme = LyraTheme.shared

    var body: some View {
        // Both axes — the catalog is wider than the minimum window.
        ScrollView([.horizontal, .vertical]) {
            VStack(alignment: .leading, spacing: Bubble.Space.xl) {
                Text("Component catalog")
                    .font(.uiHeadline)
                    .foregroundStyle(Color.bubbleLegend)
                    .accessibilityAddTraits(.isHeader)
                Text("Simulated states — cells are inert snapshots, not working controls.")
                    .font(.uiCaption)
                    .foregroundStyle(Ui.inkSoft)

                Grid(alignment: .leading,
                     horizontalSpacing: Bubble.Space.lg,
                     verticalSpacing: Bubble.Space.xl) {
                    GridRow {
                        Text("Component")
                            .frame(width: Bubble.Size.catalogLabel, alignment: .leading)
                        ForEach(BubblePreviewState.allCases) { state in
                            Text(state.rawValue)
                                .frame(width: Bubble.Size.catalogCell, alignment: .leading)
                        }
                    }
                    .font(.uiCaption)
                    .foregroundStyle(Ui.inkSoft)

                    GridRow { label("Circular key"); cells { state in
                        Button {} label: {
                            Image(systemName: "star.fill").font(.bubbleGlyph)
                        }
                        .buttonStyle(BubbleButtonStyle(shape: .circle))
                        .bubbleSnapshot(state)
                    } }

                    GridRow { label("Pill key"); cells { state in
                        Button("Keycap") {}
                            .buttonStyle(.bubble)
                            .bubbleSnapshot(state)
                    } }

                    GridRow { label("Prominent pill"); cells { state in
                        Button("Primary") {}
                            .buttonStyle(.bubbleProminent)
                            .bubbleSnapshot(state)
                    } }

                    GridRow { label("Toggle"); cells { state in
                        Toggle(isOn: .constant(state == .pressed)) {
                            Text("Keys")
                        }
                        .toggleStyle(.bubble)
                        .bubbleSnapshot(state)
                    } }

                    GridRow { label("Text field"); cells { state in
                        TextField("Matrix name", text: .constant("Lyra"))
                            .textFieldStyle(.bubble)
                            .frame(width: Bubble.Size.catalogCell)
                            .bubbleSnapshot(state)
                    } }

                    // Tray is passive chrome — its "states" are the
                    // states of the child key inside it.
                    GridRow { label("Tray"); cells { state in
                        VStack(spacing: Bubble.Space.sm) {
                            Button {} label: {
                                Image(systemName: "command").font(.uiHeadline)
                            }
                            .buttonStyle(BubbleButtonStyle(
                                shape: .circle, diameter: Bubble.Size.compactKey))
                            Text("Passive container — state is the child's")
                                .font(.uiMicro)
                                .foregroundStyle(Ui.inkSoft)
                        }
                        .bubbleTray(padding: Bubble.Space.sm)
                        .bubbleSnapshot(state)
                    } }
                }

                Divider()

                // Real controls — pointer press/release, hover, keyboard
                // focus, and the disabled path are exercised for real.
                Text("Live controls")
                    .font(.uiHeadline)
                    .foregroundStyle(Color.bubbleLegend)
                    .accessibilityAddTraits(.isHeader)
                HStack(spacing: Bubble.Space.lg) {
                    Button { live.presses += 1 } label: {
                        Label("Press me", systemImage: "hand.tap.fill")
                    }
                    .buttonStyle(.bubbleProminent)
                    .disabled(!live.enabled)
                    Text("Pressed \(live.presses)×")
                        .font(.uiCaption)
                        .foregroundStyle(Ui.inkSoft)
                    Toggle("Live toggle", isOn: $live.on)
                        .toggleStyle(.bubble)
                        .disabled(!live.enabled)
                    VStack(alignment: .leading, spacing: Bubble.Space.xs) {
                        Text("Name")
                            .font(.uiCaption)
                            .foregroundStyle(Ui.inkSoft)
                        TextField("Live field", text: $live.text)
                            .textFieldStyle(.bubble)
                            .frame(width: Bubble.Size.searchIdeal)
                            .disabled(!live.enabled)
                            .help("Type to edit — Tab focuses it")
                    }
                }
                Toggle("Enable live samples", isOn: $live.enabled)
                    .toggleStyle(.bubble)
                Text("Toggle is \(live.on ? "on" : "off") · field says “\(live.text)”")
                    .font(.uiCaption)
                    .foregroundStyle(Ui.inkSoft)
            }
            .padding(Bubble.Space.sm)
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
    }

    private func label(_ title: String) -> some View {
        Text(title)
            .font(.uiBodyStrong)
            .foregroundStyle(Color.bubbleLegend)
            .frame(width: Bubble.Size.catalogLabel, alignment: .leading)
    }

    /// One catalog row: a descriptive (accessible) label per state cell
    /// wrapping an inert specimen view.
    private func cells<Specimen: View>(
        @ViewBuilder _ specimen: @escaping (BubblePreviewState) -> Specimen
    ) -> some View {
        ForEach(BubblePreviewState.allCases) { state in
            VStack(alignment: .leading, spacing: Bubble.Space.xs) {
                specimen(state)
                    .allowsHitTesting(false)
                    .accessibilityHidden(true)
                Text("\(state.rawValue), simulated")
                    .font(.uiMicro)
                    .foregroundStyle(Ui.inkSoft)
                    .fixedSize(horizontal: false, vertical: true)
            }
            .frame(width: Bubble.Size.catalogCell, alignment: .leading)
        }
    }
}

private extension View {
    /// Force one deterministic catalog state: the environment snapshot
    /// drives pressed/hover visuals, `.disabled` drives the disabled
    /// cell's environment too.
    func bubbleSnapshot(_ state: BubblePreviewState) -> some View {
        self
            .environment(\.bubblePreviewState, state)
            .disabled(state == .disabled)
    }
}

// Xcode previews: add LYRA_XCODE_PREVIEWS to the compilation conditions
// to enable these — the CLT Makefile build skips macro-plugin previews
// entirely and uses the live Design Lab pane instead.
#if LYRA_XCODE_PREVIEWS
#Preview("Bubblegum • Catalog — LIGHT") {
    ComponentPlaygroundView()
        .frame(width: Bubble.Size.previewWidth, height: Bubble.Size.previewHeight)
        .preferredColorScheme(.light)
}
#Preview("Bubblegum • Catalog — DARK") {
    ComponentPlaygroundView()
        .frame(width: Bubble.Size.previewWidth, height: Bubble.Size.previewHeight)
        .preferredColorScheme(.dark)
}
#Preview("Bubblegum • Layout Canvas — LIGHT") {
    BubbleDesignLabView()
        .frame(width: Bubble.Size.previewWidth, height: Bubble.Size.previewHeight)
        .preferredColorScheme(.light)
}
#Preview("Bubblegum • Layout Canvas — DARK") {
    BubbleDesignLabView()
        .frame(width: Bubble.Size.previewWidth, height: Bubble.Size.previewHeight)
        .preferredColorScheme(.dark)
}
#endif
