import SwiftUI

/// Per-mode Canvas renderers — the Swift port of cliamp's ui/vis_*.go.
/// Every renderer draws into a single Canvas per frame; state lives in
/// VizState (particles ≤512, bounded rings/CA grids) per the contract.
enum VizDraw {

    /// Spectrum tier ramp — cliamp's specTag (low/mid/high by normalized
    /// height) mapped onto the Ui palette: mint body, indigo mid,
    /// terracotta peak. `t` is height-from-bottom 0...1.
    static func spec(_ t: Float) -> Color {
        if t >= 0.6 { return Ui.accent }
        if t >= 0.3 { return Ui.indigo }
        return Ui.mint
    }

    /// Vertical gradient anchored to the panel (not the bar) so every bar
    /// shows the same mint→indigo→accent climb — what cliamp's per-row
    /// coloring produces.
    static func specGradient(_ size: CGSize) -> GraphicsContext.Shading {
        .linearGradient(
            Gradient(stops: [
                .init(color: Ui.accent, location: 0.0),
                .init(color: Ui.indigo, location: 0.4),
                .init(color: Ui.mint, location: 0.7),
            ]),
            startPoint: .zero, endPoint: CGPoint(x: 0, y: size.height))
    }

    static func render(_ mode: VizMode, _ ctx: inout GraphicsContext,
                       _ size: CGSize, _ f: VizFrame, _ st: VizState) {
        switch mode {
        case .bars: bars(&ctx, size, f)
        case .barsDot: barsDot(&ctx, size, f)
        case .rain: rain(&ctx, size, f, st)
        case .outline: outline(&ctx, size, f)
        case .bricks: bricks(&ctx, size, f)
        case .columns: columns(&ctx, size, f)
        case .classicPeak: classicPeak(&ctx, size, f, st)
        case .wave: wave(&ctx, size, f)
        case .scatter: scatter(&ctx, size, f, st)
        case .flame: flame(&ctx, size, f, st)
        case .retro: retro(&ctx, size, f, st)
        case .pulse: pulse(&ctx, size, f, st)
        case .matrix: matrix(&ctx, size, f, st)
        case .binary: binary(&ctx, size, f, st)
        case .sakura: sakura(&ctx, size, f, st)
        case .firework: firework(&ctx, size, f, st)
        case .bubbles: bubbles(&ctx, size, f, st)
        case .logo: logo(&ctx, size, f, st)
        case .terrain: terrain(&ctx, size, f, st)
        case .scope: scope(&ctx, size, f)
        case .heartbeat: heartbeat(&ctx, size, f, st)
        case .butterfly: butterfly(&ctx, size, f, st)
        case .density: density(&ctx, size, f)
        case .firefly: firefly(&ctx, size, f, st)
        case .mosaic: mosaic(&ctx, size, f, st)
        case .sand: sand(&ctx, size, f, st)
        case .geyser: geyser(&ctx, size, f, st)
        case .classicLED: classicLED(&ctx, size, f, st)
        case .stereo: stereo(&ctx, size, f, st)
        case .mirror: mirror(&ctx, size, f, st)
        case .dither: dither(&ctx, size, f, st)
        case .redSector: redSector(&ctx, size, f, st)
        case .spectrogram: spectrogram(&ctx, size, f, st)
        case .waveSeek: waveSeek(&ctx, size, f, st)
        }
        clipBadge(&ctx, size, f)
    }

    /// Sticky clip lamps, top-right — L/R squares when the engine clipped.
    static func clipBadge(_ ctx: inout GraphicsContext, _ size: CGSize, _ f: VizFrame) {
        guard f.clipL || f.clipR else { return }
        if f.clipL {
            ctx.fill(Path(CGRect(x: size.width - 26, y: 6, width: 8, height: 8)),
                     with: .color(.red.opacity(0.85)))
        }
        if f.clipR {
            ctx.fill(Path(CGRect(x: size.width - 14, y: 6, width: 8, height: 8)),
                     with: .color(.red.opacity(0.85)))
        }
    }

    // shared: hairline frame inside the canvas edge
    static func frameHairline(_ ctx: inout GraphicsContext, _ size: CGSize) {
        ctx.stroke(Path(CGRect(x: 0.5, y: 0.5,
                               width: size.width - 1, height: size.height - 1)),
                   with: .color(Ui.border.opacity(0.6)), lineWidth: 1)
    }
}
