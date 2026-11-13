import AppKit
import CryptoKit
import QuartzCore

/// The sidebar: tabs, contents, the palette, the panel and the reveal.
extension QAChannel {
    /// The sidebar verbs. Answers `nil` for a verb that isn't one of them, `true` when the verb ends as most do (the window laid out, then the report), `false` when it has already answered or failed.
    func qaSidebar(_ id: UInt64, _ verb: String, _ value: String, _ controller: MainWindowController?) -> Bool? {
        switch verb {
        case "compare_click":
            controller?.sidebar.compareClickRow(Int(value) ?? 0)
            return true
        case "click_row":
            if value == "note" { controller?.sidebar.clickNote() } else if value.hasPrefix("fold:") { controller?.sidebar.clickFold(Int(value.dropFirst(5)) ?? -1) } else { controller?.sidebar.clickRow(Int(value) ?? 0) }
            return true
        case "contents_scroll":
            controller?.sidebar.userScrollContents(toRow: Int(value) ?? 0)
            return true
        case "palette_type":
            // Rust has checked it's open, and says so in its words if not.
            guard let palette = controller?.sidebar.palette else { return false }
            palette.field.stringValue = value
            palette.typed(value)
            return true
        case "palette_key":
            guard let palette = controller?.sidebar.palette else { return false }
            palette.press(value)
            return true
        case "panel":
            guard let sidebar = controller?.sidebar else { return fail(id, "The window is closed.") }
            guard let button = value == "toggle" ? sidebar.moreButton : sidebar.panel?.buttons[value] else { return fail(id, "That isn't in the panel, or it isn't open.") }
            button.performClick(nil)
            return true
        case "reveal":
            controller?.sidebar.revealRow(value == "none" ? nil : Int(value))
            return true
        case "sidebar_width":
            delegate.core.setSidebarWidth(width: Double(value) ?? 0)
            return true
        default:
            return nil
        }
    }

    /// This area's part of the report.
    func sidebarSection(controller: MainWindowController, reader: ReaderView) -> [String: Any] {
        let sidebar = controller.sidebar
        return [
            "sidebar_hidden": reader.sidebarHidden,
            "sidebar_visible": !sidebar.view.isHidden,
            "sidebar_width": sidebar.view.frame.width,
            "focus_view": delegate.core.focusView(),
            "sidebar_rows": sidebar.rowCount,
            "reveal": sidebar.revealReport(),
            "contents": sidebar.contentsReport(),
            "palette": sidebar.palette.report(),
            "hover": sidebar.hoveredRow.map { $0 as Any } ?? NSNull(),
            "panel": sidebar.panelReport(),
        ]
    }
}
