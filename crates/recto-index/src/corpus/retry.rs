//! Reading again the pages that failed only for the moment they were read: the new text goes on the end of the pack and the table is replaced in one step.

use super::*;

/// Reads pages that failed for a temporary reason again, and puts in place what comes back. A search may be reading this corpus now, so what it holds stays good throughout: the new text goes on the end of the pack, and then the table goes in place in one step. A reader takes the table before the pack, so any table it has only points at text it can see. Killed partway, the worst left is text nothing points at.
#[allow(clippy::too_many_arguments)]
pub(super) fn retry_failed_pages(
    find_key: &FindKey,
    paths: &CorpusPaths,
    seal: &CorpusSeal,
    feed: &mut impl PageFeed,
    meta: &DocMeta,
    summary: Summary,
    limits: &BuildLimits,
) -> io::Result<BuildOutcome> {
    let mut table = read_table(paths, summary.pages)?;
    let width = summary.width as usize;
    let reader = local::open_private_file(&paths.pack)?;
    let reader_len = reader.metadata()?.len();
    let mut pack = local::append_private_file(&paths.pack)?;
    let mut offset = pack.metadata()?.len();
    let mut bytes = summary.corpus_bytes;
    feed.forget_failures();
    let mut tried = 0;
    let (mut blob, mut chunk, mut buffer) = (crate::seal::Plain::default(), Vec::new(), crate::seal::Plain::default());
    for page in 0..summary.pages {
        let at = page as usize * RECORD_LEN;
        let old = table
            .get(at..at + RECORD_LEN)
            .and_then(Entry::decode)
            .and_then(|entry| open_page(&reader, reader_len, seal, page, entry, &mut buffer))
            .map(|(record, _)| record)
            .ok_or(io::ErrorKind::InvalidData)?;
        if old.kind != PageKind::Failed || !FailReason::from_byte(old.reason).is_temporary() {
            continue;
        }
        if tried >= MAX_RETRY_PAGES {
            break;
        }
        tried += 1;
        if !feed.still_the_same_file() {
            return throw_away(paths);
        }
        let read = match feed.read(page) {
            Ok(read) => Ok(read),
            Err(FeedError::Page(reason)) => Err(reason),
            Err(FeedError::Refused(code)) => return refuse(paths, code),
            // It can't be tried now, which says nothing about this page.
            Err(FeedError::Cant(_)) => break,
            Err(FeedError::FileChanged) => return throw_away(paths),
        };
        let record = digest_into(read, &mut blob);
        seal_page(seal, page, &record, &blob, &mut chunk)?;
        // Past the cap it stays failed: a search still reads it live, and what's here doesn't push other documents out.
        if bytes + chunk.len() as u64 > limits.max_doc_bytes {
            break;
        }
        pack.write_all(&chunk)?;
        table[at..at + RECORD_LEN].copy_from_slice(
            &Entry {
                offset,
                len: chunk.len() as u32,
            }
            .encode(),
        );
        offset += chunk.len() as u64;
        bytes += chunk.len() as u64;
    }
    pack.sync_all()?;
    local::write_private_file(&paths.table, &table)?;
    let summary = finish(paths, seal, summary.pages, width, offset, bytes, meta, summary.retries + 1)?;
    // An agent's folder made before this says these pages failed, so what it says goes: it mustn't say what's no longer so. It's made anew only inside a folder a live session holds, keeping that session's hold, and never made where there's none: once its session has set it aside, nothing appears in its place (the second review's item 3).
    if fs::symlink_metadata(&paths.manifest).is_ok() {
        agent_life::empty_agent_folder(paths);
        if agent_life::held_by_a_session(&paths.agent) && make_agent_folder(find_key, paths, &BuildLimits::default(), Making::IntoAHeldFolder).is_err() {
            agent_life::empty_agent_folder(paths);
        }
    }
    Ok(BuildOutcome::AlreadyReady(summary))
}
