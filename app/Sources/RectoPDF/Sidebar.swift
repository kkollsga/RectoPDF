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

/// The tabs, with the active tab's contents under it in a scrolling area of their own, whose height Rust decides. Both are tables with reused rows, and contents rows are fetched for what's on screen, so a thousand tabs or a 10,000-entry outline cost what fills the sidebar.
@MainActor final class SidebarPane: NSObject, NSTableViewDataSource, NSTableViewDelegate {
    let view = FlippedView()
    let scroll = NSScrollView()
    let table = NSTableView()
    let contentsScroll = ContentsScrollView()
    let contentsTable = NSTableView()
    let footer = TextLine()
    let addButton = NSButton(title: "", target: nil, action: nil)
    private let core: AppCore
    private(set) var model = SidebarView(tabs: [], contentsCount: 0, contentsChangedFrom: 0, contentsRemoved: 0, contentsInserted: 0, contentsCurrent: nil, tocOpen: false, place: "", edited: false, undoneOwnerSteps: false)
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
    private var lastPointerRow: Int?
    /// How long the pointer rests on a shortened name before it opens.
    private static let revealDelay: TimeInterval = 0.5
    /// How far rows opening and closing have moved the scroll, so what QA records is what the eye sees.
    private var rowShift = 0.0

    init(core: AppCore, strings: AppStrings, isQuiet: Bool) {
        self.core = core
        super.init()
        frames = FrameRunner(view: contentsScroll, onScreen: !isQuiet) { [weak self] dt in
            MainActor.assumeIsolated { self?.frame(dt) ?? false }
        }
        view.wantsLayer = true
        for (scrollView, list) in [(scroll, table), (contentsScroll as NSScrollView, contentsTable)] {
            scrollView.documentView = list
            scrollView.hasVerticalScroller = true
            scrollView.autohidesScrollers = true
            scrollView.drawsBackground = false
            scrollView.scrollerStyle = .overlay
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
        core.contentsPointer(inside: inside)
        if inside { frames?.stop() }
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
            "stale_rows": staleContentsRows(),
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
    var rowCount: Int { model.tabs.count + Int(model.tocOpen ? model.contentsCount : 0) }

    private var activeIndex: Int? { model.tabs.firstIndex { $0.current } }

    private var contentsHeight: CGFloat { model.tocOpen ? layout.contentsHeight : 0 }

    @objc private func frameChanged() {
        let size = view.bounds.size
        scroll.frame = NSRect(x: 0, y: 52, width: size.width, height: max(size.height - 52 - 40, 0))
        placeFooter()
        addButton.frame = NSRect(x: size.width - 38, y: 12, width: 22, height: 22)
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

    func mouseEntered(with event: NSEvent) { setHover(0) }

    func mouseExited(with event: NSEvent) {
        setHover(nil)
        pointerAt(nil)
    }

    func mouseMoved(with event: NSEvent) {
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
        let timer = Timer(timeInterval: Self.revealDelay, repeats: false) { [weak self] _ in
            MainActor.assumeIsolated { self?.revealRow(row) }
        }
        RunLoop.main.add(timer, forMode: .common)
        revealTimer = timer
    }

    /// What's drawn for a row, and its data, if it's on screen.
    private func cellAndRow(_ row: Int) -> (SidebarCell, SidebarRow)? {
        if model.tocOpen, let active = activeIndex, row > active, row - active - 1 < Int(model.contentsCount) {
            let inContents = row - active - 1
            guard let cell = contentsTable.view(atColumn: 0, row: inContents, makeIfNecessary: false) as? SidebarCell, let data = core.contentsRows(first: UInt32(inContents), count: 1).first else { return nil }
            return (cell, data)
        }
        let tab = model.tocOpen && activeIndex.map({ row > $0 }) == true ? row - Int(model.contentsCount) : row
        guard tab >= 0, tab < model.tabs.count, let view = table.view(atColumn: 0, row: tab, makeIfNecessary: false) else { return nil }
        guard let cell = (view as? ActiveTabCell)?.tab ?? view as? SidebarCell else { return nil }
        return (cell, model.tabs[tab])
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
            guard index < model.tabs.count, let view = table.view(atColumn: 0, row: index, makeIfNecessary: false) else { return }
            let cell = (view as? ActiveTabCell)?.tab ?? view as? SidebarCell
            cell?.show(row: model.tabs[index], foldedActive: !model.tocOpen, hovered: hoveredRow != nil)
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
        guard row >= 0, row < model.tabs.count else { return }
        core.tapRow(row: model.tabs[row])
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

    func update(_ new: SidebarView) {
        let old = model
        let oldActive = activeIndex
        model = new
        // A row under the label may have changed. It opens again only on the next rest.
        if new.tabs != old.tabs || new.contentsCount != old.contentsCount { reveal.isHidden = true }
        placeFooter()
        let next = core.sidebarLayout(room: scroll.frame.height)
        let sameTab = new.tabs.first { $0.current }?.tab == old.tabs.first { $0.current }?.tab
        let sameShape = new.tabs.count == old.tabs.count && new.tocOpen == old.tocOpen && activeIndex == oldActive && next == layout
        layout = next
        contentsTable.isHidden = !new.tocOpen
        if !sameTab { rowShift = 0 }
        if sameShape && new.tabs == old.tabs {
            if new.contentsRemoved > 0 || new.contentsInserted > 0 {
                applyRowChange(new)
            } else {
                reloadContentsCells()
            }
        } else if sameShape {
            reloadCells()
            if new.contentsRemoved > 0 || new.contentsInserted > 0 { applyRowChange(new) }
        } else {
            table.reloadData()
            contentsTable.reloadData()
            if let active = activeIndex { table.scrollRowToVisible(active) }
        }
        revealPlace()
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

    /// Rows opened or closed as the reader moved. Above the view, the scroll moves by what changed, in the same transaction, so nothing can be seen to move. In the view, they slide, and with Reduce Motion they don't.
    private func applyRowChange(_ new: SidebarView) {
        let from = Int(new.contentsChangedFrom)
        let (removed, inserted) = (Int(new.contentsRemoved), Int(new.contentsInserted))
        let topRow = Int(contentsY / layout.contentsRow)
        if topRow >= from + removed {
            let by = Double(inserted - removed) * layout.contentsRow
            contentsTable.reloadData()
            let before = contentsY
            setContentsY(before + by)
            let moved = contentsY - before
            rowShift += moved
            core.contentsShifted(by: moved)
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

    func appearanceChanged() {
        view.layer?.backgroundColor = Theme.sidebar.cg
        placeFooter()
        addButton.contentTintColor = Theme.textQuiet.ns
        reloadCells()
    }

    func numberOfRows(in tableView: NSTableView) -> Int {
        tableView === contentsTable ? Int(model.tocOpen ? model.contentsCount : 0) : model.tabs.count
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
        let tab = model.tabs[row]
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
