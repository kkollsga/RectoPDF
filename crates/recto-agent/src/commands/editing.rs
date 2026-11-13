//! Changing pages: `page rotate`, `page move`, `page delete`, `page insert`, `edits`, `discard`, `undo`, `redo` and `clear-history`. Saving is `saving.rs`'s.

#![allow(unused_imports)]
use super::{Cmd, Cx, row_access};
use crate::human::*;
use crate::table::*;
use crate::*;
use serde::Deserialize;
use serde_json::{Value, json};
use std::fmt::Write;
use std::path::PathBuf;

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PageMove {
    #[serde(default = "crate::active_tab")]
    pub tab: String,
    pub pages: String,
    pub to: u32,
}

impl Cmd for PageMove {
    const TAG: &'static str = "page_move";
    const ROWS: &'static [Spec] = ROWS_PAGE_MOVE;
    const DRAWS: bool = false;
    const READS_PAGES: bool = false;
    const PROTECTED_OK: bool = false;
    const WAITS: bool = false;
    fn tab(&self) -> Option<&str> {
        Some(&self.tab)
    }
    fn access(&self) -> Access {
        row_access(Self::TAG)
    }
    fn route(&self) -> Runs {
        Runs::Session
    }
    fn log_details(&self) -> Value {
        let Self { tab, pages, to } = self;
        json!({ "tab": super::log_tab(tab), "pages": crate::clean_range(pages), "to": to })
    }
    fn run<A: SessionAccess>(&self, cx: &mut Cx<'_, A>) -> Result<Value, AgentError> {
        let Self { tab, pages, to } = self;
        let (access, actor, events) = (cx.access, cx.actor, &mut *cx.events);
        {
            let (start, len) = parse_pages(pages, false)?;
            let destination = one_based(*to)?;
            let did = if start == destination {
                format!("{} already at {to}, so nothing changed.", which_pages(pages, "is", "are"))
            } else {
                format!("Moved {} to {to}.", which_pages(pages, "", ""))
            };
            edit(access, actor, tab, events, did, |e| e.move_pages(start, len, destination))
        }
    }
    fn human(data: &Value) -> String {
        edit_state(data)
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PageDelete {
    #[serde(default = "crate::active_tab")]
    pub tab: String,
    pub pages: String,
}

impl Cmd for PageDelete {
    const TAG: &'static str = "page_delete";
    const ROWS: &'static [Spec] = ROWS_PAGE_DELETE;
    const DRAWS: bool = false;
    const READS_PAGES: bool = false;
    const PROTECTED_OK: bool = false;
    const WAITS: bool = false;
    fn tab(&self) -> Option<&str> {
        Some(&self.tab)
    }
    fn access(&self) -> Access {
        row_access(Self::TAG)
    }
    fn route(&self) -> Runs {
        Runs::Session
    }
    fn log_details(&self) -> Value {
        let Self { tab, pages } = self;
        json!({ "tab": super::log_tab(tab), "pages": crate::clean_range(pages) })
    }
    fn run<A: SessionAccess>(&self, cx: &mut Cx<'_, A>) -> Result<Value, AgentError> {
        let Self { tab, pages } = self;
        let (access, actor, events) = (cx.access, cx.actor, &mut *cx.events);
        {
            let (start, len) = parse_pages(pages, false)?;
            edit(access, actor, tab, events, format!("Deleted {}.", which_pages(pages, "", "")), |e| {
                e.delete(start, len)
            })
        }
    }
    fn human(data: &Value) -> String {
        edit_state(data)
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PageInsert {
    #[serde(default = "crate::active_tab")]
    pub tab: String,
    pub path: PathBuf,
    pub at: u32,
}

impl Cmd for PageInsert {
    const TAG: &'static str = "page_insert";
    const ROWS: &'static [Spec] = ROWS_PAGE_INSERT;
    const DRAWS: bool = false;
    const READS_PAGES: bool = false;
    const PROTECTED_OK: bool = false;
    const WAITS: bool = true;
    fn tab(&self) -> Option<&str> {
        Some(&self.tab)
    }
    fn access(&self) -> Access {
        row_access(Self::TAG)
    }
    fn route(&self) -> Runs {
        Runs::Session
    }
    fn log_details(&self) -> Value {
        let Self { tab, path, at } = self;
        json!({ "tab": super::log_tab(tab), "at": at, "untrusted": super::log_file(path) })
    }
    fn run<A: SessionAccess>(&self, cx: &mut Cx<'_, A>) -> Result<Value, AgentError> {
        let Self { tab, path, at } = self;
        let (access, actor, events) = (cx.access, cx.actor, &mut *cx.events);
        {
            require_absolute(path)?;
            let position = one_based(*at)?;
            // The file is read first and outside the session, so a slow one holds nobody else up.
            let loader = access.with(|s| s.loader().clone());
            let stamp = recto_platform::file_stamp(path).ok();
            let facts = loader.load(path)?;
            access.with(|s| {
                let id = select(s, tab)?;
                keep_owners_undone_steps(s, id, actor.level())?;
                note_agent_at_full(s, actor);
                s.insert_pdf_with_facts(id, actor.author(), position, path, &facts, stamp)?;
                events.push(edits_changed(s, id));
                let mut data = edit_state_json(s, id);
                data["did"] = json!(format!(
                    "Added {} at page {at}.",
                    if facts.page_count == 1 {
                        "1 page".to_string()
                    } else {
                        format!("{} pages", facts.page_count)
                    }
                ));
                Ok(with_place_note(s, id, data))
            })
        }
    }
    fn human(data: &Value) -> String {
        edit_state(data)
    }
}

/// The unsaved page edits on a tab.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Edits {
    #[serde(default = "crate::active_tab")]
    pub tab: String,
}

impl Cmd for Edits {
    const TAG: &'static str = "edits";
    const ROWS: &'static [Spec] = ROWS_EDITS;
    const DRAWS: bool = false;
    const READS_PAGES: bool = false;
    const PROTECTED_OK: bool = false;
    const WAITS: bool = false;
    fn tab(&self) -> Option<&str> {
        Some(&self.tab)
    }
    fn access(&self) -> Access {
        row_access(Self::TAG)
    }
    fn route(&self) -> Runs {
        Runs::Session
    }
    fn log_details(&self) -> Value {
        let Self { tab } = self;
        json!({ "tab": super::log_tab(tab) })
    }
    fn run<A: SessionAccess>(&self, cx: &mut Cx<'_, A>) -> Result<Value, AgentError> {
        let Self { tab } = self;
        let access = cx.access;
        access.with(|s| {
            let id = select(s, tab)?;
            Ok(edits_json(s, id))
        })
    }
    fn human(data: &Value) -> String {
        edits_words(data)
    }
}

/// Throws away a tab's edits. They can't be got back, so below Full it refuses if any are the user's.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Discard {
    #[serde(default = "crate::active_tab")]
    pub tab: String,
}

impl Cmd for Discard {
    const TAG: &'static str = "discard";
    const ROWS: &'static [Spec] = ROWS_DISCARD;
    const DRAWS: bool = false;
    const READS_PAGES: bool = false;
    const PROTECTED_OK: bool = false;
    const WAITS: bool = false;
    fn tab(&self) -> Option<&str> {
        Some(&self.tab)
    }
    fn access(&self) -> Access {
        row_access(Self::TAG)
    }
    fn route(&self) -> Runs {
        Runs::Session
    }
    fn log_details(&self) -> Value {
        let Self { tab } = self;
        json!({ "tab": super::log_tab(tab) })
    }
    fn run<A: SessionAccess>(&self, cx: &mut Cx<'_, A>) -> Result<Value, AgentError> {
        let Self { tab } = self;
        let (access, level, events) = (cx.access, cx.level, &mut *cx.events);
        access.with(|s| {
            let id = select(s, tab)?;
            refuse_discard_of_owners_edits(s, id, level)?;
            let had_any = s.page_edits(id).is_some();
            s.discard_edits(id);
            events.push(edits_changed(s, id));
            let mut data = edit_state_json(s, id);
            data["did"] = json!(if had_any {
                "Threw away this tab's edits."
            } else {
                "There were no edits to throw away."
            });
            Ok(with_place_note(s, id, data))
        })
    }
    fn human(data: &Value) -> String {
        edit_state(data)
    }
}

pub(crate) const ROWS_PAGE_MOVE: &[Spec] = &[Spec {
    name: "page move",
    tag: "page_move",
    group: GROUP_EDIT,
    access: Access::At(Level::Control),
    summary: "Moves pages, so the first of them ends up at `--to`.",
    args: &[
        Arg {
            key: "pages",
            flag: None,
            kind: Kind::Pages,
            required: true,
            help: "Which: `4` or `4-6`.",
        },
        Arg {
            key: "to",
            flag: Some("--to"),
            kind: Kind::Page,
            required: true,
            help: "Where the first of them lands, counted from 1 in the new order.",
        },
        TAB,
    ],
    data: EDIT_STATE,
    emits: &["edits_changed"],
    examples: &["recto page move 3 --to 1", "recto page move 4-6 --to 10 --tab report"],
    ..BASE
}];

pub(crate) const ROWS_PAGE_DELETE: &[Spec] = &[Spec {
    name: "page delete",
    tag: "page_delete",
    group: GROUP_EDIT,
    access: Access::At(Level::Control),
    summary: "Removes pages from the edited document. A document keeps at least one page. Below Full, an edit that would clear undone steps of the user's is refused.",
    args: &[
        Arg {
            key: "pages",
            flag: None,
            kind: Kind::Pages,
            required: true,
            help: "Which: `4` or `4-6`.",
        },
        TAB,
    ],
    data: EDIT_STATE,
    emits: &["edits_changed"],
    examples: &["recto page delete 5", "recto page delete 2-3 --tab report"],
    ..BASE
}];

pub(crate) const ROWS_PAGE_INSERT: &[Spec] = &[Spec {
    name: "page insert",
    tag: "page_insert",
    group: GROUP_EDIT,
    access: Access::At(Level::Control),
    summary: "Adds every page of another PDF.",
    args: &[
        Arg {
            key: "path",
            flag: None,
            kind: Kind::Path,
            required: true,
            help: "The PDF to add.",
        },
        Arg {
            key: "at",
            flag: Some("--at"),
            kind: Kind::Page,
            required: true,
            help: "Where its first page lands, counted from 1. One past the last page adds to the end.",
        },
        TAB,
    ],
    data: EDIT_STATE,
    emits: &["edits_changed"],
    examples: &["recto page insert appendix.pdf --at 4"],
    ..BASE
}];

pub(crate) const ROWS_EDITS: &[Spec] = &[Spec {
    name: "edits",
    tag: "edits",
    group: GROUP_EDIT,
    summary: "A tab's unsaved edits, as runs of pages.",
    args: &[TAB],
    data: "`{ ...same as undo, runs: [ { pages, from_file_pages, rotation_degrees, added, untrusted: { added_file } } ], runs_truncated }`.",
    examples: &["recto edits", "recto edits --json"],
    ..BASE
}];

pub(crate) const ROWS_DISCARD: &[Spec] = &[Spec {
    name: "discard",
    tag: "discard",
    group: GROUP_EDIT,
    access: Access::At(Level::Control),
    summary: "Throws away a tab's edits, undone ones included. They can't be got back. Below Full it's refused, and throws nothing away, if any are the user's.",
    args: &[TAB],
    data: EDIT_STATE,
    emits: &["edits_changed"],
    examples: &["recto discard --tab report"],
    ..BASE
}];

fn edits_words(data: &Value) -> String {
    let mut out = edit_state(data);
    for run in data["runs"].as_array().map_or(&[][..], Vec::as_slice) {
        let _ = write!(out, "\n  pages {}: ", text(&run["pages"]));
        match run["untrusted"]["added_file"].as_str() {
            Some(file) => {
                let _ = write!(out, "added from {file:?}, its pages {}", text(&run["from_file_pages"]));
            }
            None => {
                let _ = write!(out, "this file's pages {}", text(&run["from_file_pages"]));
            }
        }
        let degrees = number(&run["rotation_degrees"]);
        if degrees != 0 {
            let _ = write!(out, ", turned {degrees} degrees");
        }
    }
    out
}

/// Turns pages clockwise. `all` turns every page as one step, as the panel's Rotate does outside edit mode.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PageRotate {
    #[serde(default = "crate::active_tab")]
    pub tab: String,
    pub pages: String,
    pub degrees: i32,
}

impl Cmd for PageRotate {
    const TAG: &'static str = "page_rotate";
    const ROWS: &'static [Spec] = ROWS_PAGE_ROTATE;
    const DRAWS: bool = false;
    const READS_PAGES: bool = false;
    const PROTECTED_OK: bool = false;
    const WAITS: bool = false;
    fn tab(&self) -> Option<&str> {
        Some(&self.tab)
    }
    fn access(&self) -> Access {
        row_access(Self::TAG)
    }
    fn route(&self) -> Runs {
        Runs::Session
    }
    fn log_details(&self) -> Value {
        let Self { tab, pages, degrees } = self;
        json!({ "tab": super::log_tab(tab), "pages": crate::clean_range(pages), "degrees": degrees })
    }
    fn run<A: SessionAccess>(&self, cx: &mut Cx<'_, A>) -> Result<Value, AgentError> {
        let Self { tab, pages, degrees } = self;
        let (access, actor, events) = (cx.access, cx.actor, &mut *cx.events);
        let turns = quarter_turns(*degrees)?;
        // `all` turns every page as one step, as the panel's Rotate does outside edit mode.
        if pages == "all" {
            return edit(access, actor, tab, events, format!("Rotated every page by {degrees} degrees."), |e| {
                e.rotate(0, e.len(), turns)
            });
        }
        let (start, len) = parse_pages(pages, false)?;
        edit(
            access,
            actor,
            tab,
            events,
            format!("Rotated {} by {degrees} degrees.", which_pages(pages, "", "")),
            |e| e.rotate(start, len, turns),
        )
    }
    fn human(data: &Value) -> String {
        edit_state(data)
    }
}

/// Reverses the last page edit.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Undo {
    #[serde(default = "crate::active_tab")]
    pub tab: String,
}

impl Cmd for Undo {
    const TAG: &'static str = "undo";
    const ROWS: &'static [Spec] = ROWS_UNDO;
    const DRAWS: bool = false;
    const READS_PAGES: bool = false;
    const PROTECTED_OK: bool = false;
    const WAITS: bool = false;
    fn tab(&self) -> Option<&str> {
        Some(&self.tab)
    }
    fn access(&self) -> Access {
        row_access(Self::TAG)
    }
    fn route(&self) -> Runs {
        Runs::Session
    }
    fn log_details(&self) -> Value {
        json!({ "tab": super::log_tab(&self.tab) })
    }
    fn run<A: SessionAccess>(&self, cx: &mut Cx<'_, A>) -> Result<Value, AgentError> {
        let (actor, events) = (cx.actor, &mut *cx.events);
        cx.access.with(|s| {
            let id = select(s, &self.tab)?;
            note_agent_at_full(s, actor);
            if !s.undo_edit_as(id, actor.author())? {
                return Err(AgentError::new("nothing_to_undo", "There's nothing to undo on this tab.", exit::FAILED));
            }
            events.push(edits_changed(s, id));
            let mut data = edit_state_json(s, id);
            data["did"] = json!("Undid the last edit.");
            Ok(with_place_note(s, id, data))
        })
    }
    fn human(data: &Value) -> String {
        edit_state(data)
    }
}

/// Repeats the last undone page edit.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Redo {
    #[serde(default = "crate::active_tab")]
    pub tab: String,
}

impl Cmd for Redo {
    const TAG: &'static str = "redo";
    const ROWS: &'static [Spec] = ROWS_REDO;
    const DRAWS: bool = false;
    const READS_PAGES: bool = false;
    const PROTECTED_OK: bool = false;
    const WAITS: bool = false;
    fn tab(&self) -> Option<&str> {
        Some(&self.tab)
    }
    fn access(&self) -> Access {
        row_access(Self::TAG)
    }
    fn route(&self) -> Runs {
        Runs::Session
    }
    fn log_details(&self) -> Value {
        json!({ "tab": super::log_tab(&self.tab) })
    }
    fn run<A: SessionAccess>(&self, cx: &mut Cx<'_, A>) -> Result<Value, AgentError> {
        let (actor, events) = (cx.actor, &mut *cx.events);
        cx.access.with(|s| {
            let id = select(s, &self.tab)?;
            note_agent_at_full(s, actor);
            if !s.redo_edit_as(id, actor.author())? {
                return Err(AgentError::new("nothing_to_redo", "There's nothing to redo on this tab.", exit::FAILED));
            }
            events.push(edits_changed(s, id));
            let mut data = edit_state_json(s, id);
            data["did"] = json!("Redid the last undone edit.");
            Ok(with_place_note(s, id, data))
        })
    }
    fn human(data: &Value) -> String {
        edit_state(data)
    }
}

/// Clears a tab's edit history: every step undo or redo could take, and its file. Unsaved edits stay. Below Full it refuses if any are the user's.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClearHistory {
    #[serde(default = "crate::active_tab")]
    pub tab: String,
}

impl Cmd for ClearHistory {
    const TAG: &'static str = "clear_history";
    const ROWS: &'static [Spec] = ROWS_CLEAR_HISTORY;
    const DRAWS: bool = false;
    const READS_PAGES: bool = false;
    const PROTECTED_OK: bool = false;
    const WAITS: bool = false;
    fn tab(&self) -> Option<&str> {
        Some(&self.tab)
    }
    fn access(&self) -> Access {
        row_access(Self::TAG)
    }
    fn route(&self) -> Runs {
        Runs::Session
    }
    fn log_details(&self) -> Value {
        json!({ "tab": super::log_tab(&self.tab) })
    }
    fn run<A: SessionAccess>(&self, cx: &mut Cx<'_, A>) -> Result<Value, AgentError> {
        let (level, events) = (cx.level, &mut *cx.events);
        cx.access.with(|s| {
            let id = select(s, &self.tab)?;
            refuse_discard_of_owners_edits(s, id, level)?;
            let had_any = s.page_edits(id).is_some_and(|list| list.can_undo() || list.can_redo());
            s.clear_history(id, recto_session::ClearReason::UserAsked);
            // Announced when there was history to clear, and only then (R2.3, by A2's rule).
            if had_any {
                events.push(edits_changed(s, id));
            }
            let mut data = edit_state_json(s, id);
            data["did"] = json!(if had_any {
                "Cleared this tab's edit history. Unsaved edits are still here, and can't be undone now."
            } else {
                "There was no edit history to clear."
            });
            Ok(with_place_note(s, id, data))
        })
    }
    fn human(data: &Value) -> String {
        edit_state(data)
    }
}

pub(crate) const ROWS_PAGE_ROTATE: &[Spec] = &[Spec {
    name: "page rotate",
    tag: "page_rotate",
    group: GROUP_EDIT,
    access: Access::At(Level::Control),
    summary: "Turns pages clockwise. `all` turns every page as one step, as Rotate in the sidebar does outside edit mode.",
    args: &[
        Arg {
            key: "pages",
            flag: None,
            kind: Kind::Pages,
            required: true,
            help: "Which: `4`, `4-6`, or `all`.",
        },
        Arg {
            key: "degrees",
            flag: Some("--by"),
            kind: Kind::Degrees,
            required: true,
            help: "`90`, `180`, `270`, or `-90` for the other way.",
        },
        TAB,
    ],
    data: EDIT_STATE,
    emits: &["edits_changed"],
    examples: &[
        "recto page rotate 2 --by 90",
        "recto page rotate 1-4 --by -90 --json",
        "recto page rotate all --by 90",
    ],
    ..BASE
}];

pub(crate) const ROWS_UNDO: &[Spec] = &[Spec {
    name: "undo",
    tag: "undo",
    group: GROUP_EDIT,
    access: Access::At(Level::Control),
    summary: "Reverses the last page edit. It can be redone until a new edit clears it. Below Full, a new edit may clear only agents' undone steps: if the user's are among them it's refused until you've redone them.",
    args: &[TAB],
    data: EDIT_STATE,
    emits: &["edits_changed"],
    examples: &["recto undo"],
    ..BASE
}];

pub(crate) const ROWS_REDO: &[Spec] = &[Spec {
    name: "redo",
    tag: "redo",
    group: GROUP_EDIT,
    access: Access::At(Level::Control),
    summary: "Repeats the last undone page edit.",
    args: &[TAB],
    data: EDIT_STATE,
    emits: &["edits_changed"],
    examples: &["recto redo"],
    ..BASE
}];

pub(crate) const ROWS_CLEAR_HISTORY: &[Spec] = &[Spec {
    name: "clear-history",
    tag: "clear_history",
    group: GROUP_EDIT,
    access: Access::At(Level::Control),
    summary: "Clears a tab's edit history: every step undo or redo could take, and its file on disk. Unsaved edits stay. Below Full it's refused, and clears nothing, if any steps are the user's.",
    args: &[TAB],
    data: EDIT_STATE,
    emits: &["edits_changed"],
    examples: &["recto clear-history --tab report"],
    ..BASE
}];
