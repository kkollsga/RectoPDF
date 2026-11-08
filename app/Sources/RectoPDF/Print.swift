import AppKit

/// What the system's printing draws: the pages of the file Rust prepared, each fitted to the paper. Core Graphics reads that file, which is pictures Rust drew, so nothing here decides anything and nothing is drawn on the UI thread: the print runs on a thread of its own.
final class PrintPagesView: NSView {
    private let document: CGPDFDocument
    private let pageSize: NSSize

    init?(url: URL, paper: NSSize) {
        guard let document = CGPDFDocument(url as CFURL), document.numberOfPages > 0 else { return nil }
        self.document = document
        pageSize = paper
        super.init(frame: NSRect(x: 0, y: 0, width: paper.width, height: paper.height * CGFloat(document.numberOfPages)))
    }

    required init?(coder: NSCoder) { nil }

    override var isFlipped: Bool { true }

    override func knowsPageRange(_ range: NSRangePointer) -> Bool {
        range.pointee = NSRange(location: 1, length: document.numberOfPages)
        return true
    }

    override func rectForPage(_ page: Int) -> NSRect {
        NSRect(x: 0, y: CGFloat(page - 1) * pageSize.height, width: pageSize.width, height: pageSize.height)
    }

    override func draw(_ dirtyRect: NSRect) {
        guard let context = NSGraphicsContext.current?.cgContext else { return }
        let first = max(Int(dirtyRect.minY / pageSize.height), 0)
        let last = min(Int((dirtyRect.maxY - 0.5) / pageSize.height), document.numberOfPages - 1)
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
        guard let window = windowController?.window, window.attachedSheet == nil else { return }
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
        panel.beginSheet(using: info, on: window) { [weak self] response in
            core.sheetChanged(open: false)
            self?.sheetShowing = nil
            guard response == .printed else { return }
            self?.startPrint(model: model, info: info)
        }
    }

    /// The pages the panel chose, to Rust, and the print info kept for when they're ready.
    func startPrint(model: PrintModel, info: NSPrintInfo) {
        let settings = info.dictionary()
        let all = (settings[NSPrintInfo.AttributeKey.allPages] as? Bool) ?? true
        let first = (settings[NSPrintInfo.AttributeKey.firstPage] as? Int) ?? 1
        let last = (settings[NSPrintInfo.AttributeKey.lastPage] as? Int) ?? Int(model.pages)
        pendingPrintInfo = info
        // Saving or previewing a PDF from the panel is a print to a file, which Rust treats as an export; on paper it never asks.
        let toFile = info.jobDisposition != .spool
        if !core.printChosen(tab: model.tab, all: all, first: UInt32(max(first, 1)), last: UInt32(max(last, 1)), selectionOnly: info.isSelectionOnly, toFile: toFile) {
            pendingPrintInfo = nil
        }
    }

    /// Rust says a print's pages are ready: they go to the system's printing, with no panel, as it was answered already. It runs on a thread of its own, and the file goes when it's done.
    func runReadyPrint() {
        guard let path = core.takePrint() else { return }
        let info = pendingPrintInfo ?? ((NSPrintInfo.shared.copy() as? NSPrintInfo) ?? NSPrintInfo.shared)
        pendingPrintInfo = nil
        // The range was taken already: the file is just those pages.
        info.dictionary()[NSPrintInfo.AttributeKey.allPages] = true
        info.isSelectionOnly = false
        let paper = NSSize(width: info.paperSize.width - info.leftMargin - info.rightMargin, height: info.paperSize.height - info.topMargin - info.bottomMargin)
        guard let window = windowController?.window, let view = PrintPagesView(url: URL(fileURLWithPath: path), paper: paper) else {
            core.printFinished(path: path)
            return
        }
        let operation = NSPrintOperation(view: view, printInfo: info)
        operation.showsPrintPanel = false
        operation.showsProgressPanel = !isQuiet
        operation.canSpawnSeparateThread = true
        operation.jobTitle = core.printModel().jobTitle
        printingPath = path
        operation.runModal(for: window, delegate: self, didRun: #selector(printDidRun(_:success:contextInfo:)), contextInfo: nil)
    }

    @objc func printDidRun(_ operation: NSPrintOperation, success: Bool, contextInfo: UnsafeMutableRawPointer?) {
        if let path = printingPath { core.printFinished(path: path) }
        printingPath = nil
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
        pendingPrintInfo = info
        let all = first == 0
        let started = core.printChosen(tab: model.tab, all: all, first: max(first, 1), last: max(last, first), selectionOnly: false, toFile: true)
        if !started { pendingPrintInfo = nil }
        return started
    }
}
