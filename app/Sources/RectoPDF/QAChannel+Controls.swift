import AppKit
import CryptoKit
import QuartzCore

/// The window's buttons and fields: Save, zoom, the page number and go to.
extension QAChannel {
    /// The controls verbs. Answers `nil` for a verb that isn't one of them, `true` when the verb ends as most do (the window laid out, then the report), `false` when it has already answered or failed.
    func qaControls(_ id: UInt64, _ verb: String, _ value: String, _ controller: MainWindowController?) -> Bool? {
        switch verb {
        case "button":
            guard let controller else { return fail(id, "The window is closed.") }
            switch value {
            case "sidebar": controller.sidebarButton.performClick(nil)
            case "save":
                guard !controller.saveButton.isHidden else { return fail(id, "The Save icon isn't showing.") }
                guard controller.saveButton.isEnabled else { return fail(id, "Save is off: \(controller.saveButton.toolTip ?? "")") }
                controller.saveButton.performClick(nil)
            case "zoom":
                guard !controller.zoomButton.isHidden else { return fail(id, "The zoom level isn't showing.") }
                controller.zoomButton.performClick(nil)
            case "zoom_reset":
                guard controller.zoomEditing else { return fail(id, "The zoom level's field isn't open.") }
                controller.zoomReset.performClick(nil)
            case "page":
                // What a click on the page number calls.
                guard !controller.placeLabel.isHidden, let clicked = controller.placeLabel.clicked else { return fail(id, "The page number isn't showing.") }
                clicked()
            default: return fail(id, "That isn't one of the window's buttons.")
            }
            return true
        case "zoom_escape":
            // Escape in the field, by the method AppKit calls for it.
            guard let controller, controller.zoomEditing else { return fail(id, "The zoom level's field isn't open.") }
            _ = controller.control(controller.zoomField, textView: NSTextView(), doCommandBy: #selector(NSResponder.cancelOperation(_:)))
            return true
        case "page_type":
            guard let controller else { return fail(id, "The window is closed.") }
            if !controller.pageEditing {
                guard !controller.placeLabel.isHidden, let clicked = controller.placeLabel.clicked else { return fail(id, "The page number isn't showing.") }
                clicked()
            }
            controller.pageField.stringValue = value
            // Return, by the field's own action.
            controller.pageField.sendAction(controller.pageField.action, to: controller.pageField.target)
            controller.drainAll()
            return true
        case "page_escape":
            guard let controller, controller.pageEditing else { return fail(id, "The go-to field isn't open.") }
            _ = controller.control(controller.pageField, textView: NSTextView(), doCommandBy: #selector(NSResponder.cancelOperation(_:)))
            return true
        case "zoom_type":
            guard let controller else { return fail(id, "The window is closed.") }
            if !controller.zoomEditing {
                guard !controller.zoomButton.isHidden else { return fail(id, "The zoom level isn't showing.") }
                controller.zoomButton.performClick(nil)
            }
            controller.zoomField.stringValue = value
            // Return, by the field's own action.
            controller.zoomField.sendAction(controller.zoomField.action, to: controller.zoomField.target)
            controller.drainAll()
            return true
        default:
            return nil
        }
    }

    /// This area's part of the report.
    func controlsSection(controller: MainWindowController, overPages: (NSRect) -> Bool) -> [String: Any] {
        return [
            "zoom_level": ["text": controller.zoomButton.attributedTitle.string, "visible": !controller.zoomButton.isHidden, "editing": controller.zoomEditing, "field": controller.zoomEditing ? controller.zoomField.stringValue : NSNull() as Any, "reset": controller.zoomReset.isHidden ? NSNull() as Any : controller.zoomReset.attributedTitle.string, "in_the_top_right": controller.zoomButton.frame.maxX <= controller.root.bounds.maxX - 6 && controller.zoomButton.frame.minX > controller.root.bounds.maxX - 160 && controller.zoomButton.frame.minY < 30, "clear_of_agent_icon": controller.agentIcon.isHidden || (!controller.zoomButton.frame.intersects(controller.agentIcon.frame) && controller.zoomButton.frame.maxX - MainWindowController.zoomPadding <= controller.agentIcon.frame.minX - 4 && controller.zoomField.frame.maxX <= controller.agentIcon.frame.minX - 4), "in_the_agent_icons_row": abs(controller.zoomButton.frame.midY - controller.agentIcon.frame.midY) < 1, "right_margin": controller.root.bounds.maxX - (controller.zoomEditing ? controller.zoomField.frame.maxX : controller.zoomButton.frame.maxX - MainWindowController.zoomPadding), "reset_clear_of_field": controller.zoomReset.frame.maxX <= controller.zoomField.frame.minX, "clear_of_pages": controller.zoomButton.isHidden || !overPages(controller.zoomButton.frame.insetBy(dx: MainWindowController.zoomPadding, dy: 0)), "above_the_scroller": max(controller.zoomButton.frame.maxY, controller.zoomField.frame.maxY) + 4 <= controller.paneAreas[controller.paneAreas.count - 1].scrollView.scrollerInsets.top] as [String: Any],
            "note": controller.noteLabel.isHidden ? NSNull() as Any : controller.noteLabel.label.text,
            // Where the note's bubble is and how many lines it wraps to, so a check can hold its shape and not only its words.
            "note_box": controller.noteLabel.isHidden ? NSNull() as Any : controller.noteLabel.qaBox(in: controller.root),
            "copy_notice": controller.copyNotice.isHidden ? NSNull() as Any : ["text": controller.copyNotice.label.text, "button": controller.copyNotice.button.isHidden ? NSNull() as Any : controller.copyNotice.button.attributedTitle.string, "frame_ok": controller.root.bounds.contains(controller.copyNotice.frame), "box": controller.copyNotice.qaBox(in: controller.root)] as [String: Any],
            "place_label": controller.placeLabel.isHidden ? NSNull() as Any : controller.placeLabel.label.text,
            "links": ["hint": controller.linkHint.isHidden ? NSNull() as Any : controller.linkHint.label.text, "hint_in_lower_left": !controller.linkHint.isHidden && controller.linkHint.frame.minX < controller.root.bounds.midX && controller.linkHint.frame.maxY > controller.root.bounds.maxY - 80 && controller.root.bounds.contains(controller.linkHint.frame), "page_field": controller.pageEditing ? controller.pageField.stringValue : NSNull() as Any, "address_opened": delegate.addressOpenedForQA as Any? ?? NSNull()] as [String: Any],
            "place_label_in_lower_right": !controller.placeLabel.isHidden && controller.placeLabel.frame.maxX > controller.root.bounds.maxX - 40 && controller.placeLabel.frame.maxY > controller.root.bounds.maxY - 40 && controller.root.bounds.contains(controller.placeLabel.frame),
        ]
    }
}
