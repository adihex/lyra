import SwiftUI

/// "Visuals" sidebar pane — mode chip strip on top, active renderer
/// filling the rest. Fits the ~400pt detail column: chips wrap via an
/// adaptive grid, surface takes all remaining height.
struct VisualsPane: View {
    @ObservedObject private var vm = ViewModel.shared
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    var body: some View {
        VStack(alignment: .leading, spacing: Ui.s12) {
            HStack(alignment: .firstTextBaseline) {
                Text("Visuals").font(.uiTitle).foregroundStyle(Ui.ink)
                Spacer()
                Text("\(vm.vizMode.displayName) · \(vm.vizMode.inputs)")
                    .font(.uiCaption).foregroundStyle(Ui.inkSoft)
            }
            if reduceMotion && vm.vizMode.decorative {
                Text("Reduce Motion — rendering Bars")
                    .font(.uiMicro).foregroundStyle(Ui.inkSoft)
            }
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
        .padding(Ui.s20)
        .frame(maxWidth: .infinity, alignment: .leading)
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
