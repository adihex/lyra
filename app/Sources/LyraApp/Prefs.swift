import Foundation
import ServiceManagement

/// App-wide preferences — ObservableObject so SwiftUI binds live
/// (CLT-safe: @Published, no macros). Everything persists to UserDefaults.
final class Prefs: ObservableObject {
    static let shared = Prefs()

    /// Track-change banners: "off" | "album" (default) | "all" —
    /// policy in docs/research/macos-integration.md §2.
    @Published var notifyMode: String {
        didSet { UserDefaults.standard.set(notifyMode, forKey: "notifyMode") }
    }
    /// Menu-bar label: "spectrum" (default) | "pulse" | "note".
    @Published var menuBarMode: String {
        didSet { UserDefaults.standard.set(menuBarMode, forKey: "menuBarMode") }
    }
    /// Our own mount toggle for the MenuBarExtra scene — distinct from
    /// Tahoe's per-app kill switch (System Settings → Menu Bar).
    @Published var menuBarExtra: Bool {
        didSet { UserDefaults.standard.set(menuBarExtra, forKey: "menuBarExtra") }
    }
    /// Dock-icon driver mode — "off" | "hidden" | "playing" (default
    /// playing). Read live by DockCosmos' DockVizDriver; UI + key only.
    @Published var dockIconMode: String {
        didSet { UserDefaults.standard.set(dockIconMode, forKey: "dockIconMode") }
    }
    /// Mirrors SMAppService state — write through `setLaunchAtLogin`
    /// so the UI reflects denial/`requiresApproval`, not just intent.
    @Published private(set) var launchAtLogin: Bool

    private init() {
        let d = UserDefaults.standard
        notifyMode = d.string(forKey: "notifyMode") ?? "album"
        menuBarMode = d.string(forKey: "menuBarMode") ?? "spectrum"
        dockIconMode = d.string(forKey: "dockIconMode") ?? "playing"
        menuBarExtra = d.object(forKey: "menuBarExtra") as? Bool ?? true
        launchAtLogin = LoginItem.status == .enabled
    }

    func setLaunchAtLogin(_ on: Bool) {
        LoginItem.set(on)
        launchAtLogin = LoginItem.status == .enabled
    }

    var launchNeedsApproval: Bool { LoginItem.status == .requiresApproval }
}

/// Launch-at-login via SMAppService (macOS 13+, sandbox-clean, no
/// entitlement). `.requiresApproval` deep-links System Settings.
enum LoginItem {
    static var status: SMAppService.Status { SMAppService.mainApp.status }

    static func set(_ on: Bool) {
        do {
            if on { try SMAppService.mainApp.register() }
            else { try SMAppService.mainApp.unregister() }
        } catch {
            NSLog("lyra: login item \(on ? "register" : "unregister") failed: \(error.localizedDescription)")
        }
    }

    static func openSystemSettings() { SMAppService.openSystemSettingsLoginItems() }
}
