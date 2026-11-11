import AppKit

/// `recto qa accessibility`: what the system does to the app's own views for VoiceOver and its display settings, by the same methods, with nothing posted. The words and what's folded are Rust's (`recto-ffi`'s `sidebar_access.rs`); this only finds the view and calls what the system would.
extension MainWindowController {
    /// Does one, and says what went wrong if anything did.
    func qaAccessibility(_ value: String) -> String? {
        let parts = value.split(separator: ":").map(String.init)
        switch (parts.first, parts.count) {
        case ("contrast", 2):
            ThumbView.increaseContrast = parts[1] == "on"
            return nil
        case ("key", 2):
            return sidebar.foldKey(parts[1]) ? nil : "Nothing to fold or open there."
        case ("fold", 2):
            guard let row = Int(parts[1]) else { return "A row is a number." }
            return sidebar.performFoldAction(row) ? nil : "That row has no fold, or isn't on screen."
        case ("thumb", 3):
            let thumbs: [String: ThumbView] = ["contents": sidebar.contentsThumb, "tabs": sidebar.tabsThumb, "find": sidebar.find.thumb, "pages": pages.thumb]
            guard let thumb = thumbs[parts[1]] else { return "No such thumb." }
            let done = parts[2] == "increment" ? thumb.accessibilityPerformIncrement() : thumb.accessibilityPerformDecrement()
            return done ? nil : "That thumb has nothing to scroll."
        default:
            return "That isn't something `qa accessibility` takes."
        }
    }
}
