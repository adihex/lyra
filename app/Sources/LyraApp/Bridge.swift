import Foundation

/// Thin Swift face over the lyra-ffi C ABI.
/// Contract: lyra_* strings are caller-freed via lyra_string_free.
enum LyraCore {

    static var version: String {
        guard let p = lyra_version() else { return "?" }
        return String(cString: p)
    }

    /// Probe an audio file → decoded JSON dictionary.
    /// Access is via security-scoped user selection (sandbox-safe).
    static func probe(path: String) -> [String: Any]? {
        guard let raw = path.withCString({ lyra_probe($0) }) else { return nil }
        defer { lyra_string_free(raw) }
        let json = String(cString: raw)
        return try? JSONSerialization.jsonObject(with: Data(json.utf8)) as? [String: Any]
    }

    /// Start the LAN remote server. Returns true if it launched.
    /// Security model is in lyra-remote; see BLUEPRINT.md § remote.
    @discardableResult
    static func startRemote(port: UInt16) -> Bool {
        lyra_remote_start(port) == 0
    }
}
