import AppKit
import CryptoKit
import QuartzCore

/// The window and the app around it: appearance, full screen, minimise, hide, focus, frontmost, size, close and open, quit, the scroller style, accessibility, and the stall.
extension QAChannel {
    /// The window verbs. Answers `nil` for a verb that isn't one of them, `true` when the verb ends as most do (the window laid out, then the report), `false` when it has already answered or failed.
    func qaWindow(_ id: UInt64, _ verb: String, _ value: String, _ controller: MainWindowController?) -> Bool? {
        switch verb {
        case "appearance":
            NSApp.appearance = NSAppearance(named: value == "dark" ? .darkAqua : .aqua)
            delegate.core.setAppearanceChoice(choice: value)
            controller?.appearanceChanged()
            return true
        case "full_screen":
            controller?.setQuietFullScreen(value == "on")
            return true
        case "minimise":
            guard let controller else { return fail(id, "The window is closed.") }
            controller.setQuietMinimised(value == "on")
            return true
        case "hide_app":
            delegate.setQuietHidden(value == "on")
            return true
        case "focus":
            controller?.applyFocus(value)
            return true
        case "frontmost":
            delegate.core.frontmostChanged(app: value == "on", windowKey: value == "on")
            return true
        case "quit":
            answer(id, ["quitting": true])
            DispatchQueue.main.async { NSApp.terminate(nil) }
            return false
        case "scroller_style":
            scrollerStyle = value == "legacy" ? .legacy : .overlay
            controller?.layout()
            return true
        case "accessibility":
            if let problem = controller.map({ $0.qaAccessibility(value) }) ?? "The window is closed." { return fail(id, problem) }
            return true
        case "reduce_motion":
            controller?.sidebar.reduceMotion = value == "on"
            return true
        case "resize":
            let parts = value.split(separator: "x").compactMap { Double($0) }
            guard parts.count == 2 else { return fail(id, "That isn't a size.") }
            controller?.window.setFrame(NSRect(x: -40_000, y: -40_000, width: parts[0], height: parts[1]), display: false)
            controller?.layout()
            controller?.noteWindow()
            return true
        case "window":
            if value == "close" {
                controller?.window.close()
            } else {
                delegate.openWindow()
            }
            return true
        case "stall":
            Thread.sleep(forTimeInterval: (Double(value) ?? 0) / 1000)
            return true
        default:
            return nil
        }
    }

    /// This area's part of the report.
    func windowSection(controller: MainWindowController, reader: ReaderView, overPages: (NSRect) -> Bool) -> [String: Any] {
        let frame = controller.window.frame
        return [
            "window_open": true,
            "window": ["x": frame.minX, "y": frame.minY, "width": frame.width, "height": frame.height, "full_screen": controller.isFullScreen, "minimised": controller.isMinimised, "on_any_screen": NSScreen.screens.contains { $0.frame.intersects(frame) }, "is_visible": controller.window.isVisible],
            "is_active": NSApp.isActive,
            "hidden": delegate.isAppHidden,
            "activation": policyName(),
            "appearance": Theme.isDark(controller.root) ? "dark" : "light",
            "save_button": ["visible": !controller.saveButton.isHidden, "enabled": controller.saveButton.isEnabled, "tooltip": controller.saveButton.toolTip ?? "", "frame": [controller.saveButton.frame.minX, controller.saveButton.frame.minY, controller.saveButton.frame.width, controller.saveButton.frame.height], "clear_of_window_buttons": controller.windowButtonFrames(in: controller.root).allSatisfy { !$0.intersects(controller.saveButton.frame) } && !controller.sidebarButton.frame.intersects(controller.saveButton.frame)] as [String: Any],
            "sheet": delegate.sheetShowing ?? [:],
            "sidebar_button": ["visible": !controller.sidebarButton.isHidden && controller.sidebarButton.superview != nil, "frame": [controller.sidebarButton.frame.minX, controller.sidebarButton.frame.minY, controller.sidebarButton.frame.width, controller.sidebarButton.frame.height], "clear_of_window_buttons": controller.windowButtonFrames(in: controller.root).allSatisfy { !$0.intersects(controller.sidebarButton.frame) }] as [String: Any],
            "window_buttons_shown": controller.windowButtonsShown,
            "chrome": controller.chrome.report,
            "home": !controller.home.isHidden,
            "home_note": controller.homeNote.text.isEmpty ? NSNull() as Any : ["text": controller.homeNote.text, "frame": [controller.homeNote.frame.minX, controller.homeNote.frame.minY, controller.homeNote.frame.width, controller.homeNote.frame.height], "in_home": controller.home.bounds.contains(controller.homeNote.frame)] as [String: Any],
            "paper": reader.paper,
            "animations": controller.runningAnimations(),
            "place": reader.place,
            "agent_icon": !controller.agentIcon.isHidden,
            "document_edited": controller.window.isDocumentEdited,
            "agent_icon_margins": ["top": controller.agentIcon.frame.minY, "right": controller.root.bounds.maxX - controller.agentIcon.frame.maxX, "clear_of_pages": controller.agentIcon.isHidden || !overPages(controller.agentIcon.frame)] as [String: Any],
            "layout": reader.layout,
            "backing": controller.window.backingScaleFactor,
        ]
    }
}
