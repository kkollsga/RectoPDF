//! What `recto-headings` does, apart from being a process: finds a document's headings with distillPDF, a page at a time.
//!
//! **Only the lazy engine,** `LazyStrict`, which refuses a file rather than fall back to the eager one, whose page content isn't capped (distillPDF 0.1.7's release review). A file it refuses gets no generated contents, and says so.
//!
//! **It only finds them.** Whether a heading can be seen, and not hidden text, is the session's to check (`recto_index::generated::visible`): it needs a render worker, which a helper walled off as the page drawer is can't start. What is found goes out a page a line ([`answer`]).

pub mod answer;
pub mod blocks;

use distillpdf::model::BlockKind;
use distillpdf::pages::{CancelToken, DocStats};
use distillpdf::{Engine, PdfDocument};
use recto_index::generated::visible::{self, Found, Headings};
use recto_index::generated::{MAX_ENTRIES, short_title};
use std::path::Path;

/// How much of the document distillPDF reads first. `Full` renders every page once more to learn which heading styles flood the document and which headings recur as running heads; `Typography` doesn't, and is faster.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scan {
    Full,
    Typography,
}

/// Why a document has no generated contents. Rust's words, as codes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Failure {
    /// It needs a password, so nothing of it is read or kept.
    NeedsPassword,
    /// distillPDF's bounded engine wouldn't open it.
    CouldntRead,
    /// It has no pages.
    NoPages,
    /// The file changed while it was being read.
    Changed,
}

impl Failure {
    pub fn code(self) -> &'static str {
        match self {
            Self::NeedsPassword => "needs_password",
            Self::CouldntRead => "couldnt_read",
            Self::NoPages => "no_pages",
            Self::Changed => "changed",
        }
    }
}

/// Opens a document with distillPDF's bounded engine: what every job of the helper starts with, by a path or by `/dev/fd/3` (the one descriptor a walled helper is given).
pub fn open(path: &Path) -> Result<PdfDocument, Failure> {
    let Some(text_path) = path.to_str() else { return Err(Failure::CouldntRead) };
    PdfDocument::open_with_engine(text_path, Engine::LazyStrict).map_err(|error| match error {
        distillpdf::Error::Encrypted => Failure::NeedsPassword,
        _ => Failure::CouldntRead,
    })
}

/// Reads a document's headings, a page at a time. `progress` hears each page as `(done, total)`. Nothing here starts a thread or holds more than a few pages.
pub fn find_headings(path: &Path, scan: Scan, cancel: Option<&CancelToken>, progress: &mut dyn FnMut(u32, u32)) -> Result<Headings, Failure> {
    let document = open(path)?;
    let mut on_page = |_: distillpdf::pages::PageProgress| {};
    let stats: DocStats = match scan {
        Scan::Full => document.scan_document(cancel, &mut on_page),
        Scan::Typography => document.scan_typography(cancel, &mut on_page),
    }
    .map_err(failure_of)?;
    let mut stream = document.page_stream(stats).map_err(failure_of)?;
    if let Some(cancel) = cancel {
        stream = stream.with_cancel(cancel.clone());
    }
    let total = stream.page_count();
    if total == 0 {
        return Err(Failure::NoPages);
    }
    let mut headings = Headings {
        pages: total,
        ..Headings::default()
    };
    for number in 1..=total {
        match stream.page(number) {
            Ok(page) => {
                for block in page.blocks.iter().filter(|block| block.kind == BlockKind::Heading) {
                    let title = short_title(&plain(&block.text));
                    if !title.chars().any(char::is_alphanumeric) {
                        continue;
                    }
                    // Repeated on one page, it's the same heading drawn twice, as a shadow or a bold done by overprinting.
                    if headings.found.last().is_some_and(|last| last.page == number - 1 && last.title == title) {
                        continue;
                    }
                    // Held to far more than are kept, so running heads can go before the list is cut, and memory stays bounded.
                    if headings.found.len() >= MAX_FOUND {
                        headings.truncated = true;
                        break;
                    }
                    let bbox = block.bbox.filter(|bbox| bbox.iter().all(|edge| edge.is_finite()));
                    headings.found.push(Found {
                        level: block.heading_level.unwrap_or(1).clamp(1, 6),
                        page: number - 1,
                        bbox,
                        title,
                    });
                }
            }
            Err(distillpdf::Error::StaleStats(_)) => return Err(Failure::Changed),
            Err(distillpdf::Error::Cancelled) => return Err(Failure::Changed),
            Err(_) => headings.pages_failed += 1,
        }
        progress(number, total);
    }
    visible::drop_running_heads(&mut headings);
    if headings.found.len() > MAX_ENTRIES {
        headings.found.truncate(MAX_ENTRIES);
        headings.truncated = true;
    }
    Ok(headings)
}

/// Most headings held while a document's read, before running heads go and the list is cut to [`MAX_ENTRIES`].
const MAX_FOUND: usize = 50_000;

/// A heading's text as words. distillPDF's `text` keeps inline markup, `<b>`, `<i>`, `<sup>` and the like, with `&`, `<` and `>` written as entities, so a tag is never the file's own text.
pub fn plain(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut rest = html;
    while let Some(at) = rest.find(['<', '&']) {
        out.push_str(&rest[..at]);
        rest = &rest[at..];
        if rest.starts_with('<') {
            match rest.find('>') {
                Some(end) => {
                    // A line break inside a heading is a space between its words.
                    if rest[..end].starts_with("<br") {
                        out.push(' ');
                    }
                    rest = &rest[end + 1..];
                }
                None => rest = "",
            }
            continue;
        }
        let entity = rest.find(';').filter(|end| *end <= 10).map(|end| &rest[1..end]);
        let decoded = entity.and_then(|name| match name {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            "nbsp" => Some(' '),
            _ => name
                .strip_prefix("#x")
                .or_else(|| name.strip_prefix("#X"))
                .map_or_else(
                    || name.strip_prefix('#').and_then(|digits| digits.parse().ok()),
                    |hex| u32::from_str_radix(hex, 16).ok(),
                )
                .and_then(char::from_u32),
        });
        match (decoded, entity) {
            (Some(c), Some(name)) => {
                out.push(c);
                rest = &rest[name.len() + 2..];
            }
            _ => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

fn failure_of(error: distillpdf::Error) -> Failure {
    match error {
        distillpdf::Error::StaleStats(_) => Failure::Changed,
        distillpdf::Error::Encrypted => Failure::NeedsPassword,
        _ => Failure::CouldntRead,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Failure: titles show distillPDF's markup, `<b>1. Introduction</b>`, or an entity as `&amp;`.
    #[test]
    fn titles_are_words_without_markup() {
        assert_eq!(plain("<b>1. Introduction</b>"), "1. Introduction");
        assert_eq!(
            plain("R&amp;D &lt;b&gt; x<sup>2</sup> &#233;t&#xE9; &bogus; a&b<br/>c"),
            "R&D <b> x2 été &bogus; a&b c"
        );
        assert_eq!(plain("<unclosed"), "");
        assert_eq!(plain("&#1114112;&#xD800;"), "&#1114112;&#xD800;");
    }
}
