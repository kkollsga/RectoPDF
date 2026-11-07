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
    var keyPressed: ((NSEvent) -> Bool)?
    override var isFlipped: Bool { true }
    override var acceptsFirstResponder: Bool { true }

    override func keyDown(with event: NSEvent) {
        if keyPressed?(event) != true { super.keyDown(with: event) }
    }

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

/// The sidebar's edge, to take hold of. Says where it's been dragged to, and Rust holds it to its limits.
final class SidebarGrip: NSView {
    var dragged: ((CGFloat) -> Void)?
    override var isFlipped: Bool { true }

    override func resetCursorRects() {
        addCursorRect(bounds, cursor: .resizeLeftRight)
    }

    override func mouseDragged(with event: NSEvent) {
        dragged?(event.locationInWindow.x)
    }

    override func mouseDown(with event: NSEvent) {}
}

/// The agent's place, in the empty margin at the top right: an AI four-point star, by the owner's word on 2026-11-07. A placeholder until the panel exists: it does nothing, and says so.
final class AgentIcon: NSView {
    override var isFlipped: Bool { true }

    override func draw(_ dirtyRect: NSRect) {
        let middle = NSPoint(x: bounds.midX, y: bounds.midY)
        let reach = min(bounds.width, bounds.height) / 2 - 1
        // How close to the middle each point's sides draw in: the less, the longer and finer the points.
        let waist = reach * 0.14
        let star = NSBezierPath()
        star.move(to: NSPoint(x: middle.x, y: middle.y - reach))
        for (x, y, cornerX, cornerY) in [(reach, 0.0, waist, -waist), (0.0, reach, waist, waist), (-reach, 0.0, -waist, waist), (0.0, -reach, -waist, -waist)] as [(CGFloat, CGFloat, CGFloat, CGFloat)] {
            let control = NSPoint(x: middle.x + cornerX, y: middle.y + cornerY)
            star.curve(to: NSPoint(x: middle.x + x, y: middle.y + y), controlPoint1: control, controlPoint2: control)
        }
        star.close()
        Theme.textQuiet.ns.setFill()
        star.fill()
    }
}

/// A small label in the margin: the page number in focus view, or what the reader needs to be told.
class MarginLabel: NSView {
    let label = TextLine()

    override init(frame: NSRect) {
        super.init(frame: frame)
        wantsLayer = true
        layer?.cornerRadius = 8
        addSubview(label)
    }

    @available(*, unavailable) required init?(coder: NSCoder) { fatalError() }

    /// Lays itself out at most this wide, with its bottom right at the point given.
    func place(text: String, maxWidth: CGFloat, bottomRight: NSPoint, flippedParent: Bool = true) {
        label.set(text, font: .systemFont(ofSize: 11.5), color: Theme.textBody.ns, wraps: true)
        let room = max(maxWidth - 20, 40)
        let size = label.size(width: room)
        let width = min(size.width, room)
        label.frame = NSRect(x: 10, y: 6, width: width, height: size.height)
        frame = NSRect(x: bottomRight.x - width - 20, y: bottomRight.y - size.height - 12, width: width + 20, height: size.height + 12)
        layer?.backgroundColor = Theme.pill.cg
    }
}

/// What a copy has to say, in the margin with the other labels. It's a label, with a button if there's something to choose.
final class CopyNotice: MarginLabel {
    let button = NSButton(title: "", target: nil, action: nil)

    override init(frame: NSRect) {
        super.init(frame: frame)
        button.isBordered = false
        button.isHidden = true
        addSubview(button)
    }

    @available(*, unavailable) required init?(coder: NSCoder) { fatalError() }

    func placeWithAction(text: String, action: String?, maxWidth: CGFloat, bottomRight: NSPoint) {
        place(text: text, maxWidth: maxWidth, bottomRight: bottomRight)
        guard let action else {
            button.isHidden = true
            return
        }
        button.isHidden = false
        button.attributedTitle = NSAttributedString(string: action, attributes: [.foregroundColor: Theme.accent.ns, .font: NSFont.systemFont(ofSize: 11.5, weight: .medium)])
        let size = button.attributedTitle.size()
        frame.size.height += size.height + 6
        frame.origin.y -= size.height + 6
        button.frame = NSRect(x: 10, y: frame.height - size.height - 6, width: size.width + 4, height: size.height + 2)
        frame.size.width = max(frame.width, size.width + 24)
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
        Theme.bookLeft.ns.setFill()
        left.fill()
        let right = NSBezierPath()
        right.move(to: NSPoint(x: 25.5, y: 8))
        right.line(to: NSPoint(x: 49, y: 2))
        right.line(to: NSPoint(x: 49, y: 42))
        right.line(to: NSPoint(x: 25.5, y: 48))
        right.close()
        Theme.accent.ns.setFill()
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
    let sidebarGrip = SidebarGrip()
    let agentIcon = AgentIcon()
    let noteLabel = MarginLabel()
    let placeLabel = MarginLabel()
    let copyNotice = CopyNotice()
    private var noticeFade: Timer?
    private var shownNotice: SelectionNotice?
    let home = FlippedView()
    private let homeTitle = TextLine()
    private let homeHint = TextLine()
    private let homeButton = NSButton(title: "", target: nil, action: nil)
    var homeButtonForQA: NSControl { homeButton }
    private let homeBook = BookMark()
    private(set) var reader: ReaderView
    private let placement: WindowPlacement
    /// In a quiet run, which never really goes full screen, what it's been told.
    private var quietFullScreen = false
    private var shown = false
    private var firstResponderWatch: NSKeyValueObservation?

    init(delegate: AppDelegate) {
        self.delegate = delegate
        let limits = delegate.limits
        let style: NSWindow.StyleMask = [.titled, .closable, .miniaturizable, .resizable, .fullSizeContentView]
        placement = delegate.core.windowPlacement(screens: NSScreen.screens.map(MainWindowController.info))
        let rect = NSRect(x: 0, y: 0, width: placement.saved ? placement.width : limits.windowWidth, height: placement.saved ? placement.height : limits.windowHeight)
        if delegate.isQuiet {
            // Off every screen, and never ordered in.
            window = QuietWindow(contentRect: rect.offsetBy(dx: -40_000, dy: -40_000), styleMask: style, backing: .buffered, defer: false)
        } else {
            window = NSWindow(contentRect: rect, styleMask: style, backing: .buffered, defer: false)
            window.center()
            if placement.saved { window.setFrame(NSRect(x: placement.x, y: placement.y, width: placement.width, height: placement.height), display: false) }
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
        sidebar = SidebarPane(core: delegate.core, strings: delegate.strings, isQuiet: delegate.isQuiet)
        reader = delegate.core.reader()
        super.init()
        agentIcon.isHidden = reader.agentIconHidden
        window.delegate = self
        window.contentView = root
        root.wantsLayer = true
        root.layerContentsRedrawPolicy = .onSetNeedsDisplay
        root.addSubview(pages.scrollView)
        root.addSubview(home)
        root.addSubview(sidebar.view)
        root.addSubview(sidebarEdge)
        root.addSubview(sidebarGrip)
        root.addSubview(agentIcon)
        root.addSubview(noteLabel)
        root.addSubview(placeLabel)
        root.addSubview(copyNotice)
        copyNotice.isHidden = true
        copyNotice.button.target = self
        copyNotice.button.action = #selector(copyAnyway)
        pages.setHighlightLook(darkColour: reader.highlightDarkColour, lighten: reader.highlightLighten, active: true)
        sidebarEdge.wantsLayer = true
        home.wantsLayer = true
        agentIcon.toolTip = delegate.strings.agentTooltip
        noteLabel.isHidden = true
        placeLabel.isHidden = true
        buildHome()
        sidebar.addPressed = { [weak delegate] in delegate?.openDocument(nil) }
        sidebar.performAction = { [weak delegate] id in delegate?.perform(id) }
        sidebar.revealHost = root
        sidebarGrip.dragged = { [weak delegate] x in delegate?.core.setSidebarWidth(width: x) }
        root.appearanceChanged = { [weak self] in self?.appearanceChanged() }
        root.fileDropped = { [weak delegate] urls in
            for url in urls { delegate?.core.openFile(path: url.path) }
        }
        root.registerForDraggedTypes([.fileURL])
        root.keyPressed = { [weak delegate] event in
            // Plain keys, with nothing held. With no text field having the focus they get here, and when there is one it has them first.
            guard let delegate, event.modifierFlags.intersection([.command, .control, .option]).isEmpty, let id = delegate.core.plainKeyAction(characters: event.charactersIgnoringModifiers ?? "") else { return false }
            delegate.perform(id)
            return true
        }
        // AppKit changes the scrollers' style itself when the system's does, with a mouse plugged in or the setting changed. What's fitted to the room they leave follows.
        NotificationCenter.default.addObserver(self, selector: #selector(scrollerStyleChanged), name: NSScroller.preferredScrollerStyleDidChangeNotification, object: nil)
        // In focus view the window's buttons come back with the pointer near them. One area in a fixed corner, and it costs nothing when nothing's there.
        let corner = NSTrackingArea(rect: NSRect(x: 0, y: 0, width: 100, height: 44), options: [.mouseEnteredAndExited, .activeInActiveApp], owner: self, userInfo: nil)
        root.addTrackingArea(corner)
        layout()
        appearanceChanged()
        if delegate.isQuiet { QAChannel.drawActive(window) }
    }

    /// The window's made and laid out. It isn't on the screen yet: the page is drawn behind it first, so what comes up is right from its first frame.
    func windowIsUp() {
        quietFullScreen = delegate.isQuiet && placement.fullScreen
        pages.windowReady()
        noteWindow()
        firstResponderWatch = window.observe(\.firstResponder, options: []) { [weak self] _, _ in
            MainActor.assumeIsolated { self?.firstResponderChanged() }
        }
    }

    /// Shows the window as soon as what's on it is right: Rust says.
    func showIfReady() {
        guard !shown, delegate.core.readyToShow() else { return }
        showNow()
    }

    /// Whatever's on it. In a quiet run, nothing is ever ordered in, and the rest is the same.
    func showNow() {
        guard !shown else { return }
        shown = true
        layout()
        if !delegate.isQuiet {
            window.makeKeyAndOrderFront(nil)
            // Back in front only if it was there: an app that was behind doesn't take the focus from anyone.
            if placement.activate { NSApp.activate() }
            if placement.fullScreen { window.toggleFullScreen(nil) }
        }
        applyFocus(placement.focusSidebar ? "sidebar" : delegate.core.focusView())
        delegate.core.windowShown(home: reader.home, tileLayers: UInt32(pages.report()["tile_layers"] as? Int ?? 0))
    }

    /// Gives the keyboard to what's named. There's no Find field yet, so that's only kept.
    func applyFocus(_ view: String) {
        switch view {
        case "sidebar": window.makeFirstResponder(sidebar.table)
        case "page": window.makeFirstResponder(root)
        default: break
        }
        delegate.core.focusChanged(view: view)
    }

    private func firstResponderChanged() {
        guard let responder = window.firstResponder else { return }
        if responder === sidebar.table || responder === sidebar.contentsTable {
            delegate.core.focusChanged(view: "sidebar")
        } else if responder === root || responder === window {
            delegate.core.focusChanged(view: "page")
        }
    }

    static func info(_ screen: NSScreen) -> ScreenInfo {
        let (frame, visible) = (screen.frame, screen.visibleFrame)
        let number = (screen.deviceDescription[NSDeviceDescriptionKey("NSScreenNumber")] as? NSNumber)?.uint64Value ?? 0
        return ScreenInfo(id: number, name: screen.localizedName, x: frame.minX, y: frame.minY, width: frame.width, height: frame.height, visibleX: visible.minX, visibleY: visible.minY, visibleWidth: visible.width, visibleHeight: visible.height)
    }

    var isFullScreen: Bool { delegate.isQuiet ? quietFullScreen : window.styleMask.contains(.fullScreen) }

    /// Tells Rust how the window is, for the record. Its frame in full screen isn't where leaving full screen takes it.
    func noteWindow() {
        let frame = window.frame
        let realFullScreen = window.styleMask.contains(.fullScreen)
        delegate.core.windowChanged(frame: realFullScreen ? nil : [frame.minX, frame.minY, frame.width, frame.height], screen: window.screen.map(MainWindowController.info), fullScreen: isFullScreen)
    }

    /// What going into full screen does, for QA: a quiet run's window stays where it is.
    func setQuietFullScreen(_ on: Bool) {
        quietFullScreen = on
        noteWindow()
    }

    private func buildHome() {
        let strings = delegate.strings
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
        // Thin scrollers that come with scrolling and fade, as in Pages, with nothing reserved for them, unless the Mac is set to always show them. With the setting on automatic and a mouse paired, AppKit puts classic ones back whatever a view was told, so it's said again here. QA can ask for either.
        let style = delegate.qa?.scrollerStyle ?? (UserDefaults.standard.string(forKey: "AppleShowScrollBars") == "Always" ? .legacy : .overlay)
        for view in [pages.scrollView, sidebar.scroll, sidebar.contentsScroll as NSScrollView] where view.scrollerStyle != style { view.scrollerStyle = style }
        sidebar.updateContentsScroller()
        let size = window.frame.size
        root.frame = NSRect(origin: .zero, size: size)
        let focus = reader.focus
        let sidebarWidth = CGFloat(reader.sidebarWidth)
        let side = focus ? 0 : sidebarWidth
        // In focus view nothing shows but the page, its number and the agent icon. The window's buttons come back with the pointer near them.
        setWindowButtons(shown: windowButtonsShow(focus: focus, pointerNear: pointerNearButtons))
        sidebar.view.isHidden = focus
        sidebarEdge.isHidden = focus
        sidebarGrip.isHidden = focus
        sidebar.view.frame = NSRect(x: 0, y: 0, width: sidebarWidth, height: size.height)
        sidebarEdge.frame = NSRect(x: sidebarWidth - 1, y: 0, width: 1, height: size.height)
        sidebarGrip.frame = NSRect(x: sidebarWidth - 3, y: 0, width: 7, height: size.height)
        window.invalidateCursorRects(for: sidebarGrip)
        let area = NSRect(x: side, y: 0, width: size.width - side, height: size.height)
        pages.scrollView.frame = area
        home.frame = area
        agentIcon.frame = NSRect(x: size.width - 40, y: 16, width: 20, height: 20)
        // Owner, 2026-11-07: "To save horizontal space the scrollbar can stop below the agent icon".
        pages.setScrollerTop(agentIcon.isHidden ? 0 : agentIcon.frame.maxY + 8)
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
            bottom.y = noteLabel.frame.minY - 8
        } else {
            noteLabel.isHidden = true
        }
        if let notice = reader.selectionNotice {
            copyNotice.isHidden = false
            copyNotice.placeWithAction(text: notice.text, action: notice.actionTitle, maxWidth: marginWidth - 24, bottomRight: bottom)
        } else {
            copyNotice.isHidden = true
        }
    }

    @objc private func copyAnyway() {
        delegate.perform("edit.copy_anyway")
    }

    /// A notice that goes by itself goes after a moment, with a timer that's there only while it does.
    private func fadeNoticeIfItSaysTo() {
        guard reader.selectionNotice != shownNotice else { return }
        shownNotice = reader.selectionNotice
        noticeFade?.invalidate()
        noticeFade = nil
        guard shownNotice?.fades == true else { return }
        let core = delegate.core
        let timer = Timer(timeInterval: delegate.limits.noticeSeconds, repeats: false) { [weak self] _ in
            MainActor.assumeIsolated { self?.noticeFade = nil }
            core.dismissSelectionNotice()
        }
        RunLoop.main.add(timer, forMode: .common)
        noticeFade = timer
    }

    private func layoutHome() {
        let middle = home.bounds.midX
        var y = max(home.bounds.midY - 130, 40)
        homeBook.frame = NSRect(x: middle - 24.5, y: y, width: 49, height: 50)
        y += 74
        let title = homeTitle.size()
        homeTitle.frame = NSRect(x: middle - title.width / 2, y: y, width: title.width, height: title.height)
        y += title.height + 14
        let hint = homeHint.size()
        homeHint.frame = NSRect(x: middle - hint.width / 2, y: y, width: hint.width, height: hint.height)
        y += hint.height + 18
        let width = homeButton.attributedTitle.size().width + 44
        homeButton.frame = NSRect(x: middle - width / 2, y: y, width: width, height: 30)
    }

    private var pointerNearButtons = false

    @objc private func scrollerStyleChanged() {
        sidebar.updateContentsScroller()
        layout()
    }

    // AppKit calls these by name on a tracking area's owner. This isn't a responder, so they say so, by that name.
    @objc(mouseEntered:) func mouseEntered(with event: NSEvent) { pointerNearButtons(true) }

    @objc(mouseExited:) func mouseExited(with event: NSEvent) { pointerNearButtons(false) }

    /// Where the window's buttons are, in a view, for QA.
    func windowButtonFrames(in view: NSView) -> [NSRect] {
        [NSWindow.ButtonType.closeButton, .miniaturizeButton, .zoomButton].compactMap { type in
            window.standardWindowButton(type).map { $0.convert($0.bounds, to: view) }
        }
    }

    /// Whether the window's buttons are showing, for QA.
    var windowButtonsShown: Bool { window.standardWindowButton(.closeButton)?.isHidden == false }

    private func setWindowButtons(shown: Bool) {
        for type in [NSWindow.ButtonType.closeButton, .miniaturizeButton, .zoomButton] {
            window.standardWindowButton(type)?.isHidden = !shown
        }
    }

    /// What the pointer reaching the window's top left corner does.
    func pointerNearButtons(_ near: Bool) {
        pointerNearButtons = near
        setWindowButtons(shown: windowButtonsShow(focus: reader.focus, pointerNear: near))
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
        pages.setTint(reader.pageTint)
        pages.setHighlightLook(darkColour: reader.highlightDarkColour, lighten: reader.highlightLighten, active: window.isKeyWindow || delegate.isQuiet)
        fadeNoticeIfItSaysTo()
        agentIcon.isHidden = reader.agentIconHidden
        layout()
    }

    func appearanceChanged() {
        Theme.dark = Theme.isDark(root)
        delegate.core.setDarkAppearance(dark: Theme.dark)
        root.layer?.backgroundColor = Theme.margin.cg
        pages.scrollView.backgroundColor = Theme.margin.ns
        home.layer?.backgroundColor = Theme.margin.cg
        sidebarEdge.layer?.backgroundColor = Theme.hairline.cg
        homeTitle.set(delegate.strings.appName, font: .boldSystemFont(ofSize: 28), color: Theme.textStrong.ns)
        homeHint.set(delegate.strings.homeHint, font: .systemFont(ofSize: 14), color: Theme.textQuiet.ns)
        homeButton.layer?.backgroundColor = Theme.buttonFill.cg
        sidebar.appearanceChanged()
        for view in [agentIcon, homeBook] { view.needsDisplay = true }
        for label in [noteLabel, placeLabel, copyNotice as MarginLabel] {
            label.layer?.backgroundColor = Theme.pill.cg
            if !label.label.text.isEmpty { label.label.set(label.label.text, font: .systemFont(ofSize: 11.5), color: Theme.textBody.ns, wraps: true) }
        }
    }

    func windowDidResize(_ notification: Notification) {
        layout()
        noteWindow()
    }

    func windowDidMove(_ notification: Notification) {
        noteWindow()
    }

    func windowDidEnterFullScreen(_ notification: Notification) {
        noteWindow()
    }

    func windowDidExitFullScreen(_ notification: Notification) {
        noteWindow()
    }


    func windowWillStartLiveResize(_ notification: Notification) {
        pages.setLive(true)
    }

    func windowDidEndLiveResize(_ notification: Notification) {
        pages.setLive(false)
    }

    func windowDidBecomeKey(_ notification: Notification) {
        if !delegate.isQuiet { delegate.core.frontmostChanged(app: NSApp.isActive, windowKey: true) }
        pages.setHighlightLook(darkColour: reader.highlightDarkColour, lighten: reader.highlightLighten, active: true)
    }

    func windowDidResignKey(_ notification: Notification) {
        if !delegate.isQuiet { delegate.core.frontmostChanged(app: NSApp.isActive, windowKey: false) }
        pages.setHighlightLook(darkColour: reader.highlightDarkColour, lighten: reader.highlightLighten, active: delegate.isQuiet)
    }

    func windowDidChangeBackingProperties(_ notification: Notification) {
        pages.layoutChanged()
    }

    func windowWillClose(_ notification: Notification) {
        firstResponderWatch = nil
        pages.windowClosing()
        delegate.windowDidClose()
    }
}
