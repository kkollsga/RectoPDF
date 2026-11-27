import AppKit

extension MainWindowController {
    /// The sheet Rust has over the window, as the system's alert: its words, its buttons in its order, the destructive one marked. The answer goes back as
    /// the button's index; what each button does is the feature's, in Rust.
    func showSheetModelIfAny() {
        guard let model = reader.sheet else {
            sheetModelShowing = nil
            return
        }
        guard sheetModelShowing != model, !delegate.sheets.isUp(over: window) else { return }
        sheetModelShowing = model
        let alert = NSAlert()
        alert.alertStyle = .warning
        alert.messageText = model.title
        alert.informativeText = model.message
        for (index, title) in model.buttons.enumerated() {
            let button = alert.addButton(withTitle: title)
            button.hasDestructiveAction = model.destructive == UInt32(index)
        }
        let core = delegate.core
        core.sheetChanged(open: true)
        delegate.sheetShowing = ["kind": model.kind, "message": model.title, "informative": model.message, "buttons": model.buttons]
        let delegate = self.delegate
        let count = model.buttons.count
        delegate.sheets.present(alert, over: window) { response in
            core.sheetChanged(open: false)
            delegate.sheetShowing = nil
            // Its window went: Rust still has the sheet, and the next window says it.
            guard response != SheetPresenter.windowGone else { return }
            let index = response.rawValue - NSApplication.ModalResponse.alertFirstButtonReturn.rawValue
            let button = index >= 0 && index < count ? UInt32(index) : UInt32.max
            let action = core.act(action: .sheetAnswered(kind: model.kind, id: model.id, button: button)).host
            delegate.handle(action)
        }
    }
}
