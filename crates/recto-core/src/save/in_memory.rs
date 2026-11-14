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
/// of `added` copied in at the end, written as an update into memory and read back. Gives back the pages the result has. The same code a save runs, with no file.
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
    let request = SaveRequest {
        path: Path::new("target.pdf"),
        stamp: None,
        file_pages: pages,
        pages: &order,
        added: &added_file,
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
