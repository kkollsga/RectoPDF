//! Commands that read a document's structure: facts, contents and signatures. Each reads the file outside the session, and every string from the file goes under `untrusted`.

use crate::{AgentError, SessionAccess, exit, select};
use recto_core::SignatureEntry;
use recto_session::DocLoader;
use recto_sign::{SignatureFacts, Status};
use serde_json::{Value, json};
use std::path::PathBuf;

/// Contents entries in one answer if not told otherwise, which fits in an agent's reading without filling it.
const DEFAULT_CONTENTS: u32 = 200;
const MAX_CONTENTS: u32 = 2000;

struct TabFile {
    id: u32,
    path: PathBuf,
    name: String,
    pages: u32,
    edited: bool,
    renderer_only: bool,
    /// For each of the file's pages, its place now. Only for an edited tab.
    positions: Option<Vec<u32>>,
}

fn tab_file<A: SessionAccess>(access: &A, tab: &str) -> Result<TabFile, AgentError> {
    access.with(|s| {
        let id = select(s, tab)?;
        let record = s.tab(id).ok_or_else(|| AgentError::new("no_such_tab", "That tab isn't open.", exit::NOT_FOUND))?;
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
        Ok(TabFile { id: id.0, path: s.path_of(id).ok_or_else(|| AgentError::new("no_such_tab", "That tab isn't open.", exit::NOT_FOUND))?, name: record.name.to_string(), pages: s.pages_in(id), edited: s.is_edited(id), renderer_only: record.renderer_only, positions })
    })
}

pub(crate) fn facts<A: SessionAccess>(access: &A, tab: &str) -> Result<Value, AgentError> {
    let file = tab_file(access, tab)?;
    let loader = access.with(|s| s.loader().clone());
    let facts = loader.load(&file.path)?;
    Ok(json!({
        "tab": file.id,
        "pages": file.pages,
        "file_pages": facts.page_count,
        "file_bytes": facts.byte_len,
        "encrypted": facts.encrypted,
        "pdf_version": (!facts.version.is_empty()).then_some(&*facts.version),
        "structure": if facts.renderer_only { "renderer_only" } else { "readable" },
        "has_contents": !facts.toc.is_empty(),
        "contents_entries": facts.toc.entries().len(),
        "edited": file.edited,
        "note": facts.renderer_only.then_some("Only the renderer could read this file. Its text and pages can be read, but its title, contents and signatures can't be, and it can't be edited."),
        "untrusted": { "name": file.name, "title": facts.title.as_deref() },
    }))
}

pub(crate) fn contents<A: SessionAccess>(access: &A, tab: &str, from: Option<u32>, limit: Option<u32>, depth: Option<u8>) -> Result<Value, AgentError> {
    let file = tab_file(access, tab)?;
    if file.renderer_only {
        return Ok(json!({ "tab": file.id, "available": false, "note": "Only the renderer could read this file, so its contents can't be read. That doesn't mean it has none.", "entries": [], "total": 0, "next_from": null }));
    }
    let loader = access.with(|s| s.loader().clone());
    let toc = loader.load(&file.path)?.toc;
    let limit = limit.unwrap_or(DEFAULT_CONTENTS).clamp(1, MAX_CONTENTS) as usize;
    let from = from.unwrap_or(0) as usize;
    let visible: Vec<_> = toc.entries().iter().filter(|entry| depth.is_none_or(|depth| entry.depth <= depth)).collect();
    let entries: Vec<Value> = visible
        .iter()
        .skip(from)
        .take(limit)
        .map(|entry| {
            let page = (entry.page != recto_core::NONE)
                .then(|| match &file.positions {
                    Some(positions) => positions.get(entry.page as usize).copied().filter(|position| *position != u32::MAX),
                    None => Some(entry.page),
                })
                .flatten();
            json!({ "depth": entry.depth, "page": page.map(|page| page + 1), "untrusted": { "title": toc.title(entry) } })
        })
        .collect();
    let next_from = (from + entries.len() < visible.len()).then_some(from + entries.len());
    Ok(json!({ "tab": file.id, "available": true, "total": visible.len(), "entries": entries, "next_from": next_from }))
}

pub(crate) fn signatures<A: SessionAccess>(access: &A, tab: &str) -> Result<Value, AgentError> {
    let file = tab_file(access, tab)?;
    if file.renderer_only {
        return Ok(json!({
            "tab": file.id,
            "structure_read": false,
            "count": null,
            "signatures": [],
            "note": "Only the renderer could read this file, so RectoPDF can't say whether it holds signatures. That doesn't mean it has none.",
        }));
    }
    let inventory = recto_core::signatures(&file.path)?;
    let rows: Vec<Value> = inventory.entries.iter().map(signature_json).collect();
    let signed = inventory.entries.iter().filter(|entry| entry.signed).count();
    Ok(json!({
        "tab": file.id,
        "structure_read": true,
        "count": rows.len(),
        "signed": signed,
        "truncated": inventory.truncated,
        "certification_named": inventory.certified,
        "signatures": rows,
        "note": "These are facts from the file. RectoPDF hasn't checked any signature, and says nothing about whether one holds up.",
    }))
}

fn signature_json(entry: &SignatureEntry) -> Value {
    let summary = if entry.signed { SignatureFacts { certificate_name: None, status: Status::NotChecked }.summary() } else { "Signature field, empty" };
    json!({
        "summary": summary,
        "status": if entry.signed { "not_checked" } else { "empty" },
        "page": entry.page.map(|page| page + 1),
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
