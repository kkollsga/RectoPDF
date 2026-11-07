import AppKit

/// The drawing's colours and sizes, from `docs/concepts/`. Which appearance is on comes from the view that's asking.
enum Theme {
    static let sidebarWidth: CGFloat = 272

    static func color(_ light: UInt32, _ dark: UInt32) -> NSColor {
        NSColor(name: nil) { appearance in
            rgb(appearance.bestMatch(from: [.aqua, .darkAqua]) == .darkAqua ? dark : light)
        }
    }

    static func rgb(_ value: UInt32) -> NSColor {
        NSColor(srgbRed: CGFloat((value >> 16) & 0xFF) / 255, green: CGFloat((value >> 8) & 0xFF) / 255, blue: CGFloat(value & 0xFF) / 255, alpha: 1)
    }

    static let sidebar = color(0xF6F7F8, 0x1B1D21)
    static let margin = color(0xE8EAEC, 0x121316)
    static let hairline = color(0xD9DBDE, 0x2C2E33)
    static let rowActive = color(0xE6E7E9, 0x2C2E33)
    static let textStrong = color(0x17181A, 0xF2F3F4)
    static let textBody = color(0x24262A, 0xE6E7E9)
    static let textQuiet = color(0x63666B, 0xA8ABB0)
    static let accent = color(0x176A94, 0x84CBF0)
    static let railRest = color(0xC6C8CC, 0x3C3F45)
    static let pill = color(0xF6F7F8, 0x24262A)
    static let buttonFill = color(0x176A94, 0x2B7EA9)
    static let bookLeft = color(0xD3D5D8, 0x3C3F45)

    static func cg(_ color: NSColor, in view: NSView) -> CGColor {
        var resolved = color.cgColor
        view.effectiveAppearance.performAsCurrentDrawingAppearance { resolved = color.cgColor }
        return resolved
    }

    static func isDark(_ view: NSView) -> Bool {
        view.effectiveAppearance.bestMatch(from: [.aqua, .darkAqua]) == .darkAqua
    }
}
