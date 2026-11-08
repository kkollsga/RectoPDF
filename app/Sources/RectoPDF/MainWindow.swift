import AppKit

/// The main window. Undo and Redo are the tab's edits, and NSWindow would take `undo:` and `redo:` itself, ahead of the app, and grey them out with an undo manager that has nothing in it. So the window answers them, by Rust's word, and a text field that has the keyboard still keeps its own.
class ReaderWindow: NSWindow {
    /// What an item does, and whether it can be chosen, by its id: the app's `perform` and Rust's `menuItemEnabled`.
    var edit: ((String) -> Void)?
    var editEnabled: ((String) -> Bool)?

    static func editId(_ action: Selector?) -> String? {
        switch action {
        case #selector(ReaderWindow.undo(_:)): "edit.undo"
        case #selector(ReaderWindow.redo(_:)): "edit.redo"
        default: nil
        }
    }

    @objc func undo(_ sender: Any?) {
        edit?("edit.undo")
    }

    @objc func redo(_ sender: Any?) {
        edit?("edit.redo")
    }

    override func validateUserInterfaceItem(_ item: NSValidatedUserInterfaceItem) -> Bool {
        if let id = Self.editId(item.action) { return editEnabled?(id) ?? false }
        return super.validateUserInterfaceItem(item)
    }
}

/// A window that draws as the key window without being it, and at 2x whatever the display, in a quiet run. It never reaches a screen.
final class QuietWindow: ReaderWindow {
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

/// A small label in the margin: the page number, or what the reader needs to be told.
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
    func place(text: String, maxWidth: CGFloat, bottomRight: NSPoint, flippedParent: Bool = true, font: NSFont = .systemFont(ofSize: 11.5), color: NSColor? = nil) {
        label.set(text, font: font, color: color ?? Theme.textBody.ns, wraps: true)
        let room = max(maxWidth - 20, 40)
        let size = label.size(width: room)
        let width = min(size.width, room)
        label.frame = NSRect(x: 10, y: 6, width: width, height: size.height)
        frame = NSRect(x: bottomRight.x - width - 20, y: bottomRight.y - size.height - 12, width: width + 20, height: size.height + 12)
        layer?.backgroundColor = Theme.pill.cg
    }

    /// Its frame in the window's root view and how many lines its words take, for QA.
    func qaBox(in root: NSView) -> [String: Any] {
        let frame = superview.map { $0.convert(self.frame, to: root) } ?? self.frame
        return ["x": frame.minX, "y": frame.minY, "width": frame.width, "height": frame.height, "lines": label.lineCount]
    }
}

/// The page number in the corner, which opens the go-to field when it's clicked.
final class ClickableLabel: MarginLabel {
    var clicked: (() -> Void)?

    override func mouseDown(with event: NSEvent) {
        clicked?()
    }

    override func resetCursorRects() {
        addCursorRect(bounds, cursor: .pointingHand)
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
@MainActor final class MainWindowController: NSObject, NSWindowDelegate, NSTextFieldDelegate {
    unowned let delegate: AppDelegate
    let window: NSWindow
    let root = RootView()
    /// A page area for each pane, left to right: one, or a comparison's. Owner, 2026-11-08: "Ctrl clicking more then one pdf should enable comparison view".
    private(set) var paneAreas: [PageArea]
    /// The first pane: the only one with one tab showing.
    var pages: PageArea { paneAreas[0] }
    /// The pane being read, as Rust says: where the zoom level, the pointer's selection and QA go.
    var readingPane: PageArea { paneAreas[min(Int(delegate.core.readingPane()), paneAreas.count - 1)] }
    /// Each pane's name, in its lower left corner, with more than one: see `layoutPanes`.
    private var paneLabels: [MarginLabel] = []
    /// What's wrong with a pane's file, in the pane, for a pane that isn't read: the one read says it with the page number.
    private var paneNotes: [MarginLabel] = []
    /// A hairline between panes, as between the sidebar and the page.
    private var paneEdges: [NSView] = []
    let sidebar: SidebarPane
    let sidebarEdge = NSView()
    let sidebarGrip = SidebarGrip()
    let agentIcon = AgentIcon()
    /// Beside the window's controls: shows and hides the sidebar. Owner, 2026-11-08: "simply have a sidebar icon to the right of the traffic light buttons that toggles sidebar on and of".
    let sidebarButton = NSButton(title: "", target: nil, action: nil)
    /// Save, beside the sidebar's button, while the document showing has unsaved edits (owner: "something that looks more like a native save / save as feature"). Rust says when, and its words.
    let saveButton = NSButton(title: "", target: nil, action: nil)
    /// The save problem showing as a sheet, so the same one isn't shown twice.
    private var problemShowing: SaveProblem?
    /// The zoom level, in the top right just left of the agent icon. Clicking it opens a field to type a level in, with a button to fit the page again. Owner, 2026-11-07: "lets show the zoom level in upper left and clicking the zoom value enables you to type in zoom level or reset". Its words are Rust's.
    let zoomButton = NSButton(title: "", target: nil, action: nil)
    let zoomField = NSTextField(string: "")
    let zoomReset = NSButton(title: "", target: nil, action: nil)
    private(set) var zoomEditing = false
    private var zoomLabel: ZoomLabel?
    let noteLabel = MarginLabel()
    /// The page number, always in the lower right. Clicked, it opens the go-to field, as the zoom level opens its own.
    let placeLabel = ClickableLabel()
    /// Where the link under the pointer goes, in Rust's words, in the lower left of its pane, as a browser shows it.
    let linkHint = MarginLabel()
    private var linkHintPane: PageArea?
    /// The go-to field: a page number, or what the document calls a page, such as iv. Rust reads it.
    let pageField = NSTextField(string: "")
    private(set) var pageEditing = false
    /// The sheet asking before a link's address opens, so the same one isn't shown twice.
    private var promptShowing: LinkPrompt?
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
    private var waitingToComeForward = false
    private var pendingFullScreen = false

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
            window = ReaderWindow(contentRect: rect, styleMask: style, backing: .buffered, defer: false)
            window.center()
            if placement.saved { window.setFrame(NSRect(x: placement.x, y: placement.y, width: placement.width, height: placement.height), display: false) }
        }
        window.titleVisibility = .hidden
        window.titlebarAppearsTransparent = true
        window.isReleasedWhenClosed = false
        window.minSize = NSSize(width: 640, height: 420)
        // A pointer moving over the app counts as someone being there, for a silent update. Comes in only while it moves.
        window.acceptsMouseMovedEvents = true
        // Pinned to sRGB: the display's own wide-gamut tables cost several megabytes. Platform report, §1.
        window.colorSpace = .sRGB
        if delegate.isQuiet { window.setFrameOrigin(NSPoint(x: -40_000, y: -40_000)) }
        window.animationBehavior = .none
        paneAreas = [PageArea(core: delegate.core, limits: limits, isQuiet: delegate.isQuiet)]
        sidebar = SidebarPane(core: delegate.core, strings: delegate.strings, isQuiet: delegate.isQuiet)
        reader = delegate.core.reader()
        super.init()
        agentIcon.isHidden = reader.agentIconHidden
        window.delegate = self
        if let reader = window as? ReaderWindow {
            reader.edit = { [weak delegate] id in delegate?.perform(id) }
            reader.editEnabled = { [weak delegate] id in delegate?.core.menuItemEnabled(id: id) ?? false }
        }
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
        root.addSubview(linkHint)
        root.addSubview(pageField)
        root.addSubview(copyNotice)
        root.addSubview(sidebarButton)
        root.addSubview(saveButton)
        for view in [zoomButton, zoomField, zoomReset] as [NSView] { root.addSubview(view) }
        buildZoomLevel()
        pages.zoomChanged = { [weak self] in self?.updateZoomLevel() }
        pages.marginChanged = { [weak self] in self?.relayoutMargin() }
        sidebarButton.isBordered = false
        sidebarButton.imagePosition = .imageOnly
        sidebarButton.image = NSImage(systemSymbolName: "sidebar.left", accessibilityDescription: delegate.strings.sidebarButtonTooltip)?.withSymbolConfiguration(.init(pointSize: 14, weight: .regular))
        sidebarButton.toolTip = delegate.strings.sidebarButtonTooltip
        sidebarButton.target = self
        sidebarButton.action = #selector(sidebarButtonPressed)
        saveButton.isBordered = false
        saveButton.imagePosition = .imageOnly
        saveButton.image = NSImage(systemSymbolName: "square.and.arrow.down", accessibilityDescription: reader.saveTooltip)?.withSymbolConfiguration(.init(pointSize: 14, weight: .regular))
        saveButton.target = self
        saveButton.action = #selector(saveButtonPressed)
        saveButton.isHidden = true
        copyNotice.isHidden = true
        copyNotice.button.target = self
        copyNotice.button.action = #selector(copyAnyway)
        pages.setHighlightLook(darkColour: reader.highlightDarkColour, lighten: reader.highlightLighten, active: true)
        sidebarEdge.wantsLayer = true
        home.wantsLayer = true
        agentIcon.toolTip = delegate.strings.agentTooltip
        noteLabel.isHidden = true
        placeLabel.isHidden = true
        linkHint.isHidden = true
        placeLabel.clicked = { [weak self] in self?.startPageEditing() }
        pageField.font = Self.zoomFont
        pageField.alignment = .center
        pageField.bezelStyle = .roundedBezel
        pageField.controlSize = .small
        pageField.target = self
        pageField.action = #selector(pageTyped)
        pageField.cell?.sendsActionOnEndEditing = false
        pageField.delegate = self
        pageField.isHidden = true
        pages.linkHintChanged = { [weak self, weak pages] hint in self?.showLinkHint(hint, in: pages) }
        buildHome()
        sidebar.addPressed = { [weak delegate] in delegate?.openDocument(nil) }
        sidebar.performAction = { [weak delegate] id in delegate?.perform(id) }
        // What a row run from the palette leaves the host to do: the Open panel, Settings, Find's field taking the keyboard, and the rest.
        sidebar.palette.hostAction = { [weak delegate] action in delegate?.handle(action) }
        sidebar.revealHost = root
        sidebarGrip.dragged = { [weak delegate] x in delegate?.core.setSidebarWidth(width: x) }
        root.appearanceChanged = { [weak self] in self?.appearanceChanged() }
        root.fileDropped = { [weak delegate] urls in
            for url in urls { delegate?.core.openFile(path: url.path) }
        }
        root.registerForDraggedTypes([.fileURL])
        syncPaneAreas()
        root.keyPressed = { [weak delegate] event in
            // Plain keys, with nothing held. With no text field having the focus they get here, and when there is one it has them first.
            guard let delegate, event.modifierFlags.intersection([.command, .control, .option]).isEmpty, let id = delegate.core.plainKeyAction(characters: event.charactersIgnoringModifiers ?? "") else { return false }
            delegate.perform(id)
            return true
        }
        // AppKit changes the scrollers' style itself when the system's does, with a mouse plugged in or the setting changed. What's fitted to the room they leave follows.
        NotificationCenter.default.addObserver(self, selector: #selector(scrollerStyleChanged), name: NSScroller.preferredScrollerStyleDidChangeNotification, object: nil)
        layout()
        appearanceChanged()
        if delegate.isQuiet { QAChannel.drawActive(window) }
    }

    /// The window's made and laid out. It isn't on the screen yet: the page is drawn behind it first, so what comes up is right from its first frame.
    func windowIsUp() {
        quietFullScreen = delegate.isQuiet && placement.fullScreen
        for area in paneAreas { area.windowReady() }
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
            if placement.inBackground {
                // An update arriving while the user's in something else. It's there to come to, behind whatever's in front, and isn't key, and full screen's another Space, so that waits until they come to it.
                waitingToComeForward = true
                pendingFullScreen = placement.fullScreenWhenActive
                window.orderBack(nil)
            } else {
                window.makeKeyAndOrderFront(nil)
                // Back in front only if it was there: an app that was behind doesn't take the focus from anyone.
                if placement.activate { NSApp.activate() }
                if placement.fullScreen { window.toggleFullScreen(nil) }
            }
        }
        applyFocus(placement.focusSidebar ? "sidebar" : delegate.core.focusView())
        delegate.core.windowShown(home: reader.home, tileLayers: UInt32(paneAreas.reduce(0) { $0 + ($1.report()["tile_layers"] as? Int ?? 0) }))
    }

    /// ⌘K: the palette's field takes the keyboard. Rust has opened it.
    func focusPalette() {
        sidebar.palette.wantsFocus = true
        sidebar.palette.refresh()
    }

    /// Gives the keyboard to what's named: the page, the sidebar, or Find's field if Find is open.
    func applyFocus(_ view: String) {
        switch view {
        case "sidebar": window.makeFirstResponder(sidebar.table)
        case "page": window.makeFirstResponder(root)
        case "find" where sidebar.find.model.open: sidebar.find.focusField()
        default: break
        }
        delegate.core.focusChanged(view: view)
    }

    /// ⌘F: Find's field takes the keyboard. Rust is opening Find, and showing the sidebar if it was hidden: the field takes it as soon as it's there, now if it already is.
    func focusFind() {
        sidebar.find.wantsFocus = true
        if sidebar.find.model.open { sidebar.find.focusField() }
    }

    private func firstResponderChanged() {
        guard let responder = window.firstResponder else { return }
        // Going somewhere else with the zoom level's field open leaves the level as it was, as Escape does.
        if zoomEditing, responder === root || responder === sidebar.table || responder === sidebar.contentsTable {
            endZoomEditing()
            return
        }
        if pageEditing, responder === root || responder === sidebar.table || responder === sidebar.contentsTable {
            endPageEditing()
            return
        }
        if responder === sidebar.table || responder === sidebar.contentsTable {
            delegate.core.focusChanged(view: "sidebar")
        } else if let editor = responder as? NSTextView, editor.delegate === sidebar.find.field {
            delegate.core.focusChanged(view: "find")
        } else if responder === root || responder === window {
            delegate.core.focusChanged(view: "page")
        }
    }

    static func info(_ screen: NSScreen) -> ScreenInfo {
        let (frame, visible) = (screen.frame, screen.visibleFrame)
        let number = (screen.deviceDescription[NSDeviceDescriptionKey("NSScreenNumber")] as? NSNumber)?.uint64Value ?? 0
        return ScreenInfo(id: number, name: screen.localizedName, x: frame.minX, y: frame.minY, width: frame.width, height: frame.height, visibleX: visible.minX, visibleY: visible.minY, visibleWidth: visible.width, visibleHeight: visible.height)
    }

    var isFullScreen: Bool { delegate.isQuiet ? quietFullScreen : window.styleMask.contains(.fullScreen) || pendingFullScreen }

    /// The user's come to an app that updated behind their back: now it can be what it was.
    func cameToTheFront() {
        guard waitingToComeForward else { return }
        waitingToComeForward = false
        window.makeKeyAndOrderFront(nil)
        if pendingFullScreen {
            pendingFullScreen = false
            window.toggleFullScreen(nil)
        }
    }

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
        for area in paneAreas where area.scrollView.scrollerStyle != style { area.scrollView.scrollerStyle = style }
        // The sidebar's lists always have the slim thumb over their margin, with no track, whatever the setting (owner, 2026-11-07: "the scroll thumb always visible, the rail always hidden"), so they're overlay whatever AppKit puts back.
        for view in [sidebar.scroll, sidebar.contentsScroll as NSScrollView] where view.scrollerStyle != .overlay { view.scrollerStyle = .overlay }
        sidebar.updateContentsScroller()
        let size = window.frame.size
        root.frame = NSRect(origin: .zero, size: size)
        let hidden = reader.sidebarHidden
        let sidebarWidth = CGFloat(reader.sidebarWidth)
        let side = hidden ? 0 : sidebarWidth
        sidebar.view.isHidden = hidden
        sidebarEdge.isHidden = hidden
        sidebarGrip.isHidden = hidden
        sidebar.view.frame = NSRect(x: 0, y: 0, width: sidebarWidth, height: size.height)
        sidebarEdge.frame = NSRect(x: sidebarWidth - 1, y: 0, width: 1, height: size.height)
        sidebarGrip.frame = NSRect(x: sidebarWidth - 3, y: 0, width: 7, height: size.height)
        window.invalidateCursorRects(for: sidebarGrip)
        let area = NSRect(x: side, y: 0, width: size.width - side, height: size.height)
        layoutPanes(in: area)
        home.frame = area
        // The same margin to the right as above, and less above than the 16 it had. Owner, 2026-11-07: "The margin left of the ai icon is too large. It should have the same margin to the right and above. Above should have slightly smaller margin then now (and right gets the same one)."
        agentIcon.frame = NSRect(x: size.width - Self.cornerMargin - 20, y: Self.cornerMargin, width: 20, height: 20)
        marginArea = area
        layoutMargin(area: area)
        layoutHome()
        positionWindowButtons()
        layoutTopLeft()
        layoutTopRight()
        for area in paneAreas { area.layoutChanged() }
    }

    /// **Panes side by side**, the same width, with a hairline between. Each one's name sits in its lower left corner, as the page number sits in the lower right, small and quiet: the owner's rule has nothing above or over a page, and a pill in the margin's corner is what the page number already is (owner, 2026-11-07: "The page number should always be visible in the lower right"), so it costs no reading height. The pane being read has its name in the accent colour. With one tab there's one pane and no name, as before.
    private func layoutPanes(in area: NSRect) {
        let count = CGFloat(paneAreas.count)
        let width = ((area.width - (count - 1)) / count).rounded(.down)
        let comparing = reader.panes.count > 1
        for (index, pane) in paneAreas.enumerated() {
            let x = area.minX + CGFloat(index) * (width + 1)
            let last = index == paneAreas.count - 1
            pane.scrollView.frame = NSRect(x: x, y: area.minY, width: last ? area.maxX - x : width, height: area.height)
            if index > 0, index - 1 < paneEdges.count { paneEdges[index - 1].frame = NSRect(x: x - 1, y: area.minY, width: 1, height: area.height) }
            guard index < paneLabels.count, index < paneNotes.count else { continue }
            let label = paneLabels[index]
            if comparing, index < reader.panes.count {
                let view = reader.panes[index]
                label.isHidden = false
                label.place(text: view.title, maxWidth: min(width - 32, 260), bottomRight: NSPoint(x: x + 16, y: area.maxY - 16), font: .systemFont(ofSize: 11.5, weight: view.reading ? .semibold : .regular), color: (view.reading ? Theme.accent : Theme.textBody).ns)
                // Laid out from its left: `place` puts its bottom right at the point, so it's moved by its width.
                label.frame.origin.x = x + 16
                let note = paneNotes[index]
                if let words = view.note, !view.reading {
                    note.isHidden = false
                    note.place(text: words, maxWidth: min(width - 32, 320), bottomRight: NSPoint(x: x + 16, y: label.frame.minY - 8))
                    note.frame.origin.x = x + 16
                } else {
                    note.isHidden = true
                }
            } else {
                label.isHidden = true
                paneNotes[index].isHidden = true
            }
        }
    }

    /// What QA reads about the panes: one, or a comparison's, each with its label and frame, and whether pages run under the pane beside them.
    func paneReport() -> [[String: Any]] {
        paneAreas.enumerated().map { index, area in
            let frame = area.scrollView.frame
            let label = index < paneLabels.count && !paneLabels[index].isHidden ? paneLabels[index] : nil
            let report = area.report()
            let note = index < paneNotes.count && !paneNotes[index].isHidden ? paneNotes[index].label.text : nil
            return ["pane": index, "note": note as Any? ?? NSNull(), "x": frame.minX, "width": frame.width, "label": label.map { $0.label.text as Any } ?? NSNull(), "label_in_lower_left": label.map { $0.frame.minX >= frame.minX && $0.frame.maxX <= frame.maxX && $0.frame.maxY > frame.maxY - 40 } ?? false, "reading": index == Int(delegate.core.readingPane()), "magnification": report["magnification"] ?? 0, "scroll_y": report["scroll_y"] ?? 0, "highlight_rects": report["highlight_rects"] ?? 0, "pages_inside": area.pagesInside()]
        }
    }

    /// **Every host animation running that changes what a shot shows**, by name: the sidebar's, and a ⌘-scroll zoom about to settle. Each ends by itself in a moment. A notice that goes after some seconds isn't one: it's what's shown, not a move. Nor is a drag's autoscroll, which runs for as long as the button's held.
    func runningAnimations() -> [String] {
        sidebar.runningAnimations() + (paneAreas.contains { $0.zoomSettling } ? ["zoom_settling"] : [])
    }

    /// Drains every pane, when Rust says there's something for the layers.
    func drainAll() {
        for area in paneAreas { area.drain() }
    }

    /// **As many page areas as Rust has panes,** made or let go of here, each drawing what its pane's page view says. A pane that goes takes its layers with it.
    private func syncPaneAreas() {
        let wanted = max(1, reader.panes.count)
        while paneAreas.count < wanted {
            let area = PageArea(core: delegate.core, limits: delegate.limits, isQuiet: delegate.isQuiet, pane: UInt32(paneAreas.count))
            area.zoomChanged = { [weak self] in self?.updateZoomLevel() }
            area.marginChanged = { [weak self] in self?.relayoutMargin() }
            area.linkHintChanged = { [weak self, weak area] hint in self?.showLinkHint(hint, in: area) }
            area.scrollView.backgroundColor = Theme.margin.ns
            root.addSubview(area.scrollView, positioned: .above, relativeTo: paneAreas.last?.scrollView)
            let edge = NSView()
            edge.wantsLayer = true
            edge.layer?.backgroundColor = Theme.hairline.cg
            root.addSubview(edge, positioned: .above, relativeTo: area.scrollView)
            paneEdges.append(edge)
            paneAreas.append(area)
            if window.isVisible || delegate.isQuiet { area.windowReady() }
        }
        while paneAreas.count > wanted, let gone = paneAreas.popLast() {
            gone.windowClosing()
            gone.scrollView.removeFromSuperview()
            paneEdges.popLast()?.removeFromSuperview()
        }
        while paneLabels.count < paneAreas.count {
            let label = MarginLabel()
            label.isHidden = true
            root.addSubview(label, positioned: .below, relativeTo: placeLabel)
            paneLabels.append(label)
        }
        while paneLabels.count > paneAreas.count { paneLabels.popLast()?.removeFromSuperview() }
        while paneNotes.count < paneAreas.count {
            let note = MarginLabel()
            note.isHidden = true
            root.addSubview(note, positioned: .below, relativeTo: placeLabel)
            paneNotes.append(note)
        }
        while paneNotes.count > paneAreas.count { paneNotes.popLast()?.removeFromSuperview() }
    }

    /// The pages' part of the window, as the last layout found it, for laying out the margin again by itself.
    private var marginArea: NSRect = .zero

    /// The margin has changed under the labels, by a zoom, a fit or other pages coming into view: they're laid out for the room there is now. Without this, a note kept the shape it had when it was last laid out, for whatever room there was then, until something unrelated, such as a change of appearance, laid it out again.
    private func relayoutMargin() {
        guard marginArea != .zero else { return }
        layoutMargin(area: marginArea)
    }

    private func layoutMargin(area: NSRect) {
        let marginWidth = max(paneAreas[paneAreas.count - 1].marginWidth(), 140)
        var bottom = NSPoint(x: area.maxX - 16, y: area.maxY - 16)
        // Owner, 2026-11-07: "The page number should always be visible in the lower right." In the margin's corner, quiet, over the page only when a page fills the width there.
        if !reader.home {
            placeLabel.isHidden = pageEditing
            placeLabel.place(text: reader.place, maxWidth: 160, bottomRight: bottom)
            window.invalidateCursorRects(for: placeLabel)
            let fieldWidth = max(placeLabel.frame.width, 72)
            pageField.frame = NSRect(x: placeLabel.frame.maxX - fieldWidth, y: placeLabel.frame.midY - 11, width: fieldWidth, height: 22).integral
            bottom.y = placeLabel.frame.minY - 8
        } else {
            placeLabel.isHidden = true
        }
        layoutLinkHint()
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

    /// Where the link under the pointer goes, shown in the lower left of the pane it's in, above a comparison's pane label. Gone off a link.
    private func showLinkHint(_ hint: String?, in pane: PageArea?) {
        linkHintPane = hint == nil ? nil : pane
        layoutLinkHint()
    }

    private func layoutLinkHint() {
        guard let pane = linkHintPane, let hint = pane.linkHint, !reader.home else {
            linkHint.isHidden = true
            return
        }
        let frame = pane.scrollView.frame
        let index = paneAreas.firstIndex { $0 === pane } ?? 0
        let above = index < paneLabels.count && !paneLabels[index].isHidden ? paneLabels[index].frame.minY - 8 : frame.maxY - 16
        linkHint.isHidden = false
        linkHint.place(text: hint, maxWidth: min(frame.width - 140, 520), bottomRight: NSPoint(x: frame.minX + 16, y: above))
        // Laid out from its left: `place` puts its bottom right at the point.
        linkHint.frame.origin.x = frame.minX + 16
    }

    /// The go-to field opens over the page number, with the page being read in it to type over.
    func startPageEditing() {
        guard !reader.home, window.attachedSheet == nil else { return }
        let field = delegate.core.pageField()
        pageEditing = true
        pageField.stringValue = field.editingText
        pageField.toolTip = field.tooltip
        placeLabel.isHidden = true
        pageField.isHidden = false
        window.makeFirstResponder(pageField)
        pageField.selectText(nil)
    }

    /// Return in the go-to field: Rust finds the page by its label or number, or says there's none by that.
    @objc private func pageTyped() {
        guard pageEditing else { return }
        _ = delegate.core.pageTyped(text: pageField.stringValue)
        endPageEditing()
    }

    func endPageEditing() {
        guard pageEditing else { return }
        pageEditing = false
        pageField.isHidden = true
        placeLabel.isHidden = reader.home
        if window.firstResponder === pageField.currentEditor() || window.firstResponder === pageField { window.makeFirstResponder(root) }
    }

    /// A click on a web or email link asks first, on the system's sheet, in Rust's words with the real address in full. Only Open hands the address to the system; a quiet run notes it and opens nothing.
    private func showLinkPromptIfAny() {
        guard let prompt = reader.linkPrompt else {
            promptShowing = nil
            return
        }
        guard promptShowing != prompt, window.attachedSheet == nil else { return }
        promptShowing = prompt
        let alert = NSAlert()
        alert.alertStyle = .informational
        alert.messageText = prompt.title
        alert.informativeText = prompt.message
        alert.addButton(withTitle: prompt.openTitle)
        alert.addButton(withTitle: prompt.cancelTitle)
        let core = delegate.core
        core.sheetChanged(open: true)
        delegate.sheetShowing = ["kind": "link_prompt", "message": prompt.title, "informative": prompt.message, "buttons": [prompt.openTitle, prompt.cancelTitle], "address": prompt.address]
        alert.beginSheetModal(for: window) { [weak self] response in
            core.sheetChanged(open: false)
            self?.delegate.sheetShowing = nil
            guard let address = core.linkPromptAnswered(id: prompt.id, open: response == .alertFirstButtonReturn) else { return }
            self?.delegate.openAddress(address)
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

    @objc private func scrollerStyleChanged() {
        sidebar.updateContentsScroller()
        layout()
    }

    /// Where the window's buttons are, in a view, for QA.
    func windowButtonFrames(in view: NSView) -> [NSRect] {
        [NSWindow.ButtonType.closeButton, .miniaturizeButton, .zoomButton].compactMap { type in
            window.standardWindowButton(type).map { $0.convert($0.bounds, to: view) }
        }
    }

    /// Whether the window's buttons are showing, for QA.
    var windowButtonsShown: Bool { window.standardWindowButton(.closeButton)?.isHidden == false }

    /// The window's own buttons' row: where they end, and the middle of the row, in the root's coordinates, which run down.
    private func windowButtonsRow() -> (right: CGFloat, middle: CGFloat) {
        let frames = windowButtonFrames(in: root)
        guard let last = frames.max(by: { $0.maxX < $1.maxX }) else { return (72, 19) }
        return (last.maxX, last.midY)
    }

    /// Right of the window's buttons, in the same row: the sidebar's button. Shown whether the sidebar is or not, as it's the way back.
    private func layoutTopLeft() {
        let row = windowButtonsRow()
        let size: CGFloat = 24
        sidebarButton.frame = NSRect(x: (row.right + 10).rounded(), y: (row.middle - size / 2).rounded(), width: size, height: size)
        saveButton.frame = NSRect(x: sidebarButton.frame.maxX + 4, y: sidebarButton.frame.minY, width: size, height: size)
    }

    /// The agent icon's margin from the window's top and right edges, the same both ways, by the owner's word of 2026-11-07.
    static let cornerMargin: CGFloat = 12
    /// Room either side of the zoom level's words inside its button.
    static let zoomPadding: CGFloat = 6

    /// The top right margin, where the agent icon is: the zoom level just left of it, in its row, or in its place when it's hidden, at the same margin from the edge. Owner, 2026-11-08: "The zoom level should be in the upper right of the app (not in the sidebar)". Its field opens leftwards, with Fit before it. The page's scroller stops below whatever's there.
    private func layoutTopRight() {
        let width = root.bounds.width
        let middle = agentIcon.frame.midY
        let right = agentIcon.isHidden ? width - Self.cornerMargin : agentIcon.frame.minX - 8
        let text = zoomButton.attributedTitle.size()
        // The words have this much room either side inside the button, so it's they that keep the margin.
        let padding = Self.zoomPadding
        let buttonWidth = (text.width + 2 * padding).rounded(.up)
        zoomButton.frame = NSRect(x: (right + padding - buttonWidth).rounded(), y: (middle - 11).rounded(), width: buttonWidth, height: 22)
        zoomField.frame = NSRect(x: (right - 54).rounded(), y: (middle - 11).rounded(), width: 54, height: 22)
        let reset = zoomReset.attributedTitle.size()
        let resetWidth = (reset.width + 10).rounded(.up)
        zoomReset.frame = NSRect(x: zoomField.frame.minX - 6 - resetWidth, y: (middle - 11).rounded(), width: resetWidth, height: 22)
        // Owner, 2026-11-07: "To save horizontal space the scrollbar can stop below the agent icon". And below the zoom level beside it, and the whole way with nothing there.
        let showing = [agentIcon, zoomButton, zoomField].filter { !$0.isHidden }
        // Only the pane at the right reaches the corner.
        for (index, area) in paneAreas.enumerated() {
            area.setScrollerTop(index == paneAreas.count - 1 ? showing.map(\.frame.maxY).max().map { $0 + 8 } ?? 0 : 0)
        }
    }

    @objc private func sidebarButtonPressed() {
        delegate.perform("view.sidebar")
    }

    @objc private func saveButtonPressed() {
        delegate.perform("save.button")
    }

    /// The Save icon: there while the document showing has unsaved edits, greyed while it's being saved, with Rust's words for what it's doing.
    private func updateSaveButton() {
        saveButton.isHidden = !reader.saveButton
        saveButton.isEnabled = reader.saveEnabled
        saveButton.toolTip = reader.saveTooltip
        saveButton.setAccessibilityLabel(reader.saveTooltip)
    }

    /// A save that didn't happen, as a sheet over the window, as long as Rust has it. Save As opens the save sheet, where a copy is the way on.
    private func showSaveProblemIfAny() {
        guard let problem = reader.saveProblem else {
            problemShowing = nil
            return
        }
        guard problemShowing != problem, window.attachedSheet == nil else { return }
        problemShowing = problem
        let alert = NSAlert()
        alert.alertStyle = .warning
        alert.messageText = problem.title
        alert.informativeText = problem.message
        alert.addButton(withTitle: problem.okTitle)
        if let saveAs = problem.saveAsTitle { alert.addButton(withTitle: saveAs) }
        let core = delegate.core
        core.sheetChanged(open: true)
        delegate.sheetShowing = ["kind": "save_problem", "message": problem.title, "informative": problem.message, "buttons": [problem.okTitle] + (problem.saveAsTitle.map { [$0] } ?? [])]
        alert.beginSheetModal(for: window) { [weak self] response in
            core.sheetChanged(open: false)
            self?.delegate.sheetShowing = nil
            let action = core.saveProblemAnswered(saveAs: response == .alertSecondButtonReturn)
            if action == .showSavePanel { self?.delegate.showSavePanel() }
        }
    }

    private static let zoomFont = NSFont.monospacedDigitSystemFont(ofSize: 12, weight: .regular)

    private func buildZoomLevel() {
        zoomButton.isBordered = false
        zoomButton.target = self
        zoomButton.action = #selector(zoomButtonPressed)
        zoomField.font = Self.zoomFont
        zoomField.alignment = .center
        zoomField.bezelStyle = .roundedBezel
        zoomField.controlSize = .small
        zoomField.target = self
        zoomField.action = #selector(zoomTyped)
        // Return applies a level, and nothing else does: leaving the field leaves it as it was.
        zoomField.cell?.sendsActionOnEndEditing = false
        zoomField.delegate = self
        zoomField.isHidden = true
        zoomReset.isBordered = false
        zoomReset.target = self
        zoomReset.action = #selector(zoomResetPressed)
        zoomReset.isHidden = true
    }

    /// The level as it stands, from the page's magnification and fit, in Rust's words. Not while it's being typed over.
    func updateZoomLevel() {
        // One zoom for every pane: the level's the pane being read's.
        let reading = readingPane
        let label = delegate.core.zoomLabel(magnification: reading.scrollView.magnification, fit: reading.fitForQA)
        let hidden = reader.home || zoomEditing
        if zoomButton.isHidden != hidden {
            zoomButton.isHidden = hidden
            layoutTopRight()
        }
        guard label != zoomLabel else { return }
        zoomLabel = label
        zoomButton.toolTip = label.tooltip
        zoomReset.toolTip = label.tooltip
        styleZoomLevel()
        layoutTopRight()
    }

    private func styleZoomLevel() {
        guard let label = zoomLabel else { return }
        zoomButton.attributedTitle = NSAttributedString(string: label.text, attributes: [.font: Self.zoomFont, .foregroundColor: Theme.textQuiet.ns])
        zoomReset.attributedTitle = NSAttributedString(string: label.resetTitle, attributes: [.font: Self.zoomFont, .foregroundColor: Theme.accent.ns])
    }

    @objc private func zoomButtonPressed() {
        guard !reader.home, let label = zoomLabel else { return }
        zoomEditing = true
        zoomField.stringValue = label.editingText
        zoomButton.isHidden = true
        zoomField.isHidden = false
        zoomReset.isHidden = false
        layoutTopRight()
        window.makeFirstResponder(zoomField)
        zoomField.selectText(nil)
    }

    /// Return in the field. Rust says whether it was a level, and if it wasn't, nothing changes.
    @objc private func zoomTyped() {
        guard zoomEditing else { return }
        _ = delegate.core.zoomTyped(text: zoomField.stringValue, decimalSeparator: Locale.current.decimalSeparator ?? ".")
        endZoomEditing()
    }

    @objc private func zoomResetPressed() {
        delegate.perform("view.fit")
        endZoomEditing()
    }

    func endZoomEditing() {
        guard zoomEditing else { return }
        zoomEditing = false
        zoomField.isHidden = true
        zoomReset.isHidden = true
        if window.firstResponder === zoomField.currentEditor() || window.firstResponder === zoomField { window.makeFirstResponder(root) }
        updateZoomLevel()
        layoutTopRight()
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
        syncPaneAreas()
        sidebar.update(delegate.core.sidebar())
        home.isHidden = !reader.home
        for area in paneAreas {
            area.scrollView.isHidden = reader.home
            area.setTint(reader.pageTint)
            area.setHighlightLook(darkColour: reader.highlightDarkColour, lighten: reader.highlightLighten, active: window.isKeyWindow || delegate.isQuiet)
        }
        for edge in paneEdges { edge.isHidden = reader.home }
        fadeNoticeIfItSaysTo()
        agentIcon.isHidden = reader.agentIconHidden
        // Unsaved edits: the red button's dot, as on any Mac app. Rust says.
        window.isDocumentEdited = reader.edited
        updateSaveButton()
        showSaveProblemIfAny()
        showLinkPromptIfAny()
        if reader.home { endPageEditing() }
        if reader.home { endZoomEditing() }
        updateZoomLevel()
        layout()
    }

    func appearanceChanged() {
        Theme.dark = Theme.isDark(root)
        delegate.core.setDarkAppearance(dark: Theme.dark)
        root.layer?.backgroundColor = Theme.margin.cg
        for area in paneAreas { area.scrollView.backgroundColor = Theme.margin.ns }
        for edge in paneEdges { edge.layer?.backgroundColor = Theme.hairline.cg }
        home.layer?.backgroundColor = Theme.margin.cg
        sidebarEdge.layer?.backgroundColor = Theme.hairline.cg
        homeTitle.set(delegate.strings.appName, font: .boldSystemFont(ofSize: 28), color: Theme.textStrong.ns)
        homeHint.set(delegate.strings.homeHint, font: .systemFont(ofSize: 14), color: Theme.textQuiet.ns)
        homeButton.layer?.backgroundColor = Theme.buttonFill.cg
        sidebar.appearanceChanged()
        for view in [agentIcon, homeBook] { view.needsDisplay = true }
        sidebarButton.contentTintColor = Theme.textQuiet.ns
        saveButton.contentTintColor = Theme.textQuiet.ns
        styleZoomLevel()
        // A pane's label takes its colour from whether it's read, when it's laid out.
        for label in paneLabels + paneNotes { label.layer?.backgroundColor = Theme.pill.cg }
        if paneLabels.contains(where: { !$0.isHidden }) { layout() }
        for label in [noteLabel, placeLabel, linkHint, copyNotice as MarginLabel] {
            label.layer?.backgroundColor = Theme.pill.cg
            if !label.label.text.isEmpty { label.label.set(label.label.text, font: .systemFont(ofSize: 11.5), color: Theme.textBody.ns, wraps: true) }
        }
    }

    /// Escape in the zoom level's field leaves it as it was.
    func control(_ control: NSControl, textView: NSTextView, doCommandBy selector: Selector) -> Bool {
        guard selector == #selector(NSResponder.cancelOperation(_:)) else { return false }
        if control === pageField {
            endPageEditing()
            return true
        }
        guard control === zoomField else { return false }
        endZoomEditing()
        return true
    }

    /// Going somewhere else with the field open, by Tab or a click, leaves the level as it was, as Escape does. A quiet run's window can't hold the keyboard, so there the field stays open until QA says what's typed.
    func controlTextDidEndEditing(_ notification: Notification) {
        let movement = (notification.userInfo?["NSTextMovement"] as? Int).flatMap(NSTextMovement.init(rawValue:))
        if (notification.object as? NSTextField) === pageField, pageEditing, window.canBecomeKey, movement != .return { endPageEditing() }
        guard (notification.object as? NSTextField) === zoomField, zoomEditing, window.canBecomeKey else { return }
        if movement != .return { endZoomEditing() }
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
        for area in paneAreas { area.setLive(true) }
    }

    func windowDidEndLiveResize(_ notification: Notification) {
        for area in paneAreas { area.setLive(false) }
    }

    func windowDidBecomeKey(_ notification: Notification) {
        if !delegate.isQuiet { delegate.core.frontmostChanged(app: NSApp.isActive, windowKey: true) }
        for area in paneAreas { area.setHighlightLook(darkColour: reader.highlightDarkColour, lighten: reader.highlightLighten, active: true) }
    }

    func windowDidResignKey(_ notification: Notification) {
        if !delegate.isQuiet { delegate.core.frontmostChanged(app: NSApp.isActive, windowKey: false) }
        for area in paneAreas { area.setHighlightLook(darkColour: reader.highlightDarkColour, lighten: reader.highlightLighten, active: delegate.isQuiet) }
    }

    func windowDidChangeBackingProperties(_ notification: Notification) {
        for area in paneAreas { area.layoutChanged() }
    }

    func windowWillClose(_ notification: Notification) {
        firstResponderWatch = nil
        for area in paneAreas { area.windowClosing() }
        delegate.windowDidClose()
    }
}
