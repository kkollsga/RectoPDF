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
        case "entry_points":
            answer(id, entryPoints())
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
            controller?.sidebar.clickRow(Int(value) ?? 0)
        case "contents_scroll":
            controller?.sidebar.userScrollContents(toRow: Int(value) ?? 0)
        case "contents_pointer":
            controller?.sidebar.setContentsPointer(value == "in")
        case "reduce_motion":
            controller?.sidebar.reduceMotion = value == "on"
        case "reveal":
            controller?.sidebar.revealRow(value == "none" ? nil : Int(value))
        case "resize":
            let parts = value.split(separator: "x").compactMap { Double($0) }
            guard parts.count == 2 else { return fail(id, "That isn't a size.") }
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
            "reveal": sidebar.revealReport(),
            "paper": reader.paper,
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

    /// What AppKit reaches this app by, which no other QA verb goes through: they call the method behind it. That the owner responds to the selector AppKit will send, and for those that can be, that going by the selector does what it should. Events are made here and handed straight to the method: nothing is posted, and nothing outside this process hears. A real pinch or click can't be made this way, so for those it's the selector and the notification.
    private func entryPoints() -> [String: Bool] {
        guard let controller = delegate.windowController else { return ["window_open": false] }
        let sidebar = controller.sidebar
        let pages = controller.pages
        var found: [String: Bool] = ["window_open": true]
        let entered = #selector(NSResponder.mouseEntered(with:))
        let exited = #selector(NSResponder.mouseExited(with:))
        let moved = #selector(NSResponder.mouseMoved(with:))
        let wheel = #selector(NSResponder.scrollWheel(with:))
        let owners: [(String, NSObject, Selector)] = [
            ("sidebar_hover_in", sidebar, entered), ("sidebar_hover_out", sidebar, exited), ("sidebar_pointer_moved", sidebar, moved),
            ("corner_hover_in", controller, entered), ("corner_hover_out", controller, exited),
            ("contents_hover_in", sidebar.contentsScroll, entered), ("contents_hover_out", sidebar.contentsScroll, exited), ("contents_wheel", sidebar.contentsScroll, wheel),
            ("pages_wheel", pages.scrollView, wheel), ("pages_magnify", pages.scrollView, #selector(NSResponder.magnify(with:))),
        ]
        for (name, owner, selector) in owners { found[name] = owner.responds(to: selector) }
        // Going by a selector nothing answers would throw, and this would never answer. What's failing is reported above.
        guard owners.allSatisfy({ $0.1.responds(to: $0.2) }) else { return found }
        let actions: [(String, NSControl)] = [("tab_row_click", sidebar.table), ("contents_row_click", sidebar.contentsTable), ("add_button", sidebar.addButton), ("home_button", controller.homeButtonForQA)]
        for (name, control) in actions {
            found[name] = control.action.flatMap { action in (control.target as? NSObject)?.responds(to: action) } ?? false
        }
        // Through the selector, as AppKit would.
        let window = controller.window
        func crossing(_ type: NSEvent.EventType) -> NSEvent? {
            NSEvent.enterExitEvent(with: type, location: .zero, modifierFlags: [], timestamp: 0, windowNumber: window.windowNumber, context: nil, eventNumber: 0, trackingNumber: 0, userData: nil)
        }
        sidebar.perform(entered, with: crossing(.mouseEntered))
        found["hover_shows_details"] = sidebar.hoveredRow != nil
        let row = NSPoint(x: 100, y: sidebar.view.bounds.height - 110)
        let pointer = NSEvent.mouseEvent(with: .mouseMoved, location: sidebar.view.convert(row, to: nil), modifierFlags: [], timestamp: 0, windowNumber: window.windowNumber, context: nil, eventNumber: 0, clickCount: 0, pressure: 0)
        sidebar.perform(moved, with: pointer)
        found["pointer_finds_row"] = sidebar.lastPointerRow != nil
        sidebar.perform(exited, with: crossing(.mouseExited))
        found["hover_ends"] = sidebar.hoveredRow == nil && sidebar.lastPointerRow == nil
        // A wheel, made and handed over.
        let wheelEvent = CGEvent(scrollWheelEvent2Source: nil, units: .pixel, wheelCount: 1, wheel1: -120, wheel2: 0, wheel3: 0).flatMap { NSEvent(cgEvent: $0) }
        if let wheelEvent {
            let before = pages.scrollView.contentView.bounds.minY
            pages.scrollView.perform(wheel, with: wheelEvent)
            found["wheel_scrolls_pages"] = pages.scrollView.contentView.bounds.minY != before
            if sidebar.model.contentsCount > 0, sidebar.model.tocOpen {
                sidebar.contentsScroll.perform(wheel, with: wheelEvent)
                found["wheel_in_contents_stops_following"] = !delegate.core.contentsIsFollowing()
            }
        }
        // A pinch comes with these from AppKit, and they're what stops drawing until it ends.
        NotificationCenter.default.post(name: NSScrollView.willStartLiveMagnifyNotification, object: pages.scrollView)
        found["pinch_starts"] = pages.isLive
        NotificationCenter.default.post(name: NSScrollView.didEndLiveMagnifyNotification, object: pages.scrollView)
        found["pinch_ends"] = !pages.isLive
        // Focus view: the window's buttons, hidden, come back with the pointer near them.
        if delegate.core.reader().focus {
            controller.perform(entered, with: crossing(.mouseEntered))
            found["buttons_come_back"] = controller.windowButtonsShown
            controller.perform(exited, with: crossing(.mouseExited))
            found["buttons_go_again"] = !controller.windowButtonsShown
        }
        return found
    }

    private func policyName() -> String {
        switch NSApp.activationPolicy() {
        case .regular: "regular"
        case .accessory: "accessory"
        default: "prohibited"
        }
    }

    /// Warm paper in a shot: the sum the compositor does, on the picture. `render(in:)` leaves compositing filters out.
    nonisolated private static func multiply(_ rep: NSBitmapImageRep, by rgb: UInt32, in rects: [NSRect], viewHeight: CGFloat, flipped: Bool) {
        guard let data = rep.bitmapData else { return }
        let (red, green, blue) = (Int((rgb >> 16) & 0xFF), Int((rgb >> 8) & 0xFF), Int(rgb & 0xFF))
        let (stride, width, height) = (rep.bytesPerRow, rep.pixelsWide, rep.pixelsHigh)
        for rect in rects {
            let top = flipped ? rect.minY : viewHeight - rect.maxY
            let (x0, x1) = (max(Int(rect.minX * 2), 0), min(Int(rect.maxX * 2), width))
            let (y0, y1) = (max(Int(top * 2), 0), min(Int((top + rect.height) * 2), height))
            guard x0 < x1, y0 < y1 else { continue }
            for y in y0..<y1 {
                let row = data + y * stride
                for x in x0..<x1 {
                    let pixel = row + x * 4
                    pixel[0] = UInt8(Int(pixel[0]) * red / 255)
                    pixel[1] = UInt8(Int(pixel[1]) * green / 255)
                    pixel[2] = UInt8(Int(pixel[2]) * blue / 255)
                }
            }
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
        let tint = controller.pages.tintRectsForShot(in: view)
        controller.pages.hideTintForShot(true)
        CATransaction.flush()
        view.cacheDisplay(in: bounds, to: rep)
        controller.pages.hideTintForShot(false)
        let flipped = view.isFlipped
        let core = delegate.core
        // The encoding and the write are slow, so they stay off this thread.
        nonisolated(unsafe) let encoded = rep
        DispatchQueue.global(qos: .userInitiated).async {
            if let tint { QAChannel.multiply(encoded, by: tint.rgb, in: tint.rects, viewHeight: bounds.height, flipped: flipped) }
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
