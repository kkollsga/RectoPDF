//! **The sidebar's Find**: its field, the line under it and the hit list, as Rust's. The host draws them and says what was typed, clicked or pressed. Every step is the one `recto find` takes, through `SessionHost`'s `find_*`, on the session thread; the UI thread only reads what's published here and the Finder's hits, each under a short lock.
//!
//! **Where it goes** (owner, 2026-11-06: "We need to maximize vertical reading space"): in the sidebar, over the tabs, and only while it's open, so nothing is above or over a page and the sidebar at rest is still names and contents. Its list takes at most [`MOST_OF_ROOM`] of the sidebar's height and scrolls inside that, as the contents do, so the tabs and the contents stay on screen (owner, 2026-11-07: "The left sidebar toc should have a max height within scroll so it doesn't push everything of screen").
//!
//! **Which document:** the one being read, and in a comparison the pane being read, which is the session's active tab. When that changes with a query in the field, it's found again there.

use crate::{Inner, lock};
use recto_agent::Event;
use recto_agent::find::{self, Finding, Row};
use serde_json::{Value, json};

/// A row of the hit list, a page's or a hit's: one height, so the list asks nothing per row.
pub const ROW_HEIGHT: f64 = 24.0;
/// The field, and the line under it.
pub const FIELD_HEIGHT: f64 = 28.0;
pub const STATUS_HEIGHT: f64 = 20.0;
/// Space under the list, before the tabs.
pub const GAP: f64 = 8.0;
/// The most of the sidebar's room the hit list takes. With the contents' 60% of what's left, the tabs keep the rest.
pub const MOST_OF_ROOM: f64 = 0.4;
/// Rows handed over in one call. More than any screen shows.
const MAX_ROWS_FETCH: u32 = 256;

/// What the Find field shows, from the session thread, for the UI thread, which never waits on the session.
#[derive(Debug, Default)]
pub(crate) struct Field {
    open: bool,
    text: String,
    /// A step asked for while the hits were being found again, done when they're in.
    pending_step: Option<bool>,
}

/// What the host's told to do about Find, through the session thread's queue.
#[derive(Debug)]
pub(crate) enum FindCall {
    /// What's in the field now, as typed: found at once, the last query stopping.
    Typed(String),
    /// ⌘F: open, with what was last typed found again.
    Open,
    /// Escape in the field.
    Close,
    /// ⌘G, ⇧⌘G, Return and ⇧Return in the field.
    Step(bool),
    /// A click on a row of the list.
    Choose(u32),
    /// The Finder has more, or has finished.
    News,
    /// `recto find --next` went to a hit, which the page view selects.
    Reveal(u32, find::Hit),
}

/// Everything the sidebar draws of Find but its rows, which are pulled with [`rows`] as they come on screen.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct FindView {
    pub open: bool,
    /// What's in the field.
    pub text: String,
    pub placeholder: String,
    /// The line under the field, in words. Empty with nothing typed.
    pub status: String,
    /// Goes up with every change to the rows or the line, which is when to pull them.
    pub version: u64,
    pub row_count: u32,
    /// The row of the hit gone to last.
    pub current_row: Option<u32>,
    /// A query's running, which `qa wait-settled` waits for.
    pub running: bool,
    pub close_tooltip: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum FindRowKind {
    /// A page, heading its hits.
    Page,
    Hit,
}

/// A row of the hit list. A hit's words are the document's, so they're only ever drawn as text.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct FindRow {
    pub kind: FindRowKind,
    /// A page row's label, or the words before a hit.
    pub label: String,
    pub matched: String,
    pub after: String,
    pub current: bool,
}

/// The Find block's heights, for this much room in the sidebar: the whole block, and its list.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Heights {
    pub block: f64,
    pub list: f64,
}

/// Rust's height rule for Find: nothing while it's closed; the field and its line while it's open; and the list, as tall as its rows up to [`MOST_OF_ROOM`].
pub fn heights(room: f64, open: bool, rows: u32) -> Heights {
    if !open || !room.is_finite() || room <= 0.0 {
        return Heights { block: 0.0, list: 0.0 };
    }
    let list = (f64::from(rows) * ROW_HEIGHT).min((room * MOST_OF_ROOM / ROW_HEIGHT).floor() * ROW_HEIGHT).max(0.0);
    let block = (FIELD_HEIGHT + STATUS_HEIGHT + list + GAP).min(room);
    Heights { block, list: list.min((block - FIELD_HEIGHT - STATUS_HEIGHT - GAP).max(0.0)) }
}

/// What the host draws now. A short lock on the field and one on the hits.
pub(crate) fn view(inner: &Inner) -> FindView {
    let field = lock(&inner.find_field);
    let (open, text) = (field.open, field.text.clone());
    drop(field);
    let mut out = FindView { open, text, placeholder: "Find in this document".into(), status: String::new(), version: 0, row_count: 0, current_row: None, running: false, close_tooltip: "Close Find (Esc)".into() };
    inner.host.session.finder().with_results(&mut |results| {
        out.status = if open { results.status() } else { String::new() };
        out.version = results.version;
        out.running = results.running();
        if open {
            let rows = results.rows();
            out.row_count = u32::try_from(rows.len()).unwrap_or(u32::MAX);
            out.current_row = results.current.and_then(|current| rows.iter().position(|row| *row == Row::Hit(current))).and_then(|at| u32::try_from(at).ok());
        }
    });
    out
}

/// Some of the list's rows, for what's on screen.
pub(crate) fn rows(inner: &Inner, first: u32, count: u32) -> Vec<FindRow> {
    let mut out = Vec::new();
    inner.host.session.finder().with_results(&mut |results| {
        let rows = results.rows();
        let end = (first as usize).saturating_add(count.min(MAX_ROWS_FETCH) as usize).min(rows.len());
        out = rows.get(first as usize..end).unwrap_or(&[]).iter().map(|row| match *row {
            Row::Page(page) => FindRow { kind: FindRowKind::Page, label: find::page_label(page), matched: String::new(), after: String::new(), current: false },
            Row::Hit(index) => {
                let hit = &results.hits[index];
                FindRow { kind: FindRowKind::Hit, label: hit.before.to_string(), matched: hit.matched.to_string(), after: hit.after.to_string(), current: results.current == Some(index) }
            }
        }).collect();
    });
    out
}

/// The hit a row is, if it's a hit's.
fn hit_of_row(inner: &Inner, row: u32) -> Option<usize> {
    let mut hit = None;
    inner.host.session.finder().with_results(&mut |results| {
        if let Some(Row::Hit(index)) = results.rows().get(row as usize) {
            hit = Some(*index);
        }
    });
    hit
}

/// Does what the host asked, on the session thread.
pub(crate) fn act(inner: &Inner, call: FindCall, events: &mut Vec<Event>) {
    let session = &inner.host.session;
    match call {
        FindCall::Typed(text) => {
            let cut: String = text.chars().take(recto_session::record::MAX_FIND_BYTES).collect();
            if find::needle(&cut).is_some() {
                // Refused only for what `needle` already let through, as nothing being read: the field keeps it, and nothing's found.
                if session.find_begin(&cut).is_err() {
                    session.with_session(|s| s.set_find(true, "", &cut));
                }
            } else {
                session.finder().clear();
                session.with_session(|s| s.set_find(true, "", &cut));
            }
            events.push(Event::new("find_changed", json!({ "open": true })));
        }
        FindCall::Open => {
            // Find is in the sidebar, so ⌘F shows it.
            if session.with_session(|s| s.view().sidebar_hidden) {
                crate::set_view(inner, events, |view| view.sidebar_hidden = false);
            }
            let (query, draft) = session.with_session(|s| {
                let find = s.find().clone();
                s.set_find(true, &find.query, &find.draft);
                (find.query, find.draft)
            });
            // What was typed last is found again, as it would be on a Mac: it's been closed, so its hits went.
            if query.is_empty() && find::needle(&draft).is_some() {
                let _ = session.find_begin(&draft);
            }
            events.push(Event::new("find_changed", json!({ "open": true })));
        }
        FindCall::Close => {
            session.find_close();
            lock(&inner.find_field).pending_step = None;
            events.push(Event::new("find_changed", json!({ "open": false })));
        }
        FindCall::Step(forward) => {
            if !session.with_session(|s| s.find().open) {
                session.with_session(|s| {
                    let find = s.find().clone();
                    s.set_find(true, &find.query, &find.draft);
                });
            }
            if let Some(query) = session.find_stale_query() {
                let _ = session.find_begin(&query);
                lock(&inner.find_field).pending_step = Some(forward);
            } else if session.finder().is_running() && session.finder().hits_held() == 0 {
                lock(&inner.find_field).pending_step = Some(forward);
            } else if let Some((tab, hit)) = session.find_step(forward) {
                reveal(inner, tab, &hit, events);
            }
        }
        FindCall::Choose(row) => {
            if let Some(index) = hit_of_row(inner, row)
                && let Some((tab, hit)) = session.find_choose(index)
            {
                reveal(inner, tab, &hit, events);
            }
        }
        FindCall::News => {
            let finished = !session.finder().is_running();
            let has_hits = session.finder().hits_held() > 0;
            let pending = lock(&inner.find_field).pending_step;
            if let Some(forward) = pending.filter(|_| finished || has_hits) {
                lock(&inner.find_field).pending_step = None;
                if let Some((tab, hit)) = session.find_step(forward) {
                    reveal(inner, tab, &hit, events);
                }
            }
            // The hit gone to before a restart, chosen again by its place once the hits are back. Its selection came back with the page view.
            if finished && let Some(place) = session.with_session(|s| s.find().current) {
                session.finder().with_results(&mut |results| {
                    if results.current.is_none() && results.end.is_some() {
                        results.choose_place(place);
                    }
                });
            }
        }
        FindCall::Reveal(tab, hit) => reveal(inner, tab, &hit, events),
    }
    keep_up(inner);
    inner.listener.model_changed();
}

/// Selects a hit on the page and brings it into view, in the pane being read if it's that tab's.
fn reveal(inner: &Inner, tab: u32, hit: &find::Hit, events: &mut Vec<Event>) {
    let engine = inner.panes.reading_engine(&inner.engine);
    if engine.hot_tab() == Some(tab) {
        engine.reveal(hit.page, hit.start, hit.end);
    }
    events.push(Event::new("page_changed", json!({ "tab": tab, "page": hit.page + 1 })));
}

/// Keeps Find with the document being read, and the field's mirror with the session. Called on the session thread after every change: a query in the field is found again when the document being read isn't the one its hits are of, as after a restart, a tap or a move into another pane.
pub(crate) fn keep_up(inner: &Inner) {
    let session = &inner.host.session;
    let (record, active) = session.with_session(|s| (s.find().clone(), s.active()));
    {
        let mut field = lock(&inner.find_field);
        if field.open != record.open || field.text != record.draft {
            field.open = record.open;
            field.text.clone_from(&record.draft);
        }
    }
    if !record.open || record.query.is_empty() {
        return;
    }
    let mut tab = None;
    let mut query = String::new();
    session.finder().with_results(&mut |results| {
        tab = results.tab;
        query.clone_from(&results.query);
    });
    if let Some(active) = active
        && (tab != Some(active.0) || query != record.query)
    {
        // The field stays as it is: a draft typed and not run is the reader's.
        if let Some(needle) = find::needle(&record.query)
            && let Ok(plan) = session.with_session(|s| find::plan(s, active))
        {
            session.finder().start(plan, &record.query, needle);
        }
    }
}

/// What QA reads of Find.
pub(crate) fn report(inner: &Inner) -> Value {
    let view = view(inner);
    let finder = inner.host.session.finder();
    let mut hits = 0;
    finder.with_results(&mut |results| hits = results.hits.len());
    json!({ "open": view.open, "untrusted": { "text": view.text }, "status": view.status, "rows": view.row_count, "current_row": view.current_row, "running": view.running, "hits_held": hits, "thread": finder.has_thread() })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Failure: Find's list grows with its hits and pushes the tabs and the contents off the sidebar (owner, 2026-11-07: "max height within scroll so it doesn't push everything of screen"), or takes room while it's closed.
    #[test]
    fn the_list_never_takes_more_than_its_share() {
        assert_eq!(heights(800.0, false, 500), Heights { block: 0.0, list: 0.0 });
        let few = heights(800.0, true, 3);
        assert_eq!(few.list, 3.0 * ROW_HEIGHT, "only what it needs");
        let many = heights(800.0, true, 2000);
        assert!(many.list <= 800.0 * MOST_OF_ROOM && many.list > 0.0, "{many:?}");
        assert_eq!(many.list % ROW_HEIGHT, 0.0, "whole rows");
        let none = heights(800.0, true, 0);
        assert_eq!(none.block, FIELD_HEIGHT + STATUS_HEIGHT + GAP);
        let tiny = heights(20.0, true, 50);
        assert!(tiny.block <= 20.0 && tiny.list >= 0.0, "{tiny:?}");
        assert_eq!(heights(f64::NAN, true, 5).block, 0.0);
    }
}
