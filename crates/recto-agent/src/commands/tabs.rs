//! The list of tabs and files: `open`, `tap`, `recent`, `recent clear` and `close`.

#![allow(unused_imports)]
use super::{Cmd, Cx, row_access};
use crate::human::*;
use crate::table::*;
use crate::*;
use serde::Deserialize;
use serde_json::{Value, json};
use std::fmt::Write;
use std::path::PathBuf;

/// Opens a file in a tab, or goes to its tab if it's open already.
#[derive(Clone, Debug, Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub struct Open {
    pub path: PathBuf,
    /// One-based page to land on.
    #[serde(default)]
    pub page: Option<u32>,
}

impl Cmd for Open {
    const TAG: &'static str = "open";
    const ROWS: &'static [Spec] = ROWS_OPEN;
    const DRAWS: bool = false;
    const READS_PAGES: bool = false;
    const PROTECTED_OK: bool = false;
    const WAITS: bool = false;
    fn tab(&self) -> Option<&str> {
        None
    }
    fn access(&self) -> Access {
        row_access(Self::TAG)
    }
    fn route(&self) -> Runs {
        Runs::Session
    }
    fn log_details(&self) -> Value {
        json!({ "page": self.page, "untrusted": super::log_file(&self.path) })
    }
    fn run<A: SessionAccess>(&self, cx: &mut Cx<'_, A>) -> Result<Value, AgentError> {
        open(cx.access, cx.actor, &self.path, self.page, cx.events)
    }
    fn human(data: &Value) -> String {
        place(data)
    }
}

/// Brings a tab to the front, as a click on its row does.
#[derive(Clone, Debug, Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub struct Tap {
    #[serde(default = "crate::active_tab")]
    pub tab: String,
}

impl Cmd for Tap {
    const TAG: &'static str = "tap";
    const ROWS: &'static [Spec] = ROWS_TAP;
    const DRAWS: bool = false;
    const READS_PAGES: bool = false;
    const PROTECTED_OK: bool = true;
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
        let (access, events) = (cx.access, &mut *cx.events);
        let id = access.with(|s| select(s, &self.tab))?;
        read_ahead(access, |s| s.wake_for_tap(id))?;
        access.with(|s| {
            let id = select(s, &self.tab)?;
            let switched = s.tap(id)?;
            event::switched(s, switched, events);
            // The sidebar's every row, which the window's own tap would build for nobody: SessionAccess::answer_read.
            Ok(if access.answer_read() { sidebar_json(s) } else { Value::Null })
        })
    }
    fn human(data: &Value) -> String {
        sidebar(data)
    }
}

/// Files opened lately, as File > Open Recent lists them.
#[derive(Clone, Debug, Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub struct Recent {}

impl Cmd for Recent {
    const TAG: &'static str = "recent";
    const ROWS: &'static [Spec] = ROWS_RECENT;
    const DRAWS: bool = false;
    const READS_PAGES: bool = false;
    const PROTECTED_OK: bool = false;
    const WAITS: bool = false;
    fn tab(&self) -> Option<&str> {
        None
    }
    fn access(&self) -> Access {
        row_access(Self::TAG)
    }
    fn route(&self) -> Runs {
        Runs::Session
    }
    fn log_details(&self) -> Value {
        Value::Null
    }
    fn run<A: SessionAccess>(&self, cx: &mut Cx<'_, A>) -> Result<Value, AgentError> {
        let (access, level) = (cx.access, cx.level);
        Ok(access.with(|s| recent_json(s, level)))
    }
    fn human(data: &Value) -> String {
        match data["files"].as_array().filter(|files| !files.is_empty()) {
            Some(files) => {
                let mut out = format!("{} opened lately, newest first:", count(files.len() as u64, "file", "files"));
                for file in files {
                    let open = file["open_tab"].as_str().map(|tab| format!(", open as {tab}")).unwrap_or_default();
                    let path = file["untrusted"]["path"].as_str().map(|path| format!(" at {path:?}")).unwrap_or_default();
                    let _ = write!(out, "\n{}. {:?}{path}{open}", number(&file["file"]), text(&file["untrusted"]["name"]));
                }
                let _ = write!(out, "\n{QUOTED_NAMES}");
                out
            }
            None => "No files opened lately.".to_string(),
        }
    }
}

/// Clears that list, as Open Recent > Clear Menu does.
#[derive(Clone, Debug, Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub struct RecentClear {}

impl Cmd for RecentClear {
    const TAG: &'static str = "recent_clear";
    const ROWS: &'static [Spec] = ROWS_RECENT_CLEAR;
    const DRAWS: bool = false;
    const READS_PAGES: bool = false;
    const PROTECTED_OK: bool = false;
    const WAITS: bool = false;
    fn tab(&self) -> Option<&str> {
        None
    }
    fn access(&self) -> Access {
        row_access(Self::TAG)
    }
    fn route(&self) -> Runs {
        Runs::Session
    }
    fn log_details(&self) -> Value {
        Value::Null
    }
    fn run<A: SessionAccess>(&self, cx: &mut Cx<'_, A>) -> Result<Value, AgentError> {
        let access = cx.access;
        access.with(|s| {
            let cleared = s.recent_mut().clear();
            Ok(json!({ "cleared": cleared }))
        })
    }
    fn human(data: &Value) -> String {
        if data["cleared"] == true {
            "Cleared the list of files opened lately."
        } else {
            "There were no files opened lately to clear."
        }
        .to_string()
    }
}

#[derive(Clone, Debug, Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub struct Close {
    #[serde(default = "crate::active_tab")]
    pub tab: String,
}

impl Cmd for Close {
    const TAG: &'static str = "close";
    const ROWS: &'static [Spec] = ROWS_CLOSE;
    const DRAWS: bool = false;
    const READS_PAGES: bool = false;
    const PROTECTED_OK: bool = true;
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
        let (access, actor, level, events) = (cx.access, cx.actor, cx.level, &mut *cx.events);
        // The close rule, said once: what a close would lose, and what this actor does about it.
        let refused = |s: &Session<A::Loader>, id| crate::close_rule::WouldLose::of(s, id).refusal(actor).map_or(Ok(()), Err);
        {
            // What would refuse it comes first: no reading a file for a close that won't happen.
            let id = access.with(|s| {
                let id = select(s, tab)?;
                refused(s, id)?;
                Ok::<_, AgentError>(id)
            })?;
            read_soon(access, |s| s.wake_for_close(id));
            access.with(|s| {
                let id = select(s, tab)?;
                refused(s, id)?;
                // At Full, what's lost is the holder's call, and the close rule has let it through. The session itself won't lose the owner's undone steps unasked.
                if level >= Level::Full && s.lists().has_undone_owner_steps(id) {
                    s.discard_edits(id);
                }
                let closed = s.close(id)?;
                event::closed(s, closed, events);
                // Not the whole list: closing a thousand tabs shouldn't build a thousand lists.
                Ok(json!({ "closed": id.0, "count": s.tabs().len(), "active": s.tabs().active().map(|active| where_json(s, active)) }))
            })
        }
    }
    fn human(data: &Value) -> String {
        {
            let mut out = format!("Closed #{}. {} open.", number(&data["closed"]), count(number(&data["count"]), "tab", "tabs"));
            if data["active"].is_object() {
                let _ = write!(out, "\nActive: {}\n{QUOTED_NAMES}", tab_line(&data["active"]));
            }
            out
        }
    }
}

pub(crate) const ROWS_RECENT: &[Spec] = &[Spec {
    name: "recent",
    tag: "recent",
    group: GROUP_MOVE,
    access: Access::At(Level::Control),
    summary: "Files opened lately, newest first, as File > Open Recent lists them, with the tab each is open in and its whole path, which `recto open` can open. Needs Control: what someone's been reading, closed files included, is theirs, and more than Read-only's look at what's open.",
    data: "`{ count, files: [ { file, open_tab, untrusted: { name, path } } ], paths_shown }`. At most 20.",
    examples: &["recto recent", "recto recent --json"],
    ..BASE
}];

pub(crate) const ROWS_RECENT_CLEAR: &[Spec] = &[Spec {
    name: "recent clear",
    tag: "recent_clear",
    group: GROUP_MOVE,
    access: Access::At(Level::Control),
    summary: "Empties the list of files opened lately, as Open Recent > Clear Menu does. Opens and closes nothing.",
    data: "`{ cleared }`: whether there was anything.",
    examples: &["recto recent clear"],
    ..BASE
}];

pub(crate) const ROWS_CLOSE: &[Spec] = &[Spec {
    name: "close",
    tag: "close",
    group: GROUP_MOVE,
    access: Access::At(Level::Control),
    summary: "Closes a tab. Refused while it has unsaved edits, so `recto discard` first, and below Full if undone steps of the user's would go. Undone steps of an agent's own go with it. The refusal says what's at stake and each way on.",
    args: &[TAB],
    data: "`{ closed, count, active }`, with `active` as `where` gives it.",
    emits: &["tab_closed", "tab_activated"],
    examples: &["recto close", "recto close --tab \"#2\""],
    ..BASE
}];

fn open<A: SessionAccess>(access: &A, actor: Actor, path: &Path, page: Option<u32>, events: &mut Vec<Event>) -> Result<Value, AgentError> {
    require_absolute(path)?;
    let wrong_page = |pages: u32| {
        AgentError::new(
            "no_such_page",
            format!("Nothing was opened: it has {pages} pages, and page {} isn't one of them.", page.unwrap_or(0)),
            exit::NOT_FOUND,
        )
        .then("Open it with a page it has, or with no `--page`.")
    };
    let fits = |pages: u32| page.is_none_or(|page| page >= 1 && page <= pages);
    // A host that mustn't wait on a disk has the file looked at and read off its thread, and this runs again once it's in: unless a tab has it by this name, as
    // the window has always gone to that tab without reading the file. Everywhere else it's looked at here.
    let fetched = match access.reads_later() {
        Some(reads) if access.with(|s| s.tabs().for_path(path)).is_none() => Some(reads.load(path).ok_or_else(later::later)?),
        _ => None,
    };
    // Looked at first, so a change during a read shows as a change, and what's the same file can be told. A slow file holds up nobody: the session isn't held.
    let stamp = match (&fetched, access.reads_later()) {
        (Some(fetched), _) => fetched.stamp,
        (None, Some(_)) => None,
        (None, None) => recto_platform::file_stamp(path).ok(),
    };
    // A file already open, in a tab gone cold, is read with the session free too.
    read_ahead(access, |s| s.tab_for_file(path, stamp).and_then(|id| s.wake_for_tap(id)))?;
    // What was in front, so a tab brought there is announced and one that was there already isn't (R2.3, by A2's rule).
    let active_before = access.with(|s| s.tabs().active());
    // Already open, by this name or another: SPEC decision 26. Check the page before anything moves.
    let existing = access.with(|s| {
        let id = s.tab_for_file(path, stamp)?;
        // Whether a page is in a protected document would say how long it is.
        if page.is_some() && !actor.sees_protected() && s.tabs().withheld_from_agents(id) {
            return Some(Err(protected_refusal()));
        }
        let pages = s.pages_in(id);
        if !fits(pages) {
            return Some(Err(wrong_page(pages)));
        }
        s.go_to_file(path, stamp);
        Some(Ok(id))
    });
    let (id, is_new) = match existing {
        Some(found) => (found?, false),
        None => {
            // The file is read outside the session, so a slow or hostile one holds nobody else up.
            let (facts, read_history, unlocked) = match fetched {
                Some(fetched) => (fetched.facts?, Some(fetched.history), fetched.unlocked),
                None => (access.with(|s| s.loader().clone()).load(path)?, None, false),
            };
            // No tab for a page that isn't there: failing here leaves nothing to explain.
            if !fits(facts.page_count) {
                return Err(wrong_page(facts.page_count));
            }
            let before = access.with(|s| s.tabs().len());
            // A closed PDF's history comes back on this route as on any, read with the session free too.
            let history = read_history.unwrap_or_else(|| {
                let source = access.with(|s| {
                    if s.tab_for_file(path, stamp).is_some() {
                        None
                    } else {
                        s.journals().history().cloned()
                    }
                });
                source.and_then(|source| source.history_of(stamp, path))
            });
            let id = access.with(|s| {
                if unlocked {
                    s.open_unlocked(path, facts, stamp)
                } else {
                    s.open_with_facts(path, facts, stamp, history)
                }
            })?;
            (id, access.with(|s| s.tabs().len()) > before)
        }
    };
    access.with(|s| {
        if let Some(page) = page {
            go_to(s, id, page, 0.0, actor.author())?;
        }
        if is_new {
            events.push(event::tab_opened(s, id));
        } else if active_before != Some(id) {
            events.push(event::tab_activated(s, id));
        }
        if page.is_some() {
            events.push(page_now(s, id));
        }
        let mut data = where_json(s, id);
        // A PDF is open once, so what's asked for by any name is this tab, and an agent is told.
        if !is_new {
            data["already_open"] = json!(true);
            data["open_note"] = json!("That PDF was already open, so this is its tab. A link to a file is the same file.");
        }
        Ok(data)
    })
}

pub(crate) const ROWS_OPEN: &[Spec] = &[Spec {
    name: "open",
    tag: "open",
    group: GROUP_MOVE,
    access: Access::At(Level::Control),
    summary: "Opens a PDF as the active tab, or goes to its tab if it's already open. A `--page` it doesn't have opens nothing.",
    args: &[
        Arg {
            key: "path",
            flag: None,
            kind: Kind::Path,
            required: true,
            help: "The PDF.",
        },
        Arg {
            key: "page",
            flag: Some("--page"),
            kind: Kind::Page,
            required: false,
            help: "The page to land on, counted from 1.",
        },
    ],
    data: AGENT_EXITS_PAGE,
    emits: &["tab_opened", "tab_activated", "page_changed"],
    examples: &["recto open report.pdf", "recto open /Users/me/report.pdf --page 12 --json"],
    ..BASE
}];

pub(crate) const ROWS_TAP: &[Spec] = &[Spec {
    name: "tap",
    tag: "tap",
    group: GROUP_MOVE,
    access: Access::At(Level::Control),
    summary: "Taps a tab in the sidebar: switches to it, or folds its contents if it's already active.",
    args: &[Arg {
        key: "tab",
        flag: None,
        kind: Kind::Tab,
        required: true,
        help: "Which tab: `#3`, or part of its name.",
    }],
    data: "The sidebar, as `sidebar` gives it.",
    emits: &["tab_activated"],
    examples: &["recto tap \"#2\"", "recto tap report"],
    ..BASE
}];
