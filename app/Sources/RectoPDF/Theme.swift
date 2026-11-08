import AppKit

/// A colour for each appearance, made once. Everything's told again when the appearance changes, so nothing here is a dynamic colour: a text field given a dynamic `NSColor` keeps a dependency record per setting, 7 KiB a tab in all, and for as long as the app runs.
struct ThemeColor {
    private let light: NSColor
    private let darkColor: NSColor

    init(_ lightValue: UInt32, _ darkValue: UInt32) {
        light = Theme.rgb(lightValue)
        darkColor = Theme.rgb(darkValue)
    }

    /// For the appearance the window is in now.
    @MainActor var ns: NSColor { Theme.dark ? darkColor : light }
    @MainActor var cg: CGColor { ns.cgColor }
}

/// The drawing's colours and sizes, from `docs/concepts/`.
enum Theme {
    /// Set when the window's appearance changes, on the UI thread.
    @MainActor static var dark = false

    static func rgb(_ value: UInt32) -> NSColor {
        NSColor(srgbRed: CGFloat((value >> 16) & 0xFF) / 255, green: CGFloat((value >> 8) & 0xFF) / 255, blue: CGFloat(value & 0xFF) / 255, alpha: 1)
    }

    static let sidebar = ThemeColor(0xF6F7F8, 0x1B1D21)
    static let margin = ThemeColor(0xE8EAEC, 0x121316)
    static let hairline = ThemeColor(0xD9DBDE, 0x2C2E33)
    static let rowActive = ThemeColor(0xE6E7E9, 0x2C2E33)
    static let textStrong = ThemeColor(0x17181A, 0xF2F3F4)
    static let textBody = ThemeColor(0x24262A, 0xE6E7E9)
    static let textQuiet = ThemeColor(0x63666B, 0xA8ABB0)
    static let accent = ThemeColor(0x176A94, 0x84CBF0)
    static let railRest = ThemeColor(0xC6C8CC, 0x3C3F45)
    static let pill = ThemeColor(0xF6F7F8, 0x24262A)
    static let buttonFill = ThemeColor(0x176A94, 0x2B7EA9)
    static let bookLeft = ThemeColor(0xD3D5D8, 0x3C3F45)

    static func isDark(_ view: NSView) -> Bool {
        view.effectiveAppearance.bestMatch(from: [.aqua, .darkAqua]) == .darkAqua
    }
}

/// Text, drawn with no text field. `NSTextField` on macOS 27 makes a view each time it lays out, and each of those leaves a dependency record behind in AppKit for good (`-[NSKeyValueDependencyInfo initWithOwner:]`). A sidebar's labels lay out at every page crossed, and it came to 7 KiB for each tab opened and more while reading. This keeps no such thing, and draws only when told.
final class TextLine: NSView {
    private var string = NSAttributedString()
    private var wraps = false
    private var alignment: NSTextAlignment = .left
    private var measured: NSSize = .zero

    override var isFlipped: Bool { true }
    override var isOpaque: Bool { false }

    /// Returns whether anything changed.
    @discardableResult
    func set(_ text: String, font: NSFont, color: NSColor, alignment: NSTextAlignment = .left, wraps: Bool = false) -> Bool {
        let style = NSMutableParagraphStyle()
        style.alignment = alignment
        style.lineBreakMode = wraps ? .byWordWrapping : .byTruncatingTail
        let next = NSAttributedString(string: text, attributes: [.font: font, .foregroundColor: color, .paragraphStyle: style])
        guard next != string || wraps != self.wraps else { return false }
        string = next
        self.wraps = wraps
        self.alignment = alignment
        measured = .zero
        needsDisplay = true
        return true
    }

    var text: String { string.string }

    /// Styled text on one line, cut at its end, as a Find row with its match in bold.
    func setAttributed(_ next: NSAttributedString) {
        guard next != string || wraps else { return }
        string = next
        wraps = false
        measured = .zero
        needsDisplay = true
    }

    /// The room it takes, on one line or wrapped to this width.
    func size(width: CGFloat = 10_000) -> NSSize {
        if wraps {
            let box = string.boundingRect(with: NSSize(width: width, height: 10_000), options: [.usesLineFragmentOrigin])
            return NSSize(width: ceil(box.width), height: ceil(box.height))
        }
        if measured == .zero {
            let box = string.size()
            measured = NSSize(width: ceil(box.width), height: ceil(box.height))
        }
        return measured
    }

    /// How many lines it's drawn on, at the height it's been given: for QA, which counts what the eye sees.
    var lineCount: Int {
        let one = ceil(string.size().height)
        guard one > 0, !string.string.isEmpty else { return 0 }
        return max(1, Int((bounds.height / one).rounded()))
    }

    override func draw(_ dirtyRect: NSRect) {
        string.draw(with: bounds, options: wraps ? [.usesLineFragmentOrigin] : [.usesLineFragmentOrigin, .truncatesLastVisibleLine])
    }
}
