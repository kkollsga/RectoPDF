import AppKit
import QuartzCore

/// QA commands, on the UI thread. Each calls what the UI calls, posts no event, and answers once the app has done it and laid out. A shot is drawn in the app, so it needs no permission, no display and no focus.
@MainActor final class QAChannel {
    private unowned let delegate: AppDelegate
    private var stepCarry = 0.0

    init(delegate: AppDelegate) {
        self.delegate = delegate
    }

    /// The window's frame and buttons take the key look from being told, which reaches nothing outside this process.
    static func drawActive(_ window: NSWindow) {
        window.becomeMain()
        window.becomeKey()
    }

    private func answer(_ id: UInt64, _ value: Any) {
        guard id != 0 else { return }
        let data = (try? JSONSerialization.data(withJSONObject: value, options: [.fragmentsAllowed])) ?? Data("null".utf8)
        delegate.core.qaAnswer(id: id, ok: true, json: String(data: data, encoding: .utf8) ?? "null")
    }

    private func fail(_ id: UInt64, _ words: String) {
        delegate.core.qaAnswer(id: id, ok: false, json: words)
    }

    func handle(id: UInt64, verb: String, value: String) {
        let controller = delegate.windowController
        switch verb {
        case "scroll_step":
            // Time spent here is the UI thread's share of a frame: the scroll, Rust's call, and applying what it said.
            let started = CACurrentMediaTime()
            controller?.pages.scroll(by: Double(value) ?? 0)
            let took = CACurrentMediaTime() - started
            let contents = controller?.sidebar.contentsSample() ?? (y: 0, currentVisible: true)
            delegate.core.qaNoteStep(micros: UInt64(took * 1_000_000), contentsY: contents.y, currentVisible: contents.currentVisible)
            return
        case "state":
            answer(id, state())
            return
        case "shot":
            shot(id: id, path: value)
            return
        case "appearance":
            NSApp.appearance = NSAppearance(named: value == "dark" ? .darkAqua : .aqua)
            controller?.appearanceChanged()
        case "scroll":
            controller?.pages.scroll(by: Double(value) ?? 0)
        case "zoom":
            controller?.pages.setZoom(Double(value) ?? 1)
        case "hover":
            controller?.sidebar.setHover(value == "none" ? nil : Int(value))
        case "click_row":
            guard let row = Int(value) else { return fail(id, "`\(value)` isn't a row.") }
            controller?.sidebar.clickRow(row)
        case "contents_scroll":
            guard let row = Int(value), row >= 0 else { return fail(id, "`\(value)` isn't a row.") }
            controller?.sidebar.userScrollContents(toRow: row)
        case "contents_pointer":
            controller?.sidebar.setContentsPointer(value == "in")
        case "reduce_motion":
            controller?.sidebar.reduceMotion = value == "on"
        case "resize":
            let parts = value.split(separator: "x").compactMap { Double($0) }
            guard parts.count == 2, parts[0] >= 400, parts[1] >= 300, parts[0] <= 8000, parts[1] <= 8000 else { return fail(id, "`\(value)` isn't a size. It's WIDTHxHEIGHT.") }
            controller?.window.setFrame(NSRect(x: -40_000, y: -40_000, width: parts[0], height: parts[1]), display: false)
            controller?.layout()
        case "window":
            if value == "close" {
                controller?.window.close()
            } else {
                delegate.openWindow()
            }
        case "stall":
            Thread.sleep(forTimeInterval: (Double(value) ?? 0) / 1000)
        default:
            return fail(id, "That isn't a QA command.")
        }
        delegate.windowController?.window.layoutIfNeeded()
        answer(id, state())
    }

    private func state() -> [String: Any] {
        guard let controller = delegate.windowController else {
            return ["window": NSNull(), "window_open": false, "is_active": NSApp.isActive, "activation": policyName()]
        }
        let reader = delegate.core.reader()
        let sidebar = controller.sidebar
        var pages = controller.pages.report()
        pages["visible"] = !controller.pages.scrollView.isHidden
        let frame = controller.window.frame
        return [
            "window_open": true,
            "window": ["width": frame.width, "height": frame.height, "on_any_screen": NSScreen.screens.contains { $0.frame.intersects(frame) }, "is_visible": controller.window.isVisible],
            "is_active": NSApp.isActive,
            "activation": policyName(),
            "appearance": Theme.isDark(controller.root) ? "dark" : "light",
            "focus": reader.focus,
            "home": !controller.home.isHidden,
            "sidebar_visible": !sidebar.view.isHidden,
            "sidebar_rows": sidebar.rowCount,
            "contents": sidebar.contentsReport(),
            "hover": sidebar.hoveredRow.map { $0 as Any } ?? NSNull(),
            "place": reader.place,
            "note": controller.noteLabel.isHidden ? NSNull() as Any : controller.noteLabel.label.text,
            "place_label": controller.placeLabel.isHidden ? NSNull() as Any : controller.placeLabel.label.text,
            "agent_icon": !controller.agentIcon.isHidden,
            "pages": pages,
            "backing": controller.window.backingScaleFactor,
        ]
    }

    private func policyName() -> String {
        switch NSApp.activationPolicy() {
        case .regular: "regular"
        case .accessory: "accessory"
        default: "prohibited"
        }
    }

    private func shot(id: UInt64, path: String) {
        guard let controller = delegate.windowController else { return fail(id, "The window is closed, so there's nothing to draw.") }
        controller.window.layoutIfNeeded()
        controller.root.layoutSubtreeIfNeeded()
        // The frame view, so the window's buttons are in it.
        let view: NSView = controller.root.superview ?? controller.root
        let bounds = view.bounds
        guard let rep = NSBitmapImageRep(bitmapDataPlanes: nil, pixelsWide: Int(bounds.width * 2), pixelsHigh: Int(bounds.height * 2), bitsPerSample: 8, samplesPerPixel: 4, hasAlpha: true, isPlanar: false, colorSpaceName: .deviceRGB, bytesPerRow: 0, bitsPerPixel: 0) else { return fail(id, "The picture couldn't be made.") }
        rep.size = bounds.size
        CATransaction.flush()
        view.cacheDisplay(in: bounds, to: rep)
        let core = delegate.core
        // The encoding and the write are slow, so they stay off this thread.
        nonisolated(unsafe) let encoded = rep
        DispatchQueue.global(qos: .userInitiated).async {
            guard let data = encoded.representation(using: .png, properties: [:]) else {
                core.qaAnswer(id: id, ok: false, json: "The picture couldn't be encoded.")
                return
            }
            do {
                try data.write(to: URL(fileURLWithPath: path), options: .atomic)
                let answer = try JSONSerialization.data(withJSONObject: ["out": path, "width": encoded.pixelsWide, "height": encoded.pixelsHigh, "bytes": data.count])
                core.qaAnswer(id: id, ok: true, json: String(data: answer, encoding: .utf8) ?? "null")
            } catch {
                core.qaAnswer(id: id, ok: false, json: "The picture couldn't be written: \(error.localizedDescription)")
            }
        }
    }
}
