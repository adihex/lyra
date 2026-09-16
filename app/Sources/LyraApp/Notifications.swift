import AppKit
import UserNotifications

/// Track-change banners — the anti-spam policy lives here
/// (docs/research/macos-integration.md §2):
///
///   mode off / album (default) / all
///   × suppress while Lyra is frontmost (they're looking at the player)
///   × suppress when the change was user-initiated (they clicked — they know)
///   × dedupe same-track bursts (engine restarts, double fires)
///   × coarse cadence cap — no banner storm while skipping
///
/// Banners are `.passive`: silent, Focus/DND-safe, coalesced per album
/// in Notification Center via `threadIdentifier`.
final class TrackNotifier {
    static let shared = TrackNotifier()

    static let nextActionId = "LYRA_NEXT"
    static let categoryId = "TRACK"

    private var prevAlbum: String?
    private var lastNotifiedId: String?
    private var lastPostAt = Date.distantPast
    private let cadence: TimeInterval = 4  // coarse cap between banners
    private let dedupe: TimeInterval = 8   // same-track restart window

    /// Declared once at launch. "Next" is the only action until a ratings
    /// column exists — Love stays gated on that (doc §2).
    func registerCategory() {
        let next = UNNotificationAction(identifier: Self.nextActionId,
                                        title: "Next")
        let cat = UNNotificationCategory(identifier: Self.categoryId,
                                         actions: [next],
                                         intentIdentifiers: [])
        UNUserNotificationCenter.current().setNotificationCategories([cat])
    }

    /// Call on every successful track start. `userInitiated` = the user
    /// explicitly picked this track (in-app click, Spotlight restore) —
    /// those never banner.
    func trackStarted(_ t: Track, userInitiated: Bool) {
        let album = t.album
        defer { prevAlbum = album }
        let mode = Prefs.shared.notifyMode
        guard mode != "off", !userInitiated, !NSApp.isActive else { return }
        if mode == "album", album == prevAlbum { return } // same album → quiet
        let now = Date()
        if t.id == lastNotifiedId, now.timeIntervalSince(lastPostAt) < dedupe { return }
        if now.timeIntervalSince(lastPostAt) < cadence { return }
        post(t, at: now)
    }

    private func post(_ t: Track, at now: Date) {
        ensureAuth { [weak self] granted in
            guard let self, granted else { return }
            let content = UNMutableNotificationContent()
            content.title = t.title
            content.body = [t.artist, t.album]
                .filter { !$0.isEmpty }.joined(separator: " — ")
            content.threadIdentifier = t.album.isEmpty ? "lyra.tracks" : t.album
            content.categoryIdentifier = Self.categoryId
            content.interruptionLevel = .passive // never breaks Focus, no sound
            if let hash = t.artworkHash {
                let u = Artwork.url(hash, size: 256)
                if FileManager.default.fileExists(atPath: u.path),
                   let att = try? UNNotificationAttachment(identifier: "art", url: u) {
                    content.attachments = [att]
                }
            }
            // One outstanding banner per album — a new track replaces the
            // previous; threadIdentifier groups them in the Center.
            let req = UNNotificationRequest(
                identifier: "lyra.track.\(Self.stableId(t.album))",
                content: content, trigger: nil)
            UNUserNotificationCenter.current().add(req)
            self.lastNotifiedId = t.id
            self.lastPostAt = now
        }
    }

    /// Runtime permission, asked lazily on the first eligible banner —
    /// never at launch. Denial is silent: we simply don't post.
    private func ensureAuth(_ done: @escaping (Bool) -> Void) {
        let c = UNUserNotificationCenter.current()
        c.getNotificationSettings { s in
            let proceed = { (g: Bool) in DispatchQueue.main.async { done(g) } }
            switch s.authorizationStatus {
            case .authorized, .provisional:
                proceed(true)
            case .notDetermined:
                c.requestAuthorization(options: [.alert, .sound]) { g, _ in
                    proceed(g)
                }
            default:
                proceed(false)
            }
        }
    }

    /// Per-album request identifier — hashValue isn't stable across
    /// launches, so banners from a previous session would never be
    /// replaced. djb2 keeps the id stable.
    private static func stableId(_ s: String) -> String {
        var h: UInt64 = 5381
        for b in s.utf8 { h = h &* 33 &+ UInt64(b) }
        return String(h, radix: 36)
    }
}
