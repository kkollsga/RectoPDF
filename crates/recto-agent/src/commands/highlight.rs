//! `recto highlight`, `recto highlight remove` and `recto highlights` (R4.3, the pilot of `docs/FEATURES.md`). Made from `tools/new-feature.sh
//! highlight`; what they do is `crate::highlighting`'s.
//!
//! **Their gates.** `highlight` and `highlight remove` are edits at Control, as `page rotate` is: an agent's highlight is its own step, which below Full
//! can't throw away the user's (the one edit path's rules), and whether an agent's Save at Control may write it is `recto save`'s own rule, unchanged.
//! `highlights` reads, at Read-only. All three read the tab they name (`ABOUT` is `Its`) and none acts on a protected document (`PROTECTED_OK` false),
//! as each reads that document's pages: its text, for what a pick takes, or its structure. None gives any text of the document back, so a file that
//! asks not to be copied needs no `--anyway`. `highlight` and `highlight remove` with a pick read pages through a worker, so they draw and wait.

use super::{Cmd, Cx, row_access};
use crate::highlighting::{self, Which};
use crate::human::edit_state;
use crate::selecting::Pick;
use crate::table::*;
use crate::*;
use serde_json::{Value, json};

/// Highlights what a pick takes on a page, or across pages: the same arguments as `recto select`.
#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub struct Highlight {
    #[serde(default)]
    pub tab: TabRef,
    #[serde(default)]
    pub page: Option<u32>,
    #[serde(default)]
    pub to_page: Option<u32>,
    #[serde(default)]
    pub chars: Option<String>,
    #[serde(default)]
    pub at: Option<String>,
    #[serde(default)]
    pub to: Option<String>,
    #[serde(default)]
    pub unit: Option<String>,
}

impl Highlight {
    fn pick(&self) -> Pick<'_> {
        Pick {
            tab: &self.tab,
            page: self.page,
            to_page: self.to_page,
            chars: self.chars.as_deref(),
            at: self.at.as_deref(),
            to: self.to.as_deref(),
            unit: self.unit.as_deref(),
            all: false,
            anyway: false,
        }
    }
}

impl Cmd for Highlight {
    const TAG: &'static str = "highlight";
    const ROWS: &'static [Spec] = ROWS_HIGHLIGHT;
    const DRAWS: bool = true;
    const READS_PAGES: bool = true;
    const PROTECTED_OK: bool = false;
    const ABOUT: super::About = super::About::Its;
    const WAITS: bool = true;
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
        json!({ "tab": super::log_tab(&self.tab), "page": self.page, "to_page": self.to_page })
    }
    fn run<A: SessionAccess>(&self, cx: &mut Cx<'_, A>) -> Result<Value, AgentError> {
        highlighting::add(cx.access, cx.actor, cx.events, &self.pick())
    }
    fn human(data: &Value) -> String {
        edit_state(data)
    }
}

/// Takes highlights away: one made as an edit by its number, or what's under a point, or what a pick touches, the file's own too. One way of naming at a time, refused as it's
/// read otherwise, before the level or the tab is looked at (`commands/mod.rs`, "A malformed request is refused as malformed first").
#[derive(Clone, PartialEq, serde::Serialize)]
#[serde(into = "line::HighlightRemove")]
pub struct HighlightRemove {
    pub tab: TabRef,
    pub what: RemoveWhat,
}

/// What `highlight remove` names.
#[derive(Clone, Debug, PartialEq)]
pub enum RemoveWhat {
    /// A highlight's part by its number in `recto highlights`.
    Id(u32),
    /// Whatever is under a point on a page.
    At { page: u32, x: f32, y: f32 },
    /// Whatever the text a pick takes touches, as the person's selection does.
    Pick {
        page: Option<u32>,
        to_page: Option<u32>,
        chars: Option<String>,
        at: Option<String>,
        to: Option<String>,
    },
}

/// `highlight remove` as it's written on the wire.
pub mod line {
    use serde::{Deserialize, Serialize};

    #[derive(Clone, Debug, Default, Deserialize, Serialize)]
    #[serde(deny_unknown_fields)]
    pub struct HighlightRemove {
        #[serde(default)]
        pub tab: crate::TabRef,
        #[serde(default)]
        pub id: Option<u32>,
        #[serde(default)]
        pub page: Option<u32>,
        #[serde(default)]
        pub to_page: Option<u32>,
        #[serde(default)]
        pub chars: Option<String>,
        #[serde(default)]
        pub at: Option<String>,
        #[serde(default)]
        pub to: Option<String>,
    }
}

/// The refusal of `--id` with anything else.
pub(crate) const ID_ALONE: &str = "`--id` goes alone: a highlight's number names it.";

/// A point `X,Y`, both finite.
fn point(at: &str) -> Option<(f32, f32)> {
    let (x, y) = at.split_once(',')?;
    let (x, y) = (x.trim().parse::<f32>().ok()?, y.trim().parse::<f32>().ok()?);
    (x.is_finite() && y.is_finite()).then_some((x, y))
}

impl TryFrom<line::HighlightRemove> for HighlightRemove {
    type Error = AgentError;
    fn try_from(line: line::HighlightRemove) -> Result<Self, AgentError> {
        let usage = |words: &str| AgentError::new(Code::USAGE, words.to_string(), exit::USAGE);
        let line::HighlightRemove {
            tab,
            id,
            page,
            to_page,
            chars,
            at,
            to,
        } = line;
        let what = match (id, page) {
            (Some(id), None) if to_page.is_none() && chars.is_none() && at.is_none() && to.is_none() => RemoveWhat::Id(id),
            (Some(_), _) => return Err(usage(ID_ALONE)),
            (None, Some(0)) => return Err(usage("Pages count from 1.")),
            (None, Some(page)) if to_page.is_none() && chars.is_none() && to.is_none() && at.is_some() => {
                let (x, y) = at.as_deref().and_then(point).ok_or_else(|| usage("`--at` is `X,Y` in points."))?;
                RemoveWhat::At { page, x, y }
            }
            // Nothing named is a pick of nothing, which is the page being read, as `recto select` has it: every unsaved highlight on it.
            (None, page) => RemoveWhat::Pick { page, to_page, chars, at, to },
        };
        Ok(Self { tab, what })
    }
}

impl From<&HighlightRemove> for line::HighlightRemove {
    fn from(asked: &HighlightRemove) -> Self {
        let mut line = Self {
            tab: asked.tab.clone(),
            ..Self::default()
        };
        match &asked.what {
            RemoveWhat::Id(id) => line.id = Some(*id),
            RemoveWhat::At { page, x, y } => {
                line.page = Some(*page);
                line.at = Some(format!("{x},{y}"));
            }
            RemoveWhat::Pick { page, to_page, chars, at, to } => {
                (line.page, line.to_page, line.chars, line.at, line.to) = (*page, *to_page, chars.clone(), at.clone(), to.clone());
            }
        }
        line
    }
}

impl From<HighlightRemove> for line::HighlightRemove {
    fn from(asked: HighlightRemove) -> Self {
        Self::from(&asked)
    }
}

/// Prints as the line it is, so a command made in code reads as one an agent could send.
impl std::fmt::Debug for HighlightRemove {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        line::HighlightRemove::from(self).fmt(f)
    }
}

impl Cmd for HighlightRemove {
    const TAG: &'static str = "highlight_remove";
    const ROWS: &'static [Spec] = ROWS_HIGHLIGHT_REMOVE;
    const DRAWS: bool = true;
    const READS_PAGES: bool = true;
    const PROTECTED_OK: bool = false;
    const ABOUT: super::About = super::About::Its;
    const WAITS: bool = true;
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
        let (id, page) = match &self.what {
            RemoveWhat::Id(id) => (Some(*id), None),
            RemoveWhat::At { page, .. } => (None, Some(*page)),
            RemoveWhat::Pick { page, .. } => (None, *page),
        };
        json!({ "tab": super::log_tab(&self.tab), "id": id, "page": page })
    }
    fn run<A: SessionAccess>(&self, cx: &mut Cx<'_, A>) -> Result<Value, AgentError> {
        let (access, actor, events) = (cx.access, cx.actor, &mut *cx.events);
        match &self.what {
            RemoveWhat::Id(id) => highlighting::remove(access, actor, events, &self.tab, Which::Id(*id)),
            RemoveWhat::At { page, x, y } => highlighting::remove(access, actor, events, &self.tab, Which::At { page: *page, x: *x, y: *y }),
            RemoveWhat::Pick { page, to_page, chars, at, to } => {
                let pick = Pick {
                    tab: &self.tab,
                    page: *page,
                    to_page: *to_page,
                    chars: chars.as_deref(),
                    at: at.as_deref(),
                    to: to.as_deref(),
                    unit: None,
                    all: false,
                    anyway: false,
                };
                highlighting::remove(access, actor, events, &self.tab, Which::Pick(&pick))
            }
        }
    }
    fn human(data: &Value) -> String {
        edit_state(data)
    }
}

/// The tab's highlights: its unsaved ones, and with `--page` the file's own on that page.
#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub struct Highlights {
    #[serde(default)]
    pub tab: TabRef,
    #[serde(default)]
    pub page: Option<u32>,
}

impl Cmd for Highlights {
    const TAG: &'static str = "highlights";
    const ROWS: &'static [Spec] = ROWS_HIGHLIGHTS;
    const DRAWS: bool = false;
    const READS_PAGES: bool = true;
    const PROTECTED_OK: bool = false;
    const ABOUT: super::About = super::About::Its;
    const WAITS: bool = true;
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
        highlighting::list(cx.access, &self.tab, self.page)
    }
    fn human(data: &Value) -> String {
        let edits = data["edits"].as_array().map_or(0, Vec::len);
        let mut out = match edits {
            0 => "No unsaved highlights.".to_string(),
            1 => "1 unsaved highlight:".to_string(),
            many => format!("{many} unsaved highlights:"),
        };
        for edit in data["edits"].as_array().map_or(&[][..], Vec::as_slice) {
            let page = edit["page"]
                .as_u64()
                .map_or_else(|| "a page not in the document now".to_string(), |page| format!("page {page}"));
            out.push_str(&format!("\n  #{} on {page}, by the {}", edit["id"], edit["by"].as_str().unwrap_or("user")));
        }
        if let Some(removed) = data["removed"].as_array().filter(|removed| !removed.is_empty()) {
            out.push_str(&format!("\n{} of the file's own taken out, unsaved.", removed.len()));
        }
        if let Some(found) = data["file"]["highlights"].as_array() {
            out.push_str(&format!("\nIn the file, on page {}: {}.", data["file"]["page"], found.len()));
        }
        out
    }
}

pub(crate) const ROWS_HIGHLIGHT: &[Spec] = &[Spec {
    name: "highlight",
    tag: "highlight",
    group: GROUP_EDIT,
    access: Access::At(Level::Control),
    summary: "Highlights what `recto select` takes for the same arguments, as Mark Up > Highlight does: an unsaved edit like a turn, written into the PDF as a standard highlight on Save. Across pages it's one step.",
    args: &[PICK_PAGE, PICK_TO_PAGE, PICK_CHARS, PICK_AT, PICK_TO, PICK_UNIT, TAB],
    bare_page: true,
    data: "As `page rotate` (`did`, `edited`, …), with `highlighted: { pages, lines }`.",
    emits: &["edits_changed"],
    examples: &[
        "recto highlight --page 3 --chars 120-180",
        "recto highlight --page 3 --at 150,220 --unit line",
        "recto highlight --page 3 --chars 900- --to-page 4 --json",
    ],
    ..BASE
}];

pub(crate) const ROWS_HIGHLIGHT_REMOVE: &[Spec] = &[Spec {
    name: "highlight remove",
    tag: "highlight_remove",
    group: GROUP_EDIT,
    access: Access::At(Level::Control),
    summary: "Takes highlights away, as one step: those made as edits, by number, under a point, or wherever a pick touches one (with none, every one on the page being read), and the file's own under the point or the pick, which the next save takes out of the file (a rewrite, at Full). The file's own on a page added from another PDF can be removed once the document is saved.",
    args: &[
        Arg {
            key: "id",
            flag: Some("--id"),
            kind: Kind::Count(u32::MAX),
            required: false,
            help: "A highlight's number, from `recto highlights`.",
        },
        PICK_PAGE,
        PICK_TO_PAGE,
        PICK_CHARS,
        PICK_AT,
        PICK_TO,
        TAB,
    ],
    data: "As `page rotate` (`did`, `edited`, …).",
    emits: &["edits_changed"],
    examples: &[
        "recto highlight remove --page 3 --chars 100-200",
        "recto highlight remove --page 3 --at 150,220",
        "recto highlight remove --id 2",
    ],
    ..BASE
}];

pub(crate) const ROWS_HIGHLIGHTS: &[Spec] = &[Spec {
    name: "highlights",
    tag: "highlights",
    group: GROUP_READ,
    access: Access::At(Level::ReadOnly),
    summary: "The tab's unsaved highlights, with their numbers and who made them, the file's own taken out and unsaved, and with `--page` the file's own on that page. Where they are and their colour, never any text.",
    args: &[
        Arg {
            key: "page",
            flag: Some("--page"),
            kind: Kind::Page,
            required: false,
            help: "A page whose own highlights to read from the file, counted from 1.",
        },
        TAB,
    ],
    data: "`{ tab, edits: [ { id, page, rects, by: user|agent, from: edit } ], removed: [ { page, index, from: file, removed: true } ], file: { page, highlights: [ { index, rects, colour, from: file, removed } ], truncated } | null, colour, note }`. `rects` are x, y, width, height in points on the page as the file shows it, from its top left; a page reads at most 256 of the file's highlights.",
    examples: &["recto highlights", "recto highlights --page 3 --json"],
    ..BASE
}];

#[cfg(test)]
mod tests {
    use super::*;

    /// Failure: `highlight remove` with two ways of naming what to take, or none, guesses one; `--at` that isn't a point is read as one; or page 0
    /// is read as page 1. Each refused as it's read, so before the level.
    #[test]
    fn highlight_remove_takes_one_way_of_naming_what_goes() {
        let read = |line: line::HighlightRemove| HighlightRemove::try_from(line);
        let refused = |line: line::HighlightRemove| read(line).err().map(|error| error.code);
        assert_eq!(
            read(line::HighlightRemove::default()).unwrap().what,
            RemoveWhat::Pick {
                page: None,
                to_page: None,
                chars: None,
                at: None,
                to: None
            },
            "nothing named is the page being read, as `select` has it"
        );
        let both = line::HighlightRemove {
            id: Some(1),
            page: Some(2),
            ..Default::default()
        };
        assert_eq!(refused(both), Some(Code::USAGE), "a number and a page");
        let not_a_point = line::HighlightRemove {
            page: Some(2),
            at: Some("NaN,4".into()),
            ..Default::default()
        };
        assert_eq!(refused(not_a_point), Some(Code::USAGE));
        let point = line::HighlightRemove {
            page: Some(2),
            at: Some("3, 4".into()),
            ..Default::default()
        };
        assert_eq!(read(point).unwrap().what, RemoveWhat::At { page: 2, x: 3.0, y: 4.0 });
        let page_zero = line::HighlightRemove {
            page: Some(0),
            at: Some("3,4".into()),
            ..Default::default()
        };
        assert_eq!(
            refused(page_zero),
            Some(Code::USAGE),
            "pages count from 1: page 0 isn't page 1 (review of R4.3)"
        );
        let id = read(line::HighlightRemove {
            id: Some(7),
            ..Default::default()
        })
        .unwrap();
        assert_eq!(id.what, RemoveWhat::Id(7));
        assert_eq!(read(line::HighlightRemove::from(&id)).unwrap(), id, "it prints as the line it was read from");
    }
}
