import AppKit
import QuartzCore

/// QA commands, on the UI thread. Each calls what the UI calls, posts no event, and answers once the app has done it and laid out. A shot is drawn in the app, so it needs no permission, no display and no focus.
@MainActor final class QAChannel {
    private unowned let delegate: AppDelegate
    private var stepCarry = 0.0
    /// Set by `recto qa scroller-style`, for each style on any Mac. With none, what the system says.
    private(set) var scrollerStyle: NSScroller.Style?

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
            delegate.core.setAppearanceChoice(choice: value)
            controller?.appearanceChanged()
        case "full_screen":
            controller?.setQuietFullScreen(value == "on")
        case "focus":
            controller?.applyFocus(value)
        case "frontmost":
            delegate.core.frontmostChanged(app: value == "on", windowKey: value == "on")
        case "sheet":
            delegate.core.sheetChanged(open: value == "open")
        case "sidebar_width":
            delegate.core.setSidebarWidth(width: Double(value) ?? 0)
        case "quit":
            answer(id, ["quitting": true])
            DispatchQueue.main.async { NSApp.terminate(nil) }
            return
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
        case "menu":
            guard let item = menuItem(value), let action = item.action else { return fail(id, "That isn't on the menu bar.") }
            NSApp.sendAction(action, to: item.target, from: item)
        case "panel":
            guard let sidebar = controller?.sidebar else { return fail(id, "The window is closed.") }
            guard let button = value == "toggle" ? sidebar.moreButton : sidebar.panel?.buttons[value] else { return fail(id, "That isn't in the panel, or it isn't open.") }
            button.performClick(nil)
        case "scroller_style":
            scrollerStyle = value == "legacy" ? .legacy : .overlay
            controller?.layout()
        case "reduce_motion":
            controller?.sidebar.reduceMotion = value == "on"
        case "reveal":
            controller?.sidebar.revealRow(value == "none" ? nil : Int(value))
        case "resize":
            let parts = value.split(separator: "x").compactMap { Double($0) }
            guard parts.count == 2 else { return fail(id, "That isn't a size.") }
            controller?.window.setFrame(NSRect(x: -40_000, y: -40_000, width: parts[0], height: parts[1]), display: false)
            controller?.layout()
            controller?.noteWindow()
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

    /// An item of the real menu bar, by its id.
    private func menuItem(_ id: String) -> NSMenuItem? {
        NSApp.mainMenu?.items.compactMap(\.submenu).flatMap(\.items).first { ($0.representedObject as? String) == id && !$0.isHidden }
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
            "window": ["x": frame.minX, "y": frame.minY, "width": frame.width, "height": frame.height, "full_screen": controller.isFullScreen, "on_any_screen": NSScreen.screens.contains { $0.frame.intersects(frame) }, "is_visible": controller.window.isVisible],
            "is_active": NSApp.isActive,
            "activation": policyName(),
            "appearance": Theme.isDark(controller.root) ? "dark" : "light",
            "focus": reader.focus,
            "home": !controller.home.isHidden,
            "sidebar_visible": !sidebar.view.isHidden,
            "sidebar_width": sidebar.view.frame.width,
            "focus_view": delegate.core.focusView(),
            "sidebar_rows": sidebar.rowCount,
            "reveal": sidebar.revealReport(),
            "paper": reader.paper,
            "contents": sidebar.contentsReport(),
            "hover": sidebar.hoveredRow.map { $0 as Any } ?? NSNull(),
            "place": reader.place,
            "note": controller.noteLabel.isHidden ? NSNull() as Any : controller.noteLabel.label.text,
            "place_label": controller.placeLabel.isHidden ? NSNull() as Any : controller.placeLabel.label.text,
            "agent_icon": !controller.agentIcon.isHidden,
            "panel": sidebar.panelReport(),
            "menu_bar": (NSApp.mainMenu?.items ?? []).compactMap(\.submenu).map { menu -> [String: Any] in
                ["title": menu.title, "items": menu.items.filter { !$0.isSeparatorItem && !$0.isHidden }.map { ["id": ($0.representedObject as? String) ?? "", "title": $0.title, "key": $0.keyEquivalent, "checked": $0.state == .on] as [String: Any] }]
            },
            "pages": pages,
            "backing": controller.window.backingScaleFactor,
        ]
    }

    /// What AppKit reaches this app by, which no other QA verb goes through: they call the method behind it. Events are made here and handed straight to the responder that would get them, by the selector AppKit sends, and what's checked is what comes of it. Nothing is posted, and nothing outside this process hears. A real pinch or click can't be made this way, so for those it's the notification, and that the target answers the action.
    private func entryPoints() -> [String: Bool] {
        guard let controller = delegate.windowController else { return ["window_open": false] }
        let sidebar = controller.sidebar
        let pages = controller.pages
        var found: [String: Bool] = ["window_open": true]
        let entered = #selector(NSResponder.mouseEntered(with:))
        let exited = #selector(NSResponder.mouseExited(with:))
        let moved = #selector(NSResponder.mouseMoved(with:))
        let wheel = #selector(NSResponder.scrollWheel(with:))
        // These two aren't responders, so nothing but their own code answers these, and tracking areas call them by name. Going by a selector nothing answers would throw, and this would never answer.
        let owners: [(String, NSObject, Selector)] = [("sidebar_hover_in", sidebar, entered), ("sidebar_hover_out", sidebar, exited), ("sidebar_pointer_moved", sidebar, moved), ("corner_hover_in", controller, entered), ("corner_hover_out", controller, exited)]
        for (name, owner, selector) in owners { found[name] = owner.responds(to: selector) }
        guard owners.allSatisfy({ $0.1.responds(to: $0.2) }) else { return found }
        let actions: [(String, NSControl)] = [("tab_row_click", sidebar.table), ("contents_row_click", sidebar.contentsTable), ("add_button", sidebar.addButton), ("home_button", controller.homeButtonForQA)]
        for (name, control) in actions {
            found[name] = control.action.flatMap { action in (control.target as? NSObject)?.responds(to: action) } ?? false
        }
        // The areas that send them are really there, and over what they're for.
        found["sidebar_area_is_on_the_sidebar"] = sidebar.view.trackingAreas.contains { $0.owner === sidebar && $0.options.contains(.mouseMoved) && $0.options.contains(.inVisibleRect) }
        sidebar.contentsScroll.updateTrackingAreas()
        found["contents_area_is_on_the_contents"] = sidebar.contentsScroll.trackingAreas.contains { $0.owner === sidebar.contentsScroll && $0.options.contains(.inVisibleRect) }
        let corner = controller.root.trackingAreas.first { $0.owner === controller }?.rect
        found["corner_covers_the_buttons"] = corner.map { rect in controller.windowButtonFrames(in: controller.root).allSatisfy { rect.contains($0) } } ?? false

        let window = controller.window
        func crossing(_ type: NSEvent.EventType) -> NSEvent? {
            NSEvent.enterExitEvent(with: type, location: .zero, modifierFlags: [], timestamp: 0, windowNumber: window.windowNumber, context: nil, eventNumber: 0, trackingNumber: 0, userData: nil)
        }
        func pointer(at place: NSPoint) -> NSEvent? {
            NSEvent.mouseEvent(with: .mouseMoved, location: place, modifierFlags: [], timestamp: 0, windowNumber: window.windowNumber, context: nil, eventNumber: 0, clickCount: 0, pressure: 0)
        }
        sidebar.perform(entered, with: crossing(.mouseEntered))
        found["hover_shows_details"] = sidebar.hoveredRow != nil
        sidebar.perform(moved, with: pointer(at: sidebar.view.convert(NSPoint(x: 100, y: 70), to: nil)))
        found["pointer_finds_a_tab_row"] = sidebar.lastPointerRow != nil

        // The contents: the pointer coming in, as their own area says it does.
        sidebar.contentsScroll.perform(entered, with: crossing(.mouseEntered))
        found["contents_hover_shows_their_scroller"] = sidebar.contentsScroll.hasVerticalScroller
        // A shortened contents entry, by where the pointer is, and left to rest as a real pointer would be, on the real timer.
        if let place = sidebar.cutOffContentsPlaceInWindow() {
            sidebar.perform(moved, with: pointer(at: place.point))
            found["pointer_finds_the_contents_row"] = sidebar.lastPointerRow == place.row
            RunLoop.current.run(until: Date(timeIntervalSinceNow: delegate.limits.revealDelaySeconds + 0.3))
            let report = sidebar.revealReport()
            found["contents_hover_reveals_the_full_name"] = (report["shown"] as? Bool) == true && (report["row"] as? Int) == place.row
        }
        sidebar.contentsScroll.perform(exited, with: crossing(.mouseExited))
        found["contents_hover_ends"] = !sidebar.contentsScroll.hasVerticalScroller
        sidebar.perform(exited, with: crossing(.mouseExited))
        found["hover_ends"] = sidebar.hoveredRow == nil && sidebar.lastPointerRow == nil
        found["reveal_goes_with_the_pointer"] = (sidebar.revealReport()["shown"] as? Bool) != true

        // A wheel, made and handed to what a wheel reaches. With its place and window said, as a real one has them: left to itself it's wherever the real pointer is, on this Mac, over some other window.
        func wheelEvent(_ points: Int32, _ flags: CGEventFlags, at place: NSPoint) -> NSEvent? {
            guard let made = CGEvent(scrollWheelEvent2Source: nil, units: .pixel, wheelCount: 1, wheel1: points, wheel2: 0, wheel3: 0) else { return nil }
            made.flags = flags
            // As two fingers send it. A mouse's wheel can be eased by AppKit on the display's beat, which a quiet run doesn't have.
            made.setIntegerValueField(.scrollWheelEventIsContinuous, value: 1)
            let onScreen = window.convertPoint(toScreen: place)
            made.location = CGPoint(x: onScreen.x, y: (NSScreen.screens.first?.frame.height ?? 0) - onScreen.y)
            made.setIntegerValueField(.mouseEventWindowUnderMousePointer, value: Int64(window.windowNumber))
            made.setIntegerValueField(.mouseEventWindowUnderMousePointerThatCanHandleThisEvent, value: Int64(window.windowNumber))
            return NSEvent(cgEvent: made)
        }
        let overPages = pages.scrollView.convert(NSPoint(x: pages.scrollView.bounds.midX + 100, y: pages.scrollView.bounds.midY - 80), to: nil)
        if let event = wheelEvent(-120, [], at: overPages) {
            // That it's handed to AppKit's own scrolling, unzoomed. What AppKit does with it is off the display's own thread, which a window off every screen doesn't have, so the scroll itself isn't seen here.
            let (passed, zoom) = (pages.scrollView.wheelsPassedOn, pages.scrollView.magnification)
            pages.scrollView.perform(wheel, with: event)
            found["plain_wheel_goes_to_the_scroll_view"] = pages.scrollView.wheelsPassedOn == passed + 1 && pages.scrollView.magnification == zoom
            if sidebar.model.contentsCount > 0, sidebar.model.tocOpen {
                sidebar.contentsScroll.perform(wheel, with: event)
                found["wheel_in_contents_stops_following"] = !delegate.core.contentsIsFollowing()
            }
        }
        // Zoom from the keyboard, through the real menu bar and the real responder, and ⌘-scroll about the pointer.
        func key(_ characters: String, _ flags: NSEvent.ModifierFlags) -> NSEvent? {
            NSEvent.keyEvent(with: .keyDown, location: .zero, modifierFlags: flags, timestamp: 0, windowNumber: window.windowNumber, context: nil, characters: characters, charactersIgnoringModifiers: characters, isARepeat: false, keyCode: 0)
        }
        func zoom() -> Double {
            pages.drain()
            return pages.scrollView.magnification
        }
        func pressed(_ characters: String) -> Double {
            if let event = key(characters, .command), NSApp.mainMenu?.performKeyEquivalent(with: event) != true { return -1 }
            return zoom()
        }
        var was = zoom()
        var now = pressed("+")
        found["command_plus_zooms_in"] = now > was
        was = now
        now = pressed("-")
        found["command_minus_zooms_out"] = now < was
        now = pressed("9")
        found["command_9_fits"] = now == 1
        now = pressed("0")
        found["command_0_is_actual_size"] = now > 0 && abs(now - 1 / (delegate.engineFit())) < 0.01
        _ = pressed("9")
        controller.root.perform(#selector(NSResponder.keyDown(with:)), with: key("+", []))
        now = zoom()
        found["plain_plus_zooms_in"] = now > 1
        controller.root.perform(#selector(NSResponder.keyDown(with:)), with: key("-", []))
        found["plain_minus_zooms_out"] = zoom() < now
        _ = pressed("9")
        if let event = wheelEvent(60, .maskCommand, at: overPages) {
            let at = pages.documentView.convert(overPages, from: nil)
            let before = pages.scrollView.magnification
            pages.scrollView.perform(wheel, with: event)
            found["command_scroll_zooms"] = pages.scrollView.magnification > before
            let after = pages.documentView.convert(overPages, from: nil)
            found["command_scroll_keeps_the_point_still"] = abs(after.x - at.x) < 2 && abs(after.y - at.y) < 2
        }
        _ = pressed("9")
        found["more_button"] = sidebar.moreButton.action.flatMap { action in (sidebar.moreButton.target as? NSObject)?.responds(to: action) } ?? false
        // The panel, by clicking the real buttons. It's left as it was.
        sidebar.moreButton.performClick(nil)
        RunLoop.current.run(until: Date(timeIntervalSinceNow: 0.05))
        let opened = (sidebar.panelReport()["open"] as? Bool) == true
        found["panel_opens"] = opened
        if opened, let warm = sidebar.panel?.buttons["paper.warm"] {
            found["panel_button_answers"] = warm.action.flatMap { action in (warm.target as? NSObject)?.responds(to: action) } ?? false
        }
        sidebar.moreButton.performClick(nil)
        RunLoop.current.run(until: Date(timeIntervalSinceNow: 0.05))
        found["panel_closes"] = (sidebar.panelReport()["open"] as? Bool) == false
        // A pinch comes with these from AppKit, and they're what stops drawing until it ends.
        found["pages_can_be_pinched"] = pages.scrollView.allowsMagnification
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
