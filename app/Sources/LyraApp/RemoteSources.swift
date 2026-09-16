import Foundation

/// A remote library root reached over SSH/SFTP. Persisted in UserDefaults;
/// the password lives in the Keychain, never in this model. `keyBookmark`
/// is a security-scoped bookmark so a sandboxed build can keep reading an
/// identity file the user picked once.
struct RemoteSource: Codable, Identifiable, Hashable {
    var id = UUID()
    var name = ""            // display label; falls back to host
    var host = ""
    var port = 22
    var user = ""            // empty = ssh default / current user
    var rootPath = ""        // e.g. /mnt/music/flac
    var keyBookmark: Data?

    var label: String { name.isEmpty ? host : name }

    /// Prefix the store keys remote rows under — mirrors
    /// RemoteProfile::source_uri (`sftp://host[:port]` + absolute path).
    var uriPrefix: String {
        port == 22 ? "sftp://\(host)" : "sftp://\(host):\(port)"
    }

    /// Resolve + open the security-scoped key file. Returns the path Rust
    /// should read; nil when no key is set. Caller should NOT
    /// stopAccessing — the grant is cheap and outlives the call.
    func keyPath() -> String? {
        guard let bm = keyBookmark else { return nil }
        var stale = false
        guard let url = try? URL(resolvingBookmarkData: bm,
                                 options: .withSecurityScope,
                                 bookmarkDataIsStale: &stale)
        else { return nil }
        _ = url.startAccessingSecurityScopedResource()
        return url.path
    }

    /// FFI profile JSON ({host,port,user,key_path,root_path,password?}).
    func profileJSON(password: String?) -> String? {
        var d: [String: Any] = ["host": host, "port": port, "root_path": rootPath]
        if !user.isEmpty { d["user"] = user }
        if let k = keyPath() { d["key_path"] = k }
        if let pw = password, !pw.isEmpty { d["password"] = pw }
        guard let data = try? JSONSerialization.data(withJSONObject: d) else { return nil }
        return String(data: data, encoding: .utf8)
    }
}

/// Passwords for RemoteSource — service-scoped SecItems, one per profile.
enum Keychain {
    private static let service = "app.lyra.player.remote"

    static func set(_ secret: String, for account: String) {
        let q: [String: Any] = [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: service,
            kSecAttrAccount as String: account,
        ]
        SecItemDelete(q as CFDictionary)
        var add = q
        add[kSecValueData as String] = Data(secret.utf8)
        SecItemAdd(add as CFDictionary, nil)
    }

    static func get(_ account: String) -> String? {
        var q: [String: Any] = [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: service,
            kSecAttrAccount as String: account,
            kSecReturnData as String: true,
        ]
        var out: CFTypeRef?
        guard SecItemCopyMatching(q as CFDictionary, &out) == errSecSuccess,
              let data = out as? Data else { return nil }
        q[kSecMatchLimit as String] = kSecMatchLimitOne
        return String(data: data, encoding: .utf8)
    }

    static func remove(_ account: String) {
        SecItemDelete([
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: service,
            kSecAttrAccount as String: account,
        ] as CFDictionary)
    }
}

/// The persisted set of remote roots + the sftp:// → profile lookup the
/// player uses to route remote tracks.
final class RemoteSources: ObservableObject {
    static let shared = RemoteSources()
    @Published private(set) var all: [RemoteSource] = []
    private let defaultsKey = "remoteSources"

    private init() {
        if let data = UserDefaults.standard.data(forKey: defaultsKey),
           let list = try? JSONDecoder().decode([RemoteSource].self, from: data) {
            all = list
        }
    }

    private func save() {
        if let data = try? JSONEncoder().encode(all) {
            UserDefaults.standard.set(data, forKey: defaultsKey)
        }
    }

    func upsert(_ s: RemoteSource, password: String?) {
        var v = s
        if v.name.isEmpty { v.name = v.host }
        if let i = all.firstIndex(where: { $0.id == v.id }) { all[i] = v }
        else { all.append(v) }
        save()
        // Empty string clears; nil leaves any existing secret alone.
        if let pw = password {
            if pw.isEmpty { Keychain.remove(v.id.uuidString) }
            else { Keychain.set(pw, for: v.id.uuidString) }
        }
    }

    func remove(_ s: RemoteSource) {
        all.removeAll { $0.id == s.id }
        Keychain.remove(s.id.uuidString)
        save()
    }

    func password(for s: RemoteSource) -> String? {
        Keychain.get(s.id.uuidString)
    }

    /// Map an `sftp://` track path back to its profile — longest URI
    /// prefix wins when two profiles share a host.
    func profile(forPath path: String) -> RemoteSource? {
        all.filter { path.hasPrefix($0.uriPrefix) }
            .max { $0.uriPrefix.count < $1.uriPrefix.count }
    }

    /// Strip `sftp://host[:port]` off a store path → absolute remote path.
    func remotePath(_ storePath: String, for s: RemoteSource) -> String? {
        guard storePath.hasPrefix(s.uriPrefix) else { return nil }
        return String(storePath.dropFirst(s.uriPrefix.count))
    }
}

/// FFI wrapper — scan/test/pin (remote library) sit beside LyraPlayer's
/// playRemote. All blocking calls; callers go through DispatchQueue.
final class LyraFS {
    static let shared = LyraFS()
    private init() {}

    /// Health check: ssh find-count over the root → {ok,files,elapsed_ms}.
    func test(_ s: RemoteSource) -> [String: Any]? {
        guard let pj = s.profileJSON(password: RemoteSources.shared.password(for: s)),
              let raw = pj.withCString({ lyra_remlib_test($0) })
        else { return nil }
        defer { lyra_string_free(raw) }
        return try? JSONSerialization.jsonObject(with: Data(String(cString: raw).utf8)) as? [String: Any]
    }
}
