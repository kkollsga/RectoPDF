import AppKit

/// A window that draws as the key window without being it, and at 2x whatever the display, in a quiet run. It never reaches a screen.
final class QuietWindow: NSWindow {
    override var isKeyWindow: Bool { true }
    override var isMainWindow: Bool { true }
    override var canBecomeKey: Bool { false }
    override var canBecomeMain: Bool { false }
    override var backingScaleFactor: CGFloat { 2 }
    override func constrainFrameRect(_ frameRect: NSRect, to screen: NSScreen?) -> NSRect { frameRect }
}

/// The window's root, for appearance changes and dropped files.
final class RootView: NSView {
    var appearanceChanged: (() -> Void)?
    var fileDropped: (([URL]) -> Void)?
    override var isFlipped: Bool { true }

    override func viewDidChangeEffectiveAppearance() {
        super.viewDidChangeEffectiveAppearance()
        appearanceChanged?()
    }

    override func draggingEntered(_ sender: NSDraggingInfo) -> NSDragOperation {
        sender.draggingPasteboard.canReadObject(forClasses: [NSURL.self], options: [.urlReadingFileURLsOnly: true]) ? .copy : []
    }

    override func performDragOperation(_ sender: NSDraggingInfo) -> Bool {
        let urls = sender.draggingPasteboard.readObjects(forClasses: [NSURL.self], options: [.urlReadingFileURLsOnly: true]) as? [URL] ?? []
        fileDropped?(urls)
        return !urls.isEmpty
    }
}

/// A plain view with y running down, as everything in this window does.
class FlippedView: NSView {
    override var isFlipped: Bool { true }
}

/// The agent's place, in the empty margin at the top right. A placeholder until the panel exists: it does nothing, and says so.
final class AgentIcon: NSView {
    override var isFlipped: Bool { true }

    override func draw(_ dirtyRect: NSRect) {
        let bubble = NSBezierPath(roundedRect: bounds.insetBy(dx: 2.5, dy: 3.5).offsetBy(dx: 0, dy: -0.5), xRadius: 3.5, yRadius: 3.5)
        bubble.lineWidth = 1.4
        Theme.textQuiet.setStroke()
        bubble.stroke()
        let prompt = NSBezierPath()
        prompt.lineWidth = 1.4
        prompt.lineCapStyle = .round
        prompt.lineJoinStyle = .round
        prompt.move(to: NSPoint(x: 7, y: 7.5))
        prompt.line(to: NSPoint(x: 9.5, y: 10))
        prompt.line(to: NSPoint(x: 7, y: 12.5))
        prompt.move(to: NSPoint(x: 11, y: 12.5))
        prompt.line(to: NSPoint(x: 14, y: 12.5))
        prompt.stroke()
    }
}

/// A small label in the margin: the page number in focus view, or what the reader needs to be told.
final class MarginLabel: NSView {
    let label = NSTextField(wrappingLabelWithString: "")

    override init(frame: NSRect) {
        super.init(frame: frame)
        wantsLayer = true
        layer?.cornerRadius = 8
        label.font = .systemFont(ofSize: 11.5)
        label.textColor = Theme.textBody
        label.isSelectable = false
        label.maximumNumberOfLines = 6
        addSubview(label)
    }

    @available(*, unavailable) required init?(coder: NSCoder) { fatalError() }

    /// Lays itself out at most this wide, with its bottom right at the point given.
    func place(text: String, maxWidth: CGFloat, bottomRight: NSPoint, flippedParent: Bool = true) {
        label.stringValue = text
        let room = max(maxWidth - 20, 40)
        let size = label.sizeThatFits(NSSize(width: room, height: 200))
        let width = ceil(min(size.width, room))
        label.frame = NSRect(x: 10, y: 6, width: width, height: ceil(size.height))
        frame = NSRect(x: bottomRight.x - width - 20, y: bottomRight.y - ceil(size.height) - 12, width: width + 20, height: ceil(size.height) + 12)
        layer?.backgroundColor = Theme.cg(Theme.pill, in: self)
    }
}

/// The home state's book.
final class BookMark: NSView {
    override var isFlipped: Bool { true }

    override func draw(_ dirtyRect: NSRect) {
        let left = NSBezierPath()
        left.move(to: NSPoint(x: 0, y: 2))
        left.line(to: NSPoint(x: 23.5, y: 8))
        left.line(to: NSPoint(x: 23.5, y: 48))
        left.line(to: NSPoint(x: 0, y: 42))
        left.close()
        Theme.bookLeft.setFill()
        left.fill()
        let right = NSBezierPath()
        right.move(to: NSPoint(x: 25.5, y: 8))
        right.line(to: NSPoint(x: 49, y: 2))
        right.line(to: NSPoint(x: 49, y: 42))
        right.line(to: NSPoint(x: 25.5, y: 48))
        right.close()
        Theme.accent.setFill()
        right.fill()
        NSColor.white.setStroke()
        for (y, length) in [(15.0, 13.0), (23.0, 13.0), (31.0, 8.0)] as [(CGFloat, CGFloat)] {
            let line = NSBezierPath()
            line.lineWidth = 2
            line.lineCapStyle = .round
            line.move(to: NSPoint(x: 31, y: y + 2))
            line.line(to: NSPoint(x: 31 + length, y: y - length * 0.2 + 2))
            line.stroke()
        }
    }
}

/// The window, and everything in it. It draws what it's told and passes on what the user does.
@MainActor final class MainWindowController: NSObject, NSWindowDelegate {
    unowned let delegate: AppDelegate
    let window: NSWindow
    let root = RootView()
    let pages: PageArea
    let sidebar: SidebarPane
    let sidebarEdge = NSView()
    let agentIcon = AgentIcon()
    let noteLabel = MarginLabel()
    let placeLabel = MarginLabel()
    let home = FlippedView()
    private let homeTitle = NSTextField(labelWithString: "")
    private let homeHint = NSTextField(labelWithString: "")
    private let homeButton = NSButton(title: "", target: nil, action: nil)
    private let homeBook = BookMark()
    private(set) var reader: ReaderView

    init(delegate: AppDelegate) {
        self.delegate = delegate
        let limits = delegate.limits
        let style: NSWindow.StyleMask = [.titled, .closable, .miniaturizable, .resizable, .fullSizeContentView]
        let rect = NSRect(x: 0, y: 0, width: limits.windowWidth, height: limits.windowHeight)
        if delegate.isQuiet {
            // Off every screen, and never ordered in.
            window = QuietWindow(contentRect: rect.offsetBy(dx: -40_000, dy: -40_000), styleMask: style, backing: .buffered, defer: false)
        } else {
            window = NSWindow(contentRect: rect, styleMask: style, backing: .buffered, defer: false)
            window.center()
        }
        window.titleVisibility = .hidden
        window.titlebarAppearsTransparent = true
        window.isReleasedWhenClosed = false
        window.minSize = NSSize(width: 640, height: 420)
        // Pinned to sRGB: the display's own wide-gamut tables cost several megabytes. Platform report, §1.
        window.colorSpace = .sRGB
        if delegate.isQuiet { window.setFrameOrigin(NSPoint(x: -40_000, y: -40_000)) }
        window.animationBehavior = .none
        pages = PageArea(core: delegate.core, limits: limits, isQuiet: delegate.isQuiet)
        sidebar = SidebarPane(core: delegate.core, strings: delegate.strings)
        reader = delegate.core.reader()
        super.init()
        window.delegate = self
        window.contentView = root
        root.wantsLayer = true
        root.layerContentsRedrawPolicy = .onSetNeedsDisplay
        root.addSubview(pages.scrollView)
        root.addSubview(home)
        root.addSubview(sidebar.view)
        root.addSubview(sidebarEdge)
        root.addSubview(agentIcon)
        root.addSubview(noteLabel)
        root.addSubview(placeLabel)
        sidebarEdge.wantsLayer = true
        home.wantsLayer = true
        agentIcon.toolTip = delegate.strings.agentTooltip
        noteLabel.isHidden = true
        placeLabel.isHidden = true
        buildHome()
        sidebar.addPressed = { [weak delegate] in delegate?.openDocument(nil) }
        root.appearanceChanged = { [weak self] in self?.appearanceChanged() }
        root.fileDropped = { [weak delegate] urls in
            for url in urls { delegate?.core.openFile(path: url.path) }
        }
        root.registerForDraggedTypes([.fileURL])
        layout()
        appearanceChanged()
        if delegate.isQuiet { QAChannel.drawActive(window) }
    }

    /// In a quiet run, nothing is ever ordered in.
    func show() {
        pages.windowReady()
        if !delegate.isQuiet {
            window.makeKeyAndOrderFront(nil)
            NSApp.activate()
        }
    }

    private func buildHome() {
        let strings = delegate.strings
        homeTitle.stringValue = strings.appName
        homeTitle.font = .boldSystemFont(ofSize: 28)
        homeHint.stringValue = strings.homeHint
        homeHint.font = .systemFont(ofSize: 14)
        homeButton.title = strings.openButton
        homeButton.isBordered = false
        homeButton.wantsLayer = true
        homeButton.layer?.cornerRadius = 10
        homeButton.contentTintColor = .white
        homeButton.font = .boldSystemFont(ofSize: 14)
        homeButton.attributedTitle = NSAttributedString(string: strings.openButton, attributes: [.foregroundColor: NSColor.white, .font: NSFont.boldSystemFont(ofSize: 14)])
        homeButton.target = delegate
        homeButton.action = #selector(AppDelegate.openDocument(_:))
        for view in [homeBook, homeTitle, homeHint, homeButton] { home.addSubview(view) }
    }

    /// Whole pixels, one pass. Autoresizing would do the same work with more to go wrong.
    func layout() {
        let size = window.frame.size
        root.frame = NSRect(origin: .zero, size: size)
        let focus = reader.focus
        let side = focus ? 0 : Theme.sidebarWidth
        // In focus view nothing shows but the page, its number and the agent icon. The window's buttons come back with the pointer near them.
        setWindowButtons(shown: !focus || pointerNearButtons)
        sidebar.view.isHidden = focus
        sidebarEdge.isHidden = focus
        sidebar.view.frame = NSRect(x: 0, y: 0, width: Theme.sidebarWidth, height: size.height)
        sidebarEdge.frame = NSRect(x: Theme.sidebarWidth - 1, y: 0, width: 1, height: size.height)
        let area = NSRect(x: side, y: 0, width: size.width - side, height: size.height)
        pages.scrollView.frame = area
        home.frame = area
        agentIcon.frame = NSRect(x: size.width - 40, y: 16, width: 20, height: 20)
        layoutMargin(area: area)
        layoutHome()
        positionWindowButtons()
        pages.layoutChanged()
    }

    private func layoutMargin(area: NSRect) {
        let marginWidth = max(pages.marginWidth(), 140)
        var bottom = NSPoint(x: area.maxX - 16, y: area.maxY - 16)
        if reader.focus && !reader.home {
            placeLabel.isHidden = false
            placeLabel.place(text: reader.place, maxWidth: 160, bottomRight: bottom)
            bottom.y = placeLabel.frame.minY - 8
        } else {
            placeLabel.isHidden = true
        }
        if let note = reader.note {
            noteLabel.isHidden = false
            noteLabel.place(text: note, maxWidth: marginWidth - 24, bottomRight: bottom)
        } else {
            noteLabel.isHidden = true
        }
    }

    private func layoutHome() {
        let middle = home.bounds.midX
        var y = max(home.bounds.midY - 130, 40)
        homeBook.frame = NSRect(x: middle - 24.5, y: y, width: 49, height: 50)
        y += 74
        homeTitle.sizeToFit()
        homeTitle.frame.origin = NSPoint(x: middle - homeTitle.frame.width / 2, y: y)
        y += homeTitle.frame.height + 14
        homeHint.sizeToFit()
        homeHint.frame.origin = NSPoint(x: middle - homeHint.frame.width / 2, y: y)
        y += homeHint.frame.height + 18
        let width = homeButton.attributedTitle.size().width + 44
        homeButton.frame = NSRect(x: middle - width / 2, y: y, width: width, height: 30)
    }

    private var pointerNearButtons = false
    private var buttonsTracking: NSTrackingArea?

    private func setWindowButtons(shown: Bool) {
        for type in [NSWindow.ButtonType.closeButton, .miniaturizeButton, .zoomButton] {
            window.standardWindowButton(type)?.isHidden = !shown
        }
    }

    /// What the pointer reaching the window's top left corner does.
    func pointerNearButtons(_ near: Bool) {
        pointerNearButtons = near
        setWindowButtons(shown: !reader.focus || near)
    }

    /// Into the sidebar's top corner, where the concept has them, without a toolbar's strip across the page.
    private func positionWindowButtons() {
        let places: [(NSWindow.ButtonType, CGFloat)] = [(.closeButton, 18), (.miniaturizeButton, 38), (.zoomButton, 58)]
        for (type, x) in places {
            guard let button = window.standardWindowButton(type) else { continue }
            button.setFrameOrigin(NSPoint(x: x, y: button.superview.map { $0.bounds.height - button.frame.height - 12 } ?? 12))
        }
    }

    func refreshModel() {
        reader = delegate.core.reader()
        sidebar.update(delegate.core.sidebar())
        home.isHidden = !reader.home
        pages.scrollView.isHidden = reader.home
        layout()
    }

    func appearanceChanged() {
        delegate.core.setDarkAppearance(dark: Theme.isDark(root))
        root.layer?.backgroundColor = Theme.cg(Theme.margin, in: root)
        pages.scrollView.backgroundColor = Theme.margin
        home.layer?.backgroundColor = Theme.cg(Theme.margin, in: root)
        sidebarEdge.layer?.backgroundColor = Theme.cg(Theme.hairline, in: root)
        homeTitle.textColor = Theme.textStrong
        homeHint.textColor = Theme.textQuiet
        homeButton.layer?.backgroundColor = Theme.cg(Theme.buttonFill, in: root)
        sidebar.appearanceChanged()
        for view in [agentIcon, homeBook] { view.needsDisplay = true }
        for label in [noteLabel, placeLabel] {
            label.layer?.backgroundColor = Theme.cg(Theme.pill, in: root)
            label.label.textColor = Theme.textBody
        }
    }

    func windowDidResize(_ notification: Notification) {
        layout()
    }

    func windowWillStartLiveResize(_ notification: Notification) {
        pages.setLive(true)
    }

    func windowDidEndLiveResize(_ notification: Notification) {
        pages.setLive(false)
    }

    func windowDidChangeBackingProperties(_ notification: Notification) {
        pages.layoutChanged()
    }

    func windowWillClose(_ notification: Notification) {
        pages.windowClosing()
        delegate.windowDidClose()
    }
}
