//! The save's planner, copier and rewrite on bytes in memory, for the fuzzer and the tests. Out of `save.rs` unchanged (R4.3), so the save had
//! room for highlights.

use super::*;

/// Opens bytes the way a save reads a file, for [`plan_in_memory`].
fn open_bytes_for_save(bytes: &[u8]) -> Result<(IndexedReader, lopdf::PageMap), SaveError> {
    let options = IndexedReaderOptions {
        object_bytes: SAVE_OBJECT_BYTES,
        ..IndexedReaderOptions::default()
    };
    let reader = IndexedReader::open_with_options(lopdf::BytesSource::new(std::sync::Arc::from(bytes)), options).map_err(refused_on_open)?;
    if reader.is_encrypted() {
        return Err(SaveError::Protected { password: false });
    }
    let map = reader.page_map().map_err(|_| SaveError::Damaged)?;
    Ok((reader, map))
}

/// **The planner, the copier and the remapper on bytes in memory, for the fuzzer:** `target` with its last page moved to the front, page 1 turned, and every page
/// of `added` copied in at the end, with a highlight on the first page of each file and on the second (whatever their `/Annots` say, R4.3), written as an
/// update into memory and read back. Gives back the pages the result has. The same code a save runs, with no file.
pub fn plan_in_memory(target: &[u8], added: &[u8]) -> Result<usize, SaveError> {
    plan_in_memory_within(target, added, MAX_COPY_WORK)
}

/// [`plan_in_memory`] with a smaller bound on the copy's work, so a test can show the bound on a small file.
pub fn plan_in_memory_within(target: &[u8], added: &[u8], work_limit: u64) -> Result<usize, SaveError> {
    let (reader, map) = open_bytes_for_save(target)?;
    reader.check_appendable().map_err(from_append)?;
    let pages = u32::try_from(map.len()).map_err(|_| SaveError::Damaged)?;
    let (source, source_map) = open_bytes_for_save(added)?;
    let added_pages = u32::try_from(source_map.len()).map_err(|_| SaveError::Damaged)?.min(64);
    let mut order: Vec<PageRef> = Vec::new();
    order.push(PageRef {
        source: 0,
        page: pages.saturating_sub(1),
        quarter_turns: 0,
    });
    order.extend((0..pages.saturating_sub(1)).map(|page| PageRef {
        source: 0,
        page,
        quarter_turns: u8::from(page == 0),
    }));
    order.extend((0..added_pages).map(|page| PageRef {
        source: 1,
        page,
        quarter_turns: 2,
    }));
    let added_file = [AddedFile {
        path: Path::new("added.pdf"),
        page_count: added_pages,
        stamp: None,
    }];
    let line = [[10.0, 10.0, 100.0, 12.0]];
    let highlights: Vec<NewHighlight<'_>> = [0, 1, order.len().saturating_sub(added_pages as usize)]
        .into_iter()
        .filter(|position| *position < order.len())
        .map(|position| NewHighlight {
            position: u32::try_from(position).unwrap_or(0),
            rects: &line,
        })
        .collect();
    let request = SaveRequest {
        path: Path::new("target.pdf"),
        stamp: None,
        file_pages: pages,
        pages: &order,
        added: &added_file,
        highlights: &highlights,
        highlights_gone: &[],
    };
    let sources = vec![Some((source, source_map))];
    let plan = Plan::make(
        &reader,
        &map,
        &request,
        &sources,
        signature_facts(&reader),
        Writer::Append,
        work_limit.min(MAX_COPY_WORK),
    )?;
    let mut out = Vec::new();
    let mut writer = AppendWriter::new(&reader, &mut out).map_err(from_append)?;
    let began = writer.position();
    let expected = plan.write(&reader, &sources, &mut AppendSink { writer: &mut writer, began })?;
    writer.finish().map_err(from_append)?;
    // Read back as a save reads back what it wrote: the pages are the objects written, in order.
    let (_, written) = open_bytes_for_save(&out).map_err(|_| SaveError::CheckFailed { detail: "doesn't open" })?;
    if written.len() != expected.len() || written.iter().zip(&expected).any(|(entry, id)| entry.id() != *id) {
        return Err(SaveError::CheckFailed { detail: "pages out of place" });
    }
    Ok(expected.len())
}

/// **Highlights alone, on bytes in memory, for the fuzzer** (review of R4.3): `target`'s pages as they are, with a highlight on its first two, written
/// as an update (the path a save of highlights alone takes: no new tree, each page written again with its list); then, if the first page's list
/// has a highlight among its first eight items, that highlight taken out by the rewrite, through the fork's own verify. Gives back the pages the
/// result has, and whether a highlight was taken out. The same plan and sinks a save uses, with no file. After each write what a save says of
/// its highlights is held against the bytes written (second review of R4.3, SF6): a failure there is [`HIGHLIGHTS_DONT_HOLD`], which a fuzzer
/// treats as a bug, not a refusal.
pub fn highlights_in_memory(target: &[u8]) -> Result<(usize, bool), SaveError> {
    let (reader, map) = open_bytes_for_save(target)?;
    let pages = u32::try_from(map.len()).map_err(|_| SaveError::Damaged)?;
    let order: Vec<PageRef> = (0..pages)
        .map(|page| PageRef {
            source: 0,
            page,
            quarter_turns: 0,
        })
        .collect();
    let line = [[10.0, 10.0, 100.0, 12.0]];
    let highlights: Vec<NewHighlight<'_>> = (0..pages.min(2)).map(|position| NewHighlight { position, rects: &line }).collect();
    let first_highlight = map.get(0).and_then(|entry| match reader.resolve_object(entry.id()) {
        Ok(Object::Dictionary(page)) => annotate::first_highlight(&reader, &page, 8),
        _ => None,
    });
    let gone: Vec<GoneHighlight> = first_highlight
        .into_iter()
        .map(|(index, rect)| GoneHighlight { position: 0, index, rect })
        .collect();
    let request = |highlights_gone| SaveRequest {
        path: Path::new("target.pdf"),
        stamp: None,
        file_pages: pages,
        pages: &order,
        added: &[],
        highlights: &highlights,
        highlights_gone,
    };
    reader.check_appendable().map_err(from_append)?;
    let plan = Plan::make(&reader, &map, &request(&[]), &[], signature_facts(&reader), Writer::Append, MAX_COPY_WORK)?;
    let mut out = Vec::new();
    let mut writer = AppendWriter::new(&reader, &mut out).map_err(from_append)?;
    let began = writer.position();
    let expected = plan.write(&reader, &[], &mut AppendSink { writer: &mut writer, began })?;
    writer.finish().map_err(from_append)?;
    let (written_reader, written) = open_bytes_for_save(&out).map_err(|_| SaveError::CheckFailed { detail: "doesn't open" })?;
    if written.len() != expected.len() {
        return Err(SaveError::CheckFailed { detail: "pages out of place" });
    }
    if !plan.highlights.hold_in(&written_reader, &expected) {
        return Err(SaveError::CheckFailed { detail: HIGHLIGHTS_DONT_HOLD });
    }
    if gone.is_empty() {
        return Ok((expected.len(), false));
    }
    reader.check_rewritable().map_err(from_rewrite)?;
    let plan = Plan::make(&reader, &map, &request(&gone), &[], signature_facts(&reader), Writer::Rewrite, MAX_COPY_WORK)?;
    let mut changes = RewriteChanges::new(&reader).map_err(from_rewrite)?;
    let cap = plan.append_cap.min(MAX_REWRITE_HELD_BYTES);
    let expected = plan.write(
        &reader,
        &[],
        &mut RewriteSink {
            changes: &mut changes,
            held: 0,
            cap,
        },
    )?;
    let mut out = Vec::new();
    let unverified = changes.write_unverified_to(RewritePolicy::new(), &mut out).map_err(from_rewrite)?;
    let bytes: std::sync::Arc<[u8]> = out.into();
    unverified
        .verify(&reader, std::sync::Arc::new(lopdf::BytesSource::new(std::sync::Arc::clone(&bytes))))
        .map_err(from_rewrite)?;
    let (written, _) = open_bytes_for_save(&bytes).map_err(|_| SaveError::CheckFailed { detail: "doesn't open" })?;
    // What a save reads back after a rewrite (S2), then what it must hold.
    let read_back = plan.highlights.read_back((&reader, &map, &order), (&written, &expected));
    if read_back.is_ok() && !plan.highlights.hold_in(&written, &expected) {
        return Err(SaveError::CheckFailed { detail: HIGHLIGHTS_DONT_HOLD });
    }
    Ok((expected.len(), true))
}

/// What [`highlights_in_memory`] says when a write doesn't hold what the save says of its highlights: a place that isn't a `/Highlight` with the
/// `/Rect` reported, or an object taken out that's still in the file.
pub const HIGHLIGHTS_DONT_HOLD: &str = "the highlights written aren't what the save says";

/// **The rewrite's planner on bytes in memory, for the fuzzer:** `target` with its first page deleted, and with `added`'s pages after the rest when `with_added`,
/// written by the rewrite into memory, checked by the fork's own verify and read back. Gives back the pages the result has. The same plan and sink a save
/// uses, with no file. A target of one page has nothing left to delete, so it's refused as a save would refuse it.
pub fn rewrite_in_memory(target: &[u8], added: &[u8], with_added: bool) -> Result<usize, SaveError> {
    let (reader, map) = open_bytes_for_save(target)?;
    reader.check_rewritable().map_err(from_rewrite)?;
    let pages = u32::try_from(map.len()).map_err(|_| SaveError::Damaged)?;
    let (source, source_map) = open_bytes_for_save(added)?;
    let added_pages = if with_added {
        u32::try_from(source_map.len()).map_err(|_| SaveError::Damaged)?.min(64)
    } else {
        0
    };
    let mut order: Vec<PageRef> = (1..pages)
        .map(|page| PageRef {
            source: 0,
            page,
            quarter_turns: u8::from(page == 1),
        })
        .collect();
    order.extend((0..added_pages).map(|page| PageRef {
        source: 1,
        page,
        quarter_turns: 3,
    }));
    if order.is_empty() {
        return Err(SaveError::NoPagesLeft);
    }
    let added_file = [AddedFile {
        path: Path::new("added.pdf"),
        page_count: added_pages,
        stamp: None,
    }];
    let request = SaveRequest {
        path: Path::new("target.pdf"),
        stamp: None,
        file_pages: pages,
        pages: &order,
        added: &added_file,
        highlights: &[],
        highlights_gone: &[],
    };
    let sources = vec![Some((source, source_map))];
    let plan = Plan::make(&reader, &map, &request, &sources, signature_facts(&reader), Writer::Rewrite, MAX_COPY_WORK)?;
    let mut changes = RewriteChanges::new(&reader).map_err(from_rewrite)?;
    changes.remove_page(0);
    let expected = plan.write(
        &reader,
        &sources,
        &mut RewriteSink {
            changes: &mut changes,
            held: 0,
            cap: plan.append_cap.min(MAX_REWRITE_HELD_BYTES),
        },
    )?;
    let mut out = Vec::new();
    let unverified = changes
        .write_unverified_to(RewritePolicy::new().remove_signatures().remove_usage_rights(), &mut out)
        .map_err(from_rewrite)?;
    let bytes: std::sync::Arc<[u8]> = out.into();
    unverified
        .verify(&reader, std::sync::Arc::new(lopdf::BytesSource::new(std::sync::Arc::clone(&bytes))))
        .map_err(from_rewrite)?;
    let (_, written) = open_bytes_for_save(&bytes).map_err(|_| SaveError::CheckFailed { detail: "doesn't open" })?;
    if written.len() != expected.len() || written.iter().zip(&expected).any(|(entry, id)| entry.id() != *id) {
        return Err(SaveError::CheckFailed { detail: "pages out of place" });
    }
    Ok(expected.len())
}

/// **The signature rules and the kept copy on bytes in memory, for the fuzzer:** what `save` decides about a file's signatures, and the rewrite `keep_pages`
/// runs for these pages, through the fork's own verify, with the pages held compared one for one. Gives back how many pages the copy holds.
pub fn signatures_and_keep_in_memory(bytes: &[u8], pages: &[u32]) -> Result<usize, SaveError> {
    let (reader, map) = open_bytes_for_save(bytes)?;
    let facts = signature_facts(&reader);
    let _ = (facts.has_persons_signature(), persons_signature(&reader));
    reader.check_rewritable().map_err(from_rewrite)?;
    let count = u32::try_from(map.len()).map_err(|_| SaveError::Damaged)?;
    let keep: HashSet<u32> = pages.iter().copied().filter(|page| *page < count).collect();
    if keep.is_empty() {
        return Err(SaveError::NoPagesLeft);
    }
    let mut changes = RewriteChanges::new(&reader).map_err(from_rewrite)?;
    for page in (0..count).filter(|page| !keep.contains(page)) {
        changes.remove_page(page as usize);
    }
    let mut out = Vec::new();
    let unverified = changes
        .write_unverified_to(RewritePolicy::new().remove_signatures().remove_usage_rights(), &mut out)
        .map_err(from_rewrite)?;
    let written: std::sync::Arc<[u8]> = out.into();
    let (_, copy) = unverified
        .verify(&reader, std::sync::Arc::new(lopdf::BytesSource::new(written)))
        .map_err(from_rewrite)?;
    Ok(copy.page_map().map(|map| map.len()).unwrap_or(0))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Failure (review of R4.3): a page's highlights are all written before the bound on what a save appends is looked at, so one page of thousands
    /// writes far past it into the temporary file before the save refuses. Held at a bound set small: what's written stops within one highlight of it.
    #[test]
    fn the_bound_on_what_a_save_appends_is_asked_between_a_pages_highlights() {
        let target = recto_testpdf::pages(&[recto_testpdf::Page::new(recto_testpdf::text_lines(&["one"]))]);
        let (reader, map) = open_bytes_for_save(&target).unwrap();
        let order = [PageRef {
            source: 0,
            page: 0,
            quarter_turns: 0,
        }];
        let lines = [[10.0, 10.0, 100.0, 12.0]; 40];
        let highlights = vec![NewHighlight { position: 0, rects: &lines }; 200];
        let request = SaveRequest {
            path: Path::new("target.pdf"),
            stamp: None,
            file_pages: 1,
            pages: &order,
            added: &[],
            highlights: &highlights,
            highlights_gone: &[],
        };
        let mut plan = Plan::make(&reader, &map, &request, &[], signature_facts(&reader), Writer::Append, MAX_COPY_WORK).unwrap();
        const CAP: u64 = 8 * 1024;
        plan.append_cap = CAP;
        let mut out = Vec::new();
        let mut writer = AppendWriter::new(&reader, &mut out).unwrap();
        let began = writer.position();
        let written = plan.write(&reader, &[], &mut AppendSink { writer: &mut writer, began });
        assert_eq!(written, Err(SaveError::TooLarge));
        let taken = writer.position() - began;
        assert!(taken < CAP + 8 * 1024, "{taken} bytes written against a bound of {CAP}");
    }

    /// **Failure (second review of R4.3, SF1): taking a file's highlights out does unbounded work on a hostile file**: each removal read its page and
    /// cloned its whole list again, unbounded, so 10,000 removals over pages sharing one long list came to about 10⁹ steps. A page's list is read once
    /// for all its removals, and the work counts against the save's bound: at the bound it's done, one past it it's refused. Here one page lists a
    /// link and four highlights, and three are taken out: the page and its five items, and the three looked at, are 1 + 5 + 3 = 9.
    #[test]
    fn taking_highlights_out_counts_against_the_saves_bound() {
        let highlight = |y: u32| recto_testpdf::Object::Dict(format!("<< /Type /Annot /Subtype /Highlight /Rect [50 {y} 150 {}] >>", y + 12));
        let target = recto_testpdf::build(&[
            recto_testpdf::Object::Dict("<< /Type /Catalog /Pages 2 0 R >>".into()),
            recto_testpdf::Object::Dict("<< /Type /Pages /Kids [3 0 R] /Count 1 >>".into()),
            recto_testpdf::Object::Dict("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Annots [4 0 R 5 0 R 6 0 R 7 0 R 8 0 R] >>".into()),
            recto_testpdf::Object::Dict("<< /Type /Annot /Subtype /Link /Rect [0 0 10 10] >>".into()),
            highlight(100),
            highlight(200),
            highlight(300),
            highlight(400),
        ]);
        let (reader, map) = open_bytes_for_save(&target).unwrap();
        let order = [PageRef {
            source: 0,
            page: 0,
            quarter_turns: 0,
        }];
        let gone: Vec<GoneHighlight> = [3u32, 1, 4]
            .into_iter()
            .map(|index| GoneHighlight {
                position: 0,
                index,
                rect: [50.0, 100.0 * index as f32, 150.0, 100.0 * index as f32 + 12.0],
            })
            .collect();
        let request = SaveRequest {
            path: Path::new("target.pdf"),
            stamp: None,
            file_pages: 1,
            pages: &order,
            added: &[],
            highlights: &[],
            highlights_gone: &gone,
        };
        let make = |limit| Plan::make(&reader, &map, &request, &[], signature_facts(&reader), Writer::Rewrite, limit).map(|_| ());
        assert_eq!(make(9), Ok(()), "at the bound");
        assert_eq!(make(8), Err(SaveError::TooLarge), "one past it");
    }
}
