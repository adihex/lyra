import AppKit
import SwiftUI

/// Content-addressed artwork cache reader. Rust writes
/// `Application Support/Lyra/artwork/<h[..2]>/<h>/{full.<ext>,256.jpg,64.jpg}`
/// during scan; Swift composes paths and loads files — no pixel FFI.
enum Artwork {
    /// Same root derivation as `LyraLibrary` (db's parent + "artwork").
    /// Excluded from backup — it's a rebuildable cache.
    static let root: URL = {
        let u = FileManager.default
            .urls(for: .applicationSupportDirectory, in: .userDomainMask)[0]
            .appendingPathComponent("Lyra/artwork", isDirectory: true)
        try? FileManager.default.createDirectory(at: u, withIntermediateDirectories: true)
        try? (u as NSURL).setResourceValue(true, forKey: .isExcludedFromBackupKey)
        return u
    }()

    private static let cache = NSCache<NSString, NSImage>()

    /// `size` = 64 (table rows) or 256 (hero). full.<ext> needs a glob —
    /// thumbs are the fast path.
    static func url(_ hash: String, size: Int = 64) -> URL {
        root.appendingPathComponent("\(hash.prefix(2))/\(hash)/\(size).jpg")
    }

    /// Sync load with NSCache — thumbs are small; safe on any thread.
    static func image(_ hash: String?, size: Int = 64) -> NSImage? {
        guard let hash, !hash.isEmpty else { return nil }
        let key = "\(hash)@\(size)" as NSString
        if let hit = cache.object(forKey: key) { return hit }
        let u = url(hash, size: size)
        guard let img = NSImage(contentsOfFile: u.path) else { return nil }
        cache.setObject(img, forKey: key)
        return img
    }

    static func imageAsync(_ hash: String?, size: Int = 64,
                           _ done: @escaping (NSImage?) -> Void) {
        guard let hash, !hash.isEmpty else { done(nil); return }
        DispatchQueue.global(qos: .userInitiated).async {
            let img = image(hash, size: size)
            DispatchQueue.main.async { done(img) }
        }
    }

    /// Stage-size load — globs `full.<ext>` (any original format), falling
    /// back to the 256 thumb if the full file is gone.
    static func fullImage(_ hash: String?, _ done: @escaping (NSImage?) -> Void) {
        guard let hash, !hash.isEmpty else { done(nil); return }
        DispatchQueue.global(qos: .userInitiated).async {
            let dir = root.appendingPathComponent("\(hash.prefix(2))/\(hash)")
            let hit = (try? FileManager.default.contentsOfDirectory(
                at: dir, includingPropertiesForKeys: nil))?
                .first { $0.lastPathComponent.hasPrefix("full.") }
            let img = hit.flatMap { NSImage(contentsOfFile: $0.path) }
                ?? image(hash, size: 256)
            DispatchQueue.main.async { done(img) }
        }
    }
}

/// Online artwork fetch — Cover Art Archive resolved via MusicBrainz.
/// The Rust side negative-caches in `artwork_fetch`, so repeat calls for
/// an album with no art return `not_due` instantly rather than hitting
/// the network. Blocking FFI — always call off the main thread.
final class LyraArt {
    static let shared = LyraArt()
    private init() {}

    /// Returns the fetch result dict — `state` is "ok"|"cached"|
    /// "not_due"|"not_found"|"no_album"|"no_track"|"no_artist"|"error".
    @discardableResult
    func fetch(trackPath: String) -> [String: Any]? {
        guard let lib = LyraLibrary.shared.handle,
              let raw = trackPath.withCString({ lyra_art_fetch(lib, $0) })
        else { return nil }
        defer { lyra_string_free(raw) }
        return try? JSONSerialization.jsonObject(
            with: Data(String(cString: raw).utf8)) as? [String: Any]
    }
}

/// Square art tile with async load + album-initial placeholder — sits on
/// the sharp-editorial chrome (hairline border, no rounding).
struct ArtImage: View {
    let hash: String?
    /// Placeholder seed — album name initial.
    var label: String = ""
    var size: CGFloat = 32
    /// 64 for rows, 256 for the now-playing hero, 0 = full-resolution.
    var px: Int = 64
    @ObservedObject private var loader = ArtLoader()

    var body: some View {
        ZStack {
            if let img = loader.image {
                Image(nsImage: img)
                    .resizable()
                    .interpolation(.high)
            } else {
                Ui.bg
                Text(label.prefix(1).uppercased())
                    .font(.uiTitle)
                    .foregroundStyle(Ui.inkSoft)
            }
        }
        .frame(width: size, height: size)
        .overlay(Ui.border.frame(width: 1))
        .onAppear {
            // hop off the update pass — load() publishes `image` and a
            // sync write here faults "publishing during view updates"
            let l = loader, h = hash, p = px
            DispatchQueue.main.async { l.load(h, px: p) }
        }
        .onChange(of: hash) { _, h in loader.load(h, px: px) }
    }
}

/// @State is unavailable under CLT swiftc — the HoverState pattern again.
final class ArtLoader: ObservableObject {
    @Published var image: NSImage?
    private var lastKey: String?

    func load(_ hash: String?, px: Int) {
        let key = "\(hash ?? "")@\(px)"
        guard key != lastKey else { return }
        lastKey = key
        image = nil
        if px == 0 {
            Artwork.fullImage(hash) { [weak self] img in self?.image = img }
        } else {
            Artwork.imageAsync(hash, size: px) { [weak self] img in self?.image = img }
        }
    }
}
