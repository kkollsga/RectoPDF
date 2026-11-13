import AppKit
import CryptoKit
import QuartzCore

/// Sheets, Settings, the notice, opening a file, printing, and the clipboard and Services.
extension QAChannel {
    /// The dialogs verbs. Answers `nil` for a verb that isn't one of them, `true` when the verb ends as most do (the window laid out, then the report), `false` when it has already answered or failed.
    func qaDialogs(_ id: UInt64, _ verb: String, _ value: String, _ controller: MainWindowController?) -> Bool? {
        switch verb {
        case "print_to_pdf":
            // `OUT` for every page, or `OUT|FIRST|LAST`, as Rust checked it.
            let parts = value.split(separator: "|").map(String.init)
            let (first, last) = parts.count == 3 ? (UInt32(parts[1]) ?? 0, UInt32(parts[2]) ?? 0) : (0, 0)
            guard delegate.printToPDFForQA(out: parts[0], first: first, last: last) else { return fail(id, "There's nothing to print.") }
            return true
        case "notice_button":
            guard let controller, !controller.copyNotice.isHidden, !controller.copyNotice.button.isHidden else { return fail(id, "No notice with a button is showing.") }
            controller.copyNotice.button.performClick(nil)
            return true
        case "open_file":
            _ = delegate.core.act(action: .open(path: value))
            return true
        case "sheet":
            delegate.core.sheetChanged(open: value == "open")
            return true
        case "settings":
            switch value {
            case "open": delegate.showSettings()
            case "close": delegate.settings?.window.close()
            case "privacy": delegate.settings?.privacyLink.performClick(nil)
            default:
                guard let toggle = delegate.settings?.control(value) else { return fail(id, "That switch isn't showing.") }
                toggle.performClick(nil)
            }
            return true
        case "sheet_answer":
            // The answer a sheet's button gives, through the sheet's own ending, as a click on it ends it: the same completion runs.
            let codes: [String: NSApplication.ModalResponse] = ["first": .alertFirstButtonReturn, "second": .alertSecondButtonReturn, "third": .alertThirdButtonReturn, "cancel": .cancel]
            guard let code = codes[value] else { return fail(id, "That isn't one of a sheet's answers.") }
            guard delegate.sheets.answer(code, over: controller?.window) else { return fail(id, "No sheet is open.") }
            return true
        case "password":
            // Types into the password sheet's secure field and presses Open, through the sheet's own ending, as a person's click ends it.
            guard let controller, let sheet = delegate.sheets.sheetWindow(over: controller.window), controller.passwordField.window === sheet else { return fail(id, "No password sheet is open.") }
            controller.passwordField.stringValue = value
            _ = delegate.sheets.answer(.alertFirstButtonReturn, over: controller.window)
            return true
        case "copied":
            // A type still promised is listed as such, and made only when the value names it, as a paste of it would. Which are promised is read first: reading any type in this process makes the system resolve the rest of the item's promises, which another app's paste doesn't (a second process reading the PNG and the JPEG left the SVG unmade, 2026-11-08), so it holds only for the first look after a copy.
            let promised = delegate.clipboardPromised
            let text = delegate.pasteboard.string(forType: .string)
            // Each type in the clipboard's order, with its size and a digest, so a test can say a JPEG went as its own bytes without the bytes crossing.
            let types = (delegate.pasteboard.types ?? []).map { type -> [String: Any] in
                if promised.contains(type.rawValue) && value != type.rawValue { return ["type": type.rawValue, "promised": true] }
                let data = delegate.pasteboard.data(forType: type) ?? Data()
                return ["type": type.rawValue, "bytes": data.count, "sha256": SHA256.hash(data: data).map { String(format: "%02x", $0) }.joined()]
            }
            answer(id, ["chars": text?.count ?? 0, "clipboard": delegate.pasteboard.name.rawValue, "types": types, "write": delegate.lastCopyWrite, "untrusted": ["text": text as Any? ?? NSNull()]] as [String: Any])
            return false
        case "services":
            // By the selector AppKit sends, to a clipboard nothing else can see.
            let requestor = delegate.validRequestor(forSendType: .string, returnType: nil) as? NSServicesMenuRequestor
            let board = NSPasteboard(name: NSPasteboard.Name("com.rectopdf.qa.services.\(ProcessInfo.processInfo.processIdentifier)"))
            let written = requestor?.writeSelection?(to: board, types: [.string]) ?? false
            let text = board.string(forType: .string)
            board.releaseGlobally()
            answer(id, ["offers_text": requestor != nil, "written": written, "untrusted": ["text": text as Any? ?? NSNull()]] as [String: Any])
            return false
        default:
            return nil
        }
    }

    /// This area's part of the report.
    func settingsSection() -> [String: Any] {
        return [
            "settings": delegate.settings?.report() ?? ["open": false],
        ]
    }
}
