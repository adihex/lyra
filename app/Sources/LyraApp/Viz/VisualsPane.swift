import SwiftUI

/// "Visuals" sidebar pane — two faces on one card: the visualizer (mode
/// chips + renderer) and the album-art stage. `vm.stageFace` flips it —
/// set by the face chips here or the transport bar (mini viz → viz,
/// art tile → art). The flip is a literal card rotation around Y.
struct VisualsPane: View {
    @ObservedObject private var vm = ViewModel.shared
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    var body: some View {
        VStack(alignment: .leading, spacing: Ui.s12) {
            HStack(alignment: .firstTextBaseline, spacing: Ui.s8) {
                FaceChip(title: "Visuals", active: vm.stageFace == .viz) {
                    vm.stageFace = .viz
                }
                FaceChip(title: "Artwork", active: vm.stageFace == .art) {
                    vm.stageFace = .art
                }
                Spacer()
                Text(caption)
                    .font(.uiCaption).foregroundStyle(Ui.inkSoft)
                    .lineLimit(1)
            }
            if vm.stageFace == .viz && reduceMotion && vm.vizMode.decorative {
                Text("Reduce Motion — rendering Bars")
                    .font(.uiMicro).foregroundStyle(Ui.inkSoft)
            }
            ZStack {
                vizFace.opacity(vm.stageFace == .viz ? 1 : 0)
                artFace.opacity(vm.stageFace == .art ? 1 : 0)
                    .rotation3DEffect(.degrees(180), axis: (x: 0, y: 1, z: 0))
            }
            .rotation3DEffect(.degrees(vm.stageFace == .art ? 180 : 0),
                              axis: (x: 0, y: 1, z: 0), perspective: 0.6)
            .animation(.easeInOut(duration: 0.3), value: vm.stageFace)
        }
        .padding(Ui.s20)
        .frame(maxWidth: .infinity, alignment: .leading)
    }

    private var caption: String {
        if vm.stageFace == .viz {
            "\(vm.vizMode.displayName) · \(vm.vizMode.inputs)"
        } else {
            vm.current.map { "\($0.title) — \($0.album)" } ?? "Nothing playing"
        }
    }

    // Front face — mode chip strip on top, active renderer filling the
    // rest. Chips wrap via an adaptive grid, surface takes all height.
    private var vizFace: some View {
        VStack(alignment: .leading, spacing: Ui.s12) {
            LazyVGrid(columns: [GridItem(.adaptive(minimum: 72), spacing: 6)],
                      spacing: 6) {
                ForEach(VizMode.allCases) { m in
                    VizChip(mode: m, active: m == vm.vizMode) {
                        vm.vizMode = m
                    }
                }
            }
            VizSurfaceView(mode: vm.vizMode)
                .frame(maxWidth: .infinity, maxHeight: .infinity)
                .background(Ui.surface)
                .overlay(Rectangle().stroke(Ui.border, lineWidth: 1))
                .clipped()
        }
    }

    // Back face — full-resolution artwork on the same card chrome.
    private var artFace: some View {
        GeometryReader { g in
            let side = max(140, min(g.size.width, g.size.height) - 48)
            VStack(spacing: Ui.s12) {
                Spacer(minLength: 0)
                ArtImage(hash: vm.current?.artworkHash,
                         label: vm.current?.album ?? "♪",
                         size: side, px: 0)
                Text(vm.current.map { "\($0.artist ?? "Unknown") — \($0.album)" }
                     ?? "No track selected")
                    .font(.uiCaption).foregroundStyle(Ui.inkSoft)
                    .lineLimit(1)
                Spacer(minLength: 0)
            }
            .frame(maxWidth: .infinity)
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .background(Ui.surface)
        .overlay(Rectangle().stroke(Ui.border, lineWidth: 1))
        .clipped()
    }
}

/// Face toggle — square editorial chip matching VizChip.
private struct FaceChip: View {
    let title: String
    let active: Bool
    let action: () -> Void

    var body: some View {
        Button(action: action) {
            Text(title)
                .font(.uiCaption)
                .foregroundStyle(active ? Color.white : Ui.ink)
                .padding(.horizontal, 10)
                .padding(.vertical, 4)
                .background(active ? Ui.accent : Ui.surface)
                .overlay(Rectangle().stroke(active ? Ui.accent : Ui.border,
                                            lineWidth: 1))
        }
        .buttonStyle(.plain)
        .uiElevated()
    }
}

/// Mode chip — square editorial chip with the hard-offset elevation card.
private struct VizChip: View {
    let mode: VizMode
    let active: Bool
    let action: () -> Void

    var body: some View {
        Button(action: action) {
            Text(mode.displayName)
                .font(.uiMicro)
                .foregroundStyle(active ? Color.white : Ui.ink)
                .lineLimit(1)
                .frame(maxWidth: .infinity)
                .padding(.vertical, 5)
                .background(active ? Ui.accent : Ui.surface)
                .overlay(Rectangle().stroke(active ? Ui.accent : Ui.border,
                                            lineWidth: 1))
        }
        .buttonStyle(.plain)
        .uiElevated()
    }
}
