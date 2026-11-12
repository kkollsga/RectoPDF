import AppKit
import recto_ffiFFI

/// The page area's scroll thumb across, for a page wider than its pane: the vertical `ThumbView`'s look (Rust's `ScrollThumbLook` and contrast, the same colours, width and hover) along the bottom edge, with no track. The page area has no system scroller at all: a fading one's knob animation kept each pane's scroller for good once the pane had gone (`comparisons_leave_no_views_behind`). Its geometry is the scroll view's, as the vertical one's is. It lies over the bottom margin, clear of the page number, which sits 16 points up.
final class HorizontalThumbView: NSView {
    private(set) weak var scrollView: NSScrollView?
    private var hovering = false
    private var dragging: (start: CGFloat, origin: CGFloat)?
    private var area: NSTrackingArea?
    /// Moves the view across to this x, as the reader's own scroll.
    var scrollTo: ((CGFloat) -> Void)?

    override var isFlipped: Bool { true }
    override var isOpaque: Bool { false }

    func attach(to scrollView: NSScrollView) {
        self.scrollView = scrollView
        scrollView.addSubview(self)
        scrollView.postsFrameChangedNotifications = true
        scrollView.contentView.postsBoundsChangedNotifications = true
        scrollView.contentView.postsFrameChangedNotifications = true
        scrollView.documentView?.postsFrameChangedNotifications = true
        var watched: [(NSNotification.Name, NSView)] = [(NSView.boundsDidChangeNotification, scrollView.contentView), (NSView.frameDidChangeNotification, scrollView.contentView), (NSView.frameDidChangeNotification, scrollView)]
        if let document = scrollView.documentView { watched.append((NSView.frameDidChangeNotification, document)) }
        for (name, object) in watched { NotificationCenter.default.addObserver(self, selector: #selector(moved), name: name, object: object) }
        NotificationCenter.default.addObserver(self, selector: #selector(moved), name: ThumbView.contrastChanged, object: nil)
        refresh()
    }

    @objc private func moved() { refresh() }

    /// The strip along the bottom edge, short of the vertical thumb's strip at the right.
    func refresh() {
        if let strip = wantedStrip(), frame != strip { frame = strip }
        isHidden = knob() == nil
        needsDisplay = true
    }

    private func wantedStrip() -> NSRect? {
        guard let scrollView else { return nil }
        let height = CGFloat(ThumbView.look?.hitWidth ?? 12)
        return NSRect(x: 0, y: scrollView.isFlipped ? scrollView.bounds.height - height : 0, width: max(scrollView.bounds.width - height, 0), height: height)
    }

    /// How much there is across, and how far the view is: the page's width against the pane's.
    private func extent() -> (total: CGFloat, visible: CGFloat)? {
        guard let scrollView, let document = scrollView.documentView else { return nil }
        return (document.frame.width, scrollView.contentView.bounds.width)
    }

    /// Where the thumb is, in this view, or nothing with the page no wider than its pane.
    func knob() -> NSRect? {
        guard let scrollView, let look = ThumbView.look, let (total, visible) = extent(), total > visible + 0.5, visible > 0, bounds.width > CGFloat(look.minLength) + 4 else { return nil }
        let thickness = CGFloat(hovering || dragging != nil ? look.hoverWidth : look.width)
        let room = bounds.width - 4
        let length = min(max(room * visible / total, CGFloat(look.minLength)), room)
        let along = min(max(scrollView.contentView.bounds.minX / (total - visible), 0), 1)
        return NSRect(x: 2 + (room - length) * along, y: bounds.maxY - CGFloat(look.inset) - thickness, width: length, height: thickness)
    }

    override func draw(_ dirtyRect: NSRect) {
        // The scroll view was resized since the strip was put along it: put it again, and draw then.
        guard let look = ThumbView.look, let place = knob(), wantedStrip() == frame else {
            if scrollView != nil { DispatchQueue.main.async { [weak self] in self?.refresh() } }
            return
        }
        Theme.rgb(Theme.dark ? look.darkRgb : look.lightRgb).withAlphaComponent(CGFloat(ThumbView.alpha(look, strong: hovering || dragging != nil))).setFill()
        NSBezierPath(roundedRect: place, xRadius: place.height / 2, yRadius: place.height / 2).fill()
    }

    override func updateTrackingAreas() {
        super.updateTrackingAreas()
        if area == nil {
            let made = NSTrackingArea(rect: .zero, options: [.mouseEnteredAndExited, .activeInActiveApp, .inVisibleRect], owner: self, userInfo: nil)
            addTrackingArea(made)
            area = made
        }
    }

    override func mouseEntered(with event: NSEvent) { hovering = true; needsDisplay = true }
    override func mouseExited(with event: NSEvent) { hovering = false; needsDisplay = true }
    override func hitTest(_ point: NSPoint) -> NSView? { isHidden ? nil : super.hitTest(point) }

    override func mouseDown(with event: NSEvent) {
        guard let scrollView, let place = knob() else { return }
        let point = convert(event.locationInWindow, from: nil)
        if place.insetBy(dx: 0, dy: -8).contains(point) {
            dragging = (point.x, scrollView.contentView.bounds.minX)
        } else {
            step(point.x < place.minX ? -1 : 1)
        }
        needsDisplay = true
    }

    override func mouseDragged(with event: NSEvent) {
        guard let dragging, let place = knob(), let (total, visible) = extent() else { return }
        let travel = bounds.width - 4 - place.width
        guard travel > 0 else { return }
        scroll(to: dragging.origin + (convert(event.locationInWindow, from: nil).x - dragging.start) / travel * (total - visible))
    }

    override func mouseUp(with event: NSEvent) {
        dragging = nil
        needsDisplay = true
    }

    func scroll(to x: CGFloat) { scrollTo?(x) }

    /// A page across, as a click beside the thumb: what VoiceOver's increment and decrement do too.
    @discardableResult
    func step(_ way: CGFloat) -> Bool {
        guard let scrollView, knob() != nil else { return false }
        scroll(to: scrollView.contentView.bounds.minX + way * scrollView.contentView.bounds.width * CGFloat(spoken()?.pageShare ?? 0.9))
        return true
    }

    /// How far across it is, 0 to 1.
    func fraction() -> Double {
        guard let scrollView, let (total, visible) = extent(), total > visible else { return 0 }
        return Double(min(max(scrollView.contentView.bounds.minX / (total - visible), 0), 1))
    }

    func spoken() -> ThumbAccess? { MainActor.assumeIsolated { ThumbView.access?("pages_across", fraction()) } }

    override func isAccessibilityElement() -> Bool { true }
    override func accessibilityRole() -> NSAccessibility.Role? { .scrollBar }
    override func accessibilityOrientation() -> NSAccessibilityOrientation { .horizontal }
    override func accessibilityLabel() -> String? { spoken()?.label }
    override func accessibilityValue() -> Any? { spoken().map { NSNumber(value: $0.value) } }
    override func accessibilityValueDescription() -> String? { spoken()?.valueText }
    override func accessibilityPerformIncrement() -> Bool { step(1) }
    override func accessibilityPerformDecrement() -> Bool { step(-1) }

    /// What QA reports of it.
    var report: [String: Any] {
        refresh()
        let place = knob()
        return ["shown": !isHidden && place != nil, "thumb_height": place?.height ?? 0, "thumb_length": place?.width ?? 0, "thumb_left": place?.minX ?? 0, "bottom_gap": place.map { bounds.maxY - $0.maxY } ?? 0, "fraction": fraction(), "strip": ["x": frame.minX, "y": frame.minY, "pane_height": scrollView?.bounds.height ?? 0, "pane_flipped": scrollView?.isFlipped ?? false, "width": frame.width, "height": frame.height],
                "accessibility": ["role": accessibilityRole()?.rawValue ?? "", "orientation": "horizontal", "label": spoken()?.label ?? ""] as [String: Any]]
    }
}
