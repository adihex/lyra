import Foundation

/// Analysis lane — wraps lyra_map_analyze / lyra_map_load /
/// lyra_map_for_track. `analyze` runs the full pipeline (decode + grid +
/// sections + chords + …) and is seconds-scale; call it off the main
/// thread. `load`/`forTrack` are artifact reads, main-safe.
final class LyraMap {
    static let shared = LyraMap()
    private init() {}

    /// maps/<audio_hash>.lyramap beside library.db.
    static var mapsDir: URL {
        LyraLibrary.dbDir.appendingPathComponent("maps", isDirectory: true)
    }

    /// Runs the pipeline on one track path. Returns the summary dict or nil.
    /// Heavy — background queues only.
    func analyze(lib: UnsafeMutableRawPointer?, path: String) -> [String: Any]? {
        path.withCString { p in
            Self.mapsDir.path.withCString { d in
                guard let raw = lyra_map_analyze(lib, p, d, nil) else { return nil }
                defer { lyra_string_free(raw) }
                return (try? JSONSerialization.jsonObject(
                    with: Data(String(cString: raw).utf8))) as? [String: Any]
            }
        }
    }

    /// Track path → decoded SongMap dict (nil when never analysed).
    func forTrack(lib: UnsafeMutableRawPointer?, path: String) -> [String: Any]? {
        path.withCString { p in
            guard let raw = lyra_map_for_track(lib, p) else { return nil }
            defer { lyra_string_free(raw) }
            let v = (try? JSONSerialization.jsonObject(
                with: Data(String(cString: raw).utf8))) as? [String: Any]
            guard let v, v["status"] as? String != "none",
                  v["status"] as? String != "missing_artifact",
                  v["error"] == nil
            else { return nil }
            return v
        }
    }
}

/// The pane's view of a SongMap — decoded once from the FFI JSON.
struct SongMapView {
    struct Section: Identifiable {
        let id = UUID()
        let t0: Double, t1: Double, label: String, conf: Double
    }
    struct Chord: Identifiable {
        let id = UUID()
        let t0: Double, t1: Double, name: String, conf: Double
    }
    let status: String
    let conf: Double
    let beats: Int
    let sections: [Section]
    let chords: [Chord]
    let strums: Int
    let notes: Int
    let tab: Int
    let duration: Double

    private static let roots = ["C", "C♯", "D", "D♯", "E", "F", "F♯", "G", "G♯", "A", "A♯", "B"]

    init?(_ v: [String: Any]) {
        let layers = v["quality"] as? [String: Any]
        let ls = layers?["layers"] as? [String: Any] ?? [:]
        func st(_ k: String) -> String { ls[k] as? String ?? "skipped" }
        let anyOk = ["grid", "sections", "chords", "notes", "tab"].contains { st($0) == "ok" }
        status = anyOk ? "ready" : "partial"
        conf = (layers?["overall"] as? NSNumber)?.doubleValue ?? 0
        duration = (v["duration_s"] as? NSNumber)?.doubleValue ?? 0
        beats = ((v["grid"] as? [String: Any])?["beats"] as? [Any])?.count ?? 0

        sections = ((v["sections"] as? [[String: Any]]) ?? []).map { s in
            Section(t0: (s["t0"] as? NSNumber)?.doubleValue ?? 0,
                    t1: (s["t1"] as? NSNumber)?.doubleValue ?? 0,
                    label: (s["label"] as? String ?? "unknown").capitalized,
                    conf: (s["conf"] as? NSNumber)?.doubleValue ?? 0)
        }
        chords = ((v["chords"] as? [[String: Any]]) ?? []).map { c in
            let name: String
            if let r = (c["root"] as? NSNumber)?.intValue {
                let q = c["quality"] as? String ?? "maj"
                name = Self.roots[r % 12] + (q == "min" ? "m" : q == "nc" ? " n.c." : "")
            } else {
                name = "n.c."
            }
            return Chord(t0: (c["t0"] as? NSNumber)?.doubleValue ?? 0,
                         t1: (c["t1"] as? NSNumber)?.doubleValue ?? 0,
                         name: name,
                         conf: (c["conf"] as? NSNumber)?.doubleValue ?? 0)
        }
        strums = (v["strums"] as? [Any])?.count ?? 0
        notes = (v["notes"] as? [Any])?.count ?? 0
        tab = (v["tab"] as? [Any])?.count ?? 0
    }
}
