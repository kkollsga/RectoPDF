//! **The sidebar's Find**: its field, the line under it and the hit list, as Rust's. The host draws them and says what was typed, clicked or pressed, which is the person's `find`, the command an agent sends, run by the one executor on the session thread (`core/find.rs`); what's here is what it shows, the step it keeps for a hit not known yet, and keeping up with the document being read. The UI thread only reads what's published here and the Finder's hits, each under a short lock.
//!
//! **Where it goes** (owner, 2026-11-06: "We need to maximize vertical reading space"): in the sidebar, over the tabs, and only while it's open, so nothing is above or over a page and the sidebar at rest is still names and contents. Its list takes at most [`MOST_OF_ROOM`] of the sidebar's height and scrolls inside that, as the contents do, so the tabs and the contents stay on screen (owner, 2026-11-07: "The left sidebar toc should have a max height within scroll so it doesn't push everything of screen").
//!
//! **Which document:** the one being read, and in a comparison the pane being read, which is the session's active tab. When that changes with a query in the field, it's found again there.
//!
//! It reaches the rest of the app through a [`FindHandle`]: the session host, the field's mirror, the queue's frozen flag, the panes and the page view, the listener, and the one
//! way the view is changed, passed in as a function so it stays the app's.

use crate::listener::Listener;
use crate::lock;
use crate::panes::Panes;
use recto_agent::find::{self, Finding, Going, Row};
use recto_agent::{DrawableLoader, Event};
use recto_control::host::SessionHost;
use recto_reader::Engine;
use serde_json::{Value, json};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

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
/// What a hit on hidden text says in the list.
const HIDDEN_NOTE: &str = "hidden text";

/// What the Find field shows, from the session thread, for the UI thread, which never waits on the session.
#[derive(Debug, Default)]
pub struct Field {
    open: bool,
    text: String,
    /// A step asked for before a hit for it was known, done when one is, or when the query ends.
    pending_step: Option<(bool, Going)>,
    /// What was typed can't be found, in Rust's words, as when it's too long.
    note: Option<String>,
}

impl Field {
    /// A step kept for its hit, forgotten: Find's closed.
    pub fn clear_pending(&mut self) {
        self.pending_step = None;
    }

    /// Why what was typed can't be found, in Rust's words, or nothing.
    pub fn set_note(&mut self, note: Option<String>) {
        self.note = note;
    }
}

/// Whether Find's open, as the UI thread last heard: under the mirror's short lock, never the session's.
pub fn is_open(field: &Mutex<Field>) -> bool {
    lock(field).open
}

/// What's left for Find on the session thread's queue: what the person does at it is `find`, the command (`core/find.rs`).
#[derive(Debug)]
pub enum FindCall {
    /// The Finder has more, or has finished.
    News,
    /// What an agent's `find` asks of the window: its hit selected on the page (`Effect::Reveal`).
    Effect(recto_agent::Effect),
}

impl FindCall {
    /// Whether a hand-off's freeze holds it back: see `CoreCall::is_user_action`. The Finder's news isn't a person's doing, and dropped it would leave the list as it was for good if the freeze lifted, with no more news coming.
    #[must_use]
    pub fn is_user_action(&self) -> bool {
        match self {
            // An agent's step, shown as a person's: it moves the page view, which wouldn't come back.
            Self::Effect(_) => true,
            Self::News => false,
        }
    }
}

/// Everything the sidebar draws of Find but its rows, which are pulled with [`FindHandle::rows`] as they come on screen.
#[derive(Clone, Debug, PartialEq, Eq)]
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FindRowKind {
    /// A page, heading its hits.
    Page,
    Hit,
}

/// A row of the hit list. A hit's words are the document's, so they're only ever drawn as text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FindRow {
    pub kind: FindRowKind,
    /// A page row's label, or the words before a hit.
    pub label: String,
    pub matched: String,
    pub after: String,
    pub current: bool,
    /// Its words include text that looks hidden, so it isn't gone to. What to say of it, quietly, or empty.
    pub note: String,
}

/// The Find block's heights, for this much room in the sidebar: the whole block, and its list.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Heights {
    pub block: f64,
    pub list: f64,
}

/// Rust's height rule for Find: nothing while it's closed; the field and its line while it's open; and the list, as tall as its rows up to [`MOST_OF_ROOM`].
#[must_use]
pub fn heights(room: f64, open: bool, rows: u32) -> Heights {
    if !open || !room.is_finite() || room <= 0.0 {
        return Heights { block: 0.0, list: 0.0 };
    }
    let list = (f64::from(rows) * ROW_HEIGHT)
        .min((room * MOST_OF_ROOM / ROW_HEIGHT).floor() * ROW_HEIGHT)
        .max(0.0);
    let block = (FIELD_HEIGHT + STATUS_HEIGHT + list + GAP).min(room);
    Heights {
        block,
        list: list.min((block - FIELD_HEIGHT - STATUS_HEIGHT - GAP).max(0.0)),
    }
}

/// What Find reaches: the handful of the app's parts it uses, borrowed for one call.
pub struct FindHandle<'a, L: Listener + ?Sized> {
    pub session: &'a SessionHost<DrawableLoader>,
    /// The field's mirror, for the UI thread.
    pub field: &'a Mutex<Field>,
    /// The session thread's queue is frozen for a hand-off.
    pub frozen: &'a AtomicBool,
    pub panes: &'a Panes,
    /// The app's own page view, pane 0.
    pub engine: &'a Engine,
    pub listener: &'a L,
}

impl<L: Listener + ?Sized> FindHandle<'_, L> {
    /// What the host draws now. A short lock on the field and one on the hits.
    #[must_use]
    pub fn view(&self) -> FindView {
        let field = lock(self.field);
        let (open, text, note) = (field.open, field.text.clone(), field.note.clone());
        drop(field);
        let mut out = FindView {
            open,
            text,
            placeholder: "Find in this document".into(),
            status: String::new(),
            version: 0,
            row_count: 0,
            current_row: None,
            running: false,
            close_tooltip: "Close Find (Esc)".into(),
        };
        self.session.finder().with_results(&mut |results| {
            out.status = match (&note, open) {
                (Some(note), true) => note.clone(),
                (None, true) => results.status(),
                _ => String::new(),
            };
            out.version = results.version;
            out.running = results.running();
            if open {
                let rows = results.rows();
                out.row_count = u32::try_from(rows.len()).unwrap_or(u32::MAX);
                out.current_row = results
                    .current
                    .and_then(|current| rows.iter().position(|row| *row == Row::Hit(current)))
                    .and_then(|at| u32::try_from(at).ok());
            }
        });
        out
    }

    /// Some of the list's rows, for what's on screen.
    #[must_use]
    pub fn rows(&self, first: u32, count: u32) -> Vec<FindRow> {
        let mut out = Vec::new();
        self.session.finder().with_results(&mut |results| {
            let rows = results.rows();
            let end = (first as usize).saturating_add(count.min(MAX_ROWS_FETCH) as usize).min(rows.len());
            out = rows
                .get(first as usize..end)
                .unwrap_or(&[])
                .iter()
                .map(|row| match *row {
                    Row::Page(page) => FindRow {
                        kind: FindRowKind::Page,
                        label: find::page_label(page),
                        matched: String::new(),
                        after: String::new(),
                        current: false,
                        note: String::new(),
                    },
                    Row::Hit(index) => {
                        let hit = &results.hits[index];
                        FindRow {
                            kind: FindRowKind::Hit,
                            label: hit.before.to_string(),
                            matched: hit.matched.to_string(),
                            after: hit.after.to_string(),
                            current: results.current == Some(index),
                            note: if hit.hidden { HIDDEN_NOTE.to_string() } else { String::new() },
                        }
                    }
                })
                .collect();
        });
        out
    }

    /// The hit a row is, if it's a hit's: what a click on it goes to, by the hit's index.
    #[must_use]
    pub fn hit_of_row(&self, row: u32) -> Option<usize> {
        let mut hit = None;
        self.session.finder().with_results(&mut |results| {
            if let Some(Row::Hit(index)) = results.rows().get(row as usize) {
                hit = Some(*index);
            }
        });
        hit
    }

    /// Tries a step, or keeps it for when a hit for it is known: a query still running may not have reached it.
    fn try_step(&self, forward: bool, going: Going, events: &mut Vec<Event>) {
        let session = self.session;
        match session.find_step(forward, going) {
            Some((tab, hit)) => {
                lock(self.field).pending_step = None;
                self.went(tab, &hit, events);
            }
            None => lock(self.field).pending_step = session.finder().is_running().then_some((forward, going)),
        }
    }

    /// What a command asks of the window: a hit selected on the page, or the person's step kept for when its hit is known, which the Finder's news tries
    /// again. Announces nothing: what watchers hear of a hit gone to is said where it's gone to.
    pub fn carry_out(&self, effect: recto_agent::Effect) {
        match effect {
            // Asked by the command that went to the hit, and carried out after it. A step made since, such as an agent's `back` straight after its
            // `find --next --jump`, is the newer: the hit is brought into view only while the tab is still on its page, or the page view would be taken
            // back to the hit from where the session has it.
            recto_agent::Effect::Reveal { tab, hit } => {
                if self.session.with_session(|s| s.tab(tab).is_some_and(|shown| shown.page == hit.page)) {
                    self.reveal(tab.0, &hit);
                }
            }
            recto_agent::Effect::FindStepPending { forward, going } => lock(self.field).pending_step = Some((forward, going)),
        }
    }

    /// Does what the queue holds for Find, on the session thread.
    pub fn act(&self, call: FindCall, events: &mut Vec<Event>) {
        let session = self.session;
        match call {
            FindCall::News => {
                // Taken out first: a guard in the `if let` would be held through the step, which locks the field again, and the session thread would wait on itself. A step waiting for hits waits on through a hand-off's freeze, as it would move the page after what comes back was written.
                let pending = lock(self.field).pending_step;
                if let Some((forward, going)) = pending
                    && !self.frozen.load(Ordering::SeqCst)
                {
                    self.try_step(forward, going, events);
                }
                // The hit gone to before a restart, chosen again by its place once the hits are back, if they're of the same tab. Its selection came back with the page view.
                if !session.finder().is_running()
                    && let (Some(place), tab) = session.with_session(|s| (s.find().current, s.find().current_tab))
                {
                    session.finder().with_results(&mut |results| {
                        if results.current.is_none() && results.end.is_some() && results.tab.is_some() && results.tab == tab {
                            results.choose_place(place);
                        }
                    });
                }
            }
            // What watchers hear of it is the command's, which said it as it went.
            FindCall::Effect(effect) => self.carry_out(effect),
        }
        self.keep_up();
        self.listener.model_changed();
    }

    /// Selects a hit on the page and brings it into view, in the pane being read if it's that tab's. Announces nothing: what watchers hear of a hit gone to
    /// is said where it's gone to.
    fn reveal(&self, tab: u32, hit: &find::Hit) {
        let engine = self.panes.reading_engine(self.engine);
        if engine.hot_tab() == Some(tab) {
            engine.reveal(hit.page, hit.start, hit.end);
        }
    }

    /// The person's step or click went to a hit: selected on the page, and heard.
    fn went(&self, tab: u32, hit: &find::Hit, events: &mut Vec<Event>) {
        self.reveal(tab, hit);
        self.session.with_session(|session| find::heard_going_to(session, tab, hit, events));
    }

    /// Keeps Find with the document being read, and the field's mirror with the session. Called on the session thread after every change. A query in the field is found again when its hits are about something else: another document being read (after a tap, a move into another pane or a restart), pages moved, added or taken away, the file replaced, or the page count learnt.
    pub fn keep_up(&self) {
        let session = self.session;
        let (record, now) = session.with_session(|s| (s.find().clone(), s.active().map(|id| (id, find::basis(s, id)))));
        let moved = {
            let mut field = lock(self.field);
            let moved = field.open != record.open || field.text != record.draft;
            if moved {
                field.open = record.open;
                field.text.clone_from(&record.draft);
            }
            moved
        };
        // The host is told, as Find's view isn't in what `reconcile` compares: a draft or an open that came from the record, as from an agent or a restore, would otherwise wait for something else to change.
        if moved {
            self.listener.model_changed();
        }
        if !record.open || record.query.is_empty() {
            return;
        }
        let mut had = (None, String::new());
        session.finder().with_results(&mut |results| had = (results.basis, results.query.clone()));
        if let Some((active, basis)) = now
            && (had.0 != Some(basis) || had.1 != record.query)
            && let Some(needle) = find::needle(&record.query)
            && let Some(plan) = self.plan_or_say_why(active, basis, &record.query)
        {
            // The person's own Find, with their passwords, so a document they opened with one is searched through its key, page by page, and nothing of it goes to a corpus.
            let plan = match session.find_keys() {
                Some(keys) => plan.with_keys(keys),
                None => plan,
            };
            // The hit gone to was a place in hits about something else, which may be another word now. After a restart there were none, and it's kept to be chosen again.
            if had.0.is_some() {
                session.with_session(|s| s.set_find_current(None, None));
            }
            // The field stays as it is: a draft typed and not run is the reader's.
            session.finder().start(plan, &record.query, needle);
        }
    }

    /// The plan to find again on, or, where Find can't search what the hits would be about, none: the hits held go, as they're about something else, and the line
    /// under the field says why, once, as it's kept against that basis.
    fn plan_or_say_why(&self, active: recto_session::TabId, basis: find::Basis, query: &str) -> Option<find::FindPlan> {
        let refused = match self.session.with_session(|s| find::plan(s, active)) {
            Ok(plan) => return Some(plan),
            Err(refused) => refused,
        };
        if basis.edits_dont_fit {
            let finder = self.session.finder();
            finder.clear();
            finder.with_results(&mut |results| results.refused(active.0, query, basis, refused.message.clone()));
            self.session.with_session(|s| s.set_find_current(None, None));
            self.listener.model_changed();
        }
        None
    }

    /// What QA reads of Find.
    #[must_use]
    pub fn report(&self) -> Value {
        let view = self.view();
        let finder = self.session.finder();
        let mut hits = 0;
        finder.with_results(&mut |results| hits = results.hits.len());
        json!({ "open": view.open, "untrusted": { "text": view.text }, "status": view.status, "rows": view.row_count, "current_row": view.current_row, "running": view.running, "hits_held": hits, "thread": finder.has_thread() })
    }
}

#[cfg(test)]
mod tests;
