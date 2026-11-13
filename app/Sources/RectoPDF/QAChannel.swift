import AppKit
import CryptoKit
import QuartzCore

/// QA commands, on the UI thread. Each calls what the UI calls, posts no event, and answers once the app has done it and laid out. A shot is drawn in the app, so it needs no permission, no display and no focus.
@MainActor final class QAChannel {
    unowned let delegate: AppDelegate
    var stepCarry = 0.0
    /// Set by `recto qa scroller-style`, for each style on any Mac. With none, what the system says.
    var scrollerStyle: NSScroller.Style?

    init(delegate: AppDelegate) {
        self.delegate = delegate
    }

    /// The window's frame and buttons take the key look from being told, which reaches nothing outside this process.
    static func drawActive(_ window: NSWindow) {
        window.becomeMain()
        window.becomeKey()
    }

    func answer(_ id: UInt64, _ value: Any) {
        guard id != 0 else { return }
        let data = (try? JSONSerialization.data(withJSONObject: value, options: [.fragmentsAllowed])) ?? Data("null".utf8)
        delegate.core.qaAnswer(id: id, ok: true, json: String(data: data, encoding: .utf8) ?? "null")
    }

    func fail(_ id: UInt64, _ words: String) {
        delegate.core.qaAnswer(id: id, ok: false, json: words)
    }

    /// The same, for a verb that ends there: `false` says it has answered and nothing more is to be done.
    @discardableResult func fail(_ id: UInt64, _ words: String) -> Bool {
        delegate.core.qaAnswer(id: id, ok: false, json: words)
        return false
    }

    func handle(id: UInt64, verb: String, value: String) {
        let controller = delegate.windowController
        // Each area answers its own verbs; `nil` is a verb it doesn't know.
        let finished = qaPointer(id, verb, value, controller) ?? qaWindow(id, verb, value, controller) ?? qaControls(id, verb, value, controller) ?? qaSidebar(id, verb, value, controller) ?? qaFind(id, verb, value, controller) ?? qaPanes(id, verb, value, controller) ?? qaMenus(id, verb, value, controller) ?? qaDialogs(id, verb, value, controller) ?? qaShots(id, verb, value, controller) ?? qaReport(id, verb, value, controller)
        guard let finished else { return fail(id, "That isn't a QA command.") }
        if !finished { return }
        delegate.windowController?.window.layoutIfNeeded()
        answer(id, state())
    }

    /// What the window's display cycle would do before anything's seen. A quiet run's window is never on screen, so AppKit has no display cycle for it, and its views are laid out only when something asks: a report or `qa wait-settled` that came before that saw a table with no rows made and a pane with no height (CI, 2026-11-11, on a slower machine where the order differed). A person's window is laid out before it's drawn, so this only makes a quiet run the same.
    ///
    /// And the run's activation policy as its guard holds it: something outside the app can change it, and the guard hears of that a turn of the run loop or more later, so a report in between said `accessory` (a full check, 2026-11-11). The guard puts it back here first, so a look sees the rule kept, and says in the run's messages that it had to.
    func readyToBeLookedAt() {
        if delegate.isQuiet { QuietRunGuard.keepProhibited() }
        for window in NSApp.windows where window.contentView != nil {
            window.layoutIfNeeded()
        }
    }

    /// The policy a report says. In a quiet run it's read through the guard, put back first and read straight after, with nothing in between: the system can change it at any moment, as a reopen does, and a report reading it at the end of a long build of the rest saw `regular` that the guard was about to undo (CI run 43085841110, and a full check's slow stage, 2026-11-11).
    func policyName() -> String {
        switch delegate.isQuiet ? QuietRunGuard.policyAsLookedAt() : NSApp.activationPolicy() {
        case .regular: "regular"
        case .accessory: "accessory"
        default: "prohibited"
        }
    }
}
