import Foundation

// Bubblegum smoke check — exercises the Design Lab's local canvas model
// only (no ViewModel, no player, no FFI). Build:
//   xcrun swiftc -parse-as-library \
//       app/Sources/LyraApp/LyraTheme.swift \
//       app/Sources/LyraApp/Theme.swift \
//       app/Sources/LyraApp/BubbleControls.swift \
//       app/Sources/LyraApp/BubbleCanvasState.swift \
//       scripts/check_bubblegum.swift \
//       -o .build/check-bubblegum && .build/check-bubblegum

@main
struct BubblegumCheck {
    static func main() {
        var passed: [String] = []
        func check(_ name: String, _ ok: Bool) {
            assert(ok, "FAILED: \(name)")
            passed.append(name)
        }

        let m = BubbleCanvasState()

        // Factory layout
        check("initial pads == 9", m.pads.count == 9)
        check("unique pad IDs", Set(m.pads.map(\.id)).count == 9)

        // Selection
        m.activate(4)
        check("activate(4) selects pad 4", m.selectedID == 4)

        // Reorder: Bass (id 4, index 4) moves to index 3, pad set intact
        m.move(-1)
        check("move(-1) moves pad to index 3",
              m.selectedIndex == 3 && m.pads[3].id == 4)
        check("pad set preserved after move",
              Set(m.pads.map(\.id)) == Set(BubblePad.defaults.map(\.id)))

        // Edges: first can't move left, last can't move right
        m.activate(0)
        check("first pad cannot move left", !m.canMove(-1))
        m.move(-1)
        check("move left at edge is a no-op", m.pads[0].id == 0)
        m.activate(8)
        check("last pad cannot move right", !m.canMove(1))
        m.move(1)
        check("move right at edge is a no-op", m.pads.last?.id == 8)

        // Invalid activation is a no-op
        m.activate(99)
        check("activate(99) ignored", m.selectedID == 8)

        // Disabled gates every mutation path
        m.controlsEnabled = false
        m.activate(2)
        check("disabled blocks activate", m.selectedID == 8)
        m.move(-1)
        check("disabled blocks move",
              m.selectedIndex == 8 && m.pads.last?.id == 8)
        m.toggleRunning()
        check("disabled blocks toggleRunning", m.running == false)

        // Re-enable recovers
        m.controlsEnabled = true
        m.toggleRunning()
        check("reenabled toggles running", m.running == true)

        // Reset restores every editable value
        m.name = "Edited"
        m.level = 0.2
        m.density = .spacious
        m.showLegends = false
        m.controlsEnabled = false // exercises false→true restore below
        m.reset()
        check("reset restores pads", m.pads == BubblePad.defaults)
        check("reset clears selection", m.selectedID == nil)
        check("reset restores density", m.density == .standard)
        check("reset restores legends", m.showLegends == true)
        check("reset restores enabled", m.controlsEnabled == true)
        check("reset stops running", m.running == false)
        check("reset restores level", m.level == Bubble.Demo.defaultLevel)
        check("reset restores name", m.name == "Studio matrix")

        // A theme flip mutates the shared store, not the canvas — the
        // same owned model object survives with all state intact.
        let suite = "app.lyra.bubblegum-check.\(UUID().uuidString)"
        let themeDefaults = UserDefaults(suiteName: suite)!
        let theme = LyraTheme(defaults: themeDefaults)
        m.activate(4)
        m.name = "Survivor"
        theme.palette = .rose
        theme.appearance = .dark
        check("canvas survives theme change",
              m.selectedID == 4 && m.name == "Survivor"
              && m.pads == BubblePad.defaults)
        check("theme store applied", theme.palette == .rose
              && theme.appearance == .dark)
        themeDefaults.removePersistentDomain(forName: suite)

        print("check_bubblegum: \(passed.count) assertions passed")
        for name in passed { print("  PASS \(name)") }
    }
}
