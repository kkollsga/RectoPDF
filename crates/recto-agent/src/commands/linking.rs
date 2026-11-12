//! Links within a document, and the trail through them: `links`, and later `goto`, `back`, `forward` and `follow`.

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
