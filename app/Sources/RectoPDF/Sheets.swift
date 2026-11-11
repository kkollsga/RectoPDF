import AppKit

/// Every sheet and panel the app puts up goes through here.
///
/// A quiet run never begins one. To hang a sheet on a window AppKit orders the window in, and a window that's off every screen is slid onto the nearest one first: the owner saw a quiet run's window flash up in the lower left of the screen (2026-11-08), sampled at 1440 x 900 sliding in from -40 000 and back each time the print panel or a close prompt came and went. So in a quiet run the sheet is made and held here, never shown, and `qa sheet-answer` ends it through the same completion a click on its button runs. A shot draws it from its own window, where it would hang.
@MainActor final class SheetPresenter {
    private let quiet: Bool
    /// The sheet a quiet run holds instead of showing: its window, if it has one of ours to draw, and what its ending does.
    private var held: (window: NSWindow?, finish: (NSApplication.ModalResponse) -> Void)?

    init(quiet: Bool) {
        self.quiet = quiet
    }

    /// Whether a sheet or panel is up over the window, shown or held: only one at a time.
    func isUp(over window: NSWindow) -> Bool {
        held != nil || window.attachedSheet != nil
    }

    /// The window of the sheet that's up, to draw into a shot or reach a field on it.
    func sheetWindow(over window: NSWindow) -> NSWindow? {
        held?.window ?? window.attachedSheet
    }

    func present(_ alert: NSAlert, over window: NSWindow, completion: @escaping (NSApplication.ModalResponse) -> Void) {
        guard quiet else { return alert.beginSheetModal(for: window, completionHandler: completion) }
        // Laid out as it would be shown, accessory and all, with nothing ordered in.
        alert.layout()
        // What shows in its rounded corners is what's behind it, as the window server would composite it, not the window's square background.
        alert.window.isOpaque = false
        alert.window.backgroundColor = .clear
        held = (alert.window, completion)
    }

    /// Save As's sheet. The file browser is drawn by another process, so a quiet run has no window of it to draw.
    func present(_ panel: NSSavePanel, over window: NSWindow, completion: @escaping (NSApplication.ModalResponse) -> Void) {
        guard quiet else { return panel.beginSheetModal(for: window, completionHandler: completion) }
        held = (nil, completion)
    }

    /// The print panel. In a quiet run the first answer is Print and anything else Cancel.
    func present(_ panel: NSPrintPanel, info: NSPrintInfo, over window: NSWindow, completion: @escaping (NSPrintPanel.Result) -> Void) {
        guard quiet else { return panel.beginSheet(using: info, on: window, completionHandler: completion) }
        held = (nil, { completion($0 == .alertFirstButtonReturn || $0 == .OK ? .printed : .cancelled) })
    }

    /// The Open panel, which is a window of its own rather than a sheet.
    func present(_ panel: NSOpenPanel, completion: @escaping (NSApplication.ModalResponse) -> Void) {
        guard quiet else { return panel.begin(completionHandler: completion) }
        held = (nil, completion)
    }

    /// An alert of the app's own, with no window to hang it on, as when the window's closed. App-modal, after this call rather than in it.
    func presentAppModal(_ alert: NSAlert, completion: @escaping (NSApplication.ModalResponse) -> Void) {
        guard held == nil, !appModalUp else { return }
        guard quiet else {
            appModalUp = true
            DispatchQueue.main.async { [weak self] in
                let response = alert.runModal()
                self?.appModalUp = false
                completion(response)
            }
            return
        }
        alert.layout()
        alert.window.isOpaque = false
        alert.window.backgroundColor = .clear
        held = (alert.window, completion)
    }

    /// Whether one of the app's own alerts is up, shown or held.
    var appModalShowing: Bool { appModalUp || held != nil }

    private var appModalUp = false

    /// Ends the sheet over the window as its button would, through the same completion. False with none up.
    func answer(_ response: NSApplication.ModalResponse, over window: NSWindow?) -> Bool {
        if let held {
            // Let go of first: an ending can put up the next one, as a close prompt's Save puts up Save As.
            self.held = nil
            held.finish(response)
            return true
        }
        guard let window, let sheet = window.attachedSheet else { return false }
        window.endSheet(sheet, returnCode: response)
        return true
    }

    /// Where the sheet's top left sits in the window, from the window's bottom left: hung from just under the title bar, in the middle, as AppKit hangs one.
    func sheetOrigin(over window: NSWindow) -> NSPoint? {
        guard let sheet = sheetWindow(over: window) else { return nil }
        if held == nil { return NSPoint(x: sheet.frame.minX - window.frame.minX, y: sheet.frame.minY - window.frame.minY) }
        let titleBar = window.frame.height - window.contentLayoutRect.maxY
        return NSPoint(x: ((window.frame.width - sheet.frame.width) / 2).rounded(), y: window.frame.height - titleBar - sheet.frame.height)
    }
}
