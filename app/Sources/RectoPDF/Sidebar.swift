import AppKit
import QuartzCore

/// A row: only its name at rest. Its chevron and page number come with hover, and all it draws comes from Rust's row.
final class SidebarCell: NSView {
    let name = TextLine()
    let pageNumber = TextLine()
    let chevron = TextLine()
    let pill = CALayer()
    let railLine = CALayer()
    let railFill = CALayer()
    let railDot = CALayer()
    private var row: SidebarRow?
    private var folded = false
    private var hovered = false

    override var isFlipped: Bool { true }

    override init(frame: NSRect) {
        super.init(frame: frame)
        wantsLayer = true
        for part in [pill, railLine, railFill, railDot] {
            part.actions = ["position": NSNull(), "bounds": NSNull(), "hidden": NSNull(), "backgroundColor": NSNull()]
            layer?.addSublayer(part)
        }
        pill.cornerRadius = 7
        railDot.cornerRadius = 3.5
        for label in [name, pageNumber, chevron] { addSubview(label) }
    }

    @available(*, unavailable) required init?(coder: NSCoder) { fatalError() }

    func show(row: SidebarRow, foldedActive: Bool, hovered: Bool) {
        self.row = row
        self.folded = foldedActive
        self.hovered = hovered
        needsLayout = true
        applyLook()
    }

    /// What it's showing, so QA can hold it to the model.
    var shownRow: SidebarRow? { row }

    func applyLook() {
        guard let row else { return }
        let isTab = row.kind == .tab
        let active = isTab && row.current
        pill.isHidden = !active
        pill.backgroundColor = Theme.rowActive.cg
        let font: NSFont = active ? .systemFont(ofSize: 14, weight: .semibold) : row.current ? .systemFont(ofSize: 13.5, weight: .semibold) : .systemFont(ofSize: isTab ? 14 : 13.5)
        let color = active ? Theme.textStrong : row.current ? Theme.accent : row.railFilled && !isTab ? Theme.textBody : Theme.textQuiet
        name.set(row.text, font: font, color: color.ns)
        pageNumber.set(row.page.map(String.init) ?? "", font: .monospacedDigitSystemFont(ofSize: 11.5, weight: .regular), color: (row.current ? Theme.accent : Theme.textQuiet).ns, alignment: .right)
        pageNumber.isHidden = isTab || !hovered || row.page == nil
        chevron.set(folded ? "›" : "⌄", font: .systemFont(ofSize: 14, weight: .medium), color: Theme.textQuiet.ns, alignment: .center)
        chevron.isHidden = !(active && hovered)
        let isContents = row.kind == .contents
        railLine.isHidden = !isContents
        railFill.isHidden = !(isContents && row.railFilled)
        railDot.isHidden = !(isContents && row.current)
        railLine.backgroundColor = Theme.railRest.cg
        railFill.backgroundColor = Theme.accent.cg
        railDot.backgroundColor = Theme.accent.cg
    }

    /// Its name doesn't fit where it's drawn.
    var isCutOff: Bool {
        name.size().width > name.frame.width + 0.5
    }

    /// The name's place, in the view given.
    func namePlace(in view: NSView) -> NSRect {
        name.convert(name.bounds, to: view)
    }

    var nameFont: NSFont { row?.kind == .tab && row?.current == true ? .systemFont(ofSize: 14, weight: .semibold) : row?.current == true ? .systemFont(ofSize: 13.5, weight: .semibold) : .systemFont(ofSize: row?.kind == .tab ? 14 : 13.5) }

    override func layout() {
        super.layout()
        guard let row else { return }
        let (width, height) = (bounds.width, bounds.height)
        let isTab = row.kind == .tab
        pill.frame = NSRect(x: 8, y: 0, width: width - 16, height: height)
        let railX: CGFloat = 25
        // The filled part stops at the dot, on the row being read.
        railLine.frame = NSRect(x: railX - 0.75, y: row.railFirst ? 4 : 0, width: 1.5, height: height - (row.railFirst ? 4 : 0) - (row.railLast ? 4 : 0))
        railFill.frame = NSRect(x: railX - 0.75, y: row.railFirst ? 4 : 0, width: 1.5, height: row.current ? height / 2 - (row.railFirst ? 4 : 0) : height - (row.railFirst ? 4 : 0))
        railDot.frame = NSRect(x: railX - 3.5, y: height / 2 - 3.5, width: 7, height: 7)
        let indent = isTab ? 18 : 40 + CGFloat(min(Int(row.depth), 5)) * 14
        // Names get the whole row at rest. Page numbers and the chevron make room when they show.
        let right = width - (hovered ? 44 : isTab ? 18 : 14)
        let text = name.size()
        name.frame = NSRect(x: indent, y: floor((height - text.height) / 2), width: max(min(text.width, right - indent), 20), height: text.height)
        let digits = pageNumber.size()
        pageNumber.frame = NSRect(x: width - 58, y: floor((height - digits.height) / 2), width: 40, height: digits.height)
        let mark = chevron.size()
        chevron.frame = NSRect(x: width - 42, y: floor((height - mark.height) / 2), width: 24, height: mark.height)
    }
}

/// The contents' scrolling area. A wheel or a scroller moving it is the reader looking somewhere.
final class ContentsScrollView: NSScrollView {
    var userScrolled: (() -> Void)?
    var pointerMoved: ((Bool) -> Void)?
    private var pointerArea: NSTrackingArea?

    override func scrollWheel(with event: NSEvent) {
        super.scrollWheel(with: event)
        userScrolled?()
    }

    override func updateTrackingAreas() {
        super.updateTrackingAreas()
        if pointerArea == nil {
            let area = NSTrackingArea(rect: .zero, options: [.mouseEnteredAndExited, .activeInActiveApp, .inVisibleRect], owner: self, userInfo: nil)
            addTrackingArea(area)
            pointerArea = area
        }
    }

    override func mouseEntered(with event: NSEvent) { pointerMoved?(true) }
    override func mouseExited(with event: NSEvent) { pointerMoved?(false) }
}

/// Runs a closure on each frame while something's moving, and nothing at all otherwise. On a screen it's the display's own beat, and off one, where a display link doesn't fire, a timer.
@MainActor final class FrameRunner: NSObject {
    private weak var view: NSView?
    private let onScreen: Bool
    private let tick: (Double) -> Bool
    private var link: CADisplayLink?
    private var timer: Timer?
    private var last = 0.0

    /// `tick` is given the seconds since the last frame, and says whether to go on.
    init(view: NSView, onScreen: Bool, tick: @escaping (Double) -> Bool) {
        self.view = view
        self.onScreen = onScreen
        self.tick = tick
    }

    var isRunning: Bool { link != nil || timer != nil }

    func start() {
        guard !isRunning else { return }
        last = CACurrentMediaTime()
        if onScreen, let made = view?.displayLink(target: self, selector: #selector(fire)) {
            made.add(to: .main, forMode: .common)
            link = made
        } else {
            let made = Timer(timeInterval: 1.0 / 60.0, repeats: true) { [weak self] _ in
                MainActor.assumeIsolated { self?.fire() }
            }
            RunLoop.main.add(made, forMode: .common)
            timer = made
        }
    }

    func stop() {
        link?.invalidate()
        timer?.invalidate()
        link = nil
        timer = nil
    }

    @objc private func fire() {
        let now = CACurrentMediaTime()
        let dt = now - last
        last = now
        if !tick(dt) { stop() }
    }
}

/// The active tab's row: its name, and the contents under it in a scrolling area of their own.
final class ActiveTabCell: FlippedView {
    let tab = SidebarCell(frame: .zero)
    private weak var box: NSView?

    override init(frame: NSRect) {
        super.init(frame: frame)
        addSubview(tab)
    }

    @available(*, unavailable) required init?(coder: NSCoder) { fatalError() }

    func hold(_ contents: NSView?, tabHeight: CGFloat) {
        if let contents, contents.superview !== self {
            contents.removeFromSuperview()
            addSubview(contents)
        }
        if contents == nil, let box, box.superview === self { box.removeFromSuperview() }
        box = contents
        self.tabHeight = tabHeight
        needsLayout = true
    }

    private var tabHeight: CGFloat = 32

    override func layout() {
        super.layout()
        tab.frame = NSRect(x: 0, y: 0, width: bounds.width, height: tabHeight)
        box?.frame = NSRect(x: 0, y: tabHeight, width: bounds.width, height: max(bounds.height - tabHeight, 0))
    }
}

/// A shortened name in full, over its own row, as Finder and Mail do: past the sidebar's edge if it needs to, gone as the pointer leaves. It takes no clicks, and moves nothing.
final class RevealLabel: FlippedView {
    let label = TextLine()

    override init(frame: NSRect) {
        super.init(frame: frame)
        wantsLayer = true
        layer?.cornerRadius = 5
        layer?.borderWidth = 0.5
        addSubview(label)
    }

    @available(*, unavailable) required init?(coder: NSCoder) { fatalError() }

    override func hitTest(_ point: NSPoint) -> NSView? { nil }

    func show(_ text: String, font: NSFont, over place: NSRect, room: CGFloat) {
        layer?.backgroundColor = Theme.pill.cg
        layer?.borderColor = Theme.hairline.cg
        label.set(text, font: font, color: Theme.textStrong.ns, wraps: true)
        let padding: CGFloat = 4
        let width = room - place.minX - padding * 2
        let size = label.size(width: max(width, 40))
        label.frame = NSRect(x: padding, y: 2, width: size.width + 1, height: size.height)
        frame = NSRect(x: place.minX - padding, y: place.minY - 2, width: size.width + 1 + padding * 2, height: size.height + 4)
        isHidden = false
    }
}

/// The panel's items, in a compact block in the lower sidebar: a label, and a row of quiet buttons, the one showing picked out. All its words are Rust's. Built when it opens, and gone when it closes.
final class PanelView: FlippedView {
    private(set) var buttons: [String: NSButton] = [:]
    private let line = NSView()
    private static let rowHeight: CGFloat = 30
    private static let labelHeight: CGFloat = 18

    func show(_ model: PanelModel, target: AnyObject, action: Selector, width: CGFloat) -> CGFloat {
        subviews.forEach { $0.removeFromSuperview() }
        buttons = [:]
        line.wantsLayer = true
        line.layer?.backgroundColor = Theme.hairline.cg
        line.frame = NSRect(x: 0, y: 0, width: width, height: 1)
        addSubview(line)
        var y: CGFloat = 8
        let inset: CGFloat = 10
        for section in model.sections {
            if !section.title.isEmpty {
                let label = TextLine()
                label.set(section.title, font: .systemFont(ofSize: 11.5), color: Theme.textQuiet.ns)
                let size = label.size()
                label.frame = NSRect(x: inset + 6, y: y, width: size.width, height: size.height)
                addSubview(label)
                y += Self.labelHeight
            }
            let each = (width - inset * 2) / CGFloat(max(section.items.count, 1))
            for (index, item) in section.items.enumerated() {
                let button = NSButton(title: item.title, target: target, action: action)
                button.isBordered = false
                button.wantsLayer = true
                button.layer?.cornerRadius = 6
                button.layer?.backgroundColor = item.selected ? Theme.rowActive.cg : nil
                button.identifier = NSUserInterfaceItemIdentifier(item.id)
                button.attributedTitle = NSAttributedString(string: item.title, attributes: [.foregroundColor: (item.selected ? Theme.accent : Theme.textBody).ns, .font: NSFont.systemFont(ofSize: 12.5, weight: item.selected ? .semibold : .regular)])
                button.frame = NSRect(x: inset + CGFloat(index) * each, y: y, width: each, height: Self.rowHeight - 2)
                addSubview(button)
                buttons[item.id] = button
            }
            y += Self.rowHeight + 2
        }
        return y + 2
    }
}

/// The tabs, with the active tab's contents under it in a scrolling area of their own, whose height Rust decides. Both are tables with reused rows, and contents rows are fetched for what's on screen, so a thousand tabs or a 10,000-entry outline cost what fills the sidebar.
@MainActor final class SidebarPane: NSObject, NSTableViewDataSource, NSTableViewDelegate {
    let view = FlippedView()
    let scroll = NSScrollView()
    let table = NSTableView()
    let contentsScroll = ContentsScrollView()
    let contentsTable = NSTableView()
    let footer = TextLine()
    let addButton = NSButton(title: "", target: nil, action: nil)
    let moreButton = NSButton(title: "", target: nil, action: nil)
    private(set) var panel: PanelView?
    private var panelModel = PanelModel(open: false, buttonTooltip: "", sections: [])
    private var panelHeight: CGFloat = 0
    var performAction: ((String) -> Void)?
    private let core: AppCore
    private(set) var model = SidebarView(tabsVersion: 0, tabCount: 0, activeIndex: nil, contentsCount: 0, contentsVersion: 0, contentsChangedFrom: 0, contentsRemoved: 0, contentsInserted: 0, contentsPlaceShift: 0, contentsCurrent: nil, tocOpen: false, place: "", edited: false, undoneOwnerSteps: false)
    /// The tabs, pulled only when their version moves.
    private(set) var tabs: [SidebarRow] = []
    private var shownTabsVersion: UInt64 = 0
    private var shownContentsVersion: UInt64 = 0
    private var activeIndex: Int?
    private var layout = SidebarLayout(tabRow: 32, contentsRow: 28, contentsHeight: 0, tabsScroll: false)
    private(set) var hoveredRow: Int?
    var addPressed: (() -> Void)?
    private var tracking: NSTrackingArea?
    private var frames: FrameRunner?
    /// As the system says, or what QA set.
    var reduceMotion: Bool?
    private var pointerInContents = false
    /// Where a shortened name goes. One label for the whole list, and no tracking area per row: the row comes from where the pointer is.
    weak var revealHost: NSView?
    private let reveal = RevealLabel()
    private var revealTimer: Timer?
    private var revealedRow: Int?
    private(set) var lastPointerRow: Int?
    private let revealDelay: TimeInterval
    /// How far rows opening and closing have moved the scroll, so what QA records is what the eye sees.
    private var rowShift = 0.0

    init(core: AppCore, strings: AppStrings, isQuiet: Bool) {
        self.core = core
        revealDelay = core.limits().revealDelaySeconds
        super.init()
        frames = FrameRunner(view: contentsScroll, onScreen: !isQuiet) { [weak self] dt in
            MainActor.assumeIsolated { self?.frame(dt) ?? false }
        }
        view.wantsLayer = true
        for (scrollView, list) in [(scroll, table), (contentsScroll as NSScrollView, contentsTable)] {
            scrollView.documentView = list
            scrollView.automaticallyAdjustsContentInsets = false
            scrollView.hasVerticalScroller = true
            scrollView.autohidesScrollers = true
            scrollView.drawsBackground = false
            scrollView.contentView.drawsBackground = false
            list.addTableColumn(NSTableColumn(identifier: .init("row")))
            list.headerView = nil
            list.backgroundColor = .clear
            list.style = .plain
            list.intercellSpacing = .zero
            list.selectionHighlightStyle = .none
            list.gridStyleMask = []
            list.allowsEmptySelection = true
            list.dataSource = self
            list.delegate = self
            list.target = self
        }
        table.action = #selector(tabClicked)
        contentsTable.action = #selector(contentsClicked)
        // One height for every row, so the table asks nothing per row, however many there are.
        contentsTable.rowHeight = 28
        updateContentsScroller()
        contentsScroll.userScrolled = { [weak self] in self?.noteUserScrolled() }
        contentsScroll.pointerMoved = { [weak self] inside in self?.setContentsPointer(inside) }
        NotificationCenter.default.addObserver(self, selector: #selector(contentsLiveScrolled), name: NSScrollView.didLiveScrollNotification, object: contentsScroll)
        view.addSubview(scroll)
        view.addSubview(footer)
        view.addSubview(addButton)
        addButton.isBordered = false
        addButton.title = ""
        addButton.toolTip = strings.addTooltip
        addButton.image = NSImage(systemSymbolName: "plus", accessibilityDescription: strings.addTooltip)
        addButton.imagePosition = .imageOnly
        addButton.target = self
        addButton.action = #selector(addClicked)
        view.addSubview(moreButton)
        moreButton.isBordered = false
        moreButton.wantsLayer = true
        moreButton.layer?.cornerRadius = 6
        moreButton.image = NSImage(systemSymbolName: "ellipsis", accessibilityDescription: core.panel().buttonTooltip)
        moreButton.imagePosition = .imageOnly
        moreButton.toolTip = core.panel().buttonTooltip
        moreButton.target = self
        moreButton.action = #selector(moreClicked)
        NotificationCenter.default.addObserver(self, selector: #selector(frameChanged), name: NSView.frameDidChangeNotification, object: view)
        view.postsFrameChangedNotifications = true
    }

    @objc private func contentsLiveScrolled() {
        noteUserScrolled()
    }

    private func noteUserScrolled() {
        frames?.stop()
        core.contentsUserScrolled()
    }

    /// What the reader's own scroll does, for `recto qa contents-scroll`.
    func userScrollContents(toRow row: Int) {
        noteUserScrolled()
        contentsTable.scrollRowToVisible(min(row, Int(model.contentsCount) - 1))
    }

    /// What the pointer entering or leaving the contents does, and what `recto qa contents-pointer` calls.
    func setContentsPointer(_ inside: Bool) {
        pointerInContents = inside
        updateContentsScroller()
        core.contentsPointer(inside: inside)
        if inside { frames?.stop() }
    }

    /// A move of the contents isn't the reader's, so thin scrollers that come with movement stay off it, and with the pointer over them they're the system's as usual. A Mac set to always show them gets that.
    func updateContentsScroller() {
        contentsScroll.hasVerticalScroller = pointerInContents || contentsScroll.scrollerStyle == .legacy
    }

    private var contentsY: Double { contentsScroll.contentView.bounds.minY }

    private func setContentsY(_ y: Double) {
        let clip = contentsScroll.contentView
        var origin = clip.bounds.origin
        origin.y = y
        clip.setBoundsOrigin(clip.constrainBoundsRect(NSRect(origin: origin, size: clip.bounds.size)).origin)
        contentsScroll.reflectScrolledClipView(clip)
    }

    /// One frame of a move. Rust says where to be, and when to stop. Nothing runs once it has.
    private func frame(_ dt: Double) -> Bool {
        guard let next = core.contentsStep(position: contentsY, dt: dt) else { return false }
        setContentsY(next)
        return true
    }

    private var shouldReduceMotion: Bool {
        reduceMotion ?? NSWorkspace.shared.accessibilityDisplayShouldReduceMotion
    }

    /// Where the contents were on this step, what the eye sees: moving the scroll to match rows opening above isn't movement.
    func contentsSample() -> (y: Double, currentVisible: Bool) {
        let visible = contentsTable.rows(in: contentsTable.visibleRect)
        return (contentsY - rowShift, model.contentsCurrent.map { visible.contains(Int($0)) } ?? true)
    }

    /// The reader's place and the contents' own position, for QA.
    func contentsReport() -> [String: Any] {
        let visible = contentsTable.rows(in: contentsTable.visibleRect)
        let shown = contentsScroll.superview != nil
        return [
            "count": Int(model.contentsCount),
            "current": model.contentsCurrent.map { Int($0) as Any } ?? NSNull(),
            "height": shown ? contentsScroll.frame.height : 0,
            "first_visible": shown ? visible.location : 0,
            "visible_rows": shown ? visible.length : 0,
            "current_in_view": model.contentsCurrent.map { visible.contains(Int($0)) } ?? false,
            "following": core.contentsIsFollowing(),
            "moving": core.contentsIsMoving(),
            "frames_running": frames?.isRunning ?? false,
            "scroll_y": contentsY,
            "reduce_motion": shouldReduceMotion,
            "row_views": shown ? contentsTable.subviews.count : 0,
            "table_rows": contentsTable.numberOfRows,
            "stale_rows": staleContentsRows(),
            "scroller": contentsScroll.hasVerticalScroller,
            "tab_rows_on_screen": table.rows(in: table.visibleRect).length,
            "tabs_scroll": layout.tabsScroll,
            "list_height": scroll.frame.height,
            "footer_on_screen": footer.frame.maxY <= view.bounds.height && !footer.isHidden,
        ]
    }

    /// Row views the table holds that show something other than the model says. There should never be any.
    private func staleContentsRows() -> Int {
        var stale = 0
        contentsTable.enumerateAvailableRowViews { [self] _, row in
            guard let shown = (contentsTable.view(atColumn: 0, row: row, makeIfNecessary: false) as? SidebarCell)?.shownRow else { return }
            if core.contentsRows(first: UInt32(row), count: 1).first != shown { stale += 1 }
        }
        return stale
    }

    /// Tabs and contents, as a person counts them.
    var rowCount: Int { tabs.count + Int(model.tocOpen ? model.contentsCount : 0) }

    private var contentsHeight: CGFloat { model.tocOpen ? layout.contentsHeight : 0 }

    @objc private func frameChanged() {
        let size = view.bounds.size
        scroll.frame = NSRect(x: 0, y: 52, width: size.width, height: max(size.height - 52 - 40 - panelHeight, 0))
        placeFooter()
        addButton.frame = NSRect(x: size.width - 38, y: 12, width: 22, height: 22)
        moreButton.frame = NSRect(x: size.width - 44, y: size.height - 36, width: 30, height: 26)
        panel?.frame = NSRect(x: 0, y: size.height - 40 - panelHeight, width: size.width, height: panelHeight)
        if let column = table.tableColumns.first { column.width = size.width }
        if let column = contentsTable.tableColumns.first { column.width = size.width }
        updateTracking()
        relayout()
    }

    /// Asks Rust how the room divides, and redraws only if that changed.
    private func relayout() {
        let next = core.sidebarLayout(room: scroll.frame.height)
        guard next != layout else { return }
        let heightChanged = next.contentsHeight != layout.contentsHeight
        layout = next
        table.reloadData()
        if heightChanged {
            core.contentsViewChanged()
            revealPlace()
        }
    }

    private func placeFooter() {
        footer.set(model.place, font: .systemFont(ofSize: 12.5), color: Theme.textQuiet.ns)
        let size = footer.size()
        footer.frame = NSRect(x: 18, y: view.bounds.height - size.height - 12, width: size.width, height: size.height)
    }

    private func updateTracking() {
        if let tracking { view.removeTrackingArea(tracking) }
        let area = NSTrackingArea(rect: .zero, options: [.mouseEnteredAndExited, .mouseMoved, .activeInActiveApp, .inVisibleRect], owner: self, userInfo: nil)
        view.addTrackingArea(area)
        tracking = area
    }

    // AppKit calls these by name on a tracking area's owner. This class isn't a responder, so nothing makes them Objective-C but saying so, by the name AppKit uses: a plain `@objc` here would be `mouseEnteredWith:`, which nothing calls.
    @objc(mouseEntered:) func mouseEntered(with event: NSEvent) { setHover(0) }

    @objc(mouseExited:) func mouseExited(with event: NSEvent) {
        setHover(nil)
        pointerAt(nil)
    }

    @objc(mouseMoved:) func mouseMoved(with event: NSEvent) {
        pointerAt(sidebarRow(atWindowPoint: event.locationInWindow))
    }

    /// The row under a point, counted down the sidebar as a person sees them. Worked out on demand, from the two tables, for any number of rows.
    private func sidebarRow(atWindowPoint point: NSPoint) -> Int? {
        if contentsScroll.superview != nil, contentsScroll.bounds.contains(contentsScroll.convert(point, from: nil)), let active = activeIndex {
            let row = contentsTable.row(at: contentsTable.convert(point, from: nil))
            return row >= 0 ? active + 1 + row : nil
        }
        let row = table.row(at: table.convert(point, from: nil))
        guard row >= 0 else { return nil }
        if let active = activeIndex, row > active, model.tocOpen { return row + Int(model.contentsCount) }
        return row
    }

    /// The pointer has reached a row, or left. After a short rest, a shortened name opens. A one-off timer, only while the pointer is on a row.
    private func pointerAt(_ row: Int?) {
        guard row != lastPointerRow else { return }
        lastPointerRow = row
        revealTimer?.invalidate()
        revealTimer = nil
        revealRow(nil)
        guard let row else { return }
        let timer = Timer(timeInterval: revealDelay, repeats: false) { [weak self] _ in
            MainActor.assumeIsolated { self?.revealRow(row) }
        }
        RunLoop.main.add(timer, forMode: .common)
        revealTimer = timer
    }

    /// Where the pointer would be to rest on a shortened contents entry, for QA, and which row that is as the sidebar counts them.
    func cutOffContentsPlaceInWindow() -> (point: NSPoint, row: Int)? {
        guard let active = activeIndex, contentsScroll.superview != nil else { return nil }
        let visible = contentsTable.rows(in: contentsTable.visibleRect)
        for row in visible.location..<(visible.location + visible.length) {
            guard let cell = contentsTable.view(atColumn: 0, row: row, makeIfNecessary: false) as? SidebarCell, cell.isCutOff else { continue }
            return (cell.convert(NSPoint(x: cell.bounds.midX, y: cell.bounds.midY), to: nil), active + 1 + row)
        }
        return nil
    }

    /// What's drawn for a row, and its data, if it's on screen.
    private func cellAndRow(_ row: Int) -> (SidebarCell, SidebarRow)? {
        if model.tocOpen, let active = activeIndex, row > active, row - active - 1 < Int(model.contentsCount) {
            let inContents = row - active - 1
            guard let cell = contentsTable.view(atColumn: 0, row: inContents, makeIfNecessary: false) as? SidebarCell, let data = core.contentsRows(first: UInt32(inContents), count: 1).first else { return nil }
            return (cell, data)
        }
        let tab = model.tocOpen && activeIndex.map({ row > $0 }) == true ? row - Int(model.contentsCount) : row
        guard tab >= 0, tab < tabs.count, let view = table.view(atColumn: 0, row: tab, makeIfNecessary: false) else { return nil }
        guard let cell = (view as? ActiveTabCell)?.tab ?? view as? SidebarCell else { return nil }
        return (cell, tabs[tab])
    }

    /// What the pointer resting on a row does, and what `recto qa reveal` calls. Only a row that's cut off shows anything, and the words are Rust's.
    func revealRow(_ row: Int?) {
        revealedRow = row
        guard let row, let host = revealHost, let (cell, data) = cellAndRow(row), cell.isCutOff, !view.isHidden else {
            reveal.isHidden = true
            return
        }
        if reveal.superview !== host { host.addSubview(reveal) }
        // Above everything else in the window.
        reveal.removeFromSuperview()
        host.addSubview(reveal)
        reveal.show(core.rowReveal(row: data).text, font: cell.nameFont, over: cell.namePlace(in: host), room: host.bounds.width - 12)
    }

    func revealReport() -> [String: Any] {
        guard let row = revealedRow, let (cell, data) = cellAndRow(row) else { return ["row": revealedRow.map { $0 as Any } ?? NSNull(), "shown": false] }
        let full = core.rowReveal(row: data)
        return ["row": row, "cut_off": cell.isCutOff, "shown": !reveal.isHidden, "is_file_name": full.isFileName, "untrusted": ["text": full.text], "frame_right": reveal.frame.maxX, "sidebar_width": view.bounds.width]
    }

    /// What the pointer entering or leaving the sidebar does, and what `recto qa hover` calls.
    func setHover(_ row: Int?) {
        guard row != hoveredRow else { return }
        hoveredRow = row
        // Page numbers and the chevron come with hovering the list, as in the concept.
        reloadCells()
    }

    private func reloadCells() {
        table.enumerateAvailableRowViews { [self] _, index in
            guard index < tabs.count, let view = table.view(atColumn: 0, row: index, makeIfNecessary: false) else { return }
            let cell = (view as? ActiveTabCell)?.tab ?? view as? SidebarCell
            cell?.show(row: tabs[index], foldedActive: !model.tocOpen, hovered: hoveredRow != nil)
        }
        reloadContentsCells()
    }

    /// Every row view the table holds, including those it has ready just outside the view. Asking only for what's visible left rows ready with an old rail, which came back showing it once they'd scrolled in.
    private func reloadContentsCells() {
        var (first, last) = (Int.max, -1)
        contentsTable.enumerateAvailableRowViews { _, row in
            first = min(first, row)
            last = max(last, row)
        }
        guard last >= first else { return }
        let rows = core.contentsRows(first: UInt32(first), count: UInt32(last - first + 1))
        for (offset, row) in rows.enumerated() {
            (contentsTable.view(atColumn: 0, row: first + offset, makeIfNecessary: false) as? SidebarCell)?.show(row: row, foldedActive: false, hovered: hoveredRow != nil)
        }
    }

    @objc private func tabClicked() {
        tapTab(table.clickedRow)
    }

    @objc private func contentsClicked() {
        tapContents(contentsTable.clickedRow)
    }

    private func tapTab(_ row: Int) {
        guard row >= 0, row < tabs.count else { return }
        core.tapRow(row: tabs[row])
    }

    private func tapContents(_ row: Int) {
        guard row >= 0, let chosen = core.contentsRows(first: UInt32(row), count: 1).first else { return }
        core.tapRow(row: chosen)
    }

    /// What a click on a row does, and what `recto qa click-row` calls. Rows count down the sidebar as a person sees them, the contents under the active tab.
    func clickRow(_ row: Int) {
        guard row >= 0 else { return }
        guard model.tocOpen, let active = activeIndex, row > active else { return tapTab(row) }
        let inContents = row - active - 1
        if inContents < Int(model.contentsCount) {
            tapContents(inContents)
        } else {
            tapTab(row - Int(model.contentsCount))
        }
    }

    @objc private func addClicked() {
        addPressed?()
    }

    @objc private func moreClicked() {
        performAction?("panel.toggle")
    }

    @objc private func panelItemClicked(_ sender: NSButton) {
        guard let id = sender.identifier?.rawValue else { return }
        performAction?(id)
    }

    /// The panel as Rust has it. It docks above the footer in room taken from the list, so the height rule goes by what's left. Built only when it's opened or changed.
    private func updatePanel() {
        let next = core.panel()
        guard next != panelModel else { return }
        panelModel = next
        moreButton.layer?.backgroundColor = next.open ? Theme.rowActive.cg : nil
        moreButton.contentTintColor = (next.open ? Theme.accent : Theme.textQuiet).ns
        var height: CGFloat = 0
        if next.open {
            let block = panel ?? PanelView()
            if panel == nil {
                panel = block
                view.addSubview(block)
                block.wantsLayer = true
            }
            height = block.show(next, target: self, action: #selector(panelItemClicked(_:)), width: view.bounds.width)
        } else {
            panel?.removeFromSuperview()
            panel = nil
        }
        panelHeight = height
        frameChanged()
    }

    /// What QA reports of the panel.
    func panelReport() -> [String: Any] {
        ["open": panelModel.open, "height": panelHeight, "items": panelModel.sections.flatMap(\.items).map { ["id": $0.id, "title": $0.title, "selected": $0.selected] as [String: Any] }]
    }

    func update(_ new: SidebarView) {
        updatePanel()
        let (oldTabCount, oldActive, oldOpen) = (tabs.count, activeIndex, model.tocOpen)
        let oldActiveTab = activeTabID
        let tabsChanged = new.tabsVersion != shownTabsVersion
        if tabsChanged {
            tabs = core.sidebarTabs()
            shownTabsVersion = new.tabsVersion
        }
        model = new
        activeIndex = new.activeIndex.map { Int($0) }.flatMap { $0 < tabs.count ? $0 : nil }
        // A row under the label may have changed. It opens again only on the next rest.
        if tabsChanged || new.contentsVersion != shownContentsVersion { reveal.isHidden = true }
        placeFooter()
        let next = core.sidebarLayout(room: scroll.frame.height)
        let sameTab = activeTabID == oldActiveTab
        let sameList = tabs.count == oldTabCount && new.tocOpen == oldOpen && activeIndex == oldActive && sameTab
        let heightChanged = next != layout
        layout = next
        contentsTable.isHidden = !new.tocOpen
        if !sameTab { rowShift = 0 }
        if sameList {
            // A version applied once and only once: a publish can come twice for one change, and two changes can come as one pull.
            if new.contentsVersion == shownContentsVersion {
                reloadContentsCells()
            } else if new.contentsVersion == shownContentsVersion + 1 {
                applyRowChange(new)
            } else {
                keepingScrollAccount { contentsTable.reloadData() }
            }
            if heightChanged {
                // The area's height moved with the count, and that can clamp the scroll.
                keepingScrollAccount { table.reloadData() }
            } else if tabsChanged {
                reloadCells()
            }
        } else {
            table.reloadData()
            contentsTable.reloadData()
            if let active = activeIndex { table.scrollRowToVisible(active) }
        }
        shownContentsVersion = new.contentsVersion
        revealPlace()
    }

    private var activeTabID: UInt32? { activeIndex.flatMap { $0 < tabs.count ? tabs[$0].tab : nil } }

    /// Whatever this does to the scroll, by clamping it or otherwise, Rust and QA hear of as rows moving and not as anyone scrolling.
    private func keepingScrollAccount(_ work: () -> Void) {
        let before = contentsY
        work()
        let moved = contentsY - before
        if moved != 0 {
            rowShift += moved
            core.contentsShifted(by: moved)
        }
    }

    /// Rust's rule says whether the place being read needs bringing into view, and how. A resize changes the height with the table not laid out yet, so the height is the rule's, not the view's.
    private func revealPlace() {
        table.layoutSubtreeIfNeeded()
        switch core.contentsReached(row: model.tocOpen ? model.contentsCurrent : nil, scrollY: contentsY, height: contentsHeight, reduceMotion: shouldReduceMotion) {
        case .stay:
            break
        case let .setTo(y):
            frames?.stop()
            setContentsY(y)
        case .move:
            frames?.start()
        }
    }

    /// Rows opened or closed as the reader moved. Rust's rule says whether the scroll takes it up, in the same transaction so that what stays still does. If not, they slide, and with Reduce Motion they don't. Whatever else moves the scroll, as a shorter list clamps it, counts the same.
    private func applyRowChange(_ new: SidebarView) {
        let from = Int(new.contentsChangedFrom)
        let (removed, inserted) = (Int(new.contentsRemoved), Int(new.contentsInserted))
        keepingScrollAccount {
            let before = contentsY
            if let by = core.contentsRowChange(scrollY: before, from: new.contentsChangedFrom, removed: new.contentsRemoved, inserted: new.contentsInserted, placeShift: new.contentsPlaceShift) {
                // Rows going in the view, with no slide to show it, fade over. A layer the size of the view for a moment, and it's gone.
                if !shouldReduceMotion, Int(before / layout.contentsRow) < from + removed, Double(from) * layout.contentsRow < before + contentsHeight {
                    let fade = CATransition()
                    fade.type = .fade
                    fade.duration = 0.15
                    contentsScroll.layer?.add(fade, forKey: "rows")
                }
                contentsTable.reloadData()
                setContentsY(before + by)
                return
            }
            let motion: NSTableView.AnimationOptions = shouldReduceMotion ? [] : [.effectFade, .slideUp]
            contentsTable.beginUpdates()
            if removed > 0 { contentsTable.removeRows(at: IndexSet(from..<(from + removed)), withAnimation: motion) }
            if inserted > 0 { contentsTable.insertRows(at: IndexSet(from..<(from + inserted)), withAnimation: motion) }
            contentsTable.endUpdates()
            // The rail and which row is the place are about every row, and they're drawn from Rust's model.
            reloadContentsCells()
        }
    }

    func appearanceChanged() {
        view.layer?.backgroundColor = Theme.sidebar.cg
        placeFooter()
        addButton.contentTintColor = Theme.textQuiet.ns
        moreButton.contentTintColor = (panelModel.open ? Theme.accent : Theme.textQuiet).ns
        moreButton.layer?.backgroundColor = panelModel.open ? Theme.rowActive.cg : nil
        // Its colours are the window's too.
        if panelModel.open, let panel {
            panelHeight = panel.show(panelModel, target: self, action: #selector(panelItemClicked(_:)), width: view.bounds.width)
        }
        reloadCells()
    }

    func numberOfRows(in tableView: NSTableView) -> Int {
        tableView === contentsTable ? Int(model.tocOpen ? model.contentsCount : 0) : tabs.count
    }

    func tableView(_ tableView: NSTableView, heightOfRow row: Int) -> CGFloat {
        if tableView === contentsTable { return layout.contentsRow }
        return layout.tabRow + (row == activeIndex ? contentsHeight : 0)
    }

    func tableView(_ tableView: NSTableView, viewFor tableColumn: NSTableColumn?, row: Int) -> NSView? {
        if tableView === contentsTable {
            let cell = reused(tableView, "contents")
            if let chosen = core.contentsRows(first: UInt32(row), count: 1).first {
                cell.show(row: chosen, foldedActive: false, hovered: hoveredRow != nil)
            }
            return cell
        }
        let tab = tabs[row]
        if row == activeIndex {
            let id = NSUserInterfaceItemIdentifier("active")
            let holder = tableView.makeView(withIdentifier: id, owner: self) as? ActiveTabCell ?? {
                let made = ActiveTabCell(frame: .zero)
                made.identifier = id
                return made
            }()
            holder.hold(contentsHeight > 0 ? contentsScroll : nil, tabHeight: layout.tabRow)
            holder.tab.show(row: tab, foldedActive: !model.tocOpen, hovered: hoveredRow != nil)
            return holder
        }
        let cell = reused(tableView, "tab")
        cell.show(row: tab, foldedActive: !model.tocOpen, hovered: hoveredRow != nil)
        return cell
    }

    private func reused(_ tableView: NSTableView, _ name: String) -> SidebarCell {
        let id = NSUserInterfaceItemIdentifier(name)
        if let cell = tableView.makeView(withIdentifier: id, owner: self) as? SidebarCell { return cell }
        let made = SidebarCell(frame: .zero)
        made.identifier = id
        return made
    }

    func tableView(_ tableView: NSTableView, shouldSelectRow row: Int) -> Bool { false }
}
