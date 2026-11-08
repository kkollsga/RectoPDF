import AppKit
import UniformTypeIdentifiers

/// Runs a block on the UI thread, in every run-loop mode, including while a menu is tracking or a panel is open.
func onMain(_ work: @escaping @MainActor () -> Void) {
    let main = CFRunLoopGetMain()
    let modes = [CFRunLoopMode.commonModes.rawValue, RunLoop.Mode.eventTracking.rawValue as CFString, RunLoop.Mode.modalPanel.rawValue as CFString, CFRunLoopMode.defaultMode.rawValue] as CFArray
    CFRunLoopPerformBlock(main, modes) {
        // AppKit drains its pool after an event, and these aren't events. With nobody touching the app, as in a quiet run or a window left reading while an agent works, what these leave would pile up with nothing to clear it.
        autoreleasepool {
            MainActor.assumeIsolated { work() }
        }
    }
    CFRunLoopWakeUp(main)
}

/// What Rust says to the host. Returns at once and hops to the UI thread: it's called from Rust's threads.
final class Bridge: AppListener, @unchecked Sendable {
    /// Set once, before Rust can call.
    weak var delegate: AppDelegate?

    /// Collapses a burst of `modelChanged` into one pull.
    private let lock = NSLock()
    private var modelPending = false

    func modelChanged() {
        lock.lock()
        let already = modelPending
        modelPending = true
        lock.unlock()
        guard !already else { return }
        onMain { [weak self] in
            guard let self else { return }
            self.lock.lock()
            self.modelPending = false
            self.lock.unlock()
            self.delegate?.modelChanged()
        }
    }

    func opsReady() {
        onMain { [weak self] in
            self?.delegate?.windowController?.pages.drain()
            self?.delegate?.windowController?.showIfReady()
        }
    }

    func restoreReady() {
        onMain { [weak self] in self?.delegate?.restoreReady() }
    }

    func quitAnswer(answer: QuitAnswer) {
        onMain { [weak self] in self?.delegate?.quitAnswer(answer) }
    }

    func handOffQuit() {
        onMain { [weak self] in self?.delegate?.handOffQuit() }
    }

    func qaRequest(id: UInt64, verb: String, value: String) {
        onMain { [weak self] in self?.delegate?.qa?.handle(id: id, verb: verb, value: value) }
    }

    func sessionEnded() {
        onMain { NSApp.terminate(nil) }
    }

    func copyReady() {
        onMain { [weak self] in self?.delegate?.copyReady() }
    }
}

@MainActor final class AppDelegate: NSObject, NSApplicationDelegate, NSMenuItemValidation, @preconcurrency NSServicesMenuRequestor {
    let core: AppCore
    let isQuiet: Bool
    /// What the sheet over the window says, for `qa state`: its kind and words. Nothing with none.
    var sheetShowing: [String: Any]?
    let strings: AppStrings
    let limits: ReaderLimits
    private let bridge: Bridge
    private(set) var windowController: MainWindowController?
    private(set) var qa: QAChannel?
    private(set) var settings: SettingsWindowController?
    /// The one timer the updater's asked for, and which time it's armed for.
    private var updateTimer: DispatchSourceTimer?
    private var updateTimerAt: UInt64?
    private var mainObserver: CFRunLoopObserver?
    private var inputMonitor: Any?
    private var handingOff = false

    init(arguments: [String]) {
        let bridge = Bridge()
        do {
            core = try AppCore.start(args: arguments, listener: bridge)
        } catch {
            // A QA client waits for one line. Anyone else gets the same words on stderr.
            let words = String(describing: error)
            let line = try? String(data: JSONSerialization.data(withJSONObject: ["ok": false, "error": ["code": "app_refused", "message": words]]), encoding: .utf8)
            print(line ?? words)
            FileHandle.standardError.write(Data((words + "\n").utf8))
            exit(1)
        }
        self.bridge = bridge
        isQuiet = core.isQa()
        strings = core.strings()
        limits = core.limits()
        super.init()
        bridge.delegate = self
        if isQuiet { qa = QAChannel(delegate: self) }
    }

    private var memoryPressureSource: DispatchSourceMemoryPressure?

    /// The system says memory's short. What's done about it is Rust's.
    private func watchMemoryPressure() {
        let source = DispatchSource.makeMemoryPressureSource(eventMask: [.warning, .critical], queue: .main)
        source.setEventHandler { [weak self, weak source] in
            guard let self, let source else { return }
            self.core.memoryPressure(critical: source.data.contains(.critical))
        }
        source.resume()
        memoryPressureSource = source
    }

    func applicationDidFinishLaunching(_ notification: Notification) {
        watchMainThread()
        watchMemoryPressure()
        installMenu()
        NSApp.registerServicesMenuSendTypes([.string], returnTypes: [])
        watchActivityAndInput()
        let info = Bundle.main.infoDictionary
        let system = ProcessInfo.processInfo.operatingSystemVersion
        core.setUpdateFacts(version: info?["CFBundleShortVersionString"] as? String ?? "", bundlePath: Bundle.main.bundlePath, systemVersion: "\(system.majorVersion).\(system.minorVersion).\(system.patchVersion)")
        // The window comes when the state's been read, as its frame and what's on it come from there. It doesn't wait long.
        core.beginRestore()
        let core = self.core
        DispatchQueue.main.asyncAfter(deadline: .now() + limits.restorePatienceSeconds) { core.restorePatienceOver() }
    }

    /// The state's read and in the session, or the wait for it is over.
    func restoreReady() {
        guard windowController == nil else { return }
        switch core.appearanceChoice() {
        case "dark": NSApp.appearance = NSAppearance(named: .darkAqua)
        case "light": NSApp.appearance = NSAppearance(named: .aqua)
        default: break
        }
        if core.windowShouldOpen() {
            openWindow()
        } else {
            core.windowClosed()
            core.noWindowToShow()
        }
        // It's part of what comes back.
        if core.settingsWindowWasOpen() { showSettings(takingKey: false) }
        core.ready()
    }

    /// Opens Settings, or brings it forward. Nothing's made for it until it's asked for. Coming back at a launch it doesn't take the keyboard, which is the main window's as it was.
    func showSettings(takingKey: Bool = true) {
        if settings == nil { settings = SettingsWindowController(delegate: self, place: core.settingsWindowPlace(screens: NSScreen.screens.map(MainWindowController.info))) }
        guard !isQuiet else { return }
        if takingKey {
            settings?.window.makeKeyAndOrderFront(nil)
        } else if core.opensInBackground() {
            settings?.window.orderBack(nil)
        } else {
            settings?.window.orderFront(nil)
        }
    }

    func settingsClosed() {
        settings = nil
    }

    /// One timer for the time the updater names, armed again only when that changes. Nothing repeats, and it's by the wall clock, so a Mac that's been asleep looks as soon as it wakes.
    private func armUpdateTimer() {
        let wanted = core.updateTimer()
        guard wanted?.atMs != updateTimerAt else { return }
        updateTimer?.cancel()
        updateTimer = nil
        updateTimerAt = wanted?.atMs
        guard let wanted else { return }
        let timer = DispatchSource.makeTimerSource(queue: .main)
        timer.schedule(wallDeadline: .now() + wanted.inSeconds, leeway: .seconds(5))
        timer.setEventHandler { [weak self] in
            MainActor.assumeIsolated {
                guard let self else { return }
                self.updateTimerAt = nil
                self.core.updateTimerFired()
                self.armUpdateTimer()
            }
        }
        timer.resume()
        updateTimer = timer
    }

    /// The app coming forward and going back, which is part of what's kept, and each input, which is how long it's been since the user touched the app. One number stored per event, and no timer.
    private func watchActivityAndInput() {
        let center = NotificationCenter.default
        center.addObserver(self, selector: #selector(activityChanged), name: NSApplication.didBecomeActiveNotification, object: nil)
        center.addObserver(self, selector: #selector(activityChanged), name: NSApplication.didResignActiveNotification, object: nil)
        let core = self.core
        inputMonitor = NSEvent.addLocalMonitorForEvents(matching: Self.actions.union(Self.nearby)) { event in
            if Self.actions.contains(NSEvent.EventTypeMask(type: event.type)) { core.noteUserInput() } else { core.noteUserNearby() }
            return event
        }
    }

    /// What someone does. Each says any notice has been read.
    nonisolated static let actions: NSEvent.EventTypeMask = [.leftMouseDown, .rightMouseDown, .otherMouseDown, .keyDown, .scrollWheel, .magnify]
    /// What shows someone's there. Owner, 2026-11-07: "hasn't touched", so a silent update errs towards never interrupting. Comes in only while there's someone moving something, so nothing at rest.
    nonisolated static let nearby: NSEvent.EventTypeMask = [.mouseMoved, .leftMouseDragged, .rightMouseDragged, .otherMouseDragged, .leftMouseUp, .rightMouseUp, .otherMouseUp, .keyUp, .flagsChanged, .rotate, .swipe, .smartMagnify, .gesture, .beginGesture, .endGesture, .pressure, .tabletPoint]

    @objc private func activityChanged() {
        guard !isQuiet else { return }
        if NSApp.isActive { windowController?.cameToTheFront() }
        core.frontmostChanged(app: NSApp.isActive, windowKey: windowController?.window.isKeyWindow ?? false)
    }

    func applicationShouldTerminate(_ sender: NSApplication) -> NSApplication.TerminateReply {
        // A hand-off has written and read back everything, and asks nothing of anyone.
        if handingOff { return .terminateNow }
        core.quitRequested()
        return .terminateLater
    }

    func quitAnswer(_ answer: QuitAnswer) {
        if answer.go || isQuiet {
            NSApp.reply(toApplicationShouldTerminate: true)
            return
        }
        let alert = NSAlert()
        alert.messageText = answer.title
        alert.informativeText = answer.message
        alert.addButton(withTitle: answer.stayButton)
        alert.addButton(withTitle: answer.quitButton)
        NSApp.reply(toApplicationShouldTerminate: alert.runModal() == .alertSecondButtonReturn)
    }

    func handOffQuit() {
        handingOff = true
        NSApp.terminate(nil)
    }

    /// Tells the hang watchdog when the UI thread wakes and sleeps.
    private func watchMainThread() {
        let core = self.core
        let observer = CFRunLoopObserverCreateWithHandler(kCFAllocatorDefault, CFRunLoopActivity.afterWaiting.rawValue | CFRunLoopActivity.beforeWaiting.rawValue, true, 0) { _, activity in
            if activity == .afterWaiting { core.mainBusy() } else { core.mainIdle() }
        }
        CFRunLoopAddObserver(CFRunLoopGetMain(), observer, .commonModes)
        mainObserver = observer
    }

    /// The red button's way back. Tabs are kept in the session, so the window comes back as it was.
    func openWindow() {
        if windowController != nil { return }
        let controller = MainWindowController(delegate: self)
        windowController = controller
        core.windowOpened()
        controller.windowIsUp()
        modelChanged()
        controller.showIfReady()
        // Past this it shows, whatever's on it: a slow page, or a file not answering, says so itself.
        DispatchQueue.main.asyncAfter(deadline: .now() + limits.showPatienceSeconds) { [weak controller] in controller?.showNow() }
    }

    /// What the window's closing does, whoever closed it.
    func windowDidClose() {
        windowController = nil
        core.windowClosed()
    }

    func modelChanged() {
        windowController?.refreshModel()
        settings?.refresh()
        installMenu()
        armUpdateTimer()
        windowController?.showIfReady()
    }

    func applicationShouldTerminateAfterLastWindowClosed(_ sender: NSApplication) -> Bool { false }

    func applicationShouldHandleReopen(_ sender: NSApplication, hasVisibleWindows: Bool) -> Bool {
        openWindow()
        return true
    }

    func application(_ application: NSApplication, open urls: [URL]) {
        for url in urls where url.isFileURL { core.openFile(path: url.path) }
    }

    func applicationWillTerminate(_ notification: Notification) {
        if isQuiet { pasteboard.releaseGlobally() }
        core.shutdown()
    }

    private var shownMenus: [MenuModel] = []

    /// The menu bar, as Rust lists it, built again only when that's changed.
    private func installMenu() {
        let menus = core.menuBar()
        guard menus != shownMenus else { return }
        shownMenus = menus
        let bar = NSMenu()
        for model in menus {
            let holder = NSMenuItem()
            let menu = NSMenu(title: model.title)
            for entry in model.entries {
                if entry.id.isEmpty {
                    menu.addItem(.separator())
                    continue
                }
                // One that goes by a standard selector has no target, so it reaches whatever has the focus first, and comes here if nothing there wants it.
                let isSystem = !entry.systemAction.isEmpty
                let item = menu.addItem(withTitle: entry.title, action: isSystem ? NSSelectorFromString(entry.systemAction) : #selector(menuChosen(_:)), keyEquivalent: entry.key)
                item.target = isSystem ? nil : self
                item.representedObject = entry.id
                if entry.id == "app.services" {
                    let services = NSMenu(title: entry.title)
                    item.submenu = services
                    item.action = nil
                    NSApp.servicesMenu = services
                }
                item.keyEquivalentModifierMask = entry.withControl ? [.command, .control] : [.command]
                item.state = entry.checked ? .on : .off
                if entry.hidden {
                    item.isHidden = true
                    item.allowsKeyEquivalentWhenHidden = true
                }
            }
            holder.submenu = menu
            bar.addItem(holder)
        }
        NSApp.mainMenu = bar
    }

    @objc func menuChosen(_ sender: NSMenuItem) {
        guard let id = sender.representedObject as? String else { return }
        perform(id)
    }

    /// Copy and Select All, from the menu bar, a context menu or their keys, once nothing with the focus has wanted them.
    @objc func copy(_ sender: Any?) {
        perform("edit.copy")
    }

    @objc func selectAll(_ sender: Any?) {
        perform("edit.select_all")
    }

    /// Undo and Redo of the tab's edits, once nothing with the focus has wanted them, as a text field typing a zoom level does.
    @objc func undo(_ sender: Any?) {
        perform("edit.undo")
    }

    @objc func redo(_ sender: Any?) {
        perform("edit.redo")
    }

    /// Rust says what can be chosen, each time a menu opens or its key is pressed.
    func validateMenuItem(_ item: NSMenuItem) -> Bool {
        guard let id = item.representedObject as? String else { return item.action != nil }
        return core.menuItemEnabled(id: id)
    }

    /// The clipboard: the system's, or a quiet run's own, which nothing else can see.
    lazy var pasteboard: NSPasteboard = isQuiet ? NSPasteboard(name: NSPasteboard.Name("com.rectopdf.qa.\(ProcessInfo.processInfo.processIdentifier)")) : .general

    /// A copy has come back from the thread that read it.
    func copyReady() {
        guard let text = core.takeCopied() else { return }
        pasteboard.clearContents()
        pasteboard.setString(text, forType: .string)
    }

    // Services: the selection as text, to anything that takes it. It has to answer now, and it can if the text's all here, as it is for a selection that's on screen.
    @objc(validRequestorForSendType:returnType:) func validRequestor(forSendType sendType: NSPasteboard.PasteboardType?, returnType: NSPasteboard.PasteboardType?) -> Any? {
        (sendType == .string && returnType == nil && core.selectionCanBeSent()) ? self : nil
    }

    func writeSelection(to pboard: NSPasteboard, types: [NSPasteboard.PasteboardType]) -> Bool {
        guard types.contains(.string), let text = core.selectionTextNow() else { return false }
        pboard.clearContents()
        return pboard.setString(text, forType: .string)
    }

    func readSelection(from pboard: NSPasteboard) -> Bool { false }

    /// The page's fit, for QA.
    func engineFit() -> Double {
        windowController?.pages.fitForQA ?? 1
    }

    /// What every menu item, panel button and zoom key comes to.
    func perform(_ id: String) {
        switch core.perform(id: id) {
        case .none: break
        case .showOpenPanel: openDocument(nil)
        case .showAbout: NSApp.orderFrontStandardAboutPanel(nil)
        case .showSettings: showSettings()
        case .quit: NSApp.terminate(nil)
        case .closeWindow: windowController?.window.performClose(nil)
        case .showSavePanel: showSavePanel()
        case .confirmClose: confirmClose()
        }
    }

    /// Save As's sheet, the system's own, with Rust's words: the name it suggests, the folder, and a line for a signed document.
    func showSavePanel() {
        guard let window = windowController?.window, window.attachedSheet == nil else { return }
        let model = core.savePanel()
        let panel = NSSavePanel()
        panel.allowedContentTypes = [.pdf]
        panel.nameFieldStringValue = model.suggestedName
        panel.prompt = model.prompt
        if !model.directory.isEmpty { panel.directoryURL = URL(fileURLWithPath: model.directory, isDirectory: true) }
        if let message = model.message { panel.message = message }
        let core = self.core
        core.sheetChanged(open: true)
        sheetShowing = ["kind": "save_panel", "name": model.suggestedName, "message": model.message ?? "", "directory": model.directory]
        panel.beginSheetModal(for: window) { [weak self] response in
            core.sheetChanged(open: false)
            self?.sheetShowing = nil
            guard response == .OK, let url = panel.url else { return }
            // The sheet has asked whether to replace a file that's there, so a path that exists was agreed to.
            core.saveAs(path: url.path, replace: FileManager.default.fileExists(atPath: url.path))
        }
    }

    /// Closing a document with unsaved edits asks, as any Mac app asks: Save, Don't Save or Cancel. A quit doesn't, as edits come back.
    func confirmClose() {
        guard let window = windowController?.window, window.attachedSheet == nil else { return }
        let prompt = core.closePrompt()
        let alert = NSAlert()
        alert.messageText = prompt.message
        alert.informativeText = prompt.informative
        alert.addButton(withTitle: prompt.saveTitle)
        alert.addButton(withTitle: prompt.cancelTitle)
        let dontSave = alert.addButton(withTitle: prompt.dontSaveTitle)
        dontSave.hasDestructiveAction = true
        let core = self.core
        core.sheetChanged(open: true)
        sheetShowing = ["kind": "close_prompt", "message": prompt.message, "informative": prompt.informative, "buttons": [prompt.saveTitle, prompt.cancelTitle, prompt.dontSaveTitle]]
        alert.beginSheetModal(for: window) { [weak self] response in
            core.sheetChanged(open: false)
            self?.sheetShowing = nil
            switch response {
            case .alertFirstButtonReturn: core.closeAnswered(choice: .save)
            case .alertThirdButtonReturn: core.closeAnswered(choice: .dontSave)
            default: core.closeAnswered(choice: .cancel)
            }
        }
    }

    @objc func openDocument(_ sender: Any?) {
        let panel = NSOpenPanel()
        panel.allowedContentTypes = [.pdf]
        panel.allowsMultipleSelection = true
        panel.title = strings.openPanelTitle
        let core = self.core
        // An update waits for a panel.
        core.sheetChanged(open: true)
        panel.begin { response in
            core.sheetChanged(open: false)
            guard response == .OK else { return }
            for url in panel.urls { core.openFile(path: url.path) }
        }
    }
}
