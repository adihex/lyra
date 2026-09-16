import SwiftUI

/// Sharp editorial UI theme — warm paper base, hairline borders, square
/// corners, a fixed type ramp, and a 4pt spacing grid. The app runs in
/// light appearance; values are fixed RGB so the palette stays
/// consistent regardless of system settings.
enum Ui {
    // palette
    static let bg = Color(red: 0.957, green: 0.941, blue: 0.898)      // warm paper
    static let surface = Color(red: 0.992, green: 0.984, blue: 0.957) // card white
    static let border = Color(red: 0.800, green: 0.760, blue: 0.680)  // hairline
    static let ink = Color(red: 0.278, green: 0.239, blue: 0.204)     // warm ink
    static let inkSoft = Color(red: 0.541, green: 0.482, blue: 0.412) // muted ink
    static let accent = Color(red: 0.780, green: 0.447, blue: 0.278)  // terracotta
    static let mint = Color(red: 0.396, green: 0.729, blue: 0.647)    // icon moon
    static let indigo = Color(red: 0.290, green: 0.337, blue: 0.643)  // icon planet

    // 4pt spacing grid
    static let s4: CGFloat = 4
    static let s8: CGFloat = 8
    static let s12: CGFloat = 12
    static let s16: CGFloat = 16
    static let s20: CGFloat = 20
    static let s24: CGFloat = 24
}

extension Font {
    /// Fixed type ramp — semibold titles, medium emphasis, regular body,
    /// monospaced digits for times and counts.
    static let uiTitle = Font.system(size: 20, weight: .semibold)
    static let uiHeadline = Font.system(size: 13, weight: .semibold)
    static let uiBody = Font.system(size: 12)
    static let uiBodyStrong = Font.system(size: 12, weight: .medium)
    static let uiCaption = Font.system(size: 11)
    static let uiMicro = Font.system(size: 10, weight: .medium)
    static let uiMono = Font.system(size: 11, design: .monospaced)
}

/// Card surface — square corners, hairline border, paper fill.
struct UiCard: ViewModifier {
    var padding: CGFloat = Ui.s12
    func body(content: Content) -> some View {
        content
            .padding(padding)
            .background(Ui.surface)
            .overlay(Rectangle().stroke(Ui.border, lineWidth: 1))
    }
}

extension View {
    func uiCard(padding: CGFloat = Ui.s12) -> some View {
        modifier(UiCard(padding: padding))
    }

    /// Square icon-button chrome (surface + hairline) — pair with a
    /// `.plain` button style.
    func sharpIconBox(_ size: CGFloat = 28) -> some View {
        self
            .frame(width: size, height: size)
            .background(Ui.surface)
            .overlay(Rectangle().stroke(Ui.border, lineWidth: 1))
    }
}

/// Sharp rectangular button — the system .bordered look is rounded and
/// fights the square language. `prominent` = filled accent.
struct SharpButtonStyle: ButtonStyle {
    var prominent = false
    @Environment(\.isEnabled) private var enabled

    func makeBody(configuration: ButtonStyleConfiguration) -> some View {
        configuration.label
            .font(.uiBodyStrong)
            .foregroundStyle(prominent ? Color.white : Ui.ink)
            .padding(.horizontal, Ui.s12)
            .padding(.vertical, 6)
            .background(prominent ? Ui.accent : Ui.surface)
            .overlay(Rectangle().stroke(prominent ? Ui.accent : Ui.border, lineWidth: 1))
            .opacity(enabled ? (configuration.isPressed ? 0.7 : 1) : 0.45)
    }
}

extension ButtonStyle where Self == SharpButtonStyle {
    static var sharp: SharpButtonStyle { SharpButtonStyle() }
    static var sharpProminent: SharpButtonStyle { SharpButtonStyle(prominent: true) }
}
