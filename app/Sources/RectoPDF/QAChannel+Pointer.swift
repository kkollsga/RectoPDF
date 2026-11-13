import AppKit
import CryptoKit
import QuartzCore

/// The pointer and scrolling in the page area, and the hover and corner marks.
extension QAChannel {
    /// The pointer verbs. Answers `nil` for a verb that isn't one of them, `true` when the verb ends as most do (the window laid out, then the report), `false` when it has already answered or failed.
    func qaPointer(_ id: UInt64, _ verb: String, _ value: String, _ controller: MainWindowController?) -> Bool? {
        switch verb {
        case "scroll_step":
            // Time spent here is the UI thread's share of a frame: the scroll, Rust's call, and applying what it said.
            let started = CACurrentMediaTime()
            let cpuStarted = clock_gettime_nsec_np(CLOCK_THREAD_CPUTIME_ID)
            controller?.pages.scroll(by: Double(value) ?? 0)
            let took = CACurrentMediaTime() - started
            let cpu = clock_gettime_nsec_np(CLOCK_THREAD_CPUTIME_ID) - cpuStarted
            let contents = controller?.sidebar.contentsSample() ?? (y: 0, currentVisible: true)
            delegate.core.qaNoteStep(micros: UInt64(took * 1_000_000), cpuMicros: cpu / 1000, contentsY: contents.y, currentVisible: contents.currentVisible)
            return false
        case "mouse_down", "mouse_drag", "pointer":
            guard let controller else { return fail(id, "The window is closed.") }
            let parts = value.split(separator: ",").compactMap { Double($0) }
            guard parts.count == 4 else { return fail(id, "That isn't a place.") }
            // The pane being read, as Rust worked the place out in it.
            let pane = controller.readingPane
            let inWindow = pane.documentView.convert(NSPoint(x: parts[0], y: parts[1]), to: nil)
            switch verb {
            case "mouse_down": pane.pointerDown(atWindow: inWindow, clicks: Int(parts[2]), shift: parts[3] != 0)
            case "mouse_drag": pane.pointerDragged(atWindow: inWindow)
            default: pane.pointerMoved(atWindow: inWindow)
            }
            return true
        case "mouse_up":
            controller?.readingPane.pointerUp()
            return true
        case "scroll":
            controller?.readingPane.scroll(by: Double(value) ?? 0)
            return true
        case "zoom":
            controller?.readingPane.setZoom(Double(value) ?? 1)
            return true
        case "swipe":
            // What the system's swipe calls, with its sideways amount.
            guard let controller, let delta = Double(value) else { return fail(id, "That isn't a swipe's sideways amount.") }
            controller.readingPane.swiped(deltaX: delta)
            return true
        case "corner_pointer":
            // `top-left:in`, `lower-right:out` and so on, by what the corner's tracking area calls.
            let parts = value.split(separator: ":").map(String.init)
            guard let controller, parts.count == 2, let mark = ChromeFader.mark(named: parts[0]) else { return fail(id, "That isn't a corner and in or out.") }
            controller.chrome.pointer(mark, inside: parts[1] == "in")
            return true
        case "contents_pointer":
            controller?.sidebar.setContentsPointer(value == "in")
            return true
        case "hover":
            controller?.sidebar.setHover(value == "none" ? nil : Int(value))
            return true
        default:
            return nil
        }
    }
}
