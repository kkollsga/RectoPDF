import AppKit
import IOSurface
import QuartzCore

/// The view that holds page layers. It's flipped, so y runs down the document as it does in Rust's numbers. It draws nothing itself. What the pointer does here goes straight to `PageArea`, where QA goes too.
final class PageDocumentView: NSView {
    weak var pointer: PageArea?

    override var isFlipped: Bool { true }
    override var isOpaque: Bool { false }
    override var wantsUpdateLayer: Bool { true }
    override func updateLayer() {}
    override class var isCompatibleWithResponsiveScrolling: Bool { true }

    /// The I-beam goes by where the pointer is, so it's told of moves in the visible part. Nothing runs with no pointer moving.
    override func updateTrackingAreas() {
        super.updateTrackingAreas()
        for area in trackingAreas where area.owner === self { removeTrackingArea(area) }
        addTrackingArea(NSTrackingArea(rect: .zero, options: [.mouseMoved, .cursorUpdate, .mouseEnteredAndExited, .activeInKeyWindow, .inVisibleRect], owner: self, userInfo: nil))
    }

    override func mouseDown(with event: NSEvent) {
        // Control-click is a right click on a Mac, and this doesn't pass it up to anything that would know. `menu(for:)` makes this pane the one read first.
        if event.modifierFlags.contains(.control) {
            if let menu = menu(for: event) { NSMenu.popUpContextMenu(menu, with: event, for: self) }
            return
        }
        pointer?.pointerDown(atWindow: event.locationInWindow, clicks: event.clickCount, shift: event.modifierFlags.contains(.shift))
    }

    override func mouseDragged(with event: NSEvent) {
        pointer?.pointerDragged(atWindow: event.locationInWindow)
    }

    override func mouseUp(with event: NSEvent) {
        pointer?.pointerUp()
    }

    override func mouseMoved(with event: NSEvent) {
        pointer?.pointerMoved(atWindow: event.locationInWindow)
    }

    override func cursorUpdate(with event: NSEvent) {
        pointer?.pointerMoved(atWindow: event.locationInWindow)
    }

    override func mouseExited(with event: NSEvent) {
        pointer?.pointerLeft()
    }

    /// What a right-click offers, from Rust's list. Each goes by a standard selector, up the chain to whoever has it, and the system adds Services by itself once something answers for them.
    override func menu(for event: NSEvent) -> NSMenu? {
        // In a comparison, a right-click on a pane is about that pane: it's the one read from here, and the menu ticks its own layout and acts on it.
        pointer?.focus()
        return pointer?.contextMenu()
    }
}

/// The page area's scrolling. ⌘ with a wheel or two fingers zooms instead, and doesn't also scroll.
final class PageScrollView: NSScrollView {
    var commandScrolled: ((NSEvent) -> Void)?
    /// Plain wheels handed on to AppKit's own scrolling, for QA.
    private(set) var wheelsPassedOn = 0

    override func scrollWheel(with event: NSEvent) {
        if event.modifierFlags.contains(.command) {
            commandScrolled?(event)
        } else {
            wheelsPassedOn += 1
            super.scrollWheel(with: event)
        }
    }
}

/// Keeps a document smaller than the window in the middle.
final class CenteringClipView: NSClipView {
    override var isFlipped: Bool { true }

    // A page shorter than the window leaves room around it that's this view's. A click there is a click off the text.
    override func mouseDown(with event: NSEvent) {
        documentView?.mouseDown(with: event)
    }

    override func mouseDragged(with event: NSEvent) {
        documentView?.mouseDragged(with: event)
    }

    override func mouseUp(with event: NSEvent) {
        documentView?.mouseUp(with: event)
    }

    override func constrainBoundsRect(_ proposedBounds: NSRect) -> NSRect {
        var rect = super.constrainBoundsRect(proposedBounds)
        if let document = documentView {
            if rect.width > document.frame.width { rect.origin.x = (document.frame.width - rect.width) / 2 }
            if rect.height > document.frame.height { rect.origin.y = (document.frame.height - rect.height) / 2 }
        }
        return rect
    }
}

/// Page layers holding IOSurface tiles. All it does is apply what Rust says, in one transaction, and tell Rust where the view is. The surfaces are shown as they are, so there's no copy between the renderer and the compositor.
@MainActor final class PageArea: NSObject {
    let scrollView = PageScrollView()
    let documentView = PageDocumentView()
    private let core: AppCore
    private let isQuiet: Bool
    /// Which pane this is, left to right: 0 with one tab showing. Rust's page view for it goes by this.
    let pane: UInt32
    private var pageLayers: [UInt32: CALayer] = [:]
    private var tiles: [UInt64: (layer: CALayer, page: UInt32)] = [:]
    private var tilesOnPage: [UInt32: [UInt64]] = [:]
    private var spareLayers: [CALayer] = []
    private var live = false
    private var scrolledTo: UInt64 = 0
    private var applying = false
    private var viewportDirty = false
    private var closed = false
    private var fit: Double = 1
    /// Warm paper: a colour multiplied over each page by the compositor.
    private var tint: UInt32?
    private var tintLayers: [UInt32: CALayer] = [:]
    /// What's selected, one layer on a page that has any: a page's sublayer, so it scrolls and zooms with the page, and it's no tile's business. Under the tint, so warm paper warms it too.
    private var highlightLayers: [UInt32: CAShapeLayer] = [:]
    private var highlightRects: [UInt32: [HighlightRect]] = [:]
    private var highlightDark = false
    private var highlightLighten = false
    private var windowIsActive = true
    private(set) var cursorIsIBeam = false
    /// A drag that's gone past the view's edge, and where it is, for the timer that keeps it scrolling.
    private var dragInWindow: NSPoint?
    private var autoscrollTimer: Timer?
    private(set) var worstPointerMicros: UInt64 = 0
    private(set) var pointerSteps: UInt64 = 0

    private let delegateAutoscrollEvery: TimeInterval
    /// Told when the zoom or the page's fit has changed, for the zoom level in the top left. Asked on each viewport, and only called on a change.
    var zoomChanged: (() -> Void)?
    private var toldZoom: (Double, Double) = (0, 0)

    init(core: AppCore, limits: ReaderLimits, isQuiet: Bool, pane: UInt32 = 0) {
        self.core = core
        self.isQuiet = isQuiet
        self.pane = pane
        delegateAutoscrollEvery = limits.autoscrollEverySeconds
        super.init()
        let clip = CenteringClipView()
        scrollView.contentView = clip
        scrollView.documentView = documentView
        documentView.pointer = self
        // The page goes under the transparent title bar, with nothing above or over it.
        scrollView.automaticallyAdjustsContentInsets = false
        scrollView.commandScrolled = { [weak self] event in self?.commandScroll(event) }
        scrollView.hasVerticalScroller = true
        scrollView.hasHorizontalScroller = true
        // The system's own scrollers, in the system's own style: nothing here sets it. Thin ones that fade, or classic ones if the Mac is set to always show them or has a mouse that can't scroll by gesture, as in Pages.
        scrollView.autohidesScrollers = true
        scrollView.allowsMagnification = true
        scrollView.minMagnification = limits.minZoom
        scrollView.maxMagnification = limits.maxZoom
        scrollView.usesPredominantAxisScrolling = true
        scrollView.wantsLayer = true
        documentView.wantsLayer = true
        documentView.layerContentsRedrawPolicy = .never
        clip.postsBoundsChangedNotifications = true
        let center = NotificationCenter.default
        center.addObserver(self, selector: #selector(viewMoved), name: NSView.boundsDidChangeNotification, object: clip)
        center.addObserver(self, selector: #selector(pinchStarted), name: NSScrollView.willStartLiveMagnifyNotification, object: scrollView)
        center.addObserver(self, selector: #selector(pinchEnded), name: NSScrollView.didEndLiveMagnifyNotification, object: scrollView)
        center.addObserver(self, selector: #selector(systemColoursChanged), name: NSColor.systemColorsDidChangeNotification, object: nil)
    }

    /// The clip view moved. It's the reader's only while something of theirs is moving it: a wheel or two fingers, with their momentum, the scroller dragged, a key, a drag past the edge, or `scroll(by:)`. A layout, a resize or a document smaller than the view being centred moves it too, and that isn't the reader leading the other panes: found by restoring a comparison with a map in a pane, which took the reading pane and moved the others.
    @objc private func viewMoved() {
        sendViewport(byReader: !applying && (readerScrolling || PageArea.isReadersOwn(NSApp.currentEvent)))
    }

    /// Set while `scroll(by:)` moves the view, which is what a scroll does, and what QA and the drag past the edge call.
    private var readerScrolling = false

    /// Whether the event being handled now is the reader's hand on the view.
    private static func isReadersOwn(_ event: NSEvent?) -> Bool {
        guard let event else { return false }
        return [.scrollWheel, .leftMouseDragged, .leftMouseDown, .keyDown, .swipe].contains(event.type)
    }

    @objc private func pinchStarted() {
        setLive(true)
    }

    @objc private func pinchEnded() {
        setLive(false)
    }

    func setLive(_ live: Bool) {
        self.live = live
        sendViewport()
    }

    /// The tint Rust says goes over each page, if any. One layer a page on screen, and no tile or render changes: white becomes the colour, and black stays black.
    func setTint(_ rgb: UInt32?) {
        guard rgb != tint else { return }
        tint = rgb
        CATransaction.begin()
        CATransaction.setDisableActions(true)
        for (page, layer) in pageLayers { applyTint(to: layer, page: page) }
        CATransaction.commit()
    }

    private func applyTint(to page: CALayer, page number: UInt32) {
        guard let tint else {
            tintLayers.removeValue(forKey: number)?.removeFromSuperlayer()
            return
        }
        let layer = tintLayers[number] ?? {
            let made = CALayer()
            made.compositingFilter = "multiplyBlendMode"
            made.anchorPoint = .zero
            made.zPosition = 10
            page.addSublayer(made)
            tintLayers[number] = made
            return made
        }()
        layer.frame = page.bounds
        layer.backgroundColor = Theme.rgb(tint).cgColor
    }

    /// For a shot. `render(in:)` leaves compositing filters out, so the tint is left off and the same multiply is done to the picture after: same colour, same sum, in a different place.
    func tintRectsForShot(in view: NSView) -> (rgb: UInt32, rects: [NSRect])? {
        guard let tint else { return nil }
        // Only where the pages can show: a zoomed page's rectangle runs under the sidebar, which the compositor's own tint can't reach.
        let visible = scrollView.convert(scrollView.bounds, to: view)
        let rects = pageLayers.values.compactMap { page -> NSRect? in
            // The frame is where the page shows, turned or not.
            let clipped = documentView.convert(page.frame, to: view).intersection(visible)
            return clipped.isNull ? nil : clipped
        }
        return (tint, rects)
    }

    /// Whether any page showing is under a rectangle of a view, for QA: what sits in the top right corner must never be over the page.
    func pagesOverlap(_ rect: NSRect, in view: NSView) -> Bool {
        let visible = scrollView.convert(scrollView.bounds, to: view)
        return pageLayers.values.contains { page in
            let shown = documentView.convert(page.frame, to: view).intersection(visible)
            return !shown.isNull && shown.intersects(rect)
        }
    }

    /// Whether every page showing stays inside this pane, for QA: a page of one pane mustn't run under the next.
    func pagesInside() -> Bool {
        let visible = scrollView.bounds
        return pageLayers.values.allSatisfy { page in
            let shown = documentView.convert(page.frame, to: scrollView).intersection(visible)
            return shown.isNull || (shown.minX >= visible.minX - 0.5 && shown.maxX <= visible.maxX + 0.5)
        }
    }

    func hideTintForShot(_ hidden: Bool) {
        CATransaction.begin()
        CATransaction.setDisableActions(true)
        for layer in tintLayers.values { layer.isHidden = hidden }
        CATransaction.commit()
        CATransaction.flush()
    }

    /// The system's selection colour, as Rust says which goes over this paper: the page's, and not the window's. With the window not key, the quieter one, as in any Mac app.
    func setHighlightLook(darkColour: Bool, lighten: Bool, active: Bool) {
        guard darkColour != highlightDark || lighten != highlightLighten || active != windowIsActive else { return }
        highlightDark = darkColour
        highlightLighten = lighten
        windowIsActive = active
        CATransaction.begin()
        CATransaction.setDisableActions(true)
        for layer in highlightLayers.values { styleHighlight(layer) }
        CATransaction.commit()
    }

    /// The Mac's accent colour changed.
    @objc private func systemColoursChanged() {
        CATransaction.begin()
        CATransaction.setDisableActions(true)
        for layer in highlightLayers.values { styleHighlight(layer) }
        CATransaction.commit()
    }

    private func highlightColour() -> NSColor {
        let wanted = windowIsActive ? NSColor.selectedTextBackgroundColor : NSColor.unemphasizedSelectedTextBackgroundColor
        var colour = wanted
        (NSAppearance(named: highlightDark ? .darkAqua : .aqua) ?? NSAppearance.currentDrawing()).performAsCurrentDrawingAppearance {
            colour = wanted.usingColorSpace(.sRGB) ?? wanted
        }
        return colour
    }

    private func styleHighlight(_ layer: CAShapeLayer) {
        layer.fillColor = highlightColour().cgColor
        layer.compositingFilter = highlightLighten ? "lightenBlendMode" : "multiplyBlendMode"
    }

    private func applyHighlight(page: UInt32, rects: [HighlightRect]) {
        guard let parent = pageLayers[page], !rects.isEmpty else {
            highlightLayers.removeValue(forKey: page)?.removeFromSuperlayer()
            highlightRects.removeValue(forKey: page)
            return
        }
        let layer = highlightLayers[page] ?? {
            let made = CAShapeLayer()
            made.anchorPoint = .zero
            made.zPosition = 5
            styleHighlight(made)
            parent.addSublayer(made)
            highlightLayers[page] = made
            return made
        }()
        layer.frame = parent.bounds
        let path = CGMutablePath()
        for rect in rects { path.addRect(CGRect(x: rect.x, y: rect.y, width: rect.width, height: rect.height)) }
        layer.path = path
        highlightRects[page] = rects
    }

    /// For a shot: `render(in:)` leaves compositing filters out, so a highlight would hide the page. They come off, and the same sum is done on the picture, as for the tint.
    func highlightsForShot(in view: NSView) -> (rgb: UInt32, lighten: Bool, rects: [NSRect])? {
        guard !highlightRects.isEmpty, let colour = highlightColour().usingColorSpace(.sRGB) else { return nil }
        let channel = { (value: CGFloat) in UInt32((value * 255).rounded()) }
        let rgb = channel(colour.redComponent) << 16 | channel(colour.greenComponent) << 8 | channel(colour.blueComponent)
        let visible = scrollView.convert(scrollView.bounds, to: view)
        var rects: [NSRect] = []
        for (number, pieces) in highlightRects {
            guard let page = pageLayers[number] else { continue }
            for piece in pieces {
                let inDocument = page.convert(CGRect(x: piece.x, y: piece.y, width: piece.width, height: piece.height), to: documentView.layer)
                let clipped = documentView.convert(inDocument, to: view).intersection(visible)
                if !clipped.isNull { rects.append(clipped) }
            }
        }
        return (rgb, highlightLighten, rects)
    }

    func hideHighlightsForShot(_ hidden: Bool) {
        CATransaction.begin()
        CATransaction.setDisableActions(true)
        for layer in highlightLayers.values { layer.isHidden = hidden }
        CATransaction.commit()
        CATransaction.flush()
    }

    private func measured(_ work: () -> Void) {
        let started = CACurrentMediaTime()
        work()
        pointerSteps += 1
        worstPointerMicros = max(worstPointerMicros, UInt64((CACurrentMediaTime() - started) * 1_000_000))
    }

    /// What a press does, whoever made it: the mouse, or QA by the same window coordinates.
    func pointerDown(atWindow place: NSPoint, clicks: Int, shift: Bool) {
        measured {
            let point = documentView.convert(place, from: nil)
            dragInWindow = place
            apply(core.selectionPress(pane: pane, x: point.x, y: point.y, clicks: UInt32(max(clicks, 0)), shift: shift))
        }
    }

    func pointerDragged(atWindow place: NSPoint) {
        measured {
            dragInWindow = place
            let point = documentView.convert(place, from: nil)
            apply(core.selectionDrag(pane: pane, x: point.x, y: point.y))
            keepScrollingIfPastTheEdge()
        }
    }

    func pointerUp() {
        measured {
            dragInWindow = nil
            autoscrollTimer?.invalidate()
            autoscrollTimer = nil
            apply(core.selectionRelease(pane: pane))
        }
    }

    func pointerMoved(atWindow place: NSPoint) {
        measured {
            // Over the scroller or anything else above the page, it isn't the page's cursor to set.
            let inFrame = scrollView.superview?.convert(place, from: nil) ?? place
            let onPage = scrollView.hitTest(inFrame) === documentView || isQuiet
            let point = documentView.convert(place, from: nil)
            setCursor(iBeam: onPage && core.pointerIsOverText(pane: pane, x: point.x, y: point.y))
        }
    }

    func pointerLeft() {
        setCursor(iBeam: false)
    }

    private func setCursor(iBeam: Bool) {
        cursorIsIBeam = iBeam
        // A quiet run's pointer is nobody's, and the real one's left alone.
        guard !isQuiet else { return }
        (iBeam ? NSCursor.iBeam : NSCursor.arrow).set()
    }

    /// A drag past the view's edge goes on scrolling with no further events, while it's there. The timer lives only that long.
    private func keepScrollingIfPastTheEdge() {
        guard beyondTheEdge() != .zero else {
            autoscrollTimer?.invalidate()
            autoscrollTimer = nil
            return
        }
        guard autoscrollTimer == nil else { return }
        let timer = Timer(timeInterval: delegateAutoscrollEvery, repeats: true) { [weak self] _ in
            MainActor.assumeIsolated { self?.autoscrollStep() }
        }
        RunLoop.main.add(timer, forMode: .common)
        autoscrollTimer = timer
    }

    private func beyondTheEdge() -> NSPoint {
        guard let place = dragInWindow else { return .zero }
        let point = documentView.convert(place, from: nil)
        let view = scrollView.contentView.bounds
        let x = point.x < view.minX ? point.x - view.minX : point.x > view.maxX ? point.x - view.maxX : 0
        let y = point.y < view.minY ? point.y - view.minY : point.y > view.maxY ? point.y - view.maxY : 0
        return NSPoint(x: x, y: y)
    }

    private func autoscrollStep() {
        guard let place = dragInWindow else { return }
        let beyond = beyondTheEdge()
        guard beyond != .zero else { return keepScrollingIfPastTheEdge() }
        let clip = scrollView.contentView
        readerScrolling = true
        defer { readerScrolling = false }
        var origin = clip.bounds.origin
        origin.x += core.autoscrollStep(beyond: beyond.x)
        origin.y += core.autoscrollStep(beyond: beyond.y)
        clip.setBoundsOrigin(clip.constrainBoundsRect(NSRect(origin: origin, size: clip.bounds.size)).origin)
        scrollView.reflectScrolledClipView(clip)
        pointerDragged(atWindow: place)
    }

    /// A place on the first highlighted line or the middle of the page, for QA's check of what the mouse reaches.
    func documentPointOnText() -> NSPoint? {
        guard let page = pageLayers.keys.min(), let layer = pageLayers[page] else { return nil }
        return NSPoint(x: layer.frame.midX, y: layer.frame.midY)
    }

    /// Rust's list: Copy and Select All by their standard selectors, then the layouts, which go to the app's own action as the menu bar's items do.
    func contextMenu() -> NSMenu {
        let menu = NSMenu()
        for entry in core.contextMenu(pane: pane) {
            if entry.id.isEmpty {
                menu.addItem(.separator())
                continue
            }
            let isSystem = !entry.systemAction.isEmpty
            let item = menu.addItem(withTitle: entry.title, action: isSystem ? NSSelectorFromString(entry.systemAction) : #selector(AppDelegate.menuChosen(_:)), keyEquivalent: "")
            item.representedObject = entry.id
            item.state = entry.checked ? .on : .off
        }
        return menu
    }

    private var liveEnd: Timer?

    /// ⌘-scroll: zooms about the point under the pointer, which stays where it is. By Rust's sum and limits, with old tiles stretching until it stops, as in a pinch.
    private func commandScroll(_ event: NSEvent) {
        let points = event.hasPreciseScrollingDeltas ? event.scrollingDeltaY : event.scrollingDeltaY * 10
        let value = core.zoomAfterScroll(current: scrollView.magnification, points: points)
        let inWindow = event.window == nil ? (scrollView.window?.convertPoint(fromScreen: event.locationInWindow) ?? event.locationInWindow) : event.locationInWindow
        setLive(true)
        scrollView.setMagnification(value, centeredAt: documentView.convert(inWindow, from: nil))
        // A wheel says nothing when it stops, so a pause says it.
        liveEnd?.invalidate()
        let timer = Timer(timeInterval: 0.15, repeats: false) { [weak self] _ in
            MainActor.assumeIsolated {
                self?.liveEnd = nil
                self?.setLive(false)
            }
        }
        RunLoop.main.add(timer, forMode: .common)
        liveEnd = timer
    }

    /// What a press anywhere in this pane does first, before it reaches a page: this pane's the one being read.
    func focus() {
        core.paneFocused(pane: pane)
    }

    /// A zoom by ⌘-scroll waits a moment for the wheel to stop before its pages are drawn sharp: while it does, what's drawn is about to change.
    var zoomSettling: Bool { liveEnd != nil }

    /// The scroller stops below the agent icon, and goes the whole way with none there.
    func setScrollerTop(_ top: CGFloat) {
        guard scrollView.scrollerInsets.top != top else { return }
        scrollView.scrollerInsets = NSEdgeInsets(top: top, left: 0, bottom: 0, right: 0)
    }

    var fitForQA: Double { fit }

    /// What the pinch's notifications left, for QA.
    var isLive: Bool { live }

    func windowReady() {
        layoutChanged()
    }

    func windowClosing() {
        closed = true
        NotificationCenter.default.removeObserver(self)
        apply([.clear])
    }

    func layoutChanged() {
        sendViewport()
    }

    /// The room left either side of the pages, for what goes in the margin: beside the widest page there is, so what goes there is clear of every one. The same pages give the same room whatever order they're held in: this once took whichever page a dictionary gave first, which differs from run to run.
    func marginWidth() -> CGFloat {
        guard let widest = pageLayers.values.map(\.frame.width).max() else { return scrollView.bounds.width / 4 }
        return max((scrollView.bounds.width - widest * scrollView.magnification) / 2, 0)
    }

    /// Said when `marginWidth` has changed, so what's laid out in the margin follows it.
    var marginChanged: (() -> Void)?
    private var toldMargin: CGFloat = -1

    /// Half points are as fine as anything in the margin is placed.
    private func noteMarginIfChanged() {
        let now = (marginWidth() * 2).rounded() / 2
        guard now != toldMargin else { return }
        toldMargin = now
        marginChanged?()
    }

    /// How many device pixels a point is. Fixed at 2 in a quiet run, whatever display this Mac has.
    private var backing: Double {
        isQuiet ? 2 : Double(scrollView.window?.backingScaleFactor ?? 2)
    }

    func sendViewport(byReader: Bool = false) {
        guard !closed, scrollView.window != nil || isQuiet else { return }
        guard !applying else {
            viewportDirty = true
            return
        }
        let bounds = scrollView.contentView.bounds
        // What a fitted page has to fit in. A classic scroller, as with a mouse connected or "always show" set, takes its width from the view: without taking that off, a fitted page is wider than the view, and a scroll bar sits under it for good. Always taken off for that style, shown or not, so its coming and going can't change the fit and bring itself back.
        let scrollerRoom = scrollView.scrollerStyle == .legacy ? NSScroller.scrollerWidth(for: .regular, scrollerStyle: .legacy) : 0
        let frameWidth = scrollView.frame.width - scrollerRoom
        guard frameWidth > 0 else { return }
        let ops = core.setViewport(pane: pane, viewport: ViewportInput(x: bounds.minX, y: bounds.minY, width: bounds.width, height: bounds.height, frameWidth: frameWidth, magnification: scrollView.magnification, backing: backing, live: live, scrolledTo: scrolledTo, byReader: byReader))
        apply(ops)
        if toldZoom != (scrollView.magnification, fit) {
            toldZoom = (scrollView.magnification, fit)
            zoomChanged?()
        }
        noteMarginIfChanged()
    }

    /// There's more from Rust: tiles that have arrived, a change of tab. Called when it says so.
    func drain() {
        guard !closed else { return }
        apply(core.drainOps(pane: pane))
        noteMarginIfChanged()
    }

    private func apply(_ ops: [ReaderOp]) {
        guard !ops.isEmpty else { return }
        applying = true
        CATransaction.begin()
        CATransaction.setDisableActions(true)
        for op in ops { apply(op) }
        CATransaction.commit()
        applying = false
        // Moving the document or the view changes the viewport. It goes back once all of it is applied, and says which scroll it has reached. Before saying it's applied, or for a moment everything looks settled on the old view.
        if viewportDirty {
            viewportDirty = false
            sendViewport()
        }
        core.opsApplied(pane: pane)
    }

    private func apply(_ op: ReaderOp) {
        switch op {
        case .clear:
            for layer in pageLayers.values { layer.removeFromSuperlayer() }
            tintLayers.removeAll()
            highlightLayers.removeAll()
            highlightRects.removeAll()
            for tile in tiles.values { tile.layer.contents = nil }
            pageLayers.removeAll()
            tiles.removeAll()
            tilesOnPage.removeAll()
        case let .document(width, height):
            documentView.setFrameSize(NSSize(width: width, height: height))
            viewportDirty = true
        case let .magnify(value):
            if scrollView.magnification != value {
                // About the middle of the view, for a key, a menu or an agent, which have no pointer to go by.
                let clip = scrollView.contentView.bounds
                scrollView.setMagnification(value, centeredAt: NSPoint(x: clip.midX, y: clip.midY))
            }
            viewportDirty = true
        case let .scrollTo(y, seq):
            let clip = scrollView.contentView
            var origin = clip.bounds.origin
            origin.y = y
            clip.setBoundsOrigin(clip.constrainBoundsRect(NSRect(origin: origin, size: clip.bounds.size)).origin)
            scrollView.reflectScrolledClipView(clip)
            scrolledTo = seq
            viewportDirty = true
        case let .scrollAcross(x):
            let clip = scrollView.contentView
            var origin = clip.bounds.origin
            origin.x = x
            clip.setBoundsOrigin(clip.constrainBoundsRect(NSRect(origin: origin, size: clip.bounds.size)).origin)
            scrollView.reflectScrolledClipView(clip)
        case let .showPage(page, x, y, width, height, fit, paperRgb, turns):
            self.fit = fit
            let layer = pageLayers[page] ?? makePageLayer(page)
            // Its own coordinates are PDF points of the page before it's turned. The scale brings them to the document's, so a change of fit moves one transform and no tile, and a turn is the same transform turned about its top left, moved so the turned page's top left is where Rust put it. y runs down here, so a positive angle is clockwise.
            layer.bounds = CGRect(x: 0, y: 0, width: width, height: height)
            let (w, h) = (width * fit, height * fit)
            let offsets: [CGPoint] = [.zero, CGPoint(x: h, y: 0), CGPoint(x: w, y: h), CGPoint(x: 0, y: w)]
            let turn = Int(turns % 4)
            layer.position = CGPoint(x: x + offsets[turn].x, y: y + offsets[turn].y)
            layer.setAffineTransform(CGAffineTransform(rotationAngle: CGFloat(turn) * .pi / 2).scaledBy(x: fit, y: fit))
            layer.backgroundColor = Theme.rgb(paperRgb).cgColor
            applyTint(to: layer, page: page)
        case let .hidePage(page):
            pageLayers.removeValue(forKey: page)?.removeFromSuperlayer()
            tintLayers.removeValue(forKey: page)
            highlightLayers.removeValue(forKey: page)
            highlightRects.removeValue(forKey: page)
            for id in tilesOnPage.removeValue(forKey: page) ?? [] {
                if let tile = tiles.removeValue(forKey: id) { recycle(tile.layer) }
            }
        case let .attach(id, page, x, y, width, height, surface, rgb, cropWidth, cropHeight, below):
            guard let parent = pageLayers[page] else {
                if surface != 0 { Unmanaged<IOSurfaceRef>.fromOpaque(UnsafeRawPointer(bitPattern: UInt(surface))!).release() }
                return
            }
            let layer = spareLayers.popLast() ?? makeTileLayer()
            layer.frame = CGRect(x: x, y: y, width: width, height: height)
            if surface != 0 {
                // One reference, which Rust gave up to us, and now the layer holds what it needs.
                layer.contents = Unmanaged<IOSurfaceRef>.fromOpaque(UnsafeRawPointer(bitPattern: UInt(surface))!).takeRetainedValue()
                layer.contentsRect = CGRect(x: 0, y: 0, width: cropWidth, height: cropHeight)
                layer.backgroundColor = nil
            } else {
                layer.contents = nil
                layer.backgroundColor = rgb.map { Theme.rgb($0).cgColor }
            }
            if below { parent.insertSublayer(layer, at: 0) } else { parent.addSublayer(layer) }
            tiles[id] = (layer, page)
            tilesOnPage[page, default: []].append(id)
        case let .detach(id):
            guard let tile = tiles.removeValue(forKey: id) else { return }
            tilesOnPage[tile.page]?.removeAll { $0 == id }
            recycle(tile.layer)
        case let .highlight(page, rects):
            applyHighlight(page: page, rects: rects)
        }
    }

    private func makePageLayer(_ page: UInt32) -> CALayer {
        let layer = CALayer()
        layer.anchorPoint = .zero
        layer.isOpaque = true
        layer.masksToBounds = true
        documentView.layer?.addSublayer(layer)
        pageLayers[page] = layer
        return layer
    }

    private func makeTileLayer() -> CALayer {
        let layer = CALayer()
        layer.isOpaque = true
        layer.contentsGravity = .resize
        layer.anchorPoint = .zero
        // Neighbouring tiles must meet with no line between them.
        layer.edgeAntialiasingMask = []
        layer.magnificationFilter = .linear
        layer.minificationFilter = .linear
        return layer
    }

    /// A layer costs 2 to 6 KiB, so a few are kept and the rest let go.
    private func recycle(_ layer: CALayer) {
        layer.removeFromSuperlayer()
        layer.contents = nil
        layer.backgroundColor = nil
        if spareLayers.count < 64 { spareLayers.append(layer) }
    }

    func zoom(by factor: Double) {
        setZoom(scrollView.magnification * factor)
    }

    /// What the end of a pinch does, and the menu, and `recto qa zoom`.
    func setZoom(_ value: Double) {
        let clip = scrollView.contentView.bounds
        scrollView.setMagnification(value, centeredAt: NSPoint(x: clip.midX, y: clip.midY))
        setLive(false)
    }

    /// What a scroll does, and what `recto qa scroll` calls.
    func scroll(by points: Double) {
        readerScrolling = true
        defer { readerScrolling = false }
        let clip = scrollView.contentView
        var origin = clip.bounds.origin
        origin.y += points
        clip.setBoundsOrigin(clip.constrainBoundsRect(NSRect(origin: origin, size: clip.bounds.size)).origin)
        scrollView.reflectScrolledClipView(clip)
    }

    /// What QA reads about the layers themselves, as opposed to what Rust thinks it asked for.
    func report() -> [String: Any] {
        ["highlight_layers": highlightLayers.count, "highlight_rects": highlightRects.values.map(\.count).reduce(0, +), "cursor": cursorIsIBeam ? "ibeam" : "arrow", "autoscrolling": autoscrollTimer != nil, "pointer_steps": pointerSteps, "worst_pointer_step_us": worstPointerMicros, "document_width": documentView.frame.width, "view_width": scrollView.contentView.bounds.width, "horizontal_scroller": scrollView.scrollerStyle == .legacy && scrollView.horizontalScroller?.isHidden == false, "scroller_top_inset": scrollView.scrollerInsets.top, "scroller_style": scrollView.scrollerStyle == .legacy ? "legacy" : "overlay", "system_scroller_style": NSScroller.preferredScrollerStyle == .legacy ? "legacy" : "overlay", "shot_tint_left": tintRectsForShot(in: scrollView.window?.contentView?.superview ?? documentView)?.rects.map(\.minX).min() ?? 0, "page_layers": pageLayers.count, "page_turns": pageLayers.keys.sorted().map { page -> Int in let t = pageLayers[page]!.affineTransform(); return Int((atan2(t.b, t.a) / (.pi / 2)).rounded() + 4) % 4 }, "page_frames_landscape": pageLayers.values.filter { $0.frame.width > $0.frame.height }.count, "tint_layers": tintLayers.count, "tile_layers": tiles.count, "spare_layers": spareLayers.count, "document_height": documentView.frame.height, "scroll_y": scrollView.contentView.bounds.minY, "scroll_x": scrollView.contentView.bounds.minX, "magnification": scrollView.magnification]
    }
}
