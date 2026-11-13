import AppKit

/// The Settings window, which draws what Rust's model says and passes on what's flipped. Small and of one size. In a quiet run it's off every screen and never ordered in, like the main window.
@MainActor final class SettingsWindowController: NSObject, NSWindowDelegate {
    private unowned let delegate: AppDelegate
    let window: NSWindow
    private(set) var switches: [String: NSSwitch] = [:]
    private var shownRows: [SettingRow] = []
    private var shownAction: SettingsAction?
    /// A waiting update and its button, drawn from Rust's words. Hidden when there's none.
    private let actionBox = NSStackView()
    private(set) var updateButton: NSButton?
    private let rows = NSStackView()
    private let status = NSTextField(wrappingLabelWithString: "")
    /// The privacy statement, in Rust's words, opened as a link the person chose.
    let privacyLink = NSButton(title: "", target: nil, action: nil)
    private var privacyURL = ""
    private static let width: CGFloat = 460

    init(delegate: AppDelegate, place: [Double]?) {
        self.delegate = delegate
        let rect = NSRect(x: 0, y: 0, width: Self.width, height: 160)
        let style: NSWindow.StyleMask = [.titled, .closable]
        if delegate.isQuiet {
            window = QuietWindow(contentRect: rect.offsetBy(dx: -40_000, dy: -40_000), styleMask: style, backing: .buffered, defer: false)
        } else {
            window = NSWindow(contentRect: rect, styleMask: style, backing: .buffered, defer: false)
        }
        window.isReleasedWhenClosed = false
        window.animationBehavior = .none
        super.init()
        window.delegate = self
        let content = NSStackView()
        content.orientation = .vertical
        content.alignment = .leading
        content.spacing = 16
        content.edgeInsets = NSEdgeInsets(top: 20, left: 20, bottom: 20, right: 20)
        rows.orientation = .vertical
        rows.alignment = .leading
        rows.spacing = 14
        status.font = .systemFont(ofSize: NSFont.smallSystemFontSize)
        status.textColor = .secondaryLabelColor
        status.preferredMaxLayoutWidth = Self.width - 40
        actionBox.orientation = .vertical
        actionBox.alignment = .leading
        actionBox.spacing = 6
        actionBox.isHidden = true
        content.addArrangedSubview(actionBox)
        content.addArrangedSubview(rows)
        content.addArrangedSubview(status)
        privacyLink.isBordered = false
        privacyLink.target = self
        privacyLink.action = #selector(openPrivacy)
        content.addArrangedSubview(privacyLink)
        window.contentView = content
        refresh()
        if delegate.isQuiet {
            window.setFrameOrigin(NSPoint(x: -40_000, y: -40_000))
        } else if let place, place.count == 2 {
            window.setFrameOrigin(NSPoint(x: place[0], y: place[1]))
        } else {
            window.center()
        }
        noteWindow()
    }

    func refresh() {
        let model = delegate.core.settingsModel()
        window.title = model.title
        status.stringValue = model.status
        privacyURL = model.privacyUrl
        privacyLink.attributedTitle = NSAttributedString(string: model.privacyTitle, attributes: [.font: NSFont.systemFont(ofSize: NSFont.smallSystemFontSize), .foregroundColor: NSColor.linkColor])
        if model.action != shownAction {
            shownAction = model.action
            showAction(model.action)
        }
        if model.rows != shownRows {
            shownRows = model.rows
            rows.arrangedSubviews.forEach { $0.removeFromSuperview() }
            switches.removeAll()
            for row in model.rows { rows.addArrangedSubview(view(for: row)) }
        }
        window.setContentSize(NSSize(width: Self.width, height: window.contentView?.fittingSize.height ?? 160))
    }

    private func showAction(_ action: SettingsAction?) {
        actionBox.arrangedSubviews.forEach { $0.removeFromSuperview() }
        updateButton = nil
        actionBox.isHidden = action == nil
        guard let action else { return }
        let title = NSTextField(labelWithString: action.title)
        title.font = .boldSystemFont(ofSize: NSFont.systemFontSize)
        let hint = NSTextField(wrappingLabelWithString: action.hint)
        hint.font = .systemFont(ofSize: NSFont.smallSystemFontSize)
        hint.textColor = .secondaryLabelColor
        hint.preferredMaxLayoutWidth = Self.width - 40
        let button = NSButton(title: action.button, target: self, action: #selector(updateNow(_:)))
        button.bezelStyle = .rounded
        button.keyEquivalent = ""
        button.isEnabled = action.enabled
        button.identifier = NSUserInterfaceItemIdentifier(action.id)
        updateButton = button
        [title, hint, button].forEach { actionBox.addArrangedSubview($0) }
    }

    /// The switch or button with this id, for QA: clicked by the same call a person's click makes.
    func control(_ id: String) -> NSControl? {
        switches[id] ?? (updateButton?.identifier?.rawValue == id ? updateButton : nil)
    }

    @objc private func updateNow(_ sender: NSButton) {
        guard let id = sender.identifier?.rawValue else { return }
        _ = delegate.core.act(action: .settingChosen(id: id, on: true))
    }

    private func view(for row: SettingRow) -> NSView {
        let title = NSTextField(labelWithString: row.title)
        let hint = NSTextField(wrappingLabelWithString: row.hint)
        hint.font = .systemFont(ofSize: NSFont.smallSystemFontSize)
        hint.textColor = .secondaryLabelColor
        hint.preferredMaxLayoutWidth = Self.width - 110
        let words = NSStackView(views: [title, hint])
        words.orientation = .vertical
        words.alignment = .leading
        words.spacing = 3
        let toggle = NSSwitch()
        toggle.state = row.on ? .on : .off
        toggle.isEnabled = row.enabled
        toggle.target = self
        toggle.action = #selector(flipped(_:))
        toggle.identifier = NSUserInterfaceItemIdentifier(row.id)
        switches[row.id] = toggle
        let line = NSStackView(views: [words, toggle])
        line.orientation = .horizontal
        line.alignment = .centerY
        line.distribution = .fill
        line.spacing = 16
        line.widthAnchor.constraint(equalToConstant: Self.width - 40).isActive = true
        return line
    }

    @objc private func openPrivacy() {
        delegate.openAddress(privacyURL)
    }

    @objc private func flipped(_ sender: NSSwitch) {
        guard let id = sender.identifier?.rawValue else { return }
        _ = delegate.core.act(action: .settingChosen(id: id, on: sender.state == .on))
    }

    private func noteWindow() {
        delegate.core.settingsWindowChanged(open: true, x: window.frame.minX, y: window.frame.minY)
    }

    func windowDidMove(_ notification: Notification) {
        noteWindow()
    }

    func windowWillClose(_ notification: Notification) {
        delegate.core.settingsWindowChanged(open: false, x: 0, y: 0)
        delegate.settingsClosed()
    }

    /// For QA: what's showing.
    func report() -> [String: Any] {
        let model = delegate.core.settingsModel()
        return ["open": true, "x": window.frame.minX, "y": window.frame.minY, "title": window.title, "status": status.stringValue, "privacy": ["title": privacyLink.attributedTitle.string, "url": privacyURL], "action": shownAction.map { ["id": $0.id, "title": $0.title, "hint": $0.hint, "button": updateButton?.title ?? "", "enabled": updateButton?.isEnabled ?? false] as [String: Any] } ?? NSNull(), "rows": model.rows.map { row in
            ["id": row.id, "title": row.title, "hint": row.hint, "on": switches[row.id]?.state == .on, "enabled": switches[row.id]?.isEnabled ?? false] as [String: Any]
        }]
    }
}
