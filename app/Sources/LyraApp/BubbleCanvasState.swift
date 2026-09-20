import SwiftUI

/// Local interaction state for the Design Lab's layout canvas. This is
/// a self-contained demo — it deliberately has ZERO access to
/// ViewModel, LyraPlayer, or the Rust core, so reordering pads or
/// "running" the sequence can never touch real playback.

/// One pad in the demo key matrix.
struct BubblePad: Identifiable, Equatable {
    let id: Int
    let title: String
    let symbol: String

    /// The factory layout — reset() restores this exact order.
    static let defaults: [Self] = [
        .init(id: 0, title: "Kick", symbol: "circle.fill"),
        .init(id: 1, title: "Snare", symbol: "circle.dotted"),
        .init(id: 2, title: "Hat", symbol: "sun.max.fill"),
        .init(id: 3, title: "Clap", symbol: "hands.clap.fill"),
        .init(id: 4, title: "Bass", symbol: "waveform"),
        .init(id: 5, title: "Keys", symbol: "pianokeys"),
        .init(id: 6, title: "Lead", symbol: "bolt.fill"),
        .init(id: 7, title: "Chord", symbol: "music.note.list"),
        .init(id: 8, title: "FX", symbol: "sparkles"),
    ]
}

/// Key spacing for the canvas matrix — demonstrates the density tokens
/// without resizing the keycaps themselves.
enum BubbleDensity: String, CaseIterable, Identifiable {
    case compact = "Compact"
    case standard = "Standard"
    case spacious = "Spacious"
    var id: String { rawValue }
    var gap: CGFloat {
        switch self {
        case .compact: Bubble.Space.sm
        case .standard: Bubble.Space.lg
        case .spacious: Bubble.Space.xxl
        }
    }
}

final class BubbleCanvasState: ObservableObject {
    @Published var pads = BubblePad.defaults
    @Published var selectedID: Int? = nil
    @Published var density: BubbleDensity = .standard
    @Published var showLegends = true
    @Published var controlsEnabled = true
    @Published var running = false
    @Published var level = Bubble.Demo.defaultLevel
    @Published var name = "Studio matrix"

    var selectedIndex: Int? { pads.firstIndex { $0.id == selectedID } }
    var selectedTitle: String {
        pads.first { $0.id == selectedID }?.title ?? "No key selected"
    }

    /// Select a pad. Unknown IDs and the disabled state are no-ops so
    /// the canvas can never select a phantom key.
    func activate(_ id: Int) {
        guard controlsEnabled, pads.contains(where: { $0.id == id }) else { return }
        selectedID = id
    }

    func canMove(_ delta: Int) -> Bool {
        guard controlsEnabled, let i = selectedIndex else { return false }
        return pads.indices.contains(i + delta)
    }

    /// Reorder the selected pad by ±1 — pure data move, sizing untouched.
    func move(_ delta: Int) {
        guard canMove(delta), let i = selectedIndex else { return }
        pads.swapAt(i, i + delta)
    }

    /// Local "sequence" flag only — no audio is triggered.
    func toggleRunning() {
        guard controlsEnabled else { return }
        running.toggle()
    }

    /// Factory reset — restores layout, selection, and every inspector
    /// value to the shipped defaults.
    func reset() {
        pads = BubblePad.defaults
        selectedID = nil
        density = .standard
        showLegends = true
        controlsEnabled = true
        running = false
        level = Bubble.Demo.defaultLevel
        name = "Studio matrix"
    }
}
