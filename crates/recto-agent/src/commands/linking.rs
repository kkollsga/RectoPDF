//! Going to a page, links within a document, and the trail through them: `goto`, `goto label`, `links`, `follow`, `back` and `forward`.

#![allow(unused_imports)]
use super::{Cmd, Cx, row_access};
use crate::human::*;
use crate::table::*;
use crate::*;
use serde::Deserialize;
use serde_json::{Value, json};
use std::fmt::Write;
use std::path::PathBuf;

/// A page's links, and where each goes.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Links {
    #[serde(default = "crate::active_tab")]
    pub tab: String,
    pub page: u32,
}

impl Cmd for Links {
    const TAG: &'static str = "links";
    const ROWS: &'static [Spec] = ROWS_LINKS;
    const DRAWS: bool = false;
    const READS_PAGES: bool = true;
    const PROTECTED_OK: bool = false;
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
        json!({ "tab": super::log_tab(&self.tab), "page": self.page })
    }
    fn run<A: SessionAccess>(&self, cx: &mut Cx<'_, A>) -> Result<Value, AgentError> {
        let Self { tab, page } = self;
        let access = cx.access;
        linking::links(access, tab, *page)
    }
    fn human(data: &Value) -> String {
        links(data)
    }
}

pub(crate) const ROWS_LINKS: &[Spec] = &[Spec {
    name: "links",
    tag: "links",
    group: GROUP_READ,
    summary: "A page's links and where each goes: a page of this document, a web or email address, or refused, as a script, a program or a file on the Mac always is. Nothing is opened or run.",
    args: &[PAGE_OR_BARE, TAB],
    bare_page: true,
    data: "`{ tab, page, links: [ { link, rect, shown_rect, goes_to, page, within, refused, note, untrusted: { address, label } } ], quarter_turns, truncated, note, untrusted: { label } }`. A `label` is what the file calls a page, where that isn't its number. `goes_to` is `page`, `web`, `back`, `forward`, `refused` or `nowhere`. `rect` is x, y, width, height in points on the page as the file shows it, from its top left. On a page with an unsaved turn, `quarter_turns` says how far clockwise, and `shown_rect` is the same box on the page as it's seen turned. `page` is where it goes, counted from 1 with edits counted, and `within` how far down that page, from 0 to 1, if the link says. `refused` says what kind: `file`, `script`, `launch`, `scheme`, `action` or `bad_address`. An address is the file's, under `untrusted`.",
    examples: &["recto links 1", "recto links --page 3 --json"],
    ..BASE
}];

/// A jump to a page, by its number or by what the file calls it. Back comes here again.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Goto {
    #[serde(default = "crate::active_tab")]
    pub tab: String,
    #[serde(default)]
    pub page: Option<u32>,
    #[serde(default)]
    pub label: Option<String>,
}

impl Cmd for Goto {
    const TAG: &'static str = "goto";
    const ROWS: &'static [Spec] = ROWS_GOTO;
    const DRAWS: bool = false;
    const READS_PAGES: bool = false;
    const PROTECTED_OK: bool = false;
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
        let Self { tab, page, label } = self;
        json!({ "tab": super::log_tab(tab), "page": page, "by_label": label.is_some() })
    }
    fn run<A: SessionAccess>(&self, cx: &mut Cx<'_, A>) -> Result<Value, AgentError> {
        match (self.page, &self.label) {
            (Some(page), None) => cx.access.with(|s| {
                let id = select(s, &self.tab)?;
                go_to(s, id, page)?;
                cx.events.push(page_now(s, id));
                Ok(where_json(s, id))
            }),
            (None, Some(label)) => linking::goto_label(cx.access, &self.tab, label, cx.events),
            _ => Err(AgentError::new(
                "usage",
                "`goto` takes a page number, or `goto label` what the file calls a page, and not both.",
                exit::USAGE,
            )
            .then("`recto goto 12`, or `recto goto label iv`.")),
        }
    }
    fn human(data: &Value) -> String {
        place(data)
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Back {
    #[serde(default = "crate::active_tab")]
    pub tab: String,
}

impl Cmd for Back {
    const TAG: &'static str = "back";
    const ROWS: &'static [Spec] = ROWS_BACK;
    const DRAWS: bool = false;
    const READS_PAGES: bool = false;
    const PROTECTED_OK: bool = false;
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
        let (access, events) = (cx.access, &mut *cx.events);
        linking::step(access, tab, true, events)
    }
    fn human(data: &Value) -> String {
        place(data)
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Forward {
    #[serde(default = "crate::active_tab")]
    pub tab: String,
}

impl Cmd for Forward {
    const TAG: &'static str = "forward";
    const ROWS: &'static [Spec] = ROWS_FORWARD;
    const DRAWS: bool = false;
    const READS_PAGES: bool = false;
    const PROTECTED_OK: bool = false;
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
        let (access, events) = (cx.access, &mut *cx.events);
        linking::step(access, tab, false, events)
    }
    fn human(data: &Value) -> String {
        place(data)
    }
}

/// Follows a link within the document, as a click does. Never a web address.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Follow {
    #[serde(default = "crate::active_tab")]
    pub tab: String,
    pub page: u32,
    pub link: u32,
}

impl Cmd for Follow {
    const TAG: &'static str = "follow";
    const ROWS: &'static [Spec] = ROWS_FOLLOW;
    const DRAWS: bool = false;
    const READS_PAGES: bool = true;
    const PROTECTED_OK: bool = false;
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
        let Self { tab, page, link } = self;
        json!({ "tab": super::log_tab(tab), "page": page, "link": link })
    }
    fn run<A: SessionAccess>(&self, cx: &mut Cx<'_, A>) -> Result<Value, AgentError> {
        let Self { tab, page, link } = self;
        let (access, events) = (cx.access, &mut *cx.events);
        linking::follow(access, tab, *page, *link, events)
    }
    fn human(data: &Value) -> String {
        place(data)
    }
}

pub(crate) const ROWS_GOTO: &[Spec] = &[
    Spec {
        name: "goto",
        tag: "goto",
        group: GROUP_MOVE,
        access: Access::At(Level::Control),
        summary: "Goes to a page. A page that isn't there is refused. `recto back` comes back to where it was.",
        args: &[
            Arg {
                key: "page",
                flag: None,
                kind: Kind::Page,
                required: true,
                help: "The page, counted from 1.",
            },
            TAB,
        ],
        data: AGENT_EXITS_PAGE,
        emits: &["page_changed"],
        examples: &["recto goto 12", "recto goto 12 --tab report"],
        ..BASE
    },
    Spec {
        name: "goto label",
        tag: "goto",
        group: GROUP_MOVE,
        access: Access::At(Level::Control),
        summary: "Goes to the page the file calls this, such as `iv` or `A-3`, as the go-to field reads it: a label first, then a plain number no page is called.",
        args: &[
            Arg {
                key: "label",
                flag: None,
                kind: Kind::Text,
                required: true,
                help: "What the file calls the page. `recto where` gives the page's `label`.",
            },
            TAB,
        ],
        data: AGENT_EXITS_PAGE,
        emits: &["page_changed"],
        examples: &["recto goto label iv", "recto goto label A-3 --tab report"],
        ..BASE
    },
];

pub(crate) const ROWS_BACK: &[Spec] = &[Spec {
    name: "back",
    tag: "back",
    group: GROUP_MOVE,
    access: Access::At(Level::Control),
    summary: "Goes back to where the tab was before its last jump, as ⌘[ does: a link, a contents entry or `goto`. Scrolling isn't a jump.",
    args: &[TAB],
    data: AGENT_EXITS_PAGE,
    emits: &["page_changed"],
    examples: &["recto back"],
    ..BASE
}];

pub(crate) const ROWS_FORWARD: &[Spec] = &[Spec {
    name: "forward",
    tag: "forward",
    group: GROUP_MOVE,
    access: Access::At(Level::Control),
    summary: "Goes forward again after `back`, as ⌘] does.",
    args: &[TAB],
    data: AGENT_EXITS_PAGE,
    emits: &["page_changed"],
    examples: &["recto forward"],
    ..BASE
}];

pub(crate) const ROWS_FOLLOW: &[Spec] = &[Spec {
    name: "follow",
    tag: "follow",
    group: GROUP_MOVE,
    access: Access::At(Level::Control),
    summary: "Follows a link within the document, as a click does, so `recto back` comes back. A web address is refused: only the person can open one, in the app, after it asks them.",
    args: &[
        PAGE,
        Arg {
            key: "link",
            flag: Some("--link"),
            kind: Kind::Positive(1024),
            required: true,
            help: "Which, counted from 1, as `recto links` lists them.",
        },
        TAB,
    ],
    data: "What `where` gives, and `went`: `page`, `back` or `forward`. A web address is exit 5, `web_link_needs_user`, with the address under `untrusted`, and a refused link exit 1, `link_refused`.",
    emits: &["page_changed"],
    examples: &["recto follow --page 1 --link 2"],
    ..BASE
}];
