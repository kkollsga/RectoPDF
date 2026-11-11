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
    /// With `inPutBack` (`RECTO_QA_POLICY_NUDGE_MS=<ms>,in-put-back`), each look also finds the policy just changed, and one more change lands
    /// between the guard's first set and its read, as Launch Services' did: the race a report lost, made to happen at every look.
    func nudge(every milliseconds: UInt64, inPutBack: Bool) {
        Self.changesAtLooks = inPutBack
        nudge = Timer.scheduledTimer(withTimeInterval: Double(milliseconds) / 1000, repeats: true) { _ in
            MainActor.assumeIsolated { _ = NSApp.setActivationPolicy(.accessory) }
        }
    }

    /// Back to no Dock icon, no menu bar and never active, if anything's changed that, and the policy it then reads. Said once for each policy in the run's messages, so a failure's logs show it happened and what it was.
    ///
    /// **Put back until a read says so,** at most `mostTries` times in a row. Neither the set's answer nor one read after it can be taken alone (2026-11-11, three quiet runs side by side, each turned `accessory` every 15 ms and opened by `open -g` every other look): a set from `accessory` said it took, and the read straight after said `regular`, as Launch Services' change from the `open` landed in it, so 2 of 300 reports said `regular`. And a set from `regular` said it hadn't, and the read said `prohibited`. If it still isn't after that, it's tried again on the next turn of the run loop, once at a time.
    @discardableResult
    static func keepProhibited() -> NSApplication.ActivationPolicy {
        var policy = NSApp.activationPolicy()
        guard policy != .prohibited else { return policy }
        let from = name(of: policy)
        var tries = 0
        var firstRead = policy
        while policy != .prohibited && tries < mostTries {
            tries += 1
            _ = NSApp.setActivationPolicy(.prohibited)
            if landsInPutBack {
                landsInPutBack = false
                _ = NSApp.setActivationPolicy(.accessory)
            }
            policy = NSApp.activationPolicy()
            if tries == 1 { firstRead = policy }
        }
        if saidPutBack.insert(from).inserted {
            FileHandle.standardError.write(Data("recto: this quiet run's activation was changed to \(from) from outside, and put back.\n".utf8))
        }
        if tries > 1 && saidPutBack.insert("again from \(name(of: firstRead))").inserted {
            FileHandle.standardError.write(Data("recto: putting this quiet run's activation back took \(tries) tries: after the first it read \(name(of: firstRead)).\n".utf8))
        }
        if policy == .prohibited {
            turnsTried = 0
        } else if !retryPending && turnsTried < mostTurns {
            retryPending = true
            turnsTried += 1
            if saidPutBack.insert("later").inserted {
                FileHandle.standardError.write(Data("recto: this quiet run's activation still read \(name(of: policy)) after \(mostTries) tries, so it's put back again on the next turn.\n".utf8))
            }
            DispatchQueue.main.async {
                MainActor.assumeIsolated {
                    retryPending = false
                    keepProhibited()
                }
            }
        } else if !retryPending && saidPutBack.insert("gave up").inserted {
            FileHandle.standardError.write(Data("recto: this quiet run's activation still read \(name(of: policy)) after \(mostTurns) turns of tries, so it's tried again only when it next changes or is looked at.\n".utf8))
        }
        return policy
    }

    /// The policy as a look says it: put back first if anything's changed it, and what the guard read last, with nothing between. The system's change lands whenever it does, in a set as much as anywhere, so a read of its own after the guard's could catch one the guard hasn't seen.
    static func policyAsLookedAt() -> NSApplication.ActivationPolicy {
        if changesAtLooks {
            _ = NSApp.setActivationPolicy(.accessory)
            landsInPutBack = true
        }
        return keepProhibited()
    }

    /// How many puts back in a row, each read after: a few calls on the main thread at most, never a wait.
    private static let mostTries = 8
    /// How many turns of the run loop in a row it tries again on, before it waits for the next change or look: never a spin.
    private static let mostTurns = 50
    private static var retryPending = false
    private static var turnsTried = 0
    /// A test's changes at each look, and the one waiting to land in the put-back that look brings: see `nudge(every:inPutBack:)`.
    private static var changesAtLooks = false
    private static var landsInPutBack = false

    private static func name(of policy: NSApplication.ActivationPolicy) -> String {
        switch policy {
        case .regular: "regular"
        case .accessory: "accessory"
        default: "prohibited"
        }
    }

    /// Each policy it's been put back from, said once: a test's own change to `accessory` mustn't hide the system's to `regular`.
    private static var saidPutBack: Set<String> = []
}
