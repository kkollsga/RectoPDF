import AppKit
import CryptoKit
import QuartzCore

/// Find: typing, its keys and its list.
extension QAChannel {
    /// The find verbs. Answers `nil` for a verb that isn't one of them, `true` when the verb ends as most do (the window laid out, then the report), `false` when it has already answered or failed.
    func qaFind(_ id: UInt64, _ verb: String, _ value: String, _ controller: MainWindowController?) -> Bool? {
        switch verb {
        case "find_type":
            // What typing in the field does: the field holds it, and its change goes where a keystroke's does.
            guard let find = controller?.sidebar.find, find.model.open else { return fail(id, "Find isn't open. `recto qa menu find.open` opens it.") }
            find.field.stringValue = value
            find.typed(value)
            return true
        case "find_key":
            // Escape in the field, by the method AppKit calls for it, as `page_escape` is. Rust has checked it's the one key this takes.
            guard let find = controller?.sidebar.find, find.model.open else { return fail(id, "Find isn't open.") }
            _ = find.control(find.field, textView: NSTextView(), doCommandBy: #selector(NSResponder.cancelOperation(_:)))
            return true
        case "find_click":
            guard let find = controller?.sidebar.find, find.model.open, let row = Int(value), row >= 0, row < Int(find.model.rowCount) else { return fail(id, "There's no such row in Find's list.") }
            find.clickRow(row)
            return true
        default:
            return nil
        }
    }

    /// This area's part of the report.
    func findSection(controller: MainWindowController) -> [String: Any] {
        let sidebar = controller.sidebar
        return [
            "find": sidebar.find.report(),
        ]
    }
}
