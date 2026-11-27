//! The structure helper's `blocks` job: what distillPDF finds on each page, as a line a page and one end line, for the session to place in the page's text (`docs/engineering/blocks.md`).
//!
//! **It only finds them.** A block goes out with its kind, level, box, its text (markup stripped as titles are) and whether it goes on onto a later page. Whether the text is the page's own and where it sits is the session's to say (`recto_index::blocks::map`): this
//! process never sees the page's characters. Tables and figures are said by kind and box and never by their cells; the page-chrome carriers (`header`, `dest_anchors`) aren't said at all.
//!
//! **A page never ends the job.** One distillPDF can't read is a line with no blocks and a count in the end line. A page's line is cut to the session's cap and says how many blocks it left out, and nothing here holds more than one page.

use crate::{Failure, Scan, failure_of};
use distillpdf::model::{Block, BlockKind};
use distillpdf::pages::{BlockContinuation, DocStats};
use recto_index::blocks::map::{MAX_LINE_BLOCKS, MAX_LINE_BYTES};
use serde_json::Value;
use std::io::Write;
use std::path::Path;
use std::time::Instant;

/// The name the session reads a block's kind by. `None` for the carriers of the page's own markup, which are never sent. No wildcard: a kind distillPDF adds is decided here.
pub fn kind_name(kind: BlockKind) -> Option<&'static str> {
    match kind {
        BlockKind::Para => Some("para"),
        BlockKind::Heading => Some("heading"),
        BlockKind::ListItem => Some("list_item"),
        BlockKind::Caption => Some("caption"),
        BlockKind::Code => Some("code"),
        BlockKind::Footnote => Some("footnote"),
        BlockKind::Table => Some("table"),
        BlockKind::Figure => Some("figure"),
        BlockKind::Header | BlockKind::DestAnchors => None,
    }
}

/// The deepest level said.
const MAX_LEVEL: u8 = 9;

/// One block as it goes out.
#[derive(Clone, Debug, PartialEq)]
pub struct Out {
    pub kind: &'static str,
    pub level: u8,
    /// Left, bottom, right, top, only when all four are finite.
    pub bbox: Option<[f32; 4]>,
    pub text: String,
    pub continues: bool,
}

/// Block `index` of a page, ready to go out. `None` for a kind that is never sent. `continuations` are the page's own.
pub fn out_of(block: &Block, index: usize, continuations: &[BlockContinuation]) -> Option<Out> {
    let kind = kind_name(block.kind)?;
    let level = if block.kind == BlockKind::Heading {
        block.heading_level.unwrap_or(0).min(MAX_LEVEL)
    } else {
        0
    };
    // A table's cells and a figure's markup are not text of the page, and not what is looked for in it.
    let text = if matches!(block.kind, BlockKind::Table | BlockKind::Figure) {
        String::new()
    } else {
        crate::plain(&block.text)
    };
    Some(Out {
        kind,
        level,
        bbox: block.bbox.filter(|bbox| bbox.iter().all(|side| side.is_finite())),
        text,
        continues: continuations.iter().any(|c| c.block == index && c.page > block.page),
    })
}

impl Out {
    fn json(&self) -> String {
        let bbox = self.bbox.map_or_else(|| "null".to_string(), |[l, b, r, t]| format!("[{l},{b},{r},{t}]"));
        let text = serde_json::to_string(&self.text).unwrap_or_else(|_| "\"\"".into());
        format!(
            "{{\"kind\":\"{}\",\"level\":{},\"box\":{bbox},\"text\":{text},\"continues\":{}}}",
            self.kind, self.level, self.continues
        )
    }
}

/// One page's line: `{"page":N,"blocks":[{"kind","level","box":[l,b,r,t]|null,"text","continues"}]}`, `page` zero-based. Returns the line, how many blocks it holds and how many were left out.
///
/// A block whose own line is over half the cap is left out and the later ones still go. Otherwise blocks go in order while the whole line, its closing `]}` included, stays within
/// `MAX_LINE_BYTES` and `MAX_LINE_BLOCKS`; the first that doesn't fit and everything after it are left out.
pub fn page_line(page: u32, outs: &[Out]) -> (String, usize, usize) {
    let mut line = format!("{{\"page\":{page},\"blocks\":[");
    let (mut sent, mut cut, mut full) = (0, 0, false);
    for out in outs {
        let one = out.json();
        if one.len() > MAX_LINE_BYTES / 2 {
            cut += 1;
            continue;
        }
        // The comma before it, if it isn't the first, and the closing `]}` still to come.
        if full || sent >= MAX_LINE_BLOCKS || line.len() + usize::from(sent > 0) + one.len() + 2 > MAX_LINE_BYTES {
            full = true;
            cut += 1;
            continue;
        }
        if sent > 0 {
            line.push(',');
        }
        line.push_str(&one);
        sent += 1;
    }
    line.push_str("]}");
    (line, sent, cut)
}

/// The last line: how the read ended. Numbers only. `blocks` is how many went out in page lines, which the session checks against what it received.
pub fn end_line(pages: u32, pages_failed: u32, blocks_sent: u64, cut: u64, distilled_ms: u64, peak: Option<u64>) -> String {
    serde_json::json!({
        "state": "ready",
        "pages": pages,
        "pages_failed": pages_failed,
        "blocks": blocks_sent,
        "cut": cut,
        "distilled_ms": distilled_ms,
        "helper_peak_bytes": peak,
    })
    .to_string()
}

/// How far distillPDF scans first. Typography unless the job says `"scan":"full"`: the blocks job doesn't need the running heads that the full scan learns, and typography is faster.
pub fn scan_of(job: &Value) -> Scan {
    if job["scan"] == "full" { Scan::Full } else { Scan::Typography }
}

fn peak() -> Option<u64> {
    recto_platform::isolate::process_memory(std::process::id()).map(|memory| memory.peak_footprint_bytes)
}

/// Reads the document at `path` and writes a line a page, then the end line, to `out`, flushing each. A page distillPDF can't read is an empty line and a count, never the end of the job. A write that fails means the
/// session has gone, and ends it quietly. Holds one page's output at a time.
///
/// # Errors
///
/// [`Failure`]: the document can't be opened, has no pages, or changed under the read.
pub fn run(path: &Path, scan: Scan, out: &mut impl Write, started: Instant) -> Result<(), Failure> {
    let document = crate::open(path)?;
    let mut on_page = |_: distillpdf::pages::PageProgress| {};
    let stats: DocStats = match scan {
        Scan::Full => document.scan_document(None, &mut on_page),
        Scan::Typography => document.scan_typography(None, &mut on_page),
    }
    .map_err(failure_of)?;
    let mut stream = document.page_stream(stats).map_err(failure_of)?;
    let total = stream.page_count();
    if total == 0 {
        return Err(Failure::NoPages);
    }
    let (mut failed, mut sent_all, mut cut_all) = (0u32, 0u64, 0u64);
    for number in 1..=total {
        let (line, sent, cut) = match stream.page(number) {
            Ok(page) => {
                let outs: Vec<Out> = page
                    .blocks
                    .iter()
                    .enumerate()
                    .filter_map(|(index, block)| out_of(block, index, &page.continuations))
                    .collect();
                page_line(number - 1, &outs)
            }
            Err(distillpdf::Error::StaleStats(_) | distillpdf::Error::Cancelled) => return Err(Failure::Changed),
            Err(_) => {
                failed += 1;
                page_line(number - 1, &[])
            }
        };
        sent_all += sent as u64;
        cut_all += cut as u64;
        if writeln!(out, "{line}").and_then(|()| out.flush()).is_err() {
            return Ok(());
        }
    }
    let end = end_line(total, failed, sent_all, cut_all, started.elapsed().as_millis() as u64, peak());
    let _ = writeln!(out, "{end}").and_then(|()| out.flush());
    Ok(())
}
