import SwiftUI

/// Cozy visual theme — warm cream surfaces, terracotta accent, rounded
/// type. The app runs in light appearance; values are fixed RGB so the
/// palette stays consistent regardless of system settings.
enum Cozy {
    static let bg = Color(red: 0.961, green: 0.941, blue: 0.894)      // warm cream
    static let surface = Color(red: 0.996, green: 0.984, blue: 0.957) // card white
    static let border = Color(red: 0.878, green: 0.835, blue: 0.749)  // soft warm border
    static let ink = Color(red: 0.278, green: 0.239, blue: 0.204)     // warm brown text
    static let inkSoft = Color(red: 0.541, green: 0.482, blue: 0.412) // muted brown
    static let accent = Color(red: 0.780, green: 0.447, blue: 0.278)  // terracotta
    static let mint = Color(red: 0.396, green: 0.729, blue: 0.647)    // icon moon mint
    static let indigo = Color(red: 0.290, green: 0.337, blue: 0.643)  // icon planet indigo
}

extension Font {
    /// SF Rounded — the "cozy" voice for headings and emphasis.
    static func cozy(_ style: Font.TextStyle, weight: Font.Weight = .semibold) -> Font {
        .system(style, design: .rounded, weight: weight)
    }
}

/// Card surface — cream white, soft warm border, 14pt radius.
struct CozyCard: ViewModifier {
    var padding: CGFloat = 12
    func body(content: Content) -> some View {
        content
            .padding(padding)
            .background(Cozy.surface, in: RoundedRectangle(cornerRadius: 14))
            .overlay(
                RoundedRectangle(cornerRadius: 14)
                    .stroke(Cozy.border, lineWidth: 1)
            )
    }
}

extension View {
    func cozyCard(padding: CGFloat = 12) -> some View {
        modifier(CozyCard(padding: padding))
    }
}
