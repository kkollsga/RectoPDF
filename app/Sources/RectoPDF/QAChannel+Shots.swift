import AppKit
import CryptoKit
import QuartzCore

/// Pictures of the window, Settings and the menus.
extension QAChannel {
    /// The shots verbs. Answers `nil` for a verb that isn't one of them, `true` when the verb ends as most do (the window laid out, then the report), `false` when it has already answered or failed.
    func qaShots(_ id: UInt64, _ verb: String, _ value: String, _ controller: MainWindowController?) -> Bool? {
        switch verb {
        case "shot":
            shot(id: id, path: value)
            return false
        case "shot_settings":
            guard let window = delegate.settings?.window, let view = window.contentView?.superview else { return fail(id, "The Settings window isn't open.") }
            window.layoutIfNeeded()
            plainShot(of: view, id: id, path: value)
            return false
        case "shot_menu":
            guard let menu = NSApp.mainMenu?.items.first?.submenu else { return fail(id, "There's no menu.") }
            menu.update()
            plainShot(of: MenuDrawing(menu: menu), id: id, path: value)
            return false
        case "shot_menus":
            // Each menu, then any submenu one of its own items opens, as File's Open Recent, so what's in it is seen too. The system's own, such as Services, aren't ours to show.
            let menus = (NSApp.mainMenu?.items ?? []).compactMap(\.submenu).flatMap { menu in [menu] + menu.items.filter { ($0.representedObject as? String)?.isEmpty == false && $0.submenu?.items.isEmpty == false }.compactMap(\.submenu) }
            guard !menus.isEmpty else { return fail(id, "There's no menu.") }
            let drawings = menus.map { menu -> MenuDrawing in
                menu.update()
                return MenuDrawing(menu: menu)
            }
            let row = FlippedView(frame: NSRect(x: 0, y: 0, width: drawings.reduce(0) { $0 + $1.frame.width + 8 }, height: drawings.map(\.frame.height).max() ?? 0))
            var x: CGFloat = 0
            for drawing in drawings {
                drawing.frame.origin = NSPoint(x: x, y: 0)
                row.addSubview(drawing)
                x += drawing.frame.width + 8
            }
            plainShot(of: row, id: id, path: value)
            return false
        default:
            return nil
        }
    }

    /// Warm paper and highlights in a shot: the sum the compositor does, on the picture. `render(in:)` leaves compositing filters out. With `lighten`, each channel is the brighter of the two, which is what a highlight on dark paper is.
    nonisolated static func multiply(_ rep: NSBitmapImageRep, by rgb: UInt32, in rects: [NSRect], viewHeight: CGFloat, flipped: Bool, lighten: Bool = false) {
        guard let data = rep.bitmapData else { return }
        let (red, green, blue) = (Int((rgb >> 16) & 0xFF), Int((rgb >> 8) & 0xFF), Int(rgb & 0xFF))
        let (stride, width, height) = (rep.bytesPerRow, rep.pixelsWide, rep.pixelsHigh)
        for rect in rects {
            let top = flipped ? rect.minY : viewHeight - rect.maxY
            let (x0, x1) = (max(Int(rect.minX * 2), 0), min(Int(rect.maxX * 2), width))
            let (y0, y1) = (max(Int(top * 2), 0), min(Int((top + rect.height) * 2), height))
            guard x0 < x1, y0 < y1 else { continue }
            for y in y0..<y1 {
                let row = data + y * stride
                for x in x0..<x1 {
                    let pixel = row + x * 4
                    if lighten {
                        pixel[0] = UInt8(max(Int(pixel[0]), red))
                        pixel[1] = UInt8(max(Int(pixel[1]), green))
                        pixel[2] = UInt8(max(Int(pixel[2]), blue))
                    } else {
                        pixel[0] = UInt8(Int(pixel[0]) * red / 255)
                        pixel[1] = UInt8(Int(pixel[1]) * green / 255)
                        pixel[2] = UInt8(Int(pixel[2]) * blue / 255)
                    }
                }
            }
        }
    }

    /// A view as it is, with none of what the page area's own layers need done to them.
    func plainShot(of view: NSView, id: UInt64, path: String) {
        let bounds = view.bounds
        guard let rep = NSBitmapImageRep(bitmapDataPlanes: nil, pixelsWide: Int(bounds.width * 2), pixelsHigh: Int(bounds.height * 2), bitsPerSample: 8, samplesPerPixel: 4, hasAlpha: true, isPlanar: false, colorSpaceName: .deviceRGB, bytesPerRow: 0, bitsPerPixel: 0) else { return fail(id, "The picture couldn't be made.") }
        rep.size = bounds.size
        view.cacheDisplay(in: bounds, to: rep)
        guard let data = rep.representation(using: .png, properties: [:]) else { return fail(id, "The picture couldn't be encoded.") }
        do {
            try data.write(to: URL(fileURLWithPath: path), options: .atomic)
            answer(id, ["out": path, "width": rep.pixelsWide, "height": rep.pixelsHigh, "bytes": data.count])
        } catch {
            let _: Void = fail(id, "The picture couldn't be written: \(error.localizedDescription)")
        }
    }

    /// A sheet over the window is a window of its own, which the window's drawing leaves out: its own drawing goes in where it sits, so a shot shows what a person would. A panel drawn by another process, as the save sheet's file browser is, may come out partly blank.
    static func drawSheet(of window: NSWindow, sheets: SheetPresenter, into rep: NSBitmapImageRep, flipped: Bool, height: CGFloat) {
        guard let sheet = sheets.sheetWindow(over: window), let origin = sheets.sheetOrigin(over: window), let sheetView = sheet.contentView?.superview ?? sheet.contentView else { return }
        // A sheet takes its window's look, light or dark. One that's held, never attached, isn't told when that changes.
        if sheet.appearance?.name != window.effectiveAppearance.name { sheet.appearance = window.effectiveAppearance }
        sheetView.layoutSubtreeIfNeeded()
        let size = sheetView.bounds
        guard let sheetRep = sheetView.bitmapImageRepForCachingDisplay(in: size) else { return }
        sheetView.cacheDisplay(in: size, to: sheetRep)
        let x = origin.x
        var y = origin.y
        if flipped { y = height - y - size.height }
        NSGraphicsContext.saveGraphicsState()
        NSGraphicsContext.current = NSGraphicsContext(bitmapImageRep: rep)
        // Over what's there, so its rounded corners show the window behind, as the window server would composite it. A plain draw copies them in clear.
        sheetRep.draw(in: NSRect(x: x, y: y, width: size.width, height: size.height), from: .zero, operation: .sourceOver, fraction: 1, respectFlipped: false, hints: nil)
        NSGraphicsContext.restoreGraphicsState()
    }

    func shot(id: UInt64, path: String) {
        guard let controller = delegate.windowController else { return fail(id, "The window is closed, so there's nothing to draw.") }
        controller.window.layoutIfNeeded()
        controller.root.layoutSubtreeIfNeeded()
        // The frame view, so the window's buttons are in it.
        let view: NSView = controller.root.superview ?? controller.root
        let bounds = view.bounds
        guard let rep = NSBitmapImageRep(bitmapDataPlanes: nil, pixelsWide: Int(bounds.width * 2), pixelsHigh: Int(bounds.height * 2), bitsPerSample: 8, samplesPerPixel: 4, hasAlpha: true, isPlanar: false, colorSpaceName: .deviceRGB, bytesPerRow: 0, bitsPerPixel: 0) else { return fail(id, "The picture couldn't be made.") }
        rep.size = bounds.size
        // Each pane's: a comparison has several.
        let tints = controller.paneAreas.compactMap { $0.tintRectsForShot(in: view) }
        let tint = tints.first.map { (rgb: $0.rgb, rects: tints.flatMap(\.rects)) }
        let highlights = controller.paneAreas.compactMap { $0.highlightsForShot(in: view) }.first
        let marks = controller.paneAreas.compactMap { $0.marksForShot(in: view) }
        for area in controller.paneAreas {
            area.hideTintForShot(true)
            area.hideHighlightsForShot(true)
            area.hideMarksForShot(true)
        }
        CATransaction.flush()
        view.cacheDisplay(in: bounds, to: rep)
        QAChannel.drawSheet(of: controller.window, sheets: delegate.sheets, into: rep, flipped: view.isFlipped, height: bounds.height)
        for area in controller.paneAreas {
            area.hideTintForShot(false)
            area.hideHighlightsForShot(false)
            area.hideMarksForShot(false)
        }
        let flipped = view.isFlipped
        let core = delegate.core
        // The encoding and the write are slow, so they stay off this thread.
        nonisolated(unsafe) let encoded = rep
        DispatchQueue.global(qos: .userInitiated).async {
            for mark in marks { QAChannel.multiply(encoded, by: mark.rgb, in: mark.rects, viewHeight: bounds.height, flipped: flipped, lighten: mark.lighten) }
            if let highlights { QAChannel.multiply(encoded, by: highlights.rgb, in: highlights.rects, viewHeight: bounds.height, flipped: flipped, lighten: highlights.lighten) }
            if let tint { QAChannel.multiply(encoded, by: tint.rgb, in: tint.rects, viewHeight: bounds.height, flipped: flipped) }
            guard let data = encoded.representation(using: .png, properties: [:]) else {
                core.qaAnswer(id: id, ok: false, json: "The picture couldn't be encoded.")
                return
            }
            do {
                try data.write(to: URL(fileURLWithPath: path), options: .atomic)
                let answer = try JSONSerialization.data(withJSONObject: ["out": path, "width": encoded.pixelsWide, "height": encoded.pixelsHigh, "bytes": data.count])
                core.qaAnswer(id: id, ok: true, json: String(data: answer, encoding: .utf8) ?? "null")
            } catch {
                core.qaAnswer(id: id, ok: false, json: "The picture couldn't be written: \(error.localizedDescription)")
            }
        }
    }
}

/// A drawing of a menu's real items, for a shot: a real menu can't be opened with no screen. What's listed and what's enabled are the menu's own, as AppKit has them. The look is near enough to read, and it isn't the system's own drawing.
private final class MenuDrawing: NSView {
    private let rows: [NSMenuItem]
    private static let rowHeight: CGFloat = 22
    private static let lineHeight: CGFloat = 11
    private static let width: CGFloat = 280

    init(menu: NSMenu) {
        rows = menu.items.filter { !$0.isHidden }
        let height = rows.reduce(CGFloat(10)) { $0 + ($1.isSeparatorItem ? Self.lineHeight : Self.rowHeight) }
        super.init(frame: NSRect(x: 0, y: 0, width: Self.width, height: height))
    }

    @available(*, unavailable) required init?(coder: NSCoder) { fatalError() }

    override var isFlipped: Bool { true }

    override func draw(_ dirtyRect: NSRect) {
        NSApp.effectiveAppearance.performAsCurrentDrawingAppearance {
            NSColor.windowBackgroundColor.setFill()
            bounds.fill()
            NSColor.separatorColor.setStroke()
            NSBezierPath(rect: bounds.insetBy(dx: 0.5, dy: 0.5)).stroke()
            let font = NSFont.menuFont(ofSize: 13)
            var y: CGFloat = 5
            for item in rows {
                if item.isSeparatorItem {
                    NSColor.separatorColor.setFill()
                    NSRect(x: 12, y: y + Self.lineHeight / 2, width: bounds.width - 24, height: 1).fill()
                    y += Self.lineHeight
                    continue
                }
                let colour: NSColor = item.isEnabled ? .labelColor : .tertiaryLabelColor
                if item.state == .on { NSAttributedString(string: "✓", attributes: [.font: font, .foregroundColor: colour]).draw(at: NSPoint(x: 6, y: y + 2)) }
                NSAttributedString(string: item.title, attributes: [.font: font, .foregroundColor: colour]).draw(at: NSPoint(x: 22, y: y + 2))
                if !item.keyEquivalent.isEmpty {
                    let control = item.keyEquivalentModifierMask.contains(.control) ? "⌃" : ""
                    let key = NSAttributedString(string: control + "⌘" + item.keyEquivalent.uppercased(), attributes: [.font: font, .foregroundColor: item.isEnabled ? NSColor.secondaryLabelColor : NSColor.tertiaryLabelColor])
                    key.draw(at: NSPoint(x: bounds.width - key.size().width - 14, y: y + 2))
                }
                y += Self.rowHeight
            }
        }
    }
}
