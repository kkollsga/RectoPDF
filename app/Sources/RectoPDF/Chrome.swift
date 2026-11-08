import AppKit

/// **The marks that come and go,** drawn: the zoom level, the page number, and the window's buttons with the sidebar's. What shows, and until when, is Rust's (`chrome.rs`). This tells it what happened, fades what it says, and arms one timer for when it says to look again. The pointer's coming and going over each corner is a tracking area's, and `recto qa corner-pointer` calls the same method.
@MainActor final class ChromeFader: NSObject {
    /// The views each mark is, set by the window.
    var zoomViews: () -> [NSView] = { [] }
    var placeViews: () -> [NSView] = { [] }
    var buttonViews: () -> [NSView] = { [] }
    /// Reduce Motion, the system's or QA's.
    var reduceMotion: () -> Bool = { NSWorkspace.shared.accessibilityDisplayShouldReduceMotion }
    private let core: AppCore
    private(set) var look: ChromeLook?
    private var timer: Timer?
    /// A fade under way, which `qa wait-settled` waits for.
    private(set) var fading = 0
    private var areas: [NSTrackingArea] = []
    /// Which corners the pointer's in, for QA.
    private(set) var pointerIn: Set<String> = []

    init(core: AppCore) {
        self.core = core
        super.init()
    }

    func changed(_ mark: ChromeMark) {
        apply(core.chromeChanged(mark: mark))
    }

    func held(_ mark: ChromeMark, _ held: Bool) {
        apply(core.chromeHeld(mark: mark, held: held))
    }

    /// What the pointer entering or leaving a corner does, and what `recto qa corner-pointer` calls.
    func pointer(_ mark: ChromeMark, inside: Bool) {
        if inside { pointerIn.insert(Self.name(mark)) } else { pointerIn.remove(Self.name(mark)) }
        apply(core.chromePointer(mark: mark, inside: inside))
    }

    /// Something else changed what shows, such as the sidebar being hidden.
    func refresh() {
        apply(core.chromeLook())
    }

    static func name(_ mark: ChromeMark) -> String {
        switch mark {
        case .zoom: "top-right"
        case .place: "lower-right"
        case .windowButtons: "top-left"
        }
    }

    static func mark(named name: String) -> ChromeMark? {
        [ChromeMark.zoom, .place, .windowButtons].first { Self.name($0) == name }
    }

    private func apply(_ look: ChromeLook) {
        let changed = look.zoomShown != self.look?.zoomShown || look.placeShown != self.look?.placeShown || look.windowButtonsShown != self.look?.windowButtonsShown
        self.look = look
        armTimer(look.nextChangeInMs)
        guard changed else { return }
        let targets: [(NSView, CGFloat)] = zoomViews().map { ($0, look.zoomShown ? 1 : 0) } + placeViews().map { ($0, look.placeShown ? 1 : 0) } + buttonViews().map { ($0, look.windowButtonsShown ? 1 : 0) }
        let moving = targets.filter { $0.0.alphaValue != $0.1 }
        guard !moving.isEmpty else { return }
        if reduceMotion() {
            for (view, alpha) in moving { view.alphaValue = alpha }
            return
        }
        fading += 1
        NSAnimationContext.runAnimationGroup({ context in
            context.duration = look.fadeSeconds
            for (view, alpha) in moving { view.animator().alphaValue = alpha }
        }, completionHandler: { [weak self] in
            MainActor.assumeIsolated { self?.fading -= 1 }
        })
    }

    /// One timer, for when Rust says something next changes by itself, and none while nothing will.
    private func armTimer(_ inMs: UInt64?) {
        timer?.invalidate()
        timer = nil
        guard let inMs else { return }
        let made = Timer(timeInterval: Double(inMs) / 1000 + 0.01, repeats: false) { [weak self] _ in
            MainActor.assumeIsolated {
                self?.timer = nil
                self?.refresh()
            }
        }
        made.tolerance = 0.05
        RunLoop.main.add(made, forMode: .common)
        timer = made
    }

    /// The corners the pointer is watched over, in the view's coordinates, made again when the window's laid out.
    func watch(_ corners: [(ChromeMark, NSRect)], in view: NSView) {
        for area in areas { view.removeTrackingArea(area) }
        areas = corners.filter { !$0.1.isEmpty }.map { mark, rect in
            let area = NSTrackingArea(rect: rect, options: [.mouseEnteredAndExited, .activeAlways], owner: self, userInfo: ["corner": Self.name(mark)])
            view.addTrackingArea(area)
            return area
        }
    }

    @objc func mouseEntered(with event: NSEvent) {
        if let mark = (event.trackingArea?.userInfo?["corner"] as? String).flatMap(Self.mark(named:)) { pointer(mark, inside: true) }
    }

    @objc func mouseExited(with event: NSEvent) {
        if let mark = (event.trackingArea?.userInfo?["corner"] as? String).flatMap(Self.mark(named:)) { pointer(mark, inside: false) }
    }

    /// What QA reads: what Rust says shows, and what's drawn.
    var report: [String: Any] {
        let alpha = { (views: [NSView]) in views.map { Double($0.alphaValue) }.max() ?? 0 }
        return ["zoom_shown": look?.zoomShown ?? false, "place_shown": look?.placeShown ?? false, "window_buttons_shown": look?.windowButtonsShown ?? true, "zoom_alpha": alpha(zoomViews()), "place_alpha": alpha(placeViews()), "window_buttons_alpha": alpha(buttonViews()), "next_change_in_ms": look?.nextChangeInMs.map { Int($0) } as Any? ?? NSNull(), "fading": fading > 0, "pointer_in": pointerIn.sorted()]
    }
}
