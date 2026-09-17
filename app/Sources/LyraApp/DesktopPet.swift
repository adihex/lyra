import AppKit
import Combine
import CoreGraphics

/// The cosmos cast living on the desktop — docs/research/desktop-pet.md.
/// Three borderless non-activating NSPanels (saturn/moon/logo) roamed by
/// `CosmosWorld`, painted by `PetPaint`, driven by a 30 Hz Timer over
/// `VizRuntime.pumpIfStale` — the pet is the pumper only when Lyra's
/// windowed surfaces aren't.
final class DesktopPet: ObservableObject {
    static let shared = DesktopPet()

    /// Lifecycle policy — UserDefaults-persisted, default While playing.
    enum Policy: Int, CaseIterable {
        case off = 0, whilePlaying = 1, always = 2
        var title: String {
            switch self {
            case .off: "Off"
            case .whilePlaying: "While Playing"
            case .always: "Always"
            }
        }
    }

    @Published var policy: Policy {
        didSet {
            UserDefaults.standard.set(policy.rawValue, forKey: "petPolicy")
            userHidden = false // a policy change re-arms the pet
            policyTick()
        }
    }
    /// Ghost (default) = click-through ambiance; pet = draggable panels.
    @Published var interactive: Bool {
        didSet {
            UserDefaults.standard.set(interactive, forKey: "petInteractive")
            for (_, p) in panels { p.ignoresMouseEvents = !interactive }
        }
    }
    @Published private(set) var isOut = false
    @Published private(set) var userHidden = false
    @Published private(set) var sleeping = false

    /// Panel sizes per doc §2's cast (~140 pt planet + moon escort).
    /// The logo note stays in the cosmos — it doesn't pop as a pet.
    static let panelSize: [CosmosScene.Body: CGFloat] = [
        .saturn: 140, .moon: 56,
    ]

    private enum Phase {
        case hidden
        case popping(CGFloat)    // t 0…1 of the ~600 ms burst
        case roaming
        case recalling(CGFloat)  // t 0…1 of the return arc
    }

    /// The shared scene on VizRuntime — a class, so "adopting the dock
    /// scene's state" is literal: the pet keeps driving the same planet.
    private var scene: CosmosScene { ViewModel.shared.viz.cosmos }
    private var bandHi: Float = 0   // bands[48…63] — twinkle aura
    private var bandMid: Float = 0  // bands[21…47] — stripes
    private var bandGlint: Float = 0 // max(peakL, peakR) — ring glint
    private var world = CosmosWorld(bounds: .zero)
    private var panels: [CosmosScene.Body: NSPanel] = [:]
    private var views: [CosmosScene.Body: PetBodyView] = [:]
    private var fxs: [CosmosScene.Body: PetPaint.FX] = [:]
    private var phase: Phase = .hidden
    private var popFrom: [CosmosScene.Body: CGPoint] = [:]
    private var popCtrl: [CosmosScene.Body: CGPoint] = [:]
    private var popTo: [CosmosScene.Body: CGPoint] = [:]
    private var perchRects: [CGWindowID: CGRect] = [:]
    private var driveTimer: Timer?
    private var policyTimer: Timer?
    private var perchTimer: Timer?
    private var lastT = CFAbsoluteTimeGetCurrent()
    private var settledT: Float = 0

    private init() {
        policy = Policy(rawValue: UserDefaults.standard.integer(forKey: "petPolicy"))
            ?? .whilePlaying
        interactive = UserDefaults.standard.bool(forKey: "petInteractive")
        policyTimer = Timer.scheduledTimer(withTimeInterval: 1, repeats: true) {
            [weak self] _ in self?.policyTick()
        }
        perchTimer = Timer.scheduledTimer(withTimeInterval: 4, repeats: true) {
            [weak self] _ in self?.enumPerches()
        }
        NotificationCenter.default.addObserver(
            self, selector: #selector(screensChanged),
            name: NSApplication.didChangeScreenParametersNotification,
            object: nil)
    }

    /// Painter's read-only scene handle (draw stays pure).
    var sceneForPaint: CosmosScene { scene }

    /// Painter inputs for one body's current frame (pop spring, dim,
    /// dash, logo spin all funnel through here).
    func fx(for b: CosmosScene.Body) -> PetPaint.FX {
        var f = fxs[b] ?? PetPaint.FX()
        f.dim = CGFloat(world.dimness)
        f.spin = CGFloat(world.logoAngle)
        f.dashT = CGFloat(world.dashT)
        f.dashDir = world.dashDir
        f.hi = CGFloat(bandHi)
        f.mid = CGFloat(bandMid)
        f.glint = CGFloat(bandGlint)
        return f
    }

    /// Menu-bar escape hatch: lost/hidden → summon; out → send home.
    func summonOrRecall() {
        if isOut { recall() } else { userHidden = false; policyTick() }
    }

    // ── policy: Off / While playing / Always (doc §5) ────────────────
    @objc private func policyTick() {
        let reduce = NSWorkspace.shared.accessibilityDisplayShouldReduceMotion
        if isOut && reduce { teardown(); return } // Reduce Motion: no arc
        guard !userHidden, !reduce else { return }
        let playing = LyraPlayer.shared.isPlaying
        switch policy {
        case .off:
            if isOut { recall() }
        case .whilePlaying:
            if playing { if !isOut { pop() } }
            else if isOut { recall() }
        case .always:
            if !isOut { pop() }
        }
        // lost-pet rescue — the drive loop can be stopped (settled sleep)
        // while a body sits fully off the live screen; policyTick always
        // runs, so it owns the last-resort re-seat
        if isOut, case .roaming = phase {
            let vb = lyraScreen().visibleFrame
            var lost = false
            for (b, c) in world.bodies
            where !vb.insetBy(dx: -20, dy: -20).contains(c) {
                world.bodies[b] = CGPoint(
                    x: min(max(c.x, vb.minX + 70), vb.maxX - 70),
                    y: vb.minY + 76)
                lost = true
            }
            if lost { ensureDrive() }
        }
        // keep the drive loop alive while roaming (it parks itself in
        // sleep; stall-wake needs it back)
        if isOut, case .roaming = phase,
           !(world.fsm == .sleep && world.settled) {
            ensureDrive()
        }
    }

    // ── the pop: panels burst from the estimated dock-tile rect ──────
    private func pop() {
        guard !isOut else { return }
        let screen = lyraScreen()
        let vis = screen.visibleFrame
        // the shared scene keeps rolling — the same planet steps out,
        // and `away` empties the dock painter's sky (doc §3/§7)
        ViewModel.shared.viz.cosmos.away = [.saturn, .moon]
        world = CosmosWorld(bounds: vis)
        let origin = dockEstimate(on: screen)
        let land = CGPoint(
            x: vis.midX + CGFloat.random(in: -0.22...0.22) * vis.width,
            y: vis.minY + 76)
        let arc = CGPoint(x: (origin.x + land.x) / 2,
                          y: max(origin.y, land.y) + 220)
        popFrom = [:]; popCtrl = [:]; popTo = [:]
        popFrom[.saturn] = origin; popCtrl[.saturn] = arc
        popTo[.saturn] = land
        popFrom[.moon] = origin; popCtrl[.moon] = arc
        popTo[.moon] = CGPoint(x: land.x + 40, y: land.y - 24)
        if panels.isEmpty { makePanels() }
        for (b, p) in panels {
            if let o = popFrom[b] { placePanel(p, center: o) }
            p.orderFrontRegardless()
            fxs[b] = PetPaint.FX(scale: 0.2, alpha: 0)
        }
        phase = .popping(0)
        isOut = true
        ensureDrive()
    }

    /// Reverse arc into the tile estimate; the dock scene regains its
    /// planet when the panels fade at the edge.
    func recall() {
        guard isOut else { return }
        if case .recalling = phase { return }
        let screen = lyraScreen()
        let home = dockEstimate(on: screen)
        popFrom = world.bodies
        popTo = [:]; popCtrl = [:]
        for b in DesktopPet.panelSize.keys {
            popTo[b] = home
            let cur = world.bodies[b] ?? home
            popCtrl[b] = CGPoint(x: (cur.x + home.x) / 2,
                                 y: max(cur.y, home.y) + 140)
        }
        phase = .recalling(0)
        ensureDrive()
    }

    // ── 30 Hz drive loop ─────────────────────────────────────────────
    private func ensureDrive() {
        guard driveTimer == nil else { return }
        lastT = CFAbsoluteTimeGetCurrent()
        driveTimer = Timer.scheduledTimer(withTimeInterval: 1.0 / 30,
                                          repeats: true) { [weak self] _ in
            self?.driveTick()
        }
    }

    @objc private func driveTick() {
        let now = CFAbsoluteTimeGetCurrent()
        let dt = Float(min(now - lastT, 0.1))
        lastT = now
        let f = ViewModel.shared.viz.pumpIfStale(.milliseconds(33))
        scene.drive(f, dt: dt)
        bandHi = bandAvg(f.bands, 48, 64)
        bandMid = bandAvg(f.bands, 21, 48)
        bandGlint = max(f.peakL, f.peakR)

        switch phase {
        case .hidden:
            return
        case .popping(let t):
            let starts: [CosmosScene.Body: CGFloat] = [.saturn: 0, .moon: 0.18]
            for b in DesktopPet.panelSize.keys {
                let tt = min(max((t - starts[b]!) / (1 - starts[b]!), 0), 1)
                let e = 1 - (1 - tt) * (1 - tt) // ease-out ballistic
                world.bodies[b] = bezier(popFrom[b]!, popCtrl[b]!, popTo[b]!, e)
                var fx = fxs[b] ?? PetPaint.FX()
                fx.scale = 0.2 + 0.8 * (1 - exp(-5.5 * tt) * cos(9 * tt)) // spring w/ overshoot
                fx.alpha = min(1, tt * 4)
                fxs[b] = fx
            }
            if t >= 1 {
                phase = .roaming
                world.awaitingBeat = true // first hop syncs to next beat
                for b in DesktopPet.panelSize.keys {
                    fxs[b] = PetPaint.FX()
                }
            } else {
                phase = .popping(t + CGFloat(dt) / 0.6)
            }
        case .roaming:
            world.roam(f, scene, dt: dt, bounds: lyraScreen().visibleFrame,
                       perches: perchRects)
        case .recalling(let t):
            let starts: [CosmosScene.Body: CGFloat] = [.saturn: 0.2, .moon: 0.05]
            var done = true
            for b in DesktopPet.panelSize.keys {
                let tt = min(max((t - starts[b]!) / (1 - starts[b]!), 0), 1)
                if tt < 1 { done = false }
                let e = tt * tt // ease-in — accelerates into the tile
                world.bodies[b] = bezier(popFrom[b]!, popCtrl[b]!, popTo[b]!, e)
                var fx = fxs[b] ?? PetPaint.FX()
                fx.alpha = 1 - smooth((tt - 0.65) / 0.35)
                fx.scale = 1 - 0.55 * smooth((tt - 0.8) / 0.2)
                fxs[b] = fx
            }
            if done { teardown(); return }
            phase = .recalling(t + CGFloat(dt) / 0.7)
        }

        // stale-bounds rescue: bodies fully outside the live screen's
        // visible frame (slept on a detached display, clamp-free phases)
        // get walked back to the bottom edge — roam() only clamps while
        // it's running, and the drive loop can sleep with a lost body
        let vb = lyraScreen().visibleFrame
        for (b, c) in world.bodies
        where !vb.insetBy(dx: -20, dy: -20).contains(c) {
            world.bodies[b] = CGPoint(
                x: min(max(c.x, vb.minX + 70), vb.maxX - 70),
                y: vb.minY + 76)
        }
        for (b, p) in panels {
            if let c = world.bodies[b] { placePanel(p, center: c) }
            views[b]?.needsDisplay = true
        }
        let s = world.fsm == .sleep
        if s != sleeping { sleeping = s }

        // seq-stall sleep: once converged, stop paying window moves —
        // the policy tick restarts the loop on wake/pop/recall
        if case .roaming = phase, world.settled {
            settledT += dt
            if settledT > 2 {
                driveTimer?.invalidate()
                driveTimer = nil
            }
        } else {
            settledT = 0
        }
    }

    // ── dock-tile rect estimate — heuristic route A (doc §3): edge
    //    from frame/visibleFrame deltas, slot from running-app count,
    //    autohide falls back to the orientation default ───────────────
    private func dockEstimate(on screen: NSScreen) -> CGPoint {
        let f = screen.frame, v = screen.visibleFrame
        var pitch = max(NSApp.dockTile.size.width, NSApp.dockTile.size.height)
        if pitch < 20 { pitch = 52 } // size unset until a custom tile lands
        let dB = v.minY - f.minY, dL = v.minX - f.minX, dR = f.maxX - v.maxX
        enum Edge { case bottom, left, right }
        let edge: Edge
        if dB > 20 { edge = .bottom }
        else if dL > 20 { edge = .left }
        else if dR > 20 { edge = .right }
        else {
            // autohide steals no space — ask the dock plist (nil under
            // sandbox is fine, bottom is the sane default)
            switch UserDefaults(suiteName: "com.apple.dock")?
                .string(forKey: "orientation") ?? "bottom" {
            case "left": edge = .left
            case "right": edge = .right
            default: edge = .bottom
            }
        }
        // slot: pinned + running-regular ≈ icon count; running apps we
        // aren't pinned among append at the end — aim ~1.5 pitches in
        // from that end. A few pt of error is invisible (doc §3).
        let apps = CGFloat(NSWorkspace.shared.runningApplications
            .filter { $0.activationPolicy == .regular }.count) + 6
        let len = pitch * apps
        switch edge {
        case .bottom:
            let x = min(max(f.midX + len / 2 - 1.5 * pitch, f.minX + pitch),
                        f.maxX - pitch)
            return CGPoint(x: x, y: f.minY + max(dB, pitch * 0.6) * 0.5)
        case .left:
            return CGPoint(x: f.minX + max(dL, pitch * 0.6) * 0.5,
                           y: f.minY + 2.2 * pitch)
        case .right:
            return CGPoint(x: f.maxX - max(dR, pitch * 0.6) * 0.5,
                           y: f.minY + 2.2 * pitch)
        }
    }

    // ── perch targets: on-screen windows' top edges, no AX needed ────
    @objc private func enumPerches() {
        guard isOut else { return }
        let opts: CGWindowListOption = [.optionOnScreenOnly, .excludeDesktopElements]
        guard let list = CGWindowListCopyWindowInfo(opts, kCGNullWindowID)
                as? [[String: Any]] else { return }
        let myPID = ProcessInfo.processInfo.processIdentifier
        // CG window bounds are top-left origin on the primary display
        let mainH = NSScreen.screens.first?.frame.height ?? 0
        let vis = lyraScreen().visibleFrame
        var out: [CGWindowID: CGRect] = [:]
        for w in list {
            guard let id = w[kCGWindowNumber as String] as? NSNumber,
                  let pid = (w[kCGWindowOwnerPID as String] as? NSNumber)?.int32Value,
                  pid != myPID,
                  (w[kCGWindowLayer as String] as? NSNumber)?.intValue == 0,
                  let b = w[kCGWindowBounds as String] as? [String: Any],
                  let bw = (b["Width"] as? NSNumber)?.doubleValue,
                  let bh = (b["Height"] as? NSNumber)?.doubleValue,
                  let bx = (b["X"] as? NSNumber)?.doubleValue,
                  let by = (b["Y"] as? NSNumber)?.doubleValue,
                  bw * bh > 140_000 // title bars worth sitting on
            else { continue }
            let r = CGRect(x: bx, y: mainH - by - bh, width: bw, height: bh)
            if r.intersects(vis) { out[CGWindowID(id.uint32Value)] = r }
        }
        perchRects = out
    }

    @objc private func screensChanged() {
        // roam() re-clamps every tick; just refresh perch geometry
        enumPerches()
    }

    /// The cast lives on Lyra's window's screen — never split (doc §5).
    private func lyraScreen() -> NSScreen {
        let w = NSApp.windows.first {
            $0.isVisible && $0.styleMask.contains(.titled) && !($0 is NSPanel)
        }
        return w?.screen ?? NSScreen.main ?? NSScreen.screens[0]
    }

    // ── pet-mode interactions (called from PetBodyView) ──────────────
    func beginDrag(_ b: CosmosScene.Body, at p: CGPoint) {
        world.beginDrag(b, at: p)
        ensureDrive() // dragging wakes a settled pet
    }
    func drag(to p: CGPoint) { world.drag(to: p) }
    func endDrag() { world.endDrag() }

    func showMenu(in view: NSView, event: NSEvent) {
        let m = NSMenu()
        m.addItem(withTitle: world.fsm == .sleep ? "Wake" : "Sleep",
                  action: #selector(menuSleep), keyEquivalent: "")
        m.addItem(withTitle: "Recall", action: #selector(menuRecall),
                  keyEquivalent: "")
        m.addItem(withTitle: "Hide", action: #selector(menuHide),
                  keyEquivalent: "")
        m.addItem(.separator())
        let mode = m.addItem(withTitle: "Interactive Mode",
                             action: #selector(menuMode), keyEquivalent: "")
        mode.state = interactive ? .on : .off
        for i in m.items { i.target = self }
        m.popUp(positioning: nil, at: event.locationInWindow, in: view)
    }

    @objc private func menuSleep() {
        if world.fsm == .sleep { world.wake() } else { world.sleep() }
        ensureDrive()
    }
    @objc private func menuRecall() { recall() }
    @objc private func menuHide() {
        userHidden = true // suppress respawn until policy change/summon
        teardown()
    }
    @objc private func menuMode() { interactive.toggle() }

    // ── panel construction (doc §1 recipe) + teardown ────────────────
    private func makePanels() {
        for (b, size) in DesktopPet.panelSize {
            let p = NSPanel(
                contentRect: CGRect(x: 0, y: 0, width: size, height: size),
                styleMask: [.borderless, .nonactivatingPanel],
                backing: .buffered, defer: false)
            p.isOpaque = false
            p.backgroundColor = .clear
            p.hasShadow = false
            p.level = .floating
            p.ignoresMouseEvents = !interactive
            p.collectionBehavior = [.canJoinAllSpaces, .stationary,
                                    .fullScreenAuxiliary]
            p.hidesOnDeactivate = false
            p.isReleasedWhenClosed = false // a stray close() nils it forever
            p.isMovable = false            // world-driven; drags go via the view
            let v = PetBodyView(body: b,
                                frame: CGRect(x: 0, y: 0, width: size, height: size))
            p.contentView = v
            panels[b] = p
            views[b] = v
        }
    }

    private func placePanel(_ p: NSPanel, center c: CGPoint) {
        // Lost-pet guard: keep ≥40pt of the panel reachable on the live
        // screen no matter what the world state says (stale perch rects,
        // a disconnected display's coords, pop/recall arcs all funnel
        // through here — this is the last line of defense).
        let v = (p.screen ?? lyraScreen()).visibleFrame
        let half = p.frame.width / 2
        let cx = min(max(c.x, v.minX - half + 40), v.maxX + half - 40)
        let cy = min(max(c.y, v.minY - half + 40), v.maxY + half - 40)
        p.setFrameOrigin(CGPoint(x: cx - half, y: cy - p.frame.height / 2))
    }

    /// Panels orderOut, `away` clears on the shared scene — the dock
    /// icon repopulates (the scene never stopped being the same planet).
    private func teardown() {
        for (_, p) in panels { p.orderOut(nil) }
        ViewModel.shared.viz.cosmos.away = []
        isOut = false
        phase = .hidden
        driveTimer?.invalidate()
        driveTimer = nil
    }

    private func bezier(_ p0: CGPoint, _ p1: CGPoint, _ p2: CGPoint,
                        _ t: CGFloat) -> CGPoint {
        let u = 1 - t
        return CGPoint(x: u * u * p0.x + 2 * u * t * p1.x + t * t * p2.x,
                       y: u * u * p0.y + 2 * u * t * p1.y + t * t * p2.y)
    }

    private func smooth(_ x: CGFloat) -> CGFloat {
        let t = min(max(x, 0), 1)
        return t * t * (3 - 2 * t)
    }
}

/// One cast member's panel content — paints via PetPaint, and in
/// Interactive mode the whole panel is the hit target: drag saturn to
/// move the system, double-click to recall, right-click for the menu.
private final class PetBodyView: NSView {
    let body: CosmosScene.Body
    /// Theme subscription — repaint on palette/appearance change even
    /// while the pet is asleep and its drive timer is stopped. Deferred
    /// to the run loop so `needsDisplay` doesn't fire mid-publisher.
    private var themeWatch: AnyCancellable?

    init(body: CosmosScene.Body, frame: NSRect) {
        self.body = body
        super.init(frame: frame)
        themeWatch = LyraTheme.shared.$palette
            .combineLatest(LyraTheme.shared.$appearance)
            .receive(on: RunLoop.main)
            .sink { [weak self] _ in self?.needsDisplay = true }
    }

    required init?(coder: NSCoder) { fatalError() }

    override func viewDidChangeEffectiveAppearance() {
        super.viewDidChangeEffectiveAppearance()
        // OS light/dark flip (or app override) — palette roles resolve
        // per appearance, so repaint with the new one.
        needsDisplay = true
    }

    override func draw(_ dirty: NSRect) {
        guard let ctx = NSGraphicsContext.current?.cgContext else { return }
        let pet = DesktopPet.shared
        let palette = PetPalette.resolve(LyraTheme.shared.palette,
                                         appearance: effectiveAppearance)
        PetPaint.draw(ctx, bounds, body, pet.sceneForPaint,
                      fx: pet.fx(for: body), palette: palette)
    }

    override func acceptsFirstMouse(for event: NSEvent?) -> Bool { true }

    override func mouseDown(with e: NSEvent) {
        if e.clickCount == 2 {
            DesktopPet.shared.summonOrRecall()
            return
        }
        DesktopPet.shared.beginDrag(body, at: NSEvent.mouseLocation)
    }

    override func mouseDragged(with e: NSEvent) {
        DesktopPet.shared.drag(to: NSEvent.mouseLocation)
    }

    override func mouseUp(with e: NSEvent) {
        DesktopPet.shared.endDrag()
    }

    override func rightMouseDown(with e: NSEvent) {
        DesktopPet.shared.showMenu(in: self, event: e)
    }
}
