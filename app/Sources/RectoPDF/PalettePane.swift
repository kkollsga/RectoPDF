import AppKit
import recto_ffiFFI

/// A row of the palette: its title, and where it lives and its key in a quieter colour on the right. Rust's words, drawn as text.
final class PaletteRowCell: NSView {
    private let title = TextLine()
    private let detail = TextLine()
    private(set) var shown: PaletteRow?
    private var selected = false
    override var isFlipped: Bool { true }

    override init(frame: NSRect) {
        super.init(frame: frame)
        wantsLayer = true
        layer?.cornerRadius = 6
        addSubview(title)
        addSubview(detail)
    }

    required init?(coder: NSCoder) { fatalError("not used") }

    func show(_ row: PaletteRow, selected: Bool) {
        shown = row
        self.selected = selected
        applyLook()
    }

    func applyLook() {
        guard let row = shown else { return }
        title.set(row.title, font: .systemFont(ofSize: 13, weight: selected ? .medium : .regular), color: (selected ? Theme.textStrong : Theme.textBody).ns)
        let right = row.shortcut.isEmpty ? row.detail : "\(row.detail)  \(row.shortcut)"
        detail.set(right, font: .systemFont(ofSize: 11.5), color: Theme.textQuiet.ns, alignment: .right)
        layer?.backgroundColor = selected ? Theme.rowActive.cg : nil
        needsLayout = true
    }

    override func layout() {
        let rightWidth = min(detail.size().width, bounds.width * 0.45)
        let height = title.size().height
        detail.frame = NSRect(x: bounds.width - rightWidth - 12, y: (bounds.height - detail.size().height) / 2, width: rightWidth, height: detail.size().height)
        title.frame = NSRect(x: 12, y: (bounds.height - height) / 2, width: max(bounds.width - rightWidth - 34, 0), height: height)
    }
}

/// ⌘K's palette, in the sidebar in place of its tabs and contents while it's open: a field and the rows it finds. What's listed, its order, its words and what each does are Rust's; this draws them and says what was typed and pressed. Keys: typing, ↑ ↓, Return, Escape.
@MainActor final class PalettePane: NSObject, NSTableViewDataSource, NSTableViewDelegate, NSSearchFieldDelegate {
    let view = FlippedView()
    let field = NSSearchField()
    let empty = TextLine()
    let listScroll = NSScrollView()
    let list = NSTableView()
    private let core: AppCore
    private(set) var model = PaletteView(open: false, text: "", placeholder: "", rows: [], selected: nil, empty: "")
    /// What a run leaves for the host, as a menu item's would.
    var hostAction: ((HostAction) -> Void)?
    /// ⌘K asked for the keyboard.
    var wantsFocus = false
    static let rowHeight: CGFloat = 30

    init(core: AppCore) {
        self.core = core
        super.init()
        view.wantsLayer = true
        view.isHidden = true
        field.delegate = self
        field.sendsSearchStringImmediately = true
        field.focusRingType = .none
        field.font = .systemFont(ofSize: 13)
        view.addSubview(field)
        view.addSubview(empty)
        listScroll.documentView = list
        listScroll.automaticallyAdjustsContentInsets = false
        listScroll.hasVerticalScroller = false
        listScroll.drawsBackground = false
        listScroll.contentView.drawsBackground = false
        list.addTableColumn(NSTableColumn(identifier: .init("row")))
        list.headerView = nil
        list.backgroundColor = .clear
        list.style = .plain
        list.intercellSpacing = .zero
        list.selectionHighlightStyle = .none
        list.refusesFirstResponder = true
        list.rowHeight = Self.rowHeight
        list.dataSource = self
        list.delegate = self
        list.target = self
        list.action = #selector(rowClicked)
        view.addSubview(listScroll)
    }

    /// Takes Rust's model. Says whether it's open.
    @discardableResult
    func refresh() -> Bool {
        let wasOpen = model.open
        model = core.paletteView()
        view.isHidden = !model.open
        field.placeholderString = model.placeholder
        if field.stringValue != model.text, field.currentEditor() == nil || !model.open { field.stringValue = model.text }
        empty.set(model.rows.isEmpty && model.open ? model.empty : "", font: .systemFont(ofSize: 12.5), color: Theme.textQuiet.ns)
        list.reloadData()
        if let selected = model.selected { list.scrollRowToVisible(Int(selected)) }
        placeViews()
        if model.open, wantsFocus { focusField() }
        if !model.open, wasOpen, field.currentEditor() != nil { view.window?.makeFirstResponder(view.window?.contentView) }
        return model.open
    }

    func placeViews() {
        let size = view.bounds.size
        field.frame = NSRect(x: 12, y: 0, width: max(size.width - 24, 0), height: 28)
        empty.frame = NSRect(x: 18, y: 40, width: max(size.width - 30, 0), height: 18)
        listScroll.frame = NSRect(x: 6, y: 36, width: max(size.width - 12, 0), height: max(size.height - 36, 0))
        if let column = list.tableColumns.first { column.width = listScroll.frame.width }
    }

    func appearanceChanged() { list.reloadData() }

    func focusField() {
        guard let window = view.window, model.open else { return }
        wantsFocus = false
        window.makeFirstResponder(field)
        field.currentEditor()?.selectAll(nil)
    }

    /// What typing does, and what `recto qa palette-type` calls.
    func typed(_ text: String) {
        core.paletteTyped(text: text)
        refresh()
    }

    func controlTextDidChange(_ notification: Notification) { typed(field.stringValue) }

    /// The keyboard went elsewhere: the palette goes, as a menu does.
    func controlTextDidEndEditing(_ notification: Notification) {
        if model.open { core.paletteClose() }
    }

    /// What a key in the field does, and what `recto qa palette-key` calls.
    func press(_ key: String) {
        switch key {
        case "up": core.paletteStep(by: -1); refresh()
        case "down": core.paletteStep(by: 1); refresh()
        case "return":
            let action = core.paletteRunSelected()
            refresh()
            hostAction?(action)
        case "escape": core.paletteClose(); refresh()
        default: break
        }
    }

    func control(_ control: NSControl, textView: NSTextView, doCommandBy selector: Selector) -> Bool {
        switch selector {
        case #selector(NSResponder.moveUp(_:)): press("up")
        case #selector(NSResponder.moveDown(_:)): press("down")
        case #selector(NSResponder.insertNewline(_:)): press("return")
        case #selector(NSResponder.cancelOperation(_:)): press("escape")
        default: return false
        }
        return true
    }

    @objc private func rowClicked() {
        let row = list.clickedRow
        guard row >= 0, row < model.rows.count else { return }
        let action = core.paletteRun(id: model.rows[row].id)
        refresh()
        hostAction?(action)
    }

    func numberOfRows(in tableView: NSTableView) -> Int { model.open ? model.rows.count : 0 }

    func tableView(_ tableView: NSTableView, viewFor tableColumn: NSTableColumn?, row: Int) -> NSView? {
        let id = NSUserInterfaceItemIdentifier("palette")
        let cell = tableView.makeView(withIdentifier: id, owner: self) as? PaletteRowCell ?? {
            let made = PaletteRowCell(frame: .zero)
            made.identifier = id
            return made
        }()
        cell.show(model.rows[row], selected: model.selected.map { Int($0) } == row)
        return cell
    }

    func tableView(_ tableView: NSTableView, shouldSelectRow row: Int) -> Bool { false }

    func report() -> [String: Any] {
        [
            "open": model.open,
            "shown": !view.isHidden,
            "untrusted": ["field": field.stringValue],
            "rows": model.rows.prefix(8).map { ["id": $0.id, "title": $0.title, "detail": $0.detail, "shortcut": $0.shortcut] as [String: Any] },
            "row_count": model.rows.count,
            "selected": model.selected.map { Int($0) as Any } ?? NSNull(),
            "field_has_focus": field.currentEditor() != nil,
            "visible_rows": list.rows(in: list.visibleRect).length,
        ]
    }
}
