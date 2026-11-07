import AppKit
import IOSurface
import QuartzCore

/// The view that holds page layers. It's flipped, so y runs down the document as it does in Rust's numbers. It draws nothing itself.
final class PageDocumentView: NSView {
    override var isFlipped: Bool { true }
    override var isOpaque: Bool { false }
    override var wantsUpdateLayer: Bool { true }
    override func updateLayer() {}
    override class var isCompatibleWithResponsiveScrolling: Bool { true }
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

    init(core: AppCore, limits: ReaderLimits, isQuiet: Bool) {
        self.core = core
        self.isQuiet = isQuiet
        super.init()
        let clip = CenteringClipView()
        scrollView.contentView = clip
        scrollView.documentView = documentView
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
    }

    @objc private func viewMoved() {
        sendViewport()
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
            let size = CGSize(width: page.bounds.width * fit, height: page.bounds.height * fit)
            let rect = NSRect(origin: page.position, size: size)
            let clipped = documentView.convert(rect, to: view).intersection(visible)
            return clipped.isNull ? nil : clipped
        }
        return (tint, rects)
    }

    func hideTintForShot(_ hidden: Bool) {
        CATransaction.begin()
        CATransaction.setDisableActions(true)
        for layer in tintLayers.values { layer.isHidden = hidden }
        CATransaction.commit()
        CATransaction.flush()
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

    /// The room left either side of a fitted page, for what goes in the margin.
    func marginWidth() -> CGFloat {
        guard let page = pageLayers.values.first else { return scrollView.bounds.width / 4 }
        let width = page.bounds.width * CGFloat(fit) * scrollView.magnification
        return max((scrollView.bounds.width - width) / 2, 0)
    }

    /// How many device pixels a point is. Fixed at 2 in a quiet run, whatever display this Mac has.
    private var backing: Double {
        isQuiet ? 2 : Double(scrollView.window?.backingScaleFactor ?? 2)
    }

    func sendViewport() {
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
        let ops = core.setViewport(viewport: ViewportInput(x: bounds.minX, y: bounds.minY, width: bounds.width, height: bounds.height, frameWidth: frameWidth, magnification: scrollView.magnification, backing: backing, live: live, scrolledTo: scrolledTo))
        apply(ops)
    }

    /// There's more from Rust: tiles that have arrived, a change of tab. Called when it says so.
    func drain() {
        guard !closed else { return }
        apply(core.drainOps())
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
        core.opsApplied()
    }

    private func apply(_ op: ReaderOp) {
        switch op {
        case .clear:
            for layer in pageLayers.values { layer.removeFromSuperlayer() }
            tintLayers.removeAll()
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
        case let .showPage(page, x, y, width, height, fit, paperRgb):
            self.fit = fit
            let layer = pageLayers[page] ?? makePageLayer(page)
            // Its own coordinates are PDF points. The scale brings them to the document's, so a change of fit moves one transform and no tile.
            layer.bounds = CGRect(x: 0, y: 0, width: width, height: height)
            layer.position = CGPoint(x: x, y: y)
            layer.setAffineTransform(CGAffineTransform(scaleX: fit, y: fit))
            layer.backgroundColor = Theme.rgb(paperRgb).cgColor
            applyTint(to: layer, page: page)
        case let .hidePage(page):
            pageLayers.removeValue(forKey: page)?.removeFromSuperlayer()
            tintLayers.removeValue(forKey: page)
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
        let clip = scrollView.contentView
        var origin = clip.bounds.origin
        origin.y += points
        clip.setBoundsOrigin(clip.constrainBoundsRect(NSRect(origin: origin, size: clip.bounds.size)).origin)
        scrollView.reflectScrolledClipView(clip)
    }

    /// What QA reads about the layers themselves, as opposed to what Rust thinks it asked for.
    func report() -> [String: Any] {
        ["document_width": documentView.frame.width, "view_width": scrollView.contentView.bounds.width, "horizontal_scroller": scrollView.scrollerStyle == .legacy && scrollView.horizontalScroller?.isHidden == false, "scroller_top_inset": scrollView.scrollerInsets.top, "scroller_style": scrollView.scrollerStyle == .legacy ? "legacy" : "overlay", "system_scroller_style": NSScroller.preferredScrollerStyle == .legacy ? "legacy" : "overlay", "shot_tint_left": tintRectsForShot(in: scrollView.window?.contentView?.superview ?? documentView)?.rects.map(\.minX).min() ?? 0, "page_layers": pageLayers.count, "tint_layers": tintLayers.count, "tile_layers": tiles.count, "spare_layers": spareLayers.count, "document_height": documentView.frame.height, "scroll_y": scrollView.contentView.bounds.minY, "scroll_x": scrollView.contentView.bounds.minX, "magnification": scrollView.magnification]
    }
}
