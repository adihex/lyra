import Foundation

/// The 35-mode registry — names and order mirror cliamp's visualizers
/// (see docs/VIZ-CONTRACT.md); `cosmos` is Lyra's own dock-icon scene.
/// `decorative` marks particle/ambient modes that fall back to Bars
/// under Reduce Motion; meters/scope/wave stay because they convey
/// information.
enum VizMode: Int, CaseIterable, Identifiable {
    case bars = 1
    case barsDot
    case rain
    case outline
    case bricks
    case columns
    case classicPeak
    case wave
    case scatter
    case flame
    case retro
    case pulse
    case matrix
    case binary
    case sakura
    case firework
    case bubbles
    case logo
    case terrain
    case scope
    case heartbeat
    case butterfly
    case density
    case firefly
    case mosaic
    case sand
    case geyser
    case classicLED
    case stereo
    case mirror
    case dither
    case redSector
    case spectrogram
    case waveSeek
    case cosmos

    var id: Int { rawValue }

    var displayName: String {
        switch self {
        case .bars: "Bars"
        case .barsDot: "BarsDot"
        case .rain: "Rain"
        case .outline: "Outline"
        case .bricks: "Bricks"
        case .columns: "Columns"
        case .classicPeak: "ClassicPeak"
        case .wave: "Wave"
        case .scatter: "Scatter"
        case .flame: "Flame"
        case .retro: "Retro"
        case .pulse: "Pulse"
        case .matrix: "Matrix"
        case .binary: "Binary"
        case .sakura: "Sakura"
        case .firework: "Firework"
        case .bubbles: "Bubbles"
        case .logo: "Logo"
        case .terrain: "Terrain"
        case .scope: "Scope"
        case .heartbeat: "Heartbeat"
        case .butterfly: "Butterfly"
        case .density: "Density"
        case .firefly: "Firefly"
        case .mosaic: "Mosaic"
        case .sand: "Sand"
        case .geyser: "Geyser"
        case .classicLED: "ClassicLED"
        case .stereo: "Stereo"
        case .mirror: "Mirror"
        case .dither: "Dither"
        case .redSector: "RedSector"
        case .spectrogram: "Spectrogram"
        case .waveSeek: "WaveSeek"
        case .cosmos: "Cosmos"
        }
    }

    /// Contract input needs, for the picker subtitle.
    var inputs: String {
        switch self {
        case .bars, .barsDot, .outline, .bricks, .columns,
             .classicPeak, .density, .mirror:
            "bands"
        case .rain, .matrix:
            "bands · beat"
        case .flame, .firefly:
            "bands · bass"
        case .wave:
            "wave L/R"
        case .retro:
            "bands · wave"
        case .pulse:
            "level · beat"
        case .binary, .dither:
            "bands"
        case .sakura, .bubbles, .logo, .butterfly:
            "bands"
        case .firework:
            "bands · beat"
        case .terrain, .spectrogram:
            "bands ring"
        case .scope:
            "wave L/R"
        case .heartbeat:
            "wave · level"
        case .mosaic, .sand:
            "bands"
        case .geyser:
            "bands · bass · beat"
        case .classicLED:
            "bands · peak"
        case .stereo:
            "peak · rms"
        case .redSector:
            "bands"
        case .waveSeek:
            "peaks"
        case .scatter:
            "bands"
        case .cosmos:
            "bands · beat · level · wave"
        }
    }

    /// Decorative particle/ambient modes collapse to Bars under
    /// accessibilityReduceMotion. Meters, scopes, and the seekbar stay.
    var decorative: Bool {
        switch self {
        case .scatter, .flame, .retro, .pulse, .matrix, .binary,
             .sakura, .firework, .bubbles, .logo, .terrain, .butterfly,
             .density, .firefly, .mosaic, .sand, .geyser, .redSector,
             .spectrogram, .rain, .barsDot, .dither, .cosmos:
            true
        case .bars, .outline, .bricks, .columns, .classicPeak, .wave,
             .scope, .heartbeat, .classicLED, .stereo, .mirror, .waveSeek:
            false
        }
    }
}
