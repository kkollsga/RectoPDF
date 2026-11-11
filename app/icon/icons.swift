// RectoPDF's app icon, "Recto": an open book seen from its right, the narrow left page (verso) in shade, the wide right page (recto) lit
// and forward, on the app's calm blue. The owner chose it on 2026-11-11 from four concepts (this file's history has the other three).
//
//   swift app/icon/icons.swift <out-dir>      Writes, into <out-dir>:
//     AppIcon.iconset/   every size a Mac bundle needs, for `iconutil -c icns` (16, 32, 128, 256, 512 at 1x and 2x)
//     AppIcon.icon/      the same artwork as an Icon Composer document, with a dark appearance, for `actool` (macOS 26's dark and tinted modes)
//     AppIcon.svg        the large artwork as SVG, for looking at
//     sheet.png          every size on a light and a dark desktop, for looking at
//
// This file is the vector source. The artwork is a list of filled paths on Apple's 1024 grid (the icon body an 824 x 824 rounded square
// at a 100 px margin, its continuous corners drawn as a superellipse, with a soft drop shadow). CoreGraphics draws the PNGs from the list
// and the SVGs are written from it, so there's one geometry and no SVG renderer is needed. Below 64 px a smooth curve or a 1.25 px line
// blurs, so 16 and 32 px have their own drawings on the pixel grid: the same book with straight edges, whole-pixel lines and less text.
// AppKit only. app/icon/build.sh runs it and turns its output into AppIcon.icns and Assets.car.
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
    /// Clip to the icon body, so nothing spills past it.
    var clipped = true
    /// Which Icon Composer layer it belongs to; nil for the body, which Icon Composer draws itself.
    var layer: String? = nil
}
/// One drawing: the body it clips to and what's drawn, in 1024-canvas units.
struct Art { var body: Path; var items: [Item] }

let k = 0.5522847498

func rect(_ x: Double, _ y: Double, _ w: Double, _ h: Double) -> Path { [.m(x, y), .l(x + w, y), .l(x + w, y + h), .l(x, y + h), .z] }
func rrect(_ x: Double, _ y: Double, _ w: Double, _ h: Double, _ r: Double) -> Path {
    [.m(x + r, y), .l(x + w - r, y), .c(x + w - r + r * k, y, x + w, y + r - r * k, x + w, y + r),
     .l(x + w, y + h - r), .c(x + w, y + h - r + r * k, x + w - r + r * k, y + h, x + w - r, y + h),
     .l(x + r, y + h), .c(x + r - r * k, y + h, x, y + h - r + r * k, x, y + h - r),
     .l(x, y + r), .c(x, y + r - r * k, x + r - r * k, y, x + r, y), .z]
}

/// Apple's icon body: an s x s square at (x, y) with continuous corners, approximated by a superellipse of exponent 5.
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

// MARK: Palette (the app's accent is 0x176A94 in light, 0x84CBF0 in dark)

let accent: UInt32 = 0x176A94
let navy: UInt32 = 0x0E3550
let paper: UInt32 = 0xFFFFFF
let tileTop: UInt32 = 0x2283B8, tileBottom: UInt32 = 0x10577F
/// The tile in macOS's dark appearance: the same blue, deeper, so the lit page stands out as it does on a dark desktop.
let darkTop: UInt32 = 0x1A4A6A, darkBottom: UInt32 = 0x0B263A
let versoOuter: UInt32 = 0x9FCDE8, versoGutter: UInt32 = 0x5F9CC5

func tile(_ body: Path, top: Double, bottom: Double, shadow: Shadow?) -> Item {
    Item(path: body, paint: .linear(0, top, 0, bottom, [Stop(at: 0, rgb: tileTop), Stop(at: 1, rgb: tileBottom)]), shadow: shadow, clipped: false)
}

// MARK: The artwork

/// The large artwork, 64 px and up.
func large() -> Art {
    let body = squircle()
    let g = 404.0
    // Page tops rise from the gutter like a book lying open; bottoms follow.
    let verso: Path = [.m(g, 318), .c(352, 290, 290, 282, 232, 292), .l(232, 712), .c(290, 702, 352, 710, g, 744), .z]
    let recto: Path = [.m(g, 318), .c(520, 284, 660, 276, 800, 290), .l(800, 716), .c(660, 702, 520, 708, g, 744), .z]
    let cover: Path = [.m(g, 342), .c(340, 310, 280, 304, 214, 314), .l(214, 746), .c(280, 738, 340, 744, g, 776),
                       .c(520, 742, 660, 734, 818, 744), .l(818, 314), .c(660, 300, 520, 308, g, 342), .z]
    var items = [tile(body, top: 100, bottom: 924, shadow: Shadow(dy: 10, blur: 22, alpha: 0.28)),
                 // A faint light edge along the top: the "subtle depth" of current macOS icons. Icon Composer adds its own, so no layer.
                 Item(path: body, paint: .linear(0, 100, 0, 512, [Stop(at: 0, rgb: 0xFFFFFF, alpha: 0.10), Stop(at: 1, rgb: 0xFFFFFF, alpha: 0)])),
                 Item(path: cover, paint: .solid(navy, 0.55), layer: "cover"),
                 Item(path: verso, paint: .linear(232, 0, g, 0, [Stop(at: 0, rgb: versoOuter), Stop(at: 1, rgb: versoGutter)]), layer: "verso"),
                 Item(path: recto, paint: .linear(g, 0, 800, 0, [Stop(at: 0, rgb: 0xDCEBF5), Stop(at: 0.14, rgb: paper), Stop(at: 1, rgb: paper)]),
                      shadow: Shadow(dy: 6, blur: 18, alpha: 0.25, rgb: navy), layer: "recto")]
    items.append(Item(path: rrect(474, 378, 150, 30, 15), paint: .solid(accent, 0.95), layer: "text"))
    for (i, w) in [254.0, 226, 254, 170].enumerated() {
        items.append(Item(path: rrect(474, 456 + Double(i) * 58, w, 20, 10), paint: .solid(accent, 0.30), layer: "text"))
    }
    return Art(body: body, items: items)
}

/// The book on a `grid`-pixel canvas, for 16 and 32 px: every edge on a pixel boundary, so nothing is half a pixel of blur.
/// `px` converts pixel coordinates to canvas units.
func small(_ grid: Int) -> Art {
    let u = 1024 / Double(grid)
    func box(_ x: Double, _ y: Double, _ w: Double, _ h: Double) -> Path { rect(x * u, y * u, w * u, h * u) }
    var items: [Item] = []
    let body: Path
    if grid == 16 {
        // A 14 px body: the 13 px of the 1024 grid's margin would put every edge between pixels.
        body = squircle(1 * u, 1 * u, 14 * u)
        items = [tile(body, top: u, bottom: 15 * u, shadow: nil),
                 Item(path: box(3, 5, 10, 7), paint: .solid(navy, 0.70)), // The cover: one dark row under the pages.
                 Item(path: box(3, 4, 3, 7), paint: .solid(0x82BADD, 1)),
                 Item(path: box(6, 4, 7, 7), paint: .solid(paper, 1)),
                 Item(path: box(7, 6, 4, 1), paint: .solid(accent, 0.85)),
                 Item(path: box(7, 8, 5, 1), paint: .solid(accent, 0.35))]
    } else {
        body = squircle(3 * u, 3 * u, 26 * u)
        items = [tile(body, top: 3 * u, bottom: 29 * u, shadow: Shadow(dy: 0.5 * u, blur: 1 * u, alpha: 0.30)),
                 Item(path: box(6, 10, 20, 15), paint: .solid(navy, 0.60)),
                 Item(path: box(7, 9, 6, 15), paint: .linear(7 * u, 0, 13 * u, 0, [Stop(at: 0, rgb: versoOuter), Stop(at: 1, rgb: versoGutter)])),
                 Item(path: box(13, 9, 12, 15), paint: .solid(paper, 1)),
                 Item(path: box(13, 9, 1, 15), paint: .solid(0xDCEBF5, 1)), // The gutter's shade on the lit page.
                 Item(path: box(15, 12, 6, 2), paint: .solid(accent, 0.95))]
        for (i, w) in [8.0, 7, 8].enumerated() {
            items.append(Item(path: box(15, 16 + Double(i) * 2, w, 1), paint: .solid(accent, 0.35)))
        }
    }
    return Art(body: body, items: items)
}

/// The drawing for an icon of `pixels`.
func art(forPixels pixels: Int) -> Art { pixels <= 16 ? small(16) : pixels <= 32 ? small(32) : large() }

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
/// `items` as an SVG document. `transform` wraps them all (Icon Composer's layers are full bleed); `clip` clips to the body.
func svg(_ items: [Item], body: Path, clip: Bool, shadows: Bool, transform: String? = nil) -> String {
    var defs: [String] = clip ? ["<clipPath id=\"body\"><path d=\"\(svgPath(body))\"/></clipPath>"] : []
    var shapes: [String] = []
    for (i, item) in items.enumerated() {
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
        if shadows, let s = item.shadow {
            // CoreGraphics' blur is about twice an SVG standard deviation.
            defs.append("<filter id=\"s\(i)\" x=\"-20%\" y=\"-20%\" width=\"140%\" height=\"140%\"><feDropShadow dx=\"0\" dy=\"\(f(s.dy))\" stdDeviation=\"\(f(s.blur / 2))\" flood-color=\"\(hex(s.rgb))\" flood-opacity=\"\(f(s.alpha))\"/></filter>")
            attrs.append("filter=\"url(#s\(i))\"")
        }
        let shape = "<path d=\"\(svgPath(item.path))\" \(attrs.joined(separator: " "))/>"
        shapes.append(clip && item.clipped ? "<g clip-path=\"url(#body)\">\(shape)</g>" : shape)
    }
    let content = shapes.joined(separator: "\n")
    return """
    <?xml version="1.0" encoding="UTF-8"?>
    <!-- RectoPDF's app icon. Generated by app/icon/icons.swift; edit that, not this. -->
    <svg xmlns="http://www.w3.org/2000/svg" width="1024" height="1024" viewBox="0 0 1024 1024">
    <defs>\(defs.joined(separator: "\n"))</defs>
    \(transform.map { "<g transform=\"\($0)\">\n\(content)\n</g>" } ?? content)
    </svg>

    """
}

// MARK: Icon Composer

/// An sRGB colour as Icon Composer writes one.
func ic(_ rgb: UInt32) -> String {
    String(format: "srgb:%.5f,%.5f,%.5f,1.00000", Double((rgb >> 16) & 0xFF) / 255, Double((rgb >> 8) & 0xFF) / 255, Double(rgb & 0xFF) / 255)
}

/// The artwork as an Icon Composer document: the tile is Icon Composer's own fill (a dark one in the dark appearance; the tinted
/// appearances are derived by the system), and the book's parts are SVG layers on its full-bleed canvas, where the body is the whole
/// 1024 square, so they're scaled up from the 824 body about its corner.
func writeIconComposer(_ art: Art, to dir: URL) throws {
    let assets = dir.appendingPathComponent("Assets")
    try FileManager.default.createDirectory(at: assets, withIntermediateDirectories: true)
    let scale = 1024.0 / 824.0
    let transform = "translate(\(f(-100 * scale)) \(f(-100 * scale))) scale(\(String(format: "%.6f", scale)))"
    // Top first, as Icon Composer lists them.
    let order = ["text", "recto", "verso", "cover"]
    for name in order {
        let items = art.items.filter { $0.layer == name }
        try svg(items, body: art.body, clip: false, shadows: false, transform: transform)
            .write(to: assets.appendingPathComponent("\(name).svg"), atomically: true, encoding: .utf8)
    }
    func gradient(_ top: UInt32, _ bottom: UInt32) -> [String: Any] {
        ["linear-gradient": [ic(top), ic(bottom)], "orientation": ["start": ["x": 0.5, "y": 0.0], "stop": ["x": 0.5, "y": 1.0]]]
    }
    let layers: [[String: Any]] = order.map { ["image-name": "\($0).svg", "name": $0, "glass": false] }
    let doc: [String: Any] = [
        "fill-specializations": [
            ["value": gradient(tileTop, tileBottom)],
            ["appearance": "dark", "value": gradient(darkTop, darkBottom)],
        ],
        "groups": [[
            "layers": layers,
            "shadow": ["kind": "neutral", "opacity": 0.5],
            "translucency": ["enabled": false, "value": 0.5],
            "specular": true,
        ]],
        "supported-platforms": ["squares": ["macOS"]],
    ]
    let json = try JSONSerialization.data(withJSONObject: doc, options: [.prettyPrinted, .sortedKeys])
    try json.write(to: dir.appendingPathComponent("icon.json"))
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

/// Draws `art` into `ctx` at `size` px, the canvas's (0, 0) top left as in the SVG.
func draw(_ art: Art, in ctx: CGContext, size: Double) {
    let scale = size / 1024
    ctx.saveGState()
    ctx.translateBy(x: 0, y: size)
    ctx.scaleBy(x: scale, y: -scale)
    for item in art.items {
        ctx.saveGState()
        if item.clipped { ctx.addPath(cgPath(art.body)); ctx.clip() }
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
func icon(_ pixels: Int) -> CGImage {
    let ctx = bitmap(pixels, pixels)
    draw(art(forPixels: pixels), in: ctx, size: Double(pixels))
    return ctx.makeImage()!
}
func writePNG(_ image: CGImage, _ url: URL) throws {
    let rep = NSBitmapImageRep(cgImage: image)
    try rep.representation(using: .png, properties: [:])!.write(to: url)
}

/// Every size on a light desktop above and a dark one below: the 1024 shown at 512, each other size at its true size, and 16 and 32
/// enlarged four times with hard pixels so their pixels can be judged.
func sheet(_ icons: [Int: CGImage]) -> CGImage {
    let w = 1640, rowH = 600, h = 2 * rowH
    let ctx = bitmap(w, h)
    NSGraphicsContext.current = NSGraphicsContext(cgContext: ctx, flipped: false)
    for (row, dark) in [false, true].enumerated() {
        let y0 = row * rowH
        ctx.setFillColor(cg(dark ? 0x1E1F22 : 0xECECEF)); ctx.fill(CGRect(x: 0, y: h - y0 - rowH, width: w, height: rowH))
        func put(_ px: Int, _ x: Int, _ yTop: Int, _ size: Int, crisp: Bool = false) {
            ctx.interpolationQuality = crisp ? .none : .high
            ctx.draw(icons[px]!, in: CGRect(x: x, y: h - yTop - size, width: size, height: size))
        }
        put(1024, 40, y0 + 44, 512)
        let cy = y0 + 300
        var x = 600
        for px in [256, 128, 64, 32, 16] { put(px, x, cy - px / 2, px); x += px + 24 }
        put(32, 1180, cy - 64, 128, crisp: true)
        put(16, 1340, cy - 32, 64, crisp: true)
        let label = NSAttributedString(string: dark ? "Dark desktop. Then 256, 128, 64, 32, 16; 32 and 16 at 4x" : "Light desktop. Then 256, 128, 64, 32, 16; 32 and 16 at 4x",
                                       attributes: [.font: NSFont.systemFont(ofSize: 14), .foregroundColor: dark ? NSColor(white: 0.6, alpha: 1) : NSColor(white: 0.4, alpha: 1)])
        label.draw(at: NSPoint(x: 40, y: Double(h - y0) - 30))
    }
    NSGraphicsContext.current = nil
    return ctx.makeImage()!
}

// MARK: Main

let args = CommandLine.arguments
guard args.count == 2 else { print("usage: swift app/icon/icons.swift <out-dir>"); exit(2) }
let out = URL(fileURLWithPath: args[1], isDirectory: true)
let fm = FileManager.default
do {
    let iconset = out.appendingPathComponent("AppIcon.iconset")
    let composer = out.appendingPathComponent("AppIcon.icon")
    for old in [iconset, composer] where fm.fileExists(atPath: old.path) { try fm.removeItem(at: old) }
    try fm.createDirectory(at: iconset, withIntermediateDirectories: true)
    var icons: [Int: CGImage] = [:]
    for px in [16, 32, 64, 128, 256, 512, 1024] { icons[px] = icon(px) }
    // iconutil's names: icon_<points>x<points>[@2x].png.
    for points in [16, 32, 128, 256, 512] {
        try writePNG(icons[points]!, iconset.appendingPathComponent("icon_\(points)x\(points).png"))
        try writePNG(icons[points * 2]!, iconset.appendingPathComponent("icon_\(points)x\(points)@2x.png"))
    }
    try writeIconComposer(large(), to: composer)
    let big = large()
    try svg(big.items, body: big.body, clip: true, shadows: true).write(to: out.appendingPathComponent("AppIcon.svg"), atomically: true, encoding: .utf8)
    try writePNG(sheet(icons), out.appendingPathComponent("sheet.png"))
} catch {
    FileHandle.standardError.write("icons: \(error)\n".data(using: .utf8)!)
    exit(1)
}
