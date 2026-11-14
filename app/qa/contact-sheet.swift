// Lays shots out as one labelled contact sheet, offscreen: CoreGraphics, ImageIO and CoreText only, so nothing reaches a screen.
//
//   swift app/qa/contact-sheet.swift OUT.png TITLE SHOT.png...
//
// Each shot is read as a thumbnail (ImageIO decodes it at that size, never whole), labelled with its file name less `-light`/`-dark`.
import CoreGraphics
import CoreText
import Foundation
import ImageIO
import UniformTypeIdentifiers

let args = CommandLine.arguments
guard args.count >= 4 else {
    FileHandle.standardError.write("usage: contact-sheet.swift OUT.png TITLE SHOT.png...\n".data(using: .utf8)!)
    exit(2)
}
let out = URL(fileURLWithPath: args[1])
let title = args[2]
let shots = Array(args[3...])

let columns = 4, cellW = 640, cellH = 400, label = 34, gap = 16, header = 64
let rowW = columns * cellW + (columns - 1) * gap

/// A shot's size, read from its header without decoding it.
func size(_ path: String) -> (Int, Int) {
    guard let source = CGImageSourceCreateWithURL(URL(fileURLWithPath: path) as CFURL, nil),
          let props = CGImageSourceCopyPropertiesAtIndex(source, 0, nil) as? [CFString: Any],
          let w = props[kCGImagePropertyPixelWidth] as? Int, let h = props[kCGImagePropertyPixelHeight] as? Int else { return (cellW, cellH) }
    return (w, h)
}

/// Where each shot goes: a window's shot in a cell of the grid; a wide one (the menus, Settings) on a row of its own, as wide as the grid, so its words
/// can be read.
struct Place { let path: String; let x: Int; let top: Int; let w: Int; let h: Int }
var places: [Place] = []
var col = 0, top = header
for path in shots {
    let (w, h) = size(path)
    if Double(w) / Double(h) > 2 {
        if col > 0 { top += cellH + label + gap; col = 0 }
        // As wide as the grid, but never drawn larger than it was shot.
        let wide = min(rowW, w)
        let tall = min(Int(Double(wide) * Double(h) / Double(w)), 2 * cellH)
        places.append(Place(path: path, x: gap, top: top, w: wide, h: tall))
        top += tall + label + gap
    } else {
        places.append(Place(path: path, x: gap + col * (cellW + gap), top: top, w: cellW, h: cellH))
        col += 1
        if col == columns { col = 0; top += cellH + label + gap }
    }
}
let width = gap + rowW + gap
let height = (col > 0 ? top + cellH + label + gap : top) + gap

let space = CGColorSpace(name: CGColorSpace.sRGB)!
guard let ctx = CGContext(data: nil, width: width, height: height, bitsPerComponent: 8, bytesPerRow: 0, space: space,
                          bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue) else { exit(1) }
ctx.setFillColor(CGColor(gray: 0.86, alpha: 1))
ctx.fill(CGRect(x: 0, y: 0, width: width, height: height))

/// Text with its baseline at `y` from the top.
func text(_ string: String, x: Int, y: Int, size: CGFloat, bold: Bool = false) {
    let font = CTFontCreateWithName((bold ? "Helvetica-Bold" : "Helvetica") as CFString, size, nil)
    let attributed = NSAttributedString(string: string, attributes: [
        NSAttributedString.Key(kCTFontAttributeName as String): font,
        NSAttributedString.Key(kCTForegroundColorAttributeName as String): CGColor(gray: 0.1, alpha: 1),
    ])
    ctx.textPosition = CGPoint(x: x, y: height - y)
    CTLineDraw(CTLineCreateWithAttributedString(attributed), ctx)
}

text("\(title): \(shots.count) shots", x: gap, y: 42, size: 28, bold: true)
for place in places {
    var name = URL(fileURLWithPath: place.path).deletingPathExtension().lastPathComponent
    if let look = ["-light", "-dark"].first(where: name.hasSuffix) { name.removeLast(look.count) }
    text(name, x: place.x, y: place.top + 24, size: 20)
    let cell = CGRect(x: place.x, y: height - (place.top + label + place.h), width: place.w, height: place.h)
    ctx.setFillColor(CGColor(gray: 0.7, alpha: 1))
    ctx.fill(cell)
    let options = [kCGImageSourceCreateThumbnailFromImageAlways: true, kCGImageSourceThumbnailMaxPixelSize: max(place.w, place.h) * 2] as CFDictionary
    guard let source = CGImageSourceCreateWithURL(URL(fileURLWithPath: place.path) as CFURL, nil),
          let image = CGImageSourceCreateThumbnailAtIndex(source, 0, options) else {
        text("(unreadable)", x: place.x + 12, y: place.top + label + 40, size: 18)
        continue
    }
    let scale = min(Double(place.w) / Double(image.width), Double(place.h) / Double(image.height))
    let w = Double(image.width) * scale, h = Double(image.height) * scale
    ctx.interpolationQuality = .high
    ctx.draw(image, in: CGRect(x: cell.midX - w / 2, y: cell.midY - h / 2, width: w, height: h))
}

guard let sheet = ctx.makeImage(),
      let dest = CGImageDestinationCreateWithURL(out as CFURL, UTType.png.identifier as CFString, 1, nil) else { exit(1) }
CGImageDestinationAddImage(dest, sheet, nil)
exit(CGImageDestinationFinalize(dest) ? 0 : 1)
