import AppKit

/// The native construction boundaries. Rust owns their validation, names, storage and end of capture.
@MainActor enum StartupWatch {
    /// Wire IDs of recto_app::watchdog::startup; these describe native facts, with no host policy.
    enum Phase: UInt8 {
        case launchBegin = 0, menuReady, servicesReady, restoreQueued, restoreBegin, appearanceReady
        case windowBegin, windowBody, nativeWindowReady, pageReady, sidebarReady, windowLayoutReady
        case windowConstructed, windowOpened, hostReady
        case menuModelReady = 15, servicesMenuReady, menuBuilt, windowModelReady
    }

    static func mark(_ phase: Phase) {
        hostStartupPhase(phase: phase.rawValue)
    }

    /// Installing the run-loop observer during this callback cannot bracket the callback already in progress.
    static func launch(_ work: @escaping @MainActor () -> Void) {
        MainWork.perform {
            mark(.launchBegin)
            work()
            mark(.restoreQueued)
        }
    }

    /// The outer boundary includes stored-property initialization, which happens before init's body.
    static func constructWindow(delegate: AppDelegate) -> MainWindowController {
        mark(.windowBegin)
        let controller = MainWindowController(delegate: delegate)
        mark(.windowConstructed)
        return controller
    }
}
