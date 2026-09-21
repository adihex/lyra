import CoreSpotlight
import UniformTypeIdentifiers

/// Library → Spotlight bridge (docs §4). `CSSearchableIndex.default()`
/// is app-owned and fully sandboxed — no entitlement. Items key on the
/// track path so a Spotlight pick routes straight into `playTrack(id:)`
/// via `continueUserActivity` (CSSearchableItemActionType).
final class SpotlightIndex {
    static let shared = SpotlightIndex()

    private let index = CSSearchableIndex.default()
    private let domain = "tracks"
    /// A restore that landed before the library finished loading (cold
    /// launch via a Spotlight hit) — flushed by `libraryReady`.
    private var pendingOpen: String?

    /// Reindex the file-backed library after a sync. Torrent rows aren't
    /// files — they stay out. Delete-domain + re-add is simpler and
    /// always consistent vs hand-rolled add/remove diffing.
    func sync(_ tracks: [Track]) {
        libraryReady(tracks)
        let items = tracks.filter { $0.source == .file }.map(item(for:))
        index.deleteSearchableItems(withDomainIdentifiers: [domain]) { _ in
            // The default index is CSSearchableIndexShared — batching
            // (begin/endIndexBatch) throws on it as of macOS 26.
            self.index.indexSearchableItems(items) { _ in }
        }
    }

    /// Wipe our domain — for a future "clear library" path.
    func clearAll() {
        index.deleteSearchableItems(withDomainIdentifiers: [domain]) { _ in }
    }

    /// Latest library snapshot — flushes a pending restore.
    func libraryReady(_ tracks: [Track]) {
        guard let id = pendingOpen else { return }
        if tracks.contains(where: { $0.id == id }) {
            pendingOpen = nil
            DispatchQueue.main.async { ViewModel.shared.playTrack(id: id) }
        }
    }

    /// Spotlight restore — select + play; defers if the library isn't
    /// loaded yet.
    func openTrack(_ id: String) {
        if ViewModel.shared.tracks.contains(where: { $0.id == id }) {
            ViewModel.shared.playTrack(id: id)
        } else {
            pendingOpen = id
        }
    }

    private func item(for t: Track) -> CSSearchableItem {
        let a = CSSearchableItemAttributeSet(contentType: .audio)
        a.title = t.title
        a.artist = t.artist
        a.album = t.album
        a.duration = NSNumber(value: t.duration)
        a.keywords = [t.codec, t.format, "Lyra"]
        a.path = t.path
        a.contentURL = URL(fileURLWithPath: t.path)
        if let hash = t.artworkHash,
           let data = try? Data(contentsOf: Artwork.url(hash, size: 256)) {
            a.thumbnailData = data
        }
        return CSSearchableItem(uniqueIdentifier: t.path,
                                domainIdentifier: domain, attributeSet: a)
    }

    /// Spotlight can ask the index owner to rebuild — delegate calls in
    /// (from AppDelegate) funnel here.
    func reindexAll() {
        DispatchQueue.main.async { self.sync(ViewModel.shared.tracks) }
    }

    /// Rebuild a subset by identifier; ids no longer in the library get
    /// deleted from the index.
    func reindex(_ ids: [String]) {
        DispatchQueue.main.async {
            let known = ViewModel.shared.tracks
            let found = known.filter { ids.contains($0.id) }.map(self.item(for:))
            let missing = ids.filter { id in !known.contains { $0.id == id } }
            if !missing.isEmpty {
                self.index.deleteSearchableItems(withIdentifiers: missing) { _ in }
            }
            if !found.isEmpty {
                self.index.indexSearchableItems(found) { _ in }
            }
        }
    }
}
