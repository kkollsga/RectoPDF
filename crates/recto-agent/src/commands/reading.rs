//! Reading what a document is and where a tab is: `tabs`, `where`, `facts`, `contents`, `signatures` and `sidebar`.

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
pub struct Tabs {
    /// Tabs to skip, from `next_from` in the last answer.
    #[serde(default)]
    pub from: Option<u32>,
    #[serde(default)]
    pub limit: Option<u32>,
}

impl Cmd for Tabs {
    const TAG: &'static str = "tabs";
    const ROWS: &'static [Spec] = ROWS_TABS;
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
        let Self { from, limit } = self;
        let access = cx.access;
        access.with(|s| tabs_json(s, *from, *limit))
    }
    fn human(data: &Value) -> String {
        tabs(data)
    }
}

/// Where a tab is: its page, its length and whether it has unsaved edits.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Where {
    #[serde(default = "crate::active_tab")]
    pub tab: String,
}

impl Cmd for Where {
    const TAG: &'static str = "where";
    const ROWS: &'static [Spec] = ROWS_WHERE;
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
        let access = cx.access;
        access.with(|s| Ok(where_json(s, select(s, tab)?)))
    }
    fn human(data: &Value) -> String {
        place(data)
    }
}

/// A document's facts: pages, size, version, what could be read of its structure.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Facts {
    #[serde(default = "crate::active_tab")]
    pub tab: String,
}

impl Cmd for Facts {
    const TAG: &'static str = "facts";
    const ROWS: &'static [Spec] = ROWS_FACTS;
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
        reading::facts(access, tab)
    }
    fn human(data: &Value) -> String {
        facts(data)
    }
}

/// A document's contents, in pieces.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Contents {
    #[serde(default = "crate::active_tab")]
    pub tab: String,
    /// Entries to skip, from `next_from` in the last answer.
    #[serde(default)]
    pub from: Option<u32>,
    #[serde(default)]
    pub limit: Option<u32>,
    /// Deepest nesting to include. 0 is the top level.
    #[serde(default)]
    pub depth: Option<u8>,
    /// The ones made from the document's headings, rather than the file's own.
    #[serde(default)]
    pub generated: bool,
}

impl Cmd for Contents {
    const TAG: &'static str = "contents";
    const ROWS: &'static [Spec] = ROWS_CONTENTS;
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
        // Making generated contents starts a helper, which is heavier work than reading the file's own, so the log can tell them apart.
        if self.generated {
            json!({ "tab": super::log_tab(&self.tab), "generated": true })
        } else {
            json!({ "tab": super::log_tab(&self.tab) })
        }
    }
    fn run<A: SessionAccess>(&self, cx: &mut Cx<'_, A>) -> Result<Value, AgentError> {
        let Self {
            tab,
            from,
            limit,
            depth,
            generated,
        } = self;
        let access = cx.access;
        reading::contents(access, tab, *from, *limit, *depth, *generated)
    }
    fn human(data: &Value) -> String {
        contents(data)
    }
}

/// What the file says about signatures. Facts only, never a verdict.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Signatures {
    #[serde(default = "crate::active_tab")]
    pub tab: String,
}

impl Cmd for Signatures {
    const TAG: &'static str = "signatures";
    const ROWS: &'static [Spec] = ROWS_SIGNATURES;
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
        let Self { tab } = self;
        json!({ "tab": super::log_tab(tab) })
    }
    fn run<A: SessionAccess>(&self, cx: &mut Cx<'_, A>) -> Result<Value, AgentError> {
        let Self { tab } = self;
        let access = cx.access;
        reading::signatures(access, tab)
    }
    fn human(data: &Value) -> String {
        signatures(data)
    }
}

// Empty braces, not unit variants: serde ignores extra fields on a unit variant, even with `deny_unknown_fields`.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Sidebar {}

impl Cmd for Sidebar {
    const TAG: &'static str = "sidebar";
    const ROWS: &'static [Spec] = ROWS_SIDEBAR;
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
        Ok(access.with(|s| sidebar_json(s)))
    }
    fn human(data: &Value) -> String {
        sidebar(data)
    }
}

pub(crate) const ROWS_TABS: &[Spec] = &[Spec {
    name: "tabs",
    tag: "tabs",
    group: GROUP_READ,
    summary: "The open tabs, with each one's page, length and unsaved edits.",
    args: &[
        Arg {
            key: "from",
            flag: Some("--from"),
            kind: Kind::Count(100_000),
            required: false,
            help: "Tabs to skip: `next_from` from the last answer.",
        },
        Arg {
            key: "limit",
            flag: Some("--limit"),
            kind: Kind::Positive(1000),
            required: false,
            help: "Tabs in this answer. 200 if left out.",
        },
    ],
    data: "`{ tabs: [ ...same as where ], count, next_from }`.",
    examples: &["recto tabs", "recto tabs --from 200 --json"],
    ..BASE
}];

pub(crate) const ROWS_WHERE: &[Spec] = &[Spec {
    name: "where",
    tag: "where",
    group: GROUP_READ,
    summary: "Where one tab is: its page, its length, and whether it has unsaved edits.",
    args: &[TAB],
    data: AGENT_EXITS_PAGE,
    examples: &["recto where", "recto where --tab \"#2\""],
    ..BASE
}];

pub(crate) const ROWS_FACTS: &[Spec] = &[Spec {
    name: "facts",
    tag: "facts",
    group: GROUP_READ,
    summary: "A document's facts: pages, size, version, and whether it has contents.",
    args: &[TAB],
    data: "`{ tab, pages, file_pages, file_bytes, encrypted, encryption_note, pdf_version, structure, has_contents, contents_entries, edited, note, untrusted: { name, title } }`. `encryption_note` says when an encrypted file opened with no password.",
    examples: &["recto facts", "recto facts --tab report --json"],
    ..BASE
}];

pub(crate) const ROWS_CONTENTS: &[Spec] = &[Spec {
    name: "contents",
    tag: "contents",
    group: GROUP_READ,
    summary: "A document's table of contents, a piece at a time. By default the document's sections and as far down as fits, and it says which depth that is.",
    args: &[
        TAB,
        Arg {
            key: "from",
            flag: Some("--from"),
            kind: Kind::Count(100_000),
            required: false,
            help: "Entries to skip: `next_from` from the last answer.",
        },
        Arg {
            key: "limit",
            flag: Some("--limit"),
            kind: Kind::Positive(2000),
            required: false,
            help: "Entries in this answer. 200 if left out.",
        },
        Arg {
            key: "depth",
            flag: Some("--depth"),
            kind: Kind::Count(32),
            required: false,
            help: "Deepest nesting to include. 0 is the top of the outline, which can be just the document's title: `sections_at` says where its top-level sections are. As much as fits if left out.",
        },
        Arg {
            key: "generated",
            flag: Some("--generated"),
            kind: Kind::Switch,
            required: false,
            help: "Contents made from the headings distillPDF finds on the pages, for a file whose own are missing or look bad, as `verdict` says. Never the file's own, which are never changed. The first ask starts them in the background and answers at once with `state`; `recto wait contents-generated` waits for them.",
        },
    ],
    data: "`{ tab, available, total, all_entries, entries: [ { depth, page, place, untrusted: { title } } ], next_from, truncated, depth_shown, deepest, sections_at, sections, depth_rule, verdict, verdict_note, generated, state, note, hidden_left_out, pages_not_checked, pages_not_read }`. `verdict` is what's thought of the file's own contents: `fine`, `missing`, `entries_go_nowhere`, `all_on_one_page`, `too_few` or `stops_early`, and `verdict_note` says it in words when generated ones are worth asking for. With `--generated`, `generated: true`, `state` is `queued`, `running` or `ready`, `place` is how far down its page each heading is in thousandths, and `note` says where they came from. `hidden_left_out` counts headings left out because their text looked hidden, and `pages_not_checked` pages whose headings were left out because that couldn't be checked. `total` counts the entries at the depth shown, and `all_entries` every entry. The document's top-level sections are the `sections` entries at depth `sections_at`, the first depth whose entries are on more than one page: anything above that is its title. `depth_rule` says all this in words. `available: false` means it couldn't be read, not that there's none. `truncated: true` means the file's contents stopped being read at a limit.",
    examples: &[
        "recto contents",
        "recto contents --depth 1 --limit 50",
        "recto contents --from 200 --json",
        "recto contents --generated",
    ],
    ..BASE
}];

pub(crate) const ROWS_SIGNATURES: &[Spec] = &[Spec {
    name: "signatures",
    tag: "signatures",
    group: GROUP_READ,
    summary: "What the file says about signatures. Facts only: nothing is checked, and nothing here can sign.",
    args: &[TAB],
    data: "`{ tab, structure_read, count, signed, truncated, unread, partial, edited, certification_named, signatures: [ { summary, status, page, file_page, byte_range, bytes_after_range, untrusted: { field, filter, sub_filter, stated_name, stated_reason, stated_location, stated_time } } ], note }`. `status` is `not_checked`, `empty` or `unread`. `structure_read: false` or `partial: true` means there may be more than this.",
    examples: &["recto signatures", "recto signatures --tab contract --json"],
    ..BASE
}];

pub(crate) const ROWS_SIDEBAR: &[Spec] = &[Spec {
    name: "sidebar",
    tag: "sidebar",
    group: GROUP_READ,
    summary: "The sidebar's rows, as the app would draw them.",
    data: "`{ rows: [ { kind, tab, page, depth, current, untrusted: { text } } ], rail_filled, toc_open, place, edited, undone_owner_steps }`.",
    examples: &["recto sidebar --json"],
    ..BASE
}];

fn tabs(data: &Value) -> String {
    let rows = data["tabs"].as_array().map_or(&[][..], Vec::as_slice);
    if rows.is_empty() {
        return "No PDFs are open. `recto open <file.pdf>` opens one.".to_string();
    }
    let mut out = rows.iter().map(tab_line).collect::<Vec<_>>().join("\n");
    let _ = write!(out, "\n{QUOTED_NAMES}");
    if let Some(next) = data["next_from"].as_u64() {
        let _ = write!(
            out,
            "\n...{} in all. `--from {next}` shows the next ones.",
            count(number(&data["count"]), "tab", "tabs")
        );
    }
    out
}

fn facts(data: &Value) -> String {
    let mut out = format!("{}, {}", count(number(&data["pages"]), "page", "pages"), bytes(number(&data["file_bytes"])));
    if let Some(version) = data["pdf_version"].as_str() {
        let _ = write!(out, ", PDF {version}");
    }
    if data["encrypted"] == true {
        out.push_str(", encrypted");
    }
    if let Some(note) = data["encryption_note"].as_str() {
        let _ = write!(out, "\n{note}");
    }
    if data["has_contents"] == true {
        let _ = write!(out, "\nContents: {}.", count(number(&data["contents_entries"]), "entry", "entries"));
    } else if data["structure"] == "renderer_only" {
        out.push_str("\nContents: couldn't be read.");
    } else {
        out.push_str("\nContents: none.");
    }
    if data["edited"] == true {
        out.push_str("\nUnsaved edits: yes.");
    }
    if let Some(note) = data["note"].as_str() {
        let _ = write!(out, "\n{note}");
    }
    let _ = write!(out, "\nFrom the file, untrusted: name {:?}", text(&data["untrusted"]["name"]));
    if let Some(title) = data["untrusted"]["title"].as_str() {
        let _ = write!(out, ", title {title:?}");
    }
    out
}

fn signatures(data: &Value) -> String {
    if data["structure_read"] == false {
        return text(&data["note"]).to_string();
    }
    let rows = data["signatures"].as_array().map_or(&[][..], Vec::as_slice);
    if rows.is_empty() {
        return if data["partial"] == true {
            format!("No signature fields found, but {}", text(&data["note"]))
        } else {
            "No signature fields found in this file.".to_string()
        };
    }
    let mut out = String::new();
    for row in rows {
        let _ = write!(out, "{}", text(&row["summary"]));
        if let Some(page) = row["page"].as_u64() {
            let _ = write!(out, ", p. {page}");
        }
        let untrusted = &row["untrusted"];
        let _ = write!(out, ". From the file, untrusted: field {:?}", text(&untrusted["field"]));
        if let Some(name) = untrusted["stated_name"].as_str() {
            let _ = write!(out, ", stated name {name:?}");
        }
        if let Some(time) = untrusted["stated_time"].as_str() {
            let _ = write!(out, ", stated time {time:?}");
        }
        out.push('\n');
    }
    out.push_str(text(&data["note"]));
    out
}
