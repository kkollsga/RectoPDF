import AppKit

/// A row: only its name at rest. Its chevron and page number come with hover, and all it draws comes from Rust's row.
final class SidebarCell: NSView {
    let name = NSTextField(labelWithString: "")
    let pageNumber = NSTextField(labelWithString: "")
    let chevron = NSTextField(labelWithString: "")
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
        for label in [name, pageNumber, chevron] {
            label.lineBreakMode = .byTruncatingTail
            label.maximumNumberOfLines = 1
            label.cell?.truncatesLastVisibleLine = true
            addSubview(label)
        }
        pageNumber.alignment = .right
        pageNumber.font = .monospacedDigitSystemFont(ofSize: 11.5, weight: .regular)
        chevron.alignment = .center
        chevron.font = .systemFont(ofSize: 14, weight: .medium)
    }

    @available(*, unavailable) required init?(coder: NSCoder) { fatalError() }

    func show(row: SidebarRow, foldedActive: Bool, hovered: Bool) {
        self.row = row
        self.folded = foldedActive
        self.hovered = hovered
        name.stringValue = row.text
        pageNumber.stringValue = row.page.map(String.init) ?? ""
        needsLayout = true
        applyLook()
    }

    func applyLook() {
        guard let row else { return }
        let isTab = row.kind == .tab
        let active = isTab && row.current
        pill.isHidden = !active
        pill.backgroundColor = Theme.cg(Theme.rowActive, in: self)
        name.font = active ? .systemFont(ofSize: 14, weight: .semibold) : row.current ? .systemFont(ofSize: 13.5, weight: .semibold) : .systemFont(ofSize: isTab ? 14 : 13.5)
        name.textColor = active ? Theme.textStrong : row.current ? Theme.accent : row.railFilled && !isTab ? Theme.textBody : Theme.textQuiet
        pageNumber.textColor = row.current ? Theme.accent : Theme.textQuiet
        pageNumber.isHidden = isTab || !hovered || row.page == nil
        chevron.stringValue = folded ? "›" : "⌄"
        chevron.textColor = Theme.textQuiet
        chevron.isHidden = !(active && hovered)
        let isContents = row.kind == .contents
        railLine.isHidden = !isContents
        railFill.isHidden = !(isContents && row.railFilled)
        railDot.isHidden = !(isContents && row.current)
        railLine.backgroundColor = Theme.cg(Theme.railRest, in: self)
        railFill.backgroundColor = Theme.cg(Theme.accent, in: self)
        railDot.backgroundColor = Theme.cg(Theme.accent, in: self)
    }

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
        name.sizeToFit()
        let textHeight = name.frame.height
        name.frame = NSRect(x: indent, y: (height - textHeight) / 2, width: max(right - indent, 20), height: textHeight)
        pageNumber.sizeToFit()
        pageNumber.frame = NSRect(x: width - 58, y: (height - pageNumber.frame.height) / 2, width: 40, height: pageNumber.frame.height)
        chevron.sizeToFit()
        chevron.frame = NSRect(x: width - 42, y: (height - chevron.frame.height) / 2, width: 24, height: chevron.frame.height)
    }
}

/// One table with the tabs and the active tab's contents, all from Rust's model. Row views are reused, so a thousand tabs cost what fills the sidebar.
@MainActor final class SidebarPane: NSObject, NSTableViewDataSource, NSTableViewDelegate {
    let view = FlippedView()
    let scroll = NSScrollView()
    let table = NSTableView()
    let footer = NSTextField(labelWithString: "")
    let addButton = NSButton(title: "", target: nil, action: nil)
    private let core: AppCore
    private(set) var model = SidebarView(rows: [], tocOpen: false, place: "", edited: false, undoneOwnerSteps: false)
    private(set) var hoveredRow: Int?
    var addPressed: (() -> Void)?
    private var tracking: NSTrackingArea?

    init(core: AppCore, strings: AppStrings) {
        self.core = core
        super.init()
        view.wantsLayer = true
        scroll.documentView = table
        scroll.hasVerticalScroller = true
        scroll.autohidesScrollers = true
        scroll.drawsBackground = false
        scroll.scrollerStyle = .overlay
        scroll.contentView.drawsBackground = false
        table.addTableColumn(NSTableColumn(identifier: .init("row")))
        table.headerView = nil
        table.backgroundColor = .clear
        table.style = .plain
        table.intercellSpacing = .zero
        table.selectionHighlightStyle = .none
        table.gridStyleMask = []
        table.allowsEmptySelection = true
        table.dataSource = self
        table.delegate = self
        table.target = self
        table.action = #selector(rowClicked)
        view.addSubview(scroll)
        view.addSubview(footer)
        view.addSubview(addButton)
        footer.font = .systemFont(ofSize: 12.5)
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

    @objc private func frameChanged() {
        let size = view.bounds.size
        scroll.frame = NSRect(x: 0, y: 52, width: size.width, height: max(size.height - 52 - 40, 0))
        placeFooter()
        addButton.frame = NSRect(x: size.width - 38, y: 12, width: 22, height: 22)
        if let column = table.tableColumns.first { column.width = size.width }
        updateTracking()
    }

    private func placeFooter() {
        footer.sizeToFit()
        footer.frame.origin = NSPoint(x: 18, y: view.bounds.height - footer.frame.height - 12)
    }

    private func updateTracking() {
        if let tracking { table.removeTrackingArea(tracking) }
        let area = NSTrackingArea(rect: .zero, options: [.mouseEnteredAndExited, .mouseMoved, .activeInActiveApp, .inVisibleRect], owner: self, userInfo: nil)
        table.addTrackingArea(area)
        tracking = area
    }

    func mouseMoved(with event: NSEvent) {
        let row = table.row(at: table.convert(event.locationInWindow, from: nil))
        setHover(row >= 0 ? row : nil)
    }

    func mouseEntered(with event: NSEvent) { mouseMoved(with: event) }
    func mouseExited(with event: NSEvent) { setHover(nil) }

    /// What the pointer entering or leaving a row does, and what `recto qa hover` calls.
    func setHover(_ row: Int?) {
        guard row != hoveredRow else { return }
        hoveredRow = row
        // Page numbers and the chevron come with hovering the row list, as in the concept.
        reloadCells()
    }

    private func reloadCells() {
        let visible = table.rows(in: table.visibleRect)
        guard visible.length > 0 else { return }
        for index in visible.location..<(visible.location + visible.length) {
            if let cell = table.view(atColumn: 0, row: index, makeIfNecessary: false) as? SidebarCell, index < model.rows.count {
                cell.show(row: model.rows[index], foldedActive: !model.tocOpen, hovered: hoveredRow != nil)
            }
        }
    }

    @objc private func rowClicked() {
        clickRow(table.clickedRow)
    }

    /// What a click on a row does, and what `recto qa click-row` calls.
    func clickRow(_ row: Int) {
        guard row >= 0, row < model.rows.count else { return }
        core.tapRow(row: model.rows[row])
    }

    @objc private func addClicked() {
        addPressed?()
    }

    func update(_ new: SidebarView) {
        guard new != model else { return }
        let sameRows = new.rows.count == model.rows.count
        model = new
        footer.stringValue = new.place
        placeFooter()
        if sameRows {
            reloadCells()
        } else {
            table.reloadData()
        }
    }

    func appearanceChanged() {
        view.layer?.backgroundColor = Theme.cg(Theme.sidebar, in: view)
        footer.textColor = Theme.textQuiet
        addButton.contentTintColor = Theme.textQuiet
        reloadCells()
    }

    func numberOfRows(in tableView: NSTableView) -> Int {
        model.rows.count
    }

    func tableView(_ tableView: NSTableView, heightOfRow row: Int) -> CGFloat {
        model.rows[row].kind == .tab ? 32 : 28
    }

    func tableView(_ tableView: NSTableView, viewFor tableColumn: NSTableColumn?, row: Int) -> NSView? {
        let id = NSUserInterfaceItemIdentifier("cell")
        let cell = tableView.makeView(withIdentifier: id, owner: self) as? SidebarCell ?? {
            let made = SidebarCell(frame: .zero)
            made.identifier = id
            return made
        }()
        cell.show(row: model.rows[row], foldedActive: !model.tocOpen, hovered: hoveredRow != nil)
        return cell
    }

    func tableView(_ tableView: NSTableView, shouldSelectRow row: Int) -> Bool { false }
}
