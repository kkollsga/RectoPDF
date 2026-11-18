import AppKit

/// The host's bounded state for the single print Rust permits. No document is retained by the diagnostic record.
@MainActor final class PrintHost {
    var infos: [UInt64: NSPrintInfo] = [:]
    var nextSerial: UInt64 = 0
    var path: String?
    var lastLoad: [String: Any]?

    var report: [String: Any] { ["answers_kept": infos.count, "last_load": lastLoad ?? NSNull()] }
}

/// Core Graphics alone reads this file off the UI thread. After loading, the document transfers to the main thread's print view and is drawn only by its print operation.
private struct LoadedPrintDocument: @unchecked Sendable {
    let document: CGPDFDocument
    let pages: Int
    let mainThread: Bool
    let micros: UInt64

    static func open(_ path: String) -> LoadedPrintDocument? {
        let began = DispatchTime.now().uptimeNanoseconds
        guard let document = CGPDFDocument(URL(fileURLWithPath: path) as CFURL) else { return nil }
        let pages = document.numberOfPages
        guard pages > 0 else { return nil }
        return LoadedPrintDocument(document: document, pages: pages, mainThread: Thread.isMainThread, micros: (DispatchTime.now().uptimeNanoseconds - began) / 1000)
    }

    var report: [String: Any] { ["main_thread": mainThread, "micros": micros, "pages": pages] }
}

/// What the system's printing draws: the pages of the file Rust prepared, each fitted to the paper. Core Graphics reads that file, which is pictures Rust drew, so nothing here decides anything with opening and page counting prepared off the UI thread. The print operation is allowed a drawing thread; quiet QA has verified that path for text and map pages.
final class PrintPagesView: NSView {
    private let document: CGPDFDocument
    private let pageSize: NSSize
    private let pageCount: Int

    fileprivate init?(loaded: LoadedPrintDocument, paper: NSSize) {
        let height = paper.height * CGFloat(loaded.pages)
        guard paper.width.isFinite, paper.width > 0, paper.height.isFinite, paper.height > 0, height.isFinite else { return nil }
        document = loaded.document
        pageCount = loaded.pages
        pageSize = paper
        super.init(frame: NSRect(x: 0, y: 0, width: paper.width, height: height))
    }

    required init?(coder: NSCoder) { nil }

    override var isFlipped: Bool { true }

    override func knowsPageRange(_ range: NSRangePointer) -> Bool {
        range.pointee = NSRange(location: 1, length: pageCount)
        return true
    }

    override func rectForPage(_ page: Int) -> NSRect {
        NSRect(x: 0, y: CGFloat(page - 1) * pageSize.height, width: pageSize.width, height: pageSize.height)
    }

    override func draw(_ dirtyRect: NSRect) {
        guard let context = NSGraphicsContext.current?.cgContext else { return }
        let first = max(Int(dirtyRect.minY / pageSize.height), 0)
        let last = min(Int((dirtyRect.maxY - 0.5) / pageSize.height), pageCount - 1)
        guard first <= last else { return }
        for index in first...last {
            guard let page = document.page(at: index + 1) else { continue }
            let box = rectForPage(index + 1)
            context.saveGState()
            // The view is flipped, and a PDF page isn't: back up the right way within this page's rectangle, then fitted, turn and all, as the page says.
            context.translateBy(x: box.minX, y: box.maxY)
            context.scaleBy(x: 1, y: -1)
            let target = CGRect(origin: .zero, size: box.size)
            context.concatenate(page.getDrawingTransform(.mediaBox, rect: target, rotate: 0, preserveAspectRatio: true))
            context.drawPDFPage(page)
            context.restoreGState()
        }
    }
}

extension AppDelegate {
    /// File > Print: the system's panel, with Rust's answers about what there is to print. Its answer goes to Rust, which prepares the pages off this thread.
    func showPrintPanel() {
        guard let window = windowController?.window, !sheets.isUp(over: window) else { return }
        let model = core.printModel()
        guard model.tab != 0, model.pages > 0 else { return }
        let info = (NSPrintInfo.shared.copy() as? NSPrintInfo) ?? NSPrintInfo.shared
        info.jobDisposition = .spool
        let panel = NSPrintPanel()
        var options: NSPrintPanel.Options = [.showsCopies, .showsPageRange, .showsPaperSize, .showsOrientation, .showsPreview]
        if model.hasSelection { options.insert(.showsPrintSelection) }
        panel.options = options
        panel.jobStyleHint = .allPresets
        let core = self.core
        core.sheetChanged(open: true)
        sheetShowing = ["kind": "print_panel", "pages": model.pages]
        sheets.present(panel, info: info, over: window) { [weak self] response in
            core.sheetChanged(open: false)
            self?.sheetShowing = nil
            guard response == .printed else { return }
            self?.startPrint(model: model, info: info)
        }
    }

    /// The pages the panel chose, to Rust as the person's action under a number of the host's, and the print info kept by that number for when they're ready.
    func startPrint(model: PrintModel, info: NSPrintInfo) {
        let settings = info.dictionary()
        let all = (settings[NSPrintInfo.AttributeKey.allPages] as? Bool) ?? true
        let first = (settings[NSPrintInfo.AttributeKey.firstPage] as? Int) ?? 1
        let last = (settings[NSPrintInfo.AttributeKey.lastPage] as? Int) ?? Int(model.pages)
        // Saving or previewing a PDF from the panel is a print to a file, which Rust treats as an export; on paper it never asks.
        let toFile = info.jobDisposition != .spool
        printHost.nextSerial += 1
        let serial = printHost.nextSerial
        let acted = core.act(action: .print(tab: model.tab, all: all, first: UInt32(max(first, 1)), last: UInt32(max(last, 1)), selectionOnly: info.isSelectionOnly, toFile: toFile, serial: serial))
        if acted.understood { printHost.infos = [serial: info] }
    }

    /// Rust says a print's pages are ready: they go to the system's printing, with no panel, as it was answered already. It runs on a thread of its own, and the file goes when it's done.
    func runReadyPrint() {
        guard let ready = core.takePrint() else { return }
        let path = ready.path
        // Its own answer's printer and way, or nothing: never another answer's, nor a default printer it wasn't sent to.
        guard let info = printHost.infos.removeValue(forKey: ready.serial) else {
            core.printFinished(path: path)
            return
        }
        // Answers before it whose prints went nowhere (refused, or stopped) won't come back.
        printHost.infos = printHost.infos.filter { $0.key > ready.serial }
        // The range was taken already: the file is just those pages.
        info.dictionary()[NSPrintInfo.AttributeKey.allPages] = true
        info.isSelectionOnly = false
        let paper = NSSize(width: info.paperSize.width - info.leftMargin - info.rightMargin, height: info.paperSize.height - info.topMargin - info.bottomMargin)
        guard let window = windowController?.window else {
            core.printFinished(path: path)
            return
        }
        let title = core.printModel().jobTitle
        let core = self.core
        // Rust keeps this unnamed file in `handing` until exactly one completion below. Its one-print rule bounds this work to one load, without idle state or a private thread.
        DispatchQueue.global(qos: .userInitiated).async { [weak self, weak window] in
            let loaded = LoadedPrintDocument.open(path)
            DispatchQueue.main.async { [weak self, weak window] in
                // A closed or replaced window must not inherit the old window's pending print.
                guard let self, let window, self.windowController?.window === window else {
                    core.printFinished(path: path)
                    return
                }
                self.printHost.lastLoad = loaded?.report
                guard let loaded, let view = PrintPagesView(loaded: loaded, paper: paper) else {
                    core.printFinished(path: path)
                    return
                }
                let operation = NSPrintOperation(view: view, printInfo: info)
                operation.showsPrintPanel = false
                operation.showsProgressPanel = !self.isQuiet
                operation.canSpawnSeparateThread = true
                operation.jobTitle = title
                self.printHost.path = path
                operation.runModal(for: window, delegate: self, didRun: #selector(self.printDidRun(_:success:contextInfo:)), contextInfo: nil)
            }
        }
    }

    @objc func printDidRun(_ operation: NSPrintOperation, success: Bool, contextInfo: UnsafeMutableRawPointer?) {
        if let path = printHost.path { core.printFinished(path: path) }
        printHost.path = nil
    }

    /// For QA: the same path a print takes, to a PDF file through the system's own printing, with no panel shown. The paper is the first page's size with no margins, so the result can be laid page for page against the source. The system writes to a new name of Rust's, which Rust puts at `out` when it's done, never over anything.
    func printToPDFForQA(out: String, first: UInt32, last: UInt32) -> Bool {
        let model = core.printModel()
        guard model.tab != 0, let target = core.printQaTarget(out: out) else { return false }
        let info = (NSPrintInfo.shared.copy() as? NSPrintInfo) ?? NSPrintInfo.shared
        info.jobDisposition = .save
        info.dictionary()[NSPrintInfo.AttributeKey.jobSavingURL] = URL(fileURLWithPath: target)
        info.paperSize = NSSize(width: model.widthPt, height: model.heightPt)
        info.leftMargin = 0
        info.rightMargin = 0
        info.topMargin = 0
        info.bottomMargin = 0
        let all = first == 0
        printHost.nextSerial += 1
        let serial = printHost.nextSerial
        let started = core.act(action: .print(tab: model.tab, all: all, first: max(first, 1), last: max(last, first), selectionOnly: false, toFile: true, serial: serial)).understood
        if started { printHost.infos = [serial: info] }
        return started
    }
}
