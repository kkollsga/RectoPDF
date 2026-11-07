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
    let scrollView = NSScrollView()
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
        scrollView.hasVerticalScroller = true
        scrollView.hasHorizontalScroller = true
        scrollView.autohidesScrollers = true
        scrollView.scrollerStyle = .overlay
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
        let rects = pageLayers.values.map { page -> NSRect in
            let size = CGSize(width: page.bounds.width * fit, height: page.bounds.height * fit)
            let rect = NSRect(origin: page.position, size: size)
            return documentView.convert(rect, to: view)
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
        let frameWidth = scrollView.frame.width
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
            if scrollView.magnification != value { scrollView.magnification = value }
            viewportDirty = true
        case let .scrollTo(y, seq):
            let clip = scrollView.contentView
            var origin = clip.bounds.origin
            origin.y = y
            clip.setBoundsOrigin(clip.constrainBoundsRect(NSRect(origin: origin, size: clip.bounds.size)).origin)
            scrollView.reflectScrolledClipView(clip)
            scrolledTo = seq
            viewportDirty = true
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
        ["page_layers": pageLayers.count, "tint_layers": tintLayers.count, "tile_layers": tiles.count, "spare_layers": spareLayers.count, "document_height": documentView.frame.height, "scroll_y": scrollView.contentView.bounds.minY, "magnification": scrollView.magnification]
    }
}
