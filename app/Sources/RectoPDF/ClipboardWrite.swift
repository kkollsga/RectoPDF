import AppKit
import QuartzCore
import recto_ffiFFI

/// What a copy puts on the clipboard now, a type at a time from Rust, timed, so `qa copied` can say what it kept the UI thread for: bringing the bytes over, and handing them to the clipboard. What's promised isn't made here.
enum ClipboardWrite {
    static func timed(_ offer: ClipboardOffer, core: AppCore, to pasteboard: NSPasteboard) -> [String: Any] {
        let began = CACurrentMediaTime()
        var (bytes, fetching, setting) = (0, 0.0, 0.0)
        for type in offer.types where !offer.promised.contains(type) {
            let fetched = CACurrentMediaTime()
            let data = core.clipboardData(serial: offer.serial, uti: type)
            let set = CACurrentMediaTime()
            fetching += set - fetched
            if let data {
                bytes += data.count
                pasteboard.setData(data, forType: NSPasteboard.PasteboardType(type))
            }
            setting += CACurrentMediaTime() - set
        }
        return ["total_ms": (CACurrentMediaTime() - began) * 1000, "fetch_ms": fetching * 1000, "set_ms": setting * 1000, "bytes": bytes]
    }
}
