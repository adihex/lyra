import AppKit
import CoreGraphics

// Theme/palette check — exercises LyraTheme's persisted store, the
// immutable palette table, the dynamic NSColor providers, and the
// PetPalette CG cast. Disposable UserDefaults suite only; never calls
// `.start()` (that would mutate NSApp). Build:
//   xcrun swiftc -parse-as-library \
//       app/Sources/LyraApp/LyraTheme.swift \
//       app/Sources/LyraApp/Theme.swift \
//       app/Sources/LyraApp/BubbleControls.swift \
//       app/Sources/LyraApp/BubbleCanvasState.swift \
//       scripts/check_theme.swift \
//       -o .build/check-theme && .build/check-theme

@main
struct ThemeCheck {
    static func main() {
        var passed: [String] = []
        func check(_ name: String, _ ok: Bool) {
            assert(ok, "FAILED: \(name)")
            passed.append(name)
        }

        let suite = "app.lyra.theme-check.\(UUID().uuidString)"
        let defaults = UserDefaults(suiteName: suite)!

        // Fresh defaults → system / lavender.
        let fresh = LyraTheme(defaults: defaults)
        check("fresh defaults appearance", fresh.appearance == .system)
        check("fresh defaults palette", fresh.palette == .lavender)

        // Invalid persisted strings fall back.
        defaults.set("neon", forKey: "lyraPalette")
        defaults.set("inverted", forKey: "lyraAppearance")
        let invalid = LyraTheme(defaults: defaults)
        check("invalid palette falls back", invalid.palette == .lavender)
        check("invalid appearance falls back", invalid.appearance == .system)

        // dark/rose persists and reloads.
        invalid.palette = .rose
        invalid.appearance = .dark
        let reloaded = LyraTheme(defaults: defaults)
        check("dark persisted", reloaded.appearance == .dark)
        check("rose persisted", reloaded.palette == .rose)

        // light/mint persists.
        reloaded.palette = .mint
        reloaded.appearance = .light
        check("mint persisted", LyraTheme(defaults: defaults).palette == .mint)
        check("light persisted",
              LyraTheme(defaults: defaults).appearance == .light)

        // .system persists too (round-trip through a prior explicit value).
        reloaded.appearance = .system
        check("system persisted",
              LyraTheme(defaults: defaults).appearance == .system)

        // System maps to nil scheme/appearance — never an override.
        check("system colorScheme nil", LyraAppearance.system.colorScheme == nil)
        check("system nsAppearance nil", LyraAppearance.system.nsAppearance == nil)
        check("light colorScheme", LyraAppearance.light.colorScheme == .light)
        check("dark colorScheme", LyraAppearance.dark.colorScheme == .dark)
        check("light nsAppearance", LyraAppearance.light.nsAppearance != nil)
        check("dark nsAppearance", LyraAppearance.dark.nsAppearance != nil)

        // Exact palette table — every role, all six combinations.
        let table: [LyraPalette: (light: LyraColors, dark: LyraColors)] = [
            .lavender: (
                LyraColors(chassis: 0xF3F0F7, tray: 0xE0D9E9, keycap: 0xFDFCFF,
                           accent: 0xC5B3D8, legend: 0x4A3C60, onAccent: 0x30263F,
                           tint: 0x665379, onTint: 0xFFFFFF, secondaryInk: 0x495F62,
                           companion: 0xC0D7D0, border: 0xB8AAC7),
                LyraColors(chassis: 0x211F29, tray: 0x19171F, keycap: 0x34303E,
                           accent: 0xBBA8CF, legend: 0xECE5F2, onAccent: 0x2B2336,
                           tint: 0xC5B3D8, onTint: 0x30263F, secondaryInk: 0xA9C9C3,
                           companion: 0xAFC9C3, border: 0x60566E)),
            .rose: (
                LyraColors(chassis: 0xF8F1F2, tray: 0xEADCE1, keycap: 0xFFFCFD,
                           accent: 0xD8B7C1, legend: 0x60434E, onAccent: 0x3D2932,
                           tint: 0x805468, onTint: 0xFFFFFF, secondaryInk: 0x56614A,
                           companion: 0xCDD8BD, border: 0xC6AAB5),
                LyraColors(chassis: 0x281F23, tray: 0x20181C, keycap: 0x3C2F35,
                           accent: 0xD3AFBD, legend: 0xF5E5EC, onAccent: 0x39232D,
                           tint: 0xDDBFCC, onTint: 0x39232D, secondaryInk: 0xC2CDAF,
                           companion: 0xC6D1B7, border: 0x735766)),
            .mint: (
                LyraColors(chassis: 0xEFF5F2, tray: 0xD7E4DD, keycap: 0xFCFFFD,
                           accent: 0xAECABD, legend: 0x354F43, onAccent: 0x24382E,
                           tint: 0x496B5A, onTint: 0xFFFFFF, secondaryInk: 0x5D5A77,
                           companion: 0xCECBE0, border: 0xA6BDB0),
                LyraColors(chassis: 0x1C2522, tray: 0x141C19, keycap: 0x2C3C34,
                           accent: 0xA3C5B4, legend: 0xE1F0E8, onAccent: 0x1F362A,
                           tint: 0xB5D5C4, onTint: 0x24382E, secondaryInk: 0xC8BFDC,
                           companion: 0xC6BED7, border: 0x4D6B5D)),
        ]
        for p in LyraPalette.allCases {
            let row = table[p]!
            check("\(p.rawValue) light table",
                  colorsEqual(p.colors(dark: false), row.light))
            check("\(p.rawValue) dark table",
                  colorsEqual(p.colors(dark: true), row.dark))
        }

        // The dynamic NSColor provider resolves each appearance exactly.
        func hex(_ ns: NSColor, under ap: NSAppearance) -> UInt32 {
            var out: UInt32 = 0
            ap.performAsCurrentDrawingAppearance {
                let c = ns.usingColorSpace(.sRGB)!
                out = (UInt32((c.redComponent * 255).rounded()) << 16)
                    | (UInt32((c.greenComponent * 255).rounded()) << 8)
                    | UInt32((c.blueComponent * 255).rounded())
            }
            return out
        }
        for p in LyraPalette.allCases {
            let row = table[p]!
            for role in LyraColorRole.allCases {
                let ns = LyraTheme.nsColor(for: p, role: role)
                check("\(p.rawValue)/\(role) light resolves",
                      hex(ns, under: NSAppearance(named: .aqua)!) == row.light[role])
                check("\(p.rawValue)/\(role) dark resolves",
                      hex(ns, under: NSAppearance(named: .darkAqua)!) == row.dark[role])
            }
        }

        // PetPalette role mapping — all six combinations.
        for p in LyraPalette.allCases {
            for dark in [false, true] {
                let c = p.colors(dark: dark)
                let pet = PetPalette(colors: c, dark: dark)
                check("\(p.rawValue)/\(dark) pet bodyLight=accent",
                      cgEqual(pet.bodyLight, c.accent))
                check("\(p.rawValue)/\(dark) pet moonLight=companion",
                      cgEqual(pet.moonLight, c.companion))
                check("\(p.rawValue)/\(dark) pet bodyDark",
                      cgEqual(pet.bodyDark, dark ? c.keycap : c.legend))
                check("\(p.rawValue)/\(dark) pet sky=tray/chassis",
                      cgEqual(pet.skyTop, c.tray) && cgEqual(pet.skyBottom, c.chassis))
            }
        }

        // resolve() returns the cached cast — same CGColor objects.
        let resA = PetPalette.resolve(.lavender,
                                      appearance: NSAppearance(named: .darkAqua)!)
        let resB = PetPalette.resolve(.lavender,
                                      appearance: NSAppearance(named: .darkAqua)!)
        check("pet cached colors reused", resA.bodyLight === resB.bodyLight)

        defaults.removePersistentDomain(forName: suite)

        print("check_theme: \(passed.count) assertions passed")
        for name in passed { print("  PASS \(name)") }
    }

    static func colorsEqual(_ a: LyraColors, _ b: LyraColors) -> Bool {
        LyraColorRole.allCases.allSatisfy { a[$0] == b[$0] }
    }

    /// Compare exact sRGB components.
    static func cgEqual(_ c: CGColor, _ hex: UInt32) -> Bool {
        guard let k = c.components, k.count >= 3 else { return false }
        return k[0] == CGFloat((hex >> 16) & 0xff) / 255
            && k[1] == CGFloat((hex >> 8) & 0xff) / 255
            && k[2] == CGFloat(hex & 0xff) / 255
    }
}
