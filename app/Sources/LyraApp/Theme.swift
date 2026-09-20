import SwiftUI

/// Bubblegum / Hardware-Pop theme — inspired by pastel mechanical
/// keyboards (Portronics Bubble): a muted pastel chassis, matte key
/// trays, keycaps that physically press down on a springy travel, and
/// rounded legends. Rendering is shadowless — tactility comes from
/// press scale, depression offset, and rim/stroke contrast.
///
/// Three layers live here:
///   * `Bubble` — the semantic token namespace (spacing, sizes, strokes,
///     depth, opacity, motion, type). All NEW views must reference these
///     tokens rather than spelling out literals.
///   * `Color.bubble*` — semantic colors, computed forwarders into
///     `LyraTheme.shared` (see LyraTheme.swift): they resolve the
///     current palette through the view's effective appearance, so a
///     view only needs to observe the store once for a palette change
///     to repaint every token it reads.
///   * `Ui` / `Font.ui*` / `.uiCard` / `.uiElevated` / `.sharp` — the
///     legacy surface names, kept as forwarding aliases so the older
///     panes (Viz contracts, library table, remote cards) compile and
///     pick up the new look without refactors.
extension Color {
    /// sRGB-hex initializer — the palette table's source format.
    init(bubbleHex hex: UInt32) {
        self.init(.sRGB,
                  red: Double((hex >> 16) & 0xff) / 255,
                  green: Double((hex >> 8) & 0xff) / 255,
                  blue: Double(hex & 0xff) / 255)
    }

    /// App chassis — the body the trays sit in.
    static var bubbleChassis: Color { LyraTheme.shared.color(.chassis) }
    /// Recessed tray plastic — the deck a key bed is cut into.
    static var bubbleTray: Color { LyraTheme.shared.color(.tray) }
    /// Pastel accent — primary keycap fill. Pair with `bubbleOnAccent`,
    /// never white.
    static var bubbleAccent: Color { LyraTheme.shared.color(.accent) }
    /// Keycap top.
    static var bubbleKeycap: Color { LyraTheme.shared.color(.keycap) }
    /// Legend ink — readable on chassis, tray and keycap; prominent
    /// (accent-filled) keys use `bubbleOnAccent` instead.
    static var bubbleLegend: Color { LyraTheme.shared.color(.legend) }
    /// Text/icons printed directly on the accent keycap.
    static var bubbleOnAccent: Color { LyraTheme.shared.color(.onAccent) }
    /// Adaptive native tint — legend-dark in light scheme, pastel in
    /// dark — for accent-fill behind `bubbleOnTint` text.
    static var bubbleTint: Color { LyraTheme.shared.color(.tint) }
    /// Text/icons on `bubbleTint`.
    static var bubbleOnTint: Color { LyraTheme.shared.color(.onTint) }
    /// Secondary ink for tags/badges.
    static var bubbleSecondaryInk: Color { LyraTheme.shared.color(.secondaryInk) }
    /// Companion hue — pet/viz accents, rim glows.
    static var bubbleCompanion: Color { LyraTheme.shared.color(.companion) }
    /// Quiet border/rim tone.
    static var bubbleBorder: Color { LyraTheme.shared.color(.border) }
    /// Viz/pet planet base — keycap in dark, legend in light; resolves
    /// through the effective appearance like every other provider.
    static var bubbleBodyDark: Color { LyraTheme.shared.bodyDark() }
    /// Native segmented controls print a white selected label; keep their
    /// fill dark in both appearances.
    static var bubbleNativeSelectionTint: Color {
        Color(bubbleHex: LyraTheme.shared.palette.colors(dark: false).tint)
    }
}

/// Semantic design tokens for the Bubblegum system. Values that carry
/// meaning (key diameter, press travel, rim opacity) are named here once;
/// views compose them instead of hard-coding numbers.
enum Bubble {
    /// Derived semantic colors — every opacity application of a palette
    /// color funnels through here so views carry no scattered literals.
    /// Shadowless design: depth comes from stroke contrast, not shadows.
    /// Computed so they follow the live palette.
    static var trayRim: Color { Color.bubbleLegend.opacity(Opacity.recess) }
    static var innerRim: Color { Color.bubbleKeycap.opacity(Opacity.rim) }
    static var quietInk: Color { Color.bubbleLegend.opacity(Opacity.quiet) }

    /// 4pt spacing grid.
    enum Space {
        static let zero: CGFloat = 0
        static let xs: CGFloat = 4
        static let sm: CGFloat = 8
        static let md: CGFloat = 12
        static let lg: CGFloat = 16
        static let xl: CGFloat = 20
        static let xxl: CGFloat = 24
        /// Pane/page margins.
        static let page: CGFloat = 28
    }

    /// Fixed hardware dimensions — keycaps, trays, window chrome.
    enum Size {
        /// Full-size round keycap diameter.
        static let key: CGFloat = 64
        /// Small round keycap (transport prev/next, icon keys).
        static let compactKey: CGFloat = 32
        /// The play key — one step up from compact.
        static let transportKey: CGFloat = 40
        /// Minimum pill-key height.
        static let pillHeight: CGFloat = 36
        /// The wide "space bar" action key under a key matrix.
        static let spaceBarHeight: CGFloat = 52
        /// Toggle switch track / thumb.
        static let toggleWidth: CGFloat = 58
        static let toggleHeight: CGFloat = 32
        static let thumb: CGFloat = 24
        /// Tray corner radius — big continuous curves, not square.
        static let trayRadius: CGFloat = 20
        /// Text-field corner radius.
        static let fieldRadius: CGFloat = 12
        /// Sidebar column + glyph widths.
        static let sidebarMin: CGFloat = 160
        static let sidebarIdeal: CGFloat = 190
        static let sidebarMax: CGFloat = 320
        static let sidebarGlyph: CGFloat = 18
        /// Main window bounds.
        static let windowMinWidth: CGFloat = 780
        static let windowMinHeight: CGFloat = 560
        static let windowWidth: CGFloat = 1040
        static let windowHeight: CGFloat = 760
        /// Component-catalog column / cell widths.
        static let catalogLabel: CGFloat = 110
        static let catalogCell: CGFloat = 132
        /// Layout-canvas inspector column.
        static let inspector: CGFloat = 240
        /// Minimum matrix width before the canvas stacks vertically.
        static let canvasMin: CGFloat = 280
        /// #Preview frame size.
        static let previewWidth: CGFloat = 920
        static let previewHeight: CGFloat = 760
        /// Inline search-field bounds.
        static let searchMin: CGFloat = 90
        static let searchIdeal: CGFloat = 200
        static let searchMax: CGFloat = 260
        /// Palette swatch dot in pickers.
        static let swatch: CGFloat = 16
    }

    /// Border line weights.
    enum Stroke {
        /// Hairline rims and separators.
        static let fine: CGFloat = 1
        /// Increased-contrast outlines.
        static let contrast: CGFloat = 2
        /// Keyboard focus ring.
        static let focus: CGFloat = 3
        /// Focus-ring offset outside the keycap rim.
        static let inset: CGFloat = 3
    }

    /// The physical press model is shadowless: a keycap reads as pressed
    /// by sinking `travel` points and shrinking to `Motion.pressedScale`;
    /// hover lifts it `hoverLift`. Recession in trays is simulated with
    /// an inset rim stroke, not an inner shadow.
    enum Depth {
        /// Vertical key travel on press.
        static let travel: CGFloat = 2
        /// Hover lift (negative = up).
        static let hoverLift: CGFloat = -1
        /// Tray recess rim inset.
        static let inset: CGFloat = 2
    }

    /// Alpha ramps, named by role.
    enum Opacity {
        static let full = 1.0
        static let disabled = 0.48
        /// White inner rim on a keycap.
        static let rim = 0.65
        /// Tray rim / recess stroke.
        static let recess = 0.22
        /// Whisper-quiet decorative ink.
        static let quiet = 0.12
    }

    /// Key motion — a springy press with real travel; callers must drop
    /// the transform and animation entirely under Reduce Motion.
    enum Motion {
        static let pressedScale: CGFloat = 0.96
        static let restScale: CGFloat = 1
        static let spring = Animation.spring(duration: 0.25, bounce: 0.45)
    }

    /// Type ramp point sizes.
    enum TypeSize {
        static let title: CGFloat = 24
        static let headline: CGFloat = 14
        static let body: CGFloat = 12
        static let caption: CGFloat = 11
        static let micro: CGFloat = 10
        /// SF Symbol point size inside a full-size keycap.
        static let glyph: CGFloat = 20
    }

    /// Values owned by the Design Lab demo canvas — local interaction
    /// state only; nothing here reaches the player or the Rust core.
    enum Demo {
        static let defaultLevel: Double = 0.65
        static let levelRange: ClosedRange<Double> = 0...1
        /// Fixed 3-column key matrix (a data constraint, not a size).
        static let columns = 3
    }
}

/// Legacy palette names, now computed forwarders into `LyraTheme` —
/// a `var` per name so palette/scheme changes repaint existing views
/// instead of freezing the first selection.
/// `Ui.accent` is the adaptive `tint` role (legend-dark in light, pastel
/// in dark) so it stays readable behind `Ui.onAccent` text; it is NOT
/// the pastel accent — the pastel fails contrast behind light text.
enum Ui {
    static var bg: Color { LyraTheme.shared.color(.chassis) }
    static var surface: Color { LyraTheme.shared.color(.keycap) }
    static var border: Color { LyraTheme.shared.color(.border) }
    static var ink: Color { LyraTheme.shared.color(.legend) }
    static var inkSoft: Color { LyraTheme.shared.color(.legend) }
    static var accent: Color { LyraTheme.shared.color(.tint) }
    /// Text/icons on `Ui.accent` fills.
    static var onAccent: Color { LyraTheme.shared.color(.onTint) }
    static var mint: Color { LyraTheme.shared.color(.secondaryInk) }
    static var indigo: Color { LyraTheme.shared.color(.tint) }

    // 4pt spacing grid — forwards to Bubble.Space.
    static let s4 = Bubble.Space.xs
    static let s8 = Bubble.Space.sm
    static let s12 = Bubble.Space.md
    static let s16 = Bubble.Space.lg
    static let s20 = Bubble.Space.xl
    static let s24 = Bubble.Space.xxl
}

extension Font {
    /// Fixed type ramp — rounded design so the UI face echoes the
    /// keycap legends; `uiMono` is rounded + monospaced digits for
    /// times and counts (not a monospace family).
    static let uiTitle = Font.system(size: Bubble.TypeSize.title, weight: .semibold, design: .rounded)
    static let uiHeadline = Font.system(size: Bubble.TypeSize.headline, weight: .semibold, design: .rounded)
    static let uiBody = Font.system(size: Bubble.TypeSize.body, design: .rounded)
    static let uiBodyStrong = Font.system(size: Bubble.TypeSize.body, weight: .medium, design: .rounded)
    static let uiCaption = Font.system(size: Bubble.TypeSize.caption, design: .rounded)
    static let uiMicro = Font.system(size: Bubble.TypeSize.micro, weight: .medium, design: .rounded)
    static let uiMono = Font.system(size: Bubble.TypeSize.caption, design: .rounded).monospacedDigit()
    /// Keycap glyph size — SF Symbols inside round keys.
    static let bubbleGlyph = Font.system(size: Bubble.TypeSize.glyph, weight: .medium, design: .rounded)
}

/// Legacy card surface — now a recessed Bubblegum tray. The padding
/// parameter is preserved so existing call sites keep their metrics.
struct UiCard: ViewModifier {
    var padding: CGFloat = Bubble.Space.md
    func body(content: Content) -> some View {
        content.modifier(BubbleTrayModifier(padding: padding))
    }
}

extension View {
    func uiCard(padding: CGFloat = Bubble.Space.md) -> some View {
        modifier(UiCard(padding: padding))
    }

    /// Hover affordance for interactive cards (VIZ-CONTRACT): the card
    /// translates up on hover — shadowless, per the Bubblegum update.
    /// Disabled cards and Reduce Motion get no translation. Apply to
    /// clickable cards (device rows, pairing/magnet cards, viz mode
    /// chips), not static text.
    func uiElevated() -> some View { modifier(UiElevated()) }

    /// Round keycap icon well (surface + rim) — legacy chrome for spots
    /// not yet migrated to `BubbleButtonStyle`; pair with `.plain`.
    func sharpIconBox(_ size: CGFloat = Bubble.Size.compactKey) -> some View {
        self
            .frame(width: size, height: size)
            .background(Circle().fill(Color.bubbleKeycap))
            .overlay(Circle().strokeBorder(Bubble.trayRim, lineWidth: Bubble.Stroke.fine))
    }
}

/// ObservableObject-backed hover state — stays stable across view
/// recomposition, which these modifier/style bodies need.
private final class UiHover: ObservableObject {
    @Published var hovering = false
}

/// See `uiElevated()` — hover lift only, shadowless.
private struct UiElevated: ViewModifier {
    @StateObject private var hover = UiHover()
    @ObservedObject private var theme = LyraTheme.shared
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    @Environment(\.isEnabled) private var enabled
    func body(content: Content) -> some View {
        content
            .offset(y: (hover.hovering && enabled && !reduceMotion)
                    ? Bubble.Depth.hoverLift : Bubble.Space.zero)
            .animation(reduceMotion ? nil : Bubble.Motion.spring,
                       value: hover.hovering)
            .onHover { hover.hovering = $0 }
    }
}

/// Legacy button name — forwards to the Bubblegum keycap style so the
/// existing `.sharp` / `.sharpProminent` call sites become pill keys.
/// `prominent` = filled accent keycap for the one primary action.
struct SharpButtonStyle: ButtonStyle {
    var prominent = false

    func makeBody(configuration: ButtonStyleConfiguration) -> some View {
        BubbleButtonStyle(prominent: prominent).makeBody(configuration: configuration)
    }
}

extension ButtonStyle where Self == SharpButtonStyle {
    static var sharp: SharpButtonStyle { SharpButtonStyle() }
    static var sharpProminent: SharpButtonStyle { SharpButtonStyle(prominent: true) }
}
