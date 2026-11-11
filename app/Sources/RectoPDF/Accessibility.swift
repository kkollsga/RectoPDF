import AppKit
import recto_ffiFFI

/// `recto qa accessibility`: what the system does to the app's own views for VoiceOver and its display settings, by the same methods, with nothing posted. The step is read, and what's said when it does nothing is worded, by Rust (`recto-ffi`'s `sidebar_access.rs`); this only finds the view and calls what the system would.
extension MainWindowController {
    /// Does one, and says what went wrong if anything did.
    func qaAccessibility(_ value: String) -> String? {
        let core = delegate.core
        guard let step = core.qaAccessibilityStep(value: value) else { return core.qaAccessibilityRefusal(value: value) }
        let done: Bool
        switch step {
        case let .contrast(on):
            ThumbView.increaseContrast = on
            done = true
        case let .foldKey(key):
            done = sidebar.foldKey(key)
        case let .foldAction(row):
            done = sidebar.performFoldAction(Int(row))
        case let .thumb(list, increment):
            let thumbs: [String: ThumbView] = ["contents": sidebar.contentsThumb, "tabs": sidebar.tabsThumb, "find": sidebar.find.thumb, "pages": pages.thumb]
            let thumb = thumbs[list]
            done = (increment ? thumb?.accessibilityPerformIncrement() : thumb?.accessibilityPerformDecrement()) ?? false
        }
        return done ? nil : core.qaAccessibilityRefusal(value: value)
    }
}

/// A scroll thumb to VoiceOver: a vertical scroll bar, named for its list, with its place as its value, and a page either way as its increment and decrement, as a click beside it. The words and the page are Rust's (`thumb_access`).
extension ThumbView {
    /// How far down its list is, 0 to 1.
    func fraction() -> Double {
        guard let scrollView, let document = scrollView.documentView else { return 0 }
        let visible = scrollView.contentView.bounds.height
        let extra = room()
        let total = ((document as? NSTableView).map { table in table.numberOfRows > 0 ? table.rect(ofRow: table.numberOfRows - 1).maxY : 0 } ?? document.frame.height) + extra.top + extra.bottom
        guard total > visible else { return 0 }
        return Double(min(max((scrollView.contentView.bounds.minY + extra.top) / (total - visible), 0), 1))
    }

    func spoken() -> ThumbAccess? { MainActor.assumeIsolated { Self.access?(list, fraction()) } }

    // A scroll bar to VoiceOver, with its place as its value, and a page either way as its increment and decrement, as a click beside it.
    override func isAccessibilityElement() -> Bool { true }
    override func accessibilityRole() -> NSAccessibility.Role? { .scrollBar }
    override func accessibilityOrientation() -> NSAccessibilityOrientation { .vertical }
    override func accessibilityLabel() -> String? { spoken()?.label }
    override func accessibilityValue() -> Any? { spoken().map { NSNumber(value: $0.value) } }
    override func accessibilityValueDescription() -> String? { spoken()?.valueText }
    override func accessibilityPerformIncrement() -> Bool { step(1) }
    override func accessibilityPerformDecrement() -> Bool { step(-1) }

    /// A page down or up, as a click below or above it: what its increment and decrement do.
    @discardableResult
    func step(_ way: CGFloat) -> Bool {
        guard let scrollView, knob() != nil else { return false }
        let page = scrollView.contentView.bounds.height * CGFloat(spoken()?.pageShare ?? 0.9)
        scroll(to: scrollView.contentView.bounds.minY + way * page)
        return true
    }
}
