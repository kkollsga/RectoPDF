import AppKit

/// What keeps a quiet run off the screen when the system, rather than the app, would put it there.
///
/// LaunchServices finds a running app by its bundle: an `open` of the same bundle, whoever runs it, reopens the quiet run, and AppKit's handling of that turned it from prohibited into a regular app, with a Dock icon and a place in the menu bar, measured 2026-11-08 (`open -g` of the bundle a quiet run came from: `qa state` said `regular` after). A full check saw the same in a restore test while another worktree's update tests were opening their copies. So a quiet run answers the system's open, reopen and print events itself, by doing nothing: it's driven by its QA channel alone. And should anything else change its policy, it goes straight back.
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

    /// Back to no Dock icon, no menu bar and never active, if anything's changed that.
    static func keepProhibited() {
        guard NSApp.activationPolicy() != .prohibited else { return }
        NSApp.setActivationPolicy(.prohibited)
    }
}
