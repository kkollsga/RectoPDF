//! `ref` and `resolve`: a reference to a page that outlives the file's page numbers, and where that page is now. Read-only: both read the tab's file with
//! the session free, and change nothing of it, of the tab or of the owner's edits. The executor and its words are `crate::locating`; the core is
//! `recto_core::locator`.

use super::{Cmd, Cx, row_access};
use crate::human::{number, text};
use crate::table::*;
use crate::*;
use recto_core::locator::{self, Locator};
use serde_json::{Value, json};

/// `recto ref <page>`: a reference to a page of the tab, to give back to `resolve` later, when the file's page numbers may have changed.
#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub struct Ref {
    pub page: u32,
    #[serde(default)]
    pub tab: TabRef,
}

impl Cmd for Ref {
    const TAG: &'static str = "ref";
    const ROWS: &'static [Spec] = ROWS_REF;
    // lopdf only: no worker draws a page.
    const DRAWS: bool = false;
    const READS_PAGES: bool = true;
    const PROTECTED_OK: bool = false;
    const ABOUT: super::About = super::About::Its;
    const WAITS: bool = true;
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
        locating::reference(cx.access, &self.tab, self.page)
    }
    fn human(data: &Value) -> String {
        format!("A reference to page {}: {}", number(&data["page"]), text(&data["ref"]))
    }
}

/// `recto resolve <ref>`: where a reference's page is in the tab now. Read from a `line`, so a token that can't be one is refused when the line is read,
/// before the level or the tab is asked, and its words never repeat the token.
#[derive(Clone, Debug, serde::Serialize)]
#[serde(into = "line::Resolve")]
pub struct Resolve {
    pub reference: Locator,
    pub tab: TabRef,
}

/// `resolve` as it's written on the wire.
pub mod line {
    use crate::TabRef;
    use serde::{Deserialize, Serialize};

    #[derive(Clone, Deserialize, Serialize)]
    #[serde(deny_unknown_fields)]
    pub struct Resolve {
        #[serde(rename = "ref")]
        pub reference: String,
        #[serde(default)]
        pub tab: TabRef,
    }
}

impl TryFrom<line::Resolve> for Resolve {
    type Error = AgentError;
    fn try_from(line: line::Resolve) -> Result<Self, AgentError> {
        let line::Resolve { reference, tab } = line;
        let reference = locator::decode(&reference).map_err(|error| match error {
            locator::Error::UnsupportedVersion => locating::reference_too_new(),
            _ => locating::bad_reference(),
        })?;
        Ok(Self { reference, tab })
    }
}

impl From<Resolve> for line::Resolve {
    fn from(asked: Resolve) -> Self {
        Self {
            reference: locator::encode(&asked.reference),
            tab: asked.tab,
        }
    }
}

impl Cmd for Resolve {
    const TAG: &'static str = "resolve";
    const ROWS: &'static [Spec] = ROWS_RESOLVE;
    const DRAWS: bool = false;
    const READS_PAGES: bool = true;
    const PROTECTED_OK: bool = false;
    const ABOUT: super::About = super::About::Its;
    const WAITS: bool = true;
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
    /// The page the reference hints at, a number, and never the token or any part of its checks.
    fn log_details(&self) -> Value {
        json!({ "tab": super::log_tab(&self.tab), "page": self.reference.page().number() })
    }
    fn run<A: SessionAccess>(&self, cx: &mut Cx<'_, A>) -> Result<Value, AgentError> {
        locating::resolve(cx.access, &self.tab, self.reference)
    }
    fn human(data: &Value) -> String {
        match data["page"].as_u64() {
            Some(page) => format!("{}: page {page}. {}", text(&data["status"]), text(&data["note"])),
            None => format!("{}. {}", text(&data["status"]), text(&data["note"])),
        }
    }
}

pub(crate) const ROWS_REF: &[Spec] = &[Spec {
    name: "ref",
    tag: "ref",
    group: GROUP_READ,
    access: Access::At(Level::ReadOnly),
    summary: "A reference to a page, as a short token that names the page by its content as well as its number. Give it to `resolve` later to find the page again after another program has added, removed or moved pages. It reads the file's pages, and changes nothing. In a tab with unsaved edits the page is counted as the tab shows it; a page added from another PDF can't be referenced until saved.",
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
    data: "`{ tab, page, file_page, ref }`. `page` is the page as asked, in the tab as shown; `file_page` is its page of the file on disk, which differs only when there are unsaved edits. `ref` is the token: pass it whole to `resolve`.",
    examples: &["recto ref 3", "recto ref 3 --tab \"#2\" --json"],
    ..BASE
}];

pub(crate) const ROWS_RESOLVE: &[Spec] = &[Spec {
    name: "resolve",
    tag: "resolve",
    group: GROUP_READ,
    access: Access::At(Level::ReadOnly),
    summary: "Where a reference's page is now. The answer is a status, never a guess: `found` where the reference said, `moved` when the file's only page with that content is somewhere else, `ambiguous` when several have it, `gone` when every page was read and none has it, `incomplete` when the search couldn't read every page. All of those are exit 0. A reference is data from outside: it grants no access, and only the tab's own file is read.",
    args: &[
        Arg {
            key: "ref",
            flag: None,
            kind: Kind::Text,
            required: true,
            help: "A reference `recto ref` gave, whole.",
        },
        TAB,
    ],
    data: "`{ tab, status, page, file_page, was_page, candidates: [ { page, file_page } ], document, frame, target, looked_at, pages_read, note }`. `status` is `found`, `moved`, `ambiguous`, `gone` or `incomplete`. `page` (in the tab as shown, null when unsaved edits removed it) and `file_page` (of the file on disk) are only for `found` and `moved`, counted from 1; `was_page` is where the reference pointed. `candidates` lists up to 8 pages for `ambiguous`, and the one match found for `incomplete`. `document` is `same`, `different` or `unknown`: information only, it never changes the status. `frame` says whether the page's crop and turn are the reference's. `target` is `{ kind: \"page\" }`, `{ kind: \"spot\", placed, rect | why }` or `{ kind: \"characters\", start, end, checked: false }`. `looked_at` is `hint`, `all` or `part`.",
    examples: &[
        "recto resolve r1.040g2081040g2.p1-081040g2081040g2081040g208-0000000",
        "recto resolve r1.040g2081040g2.p1-081040g2081040g2081040g208-0000000 --tab \"#2\" --json",
    ],
    ..BASE
}];

#[cfg(test)]
mod tests {
    use super::*;
    use recto_core::locator::{DocumentTags, PageRef, Target};
    use recto_core::page_id::PageIdentity;

    /// The reference the rows' examples use, minted as `recto ref` mints one from a made-up page.
    const EXAMPLE_REF: &str = "r1.040g2081040g2.p1-081040g2081040g2081040g208-0000000";

    fn made_up() -> Locator {
        let identity = PageIdentity {
            digest: [2; 32],
            object: (1, 0),
            frame: 0,
        };
        let page = PageRef::new(1, &identity).expect("page 1");
        Locator::new(DocumentTags::new([1; 8], None), page, Target::Page)
    }

    fn read(token: &str) -> Result<Command, AgentError> {
        crate::parse(&json!({ "cmd": "resolve", "ref": token }).to_string())
    }

    /// Failure: a token no longer than the cap but not ASCII, or past 128 bytes, reaches the level or the tab (so an agent at Off is told about its level, or
    /// a hostile token is looked at beside the session), or its refusal repeats the hostile text back. It is refused as read, with constant words.
    #[test]
    fn a_token_over_128_bytes_or_non_ascii_is_bad_reference_before_the_level_is_asked() {
        let long = format!("r1.{}", "a".repeat(126));
        for hostile in [
            long.as_str(),
            "r1.é",
            "",
            "nonsense",
            "r1.0000000000000000.p1-00000000000000000000000000000000-00000000.zzz",
        ] {
            let error = read(hostile).expect_err(hostile);
            assert_eq!((error.code, error.exit), (Code::BAD_REFERENCE, exit::USAGE), "{hostile:?}");
            assert!(
                !error.message.contains("nonsense") && !error.message.contains("r1."),
                "the refusal says only its own words: {}",
                error.message
            );
        }
        assert_eq!(read(EXAMPLE_REF).map(|_| ()).err().map(|error| error.code), None, "a whole token is read");
    }

    /// Failure: a reference made by a newer RectoPDF is called garbage, so the agent remakes it instead of updating, or is read as version 1.
    #[test]
    fn a_newer_version_is_reference_too_new() {
        let error = read("r2.040g2081040g2.p1-081040g2081040g2081040g208-0000000").expect_err("r2");
        assert_eq!((error.code, error.exit), (Code::REFERENCE_TOO_NEW, exit::USAGE));
    }

    /// Failure: a row's example is a token the command refuses, so the help teaches a line that fails.
    #[test]
    fn the_rows_example_token_decodes() {
        let decoded = locator::decode(EXAMPLE_REF).expect("the example decodes");
        assert_eq!(locator::encode(&decoded), EXAMPLE_REF, "and is the canonical form of what it names");
        assert_eq!(decoded, made_up(), "it names the made-up page it was minted from");
        for row in ROWS_RESOLVE {
            assert!(row.examples.iter().all(|example| example.contains(EXAMPLE_REF)), "{:?}", row.examples);
        }
    }

    /// Failure: the activity log, or a command printed in a test failure, carries the token or a piece of its checks, which name a page's content.
    #[test]
    fn resolve_never_logs_the_token() {
        let command = read(EXAMPLE_REF).expect("example");
        let shown = format!("{:?} {}", command, command.log_details());
        for piece in ["040g2081040g2", "081040g2081040g2081040g208", "0000000"] {
            assert!(!shown.contains(piece), "{shown}");
        }
        assert_eq!(command.log_details()["page"], 1, "only the page number it hints at");
    }
}
