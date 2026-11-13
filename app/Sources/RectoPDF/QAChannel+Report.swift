import AppKit
import CryptoKit
import QuartzCore

/// The report, the running animations and the entry points.
extension QAChannel {
    /// The report verbs. Answers `nil` for a verb that isn't one of them, `true` when the verb ends as most do (the window laid out, then the report), `false` when it has already answered or failed.
    func qaReport(_ id: UInt64, _ verb: String, _ value: String, _ controller: MainWindowController?) -> Bool? {
        switch verb {
        case "state":
            readyToBeLookedAt()
            answer(id, state())
            return false
        case "animations":
            // Cheap, as `qa wait-settled` asks it until there's nothing.
            readyToBeLookedAt()
            answer(id, controller?.runningAnimations() ?? [])
            return false
        case "entry_points":
            answer(id, entryPoints())
            return false
        default:
            return nil
        }
    }

    /// What AppKit reaches this app by, which no other QA verb goes through: they call the method behind it. Events are made here and handed straight to the responder that would get them, by the selector AppKit sends, and what's checked is what comes of it. Nothing is posted, and nothing outside this process hears. A real pinch or click can't be made this way, so for those it's the notification, and that the target answers the action.
    func entryPoints() -> [String: Bool] {
        guard let controller = delegate.windowController else { return ["window_open": false] }
        let sidebar = controller.sidebar
        let pages = controller.pages
        var found: [String: Bool] = ["window_open": true]
        let entered = #selector(NSResponder.mouseEntered(with:))
        let exited = #selector(NSResponder.mouseExited(with:))
        let moved = #selector(NSResponder.mouseMoved(with:))
        let wheel = #selector(NSResponder.scrollWheel(with:))
        // These two aren't responders, so nothing but their own code answers these, and tracking areas call them by name. Going by a selector nothing answers would throw, and this would never answer.
        // The corners' marks (window buttons, zoom, page number) are answered by the fader, which AppKit reaches by `mouseEntered:` and `mouseExited:`.
        let chrome = controller.chrome
        found["chrome_hover_in"] = chrome.responds(to: Selector(("mouseEntered:")))
        found["chrome_hover_out"] = chrome.responds(to: Selector(("mouseExited:")))
        found["chrome_areas_are_on_the_window"] = chrome.areasAreOn(controller.root)
        // The top left area covers the window's buttons and the sidebar's button, so a pointer going to them is found.
        let corner = chrome.watchedCorners["top-left"] ?? .zero
        found["top_left_area_covers_the_buttons"] = (controller.windowButtonFrames(in: controller.root) + [controller.sidebarButton.frame]).allSatisfy { corner.contains($0) }
        let owners: [(String, NSObject, Selector)] = [("sidebar_hover_in", sidebar, entered), ("sidebar_hover_out", sidebar, exited), ("sidebar_pointer_moved", sidebar, moved)]
        for (name, owner, selector) in owners { found[name] = owner.responds(to: selector) }
        guard owners.allSatisfy({ $0.1.responds(to: $0.2) }) else { return found }
        let actions: [(String, NSControl)] = [("tab_row_click", sidebar.table), ("contents_row_click", sidebar.contentsTable), ("add_button", sidebar.addButton), ("home_button", controller.homeButtonForQA), ("sidebar_button", controller.sidebarButton), ("zoom_level", controller.zoomButton), ("zoom_field", controller.zoomField), ("zoom_reset", controller.zoomReset)]
        for (name, control) in actions {
            found[name] = control.action.flatMap { action in (control.target as? NSObject)?.responds(to: action) } ?? false
        }
        // The areas that send them are really there, and over what they're for.
        found["sidebar_area_is_on_the_sidebar"] = sidebar.view.trackingAreas.contains { $0.owner === sidebar && $0.options.contains(.mouseMoved) && $0.options.contains(.inVisibleRect) }
        sidebar.contentsScroll.updateTrackingAreas()
        found["contents_area_is_on_the_contents"] = sidebar.contentsScroll.trackingAreas.contains { $0.owner === sidebar.contentsScroll && $0.options.contains(.inVisibleRect) }

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
        // The thumb is the sidebar's own, there whether or not the pointer is, so the system's scroller and its track never come.
        found["contents_hover_brings_no_system_scroller"] = !sidebar.contentsScroll.hasVerticalScroller
        // A shortened contents entry, by where the pointer is, and left to rest as a real pointer would be, on the real timer.
        if let place = sidebar.cutOffContentsPlaceInWindow() {
            sidebar.perform(moved, with: pointer(at: place.point))
            found["pointer_finds_the_contents_row"] = sidebar.lastPointerRow == place.row
            RunLoop.current.run(until: Date(timeIntervalSinceNow: delegate.limits.revealDelaySeconds + 0.3))
            let report = sidebar.revealReport()
            found["contents_hover_reveals_the_full_name"] = (report["shown"] as? Bool) == true && (report["row"] as? Int) == place.row
        }
        sidebar.contentsScroll.perform(exited, with: crossing(.mouseExited))
        found["contents_hover_ends"] = sidebar.contentsScroll.verticalScroller?.isHidden != false || !sidebar.contentsScroll.hasVerticalScroller
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
        // Rotate: a tool the panel always has. Paper is View > Paper's.
        if opened, let rotate = sidebar.panel?.buttons["rotate.right"] {
            found["panel_button_answers"] = rotate.action.flatMap { action in (rotate.target as? NSObject)?.responds(to: action) } ?? false
        }
        sidebar.moreButton.performClick(nil)
        RunLoop.current.run(until: Date(timeIntervalSinceNow: 0.05))
        found["panel_closes"] = (sidebar.panelReport()["open"] as? Bool) == false
        // The mouse on a page: a press, a drag and a release, made and handed to what AppKit sends them to, on text if there's text here. Nothing's posted.
        if let place = pages.documentPointOnText() {
            let window = controller.window
            let inWindow = pages.documentView.convert(place, to: nil)
            func mouse(_ type: NSEvent.EventType, clicks: Int) -> NSEvent? {
                NSEvent.mouseEvent(with: type, location: inWindow, modifierFlags: [], timestamp: 0, windowNumber: window.windowNumber, context: nil, eventNumber: 0, clickCount: clicks, pressure: 1)
            }
            let steps = pages.pointerSteps
            pages.documentView.perform(#selector(NSResponder.mouseDown(with:)), with: mouse(.leftMouseDown, clicks: 2))
            pages.documentView.perform(#selector(NSResponder.mouseUp(with:)), with: mouse(.leftMouseUp, clicks: 2))
            found["mouse_reaches_the_selection"] = pages.pointerSteps == steps + 2
            pages.documentView.perform(moved, with: mouse(.mouseMoved, clicks: 0))
            found["pointer_reaches_the_cursor"] = pages.pointerSteps == steps + 3
            pages.documentView.updateTrackingAreas()
            found["pointer_area_is_on_the_page"] = pages.documentView.trackingAreas.contains { $0.owner === pages.documentView && $0.options.contains(.mouseMoved) && $0.options.contains(.cursorUpdate) && $0.options.contains(.inVisibleRect) }
            found["right_click_has_a_menu"] = pages.documentView.menu(for: mouse(.rightMouseDown, clicks: 1)!)?.items.map(\.action).contains(#selector(AppDelegate.copy(_:))) == true
        }
        // Copy and Select All: their keys, through the real menu bar, reach the app when nothing with the focus wants them. What's checked is what comes of it, by what AppKit says of the real item.
        if let copy = menuItem("edit.copy"), let selectAll = menuItem("edit.select_all") {
            func enabled(_ item: NSMenuItem) -> Bool {
                item.menu?.update()
                return item.isEnabled
            }
            // A click takes any selection away, at once, whatever's been read.
            pages.pointerDown(atWindow: pages.documentView.convert(NSPoint(x: 1, y: 1), to: nil), clicks: 1, shift: false)
            pages.pointerUp()
            found["copy_is_grey_with_nothing_selected"] = !enabled(copy)
            found["select_all_can_be_chosen"] = enabled(selectAll)
            found["select_all_key_reaches_the_app"] = key("a", .command).map { NSApp.mainMenu?.performKeyEquivalent(with: $0) == true } ?? false
            found["select_all_selects"] = delegate.core.menuItemEnabled(id: "edit.copy")
            found["copy_is_enabled_with_something_selected"] = enabled(copy)
            found["copy_key_reaches_the_app"] = key("c", .command).map { NSApp.mainMenu?.performKeyEquivalent(with: $0) == true } ?? false
        }
        // Undo and Redo with the window key, which a quiet window isn't: what the window's own chain gives them to, and that it asks Rust. NSWindow alone takes them and greys them out.
        for action in [#selector(ReaderWindow.undo(_:)), #selector(ReaderWindow.redo(_:))] {
            let name = action == #selector(ReaderWindow.undo(_:)) ? "undo" : "redo"
            let target = QAChannel.keyWindowTarget(for: action, in: controller.window)
            found["\(name)_reaches_the_app_with_the_window_key"] = target.map { $0.0 is ReaderWindow && $0.1 == delegate.core.menuItemEnabled(id: "edit.\(name)") } ?? false
        }
        // A silent update waits for nobody having touched the app, and that's more than a click: what the monitor's given, by what it was given.
        let watched = AppDelegate.actions.union(AppDelegate.nearby)
        let kinds: [NSEvent.EventType] = [.mouseMoved, .leftMouseDragged, .keyDown, .leftMouseDown, .scrollWheel, .magnify, .swipe, .rotate, .flagsChanged, .gesture]
        found["pointer_and_gestures_count_as_touching"] = kinds.allSatisfy { watched.contains(NSEvent.EventTypeMask(type: $0)) }
        found["windows_hear_the_pointer_moving"] = controller.window.acceptsMouseMovedEvents
        // Services, by what they ask. That they're given the text is `qa services`, which needs its page read first.
        found["services_arent_offered_what_they_didnt_ask_for"] = delegate.validRequestor(forSendType: .tiff, returnType: nil) == nil && delegate.validRequestor(forSendType: .string, returnType: .string) == nil
        pages.pointerDown(atWindow: pages.documentView.convert(NSPoint(x: 1, y: 1), to: nil), clicks: 1, shift: false)
        pages.pointerUp()
        found["services_arent_offered_nothing"] = delegate.validRequestor(forSendType: .string, returnType: nil) == nil
        let nothing = NSPasteboard(name: NSPasteboard.Name("com.rectopdf.qa.nothing.\(ProcessInfo.processInfo.processIdentifier)"))
        found["services_write_nothing_with_nothing_selected"] = !delegate.writeSelection(to: nothing, types: [.string]) && nothing.string(forType: .string) == nil
        nothing.releaseGlobally()
        // A control-click on a tab's row, as AppKit hands it to the table: taken for comparing, by the row under it, and not left to become a right-click. Owner, 2026-11-08: "Ctrl clicking more then one pdf should enable comparison view". What it does is `qa compare-click`'s.
        if sidebar.table.numberOfRows > 1 {
            let rowRect = sidebar.table.rect(ofRow: 1)
            let place = sidebar.table.convert(NSPoint(x: rowRect.midX, y: rowRect.midY), to: nil)
            // What it would do is swapped for a note of the row for the check, so the comparison is as it was after.
            let real = sidebar.table.controlClicked
            var taken: [Int] = []
            sidebar.table.controlClicked = { row in taken.append(row) }
            if let event = NSEvent.mouseEvent(with: .leftMouseDown, location: place, modifierFlags: .control, timestamp: 0, windowNumber: window.windowNumber, context: nil, eventNumber: 0, clickCount: 1, pressure: 1) {
                sidebar.table.perform(#selector(NSResponder.mouseDown(with:)), with: event)
            }
            sidebar.table.controlClicked = real
            found["tab_row_control_click_compares"] = taken == [1]
        }
        // A pinch comes with these from AppKit, and they're what stops drawing until it ends.
        found["pages_can_be_pinched"] = pages.scrollView.allowsMagnification
        NotificationCenter.default.post(name: NSScrollView.willStartLiveMagnifyNotification, object: pages.scrollView)
        found["pinch_starts"] = pages.isLive
        NotificationCenter.default.post(name: NSScrollView.didEndLiveMagnifyNotification, object: pages.scrollView)
        found["pinch_ends"] = !pages.isLive
        return found
    }

    /// The whole report: each area gives its part.
    func state() -> [String: Any] {
        guard let controller = delegate.windowController else {
            return ["window": NSNull(), "window_open": false, "is_active": NSApp.isActive, "activation": policyName(), "hidden": delegate.isAppHidden, "sheet": delegate.sheetShowing ?? [:]]
        }
        let reader = delegate.core.reader()
        let overPages = { (rect: NSRect) in controller.paneAreas.contains { $0.pagesOverlap(rect, in: controller.root) } }
        var report = windowSection(controller: controller, reader: reader, overPages: overPages)
        for part in [
            controlsSection(controller: controller, overPages: overPages),
            sidebarSection(controller: controller, reader: reader),
            findSection(controller: controller),
            settingsSection(),
            menusSection(),
            panesSection(controller: controller),
        ] {
            report.merge(part) { first, _ in first }
        }
        return report
    }
}
