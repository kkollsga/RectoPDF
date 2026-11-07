import AppKit
import UniformTypeIdentifiers

/// Runs a block on the UI thread, in every run-loop mode, including while a menu is tracking or a panel is open.
func onMain(_ work: @escaping @MainActor () -> Void) {
    let main = CFRunLoopGetMain()
    let modes = [CFRunLoopMode.commonModes.rawValue, RunLoop.Mode.eventTracking.rawValue as CFString, RunLoop.Mode.modalPanel.rawValue as CFString, CFRunLoopMode.defaultMode.rawValue] as CFArray
    CFRunLoopPerformBlock(main, modes) {
        MainActor.assumeIsolated { work() }
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
        if !isQuiet { installMenu() }
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

    /// Only File and View's few items for now, in Rust's words. The real menu bar comes from a Rust model in a later phase.
    private func installMenu() {
        let bar = NSMenu()
        let appItem = NSMenuItem()
        let appMenu = NSMenu()
        appMenu.addItem(withTitle: strings.menuQuit, action: #selector(NSApplication.terminate(_:)), keyEquivalent: "q")
        appItem.submenu = appMenu
        bar.addItem(appItem)
        let fileItem = NSMenuItem()
        let fileMenu = NSMenu(title: strings.menuFile)
        fileMenu.addItem(withTitle: strings.menuOpen, action: #selector(openDocument(_:)), keyEquivalent: "o").target = self
        fileMenu.addItem(withTitle: strings.menuCloseWindow, action: #selector(NSWindow.performClose(_:)), keyEquivalent: "w")
        fileItem.submenu = fileMenu
        bar.addItem(fileItem)
        let viewItem = NSMenuItem()
        let viewMenu = NSMenu(title: strings.menuView)
        viewMenu.addItem(withTitle: strings.menuZoomIn, action: #selector(zoomIn(_:)), keyEquivalent: "+").target = self
        viewMenu.addItem(withTitle: strings.menuZoomOut, action: #selector(zoomOut(_:)), keyEquivalent: "-").target = self
        viewMenu.addItem(withTitle: strings.menuActualSize, action: #selector(fitWidth(_:)), keyEquivalent: "0").target = self
        viewMenu.addItem(.separator())
        let focus = viewMenu.addItem(withTitle: strings.menuFocus, action: #selector(toggleFocus(_:)), keyEquivalent: "f")
        focus.keyEquivalentModifierMask = [.command, .control]
        focus.target = self
        viewMenu.addItem(withTitle: strings.menuWhitePaper, action: #selector(toggleWhitePaper(_:)), keyEquivalent: "").target = self
        viewItem.submenu = viewMenu
        bar.addItem(viewItem)
        NSApp.mainMenu = bar
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

    @objc func zoomIn(_ sender: Any?) { windowController?.pages.zoom(by: limits.zoomStep) }
    @objc func zoomOut(_ sender: Any?) { windowController?.pages.zoom(by: 1 / limits.zoomStep) }
    @objc func fitWidth(_ sender: Any?) { windowController?.pages.setZoom(1) }
    @objc func toggleFocus(_ sender: Any?) { core.toggleFocus() }
    @objc func toggleWhitePaper(_ sender: Any?) { core.toggleWhitePaper() }
}
