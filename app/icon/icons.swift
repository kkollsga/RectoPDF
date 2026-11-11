// RectoPDF app icon concepts: one geometry, two outputs.
//
//   swift app/icon/icons.swift <out-dir>
//
// Each concept is a list of filled paths on Apple's 1024 grid (the macOS icon body is an 824 x 824 rounded square, its continuous
// "squircle" corners drawn here as a superellipse, centred with a 100 px margin, with a soft drop shadow below it). The same list is
// written as `app/icon/<concept>.svg` (the vector source) and drawn with CoreGraphics into PNGs at 1024, 128, 32 and 16 px, plus a
// comparison sheet per concept on a light and a dark desktop. Drawing the PNGs from the list, rather than asking NSImage to render the
// SVG, keeps both outputs from one source and needs no SVG renderer: NSImage's SVG support skips filters, so the shadow would be lost.
// No dependencies beyond AppKit.
import AppKit

// MARK: Geometry

enum Seg { case m(Double, Double), l(Double, Double), c(Double, Double, Double, Double, Double, Double), z }
typealias Path = [Seg]

struct Stop { var at: Double; var rgb: UInt32; var alpha: Double = 1 }
enum Paint {
    case solid(UInt32, Double)
    /// From (x1, y1) to (x2, y2), in canvas units.
    case linear(Double, Double, Double, Double, [Stop])
}
struct Shadow { var dy: Double; var blur: Double; var alpha: Double; var rgb: UInt32 = 0x000000 }
struct Item {
    var path: Path
    var paint: Paint
    var shadow: Shadow? = nil
    /// Clip to the icon body, so nothing spills past the squircle.
    var clipped = true
}
struct Concept { var name: String; var title: String; var line: String; var items: [Item] }

let k = 0.5522847498

func rrect(_ x: Double, _ y: Double, _ w: Double, _ h: Double, _ r: Double) -> Path { rrect4(x, y, w, h, r, r, r, r) }

/// A rounded rectangle with its own radius per corner: top left, top right, bottom right, bottom left.
func rrect4(_ x: Double, _ y: Double, _ w: Double, _ h: Double, _ tl: Double, _ tr: Double, _ br: Double, _ bl: Double) -> Path {
    [.m(x + tl, y), .l(x + w - tr, y), .c(x + w - tr + tr * k, y, x + w, y + tr - tr * k, x + w, y + tr),
     .l(x + w, y + h - br), .c(x + w, y + h - br + br * k, x + w - br + br * k, y + h, x + w - br, y + h),
     .l(x + bl, y + h), .c(x + bl - bl * k, y + h, x, y + h - bl + bl * k, x, y + h - bl),
     .l(x, y + tl), .c(x, y + tl - tl * k, x + tl - tl * k, y, x + tl, y), .z]
}

/// Apple's icon body: 824 x 824 at (100, 100), continuous corners, approximated by a superellipse of exponent 5.
func squircle(_ x: Double = 100, _ y: Double = 100, _ s: Double = 824, n: Double = 5) -> Path {
    let cx = x + s / 2, cy = y + s / 2, a = s / 2
    var p: Path = []
    let steps = 720
    for i in 0..<steps {
        let t = Double(i) / Double(steps) * 2 * .pi
        let ct = cos(t), st = sin(t)
        let px = cx + a * (ct < 0 ? -1 : 1) * pow(abs(ct), 2 / n)
        let py = cy + a * (st < 0 ? -1 : 1) * pow(abs(st), 2 / n)
        p.append(i == 0 ? .m(px, py) : .l(px, py))
    }
    p.append(.z)
    return p
}

/// The four-point star of the in-app agent button: points on the axes, sides pulled in towards the centre.
func star(_ cx: Double, _ cy: Double, _ r: Double, pinch: Double = 0.16) -> Path {
    let q = r * pinch
    return [.m(cx, cy - r),
            .c(cx + q * 0.6, cy - q * 1.6, cx + q * 1.6, cy - q * 0.6, cx + r, cy),
            .c(cx + q * 1.6, cy + q * 0.6, cx + q * 0.6, cy + q * 1.6, cx, cy + r),
            .c(cx - q * 0.6, cy + q * 1.6, cx - q * 1.6, cy + q * 0.6, cx - r, cy),
            .c(cx - q * 1.6, cy - q * 0.6, cx - q * 0.6, cy - q * 1.6, cx, cy - r), .z]
}

// MARK: Palette (the app's accent is 0x176A94 light, 0x84CBF0 dark)

let accent: UInt32 = 0x176A94
let accentLight: UInt32 = 0x84CBF0
let navy: UInt32 = 0x0E3550
let paper: UInt32 = 0xFFFFFF

let body = squircle()
func background(_ top: UInt32, _ bottom: UInt32) -> Item {
    Item(path: body, paint: .linear(0, 100, 0, 924, [Stop(at: 0, rgb: top), Stop(at: 1, rgb: bottom)]),
         shadow: Shadow(dy: 10, blur: 22, alpha: 0.28), clipped: false)
}
/// A faint light edge along the top of the body, the "subtle depth" of current macOS icons.
let sheen = Item(path: body, paint: .linear(0, 100, 0, 512, [Stop(at: 0, rgb: 0xFFFFFF, alpha: 0.10), Stop(at: 1, rgb: 0xFFFFFF, alpha: 0)]))
func textLines(x: Double, y: Double, widths: [Double], gap: Double, height: Double, rgb: UInt32, alpha: Double) -> [Item] {
    widths.enumerated().map { i, w in Item(path: rrect(x, y + Double(i) * gap, w, height, height / 2), paint: .solid(rgb, alpha)) }
}

// MARK: Concepts

/// 1. Recto: an open book seen from its right, the left page (verso) narrow and in shade, the right page (recto) wide, lit and forward.
/// The asymmetry is the point: Apple Books is a symmetric white book, this is one lit page with the book behind it.
func recto() -> Concept {
    let g = 404.0
    // Page tops rise from the gutter like a book lying open; bottoms follow.
    let verso: Path = [.m(g, 318), .c(352, 290, 290, 282, 232, 292), .l(232, 712), .c(290, 702, 352, 710, g, 744), .z]
    let rectoPage: Path = [.m(g, 318), .c(520, 284, 660, 276, 800, 290), .l(800, 716), .c(660, 702, 520, 708, g, 744), .z]
    let under: Path = [.m(g, 342), .c(340, 310, 280, 304, 214, 314), .l(214, 746), .c(280, 738, 340, 744, g, 776),
                       .c(520, 742, 660, 734, 818, 744), .l(818, 314), .c(660, 300, 520, 308, g, 342), .z]
    var items = [background(0x2283B8, 0x10577F), sheen,
                 Item(path: under, paint: .solid(navy, 0.55)),
                 Item(path: verso, paint: .linear(232, 0, g, 0, [Stop(at: 0, rgb: 0x9FCDE8), Stop(at: 1, rgb: 0x5F9CC5)])),
                 Item(path: rectoPage, paint: .linear(g, 0, 800, 0, [Stop(at: 0, rgb: 0xDCEBF5), Stop(at: 0.14, rgb: paper), Stop(at: 1, rgb: paper)]),
                      shadow: Shadow(dy: 6, blur: 18, alpha: 0.25, rgb: navy))]
    items += textLines(x: 474, y: 378, widths: [150], gap: 0, height: 30, rgb: accent, alpha: 0.95)
    items += textLines(x: 474, y: 456, widths: [254, 226, 254, 170], gap: 58, height: 20, rgb: accent, alpha: 0.30)
    return Concept(name: "1-recto", title: "1  Recto",
                   line: "An open book seen from its right, the narrow left page in shade and the wide right page (the recto) lit and forward: the app's name as a picture, reading at a glance, and lopsided on purpose so it isn't Apple Books' symmetric white book.",
                   items: items)
}

/// 2. Spine: one portrait page bound on its left, its inner edge shading into the binding like the right page of a book.
func spine() -> Concept {
    let x = 300.0, y = 196.0, w = 460.0, h = 632.0
    var items = [background(0xF1F6FA, 0xC4D7E6),
                 Item(path: rrect4(x, y, w, h, 6, 30, 30, 6), paint: .solid(paper, 1), shadow: Shadow(dy: 10, blur: 26, alpha: 0.22, rgb: navy)),
                 Item(path: rrect(x, y, 70, h, 0), paint: .linear(x, 0, x + 70, 0, [Stop(at: 0, rgb: accent, alpha: 0.28), Stop(at: 1, rgb: accent, alpha: 0)])),
                 Item(path: rrect(x - 52, y - 14, 60, h + 28, 18), paint: .linear(0, y, 0, y + h, [Stop(at: 0, rgb: 0x1E7DB0), Stop(at: 1, rgb: 0x115A82)]),
                      shadow: Shadow(dy: 4, blur: 10, alpha: 0.25, rgb: navy))]
    items += textLines(x: x + 96, y: y + 92, widths: [210], gap: 0, height: 34, rgb: accent, alpha: 0.95)
    items += textLines(x: x + 96, y: y + 186, widths: [300, 270, 300, 220, 300, 250], gap: 62, height: 20, rgb: 0x8FA6B8, alpha: 0.55)
    return Concept(name: "2-spine", title: "2  Spine",
                   line: "A single page bound on its left in the accent blue, its inner edge shading into the binding: a document at a glance, the right-hand page of a book on a second look, light and calm where Acrobat is red.",
                   items: items)
}

/// 3. Turn: a white page on a night-blue tile, its lower right corner lifting to show the next page underneath.
func turn() -> Concept {
    let x = 262.0, y = 184.0, w = 500.0, h = 656.0, r = 26.0, c = 250.0
    let rx = x + w, by = y + h
    // The fold, bowed in towards the page's middle as a curling sheet's is.
    let fold: [Seg] = [.c(rx - c * 0.30, by - c * 0.66, rx - c * 0.66, by - c * 0.30, rx - c, by)]
    // The page with its lower right corner cut along the fold.
    let page: Path = [.m(x + r, y), .l(rx - r, y), .c(rx - r + r * k, y, rx, y + r - r * k, rx, y + r), .l(rx, by - c)] + fold +
                     [.l(x + r, by), .c(x + r - r * k, by, x, by - r + r * k, x, by - r), .l(x, y + r), .c(x, y + r - r * k, x + r - r * k, y, x + r, y), .z]
    // The lifted flap, curling up and in from the fold, its tip towards the top left.
    let flap: Path = [.m(rx, by - c)] + fold +
                     [.c(rx - c * 1.16, by - c * 0.42, rx - c * 1.14, by - c * 0.80, rx - c * 1.04, by - c * 0.98),
                      .c(rx - c * 0.80, by - c * 1.12, rx - c * 0.40, by - c * 1.10, rx, by - c), .z]
    var items = [background(0x1A4466, 0x0B2338), sheen,
                 Item(path: rrect(x, y, w, h, r), paint: .solid(accentLight, 1), shadow: Shadow(dy: 10, blur: 26, alpha: 0.35)),
                 Item(path: page, paint: .solid(paper, 1))]
    items += textLines(x: x + 70, y: y + 96, widths: [220], gap: 0, height: 34, rgb: accent, alpha: 0.95)
    items += textLines(x: x + 70, y: y + 190, widths: [360, 330, 360, 280, 200], gap: 62, height: 20, rgb: 0x8FA6B8, alpha: 0.55)
    // Shaded across the curl like a rolled sheet: blue at the fold, white on the crest, a touch of blue again at the tip.
    items.append(Item(path: flap, paint: .linear(rx - c * 0.5, by - c * 0.5, rx - c * 1.05, by - c * 1.05,
                                                 [Stop(at: 0, rgb: 0x9CCFEC), Stop(at: 0.55, rgb: paper), Stop(at: 1, rgb: 0xD6E9F5)]),
                      shadow: Shadow(dy: 10, blur: 26, alpha: 0.40, rgb: navy)))
    return Concept(name: "3-turn", title: "3  Turn",
                   line: "A page on a night-blue tile with its corner lifting onto the next page: turning pages says reader rather than file, and a curl into blue is unlike Acrobat's red sheet or Preview's loupe.",
                   items: items)
}

/// 4. Star page: a white page with a folded corner, the agent's four-point star in the accent where the text would be.
func starPage() -> Concept {
    let x = 282.0, y = 176.0, w = 460.0, h = 672.0, r = 26.0, f = 128.0
    let rx = x + w, by = y + h
    let page: Path = [.m(x + r, y), .l(rx - f, y), .l(rx, y + f), .l(rx, by - r), .c(rx, by - r + r * k, rx - r + r * k, by, rx - r, by),
                      .l(x + r, by), .c(x + r - r * k, by, x, by - r + r * k, x, by - r), .l(x, y + r), .c(x, y + r - r * k, x + r - r * k, y, x + r, y), .z]
    let fold: Path = [.m(rx - f, y), .l(rx - f, y + f - 18), .c(rx - f, y + f - 8, rx - f + 8, y + f, rx - f + 18, y + f), .l(rx, y + f), .z]
    let items = [background(0x2283B8, 0x10577F), sheen,
                 Item(path: page, paint: .solid(paper, 1), shadow: Shadow(dy: 10, blur: 26, alpha: 0.30, rgb: navy)),
                 Item(path: fold, paint: .solid(0xB9DDF1, 1)),
                 Item(path: star(x + w / 2, y + h * 0.56, 168), paint: .linear(0, y + h * 0.56 - 168, 0, y + h * 0.56 + 168, [Stop(at: 0, rgb: 0x2283B8), Stop(at: 1, rgb: 0x10577F)]))]
    return Concept(name: "4-star", title: "4  Star page",
                   line: "A page with a folded corner carrying the app's own four-point star: a document shape at any size, tied to RectoPDF's agent button, blue not red, and no letters.",
                   items: items)
}

// MARK: SVG

func hex(_ c: UInt32) -> String { String(format: "#%06X", c) }
func f(_ v: Double) -> String { v == v.rounded() ? String(Int(v)) : String(format: "%.2f", v) }
func svgPath(_ p: Path) -> String {
    p.map {
        switch $0 {
        case let .m(x, y): return "M\(f(x)) \(f(y))"
        case let .l(x, y): return "L\(f(x)) \(f(y))"
        case let .c(a, b, c, d, e, g): return "C\(f(a)) \(f(b)) \(f(c)) \(f(d)) \(f(e)) \(f(g))"
        case .z: return "Z"
        }
    }.joined(separator: " ")
}
func svg(_ concept: Concept) -> String {
    var defs: [String] = ["<clipPath id=\"body\"><path d=\"\(svgPath(body))\"/></clipPath>"]
    var shapes: [String] = []
    for (i, item) in concept.items.enumerated() {
        var attrs: [String] = []
        switch item.paint {
        case let .solid(rgb, a):
            attrs.append("fill=\"\(hex(rgb))\"")
            if a < 1 { attrs.append("fill-opacity=\"\(f(a))\"") }
        case let .linear(x1, y1, x2, y2, stops):
            let s = stops.map { "<stop offset=\"\(f($0.at))\" stop-color=\"\(hex($0.rgb))\"\($0.alpha < 1 ? " stop-opacity=\"\(f($0.alpha))\"" : "")/>" }.joined()
            defs.append("<linearGradient id=\"g\(i)\" gradientUnits=\"userSpaceOnUse\" x1=\"\(f(x1))\" y1=\"\(f(y1))\" x2=\"\(f(x2))\" y2=\"\(f(y2))\">\(s)</linearGradient>")
            attrs.append("fill=\"url(#g\(i))\"")
        }
        if let s = item.shadow {
            // CoreGraphics' blur is about twice an SVG standard deviation.
            defs.append("<filter id=\"s\(i)\" x=\"-20%\" y=\"-20%\" width=\"140%\" height=\"140%\"><feDropShadow dx=\"0\" dy=\"\(f(s.dy))\" stdDeviation=\"\(f(s.blur / 2))\" flood-color=\"\(hex(s.rgb))\" flood-opacity=\"\(f(s.alpha))\"/></filter>")
            attrs.append("filter=\"url(#s\(i))\"")
        }
        let shape = "<path d=\"\(svgPath(item.path))\" \(attrs.joined(separator: " "))/>"
        shapes.append(item.clipped ? "<g clip-path=\"url(#body)\">\(shape)</g>" : shape)
    }
    return """
    <?xml version="1.0" encoding="UTF-8"?>
    <!-- RectoPDF app icon concept \(concept.title.replacingOccurrences(of: "  ", with: " ")). Generated by app/icon/icons.swift from the same geometry as its PNGs; edit the script, not this file. -->
    <svg xmlns="http://www.w3.org/2000/svg" width="1024" height="1024" viewBox="0 0 1024 1024">
    <defs>\(defs.joined(separator: "\n"))</defs>
    \(shapes.joined(separator: "\n"))
    </svg>

    """
}

// MARK: CoreGraphics

let space = CGColorSpace(name: CGColorSpace.sRGB)!
func cg(_ rgb: UInt32, _ a: Double = 1) -> CGColor {
    CGColor(colorSpace: space, components: [Double((rgb >> 16) & 0xFF) / 255, Double((rgb >> 8) & 0xFF) / 255, Double(rgb & 0xFF) / 255, a])!
}
func cgPath(_ p: Path) -> CGPath {
    let path = CGMutablePath()
    for s in p {
        switch s {
        case let .m(x, y): path.move(to: CGPoint(x: x, y: y))
        case let .l(x, y): path.addLine(to: CGPoint(x: x, y: y))
        case let .c(a, b, c, d, e, g): path.addCurve(to: CGPoint(x: e, y: g), control1: CGPoint(x: a, y: b), control2: CGPoint(x: c, y: d))
        case .z: path.closeSubpath()
        }
    }
    return path
}

/// Draws the concept into `ctx` at `size` px, the canvas's (0, 0) top left as in the SVG.
func draw(_ concept: Concept, in ctx: CGContext, size: Double) {
    let scale = size / 1024
    ctx.saveGState()
    ctx.translateBy(x: 0, y: size)
    ctx.scaleBy(x: scale, y: -scale)
    for item in concept.items {
        ctx.saveGState()
        if item.clipped { ctx.addPath(cgPath(body)); ctx.clip() }
        if let s = item.shadow {
            // Shadow offsets are in device space and y points up there, so "down" is negative.
            ctx.setShadow(offset: CGSize(width: 0, height: -s.dy * scale), blur: s.blur * scale, color: cg(s.rgb, s.alpha))
        }
        let path = cgPath(item.path)
        switch item.paint {
        case let .solid(rgb, a):
            ctx.addPath(path); ctx.setFillColor(cg(rgb, a)); ctx.fillPath()
        case let .linear(x1, y1, x2, y2, stops):
            let gradient = CGGradient(colorsSpace: space, colors: stops.map { cg($0.rgb, $0.alpha) } as CFArray, locations: stops.map { CGFloat($0.at) })!
            // A gradient doesn't cast a shadow, so draw the shape once in a solid colour under it for the shadow.
            if item.shadow != nil { ctx.addPath(path); ctx.setFillColor(cg(stops[0].rgb, stops[0].alpha)); ctx.fillPath(); ctx.setShadow(offset: .zero, blur: 0, color: nil) }
            ctx.addPath(path); ctx.clip()
            ctx.drawLinearGradient(gradient, start: CGPoint(x: x1, y: y1), end: CGPoint(x: x2, y: y2), options: [.drawsBeforeStartLocation, .drawsAfterEndLocation])
        }
        ctx.restoreGState()
    }
    ctx.restoreGState()
}

func bitmap(_ w: Int, _ h: Int) -> CGContext {
    let ctx = CGContext(data: nil, width: w, height: h, bitsPerComponent: 8, bytesPerRow: 0, space: space, bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue)!
    ctx.interpolationQuality = .high
    return ctx
}
func icon(_ concept: Concept, _ size: Int) -> CGImage {
    let ctx = bitmap(size, size)
    draw(concept, in: ctx, size: Double(size))
    return ctx.makeImage()!
}
func writePNG(_ image: CGImage, _ url: URL) {
    let rep = NSBitmapImageRep(cgImage: image)
    try! rep.representation(using: .png, properties: [:])!.write(to: url)
}

/// One sheet: a light desktop above, a dark one below; on each the 1024 shown at 512, then 128, 32 and 16 at true size, then 32 and
/// 16 enlarged four times with hard pixels so their actual pixels can be judged, then the 16 beside a file name as in a Finder list.
func sheet(_ concept: Concept, icons: [Int: CGImage]) -> CGImage {
    let w = 1640, rowH = 620, top = 70, h = top + 2 * rowH
    let ctx = bitmap(w, h)
    ctx.setFillColor(cg(0xFFFFFF)); ctx.fill(CGRect(x: 0, y: 0, width: w, height: h))
    let ns = NSGraphicsContext(cgContext: ctx, flipped: false)
    NSGraphicsContext.current = ns
    func text(_ s: String, _ x: Double, _ yTop: Double, _ size: Double, _ color: NSColor, weight: NSFont.Weight = .regular) {
        let a = NSAttributedString(string: s, attributes: [.font: NSFont.systemFont(ofSize: size, weight: weight), .foregroundColor: color])
        a.draw(at: NSPoint(x: x, y: Double(h) - yTop - a.size().height))
    }
    text(concept.title, 30, 18, 26, .black, weight: .semibold)
    for (row, dark) in [false, true].enumerated() {
        let y0 = top + row * rowH // from the top
        let bg: UInt32 = dark ? 0x1E1F22 : 0xECECEF
        let fg: NSColor = dark ? NSColor(white: 0.92, alpha: 1) : NSColor(white: 0.12, alpha: 1)
        let sub: NSColor = dark ? NSColor(white: 0.6, alpha: 1) : NSColor(white: 0.45, alpha: 1)
        ctx.setFillColor(cg(bg)); ctx.fill(CGRect(x: 0, y: h - y0 - rowH, width: w, height: rowH))
        func put(_ img: CGImage, _ x: Int, _ yTop: Int, _ size: Int, crisp: Bool = false) {
            ctx.interpolationQuality = crisp ? .none : .high
            ctx.draw(img, in: CGRect(x: x, y: h - yTop - size, width: size, height: size))
        }
        put(icons[1024]!, 40, y0 + 54, 512)
        text(dark ? "Dark desktop" : "Light desktop", 40, Double(y0 + 16), 15, sub)
        let cy = y0 + 54 + 256 // the row's middle
        put(icons[128]!, 610, cy - 64, 128); text("128", 660, Double(cy + 80), 13, sub)
        put(icons[32]!, 790, cy - 16, 32); text("32", 797, Double(cy + 80), 13, sub)
        put(icons[16]!, 870, cy - 8, 16); text("16", 870, Double(cy + 80), 13, sub)
        put(icons[32]!, 950, cy - 64, 128, crisp: true); text("32 at 4x", 982, Double(cy + 80), 13, sub)
        put(icons[16]!, 1110, cy - 32, 64, crisp: true); text("16 at 4x", 1112, Double(cy + 80), 13, sub)
        // A Finder list row: 16 px icon and a name.
        for (i, name) in ["RectoPDF", "Quarterly report.pdf"].enumerated() {
            let ry = cy - 30 + i * 30
            if i == 0 { put(icons[16]!, 1240, ry, 16) } else { put(icons[16]!, 1240, ry, 16) }
            text(name, 1264, Double(ry - 1), 13, fg)
        }
        text("Finder list", 1240, Double(cy + 80), 13, sub)
    }
    NSGraphicsContext.current = nil
    return ctx.makeImage()!
}

// MARK: Main

let args = CommandLine.arguments
guard args.count == 2 else { print("usage: swift app/icon/icons.swift <out-dir>"); exit(2) }
let out = URL(fileURLWithPath: args[1], isDirectory: true)
let sourceDir = URL(fileURLWithPath: #filePath).deletingLastPathComponent()
try! FileManager.default.createDirectory(at: out, withIntermediateDirectories: true)
for concept in [recto(), spine(), turn(), starPage()] {
    try! svg(concept).write(to: sourceDir.appendingPathComponent("\(concept.name).svg"), atomically: true, encoding: .utf8)
    var icons: [Int: CGImage] = [:]
    for size in [1024, 128, 32, 16] {
        icons[size] = icon(concept, size)
        writePNG(icons[size]!, out.appendingPathComponent("\(concept.name)-\(size).png"))
    }
    writePNG(sheet(concept, icons: icons), out.appendingPathComponent("\(concept.name)-sheet.png"))
    print("\(concept.name): \(concept.line)")
}
