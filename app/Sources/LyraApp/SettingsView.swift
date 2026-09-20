import SwiftUI

/// Cmd-, home for the prefs the macOS-integration surfaces need
/// (docs §10): notification policy, menu-bar label/mount, dock-icon
/// driver mode, launch-at-login, and the exclusive-output toggle —
/// moved here from the Playback menu, not duplicated.
struct SettingsView: View {
    @ObservedObject private var vm = ViewModel.shared
    @ObservedObject private var prefs = Prefs.shared
    @ObservedObject private var torznab = TorznabEndpoints.shared
    @ObservedObject private var theme = LyraTheme.shared
    @State private var torzUrl = ""
    @State private var torzKey = ""
    @State private var torzName = ""

    var body: some View {
        Form {
            Section("Appearance") {
                Picker("Appearance", selection: $theme.appearance) {
                    ForEach(LyraAppearance.allCases) { a in
                        Text(a.title).tag(a)
                    }
                }
                .pickerStyle(.segmented)
                .tint(Color.bubbleNativeSelectionTint)
                Picker("Palette", selection: $theme.palette) {
                    ForEach(LyraPalette.allCases) { p in
                        HStack {
                            PaletteSwatch(palette: p)
                            Text(p.title)
                        }
                        .tag(p)
                    }
                }
                .pickerStyle(.menu)
                Text("Applies to Lyra, the mini-player, and your desktop pet.")
                    .font(.uiCaption).foregroundStyle(Ui.inkSoft)
            }
            Section("Playback") {
                // Exclusive HAL output: hog mode + IOProc. Engine swaps
                // at runtime; choice persists to the next launch.
                Toggle("Exclusive Output (HAL)", isOn: Binding(
                    get: { vm.exclusiveOutput },
                    set: { vm.setExclusiveOutput($0) }))
                Text("Hogs the device for bit-perfect output — falls back to shared if the device is busy.")
                    .font(.uiCaption).foregroundStyle(Ui.inkSoft)
            }
            Section("Notifications") {
                Picker("Track changes", selection: $prefs.notifyMode) {
                    Text("Off").tag("off")
                    Text("On album change").tag("album")
                    Text("Every track").tag("all")
                }
                Text("Silent banners, suppressed while Lyra is frontmost — they never interrupt Focus.")
                    .font(.uiCaption).foregroundStyle(Ui.inkSoft)
            }
            Section("Menu Bar") {
                Toggle("Show mini player", isOn: $prefs.menuBarExtra)
                Picker("Icon", selection: $prefs.menuBarMode) {
                    Text("Spectrum").tag("spectrum")
                    Text("Pulse").tag("pulse")
                    Text("Note").tag("note")
                }
            }
            Section("Dock") {
                Picker("Icon", selection: $prefs.dockIconMode) {
                    Text("Animate while playing").tag("playing")
                    Text("Static").tag("off")
                    Text("Hidden").tag("hidden")
                }
            }
            Section("Discover — Torznab Indexers") {
                ForEach(torznab.all) { e in
                    HStack {
                        VStack(alignment: .leading, spacing: 2) {
                            Text(e.name).font(.uiBody)
                            Text(e.url).font(.uiCaption).foregroundStyle(Ui.inkSoft)
                                .lineLimit(1).truncationMode(.middle)
                        }
                        Spacer()
                        Button("Remove") { torznab.remove(e) }
                            .buttonStyle(.sharp)
                    }
                }
                TextField("Label (optional)", text: $torzName)
                TextField("Indexer URL — e.g. http://host:9117/api/v2.0/indexers/all", text: $torzUrl)
                SecureField("API key", text: $torzKey)
                HStack {
                    Spacer()
                    Button("Add indexer") {
                        let e = TorznabEndpoint(name: torzName, url: torzUrl)
                        guard !e.url.isEmpty else { return }
                        torznab.upsert(e, apikey: torzKey)
                        torzName = ""; torzUrl = ""; torzKey = ""
                    }
                    .buttonStyle(.sharp)
                    .disabled(torzUrl.trimmingCharacters(in: .whitespaces).isEmpty)
                }
                Text("Jackett/Prowlarr endpoints — copy an indexer's Torznab feed URL and your API key. The key stays in Keychain.")
                    .font(.uiCaption).foregroundStyle(Ui.inkSoft)
            }
            Section("System") {
                Toggle("Launch at login", isOn: Binding(
                    get: { prefs.launchAtLogin },
                    set: { prefs.setLaunchAtLogin($0) }))
                if prefs.launchNeedsApproval {
                    HStack {
                        Text("Login item needs approval.")
                            .font(.uiCaption).foregroundStyle(Ui.inkSoft)
                        Spacer()
                        Button("Open System Settings") { LoginItem.openSystemSettings() }
                            .buttonStyle(.sharp)
                    }
                }
            }
        }
        .formStyle(.grouped)
        .toggleStyle(.bubble)
        .frame(width: 460)
    }
}
