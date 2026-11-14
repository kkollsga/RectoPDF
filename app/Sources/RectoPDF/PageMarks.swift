import AppKit

/// The tab's unsaved highlights (R4.3), as Rust last gave them: a layer of each page's that has any, as the selection's is, so it scrolls and zooms
/// with the page and no tile knows. Rust decides the rectangles, the colour and how it blends; this only draws, and holds what it's given to what can
/// be drawn, as it does a renderer's output: finite, of some area, inside its page, and no more than Rust's own caps.
@MainActor
final class PageMarks {
    /// Rust's caps on one set (`recto_reader::tiles::marks`): past them a set is cut, as it can only have been made wrongly.
    static let maxPages = 10_000
    static let maxRects = 65_536

    private var byPage: [UInt32: [HighlightRect]] = [:]
    private var rgb: UInt32 = 0
    private var lighten = false
    private var layers: [UInt32: CAShapeLayer] = [:]

    /// A new set, replacing the last, drawn at once on the pages there are. Kept across a clear of the pages, as Rust gives a new one when the tab
    /// changes.
    func set(_ pages: [MarkPage], rgb: UInt32, lighten: Bool, on pageLayers: [UInt32: CALayer]) {
        self.rgb = rgb
        self.lighten = lighten
        byPage.removeAll()
        var left = Self.maxRects
        for page in pages.prefix(Self.maxPages) where left > 0 {
            let sound = page.rects.filter { rect in
                [rect.x, rect.y, rect.width, rect.height].allSatisfy(\.isFinite) && rect.width > 0 && rect.height > 0 && rect.x >= 0 && rect.y >= 0
            }.prefix(left)
            left -= sound.count
            if !sound.isEmpty { byPage[page.page] = Array(sound) }
        }
        for page in Array(layers.keys) where pageLayers[page] == nil || byPage[page] == nil {
            layers.removeValue(forKey: page)?.removeFromSuperlayer()
        }
        for (page, layer) in pageLayers { draw(page, on: layer) }
    }

    /// A page's layer placed or moved: what it has is drawn on it.
    func shown(page: UInt32, on layer: CALayer) {
        draw(page, on: layer)
    }

    /// A page's layer gone, and its highlights' with it.
    func hidden(page: UInt32) {
        layers.removeValue(forKey: page)
    }

    /// Every page's layer gone.
    func cleared() {
        layers.removeAll()
    }

    /// What's drawn, by page, for a shot, which leaves compositing out and does the same sum on the picture.
    var forShot: (rgb: UInt32, lighten: Bool, byPage: [UInt32: [HighlightRect]])? {
        layers.isEmpty ? nil : (rgb, lighten, byPage.filter { layers[$0.key] != nil })
    }

    func hideForShot(_ hidden: Bool) {
        for layer in layers.values { layer.isHidden = hidden }
    }

    private func draw(_ page: UInt32, on parent: CALayer) {
        let bounds = parent.bounds.insetBy(dx: -0.5, dy: -0.5)
        let rects = (byPage[page] ?? []).map { CGRect(x: $0.x, y: $0.y, width: $0.width, height: $0.height) }.filter { bounds.contains($0) }
        guard !rects.isEmpty else {
            layers.removeValue(forKey: page)?.removeFromSuperlayer()
            return
        }
        let layer = layers[page] ?? {
            let made = CAShapeLayer()
            made.anchorPoint = .zero
            // Under the selection's, which is 5, and over the tiles.
            made.zPosition = 4
            parent.addSublayer(made)
            layers[page] = made
            return made
        }()
        if layer.superlayer !== parent { parent.addSublayer(layer) }
        layer.frame = parent.bounds
        layer.fillColor = Theme.rgb(rgb).cgColor
        layer.compositingFilter = lighten ? "lightenBlendMode" : "multiplyBlendMode"
        let path = CGMutablePath()
        for rect in rects { path.addRect(rect) }
        layer.path = path
    }
}
