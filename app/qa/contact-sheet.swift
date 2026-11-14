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
let rows = (shots.count + columns - 1) / columns
let width = gap + columns * (cellW + gap)
let height = header + rows * (cellH + label + gap) + gap

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
for (index, path) in shots.enumerated() {
    let col = index % columns, row = index / columns
    let x = gap + col * (cellW + gap)
    let top = header + row * (cellH + label + gap)
    var name = URL(fileURLWithPath: path).deletingPathExtension().lastPathComponent
    if let look = ["-light", "-dark"].first(where: name.hasSuffix) { name.removeLast(look.count) }
    text(name, x: x, y: top + 24, size: 20)
    let cell = CGRect(x: x, y: height - (top + label + cellH), width: cellW, height: cellH)
    ctx.setFillColor(CGColor(gray: 0.7, alpha: 1))
    ctx.fill(cell)
    let options = [kCGImageSourceCreateThumbnailFromImageAlways: true, kCGImageSourceThumbnailMaxPixelSize: cellW * 2] as CFDictionary
    guard let source = CGImageSourceCreateWithURL(URL(fileURLWithPath: path) as CFURL, nil),
          let image = CGImageSourceCreateThumbnailAtIndex(source, 0, options) else {
        text("(unreadable)", x: x + 12, y: top + label + 40, size: 18)
        continue
    }
    let scale = min(Double(cellW) / Double(image.width), Double(cellH) / Double(image.height))
    let w = Double(image.width) * scale, h = Double(image.height) * scale
    ctx.interpolationQuality = .high
    ctx.draw(image, in: CGRect(x: cell.midX - w / 2, y: cell.midY - h / 2, width: w, height: h))
}

guard let sheet = ctx.makeImage(),
      let dest = CGImageDestinationCreateWithURL(out as CFURL, UTType.png.identifier as CFString, 1, nil) else { exit(1) }
CGImageDestinationAddImage(dest, sheet, nil)
exit(CGImageDestinationFinalize(dest) ? 0 : 1)
