import AppKit

/// The main-thread block and the autorelease cleanup it leaves behind are one watched interval.
@MainActor enum MainWork {
    static func perform(_ work: @escaping @MainActor () -> Void) {
        // These blocks are not events; AppKit's event pool may otherwise keep their objects until another event arrives.
        run(work, busy: { watchedCore?.mainBusy(block: true) ?? false }, idle: { watchedCore?.mainIdle() }, pool: { body in
            autoreleasepool(invoking: body)
        })
    }

    /// Synchronous OS boundary, also exercised with a pool whose drain tail can be observed without delaying the UI.
    static func run(_ work: @escaping @MainActor () -> Void, busy: () -> Bool, idle: () -> Void, pool: (@MainActor () -> Void) -> Void) {
        let began = busy()
        defer { if began { idle() } }
        pool(work)
    }

    static func entryPointChecks() -> [String: Bool] {
        var active = false
        var drainedWhileActive = false
        var ends = 0
        run({}, busy: { active = true; return true }, idle: { active = false; ends += 1 }, pool: { body in
            body()
            drainedWhileActive = active
        })
        let ownInterval = drainedWhileActive && !active && ends == 1
        active = true
        drainedWhileActive = false
        ends = 0
        run({}, busy: { false }, idle: { active = false; ends += 1 }, pool: { body in
            body()
            drainedWhileActive = active
        })
        return [
            "unbracketed_main_pool_drains_while_watched": ownInterval,
            "a_main_pool_keeps_its_parent_watch": drainedWhileActive && active && ends == 0,
        ]
    }
}
