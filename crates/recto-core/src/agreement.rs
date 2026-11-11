//! **The agreement gate** (SPEC section 3): "when the two parsers disagree about a file, it stays readable but saving and signing turn off."
//!
//! PDFium draws what the person sees, and lopdf's writer is the only thing that changes the file (AGENTS rule 5). A damaged file can be read two
//! ways, and where the two readers read it differently, a save written from lopdf's reading could drop or swap pages the person saw. So before a save,
//! what both make of the file is compared, and any difference turns Save off. This is a reduction of the risk, not a proof: PDFium offers no way to
//! look an object up by number, so the comparison is of what both can be asked cheaply (`docs/research/2026-11-06-rendering-interactive.md` §6.3):
//!
//! - **Page count.** PDFium takes the root's `/Count` where lopdf walks the kids, and keeps a slot for a kid that doesn't load where lopdf leaves it out.
//! - **Each page's shown size, in order,** after its own turn, which stands for which page is where: a tree the two walk differently (a `/Type /Page`
//!   with `/Kids`, which lopdf takes as a page and PDFium as a branch), a page inheriting by its `/Parent` rather than by the tree, or a page whose
//!   object each reads from another place in the file, shows up as a page of another size.
//! - **Whether either rebuilt the file's cross-reference data by scanning,** as a rebuilt index finds objects where the file's own index doesn't say they
//!   are. PDFium rebuilds when the first object its table lists isn't where it says; lopdf, which reads lazily, never looks there.
//!
//! The renderer's half is asked of its worker, which this crate knows nothing about (`recto_agent::agreement`). This is the structure's half, and the rule.

use crate::{OpenError, checked_reader};
use lopdf::IndexedReader;

/// The most pages whose sizes are compared. Past it, that many spread evenly through the document, its first and last among them, so a check of a
/// document of a million pages costs what one of ten thousand does.
pub const MAX_COMPARED_PAGES: u32 = 10_000;

/// How far two sizes of a page may differ, in points, and still be the same page: one reads the numbers as `f32`, the other as `f64`.
const SIZE_TOLERANCE: f32 = 0.5;

/// Which pages' sizes are compared in a document of `page_count`: every one up to [`MAX_COMPARED_PAGES`], and past that that many, spread evenly,
/// the first and the last among them. Ascending, with no repeats.
pub fn compared_pages(page_count: u32) -> Vec<u32> {
    if page_count <= MAX_COMPARED_PAGES {
        return (0..page_count).collect();
    }
    let last = u64::from(page_count - 1);
    let steps = u64::from(MAX_COMPARED_PAGES - 1);
    (0..MAX_COMPARED_PAGES)
        .map(|i| u32::try_from(u64::from(i) * last / steps).unwrap_or(page_count - 1))
        .collect()
}

/// What one reader makes of a file, as far as the gate asks.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct View {
    pub page_count: u32,
    /// This reader couldn't use the file's own cross-reference data, and rebuilt it by scanning the file.
    pub repaired: bool,
    /// The shown size of each page in [`compared_pages`] of the page count, in points, after the page's own turn. `None` for a page this reader
    /// couldn't size, which isn't compared.
    pub sizes: Vec<Option<(f32, f32)>>,
}

/// **How the two readers differ about a file,** the first difference found. Every word said about it is here.
#[derive(Clone, Debug, PartialEq)]
pub enum Disagreement {
    /// They count a different number of pages.
    PageCount { structure: u32, renderer: u32 },
    /// A page, from 0, has another size in each: they take another page to be there.
    PageSize { page: u32 },
    /// One of them had to rebuild the file's index of where things are, and the other didn't.
    Repaired { by_renderer: bool },
    /// The renderer couldn't read the file at all, so what's shown can't be checked against what would be saved.
    RendererFailed,
    /// One of them needs a password for the file and the other read it without one, as when only an older revision's trailer names `/Encrypt`.
    Encryption,
    /// It couldn't be checked just now: the file wasn't the version asked about, a read failed, a worker couldn't start, or the look ran out of
    /// time. Save is off as for any disagreement, but it's tried again at the next save rather than taken as the file's answer.
    Unchecked,
}

impl Disagreement {
    /// For agents, and for what tests check.
    pub fn code(&self) -> &'static str {
        match self {
            Self::PageCount { .. } => "page_count",
            Self::PageSize { .. } => "page_size",
            Self::Repaired { .. } => "repaired",
            Self::RendererFailed => "renderer_failed",
            Self::Encryption => "encryption",
            Self::Unchecked => "unchecked",
        }
    }

    /// Whether it's about this moment rather than the file, so it's looked at again.
    pub fn is_transient(&self) -> bool {
        matches!(self, Self::Unchecked)
    }

    /// What differs, in a person's words: the middle of [`Self::message`].
    pub fn detail(&self) -> String {
        match self {
            Self::PageCount { structure, renderer } => {
                format!("the pages shown number {renderer}, but its structure has {structure}")
            }
            Self::PageSize { page } => format!("page {} as shown isn't the page its structure has there", page + 1),
            Self::Repaired { by_renderer: true } => "its index of where things are is broken, and the pages shown come from a repaired copy of it".into(),
            Self::Repaired { by_renderer: false } => "its index of where things are is broken, and its structure was read from a repaired copy of it".into(),
            Self::RendererFailed => "its pages couldn't be read to check them".into(),
            Self::Encryption => "one of them needs a password for it and the other doesn't".into(),
            Self::Unchecked => "it couldn't be read just now to check it".into(),
        }
    }

    /// **Why Save is off,** whole: what the Save icon's tooltip says, and what a refused save says.
    pub fn message(&self) -> String {
        if self.is_transient() {
            return "RectoPDF couldn't check this file just now against what's shown, so Save and Save As are off. It can still be read; try saving again in a moment.".into();
        }
        format!(
            "This file is damaged in a way RectoPDF's two readers see differently: {}. It can still be read, but Save and Save As are off, as what's written might not be what you see.",
            self.detail()
        )
    }
}

/// **The gate's rule:** the first difference between what the structure reader and the renderer make of one file, or `None` when they agree.
/// A page one of them can size and the other can't is a difference too: lopdf would write a page PDFium can't show, or the other way round. Only
/// a page neither can size isn't compared.
pub fn compare(structure: &View, renderer: &View) -> Option<Disagreement> {
    if structure.page_count != renderer.page_count {
        return Some(Disagreement::PageCount {
            structure: structure.page_count,
            renderer: renderer.page_count,
        });
    }
    if structure.repaired != renderer.repaired {
        return Some(Disagreement::Repaired {
            by_renderer: renderer.repaired,
        });
    }
    let same = |a: (f32, f32), b: (f32, f32)| (a.0 - b.0).abs() <= SIZE_TOLERANCE && (a.1 - b.1).abs() <= SIZE_TOLERANCE;
    compared_pages(structure.page_count)
        .into_iter()
        .zip(structure.sizes.iter().zip(&renderer.sizes))
        .find_map(|(page, sizes)| match sizes {
            (Some(mine), Some(theirs)) if !same(*mine, *theirs) => Some(Disagreement::PageSize { page }),
            (Some(_), None) | (None, Some(_)) => Some(Disagreement::PageSize { page }),
            _ => None,
        })
}

/// **The structure reader's half,** read through a file already open, so it's the very bytes the renderer was given. Bounded as every walk of the
/// page tree is, by the fork's caps on pages, depth and work, and each page's size by a few objects. A file that needs a password it isn't given is
/// [`OpenError::PasswordNeeded`].
pub fn structure_view(file: std::fs::File, password: Option<&[u8]>) -> Result<View, OpenError> {
    let source = lopdf::FileSource::from_file(file).map_err(|_| OpenError::NotAllowed)?;
    let options = lopdf::IndexedReaderOptions {
        password: password.map(<[u8]>::to_vec),
        ..lopdf::IndexedReaderOptions::default()
    };
    view_of(&checked_reader(IndexedReader::open_with_options(source, options))?)
}

/// [`structure_view`] on bytes in memory: the same walk, for tests and the fuzzer.
pub fn structure_view_bytes(bytes: std::sync::Arc<[u8]>) -> Result<View, OpenError> {
    view_of(&crate::open_bytes(bytes)?)
}

fn view_of(reader: &IndexedReader) -> Result<View, OpenError> {
    let (map, stats) = reader.page_map_with_stats().map_err(|_| OpenError::NotPdf)?;
    let page_count = u32::try_from(map.len()).map_err(|_| OpenError::NotPdf)?;
    let sizes = compared_pages(page_count)
        .into_iter()
        .map(|page| map.get(page as usize).and_then(|entry| crate::nav::frame_of(reader, entry).shown_size()))
        .collect();
    Ok(View {
        page_count,
        repaired: stats.recovered(),
        sizes,
    })
}

/// **Why the structure reader refused a file:** one of its limits, which says the file is built to cost far more than any real one, is
/// [`OpenError::TooComplex`](crate::OpenError::TooComplex); anything else is `NotPdf`. The renderer may still read either, as only the structure
/// reader saves, so a file refused here opens to be read, with Save off: the two readers already disagree about it.
pub(crate) fn refusal_of(error: &lopdf::IndexedReaderError) -> crate::OpenError {
    use lopdf::IndexedReaderError as E;
    match error {
        E::StructureLimitExceeded { .. }
        | E::EntryLimitExceeded { .. }
        | E::RevisionLimitExceeded { .. }
        | E::IndirectHeaderLimitExceeded { .. }
        | E::ObjectLimitExceeded { .. }
        | E::ScalarResourceLimit { .. }
        | E::StreamLimitExceeded { .. }
        | E::ResolutionDepthExceeded { .. }
        | E::PageCountLimitExceeded { .. }
        | E::PageTreeDepthLimitExceeded { .. } => crate::OpenError::TooComplex,
        _ => crate::OpenError::NotPdf,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn view(page_count: u32, sizes: &[(f32, f32)]) -> View {
        View {
            page_count,
            repaired: false,
            sizes: sizes.iter().copied().map(Some).collect(),
        }
    }

    /// Failure: a check of a huge document sizes every page, or one past the cap misses its last page, where a tree walked differently shows first.
    /// Every page up to the cap; past it, exactly the cap, spread evenly, with the first and last, ascending and with no repeats.
    #[test]
    fn the_pages_compared_are_bounded_and_reach_both_ends() {
        assert_eq!(compared_pages(0), Vec::<u32>::new());
        assert_eq!(compared_pages(3), [0, 1, 2]);
        assert_eq!(compared_pages(MAX_COMPARED_PAGES).len(), MAX_COMPARED_PAGES as usize);
        for count in [MAX_COMPARED_PAGES + 1, 123_457, 2_000_000] {
            let pages = compared_pages(count);
            assert_eq!(pages.len(), MAX_COMPARED_PAGES as usize, "{count}");
            assert_eq!((pages[0], *pages.last().unwrap()), (0, count - 1), "{count}");
            assert!(pages.windows(2).all(|pair| pair[0] < pair[1]), "{count}: ascending, no repeats");
        }
    }

    /// Failure: the gate lets through a file the two readers count, order or index differently, or a page one of them can't size (review round 1),
    /// or turns Save off over a rounding difference. The first difference is the one named.
    #[test]
    fn the_rule_names_the_first_difference() {
        let letter = (612.0, 792.0);
        let square = (300.0, 300.0);
        assert_eq!(compare(&view(2, &[letter, square]), &view(2, &[(612.2, 791.9), square])), None, "rounding");
        assert_eq!(
            compare(&view(2, &[letter, square]), &view(3, &[letter, square, square])),
            Some(Disagreement::PageCount { structure: 2, renderer: 3 })
        );
        assert_eq!(
            compare(&view(2, &[letter, square]), &view(2, &[square, letter])),
            Some(Disagreement::PageSize { page: 0 }),
            "another page in the first place"
        );
        let mut repaired = view(2, &[letter, square]);
        repaired.repaired = true;
        assert_eq!(
            compare(&view(2, &[letter, square]), &repaired),
            Some(Disagreement::Repaired { by_renderer: true })
        );
        assert_eq!(
            compare(&repaired, &view(2, &[letter, square])),
            Some(Disagreement::Repaired { by_renderer: false })
        );
        let mut one_unread = view(2, &[letter, square]);
        one_unread.sizes[1] = None;
        assert_eq!(
            compare(&view(2, &[letter, square]), &one_unread),
            Some(Disagreement::PageSize { page: 1 }),
            "a page lopdf maps and PDFium can't size"
        );
        assert_eq!(compare(&one_unread.clone(), &one_unread), None, "a page neither can size isn't compared");
    }

    /// Failure: Save is off with no reason a person can act on, or the reason names neither what differs nor that the file can still be read.
    #[test]
    fn every_disagreement_says_what_and_that_it_can_still_be_read() {
        for disagreement in [
            Disagreement::PageCount { structure: 2, renderer: 3 },
            Disagreement::PageSize { page: 4 },
            Disagreement::Repaired { by_renderer: true },
            Disagreement::Repaired { by_renderer: false },
            Disagreement::RendererFailed,
            Disagreement::Encryption,
            Disagreement::Unchecked,
        ] {
            let message = disagreement.message();
            assert!(disagreement.is_transient() || message.contains(&disagreement.detail()), "{message}");
            assert!(
                message.contains("can still be read") && message.contains("Save and Save As are off"),
                "{message}"
            );
        }
        assert!(
            Disagreement::PageSize { page: 4 }.detail().contains("page 5"),
            "pages are counted from 1 for people"
        );
    }

    /// Failure: the structure's half reads a page's size other than as shown, so every turned page disagrees with PDFium. A turned page's size is
    /// swapped, an inherited box counts, and the crop wins within the media box.
    #[test]
    fn the_structure_reads_sizes_as_shown() {
        let bytes = recto_testpdf::build(&[
            recto_testpdf::Object::Dict("<< /Type /Catalog /Pages 2 0 R >>".into()),
            recto_testpdf::Object::Dict("<< /Type /Pages /Kids [3 0 R 4 0 R 5 0 R] /Count 3 /MediaBox [0 0 400 600] >>".into()),
            recto_testpdf::Object::Dict("<< /Type /Page /Parent 2 0 R >>".into()),
            recto_testpdf::Object::Dict("<< /Type /Page /Parent 2 0 R /Rotate 90 >>".into()),
            recto_testpdf::Object::Dict("<< /Type /Page /Parent 2 0 R /CropBox [10 10 110 210] >>".into()),
        ]);
        let view = structure_view_bytes(bytes.into()).unwrap();
        assert_eq!(view.page_count, 3);
        assert!(!view.repaired);
        assert_eq!(view.sizes, [Some((400.0, 600.0)), Some((600.0, 400.0)), Some((100.0, 200.0))]);
    }
}
