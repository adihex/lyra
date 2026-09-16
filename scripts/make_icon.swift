import Cocoa

let S: CGFloat = 1024
let ctx = CGContext(
    data: nil, width: Int(S), height: Int(S),
    bitsPerComponent: 8, bytesPerRow: 0,
    space: CGColorSpaceCreateDeviceRGB(),
    bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue
)!

func pt(_ x: CGFloat, _ y: CGFloat) -> CGPoint { CGPoint(x: x, y: y) }
func col(_ r: CGFloat, _ g: CGFloat, _ b: CGFloat, _ a: CGFloat = 1) -> CGColor {
    CGColor(srgbRed: r, green: g, blue: b, alpha: a)
}

// ── background: deep indigo → near-black (base fill first so the
//    gradient can't leave transparent corners) ──
ctx.setFillColor(col(0.02, 0.03, 0.09))
ctx.fill(CGRect(x: 0, y: 0, width: S, height: S))
let bg = CGGradient(
    colorsSpace: CGColorSpaceCreateDeviceRGB(),
    colors: [col(0.14, 0.10, 0.36), col(0.02, 0.03, 0.09)] as CFArray,
    locations: [0, 1]
)!
ctx.drawLinearGradient(bg, start: pt(S * 0.3, S), end: pt(S * 0.7, 0), options: [])

// ── star field ──
srand48(20260916)
for _ in 0..<110 {
    let x = CGFloat(drand48()) * S, y = CGFloat(drand48()) * S
    let r = CGFloat(drand48()) * 2.4 + 0.5
    let a = CGFloat(drand48()) * 0.6 + 0.12
    let tint = drand48() < 0.2 ? col(0.6, 0.95, 0.85, a) : col(0.85, 0.9, 1, a)
    ctx.setFillColor(tint)
    ctx.fillEllipse(in: CGRect(x: x, y: y, width: r, height: r))
}
// two twinkle stars (little plus signs)
for (sx, sy, sr) in [(S * 0.78, S * 0.82, S * 0.016), (S * 0.2, S * 0.68, S * 0.012)] as [(CGFloat, CGFloat, CGFloat)] {
    ctx.setFillColor(col(0.9, 0.97, 1, 0.9))
    ctx.fillEllipse(in: CGRect(x: sx - sr * 0.35, y: sy - sr * 0.35, width: sr * 0.7, height: sr * 0.7))
    ctx.setStrokeColor(col(0.9, 0.97, 1, 0.55))
    ctx.setLineWidth(sr * 0.16)
    ctx.setLineCap(.round)
    ctx.move(to: pt(sx - sr, sy)); ctx.addLine(to: pt(sx + sr, sy))
    ctx.move(to: pt(sx, sy - sr)); ctx.addLine(to: pt(sx, sy + sr))
    ctx.strokePath()
}

// ── soft glow behind the planet ──
let cx = S / 2, cy = S * 0.47
let glow = CGGradient(
    colorsSpace: CGColorSpaceCreateDeviceRGB(),
    colors: [col(0.25, 0.85, 0.75, 0.28), col(0.25, 0.85, 0.75, 0)] as CFArray,
    locations: [0, 1]
)!
ctx.drawRadialGradient(
    glow, startCenter: pt(cx, cy), startRadius: 0,
    endCenter: pt(cx, cy), endRadius: S * 0.44,
    options: [.drawsAfterEndLocation]
)

// ── faint orbit path (the little moon rides it) ──
let orbitR = S * 0.36
ctx.setStrokeColor(col(0.7, 0.85, 1, 0.16))
ctx.setLineWidth(S * 0.003)
ctx.addArc(center: pt(cx, cy), radius: orbitR, startAngle: 0, endAngle: 2 * .pi, clockwise: false)
ctx.strokePath()

// ── planet ──
let pr = S * 0.215
let prect = CGRect(x: cx - pr, y: cy - pr, width: pr * 2, height: pr * 2)
let planet = CGGradient(
    colorsSpace: CGColorSpaceCreateDeviceRGB(),
    colors: [col(0.45, 0.55, 0.95), col(0.16, 0.22, 0.55), col(0.07, 0.10, 0.28)] as CFArray,
    locations: [0, 0.55, 1]
)!
ctx.saveGState()
ctx.addEllipse(in: prect)
ctx.clip()
ctx.drawRadialGradient(
    planet,
    startCenter: pt(cx - pr * 0.45, cy + pr * 0.5), startRadius: pr * 0.1,
    endCenter: pt(cx, cy), endRadius: pr * 1.6,
    options: [.drawsAfterEndLocation]
)
// lazy latitude bands — a chill gas giant
ctx.setStrokeColor(col(0.65, 0.75, 1, 0.16))
ctx.setLineWidth(pr * 0.09)
ctx.setLineCap(.round)
for k in [-1, 0, 1] as [CGFloat] {
    let y = cy + k * pr * 0.42
    ctx.move(to: pt(cx - pr * 0.75, y))
    ctx.addCurve(to: pt(cx + pr * 0.75, y - pr * 0.10),
                 control1: pt(cx - pr * 0.2, y + pr * 0.12),
                 control2: pt(cx + pr * 0.25, y - pr * 0.18))
    ctx.strokePath()
}
// big friendly crater spots
ctx.setFillColor(col(0.10, 0.16, 0.42, 0.55))
ctx.fillEllipse(in: CGRect(x: cx + pr * 0.28, y: cy - pr * 0.30, width: pr * 0.34, height: pr * 0.34))
ctx.fillEllipse(in: CGRect(x: cx - pr * 0.55, y: cy - pr * 0.05, width: pr * 0.22, height: pr * 0.22))
ctx.restoreGState()

// ── the little moon, parked on its orbit top-right ──
let mAng = -35 * CGFloat.pi / 180 // bottom-right (y-up)
let mx = cx + orbitR * cos(mAng), my = cy + orbitR * sin(mAng)
let mr = S * 0.055
let moonGrad = CGGradient(
    colorsSpace: CGColorSpaceCreateDeviceRGB(),
    colors: [col(0.85, 1.0, 0.95), col(0.45, 0.75, 0.72)] as CFArray,
    locations: [0, 1]
)!
ctx.saveGState()
ctx.addEllipse(in: CGRect(x: mx - mr, y: my - mr, width: mr * 2, height: mr * 2))
ctx.clip()
ctx.drawRadialGradient(
    moonGrad,
    startCenter: pt(mx - mr * 0.4, my + mr * 0.4), startRadius: 0,
    endCenter: pt(mx, my), endRadius: mr * 2.2,
    options: [.drawsAfterEndLocation]
)
ctx.setFillColor(col(0.3, 0.55, 0.55, 0.5))
ctx.fillEllipse(in: CGRect(x: mx - mr * 0.15, y: my - mr * 0.3, width: mr * 0.5, height: mr * 0.5))
ctx.restoreGState()

// ── save ──
let img = ctx.makeImage()!
let rep = NSBitmapImageRep(cgImage: img)
let out = CommandLine.arguments.count > 1 ? CommandLine.arguments[1] : "/tmp/lyra_icon.png"
try! rep.representation(using: .png, properties: [:])!
    .write(to: URL(fileURLWithPath: out))
print("wrote \(out)")
