import SwiftUI

/// Cmd-, home for the prefs the macOS-integration surfaces need
/// (docs §10): notification policy, menu-bar label/mount, dock-icon
/// driver mode, launch-at-login, and the exclusive-output toggle —
/// moved here from the Playback menu, not duplicated.
struct SettingsView: View {
    @ObservedObject private var vm = ViewModel.shared
    @ObservedObject private var prefs = Prefs.shared

    var body: some View {
        Form {
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
        .frame(width: 460)
    }
}
