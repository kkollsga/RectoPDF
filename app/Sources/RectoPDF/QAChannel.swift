import AppKit
import CryptoKit
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
            let cpuStarted = clock_gettime_nsec_np(CLOCK_THREAD_CPUTIME_ID)
            controller?.pages.scroll(by: Double(value) ?? 0)
            let took = CACurrentMediaTime() - started
            let cpu = clock_gettime_nsec_np(CLOCK_THREAD_CPUTIME_ID) - cpuStarted
            let contents = controller?.sidebar.contentsSample() ?? (y: 0, currentVisible: true)
            delegate.core.qaNoteStep(micros: UInt64(took * 1_000_000), cpuMicros: cpu / 1000, contentsY: contents.y, currentVisible: contents.currentVisible)
            return
        case "state":
            answer(id, state())
            return
        case "animations":
            // Cheap, as `qa wait-settled` asks it until there's nothing.
            answer(id, controller?.runningAnimations() ?? [])
            return
        case "entry_points":
            answer(id, entryPoints())
            return
        case "shot":
            shot(id: id, path: value)
            return
        case "shot_settings":
            guard let window = delegate.settings?.window, let view = window.contentView?.superview else { return fail(id, "The Settings window isn't open.") }
            window.layoutIfNeeded()
            plainShot(of: view, id: id, path: value)
            return
        case "shot_menu":
            guard let menu = NSApp.mainMenu?.items.first?.submenu else { return fail(id, "There's no menu.") }
            menu.update()
            plainShot(of: MenuDrawing(menu: menu), id: id, path: value)
            return
        case "shot_menus":
            // Each menu, then any submenu one of its own items opens, as File's Open Recent, so what's in it is seen too. The system's own, such as Services, aren't ours to show.
            let menus = (NSApp.mainMenu?.items ?? []).compactMap(\.submenu).flatMap { menu in [menu] + menu.items.filter { ($0.representedObject as? String)?.isEmpty == false && $0.submenu?.items.isEmpty == false }.compactMap(\.submenu) }
            guard !menus.isEmpty else { return fail(id, "There's no menu.") }
            let drawings = menus.map { menu -> MenuDrawing in
                menu.update()
                return MenuDrawing(menu: menu)
            }
            let row = FlippedView(frame: NSRect(x: 0, y: 0, width: drawings.reduce(0) { $0 + $1.frame.width + 8 }, height: drawings.map(\.frame.height).max() ?? 0))
            var x: CGFloat = 0
            for drawing in drawings {
                drawing.frame.origin = NSPoint(x: x, y: 0)
                row.addSubview(drawing)
                x += drawing.frame.width + 8
            }
            plainShot(of: row, id: id, path: value)
            return
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
            answer(id, ["chars": text?.count ?? 0, "clipboard": delegate.pasteboard.name.rawValue, "types": types, "untrusted": ["text": text as Any? ?? NSNull()]] as [String: Any])
            return
        case "services":
            // By the selector AppKit sends, to a clipboard nothing else can see.
            let requestor = delegate.validRequestor(forSendType: .string, returnType: nil) as? NSServicesMenuRequestor
            let board = NSPasteboard(name: NSPasteboard.Name("com.rectopdf.qa.services.\(ProcessInfo.processInfo.processIdentifier)"))
            let written = requestor?.writeSelection?(to: board, types: [.string]) ?? false
            let text = board.string(forType: .string)
            board.releaseGlobally()
            answer(id, ["offers_text": requestor != nil, "written": written, "untrusted": ["text": text as Any? ?? NSNull()]] as [String: Any])
            return
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
        case "print_to_pdf":
            // `OUT` for every page, or `OUT|FIRST|LAST`, as Rust checked it.
            let parts = value.split(separator: "|").map(String.init)
            let (first, last) = parts.count == 3 ? (UInt32(parts[1]) ?? 0, UInt32(parts[2]) ?? 0) : (0, 0)
            guard delegate.printToPDFForQA(out: parts[0], first: first, last: last) else { return fail(id, "There's nothing to print.") }
        case "mouse_up":
            controller?.readingPane.pointerUp()
        case "notice_button":
            guard let controller, !controller.copyNotice.isHidden, !controller.copyNotice.button.isHidden else { return fail(id, "No notice with a button is showing.") }
            controller.copyNotice.button.performClick(nil)
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
        case "open_file":
            delegate.core.openFile(path: value)
        case "sheet":
            delegate.core.sheetChanged(open: value == "open")
        case "settings":
            switch value {
            case "open": delegate.showSettings()
            case "close": delegate.settings?.window.close()
            default:
                guard let toggle = delegate.settings?.switches[value] else { return fail(id, "That switch isn't showing.") }
                toggle.performClick(nil)
            }
        case "sidebar_width":
            delegate.core.setSidebarWidth(width: Double(value) ?? 0)
        case "quit":
            answer(id, ["quitting": true])
            DispatchQueue.main.async { NSApp.terminate(nil) }
            return
        case "scroll":
            controller?.readingPane.scroll(by: Double(value) ?? 0)
        case "zoom":
            controller?.readingPane.setZoom(Double(value) ?? 1)
        case "pane_scroll":
            // A scroll in a pane, by what a scroll does: the reader's own, so synced scroll follows it and it's the pane being read.
            let parts = value.split(separator: ":")
            guard let controller, parts.count == 2, let pane = Int(parts[0]), pane < controller.paneAreas.count, let points = Double(parts[1]) else { return fail(id, "That isn't a pane and a scroll.") }
            controller.paneAreas[pane].scroll(by: points)
            controller.drainAll()
        case "pane_focus":
            // As a right-click on the pane reaches it, by the method AppKit calls for one: the menu's made and not shown.
            guard let controller, let pane = Int(value), pane < controller.paneAreas.count else { return fail(id, "There's no such pane.") }
            _ = controller.paneAreas[pane].documentView.menu(for: rightClick(in: controller.paneAreas[pane], window: controller.window))
        case "pane_context_menu":
            // A right-click on a pane, by what AppKit calls for one, then the item, as a person would choose it.
            let parts = value.split(separator: ":", maxSplits: 1).map(String.init)
            guard let controller, parts.count == 2, let pane = Int(parts[0]), pane < controller.paneAreas.count else { return fail(id, "That isn't a pane and an item.") }
            guard let menu = controller.paneAreas[pane].documentView.menu(for: rightClick(in: controller.paneAreas[pane], window: controller.window)) else { return fail(id, "There's no menu.") }
            menu.update()
            guard let item = menu.items.first(where: { ($0.representedObject as? String) == parts[1] }), let action = item.action else { return fail(id, "That isn't in the pane's right-click menu.") }
            guard item.isEnabled else { return fail(id, "That's greyed out in the right-click menu right now.") }
            NSApp.sendAction(action, to: item.target, from: item)
        case "compare_click":
            controller?.sidebar.compareClickRow(Int(value) ?? 0)
        case "hover":
            controller?.sidebar.setHover(value == "none" ? nil : Int(value))
        case "click_row":
            if value == "note" { controller?.sidebar.clickNote() } else if value.hasPrefix("fold:") { controller?.sidebar.clickFold(Int(value.dropFirst(5)) ?? -1) } else { controller?.sidebar.clickRow(Int(value) ?? 0) }
        case "contents_scroll":
            controller?.sidebar.userScrollContents(toRow: Int(value) ?? 0)
        case "palette_type":
            // Rust has checked it's open, and says so in its words if not.
            guard let palette = controller?.sidebar.palette else { return }
            palette.field.stringValue = value
            palette.typed(value)
        case "palette_key":
            guard let palette = controller?.sidebar.palette else { return }
            palette.press(value)
        case "find_type":
            // What typing in the field does: the field holds it, and its change goes where a keystroke's does.
            guard let find = controller?.sidebar.find, find.model.open else { return fail(id, "Find isn't open. `recto qa menu find.open` opens it.") }
            find.field.stringValue = value
            find.typed(value)
        case "find_key":
            guard let find = controller?.sidebar.find, find.model.open else { return fail(id, "Find isn't open.") }
            switch value {
            case "return": delegate.core.findReturn(forward: true)
            case "shift_return": delegate.core.findReturn(forward: false)
            case "escape": find.closeClicked()
            default: return fail(id, "It's `return`, `shift_return` or `escape`.")
            }
        case "find_click":
            guard let find = controller?.sidebar.find, find.model.open, let row = Int(value), row >= 0, row < Int(find.model.rowCount) else { return fail(id, "There's no such row in Find's list.") }
            find.clickRow(row)
        case "contents_pointer":
            controller?.sidebar.setContentsPointer(value == "in")
        case "menu":
            guard let item = menuItem(value), let action = item.action else { return fail(id, "That isn't on the menu bar.") }
            // As a person can only choose what isn't greyed out, by what AppKit makes of it now.
            item.menu?.update()
            guard item.isEnabled else { return fail(id, "That's greyed out in the menu right now.") }
            NSApp.sendAction(action, to: item.target, from: item)
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
        case "sheet_answer":
            // The answer a sheet's button gives, through the sheet's own ending, as a click on it ends it: the same completion runs.
            let codes: [String: NSApplication.ModalResponse] = ["first": .alertFirstButtonReturn, "second": .alertSecondButtonReturn, "third": .alertThirdButtonReturn, "cancel": .cancel]
            guard let code = codes[value] else { return fail(id, "That isn't one of a sheet's answers.") }
            guard delegate.sheets.answer(code, over: controller?.window) else { return fail(id, "No sheet is open.") }
        case "context_menu":
            guard let controller else { return fail(id, "The window is closed.") }
            let menu = controller.readingPane.contextMenu()
            menu.update()
            guard let item = menu.items.first(where: { ($0.representedObject as? String) == value }), let action = item.action else { return fail(id, "That isn't in the right-click menu.") }
            guard item.isEnabled else { return fail(id, "That's greyed out in the right-click menu right now.") }
            NSApp.sendAction(action, to: item.target, from: item)
        case "key_action":
            // What a key or the menu does with the window key, as it is for a person: the first in the window's own chain that answers it, asked whether it can, then sent it. A quiet window isn't key, so AppKit's own route would skip the window, and this doesn't.
            guard let controller else { return fail(id, "The window is closed.") }
            let action = NSSelectorFromString(value + ":")
            guard let (target, enabled) = QAChannel.keyWindowTarget(for: action, in: controller.window) else { return fail(id, "Nothing in the window answers that.") }
            guard enabled else { return fail(id, "That's greyed out with the window key.") }
            target.perform(action, with: nil)
        case "password":
            // Types into the password sheet's secure field and presses Open, through the sheet's own ending, as a person's click ends it.
            guard let controller, let sheet = delegate.sheets.sheetWindow(over: controller.window), controller.passwordField.window === sheet else { return fail(id, "No password sheet is open.") }
            controller.passwordField.stringValue = value
            _ = delegate.sheets.answer(.alertFirstButtonReturn, over: controller.window)
        case "zoom_escape":
            // Escape in the field, by the method AppKit calls for it.
            guard let controller, controller.zoomEditing else { return fail(id, "The zoom level's field isn't open.") }
            _ = controller.control(controller.zoomField, textView: NSTextView(), doCommandBy: #selector(NSResponder.cancelOperation(_:)))
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
        case "page_escape":
            guard let controller, controller.pageEditing else { return fail(id, "The go-to field isn't open.") }
            _ = controller.control(controller.pageField, textView: NSTextView(), doCommandBy: #selector(NSResponder.cancelOperation(_:)))
        case "swipe":
            // What the system's swipe calls, with its sideways amount.
            guard let controller, let delta = Double(value) else { return fail(id, "That isn't a swipe's sideways amount.") }
            controller.readingPane.swiped(deltaX: delta)
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

    /// A right-click in the middle of a pane, made and handed over by selector, as the entry points are: nothing's posted.
    private func rightClick(in pane: PageArea, window: NSWindow) -> NSEvent {
        let place = pane.scrollView.convert(NSPoint(x: pane.scrollView.bounds.midX, y: pane.scrollView.bounds.midY), to: nil)
        return NSEvent.mouseEvent(with: .rightMouseDown, location: place, modifierFlags: [], timestamp: 0, windowNumber: window.windowNumber, context: nil, eventNumber: 0, clickCount: 1, pressure: 1)!
    }

    /// An item of the real menu bar, by its id.
    private func menuItem(_ id: String) -> NSMenuItem? {
        // Submenus' items too, as Open Recent's files are.
        let items = NSApp.mainMenu?.items.compactMap(\.submenu).flatMap(\.items) ?? []
        return (items + items.compactMap(\.submenu).flatMap(\.items)).first { ($0.representedObject as? String) == id && !$0.isHidden }
    }

    private func state() -> [String: Any] {
        guard let controller = delegate.windowController else {
            return ["window": NSNull(), "window_open": false, "is_active": NSApp.isActive, "activation": policyName()]
        }
        let reader = delegate.core.reader()
        let sidebar = controller.sidebar
        var pages = controller.pages.report()
        pages["visible"] = !controller.pages.scrollView.isHidden
        let overPages = { (rect: NSRect) in controller.paneAreas.contains { $0.pagesOverlap(rect, in: controller.root) } }
        let frame = controller.window.frame
        return [
            "window_open": true,
            "window": ["x": frame.minX, "y": frame.minY, "width": frame.width, "height": frame.height, "full_screen": controller.isFullScreen, "on_any_screen": NSScreen.screens.contains { $0.frame.intersects(frame) }, "is_visible": controller.window.isVisible],
            "is_active": NSApp.isActive,
            "activation": policyName(),
            "appearance": Theme.isDark(controller.root) ? "dark" : "light",
            "sidebar_hidden": reader.sidebarHidden,
            "save_button": ["visible": !controller.saveButton.isHidden, "enabled": controller.saveButton.isEnabled, "tooltip": controller.saveButton.toolTip ?? "", "frame": [controller.saveButton.frame.minX, controller.saveButton.frame.minY, controller.saveButton.frame.width, controller.saveButton.frame.height], "clear_of_window_buttons": controller.windowButtonFrames(in: controller.root).allSatisfy { !$0.intersects(controller.saveButton.frame) } && !controller.sidebarButton.frame.intersects(controller.saveButton.frame)] as [String: Any],
            "sheet": delegate.sheetShowing ?? [:],
            "sidebar_button": ["visible": !controller.sidebarButton.isHidden && controller.sidebarButton.superview != nil, "frame": [controller.sidebarButton.frame.minX, controller.sidebarButton.frame.minY, controller.sidebarButton.frame.width, controller.sidebarButton.frame.height], "clear_of_window_buttons": controller.windowButtonFrames(in: controller.root).allSatisfy { !$0.intersects(controller.sidebarButton.frame) }] as [String: Any],
            "window_buttons_shown": controller.windowButtonsShown,
            "zoom_level": ["text": controller.zoomButton.attributedTitle.string, "visible": !controller.zoomButton.isHidden, "editing": controller.zoomEditing, "field": controller.zoomEditing ? controller.zoomField.stringValue : NSNull() as Any, "reset": controller.zoomReset.isHidden ? NSNull() as Any : controller.zoomReset.attributedTitle.string, "in_the_top_right": controller.zoomButton.frame.maxX <= controller.root.bounds.maxX - 6 && controller.zoomButton.frame.minX > controller.root.bounds.maxX - 160 && controller.zoomButton.frame.minY < 30, "clear_of_agent_icon": controller.agentIcon.isHidden || (!controller.zoomButton.frame.intersects(controller.agentIcon.frame) && controller.zoomButton.frame.maxX - MainWindowController.zoomPadding <= controller.agentIcon.frame.minX - 4 && controller.zoomField.frame.maxX <= controller.agentIcon.frame.minX - 4), "in_the_agent_icons_row": abs(controller.zoomButton.frame.midY - controller.agentIcon.frame.midY) < 1, "right_margin": controller.root.bounds.maxX - (controller.zoomEditing ? controller.zoomField.frame.maxX : controller.zoomButton.frame.maxX - MainWindowController.zoomPadding), "reset_clear_of_field": controller.zoomReset.frame.maxX <= controller.zoomField.frame.minX, "clear_of_pages": controller.zoomButton.isHidden || !overPages(controller.zoomButton.frame.insetBy(dx: MainWindowController.zoomPadding, dy: 0)), "above_the_scroller": max(controller.zoomButton.frame.maxY, controller.zoomField.frame.maxY) + 4 <= controller.paneAreas[controller.paneAreas.count - 1].scrollView.scrollerInsets.top] as [String: Any],
            "home": !controller.home.isHidden,
            "sidebar_visible": !sidebar.view.isHidden,
            "sidebar_width": sidebar.view.frame.width,
            "focus_view": delegate.core.focusView(),
            "sidebar_rows": sidebar.rowCount,
            "reveal": sidebar.revealReport(),
            "paper": reader.paper,
            "contents": sidebar.contentsReport(),
            "palette": sidebar.palette.report(),
            "find": sidebar.find.report(),
            "hover": sidebar.hoveredRow.map { $0 as Any } ?? NSNull(),
            "animations": controller.runningAnimations(),
            "place": reader.place,
            "note": controller.noteLabel.isHidden ? NSNull() as Any : controller.noteLabel.label.text,
            // Where the note's bubble is and how many lines it wraps to, so a check can hold its shape and not only its words.
            "note_box": controller.noteLabel.isHidden ? NSNull() as Any : controller.noteLabel.qaBox(in: controller.root),
            "copy_notice": controller.copyNotice.isHidden ? NSNull() as Any : ["text": controller.copyNotice.label.text, "button": controller.copyNotice.button.isHidden ? NSNull() as Any : controller.copyNotice.button.attributedTitle.string, "frame_ok": controller.root.bounds.contains(controller.copyNotice.frame), "box": controller.copyNotice.qaBox(in: controller.root)] as [String: Any],
            "place_label": controller.placeLabel.isHidden ? NSNull() as Any : controller.placeLabel.label.text,
            "links": ["hint": controller.linkHint.isHidden ? NSNull() as Any : controller.linkHint.label.text, "hint_in_lower_left": !controller.linkHint.isHidden && controller.linkHint.frame.minX < controller.root.bounds.midX && controller.linkHint.frame.maxY > controller.root.bounds.maxY - 80 && controller.root.bounds.contains(controller.linkHint.frame), "page_field": controller.pageEditing ? controller.pageField.stringValue : NSNull() as Any, "address_opened": delegate.addressOpenedForQA as Any? ?? NSNull()] as [String: Any],
            "place_label_in_lower_right": !controller.placeLabel.isHidden && controller.placeLabel.frame.maxX > controller.root.bounds.maxX - 40 && controller.placeLabel.frame.maxY > controller.root.bounds.maxY - 40 && controller.root.bounds.contains(controller.placeLabel.frame),
            "agent_icon": !controller.agentIcon.isHidden,
            "document_edited": controller.window.isDocumentEdited,
            "agent_icon_margins": ["top": controller.agentIcon.frame.minY, "right": controller.root.bounds.maxX - controller.agentIcon.frame.maxX, "clear_of_pages": controller.agentIcon.isHidden || !overPages(controller.agentIcon.frame)] as [String: Any],
            "panel": sidebar.panelReport(),
            "settings": delegate.settings?.report() ?? ["open": false],
            // From Rust's list and its word on what's enabled, as the real menu is built from. Building an NSMenu on every state read costs the app megabytes that the budgets would count; `qa context-menu` goes through the real one.
            "context_menu": delegate.core.contextMenu(pane: delegate.core.readingPane()).filter { !$0.id.isEmpty }.map { ["id": $0.id, "title": $0.title, "checked": $0.checked, "enabled": delegate.core.menuItemEnabled(id: $0.id)] as [String: Any] },
            "layout": reader.layout,
            "menu_bar": (NSApp.mainMenu?.items ?? []).compactMap(\.submenu).map { menu -> [String: Any] in
                // What AppKit itself says after asking whoever answers, as it does on opening a menu, and no copy of the rule.
                menu.update()
                return ["title": menu.title, "items": menu.items.filter { !$0.isSeparatorItem && !$0.isHidden }.flatMap { item -> [NSMenuItem] in
                    // A submenu's items follow its own, as Open Recent's files do.
                    item.submenu?.update()
                    return [item] + (item.submenu?.items.filter { !$0.isSeparatorItem && !$0.isHidden } ?? [])
                }.map { ["id": ($0.representedObject as? String) ?? "", "title": $0.title, "key": $0.keyEquivalent, "checked": $0.state == .on, "enabled": $0.isEnabled] as [String: Any] }]
            },
            "pages": pages,
            "panes": controller.paneReport(),
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

    /// The responder a key window gives an action to, and whether it says it can be chosen: up the chain from the window's first responder to the window, then its delegate, as AppKit goes before it reaches the app.
    static func keyWindowTarget(for action: Selector, in window: NSWindow) -> (NSObject, Bool)? {
        let item = NSMenuItem(title: "", action: action, keyEquivalent: "")
        var responder: NSResponder? = window.firstResponder ?? window
        var chain: [NSObject] = []
        while let next = responder {
            chain.append(next)
            responder = next.nextResponder
        }
        if !chain.contains(where: { $0 === window }) { chain.append(window) }
        if let delegate = window.delegate as? NSObject { chain.append(delegate) }
        guard let target = chain.first(where: { $0.responds(to: action) }) else { return nil }
        let enabled = (target as? NSMenuItemValidation)?.validateMenuItem(item) ?? (target as? NSUserInterfaceValidations)?.validateUserInterfaceItem(item) ?? true
        return (target, enabled)
    }

    private func policyName() -> String {
        switch NSApp.activationPolicy() {
        case .regular: "regular"
        case .accessory: "accessory"
        default: "prohibited"
        }
    }

    /// Warm paper and highlights in a shot: the sum the compositor does, on the picture. `render(in:)` leaves compositing filters out. With `lighten`, each channel is the brighter of the two, which is what a highlight on dark paper is.
    nonisolated private static func multiply(_ rep: NSBitmapImageRep, by rgb: UInt32, in rects: [NSRect], viewHeight: CGFloat, flipped: Bool, lighten: Bool = false) {
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
                    if lighten {
                        pixel[0] = UInt8(max(Int(pixel[0]), red))
                        pixel[1] = UInt8(max(Int(pixel[1]), green))
                        pixel[2] = UInt8(max(Int(pixel[2]), blue))
                    } else {
                        pixel[0] = UInt8(Int(pixel[0]) * red / 255)
                        pixel[1] = UInt8(Int(pixel[1]) * green / 255)
                        pixel[2] = UInt8(Int(pixel[2]) * blue / 255)
                    }
                }
            }
        }
    }

    /// A view as it is, with none of what the page area's own layers need done to them.
    private func plainShot(of view: NSView, id: UInt64, path: String) {
        let bounds = view.bounds
        guard let rep = NSBitmapImageRep(bitmapDataPlanes: nil, pixelsWide: Int(bounds.width * 2), pixelsHigh: Int(bounds.height * 2), bitsPerSample: 8, samplesPerPixel: 4, hasAlpha: true, isPlanar: false, colorSpaceName: .deviceRGB, bytesPerRow: 0, bitsPerPixel: 0) else { return fail(id, "The picture couldn't be made.") }
        rep.size = bounds.size
        view.cacheDisplay(in: bounds, to: rep)
        guard let data = rep.representation(using: .png, properties: [:]) else { return fail(id, "The picture couldn't be encoded.") }
        do {
            try data.write(to: URL(fileURLWithPath: path), options: .atomic)
            answer(id, ["out": path, "width": rep.pixelsWide, "height": rep.pixelsHigh, "bytes": data.count])
        } catch {
            fail(id, "The picture couldn't be written: \(error.localizedDescription)")
        }
    }

    /// A sheet over the window is a window of its own, which the window's drawing leaves out: its own drawing goes in where it sits, so a shot shows what a person would. A panel drawn by another process, as the save sheet's file browser is, may come out partly blank.
    static func drawSheet(of window: NSWindow, sheets: SheetPresenter, into rep: NSBitmapImageRep, flipped: Bool, height: CGFloat) {
        guard let sheet = sheets.sheetWindow(over: window), let origin = sheets.sheetOrigin(over: window), let sheetView = sheet.contentView?.superview ?? sheet.contentView else { return }
        // A sheet takes its window's look, light or dark. One that's held, never attached, isn't told when that changes.
        if sheet.appearance?.name != window.effectiveAppearance.name { sheet.appearance = window.effectiveAppearance }
        sheetView.layoutSubtreeIfNeeded()
        let size = sheetView.bounds
        guard let sheetRep = sheetView.bitmapImageRepForCachingDisplay(in: size) else { return }
        sheetView.cacheDisplay(in: size, to: sheetRep)
        let x = origin.x
        var y = origin.y
        if flipped { y = height - y - size.height }
        NSGraphicsContext.saveGraphicsState()
        NSGraphicsContext.current = NSGraphicsContext(bitmapImageRep: rep)
        // Over what's there, so its rounded corners show the window behind, as the window server would composite it. A plain draw copies them in clear.
        sheetRep.draw(in: NSRect(x: x, y: y, width: size.width, height: size.height), from: .zero, operation: .sourceOver, fraction: 1, respectFlipped: false, hints: nil)
        NSGraphicsContext.restoreGraphicsState()
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
        // Each pane's: a comparison has several.
        let tints = controller.paneAreas.compactMap { $0.tintRectsForShot(in: view) }
        let tint = tints.first.map { (rgb: $0.rgb, rects: tints.flatMap(\.rects)) }
        let highlights = controller.paneAreas.compactMap { $0.highlightsForShot(in: view) }.first
        for area in controller.paneAreas {
            area.hideTintForShot(true)
            area.hideHighlightsForShot(true)
        }
        CATransaction.flush()
        view.cacheDisplay(in: bounds, to: rep)
        QAChannel.drawSheet(of: controller.window, sheets: delegate.sheets, into: rep, flipped: view.isFlipped, height: bounds.height)
        for area in controller.paneAreas {
            area.hideTintForShot(false)
            area.hideHighlightsForShot(false)
        }
        let flipped = view.isFlipped
        let core = delegate.core
        // The encoding and the write are slow, so they stay off this thread.
        nonisolated(unsafe) let encoded = rep
        DispatchQueue.global(qos: .userInitiated).async {
            if let highlights { QAChannel.multiply(encoded, by: highlights.rgb, in: highlights.rects, viewHeight: bounds.height, flipped: flipped, lighten: highlights.lighten) }
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

/// A drawing of a menu's real items, for a shot: a real menu can't be opened with no screen. What's listed and what's enabled are the menu's own, as AppKit has them. The look is near enough to read, and it isn't the system's own drawing.
private final class MenuDrawing: NSView {
    private let rows: [NSMenuItem]
    private static let rowHeight: CGFloat = 22
    private static let lineHeight: CGFloat = 11
    private static let width: CGFloat = 280

    init(menu: NSMenu) {
        rows = menu.items.filter { !$0.isHidden }
        let height = rows.reduce(CGFloat(10)) { $0 + ($1.isSeparatorItem ? Self.lineHeight : Self.rowHeight) }
        super.init(frame: NSRect(x: 0, y: 0, width: Self.width, height: height))
    }

    @available(*, unavailable) required init?(coder: NSCoder) { fatalError() }

    override var isFlipped: Bool { true }

    override func draw(_ dirtyRect: NSRect) {
        NSApp.effectiveAppearance.performAsCurrentDrawingAppearance {
            NSColor.windowBackgroundColor.setFill()
            bounds.fill()
            NSColor.separatorColor.setStroke()
            NSBezierPath(rect: bounds.insetBy(dx: 0.5, dy: 0.5)).stroke()
            let font = NSFont.menuFont(ofSize: 13)
            var y: CGFloat = 5
            for item in rows {
                if item.isSeparatorItem {
                    NSColor.separatorColor.setFill()
                    NSRect(x: 12, y: y + Self.lineHeight / 2, width: bounds.width - 24, height: 1).fill()
                    y += Self.lineHeight
                    continue
                }
                let colour: NSColor = item.isEnabled ? .labelColor : .tertiaryLabelColor
                if item.state == .on { NSAttributedString(string: "✓", attributes: [.font: font, .foregroundColor: colour]).draw(at: NSPoint(x: 6, y: y + 2)) }
                NSAttributedString(string: item.title, attributes: [.font: font, .foregroundColor: colour]).draw(at: NSPoint(x: 22, y: y + 2))
                if !item.keyEquivalent.isEmpty {
                    let control = item.keyEquivalentModifierMask.contains(.control) ? "⌃" : ""
                    let key = NSAttributedString(string: control + "⌘" + item.keyEquivalent.uppercased(), attributes: [.font: font, .foregroundColor: item.isEnabled ? NSColor.secondaryLabelColor : NSColor.tertiaryLabelColor])
                    key.draw(at: NSPoint(x: bounds.width - key.size().width - 14, y: y + 2))
                }
                y += Self.rowHeight
            }
        }
    }
}
