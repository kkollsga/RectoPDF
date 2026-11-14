import AppKit
import CryptoKit
import QuartzCore

extension AppDelegate {
    /// What `installMenu` marks every item it makes with, so `qa state` can say which items are AppKit's own (AutoFill, Passwords…) by what ours aren't.
    static let builtItem = NSUserInterfaceItemIdentifier("recto.menu")
}

/// The menu bar, the right-click menu and the keys the window answers.
extension QAChannel {
    /// The menus verbs. Answers `nil` for a verb that isn't one of them, `true` when the verb ends as most do (the window laid out, then the report), `false` when it has already answered or failed.
    func qaMenus(_ id: UInt64, _ verb: String, _ value: String, _ controller: MainWindowController?) -> Bool? {
        switch verb {
        case "menu":
            guard let item = menuItem(value), let action = item.action else { return fail(id, "That isn't on the menu bar.") }
            // As a person can only choose what isn't greyed out, by what AppKit makes of it now.
            item.menu?.update()
            guard item.isEnabled else { return fail(id, "That's greyed out in the menu right now.") }
            NSApp.sendAction(action, to: item.target, from: item)
            return true
        case "context_menu":
            guard let controller else { return fail(id, "The window is closed.") }
            let menu = controller.readingPane.contextMenu()
            menu.update()
            guard let item = menu.items.first(where: { ($0.representedObject as? String) == value }), let action = item.action else { return fail(id, "That isn't in the right-click menu.") }
            guard item.isEnabled else { return fail(id, "That's greyed out in the right-click menu right now.") }
            NSApp.sendAction(action, to: item.target, from: item)
            return true
        case "key_action":
            // What a key or the menu does with the window key, as it is for a person: the first in the window's own chain that answers it, asked whether it can, then sent it. A quiet window isn't key, so AppKit's own route would skip the window, and this doesn't.
            guard let controller else { return fail(id, "The window is closed.") }
            let action = NSSelectorFromString(value + ":")
            guard let (target, enabled) = QAChannel.keyWindowTarget(for: action, in: controller.window) else { return fail(id, "Nothing in the window answers that.") }
            guard enabled else { return fail(id, "That's greyed out with the window key.") }
            target.perform(action, with: nil)
            return true
        default:
            return nil
        }
    }

    /// An item of the real menu bar, by its id.
    func menuItem(_ id: String) -> NSMenuItem? {
        // Submenus' items too, as Open Recent's files are.
        let items = NSApp.mainMenu?.items.compactMap(\.submenu).flatMap(\.items) ?? []
        return (items + items.compactMap(\.submenu).flatMap(\.items)).first { ($0.representedObject as? String) == id && !$0.isHidden }
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

    /// This area's part of the report.
    func menusSection() -> [String: Any] {
        return [
            // From Rust's list and its word on what's enabled, as the real menu is built from. Building an NSMenu on every state read costs the app megabytes that the budgets would count; `qa context-menu` goes through the real one.
            "context_menu": delegate.core.contextMenu(pane: delegate.core.readingPane()).filter { !$0.id.isEmpty }.map { ["id": $0.id, "title": $0.title, "checked": $0.checked, "enabled": delegate.core.menuItemEnabled(id: $0.id)] as [String: Any] },
            "menu_bar": (NSApp.mainMenu?.items ?? []).compactMap(\.submenu).map { menu -> [String: Any] in
                // What AppKit itself says after asking whoever answers, as it does on opening a menu, and no copy of the rule.
                menu.update()
                return ["title": menu.title, "items": menu.items.filter { !$0.isSeparatorItem && !$0.isHidden }.flatMap { item -> [NSMenuItem] in
                    // A submenu's items follow its own, as Open Recent's files do.
                    item.submenu?.update()
                    return [item] + (item.submenu?.items.filter { !$0.isSeparatorItem && !$0.isHidden } ?? [])
                }.map { ["id": ($0.representedObject as? String) ?? "", "title": $0.title, "key": $0.keyEquivalent, "mask": $0.keyEquivalentModifierMask.rawValue, "checked": $0.state == .on, "enabled": $0.isEnabled, "system": $0.identifier != AppDelegate.builtItem] as [String: Any] }]
            },
        ]
    }
}
