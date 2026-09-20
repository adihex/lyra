import SwiftUI

/// Bubblegum controls — native `ButtonStyle` / `ToggleStyle` /
/// `TextFieldStyle` / `ViewModifier` implementations, so every control
/// keeps its platform interaction and accessibility contract (keyboard
/// activation, focus ring, on/off AX role, disabled gating) while
/// wearing the keycap look. Inspiration: NeoBrutalism's "theme + native
/// style" split — the press visuals react to `configuration.isPressed`
/// rather than being toggled by a custom gesture.
///
/// Durable UI state uses ObservableObject + @StateObject/@ObservedObject
/// throughout — object-backed state stays stable across the view
/// recomposition these style bodies go through.
/// Color-bearing views observe `LyraTheme.shared` so palette/scheme
/// changes repaint without remounting.

/// Diagnostic-only state override for the component catalog. Production
/// controls read real `isPressed` / hover / focus; a non-nil snapshot
/// forces one deterministic state so the catalog can render Pressed /
/// Hovered / Disabled cells without injecting fake events.
enum BubblePreviewState: String, CaseIterable, Identifiable {
    case normal = "Normal"
    case pressed = "Pressed / Active"
    case hovered = "Hovered"
    case disabled = "Disabled"
    var id: String { rawValue }
}

private struct BubblePreviewKey: EnvironmentKey {
    static let defaultValue: BubblePreviewState? = nil
}

extension EnvironmentValues {
    var bubblePreviewState: BubblePreviewState? {
        get { self[BubblePreviewKey.self] }
        set { self[BubblePreviewKey.self] = newValue }
    }
}

/// Hover tracker — ObservableObject so the flag survives recomposition.
private final class BubbleHover: ObservableObject {
    @Published var inside = false
}

/// Keycap button style — a white (or accent, when `prominent`/`selected`)
/// capsule on the tray. Shadowless: pressing scales the cap to
/// `Motion.pressedScale` and sinks it `Depth.travel`; hovering lifts it
/// `Depth.hoverLift` and strengthens the rim. `shape` switches between
/// a fixed-diameter circle and a padded pill.
struct BubbleButtonStyle: ButtonStyle {
    enum Shape { case pill, circle }
    var prominent = false
    var shape: Shape = .pill
    /// Fixed width+height when `shape == .circle`.
    var diameter: CGFloat = Bubble.Size.key
    /// Selected/held look — same accent fill as `prominent`, for
    /// toggleable keys (sidebar selection, matrix pads, switch track).
    var selected = false

    func makeBody(configuration: Configuration) -> some View {
        BubbleButtonBody(label: configuration.label,
                         pressed: configuration.isPressed,
                         prominent: prominent, shape: shape,
                         diameter: diameter, selected: selected)
    }
}

private struct BubbleButtonBody<Label: View>: View {
    let label: Label
    let pressed: Bool
    let prominent: Bool
    let shape: BubbleButtonStyle.Shape
    let diameter: CGFloat
    let selected: Bool

    @ObservedObject private var theme = LyraTheme.shared
    @Environment(\.isEnabled) private var enabled
    @Environment(\.isFocused) private var focused
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    @Environment(\.colorSchemeContrast) private var contrast
    @Environment(\.bubblePreviewState) private var snapshot
    @StateObject private var hover = BubbleHover()

    private var active: Bool { prominent || selected }

    /// A forced catalog snapshot wins over live input so diagnostics are
    /// deterministic; nil = real interaction. Disabled always gates
    /// hover/press even if a stale snapshot says otherwise.
    private var effectivePressed: Bool {
        guard enabled else { return false }
        if let snap = snapshot { return snap == .pressed }
        return pressed
    }

    private var effectiveHover: Bool {
        guard enabled else { return false }
        if let snap = snapshot { return snap == .hovered }
        return hover.inside
    }

    var body: some View {
        label
            .font(.uiBodyStrong)
            .foregroundStyle(active ? Color.bubbleOnAccent : Color.bubbleLegend)
            .modifier(BubbleKeyFrame(shape: shape, diameter: diameter))
            .background(
                Capsule(style: .continuous)
                    .fill(active ? Color.bubbleAccent : Color.bubbleKeycap)
            )
            .overlay {
                // White inner rim — the keycap's top edge highlight.
                Capsule(style: .continuous)
                    .strokeBorder(Bubble.innerRim, lineWidth: Bubble.Stroke.fine)
                    .allowsHitTesting(false)
            }
            .overlay {
                // Outer seam: strong legend line on hover or under
                // Increase Contrast, a quiet rim at rest.
                Capsule(style: .continuous)
                    .strokeBorder(
                        (effectiveHover || contrast == .increased)
                            ? Color.bubbleLegend : Bubble.trayRim,
                        lineWidth: contrast == .increased
                            ? Bubble.Stroke.contrast : Bubble.Stroke.fine)
                    .allowsHitTesting(false)
            }
            // Physical press, shadowless: cap scales down and sinks
            // `travel` points. The transform sits AFTER the rim overlays
            // so cap and rims depress as one piece; Reduce Motion drops
            // transform + animation.
            .scaleEffect(!reduceMotion && effectivePressed
                         ? Bubble.Motion.pressedScale : Bubble.Motion.restScale)
            .offset(y: reduceMotion ? Bubble.Space.zero
                    : effectivePressed ? Bubble.Depth.travel
                    : effectiveHover ? Bubble.Depth.hoverLift : Bubble.Space.zero)
            .overlay {
                // Focus ring sits outside the keycap rim — it is the
                // stationary outer marker and deliberately does NOT
                // depress with the cap. Never clipped, never needs
                // .focusable().
                if focused {
                    Capsule(style: .continuous)
                        .strokeBorder(Color.bubbleLegend, lineWidth: Bubble.Stroke.focus)
                        .padding(-Bubble.Stroke.inset)
                        .allowsHitTesting(false)
                }
            }
            .contentShape(Capsule(style: .continuous))
            .opacity(enabled ? Bubble.Opacity.full : Bubble.Opacity.disabled)
            .animation(reduceMotion ? nil : Bubble.Motion.spring,
                       value: effectivePressed)
            .animation(reduceMotion ? nil : Bubble.Motion.spring,
                       value: effectiveHover)
            .animation(reduceMotion ? nil : Bubble.Motion.spring,
                       value: selected)
            .onHover { inside in
                // Always record — a disable mid-hover would otherwise
                // leave a stale `inside` that reactivates on re-enable.
                hover.inside = inside
            }
            .onChange(of: enabled) { _, isEnabled in
                if !isEnabled { hover.inside = false }
            }
    }
}

/// Keycap silhouette + sizing: equal width/height for circles, padded
/// pill height for pills. Kept as a modifier so both branches share the
/// label upstream.
private struct BubbleKeyFrame: ViewModifier {
    let shape: BubbleButtonStyle.Shape
    let diameter: CGFloat
    func body(content: Content) -> some View {
        switch shape {
        case .circle:
            content.frame(width: diameter, height: diameter)
        case .pill:
            content
                .padding(.horizontal, Bubble.Space.md)
                .frame(minHeight: Bubble.Size.pillHeight)
        }
    }
}

/// Keycap toggle — the native Toggle's label row plus a recessed track
/// and sliding thumb. Implemented as a Button wearing
/// `BubbleButtonStyle(selected:)` so the whole selector presses like a
/// key; `accessibilityRepresentation` hands VoiceOver a real switch
/// (on/off role + toggle action) so no duplicate AX children leak.
struct BubbleToggleStyle: ToggleStyle {
    func makeBody(configuration: Configuration) -> some View {
        Button { configuration.isOn.toggle() } label: {
            HStack(spacing: Bubble.Space.sm) {
                configuration.label
                    .fixedSize(horizontal: true, vertical: false)
                Spacer(minLength: Bubble.Space.zero)
                BubbleSwitchThumb(isOn: configuration.isOn)
            }
        }
        .buttonStyle(BubbleButtonStyle(selected: configuration.isOn))
        .accessibilityRepresentation {
            Toggle(isOn: configuration.$isOn) { configuration.label }
                .toggleStyle(.switch)
        }
    }
}

/// Recessed capsule track with a white thumb that slides leading →
/// trailing. On/off is shown by the thumb glyph (checkmark vs minus) —
/// never by color alone.
private struct BubbleSwitchThumb: View {
    let isOn: Bool
    @ObservedObject private var theme = LyraTheme.shared
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    var body: some View {
        Capsule(style: .continuous)
            .fill(isOn ? Color.bubbleAccent : Color.bubbleTray)
            .frame(width: Bubble.Size.toggleWidth, height: Bubble.Size.toggleHeight)
            .overlay(alignment: isOn ? .trailing : .leading) {
                Circle()
                    .fill(Color.bubbleKeycap)
                    .overlay {
                        Image(systemName: isOn ? "checkmark" : "minus")
                            .font(.uiMicro.weight(.bold))
                            .foregroundStyle(Color.bubbleLegend)
                    }
                    .frame(width: Bubble.Size.thumb, height: Bubble.Size.thumb)
                    .padding(Bubble.Space.xs)
            }
            .animation(reduceMotion ? nil : Bubble.Motion.spring, value: isOn)
    }
}

/// Recessed tray — the lavender deck a group of keys sits in. Matte
/// tray fill against the chassis plus a thin inset rim reads as a
/// routed recess without any shadow rendering; the rim goes solid
/// legend under Increase Contrast.
struct BubbleTrayModifier: ViewModifier {
    var padding: CGFloat = Bubble.Space.lg
    @ObservedObject private var theme = LyraTheme.shared
    @Environment(\.colorSchemeContrast) private var contrast
    func body(content: Content) -> some View {
        content
            .padding(padding)
            .background {
                RoundedRectangle(cornerRadius: Bubble.Size.trayRadius, style: .continuous)
                    .fill(Color.bubbleTray)
            }
            .overlay {
                // Inset inner rim — the recess edge of the routed tray.
                RoundedRectangle(cornerRadius: Bubble.Size.trayRadius, style: .continuous)
                    .strokeBorder(Color.bubbleKeycap.opacity(Bubble.Opacity.rim),
                                  lineWidth: Bubble.Stroke.fine)
                    .padding(Bubble.Depth.inset)
                    .allowsHitTesting(false)
            }
            .overlay {
                RoundedRectangle(cornerRadius: Bubble.Size.trayRadius, style: .continuous)
                    .strokeBorder(
                        contrast == .increased ? Color.bubbleLegend : Bubble.trayRim,
                        lineWidth: contrast == .increased
                            ? Bubble.Stroke.contrast : Bubble.Stroke.fine)
                    .allowsHitTesting(false)
            }
    }
}

/// Token-chromed native text field — the real `TextField` (plain style)
/// keeps its own editing behavior; the chrome adds the white field well
/// plus a rim that strengthens on hover and a full legend ring on
/// keyboard focus. No gestures — `.focused` links a real @FocusState.
struct BubbleTextFieldStyle: TextFieldStyle {
    func _body(configuration: TextField<Self._Label>) -> some View {
        BubbleFieldChrome { configuration }
    }
}

private struct BubbleFieldChrome<Content: View>: View {
    @ViewBuilder let content: Content
    @ObservedObject private var theme = LyraTheme.shared
    @StateObject private var hover = BubbleHover()
    @FocusState private var focused: Bool
    @Environment(\.isEnabled) private var enabled
    @Environment(\.colorSchemeContrast) private var contrast
    @Environment(\.bubblePreviewState) private var snapshot

    /// Catalog semantics for a field: `.pressed` is rendered as the
    /// focused/active state (text fields have no press), `.hovered` as
    /// hover, `.disabled` rides the environment's isEnabled.
    private var rimmed: Bool {
        guard enabled else { return false }
        if let snap = snapshot {
            return snap == .pressed || snap == .hovered
        }
        return focused || hover.inside
    }

    var body: some View {
        content
            .font(.uiBody)
            .foregroundStyle(Color.bubbleLegend)
            .textFieldStyle(.plain)
            .focused($focused)
            .padding(.horizontal, Bubble.Space.md)
            .padding(.vertical, Bubble.Space.sm)
            .background {
                RoundedRectangle(cornerRadius: Bubble.Size.fieldRadius, style: .continuous)
                    .fill(Color.bubbleKeycap)
            }
            .overlay {
                RoundedRectangle(cornerRadius: Bubble.Size.fieldRadius, style: .continuous)
                    .strokeBorder(
                        rimmed || contrast == .increased
                            ? Color.bubbleLegend : Bubble.trayRim,
                        lineWidth: rimmed || contrast == .increased
                            ? Bubble.Stroke.contrast : Bubble.Stroke.fine)
                    .allowsHitTesting(false)
            }
            .opacity(enabled ? Bubble.Opacity.full : Bubble.Opacity.disabled)
            .onHover { hover.inside = $0 }
            .onChange(of: enabled) { _, isEnabled in
                if !isEnabled { hover.inside = false }
            }
    }
}

extension View {
    /// Wrap content in a recessed Bubblegum tray.
    func bubbleTray(padding: CGFloat = Bubble.Space.lg) -> some View {
        modifier(BubbleTrayModifier(padding: padding))
    }
}

extension ButtonStyle where Self == BubbleButtonStyle {
    /// White keycap pill.
    static var bubble: BubbleButtonStyle { .init() }
    /// Accent-filled keycap pill — the one primary action in a view.
    static var bubbleProminent: BubbleButtonStyle { .init(prominent: true) }
}

extension ToggleStyle where Self == BubbleToggleStyle {
    static var bubble: BubbleToggleStyle { .init() }
}

extension TextFieldStyle where Self == BubbleTextFieldStyle {
    static var bubble: BubbleTextFieldStyle { .init() }
}
