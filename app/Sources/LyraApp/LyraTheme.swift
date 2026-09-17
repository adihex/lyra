import AppKit
import SwiftUI

/// Lyra's appearance system — a persisted store plus three immutable
/// named palettes (Lavender/Rose/Mint × light/dark). Colors are native
/// SwiftUI `Color`s backed by `NSColor` dynamic providers resolved per
/// (palette, role), so light/dark flips flow through each view's
/// effective appearance — including #Preview — without remounting.
/// `Ui.*` / `Color.bubble*` in Theme.swift are computed forwarders into
/// the current palette, so existing views re-render on palette change
/// as long as the view observes `LyraTheme.shared`.

/// System / explicit scheme selection. `.system` means
/// `NSApp.appearance = nil` and no preferredColorScheme — the OS
/// decides; nothing reads AppleInterfaceStyle as authoritative.
enum LyraAppearance: String, CaseIterable, Identifiable {
    case system, light, dark
    var id: String { rawValue }
    var title: String {
        switch self { case .system: "System"; case .light: "Light"; case .dark: "Dark" }
    }
    var colorScheme: ColorScheme? {
        switch self { case .system: nil; case .light: .light; case .dark: .dark }
    }
    var nsAppearance: NSAppearance? {
        switch self {
        case .system: nil
        case .light: NSAppearance(named: .aqua)
        case .dark: NSAppearance(named: .darkAqua)
        }
    }
}

/// The three named palettes. `colors(dark:)` returns the immutable hex
/// table row — the source of truth for both the SwiftUI dynamic colors
/// and the pet/dock CG painters.
enum LyraPalette: String, CaseIterable, Identifiable {
    case lavender, rose, mint
    var id: String { rawValue }
    var title: String { rawValue.capitalized }

    func colors(dark: Bool) -> LyraColors {
        switch (self, dark) {
        case (.lavender, false):
            return LyraColors(chassis: 0xF3F0F7, tray: 0xE0D9E9, keycap: 0xFDFCFF,
                              accent: 0xC5B3D8, legend: 0x4A3C60, onAccent: 0x30263F,
                              tint: 0x665379, onTint: 0xFFFFFF, secondaryInk: 0x495F62,
                              companion: 0xC0D7D0, border: 0xB8AAC7)
        case (.lavender, true):
            return LyraColors(chassis: 0x211F29, tray: 0x19171F, keycap: 0x34303E,
                              accent: 0xBBA8CF, legend: 0xECE5F2, onAccent: 0x2B2336,
                              tint: 0xC5B3D8, onTint: 0x30263F, secondaryInk: 0xA9C9C3,
                              companion: 0xAFC9C3, border: 0x60566E)
        case (.rose, false):
            return LyraColors(chassis: 0xF8F1F2, tray: 0xEADCE1, keycap: 0xFFFCFD,
                              accent: 0xD8B7C1, legend: 0x60434E, onAccent: 0x3D2932,
                              tint: 0x805468, onTint: 0xFFFFFF, secondaryInk: 0x56614A,
                              companion: 0xCDD8BD, border: 0xC6AAB5)
        case (.rose, true):
            return LyraColors(chassis: 0x281F23, tray: 0x20181C, keycap: 0x3C2F35,
                              accent: 0xD3AFBD, legend: 0xF5E5EC, onAccent: 0x39232D,
                              tint: 0xDDBFCC, onTint: 0x39232D, secondaryInk: 0xC2CDAF,
                              companion: 0xC6D1B7, border: 0x735766)
        case (.mint, false):
            return LyraColors(chassis: 0xEFF5F2, tray: 0xD7E4DD, keycap: 0xFCFFFD,
                              accent: 0xAECABD, legend: 0x354F43, onAccent: 0x24382E,
                              tint: 0x496B5A, onTint: 0xFFFFFF, secondaryInk: 0x5D5A77,
                              companion: 0xCECBE0, border: 0xA6BDB0)
        case (.mint, true):
            return LyraColors(chassis: 0x1C2522, tray: 0x141C19, keycap: 0x2C3C34,
                              accent: 0xA3C5B4, legend: 0xE1F0E8, onAccent: 0x1F362A,
                              tint: 0xB5D5C4, onTint: 0x24382E, secondaryInk: 0xC8BFDC,
                              companion: 0xC6BED7, border: 0x4D6B5D)
        }
    }
}

/// Semantic roles every palette defines — role names are stable, the
/// hex values per palette/scheme are data.
enum LyraColorRole: CaseIterable {
    case chassis, tray, keycap, accent, legend, onAccent
    case tint, onTint, secondaryInk, companion, border
}

/// One immutable palette row — raw sRGB hex per role.
struct LyraColors {
    let chassis, tray, keycap, accent, legend, onAccent: UInt32
    let tint, onTint, secondaryInk, companion, border: UInt32

    subscript(_ role: LyraColorRole) -> UInt32 {
        switch role {
        case .chassis: chassis
        case .tray: tray
        case .keycap: keycap
        case .accent: accent
        case .legend: legend
        case .onAccent: onAccent
        case .tint: tint
        case .onTint: onTint
        case .secondaryInk: secondaryInk
        case .companion: companion
        case .border: border
        }
    }
}

/// The theme store. Persisted selection (UserDefaults), published for
/// SwiftUI, and — after `start()` — applied to `NSApp.appearance` so
/// every AppKit surface (panels, pet, dock tile) follows too. Main
/// thread only; `start()` is deferred to applicationDidFinishLaunching
/// because the singleton may initialize before NSApplication exists.
final class LyraTheme: ObservableObject {
    static let shared = LyraTheme()

    private let defaults: UserDefaults
    private var started = false

    @Published var appearance: LyraAppearance {
        didSet {
            guard appearance != oldValue else { return }
            defaults.set(appearance.rawValue, forKey: "lyraAppearance")
            if started { applyAppearance() }
        }
    }
    @Published var palette: LyraPalette {
        didSet {
            if palette != oldValue {
                defaults.set(palette.rawValue, forKey: "lyraPalette")
            }
        }
    }

    init(defaults: UserDefaults = .standard) {
        self.defaults = defaults
        appearance = LyraAppearance(
            rawValue: defaults.string(forKey: "lyraAppearance") ?? "") ?? .system
        palette = LyraPalette(
            rawValue: defaults.string(forKey: "lyraPalette") ?? "") ?? .lavender
    }

    /// Idempotent — wires the persisted appearance onto NSApp once the
    /// application object exists.
    func start() {
        guard !started else { return }
        started = true
        applyAppearance()
    }

    private func applyAppearance() {
        NSApp.appearance = appearance.nsAppearance
    }

    static func isDark(_ appearance: NSAppearance) -> Bool {
        appearance.bestMatch(from: [.darkAqua, .aqua]) == .darkAqua
    }

    /// Current palette's color for a role — wraps the cached native
    /// dynamic-color provider (one NSColor per palette × role).
    func color(_ role: LyraColorRole) -> Color {
        Color(nsColor: Self.nsColor(for: palette, role: role))
    }

    /// The cached dynamic NSColor itself — tests resolve this against
    /// explicit appearances to exercise the real provider.
    static func nsColor(for palette: LyraPalette,
                        role: LyraColorRole) -> NSColor {
        nsColorCache[palette]![role]!
    }

    /// Pet/viz planet base — resolves to keycap in dark, legend in
    /// light (the role table carries no dedicated bodyDark entry).
    /// A dynamic provider so Canvas/NSView callers never read a global
    /// appearance mid-frame.
    func bodyDark() -> Color {
        Color(nsColor: Self.bodyDarkCache[palette]!)
    }

    private static let bodyDarkCache: [LyraPalette: NSColor] =
        Dictionary(uniqueKeysWithValues: LyraPalette.allCases.map { palette in
            (palette, NSColor(name: nil) { appearance in
                let c = palette.colors(dark: LyraTheme.isDark(appearance))
                let hex = LyraTheme.isDark(appearance) ? c.keycap : c.legend
                return NSColor(
                    srgbRed: CGFloat((hex >> 16) & 0xff) / 255,
                    green: CGFloat((hex >> 8) & 0xff) / 255,
                    blue: CGFloat(hex & 0xff) / 255, alpha: 1)
            })
        })

    private static let nsColorCache: [LyraPalette: [LyraColorRole: NSColor]] =
        Dictionary(uniqueKeysWithValues: LyraPalette.allCases.map { palette in
            (palette, Dictionary(uniqueKeysWithValues:
                LyraColorRole.allCases.map { role in
                    let ns = NSColor(name: nil) { appearance in
                        let hex = palette.colors(
                            dark: LyraTheme.isDark(appearance))[role]
                        return NSColor(
                            srgbRed: CGFloat((hex >> 16) & 0xff) / 255,
                            green: CGFloat((hex >> 8) & 0xff) / 255,
                            blue: CGFloat(hex & 0xff) / 255, alpha: 1)
                    }
                    return (role, ns)
                }))
        })
}

/// Immutable CGColor cast for the AppKit/CoreGraphics painters
/// (desktop pet, dock-tile cosmos) — one cached instance per
/// (palette, scheme), handed to draw() so the painters stay pure
/// functions and never read the mutable store mid-frame.
struct PetPalette {
    let bodyLight: CGColor    // planet gradient top — accent
    let bodyMid: CGColor      // planet gradient mid — tint
    let bodyDark: CGColor     // planet gradient base — legend (light) / keycap (dark)
    let moonLight: CGColor    // moon gradient top — companion
    let moonDark: CGColor     // moon gradient base — secondaryInk
    let ring: CGColor         // orbit/ring/latitude strokes — companion
    let detail: CGColor       // craters/moon spot — legend
    let spark: CGColor        // twinkles/star accents — accent
    let glint: CGColor        // glint dot/highlight — keycap
    let note: CGColor         // comet dash + logo note — accent
    let glow: CGColor         // radial glow behind the planet — companion
    let skyTop: CGColor       // sky gradient top — tray
    let skyBottom: CGColor    // sky gradient base — chassis

    init(colors c: LyraColors, dark: Bool) {
        func cg(_ hex: UInt32) -> CGColor {
            CGColor(srgbRed: CGFloat((hex >> 16) & 0xff) / 255,
                    green: CGFloat((hex >> 8) & 0xff) / 255,
                    blue: CGFloat(hex & 0xff) / 255, alpha: 1)
        }
        bodyLight = cg(c.accent)
        bodyMid = cg(c.tint)
        bodyDark = cg(dark ? c.keycap : c.legend)
        moonLight = cg(c.companion)
        moonDark = cg(c.secondaryInk)
        ring = cg(c.companion)
        detail = cg(c.legend)
        spark = cg(c.accent)
        glint = cg(c.keycap)
        note = cg(c.accent)
        glow = cg(c.companion)
        skyTop = cg(c.tray)
        skyBottom = cg(c.chassis)
    }

    /// The six immutable casts, built once.
    private static let cache: [LyraPalette: [Bool: PetPalette]] =
        Dictionary(uniqueKeysWithValues: LyraPalette.allCases.map { choice in
            (choice, Dictionary(uniqueKeysWithValues: [false, true].map { dark in
                (dark, PetPalette(colors: choice.colors(dark: dark), dark: dark))
            }))
        })

    /// Resolve the store's palette against an explicit appearance —
    /// the view supplies its own `effectiveAppearance`, no global reads.
    static func resolve(_ choice: LyraPalette,
                        appearance: NSAppearance) -> PetPalette {
        cache[choice]![LyraTheme.isDark(appearance)]!
    }
}
