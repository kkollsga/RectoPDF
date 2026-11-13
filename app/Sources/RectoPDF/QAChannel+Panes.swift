import AppKit
import CryptoKit
import QuartzCore

/// A comparison's panes: scrolling, focus and the right-click menu of one.
extension QAChannel {
    /// The panes verbs. Answers `nil` for a verb that isn't one of them, `true` when the verb ends as most do (the window laid out, then the report), `false` when it has already answered or failed.
    func qaPanes(_ id: UInt64, _ verb: String, _ value: String, _ controller: MainWindowController?) -> Bool? {
        switch verb {
        case "pane_scroll":
            // A scroll in a pane, by what a scroll does: the reader's own, so synced scroll follows it and it's the pane being read.
            let parts = value.split(separator: ":")
            guard let controller, parts.count == 2, let pane = Int(parts[0]), pane < controller.paneAreas.count, let points = Double(parts[1]) else { return fail(id, "That isn't a pane and a scroll.") }
            controller.paneAreas[pane].scroll(by: points)
            controller.drainAll()
            return true
        case "pane_focus":
            // As a right-click on the pane reaches it, by the method AppKit calls for one: the menu's made and not shown.
            guard let controller, let pane = Int(value), pane < controller.paneAreas.count else { return fail(id, "There's no such pane.") }
            _ = controller.paneAreas[pane].documentView.menu(for: rightClick(in: controller.paneAreas[pane], window: controller.window))
            return true
        case "pane_context_menu":
            // A right-click on a pane, by what AppKit calls for one, then the item, as a person would choose it.
            let parts = value.split(separator: ":", maxSplits: 1).map(String.init)
            guard let controller, parts.count == 2, let pane = Int(parts[0]), pane < controller.paneAreas.count else { return fail(id, "That isn't a pane and an item.") }
            guard let menu = controller.paneAreas[pane].documentView.menu(for: rightClick(in: controller.paneAreas[pane], window: controller.window)) else { return fail(id, "There's no menu.") }
            menu.update()
            guard let item = menu.items.first(where: { ($0.representedObject as? String) == parts[1] }), let action = item.action else { return fail(id, "That isn't in the pane's right-click menu.") }
            guard item.isEnabled else { return fail(id, "That's greyed out in the right-click menu right now.") }
            NSApp.sendAction(action, to: item.target, from: item)
            return true
        default:
            return nil
        }
    }

    /// A right-click in the middle of a pane, made and handed over by selector, as the entry points are: nothing's posted.
    func rightClick(in pane: PageArea, window: NSWindow) -> NSEvent {
        let place = pane.scrollView.convert(NSPoint(x: pane.scrollView.bounds.midX, y: pane.scrollView.bounds.midY), to: nil)
        return NSEvent.mouseEvent(with: .rightMouseDown, location: place, modifierFlags: [], timestamp: 0, windowNumber: window.windowNumber, context: nil, eventNumber: 0, clickCount: 1, pressure: 1)!
    }

    /// This area's part of the report.
    func panesSection(controller: MainWindowController) -> [String: Any] {
        var pages = controller.pages.report()
        pages["visible"] = !controller.pages.scrollView.isHidden
        return [
            "pages": pages,
            "panes": controller.paneReport(),
        ]
    }
}
