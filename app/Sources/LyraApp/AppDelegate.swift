import AppKit
import CoreSpotlight
import OSLog
import UserNotifications

/// Delegate-mounted surfaces: dock menu, notification actions +
/// frontmost suppression, Spotlight restore and index rebuilds
/// (docs §2–4). One `@NSApplicationDelegateAdaptor` on LyraApp gives
/// all of these a home.
final class AppDelegate: NSObject, NSApplicationDelegate,
                         UNUserNotificationCenterDelegate,
                         CSSearchableIndexDelegate {

    func applicationDidFinishLaunching(_ notification: Notification) {
        UNUserNotificationCenter.current().delegate = self
        CSSearchableIndex.default().indexDelegate = self
        TrackNotifier.shared.registerCategory()
        // Headless playback hook for VM/CLI QA: LYRA_AUTOPLAY=/path/file.wav
        // plays once the engine is up — no UI automation needed.
        if let auto = ProcessInfo.processInfo.environment["LYRA_AUTOPLAY"] {
            DispatchQueue.main.asyncAfter(deadline: .now() + 1.0) {
                let ok = LyraPlayer.shared.play(path: auto)
                os_log("LYRA_AUTOPLAY %{public}@ → %{public}@", auto, ok ? "ok" : "FAILED")
            }
        }
        // LYRA_DEBUG_PANE=visuals|library|discover|eq|remote — lane select for
        // headless QA runs; combined with LYRA_AUTOPLAY it gives a
        // deterministic "playing on the visuals pane" state.
        if let pane = ProcessInfo.processInfo.environment["LYRA_DEBUG_PANE"] {
            DispatchQueue.main.asyncAfter(deadline: .now() + 1.2) {
                ViewModel.shared.selection = SidebarItem.allCases
                    .first { $0.rawValue.lowercased() == pane.lowercased() }
                    ?? ViewModel.shared.selection
            }
        }
    }

    // ── Dock right-click (iTunes pattern — rebuilt every click so the
    //    header + Play/Pause titles are always current) ───────────────
    func applicationDockMenu(_ sender: NSApplication) -> NSMenu? {
        let vm = ViewModel.shared
        let menu = NSMenu()
        func item(_ title: String, _ action: Selector? = nil,
                  enabled: Bool = true) -> NSMenuItem {
            let i = NSMenuItem(title: title, action: action, keyEquivalent: "")
            i.target = self
            i.isEnabled = enabled
            return i
        }
        menu.addItem(item(vm.current.map { "\($0.title) — \($0.artist)" }
                          ?? "Not playing", enabled: false))
        menu.addItem(.separator())
        menu.addItem(item(vm.playing ? "Pause" : "Play", #selector(dockToggle)))
        menu.addItem(item("Next", #selector(dockNext)))
        menu.addItem(item("Previous", #selector(dockPrev)))
        menu.addItem(.separator())
        menu.addItem(item("Show Lyra", #selector(dockShow)))
        return menu
    }

    @objc private func dockToggle() { ViewModel.shared.toggle() }
    @objc private func dockNext() { ViewModel.shared.next() }
    @objc private func dockPrev() { ViewModel.shared.prev() }
    @objc private func dockShow() { WindowOps.showMain() }

    // ── Spotlight restore → select + play the indexed track ──────────
    func application(_ application: NSApplication,
                     continue userActivity: NSUserActivity,
                     restorationHandler: @escaping ([NSUserActivityRestoring]) -> Void) -> Bool {
        guard userActivity.activityType == CSSearchableItemActionType,
              let id = userActivity.userInfo?[CSSearchableItemActivityIdentifier] as? String
        else { return false }
        WindowOps.showMain()
        SpotlightIndex.shared.openTrack(id)
        return true
    }

    // ── Notifications ────────────────────────────────────────────────
    func userNotificationCenter(_ center: UNUserNotificationCenter,
        willPresent notification: UNNotification,
        withCompletionHandler completionHandler: @escaping (UNNotificationPresentationOptions) -> Void) {
        // Only fires while we're frontmost — the player is on screen, so
        // a banner is pure noise (doc §2). Keep delivery suppressed.
        completionHandler([])
    }

    func userNotificationCenter(_ center: UNUserNotificationCenter,
        didReceive response: UNNotificationResponse,
        withCompletionHandler completionHandler: @escaping () -> Void) {
        switch response.actionIdentifier {
        case TrackNotifier.nextActionId:
            ViewModel.shared.next()
        default: // body click → open the player
            WindowOps.showMain()
        }
        completionHandler()
    }

    // ── Spotlight can ask the index owner to rebuild ─────────────────
    func searchableIndex(_ searchableIndex: CSSearchableIndex,
        reindexAllSearchableItemsWithAcknowledgementHandler acknowledgementHandler: @escaping () -> Void) {
        SpotlightIndex.shared.reindexAll()
        acknowledgementHandler()
    }

    func searchableIndex(_ searchableIndex: CSSearchableIndex,
        reindexSearchableItemsWithIdentifiers identifiers: [String],
        acknowledgementHandler: @escaping () -> Void) {
        SpotlightIndex.shared.reindex(identifiers)
        acknowledgementHandler()
    }
}

/// Reopening the WindowGroup window needs SwiftUI's openWindow action,
/// which only exists in the view environment — a capture view in
/// LyraApp writes `openMain` at launch so AppKit code can reach it.
enum WindowOps {
    static var openMain: (() -> Void)?

    static func showMain() {
        NSApp.unhide(nil)
        openMain?() // reopens the main window if it was closed
        NSApp.activate(ignoringOtherApps: true)
        NSApp.windows.first { $0.styleMask.contains(.titled) }?
            .makeKeyAndOrderFront(nil)
    }
}
