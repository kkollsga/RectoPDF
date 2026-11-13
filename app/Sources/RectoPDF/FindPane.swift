import AppKit
import recto_ffiFFI

/// A row of Find's list: a page's quiet label, or a hit's words with what matched in bold. Drawn from Rust's row, and only ever as text.
final class FindRowCell: NSView {
    private let line = TextLine()
    private(set) var shown: FindRow?
    override var isFlipped: Bool { true }

    override init(frame: NSRect) {
        super.init(frame: frame)
        wantsLayer = true
        layer?.cornerRadius = 5
        addSubview(line)
    }

    required init?(coder: NSCoder) { fatalError("not used") }

    func show(_ row: FindRow) {
        shown = row
        applyLook()
    }

    func applyLook() {
        guard let row = shown else { return }
        let string = NSMutableAttributedString()
        let style = NSMutableParagraphStyle()
        style.lineBreakMode = .byTruncatingTail
        switch row.kind {
        case .page:
            string.append(NSAttributedString(string: row.label, attributes: [.font: NSFont.systemFont(ofSize: 11, weight: .medium), .foregroundColor: Theme.textQuiet.ns]))
        case .hit:
            let body: [NSAttributedString.Key: Any] = [.font: NSFont.systemFont(ofSize: 12.5), .foregroundColor: Theme.textBody.ns]
            string.append(NSAttributedString(string: row.label, attributes: body))
            string.append(NSAttributedString(string: row.matched, attributes: [.font: NSFont.systemFont(ofSize: 12.5, weight: .semibold), .foregroundColor: (row.current ? Theme.accent : Theme.textStrong).ns]))
            string.append(NSAttributedString(string: row.after, attributes: body))
            if !row.note.isEmpty {
                string.append(NSAttributedString(string: "  \(row.note)", attributes: [.font: NSFont.systemFont(ofSize: 11), .foregroundColor: Theme.textQuiet.ns]))
            }
        }
        string.addAttribute(.paragraphStyle, value: style, range: NSRange(location: 0, length: string.length))
        line.setAttributed(string)
        layer?.backgroundColor = row.current ? Theme.rowActive.cg : nil
        needsLayout = true
    }

    override func layout() {
        let indent: CGFloat = shown?.kind == .hit ? 26 : 18
        let height = line.size().height
        line.frame = NSRect(x: indent, y: (bounds.height - height) / 2, width: max(bounds.width - indent - 12, 0), height: height)
    }
}

/// The Find field, the line under it and the hit list, at the top of the sidebar while Find is open. What it shows, how tall it is and what each key does are Rust's: this draws them and says what happened.
@MainActor final class FindPane: NSObject, NSTableViewDataSource, NSTableViewDelegate, NSSearchFieldDelegate {
    let view = FlippedView()
    let field = NSSearchField()
    let status = TextLine()
    let listScroll = NSScrollView()
    let list = NSTableView()
    let thumb = ThumbView()
    private let core: AppCore
    private(set) var model = FindView(open: false, text: "", placeholder: "", status: "", version: 0, rowCount: 0, currentRow: nil, running: false, closeTooltip: "")
    private var heights = (field: 28.0, status: 20.0, list: 0.0, row: 24.0)
    /// ⌘F asked for the keyboard, for once Find's open and laid out.
    var wantsFocus = false
    /// The current row last brought into view, so it's brought there once and the reader's own scroll stays.
    private var shownCurrent: UInt32?
    /// Something's been typed since the field took the keyboard. Until then the field still shows what Rust has, so a draft set a moment before focus came isn't left behind by it.
    private var typedSinceFocus = false

    init(core: AppCore) {
        self.core = core
        super.init()
        view.wantsLayer = true
        view.isHidden = true
        field.delegate = self
        field.sendsSearchStringImmediately = true
        field.sendsWholeSearchString = false
        field.focusRingType = .none
        field.font = .systemFont(ofSize: 13)
        (field.cell as? NSSearchFieldCell)?.cancelButtonCell?.target = self
        (field.cell as? NSSearchFieldCell)?.cancelButtonCell?.action = #selector(closeClicked)
        view.addSubview(field)
        view.addSubview(status)
        listScroll.documentView = list
        listScroll.automaticallyAdjustsContentInsets = false
        listScroll.hasVerticalScroller = false
        listScroll.hasHorizontalScroller = false
        listScroll.drawsBackground = false
        listScroll.contentView.drawsBackground = false
        list.addTableColumn(NSTableColumn(identifier: .init("row")))
        list.headerView = nil
        list.backgroundColor = .clear
        list.style = .plain
        list.intercellSpacing = .zero
        list.selectionHighlightStyle = .none
        list.gridStyleMask = []
        list.refusesFirstResponder = true
        list.dataSource = self
        list.delegate = self
        list.target = self
        list.action = #selector(rowClicked)
        view.addSubview(listScroll)
        thumb.list = "find"
        thumb.attach(to: listScroll)
    }

    /// Takes Rust's model and its height rule. Says whether it's a different height, which the sidebar lays out for.
    func update(_ new: FindView, layout: SidebarLayout) {
        let rowsChanged = new.version != model.version || new.rowCount != model.rowCount || new.open != model.open
        let wasOpen = model.open
        model = new
        heights = (layout.findField, layout.findStatus, layout.findList, layout.findRow)
        view.isHidden = !new.open
        field.placeholderString = new.placeholder
        // What Rust says is in the field, never once typing's started in it: an echo of what was typed a moment ago would take the newest keys away. A field that has the keyboard and nothing typed yet still shows Rust's, selected, as ⌘F leaves it.
        if !typedSinceFocus, field.stringValue != new.text {
            showRustText(new.text)
        }
        (field.cell as? NSSearchFieldCell)?.cancelButtonCell?.setAccessibilityLabel(new.closeTooltip)
        field.toolTip = new.closeTooltip.isEmpty ? nil : new.closeTooltip
        status.set(new.status, font: .systemFont(ofSize: 11.5), color: Theme.textQuiet.ns)
        list.rowHeight = heights.row
        // Laid out first, so bringing the current row into view goes by the list's real height.
        placeViews()
        if rowsChanged {
            list.reloadData()
        }
        if new.currentRow != shownCurrent, let current = new.currentRow, heights.list > 0 {
            list.scrollRowToVisible(Int(current))
        }
        shownCurrent = heights.list > 0 ? new.currentRow : nil
        if !new.open && wasOpen, field.currentEditor() != nil {
            view.window?.makeFirstResponder(nil)
        }
        thumb.refresh()
        if wantsFocus && new.open {
            DispatchQueue.main.async { [weak self] in self?.focusField() }
        }
    }

    func placeViews() {
        let width = view.bounds.width
        field.frame = NSRect(x: 12, y: 0, width: max(width - 24, 0), height: heights.field)
        status.frame = NSRect(x: 18, y: heights.field + 3, width: max(width - 30, 0), height: heights.status - 3)
        listScroll.frame = NSRect(x: 0, y: heights.field + heights.status, width: width, height: heights.list)
        if let column = list.tableColumns.first { column.width = width }
    }

    func appearanceChanged() {
        status.set(model.status, font: .systemFont(ofSize: 11.5), color: Theme.textQuiet.ns)
        list.enumerateAvailableRowViews { rowView, _ in
            (rowView.view(atColumn: 0) as? FindRowCell)?.applyLook()
        }
        thumb.refresh()
    }

    /// The keyboard goes to the field, with what's in it selected, as ⌘F does on a Mac.
    func focusField() {
        guard let window = view.window, model.open else { return }
        wantsFocus = false
        // What Rust has, before typing starts: it isn't set while the field's being edited.
        if field.currentEditor() == nil { field.stringValue = model.text }
        window.makeFirstResponder(field)
        field.currentEditor()?.selectAll(nil)
        core.focusChanged(view: "find")
    }

    var hasFocus: Bool { field.currentEditor() != nil }

    /// Rust's text in the field. While the field has the keyboard it goes through the field editor, all selected, so typing replaces it; that sends no change, so nothing is found for it.
    private func showRustText(_ text: String) {
        if let editor = field.currentEditor() {
            editor.string = text
            editor.selectAll(nil)
        } else {
            field.stringValue = text
        }
    }

    /// What typing does, and what `recto qa find-type` calls.
    func typed(_ text: String) {
        if hasFocus { typedSinceFocus = true }
        _ = core.act(action: .findTyped(text: text))
    }

    func controlTextDidChange(_ notification: Notification) {
        typed(field.stringValue)
    }

    /// Typing's over: the field shows what Rust has, which it didn't while it was being typed in. Rust's model is pulled again, as it may have moved meanwhile.
    func controlTextDidEndEditing(_ notification: Notification) {
        typedSinceFocus = false
        model = core.findView()
        if field.stringValue != model.text { field.stringValue = model.text }
    }

    /// Return and ⇧Return step through the hits; Escape closes Find. Everything else is the field's.
    func control(_ control: NSControl, textView: NSTextView, doCommandBy selector: Selector) -> Bool {
        switch selector {
        case #selector(NSResponder.insertNewline(_:)):
            _ = core.act(action: .findReturn(forward: !(NSApp.currentEvent?.modifierFlags.contains(.shift) ?? false)))
            return true
        case #selector(NSResponder.cancelOperation(_:)):
            closeClicked()
            return true
        default:
            return false
        }
    }

    @objc func closeClicked() {
        _ = core.act(action: .findClose)
        view.window?.makeFirstResponder(view.window?.contentView)
    }

    @objc private func rowClicked() {
        let row = list.clickedRow
        guard row >= 0 else { return }
        clickRow(row)
    }

    /// What a click on a row does, and what `recto qa find-click` calls.
    func clickRow(_ row: Int) {
        _ = core.act(action: .findChoose(row: UInt32(row)))
    }

    func numberOfRows(in tableView: NSTableView) -> Int { Int(model.open ? model.rowCount : 0) }

    func tableView(_ tableView: NSTableView, heightOfRow row: Int) -> CGFloat { heights.row }

    func tableView(_ tableView: NSTableView, viewFor tableColumn: NSTableColumn?, row: Int) -> NSView? {
        let id = NSUserInterfaceItemIdentifier("find")
        let cell = tableView.makeView(withIdentifier: id, owner: self) as? FindRowCell ?? {
            let made = FindRowCell(frame: .zero)
            made.identifier = id
            return made
        }()
        if let shown = core.findRows(first: UInt32(row), count: 1).first { cell.show(shown) }
        return cell
    }

    func tableView(_ tableView: NSTableView, shouldSelectRow row: Int) -> Bool { false }

    /// What QA reads: Rust's view of Find, and what's on screen.
    func report() -> [String: Any] {
        let visible = list.rows(in: list.visibleRect)
        var rows: [[String: Any]] = []
        if model.open, visible.length > 0 {
            for row in core.findRows(first: UInt32(visible.location), count: UInt32(min(visible.length, 6))) {
                rows.append(["kind": row.kind == .page ? "page" : "hit", "current": row.current, "note": row.note, "untrusted": ["label": row.label, "matched": row.matched]])
            }
        }
        return [
            "open": model.open,
            "shown": !view.isHidden && view.superview != nil,
            "untrusted": ["field": field.stringValue],
            "status": model.status,
            "rows": Int(model.rowCount),
            "current_row": model.currentRow.map { Int($0) as Any } ?? NSNull(),
            "running": model.running,
            "list_height": listScroll.frame.height,
            "block_height": view.frame.height,
            "first_visible": visible.location,
            "visible_rows": visible.length,
            "rows_on_screen": rows,
            "field_has_focus": hasFocus,
            "thumb": thumb.report,
        ]
    }
}
