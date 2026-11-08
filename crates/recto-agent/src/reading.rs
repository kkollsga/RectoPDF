//! Commands that read a document's structure: facts, contents and signatures. Each reads the file outside the session, and every string from the file goes under `untrusted`.

use crate::{AgentError, SessionAccess, exit, generated, select};
use recto_core::{OpenError, SignatureEntry, SignatureState};
use recto_session::DocLoader;
use recto_sign::{SignatureFacts, Status};
use serde_json::{Value, json};
use std::path::PathBuf;

/// Contents entries in one answer if not told otherwise, which fits in an agent's reading without filling it.
const DEFAULT_CONTENTS: u32 = 200;
const MAX_CONTENTS: u32 = 2000;

pub(crate) struct TabFile {
    id: u32,
    path: PathBuf,
    name: String,
    pub(crate) pages: u32,
    edited: bool,
    renderer_only: bool,
    /// For each of the file's pages, its place now. Only for an edited tab.
    positions: Option<Vec<u32>>,
    /// The file isn't the one those edits' page numbers came from.
    changed: bool,
    /// What the file was when those page numbers were made, to look at it against.
    check_against: Option<recto_platform::FileStamp>,
}

impl TabFile {
    /// Refuses when page numbers here would be about a different version of the file.
    fn require_current(&self) -> Result<(), AgentError> {
        if self.changed {
            return Err(crate::source_changed(&self.path));
        }
        Ok(())
    }
}

fn tab_file<A: SessionAccess>(access: &A, tab: &str) -> Result<TabFile, AgentError> {
    access
        .with(|s| {
            let id = select(s, tab)?;
            let record = s
                .tab(id)
                .ok_or_else(|| AgentError::new("no_such_tab", "That tab isn't open.", exit::NOT_FOUND))?;
            let positions = s.page_edits(id).map(|edits| {
                let mut positions = vec![u32::MAX; record.page_count as usize];
                for (position, slot) in edits.slots().iter().enumerate() {
                    if slot.source == 0
                        && let Some(place) = positions.get_mut(slot.page as usize)
                    {
                        *place = u32::try_from(position).unwrap_or(u32::MAX);
                    }
                }
                positions
            });
            Ok(TabFile {
                id: id.0,
                path: s
                    .path_of(id)
                    .ok_or_else(|| AgentError::new("no_such_tab", "That tab isn't open.", exit::NOT_FOUND))?,
                name: record.name.to_string(),
                pages: s.pages_in(id),
                edited: s.is_edited(id),
                renderer_only: record.renderer_only,
                positions,
                changed: false,
                check_against: s.page_edits(id).is_some().then(|| s.own_stamp(id)).flatten(),
            })
        })
        // Whether the file has changed is asked with the session free: a stat on a dead volume holds up this command and no other.
        .map(|mut file: TabFile| {
            file.changed = file
                .check_against
                .is_some_and(|stamp| recto_platform::file_stamp(&file.path).ok() != Some(stamp));
            file
        })
}

pub(crate) fn facts<A: SessionAccess>(access: &A, tab: &str) -> Result<Value, AgentError> {
    let file = tab_file(access, tab)?;
    let loader = access.with(|s| s.loader().clone());
    let facts = loader.load(&file.path)?;
    // What the structure reader never saw is null, and null isn't "no". Encryption is the case that matters most: a file refused for an odd encryption dictionary could read as not encrypted.
    let known = !facts.renderer_only;
    Ok(json!({
        "tab": file.id,
        "pages": file.pages,
        "file_pages": facts.page_count,
        "file_bytes": facts.byte_len,
        "encrypted": known.then_some(facts.encrypted),
        // Only a file anyone may read opens here, since a password is never taken. Worth saying: "encrypted" alone reads as though its contents are protected from whoever has the file.
        "encryption_note": (known && facts.encrypted).then_some("It's encrypted, and opened with no password, because the file allows anyone to read it. What it restricts beyond that, such as copying or printing, isn't reported here."),
        "pdf_version": (!facts.version.is_empty()).then_some(&*facts.version),
        "structure": if facts.renderer_only { "renderer_only" } else { "readable" },
        "has_contents": known.then(|| !facts.toc.is_empty()),
        "contents_entries": known.then(|| facts.toc.entries().len()),
        "contents_verdict": known.then(|| recto_core::verdict::judge(&facts.toc, facts.page_count).code()),
        "edited": file.edited,
        "file_changed_since_edits": file.changed,
        "note": facts.renderer_only.then_some("Only the renderer could read this file. Its text and pages can be read, but its title, contents and signatures can't be, and it can't be edited."),
        "untrusted": { "name": file.name, "title": facts.title.as_deref() },
    }))
}

/// A tab's file and its key now, for generated contents. Refused, as `contents` is, when page numbers would be about another version of the file.
pub(crate) fn tab_file_keyed<A: SessionAccess>(access: &A, tab: &str) -> Result<(TabFile, generated::Keyed), AgentError> {
    let file = tab_file(access, tab)?;
    file.require_current()?;
    let key = generated::key_of(&file.path)?;
    let keyed = generated::Keyed { path: file.path.clone(), key };
    Ok((file, keyed))
}

/// The words that go with generated contents wherever they're given, so nobody takes them for the file's own.
pub const GENERATED_NOTE: &str = "Generated by RectoPDF from the headings distillPDF found on the pages. They aren't part of the file, which is never changed for them, and they can be wrong. Headings whose text looked hidden are left out. `place` is how far down its page a heading is, in thousandths.";

pub(crate) fn contents<A: SessionAccess>(
    access: &A,
    tab: &str,
    from: Option<u32>,
    limit: Option<u32>,
    depth: Option<u8>,
    generated: bool,
) -> Result<Value, AgentError> {
    if generated {
        return generated_contents(access, tab, from, limit, depth);
    }
    let file = tab_file(access, tab)?;
    file.require_current()?;
    let unavailable = || json!({ "tab": file.id, "available": false, "note": "Only the renderer can read this file, so its contents can't be read. That doesn't mean it has none.", "entries": [], "total": null, "next_from": null });
    if file.renderer_only {
        return Ok(unavailable());
    }
    let loader = access.with(|s| s.loader().clone());
    // Read fresh, so it may have changed since the tab opened: a file that's now only the renderer's has no contents to give, which isn't the same as having none.
    let facts = loader.load(&file.path)?;
    if facts.renderer_only {
        return Ok(unavailable());
    }
    let verdict = recto_core::verdict::judge(&facts.toc, facts.page_count);
    let mut answer = listing(&file, &facts.toc, None, from, limit, depth)?;
    answer["verdict"] = json!(verdict.code());
    if verdict.offers() {
        answer["verdict_note"] = json!(format!(
            "{} `recto contents --generated` gives contents made from the headings on its pages.",
            verdict.words()
        ));
    }
    Ok(answer)
}

/// `recto contents --generated`: the ones made from the document's headings, if they're ready. Asking is what starts them, and an answer that isn't ready says so at once.
fn generated_contents<A: SessionAccess>(access: &A, tab: &str, from: Option<u32>, limit: Option<u32>, depth: Option<u8>) -> Result<Value, AgentError> {
    let generating = access.generating().ok_or_else(generated::no_generating)?;
    let (file, keyed) = tab_file_keyed(access, tab)?;
    if file.renderer_only {
        return Ok(
            json!({ "tab": file.id, "available": false, "generated": true, "state": "refused", "note": "Only the renderer can read this file, so distillPDF can't either.", "entries": [], "total": null, "next_from": null }),
        );
    }
    if let Some(waiting) = generated::not_ready(generating, &keyed, file.pages, true) {
        let (state, note) = waiting?;
        return Ok(
            json!({ "tab": file.id, "available": false, "generated": true, "state": state, "note": note, "entries": [], "total": null, "next_from": null }),
        );
    }
    let Some(made) = generated::find(generating, &keyed.path, keyed.key) else {
        return Err(AgentError::new("generating_failed", "Generated contents were made but couldn't be read back.", exit::FAILED).then("Ask again."));
    };
    let toc = made.toc();
    let places: Vec<u16> = made.entries.iter().map(|entry| entry.place).collect();
    let mut answer = listing(&file, &toc, Some(&places), from, limit, depth)?;
    answer["generated"] = json!(true);
    answer["state"] = json!("ready");
    answer["note"] = json!(if made.entries.is_empty() {
        "No headings were found on its pages, so there are no generated contents. A scan with no text has none to find, and neither has a document set all in one size."
    } else {
        GENERATED_NOTE
    });
    answer["hidden_left_out"] = json!(made.hidden_dropped);
    answer["pages_not_checked"] = json!(made.pages_not_checked);
    answer["pages_not_read"] = json!(made.pages_failed);
    Ok(answer)
}

/// A piece of a list of contents, the file's own or generated, with `place` beside each entry when there are places.
fn listing(
    file: &TabFile,
    toc: &recto_core::Toc,
    places: Option<&[u16]>,
    from: Option<u32>,
    limit: Option<u32>,
    depth: Option<u8>,
) -> Result<Value, AgentError> {
    let limit = crate::positive_limit(limit, DEFAULT_CONTENTS, MAX_CONTENTS)?;
    let from = from.unwrap_or(0) as usize;
    let shape = OutlineShape::of(toc.entries().iter().map(|entry| (entry.depth, entry.page)));
    // Decided from a fixed room, not from `--limit`, so asking for the next piece keeps to the same entries, however large a piece it asks for.
    let shown_depth = depth.unwrap_or_else(|| shape.depth_that_fits(DEFAULT_CONTENTS as usize));
    let visible: Vec<(usize, &recto_core::TocEntry)> = toc.entries().iter().enumerate().filter(|(_, entry)| entry.depth <= shown_depth).collect();
    let entries: Vec<Value> = visible
        .iter()
        .skip(from)
        .take(limit)
        .map(|(index, entry)| {
            let page = (entry.page != recto_core::NONE)
                .then(|| match &file.positions {
                    Some(positions) => positions.get(entry.page as usize).copied().filter(|position| *position != u32::MAX),
                    None => Some(entry.page),
                })
                .flatten();
            let mut item = json!({ "depth": entry.depth, "page": page.map(|page| page + 1), "untrusted": { "title": toc.title(entry) } });
            if let Some(place) = places.and_then(|places| places.get(*index)) {
                item["place"] = json!(place);
            }
            item
        })
        .collect();
    let next_from = (from + entries.len() < visible.len()).then_some(from + entries.len());
    let rule = shape.rule(visible.len(), shown_depth, depth.is_some());
    Ok(json!({
        "tab": file.id,
        "available": true,
        "total": visible.len(),
        "entries": entries,
        "next_from": next_from,
        "truncated": toc.truncated,
        "all_entries": toc.entries().len(),
        "depth_shown": shown_depth,
        "deepest": shape.deepest,
        "sections_at": shape.sections_at,
        "sections": shape.counts[usize::from(shape.sections_at)],
        "depth_rule": rule,
    }))
}

/// What an outline looks like from above, in one pass and with no allocation. A real one is at most a few levels deep, and a hostile one's depth is a `u8`.
struct OutlineShape {
    /// Entries at each depth.
    counts: [u32; 256],
    deepest: u8,
    /// The first depth whose entries are on more than one page: the document's real sections. An outline can open with a title, or two, on the first page, with everything under them.
    sections_at: u8,
}

impl OutlineShape {
    fn of(entries: impl Iterator<Item = (u8, u32)>) -> Self {
        let mut counts = [0u32; 256];
        let mut first_page = [recto_core::NONE; 256];
        let mut spread = [false; 256];
        let mut deepest = 0u8;
        for (depth, page) in entries {
            let at = usize::from(depth);
            counts[at] = counts[at].saturating_add(1);
            deepest = deepest.max(depth);
            if page != recto_core::NONE {
                if first_page[at] == recto_core::NONE {
                    first_page[at] = page;
                } else if first_page[at] != page {
                    spread[at] = true;
                }
            }
        }
        let sections_at = spread.iter().position(|spread| *spread).map_or(0, |at| at as u8);
        Self { counts, deepest, sections_at }
    }

    /// The deepest level that fits in `room` entries with everything above it, but never above the sections, whatever they number.
    fn depth_that_fits(&self, room: usize) -> u8 {
        let mut so_far = 0usize;
        let mut best = self.sections_at;
        for depth in 0..=self.deepest {
            so_far += self.counts[usize::from(depth)] as usize;
            if so_far <= room && depth >= self.sections_at {
                best = depth;
            }
        }
        best
    }

    /// Says which levels these are, and how to get the others. Numbers and commands only, so nothing from the file.
    fn rule(&self, shown: usize, depth: u8, asked: bool) -> String {
        let all: usize = self.counts.iter().map(|count| *count as usize).sum();
        let mut rule = if self.deepest == 0 {
            format!("{all} entries, all at one level.")
        } else if shown >= all {
            format!("All {all} entries, down to depth {}.", self.deepest)
        } else if asked {
            format!("Down to depth {depth}, as asked: {shown} of {all} entries. Depth goes to {}.", self.deepest)
        } else {
            format!(
                "Down to depth {depth} of {}, which is what fits: {shown} of {all} entries. `recto contents --depth {}` shows them all.",
                self.deepest, self.deepest
            )
        };
        let sections = self.counts[usize::from(self.sections_at)];
        if depth < self.sections_at {
            rule.push_str(&format!(" That's only the document's title: its top-level sections are the {sections} entries at depth {0}, so `recto contents --depth {0}` shows them.", self.sections_at));
        } else if self.sections_at > 0 {
            rule.push_str(&format!(
                " What's above depth {0} is the document's title: its top-level sections are the {sections} entries at depth {0}.",
                self.sections_at
            ));
        } else if self.deepest > 0 {
            rule.push_str(&format!(" Its top-level sections are the {sections} entries at depth 0."));
        }
        rule
    }
}

pub(crate) fn signatures<A: SessionAccess>(access: &A, tab: &str) -> Result<Value, AgentError> {
    let file = tab_file(access, tab)?;
    file.require_current()?;
    let not_read = || {
        json!({
            "tab": file.id,
            "structure_read": false,
            "count": null,
            "signed": null,
            "signatures": [],
            "note": "This file's structure couldn't be read, so RectoPDF can't say whether it holds signatures. That doesn't mean it has none.",
        })
    };
    if file.renderer_only {
        return Ok(not_read());
    }
    let inventory = match recto_core::signatures(&file.path) {
        // The file was readable when it opened, and isn't now. That's the same answer: not read, rather than none.
        Err(OpenError::NotPdf) => return Ok(not_read()),
        other => other?,
    };
    if !inventory.structure_read {
        return Ok(not_read());
    }
    let rows: Vec<Value> = inventory.entries.iter().map(|entry| signature_json(entry, file.positions.as_deref())).collect();
    let signed = inventory.entries.iter().filter(|entry| entry.state == SignatureState::Signed).count();
    let partial = inventory.truncated || inventory.unread > 0;
    let mut note = "These are facts from the file. RectoPDF hasn't checked any signature, and says nothing about whether one holds up.".to_string();
    if partial {
        note.push_str(" Part of the file's form couldn't be read or wasn't reached, so there may be more than these.");
    }
    if file.edited {
        note.push_str(" They come from the file on disk, not from the unsaved edits: an edit that moves a page doesn't move what a signature covers.");
    }
    Ok(json!({
        "tab": file.id,
        "structure_read": true,
        "count": rows.len(),
        "signed": signed,
        "truncated": inventory.truncated,
        "unread": inventory.unread,
        "partial": partial,
        "edited": file.edited,
        "certification_named": inventory.certified,
        "signatures": rows,
        "note": note,
    }))
}

fn signature_json(entry: &SignatureEntry, positions: Option<&[u32]>) -> Value {
    let (summary, status) = match entry.state {
        SignatureState::Signed => (
            SignatureFacts {
                certificate_name: None,
                status: Status::NotChecked,
            }
            .summary(),
            "not_checked",
        ),
        SignatureState::Empty => ("Signature field, empty", "empty"),
        SignatureState::Unread => ("Signature field that couldn't be read", "unread"),
    };
    // `page` is where it is now, like every other page number. Null if the edits removed that page.
    let page = entry.page.and_then(|page| match positions {
        Some(positions) => positions.get(page as usize).copied().filter(|position| *position != u32::MAX),
        None => Some(page),
    });
    json!({
        "summary": summary,
        "status": status,
        "page": page.map(|page| page + 1),
        "file_page": entry.page.map(|page| page + 1),
        "byte_range": entry.byte_range,
        "bytes_after_range": entry.bytes_after_range,
        "untrusted": {
            "field": entry.field,
            "filter": entry.filter,
            "sub_filter": entry.sub_filter,
            "stated_name": entry.stated_name,
            "stated_reason": entry.stated_reason,
            "stated_location": entry.stated_location,
            "stated_time": entry.stated_time,
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Failure: an agent follows the skill's `--depth 0` on an outline with one title at the root, and gets the title and nothing else, as on RFC 9110. And a large outline's default stops halfway through a deep level, with the later sections out of sight.
    #[test]
    fn the_contents_find_the_real_sections_and_say_so() {
        let rfc: Vec<(u8, u32)> = [(0, 0), (0, 0)]
            .into_iter()
            .chain((1..40).map(|at| (1, at)))
            .chain((1..150).map(|at| (2, at)))
            .chain((1..250).map(|at| (3, at)))
            .collect();
        let shape = OutlineShape::of(rfc.iter().copied());
        assert_eq!((shape.sections_at, shape.deepest), (1, 3));
        assert_eq!(shape.depth_that_fits(200), 2, "the sections and what fits under them");
        assert_eq!(shape.depth_that_fits(1), 1, "the sections, even if they don't all fit");
        let just_the_title = shape.rule(2, 0, true);
        assert!(just_the_title.contains("recto contents --depth 1"), "{just_the_title}");
        let default = shape.rule(190, 2, false);
        assert!(default.contains("depth 2") && default.contains("--depth 3"), "{default}");
        // Failure: an agent took two calls to work out what "top-level" meant on an outline with title entries at depth 0, so it's said in words, with how many.
        assert!(default.contains("top-level sections are the 39 entries at depth 1"), "{default}");
        let plain = OutlineShape::of([(0, 0), (0, 3), (1, 4)].into_iter());
        assert!(
            plain.rule(3, 1, false).contains("top-level sections are the 2 entries at depth 0"),
            "{}",
            plain.rule(3, 1, false)
        );

        let flat = OutlineShape::of((0..300).map(|at| (0, at)));
        assert_eq!((flat.sections_at, flat.depth_that_fits(200)), (0, 0), "there's nothing else to show");
        assert!(flat.rule(300, 0, false).contains("one level"));

        let one_root = OutlineShape::of([(0, 0), (1, 0), (1, 4), (2, 5)].into_iter());
        assert_eq!((one_root.sections_at, one_root.depth_that_fits(200)), (1, 2));

        let nothing_placed = OutlineShape::of([(0, recto_core::NONE), (1, recto_core::NONE)].into_iter());
        assert_eq!(nothing_placed.sections_at, 0);
        assert_eq!(OutlineShape::of(std::iter::empty()).depth_that_fits(200), 0);
    }
}
