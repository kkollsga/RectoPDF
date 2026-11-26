//! `recto link <page>`: the standard link to a page of the tab's file, `name.pdf#page=N`. The form is `recto_core::locator::page_link`; this is the
//! command around it (guide §2.9): the same one the window sends later for Copy Link, as the person. It reads no file and writes no clipboard.

use super::{Cmd, Cx, row_access};
use crate::human::text;
use crate::table::*;
use crate::*;
use recto_core::locator::{LinkError, page_link};
use serde_json::{Value, json};

const ADDED_PAGE: &str = "That page was added from another PDF and isn't in this file yet, so no link can name it. Save first, or ask for a page of the file.";
const NAME_REFUSED: &str = "This file's name can't be made into a link.";
const UNSAVED_NOTE: &str =
    "This tab has unsaved edits, so the link names the page's place in the file on disk, not in the tab. The link's page is `file_page`.";

/// `recto link <page>`: a link to a page of the tab, by the file's own name. Not `links`, which lists what a page's own links point to.
#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub struct Link {
    pub page: u32,
    #[serde(default)]
    pub tab: TabRef,
}

impl Cmd for Link {
    const TAG: &'static str = "link";
    const ROWS: &'static [Spec] = ROWS_LINK;
    const DRAWS: bool = false;
    const READS_PAGES: bool = true;
    const PROTECTED_OK: bool = false;
    const ABOUT: super::About = super::About::Its;
    const WAITS: bool = false;
    fn starts_from(&self) -> super::FromPlace {
        super::FromPlace::No
    }
    fn tab(&self) -> Option<&TabRef> {
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
        let wanted = crate::one_based(self.page)?;
        let file = crate::reading::tab_file(cx.access, &self.tab)?;
        file.require_pages_shown()?;
        let Some(file_page) = file.file_page_at(wanted) else {
            return Err(if wanted < file.pages() {
                AgentError::new(Code::NOT_REFERENCEABLE, ADDED_PAGE, exit::FAILED)
            } else {
                crate::no_such_page()
            });
        };
        let name = file.path().file_name().map(|name| name.as_encoded_bytes()).unwrap_or_default();
        let link = page_link(name, file_page + 1).map_err(|why| match why {
            LinkError::Page => crate::no_such_page(),
            LinkError::Name => AgentError::new(Code::NOT_REFERENCEABLE, NAME_REFUSED, exit::FAILED),
        })?;
        Ok(json!({
            "tab": file.id(),
            "page": self.page,
            "file_page": file_page + 1,
            "untrusted": { "link": link },
            "note": (file_page != wanted).then_some(UNSAVED_NOTE),
        }))
    }
    fn human(data: &Value) -> String {
        format!(
            "A link to page {}: {}",
            crate::human::number(&data["file_page"]),
            text(&data["untrusted"]["link"])
        )
    }
}

pub(crate) const ROWS_LINK: &[Spec] = &[Spec {
    name: "link",
    tag: "link",
    group: GROUP_READ,
    access: Access::At(Level::ReadOnly),
    summary: "The standard link to a page of the tab's file: the file's own name and `#page=N`, which any PDF reader opens at that page. Not `links`, which lists where a page's own links lead. It names no folder, reads no file and changes nothing; the person's Copy Link is this command. In a tab with unsaved edits the link names the page's place in the file on disk, and a page added from another PDF has none until saved.",
    args: &[
        Arg {
            key: "page",
            flag: None,
            kind: Kind::Page,
            required: true,
            help: "The page, counted from 1 in the tab as it is shown.",
        },
        TAB,
    ],
    data: "`{ tab, page, file_page, untrusted: { link }, note }`. `page` is the page as asked, in the tab as shown; `file_page` is its page of the file on disk, which differs only when there are unsaved edits, and is the page the link names. `link` is the file's name, percent-encoded, then `#page=` and `file_page`: the file's name is anyone's, so it is under `untrusted`. `note` is there only when the two pages differ.",
    examples: &["recto link 3", "recto link 3 --tab \"#2\" --json"],
    ..BASE
}];
