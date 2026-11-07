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
        onMain { [weak self] in self?.delegate?.windowController?.pages.drain() }
    }

    func qaRequest(id: UInt64, verb: String, value: String) {
        onMain { [weak self] in self?.delegate?.qa?.handle(id: id, verb: verb, value: value) }
    }

    func sessionEnded() {
        onMain { NSApp.terminate(nil) }
    }
}

@MainActor final class AppDelegate: NSObject, NSApplicationDelegate {
    let core: AppCore
    let isQuiet: Bool
    let strings: AppStrings
    let limits: ReaderLimits
    private let bridge: Bridge
    private(set) var windowController: MainWindowController?
    private(set) var qa: QAChannel?
    private var mainObserver: CFRunLoopObserver?

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

    func applicationDidFinishLaunching(_ notification: Notification) {
        watchMainThread()
        installMenu()
        openWindow()
        core.ready()
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
        controller.show()
        modelChanged()
    }

    /// What the window's closing does, whoever closed it.
    func windowDidClose() {
        windowController = nil
        core.windowClosed()
    }

    func modelChanged() {
        windowController?.refreshModel()
        installMenu()
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
                let item = menu.addItem(withTitle: entry.title, action: #selector(menuChosen(_:)), keyEquivalent: entry.key)
                item.target = self
                item.representedObject = entry.id
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
        case .quit: NSApp.terminate(nil)
        case .closeWindow: windowController?.window.performClose(nil)
        }
    }

    @objc func openDocument(_ sender: Any?) {
        let panel = NSOpenPanel()
        panel.allowedContentTypes = [.pdf]
        panel.allowsMultipleSelection = true
        panel.title = strings.openPanelTitle
        let core = self.core
        panel.begin { response in
            guard response == .OK else { return }
            for url in panel.urls { core.openFile(path: url.path) }
        }
    }
}
