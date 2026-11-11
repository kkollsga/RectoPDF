import AppKit

/// What keeps a quiet run off the screen when the system, rather than the app, would put it there.
///
/// LaunchServices finds a running app by its bundle: an `open` of the same bundle, whoever runs it, reopens the quiet run, and AppKit's handling of that turned it from prohibited into a regular app, with a Dock icon and a place in the menu bar, measured 2026-11-08 (`open -g` of the bundle a quiet run came from: `qa state` said `regular` after). A full check saw the same in a restore test while another worktree's update tests were opening their copies. So a quiet run answers the system's open, reopen and print events itself, by doing nothing: it's driven by its QA channel alone. And should anything else change its policy, it goes straight back. The change itself isn't the app's to stop: Launch Services makes the running app regular before it sends the reopen, as a notification on the main run loop (`runningApplicationNotificationCallback`, from the change's own stack, 2026-11-11), and the reopen event comes some 40 ms later. The guard puts it back about a millisecond after the change, and a report reads the policy through it (`policyAsLookedAt`).
@MainActor final class QuietRunGuard: NSObject {
    private var policyWatch: NSKeyValueObservation?
    /// Held for as long as it's watched: each `current` is an object of its own.
    private let running = NSRunningApplication.current

    /// Before AppKit finishes launching, so its own handlers for these events are never the ones that run.
    func install() {
        let events = NSAppleEventManager.shared()
        // Not the launch's own open-application event, which AppKit goes by to finish launching.
        for (eventClass, eventID) in [(kCoreEventClass, kAEReopenApplication), (kCoreEventClass, kAEOpenDocuments), (kCoreEventClass, kAEPrintDocuments), (AEEventClass(kInternetEventClass), AEEventID(kAEGetURL))] {
            events.setEventHandler(self, andSelector: #selector(ignore(_:withReply:)), forEventClass: AEEventClass(eventClass), andEventID: AEEventID(eventID))
        }
        NSApp.setActivationPolicy(.prohibited)
        policyWatch = running.observe(\.activationPolicy, options: [.new]) { _, _ in
            // Told on whichever thread changed it.
            onMain { QuietRunGuard.keepProhibited() }
        }
    }

    @objc private func ignore(_ event: NSAppleEventDescriptor, withReply reply: NSAppleEventDescriptor) {}

    private var nudge: Timer?

    /// A test's stand-in for the system changing the policy from outside (`RECTO_QA_POLICY_NUDGE_MS`, read in Rust): every so often the run turns itself `accessory`, on the main thread, as AppKit would.
    func nudge(every milliseconds: UInt64) {
        nudge = Timer.scheduledTimer(withTimeInterval: Double(milliseconds) / 1000, repeats: true) { _ in
            MainActor.assumeIsolated { _ = NSApp.setActivationPolicy(.accessory) }
        }
    }

    /// Back to no Dock icon, no menu bar and never active, if anything's changed that. Said once for each policy in the run's messages, so a failure's logs show it happened and what it was.
    static func keepProhibited() {
        let policy = NSApp.activationPolicy()
        guard policy != .prohibited else { return }
        NSApp.setActivationPolicy(.prohibited)
        let name = policy == .regular ? "regular" : "accessory"
        if saidPutBack.insert(name).inserted {
            FileHandle.standardError.write(Data("recto: this quiet run's activation was changed to \(name) from outside, and put back.\n".utf8))
        }
    }

    /// The policy as a look says it: put back first if anything's changed it, then read at once. The system's change lands whenever it does, and only reading right after the guard has looked keeps a report from catching the moment before the guard hears of it.
    static func policyAsLookedAt() -> NSApplication.ActivationPolicy {
        keepProhibited()
        return NSApp.activationPolicy()
    }

    /// Each policy it's been put back from, said once: a test's own change to `accessory` mustn't hide the system's to `regular`.
    private static var saidPutBack: Set<String> = []
}
