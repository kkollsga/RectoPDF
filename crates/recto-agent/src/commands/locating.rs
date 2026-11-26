//! `ref` and `resolve`: a reference to a page that outlives the file's page numbers, and where that page is now. Read-only: both read the tab's file with
//! the session free, and change nothing of it, of the tab or of the owner's edits. The executor and its words are `crate::locating`; the core is
//! `recto_core::locator`.

use super::{Cmd, Cx, row_access};
use crate::human::{number, text};
use crate::table::*;
use crate::*;
use recto_core::locator::{self, Locator};
use serde_json::{Value, json};

/// The one reference token the help, the tests and the activity log's walk use: a made-up page, as `recto ref` would write it. A
/// macro, so the rows' examples can be built from the same literal with `concat!`.
macro_rules! example_ref {
    () => {
        "r1.040g2081040g2.p1-081040g2081040g2081040g208-0000000"
    };
}

/// A made-up reference to characters 0 to 5 of the same page, for the examples of what only takes one (`highlight --ref`).
macro_rules! example_ref_chars {
    () => {
        "r1.040g2081040g2.p1-081040g2081040g2081040g208-0000000.c0-5-0c1g60r30c1g6"
    };
}

/// [`example_ref!`], for the tests that need it as a value. Holds no words: a token is page numbers and checks.
#[cfg(any(test, feature = "test-words"))]
pub const EXAMPLE_REF: &str = example_ref!();

/// [`example_ref_chars!`], for the tests that need it as a value.
#[cfg(any(test, feature = "test-words"))]
pub const EXAMPLE_REF_CHARS: &str = example_ref_chars!();

/// A reference to page 1 of a made-up document, with the target given: the one fixture of the tests that need a [`Locator`] (not every one needs to read
/// a file, and a token is only numbers).
#[cfg(test)]
pub(crate) fn made_up_locator(target: locator::Target) -> Locator {
    use recto_core::locator::{DocumentTags, PageRef};
    let identity = recto_core::page_id::PageIdentity {
        digest: [2; 32],
        object: (1, 0),
        frame: 0,
    };
    let page = PageRef::new(1, &identity).expect("page 1 is a page");
    Locator::new(DocumentTags::new([1; 8], None), page, target)
}

pub(crate) const TEXT_BOTH: &str = "`text` takes a page or `--ref`, not both.";
pub(crate) const TEXT_NEITHER: &str = "`text` needs a page or `--ref`.";
pub(crate) const RENDER_BOTH: &str = "`render` takes a page or `--ref`, not both.";
pub(crate) const RENDER_NEITHER: &str = "`render` needs a page or `--ref`.";
pub(crate) const SNAPSHOT_BOTH: &str = "`snapshot` takes a page or `--ref`, not both.";
pub(crate) const SNAPSHOT_NEITHER: &str = "`snapshot` needs a page or `--ref`.";
pub(crate) const EXPORT_SVG_BOTH: &str = "`export svg` takes a page or `--ref`, not both.";
pub(crate) const EXPORT_SVG_NEITHER: &str = "`export svg` needs a page or `--ref`.";
/// `snapshot --ref` goes to a file or to the answer, never to the person's clipboard, which is their own and is made for the page they saw.
pub(crate) const SNAPSHOT_REF_NOT_CLIPBOARD: &str = "`snapshot --ref` can't go to the clipboard: that is the person's, of the page they saw.";
/// `goto` with no page, no label and no `--ref`, whatever else it came with.
pub(crate) const GOTO_NEITHER: &str = "`goto` needs a page number, a label (`goto label`) or `--ref`.";
pub(crate) const GOTO_REF_ALONE: &str =
    "`goto --ref` goes to the page the reference points at, so it takes no page, label, `--entry` or `--link`. `--within` goes with it.";
/// Shared by `select`, `copy` and `highlight`.
pub(crate) const PICK_REF_ALONE: &str =
    "`--ref` is the page or characters the reference points at, so it takes no `--page`, `--to-page`, `--chars`, `--at`, `--to`, `--unit` or `--all`.";
pub(crate) const HIGHLIGHT_REF_CHARS: &str =
    "`highlight --ref` takes a reference to characters, as `recto ref --chars` makes one. To highlight a whole page, give `--page`.";

/// The one way a `--ref` token becomes a reference, for every command that takes one: a token that can't be one is refused with constant words that never
/// repeat it, and a newer RectoPDF's is said to be.
pub(crate) fn token(word: &str) -> Result<Locator, AgentError> {
    locator::decode(word).map_err(|error| match error {
        locator::Error::UnsupportedVersion => locating::reference_too_new(),
        _ => locating::bad_reference(),
    })
}

fn usage(words: &'static str) -> AgentError {
    AgentError::new(Code::USAGE, words, exit::USAGE)
}

/// A `--ref` that goes alone, as the commands that take one beside other flags read it: the token is read first (a hostile one is refused as such, before
/// anything is said of what it was given with), then `others` says whether anything that names a place came with it, which is refused in the command's words.
pub(crate) fn reference_alone(word: Option<&str>, others: bool, words: &'static str) -> Result<Option<Locator>, AgentError> {
    let reference = word.map(token).transpose()?;
    if reference.is_some() && others {
        return Err(usage(words));
    }
    Ok(reference)
}

/// Where `text` and `render` read: a page by its number, or the page a reference points at now. Built only by [`PageOrRef::of`], so a command that holds
/// one holds exactly one of the two, and a reference in it is already a reference RectoPDF made.
#[derive(Clone, Copy, Debug)]
pub enum PageOrRef {
    Page(u32),
    Ref(Locator),
}

impl PageOrRef {
    /// Reads the two wire fields of a command that takes either: a token that can't be a reference is refused with its constant words, and a line with both
    /// or neither with the command's own: `words` is the command's (both, neither).
    pub(crate) fn of(words: (&'static str, &'static str), page: Option<u32>, reference: Option<&str>) -> Result<Self, AgentError> {
        let (both, neither) = words;
        match (page, reference) {
            (Some(page), None) => Ok(Self::Page(page)),
            (None, Some(word)) => Ok(Self::Ref(token(word)?)),
            (Some(_), Some(word)) => {
                token(word)?;
                Err(usage(both))
            }
            (None, None) => Err(usage(neither)),
        }
    }

    /// The two wire fields it was read from.
    pub(crate) fn wire(&self) -> (Option<u32>, Option<String>) {
        match self {
            Self::Page(page) => (Some(*page), None),
            Self::Ref(reference) => (None, Some(locator::encode(reference))),
        }
    }

    /// For the activity log: the page, or the page a reference hints at. A number, never the token or any part of its checks.
    pub(crate) fn hint(&self) -> u32 {
        match self {
            Self::Page(page) => *page,
            Self::Ref(reference) => reference.page().number(),
        }
    }

    /// What `text` and `render` read: the page's place in the tab now, and what was said of finding it when it was by reference.
    pub(crate) fn find<A: SessionAccess>(
        &self,
        access: &A,
        tab: &TabRef,
        when: locating::CharsChecked,
    ) -> Result<(u32, Option<locating::Located>), AgentError> {
        match self {
            Self::Page(page) => Ok((*page, None)),
            Self::Ref(reference) => {
                let found = locating::page_of(access, tab, reference, when)?;
                Ok((found.page, Some(found)))
            }
        }
    }
}

/// The `--ref` flag of `text` and `render`.
pub(crate) const REF: Arg = Arg {
    key: "ref",
    flag: Some("--ref"),
    kind: Kind::Reference,
    required: false,
    help: "A reference `recto ref` gave, whole: reads the page it points at now, even if another program moved it. Instead of a page. It refuses when the page is gone, can't be told from others or wasn't fully looked for (`recto resolve` says which).",
};

/// [`PAGE_OR_BARE`] for a command that also takes `--ref`.
pub(crate) const PAGE_OR_REF: Arg = Arg {
    required: false,
    help: "The page, counted from 1. A bare number means the same: `60` or `--page 60`. Give this or `--ref`, not both.",
    ..PAGE_OR_BARE
};

/// Runs a pick of `select`, `copy` or `highlight` on the page a reference points at now. The tab is settled once, in `page_of`, and the pick goes on in it.
/// It is literally `--page P --chars S-E` for the page `P` found and the characters the reference names (a reference to a page or a spot picks the whole
/// page), with the page found put in the pick, so the read of that page checks the characters against the text it reads and no other read is made.
pub(crate) fn picking_by_reference<A: SessionAccess, T>(
    access: &A,
    base: crate::selecting::Pick<'_>,
    reference: &Locator,
    run: impl FnOnce(&crate::selecting::Pick<'_>) -> Result<T, AgentError>,
) -> Result<T, AgentError> {
    let found = locating::page_of(access, base.tab, reference, locating::CharsChecked::AgainstItsText)?;
    let tab = found.tab_ref();
    let chars = found.characters().map(|(start, end)| format!("{start}-{end}"));
    run(&crate::selecting::Pick {
        tab: &tab,
        page: Some(found.page),
        chars: chars.as_deref(),
        found: Some(&found),
        ..base
    })
}

/// The `--ref` flag of `select` and `copy`, and `highlight`.
pub(crate) const REF_PICK: Arg = Arg {
    key: "ref",
    flag: Some("--ref"),
    kind: Kind::Reference,
    required: false,
    help: "A reference `recto ref` gave, whole: takes the characters it points at, or the whole page if it points at a page or a spot, as that page stands now, even if another program moved it. Instead of `--page`, `--chars`, `--at` and `--all`. It refuses when the page is gone, can't be told from others, wasn't fully looked for, or its characters aren't the same (`recto resolve` says which). `highlight` takes only a reference to characters.",
};

/// The `--ref` flag of `goto`.
pub(crate) const REF_GO: Arg = Arg {
    key: "ref",
    flag: Some("--ref"),
    kind: Kind::Reference,
    required: false,
    help: "A reference `recto ref` gave, whole: goes to the page it points at now, even if another program moved it. Instead of a page. It refuses when the page is gone, can't be told from others or wasn't fully looked for (`recto resolve` says which). `--within` goes with it.",
};

pub(crate) const GOTO_REF_EXAMPLE: &str = concat!("recto goto --ref ", example_ref!());
pub(crate) const SELECT_REF_EXAMPLE: &str = concat!("recto select --ref ", example_ref!());
pub(crate) const HIGHLIGHT_REF_EXAMPLE: &str = concat!("recto highlight --ref ", example_ref_chars!());
pub(crate) const COPY_REF_EXAMPLE: &str = concat!("recto copy --ref ", example_ref!());
pub(crate) const SNAPSHOT_REF_EXAMPLE: &str = concat!("recto snapshot --ref ", example_ref!(), " --rect 72,72,300,200 --as text");
pub(crate) const EXPORT_SVG_REF_EXAMPLE: &str = concat!("recto export svg --ref ", example_ref!(), " --out page.svg");
pub(crate) const TEXT_REF_EXAMPLE: &str = concat!("recto text --ref ", example_ref!());
pub(crate) const RENDER_REF_EXAMPLE: &str = concat!("recto render --ref ", example_ref!(), " --out page.png");

/// `recto ref <page>`: a reference to a page of the tab, to give back to `resolve` later, when the file's page numbers may have changed.
#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub struct Ref {
    pub page: u32,
    #[serde(default)]
    pub tab: TabRef,
    /// `START-END` in characters of `recto text`, to refer to those characters of the page. Read in `run`, before the file is opened. After `tab`, so a
    /// positional line that has always read `[ref, page, tab]` still does.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chars: Option<String>,
}

impl Cmd for Ref {
    const TAG: &'static str = "ref";
    const ROWS: &'static [Spec] = ROWS_REF;
    // No worker draws a page, but a scan reads every page of a hostile file in this process, so it takes a place under the server's draw cap like any
    // other read of pages: at most a few run at once.
    const DRAWS: bool = true;
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
        let chars = self.chars.as_deref().map(reference_chars).transpose()?;
        locating::reference(cx.access, &self.tab, self.page, chars)
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
        let reference = token(&reference)?;
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
    const DRAWS: bool = true;
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

/// `--chars` of `ref`: a range of characters that can be referred to, or the words for why not. An empty range, or one ending past what a reference can
/// hold, is usage, whatever the page's text.
fn reference_chars(word: &str) -> Result<(u32, u32), AgentError> {
    let usage = || AgentError::new(Code::USAGE, locating::CHARS_USAGE, exit::USAGE);
    let (start, end) = crate::selecting::char_range(word).map_err(|_| usage())?;
    if start == end || end > locator::MAX_CHARACTER {
        return Err(usage());
    }
    Ok((start, end))
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
        Arg {
            key: "chars",
            flag: Some("--chars"),
            kind: Kind::Text,
            required: false,
            help: "`START-END` in characters of `recto text`, the end not included, at most 2097152: refers to those characters of the page, and checks them against the page's text, which must reach that far.",
        },
    ],
    data: "`{ tab, page, file_page, ref }`, and `chars: [start, end]` when `--chars` was given. `page` is the page as asked, in the tab as shown; `file_page` is its page of the file on disk, which differs only when there are unsaved edits. `ref` is the token: pass it whole to `resolve`. With `--chars` the token also holds a check of those characters, which `resolve` and `--ref` compare with the page's text at the same offsets.",
    examples: &["recto ref 3", "recto ref 3 --chars 120-180", "recto ref 3 --tab \"#2\" --json"],
    ..BASE
}];

pub(crate) const ROWS_RESOLVE: &[Spec] = &[Spec {
    name: "resolve",
    tag: "resolve",
    group: GROUP_READ,
    access: Access::At(Level::ReadOnly),
    summary: "Where a reference's page is now. The answer is a status, never a guess: `found` where the reference said, `text_changed` when a reference to characters finds its page but not its words, `moved` when the file's only page with that content is somewhere else, `ambiguous` when several have it, `gone` when every page was read and none has it, `incomplete` when the search couldn't read every page. All of those are exit 0, except that a reference to characters on a found page whose text can't be read is refused as `text` is for that page. A reference is data from outside: it grants no access, and only the tab's own file is read.",
    args: &[
        Arg {
            key: "ref",
            flag: None,
            kind: Kind::Reference,
            required: true,
            help: "A reference `recto ref` gave, whole.",
        },
        TAB,
    ],
    data: "`{ tab, status, page, file_page, was_page, candidates: [ { page, file_page } ], document, frame, target, looked_at, pages_read, note }`. `status` is `found`, `moved`, `text_changed`, `ambiguous`, `gone` or `incomplete`. `text_changed` is for a reference to characters: the page is found (`page` and `file_page` say where) but its text at those offsets isn't what the reference was made of, and nothing else on the page is searched. `page` (in the tab as shown, null when unsaved edits removed it) and `file_page` (of the file on disk) are only for `found`, `moved` and `text_changed`, counted from 1; `was_page` is where the reference pointed. `candidates` lists up to 8 pages for `ambiguous`, and the one match found for `incomplete`. `document` is `same`, `different` or `unknown`: information only, it never changes the status. `frame` says whether the page's crop and turn are the reference's. `target` is `{ kind: \"page\" }`, `{ kind: \"spot\", placed, rect | why }` or `{ kind: \"characters\", start, end, checked, text: \"same\" | \"changed\" }` (`text` only when checked). `looked_at` is `hint`, `all` or `part`.",
    examples: &[
        concat!("recto resolve ", example_ref!()),
        concat!("recto resolve ", example_ref!(), " --tab \"#2\" --json"),
    ],
    ..BASE
}];

#[cfg(test)]
mod tests {
    use super::*;

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

    /// Failure: the example token isn't the canonical text of a reference (it decodes, but `recto ref` would never write it that way). That every row's
    /// example is read by its command is `table::tests::every_example_parses_to_its_own_row`.
    #[test]
    fn the_rows_example_token_decodes() {
        for example in [EXAMPLE_REF, EXAMPLE_REF_CHARS] {
            let decoded = locator::decode(example).expect("the example decodes");
            assert_eq!(locator::encode(&decoded), example, "and is the canonical form of what it names");
        }
    }

    /// Failure: `ref` or `resolve` reads every page of a file with no limit on how many scans run at once, since the server's cap is of the commands that
    /// say they draw. That the cap holds at its limit plus one is `drawing_is_capped_and_other_work_goes_around_it` (recto-control); this is that these two
    /// commands are under it.
    #[test]
    fn ref_and_resolve_take_a_place_under_the_draw_cap() {
        let resolve = read(EXAMPLE_REF).expect("resolve");
        let reference = crate::parse(r##"{"cmd":"ref","page":1,"tab":"#1"}"##).expect("ref");
        assert!(resolve.draws() && reference.draws());
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

    /// A line for `text` or `render` (which also needs a place to write), with `fields` beside its `cmd`.
    fn ask(cmd: &str, fields: Value) -> Value {
        let mut line = json!({ "cmd": cmd });
        if cmd == "render" {
            line["out"] = json!("/d/x.png");
        }
        for (key, value) in fields.as_object().into_iter().flatten() {
            line[key] = value.clone();
        }
        line
    }

    fn read_line(line: Value) -> Result<Command, AgentError> {
        crate::parse(&line.to_string())
    }

    /// Failure: `text` or `render` takes both a page and a reference (so which one is read depends on which the code looks at first), or neither (so a
    /// bare `recto text` reads some page or falls through to a missing-field refusal that isn't about this), or a reference beside a page is accepted
    /// without being read. Both refuse as usage, in the command's own words; one or the other reads; and `ref: null` is no reference.
    #[test]
    fn text_and_render_take_a_page_or_a_ref_never_both_or_neither() {
        for cmd in ["text", "render"] {
            let line = |fields: Value| ask(cmd, fields);
            for refused in [json!({ "page": 3, "ref": EXAMPLE_REF }), json!({}), json!({ "page": null, "ref": null })] {
                let error = read_line(line(refused.clone())).expect_err(&refused.to_string());
                assert_eq!((error.code, error.exit), (Code::USAGE, exit::USAGE), "{cmd} {refused}");
                assert!(
                    error.message.contains(&format!("`{cmd}`")) && error.message.contains("`--ref`"),
                    "{}",
                    error.message
                );
            }
            let by_page = read_line(line(json!({ "page": 3, "ref": null }))).expect("a page");
            let by_ref = read_line(line(json!({ "ref": EXAMPLE_REF }))).expect("a reference");
            assert_eq!(
                (by_page.log_details()["page"].as_u64(), by_page.log_details().get("ref_page")),
                (Some(3), None),
                "{cmd}"
            );
            assert_eq!(
                (by_ref.log_details()["ref_page"].as_u64(), by_ref.log_details().get("page")),
                (Some(1), None),
                "{cmd}"
            );
            // What the window or a test builds prints as the line an agent could send.
            let again = crate::parse(&by_ref.wire()).expect("what it prints reads back");
            assert_eq!(again.log_details(), by_ref.log_details(), "{cmd}");
        }
    }

    /// Failure: a token that can't be a reference on `text` or `render` reaches the level or the tab (an agent at Off is told about its level, a hostile
    /// token is looked at beside the session), or its refusal repeats the hostile text. Refused as the line is read, with constant words, as `resolve`
    /// refuses; a newer version says so.
    #[test]
    fn a_hostile_ref_on_text_or_render_is_bad_reference_before_the_level_is_asked() {
        let long = format!("r1.{}", "a".repeat(126));
        for cmd in ["text", "render"] {
            for hostile in [long.as_str(), "r1.é", "", "nonsense"] {
                let error = read_line(ask(cmd, json!({ "ref": hostile }))).expect_err(hostile);
                assert_eq!((error.code, error.exit), (Code::BAD_REFERENCE, exit::USAGE), "{cmd} {hostile:?}");
                assert!(!error.message.contains("nonsense") && !error.message.contains("r1."), "{}", error.message);
            }
            let error = read_line(ask(cmd, json!({ "ref": "r2.040g2081040g2.p1-081040g2081040g2081040g208-0000000" }))).expect_err("r2");
            assert_eq!(error.code, Code::REFERENCE_TOO_NEW, "{cmd}");
        }
    }

    /// Failure: the activity log, or a command printed in a test failure, carries the token or a piece of its checks, which name a page's content, on
    /// `text` or `render`.
    #[test]
    fn text_and_render_ref_never_log_the_token() {
        for cmd in ["text", "render"] {
            let command = read_line(ask(cmd, json!({ "ref": EXAMPLE_REF }))).expect("example");
            let shown = format!("{:?} {}", command, command.log_details());
            for piece in ["040g2081040g2", "081040g2081040g2081040g208", "0000000"] {
                assert!(!shown.contains(piece), "{cmd}: {shown}");
            }
        }
    }

    /// Failure: a `--chars` that can't be a range (no dash, backwards, empty, past what a reference holds) reaches the file, so the agent is told a page or
    /// the file is wrong when its words were; or the cap itself is refused. It is usage in constant words, from the line alone.
    #[test]
    fn ref_chars_that_cannot_be_are_usage_before_the_file_is_read() {
        for hostile in ["5", "a-b", "9-3", "5-5", "0-0", "0-2097153", "-3", "1-2-3", ""] {
            let error = reference_chars(hostile).expect_err(hostile);
            assert_eq!((error.code, error.exit), (Code::USAGE, exit::USAGE), "{hostile:?}");
            assert_eq!(
                error.message,
                locating::CHARS_USAGE,
                "one wording for every shape that isn't a range: {hostile:?}"
            );
            assert!(!error.message.contains("2097153"), "the refusal says only its own words: {}", error.message);
        }
        assert_eq!(reference_chars("0-2097152").ok(), Some((0, 2_097_152)), "the cap itself is held");
        assert_eq!(reference_chars("120-180").ok(), Some((120, 180)));
    }

    /// Failure: the activity log, or a command printed in a test failure, carries more of a character reference than the page and tab (a token's checks,
    /// or text of the page).
    #[test]
    fn ref_with_chars_never_logs_the_range_text_or_token() {
        let command = crate::parse(r##"{"cmd":"ref","page":3,"chars":"120-180","tab":"#2"}"##).expect("ref with chars");
        let logged = command.log_details();
        assert_eq!(logged, json!({ "tab": "#2", "page": 3 }), "the page only: {logged}");
        assert!(
            crate::parse(&command.wire()).is_ok(),
            "what the window or a test builds prints as a line an agent could send"
        );
    }

    /// The six commands that take `--ref` beside `text` and `render`, each with the fields its line needs besides the reference.
    const REF_COMMANDS: [(&str, &str); 6] = [
        ("goto", "{}"),
        ("select", "{}"),
        ("copy", "{}"),
        ("highlight", "{}"),
        ("snapshot", r#"{"rect":[0,0,10,10],"as":"text"}"#),
        ("export_svg", r#"{"out":"/d/x.svg"}"#),
    ];

    /// A line for one of [`REF_COMMANDS`] with `fields` beside its own.
    fn line_of(cmd: &str, base: &str, fields: Value) -> Value {
        let mut line: Value = serde_json::from_str(base).expect("base");
        line["cmd"] = json!(cmd);
        for (key, value) in fields.as_object().into_iter().flatten() {
            line[key] = value.clone();
        }
        line
    }

    /// Failure: one of the commands that takes `--ref` reads a reference beside a page or a pick (so which one it goes by depends on which the code looks at
    /// first), or refuses one in words that say another command's, or refuses a line with only the reference. Both are usage in the command's own constant
    /// words; the reference alone reads; `ref: null` is no reference. `snapshot` and `export svg`, which need one of the two, say so when neither is given.
    #[test]
    fn every_ref_command_takes_the_reference_alone_and_refuses_it_beside_a_place() {
        let usage = |cmd: &str, base: &str, fields: Value| {
            let error = read_line(line_of(cmd, base, fields.clone())).map(|_| ()).expect_err(&format!("{cmd} {fields}"));
            assert_eq!((error.code, error.exit), (Code::USAGE, exit::USAGE), "{cmd} {fields}: {}", error.message);
            error.message
        };
        for (cmd, base) in REF_COMMANDS {
            read_line(line_of(cmd, base, json!({ "ref": EXAMPLE_REF_CHARS }))).unwrap_or_else(|error| panic!("{cmd} alone: {}", error.message));
            read_line(line_of(cmd, base, json!({ "ref": EXAMPLE_REF_CHARS, "tab": "#2" })))
                .unwrap_or_else(|error| panic!("{cmd} with a tab: {}", error.message));
            let nothing = read_line(line_of(cmd, base, json!({ "ref": null })));
            assert_eq!(nothing.is_err(), matches!(cmd, "goto" | "snapshot" | "export_svg"), "{cmd}: a null ref is none");
        }
        // Neither a page, a label nor a reference is refused as the line is read (before the level is asked), in its own words, whatever else came with it.
        for bare in [json!({}), json!({ "ref": null }), json!({ "within": 0.3 }), json!({ "link": true })] {
            assert_eq!(usage("goto", "{}", bare.clone()), GOTO_NEITHER, "{bare}");
        }
        assert_eq!(usage("goto", "{}", json!({ "page": 3, "ref": EXAMPLE_REF })), GOTO_REF_ALONE);
        for beside in [json!({ "label": "iv" }), json!({ "entry": 2 }), json!({ "link": true })] {
            let mut line = beside.clone();
            line["ref"] = json!(EXAMPLE_REF);
            assert_eq!(usage("goto", "{}", line), GOTO_REF_ALONE, "{beside}");
        }
        read_line(line_of("goto", "{}", json!({ "ref": EXAMPLE_REF, "within": 0.3 }))).expect("`--within` goes with a reference");
        for cmd in ["select", "copy", "highlight"] {
            for beside in [
                json!({ "page": 3 }),
                json!({ "to_page": 3 }),
                json!({ "chars": "0-4" }),
                json!({ "at": "1,2" }),
                json!({ "to": "1,2" }),
                json!({ "unit": "word" }),
            ] {
                let mut line = beside.clone();
                line["ref"] = json!(EXAMPLE_REF_CHARS);
                assert_eq!(usage(cmd, "{}", line), PICK_REF_ALONE, "{cmd} {beside}");
            }
        }
        for cmd in ["select", "copy"] {
            assert_eq!(usage(cmd, "{}", json!({ "all": true, "ref": EXAMPLE_REF })), PICK_REF_ALONE, "{cmd} --all");
        }
        let (snap, svg) = (REF_COMMANDS[4], REF_COMMANDS[5]);
        assert_eq!(usage(snap.0, snap.1, json!({ "page": 3, "ref": EXAMPLE_REF })), SNAPSHOT_BOTH);
        assert_eq!(usage(snap.0, snap.1, json!({})), SNAPSHOT_NEITHER);
        assert_eq!(usage(svg.0, svg.1, json!({ "page": 3, "ref": EXAMPLE_REF })), EXPORT_SVG_BOTH);
        assert_eq!(usage(svg.0, svg.1, json!({})), EXPORT_SVG_NEITHER);
    }

    /// Failure: `highlight --ref` takes a reference to a page or a spot (an `also_spelled` hit's, say) and highlights the whole page, an edit nobody asked for;
    /// or `snapshot --ref` goes to the person's clipboard, which is made for the page they saw. Both are refused as the line is read, in their own words.
    #[test]
    fn highlight_takes_only_characters_and_a_snapshot_by_ref_never_goes_to_the_clipboard() {
        let spot = locator::encode(&made_up_locator(locator::Target::Spot(locator::Spot::point(10, 20).expect("a point"))));
        for token in [EXAMPLE_REF, spot.as_str()] {
            let error = read_line(line_of("highlight", "{}", json!({ "ref": token })))
                .map(|_| ())
                .expect_err("not characters");
            assert_eq!((error.code, error.message.as_str()), (Code::USAGE, HIGHLIGHT_REF_CHARS), "{token}");
        }
        read_line(line_of("highlight", "{}", json!({ "ref": EXAMPLE_REF_CHARS }))).expect("characters");
        let error = read_line(line_of(
            "snapshot",
            r#"{"rect":[0,0,10,10],"as":"png"}"#,
            json!({ "ref": EXAMPLE_REF, "to_clipboard": true }),
        ))
        .map(|_| ())
        .expect_err("not the clipboard");
        assert_eq!((error.code, error.message.as_str()), (Code::USAGE, SNAPSHOT_REF_NOT_CLIPBOARD));
    }

    /// Failure: a token that can't be a reference, on one of the six, reaches the level or the tab (an agent at Off is told about its level, a hostile token
    /// is looked at beside the session), or is judged only after the combination (a page beside it is "both" and not "bad reference"), or its refusal repeats
    /// it. Refused as the line is read, with constant words; a newer version says so.
    #[test]
    fn a_hostile_ref_on_any_ref_command_is_bad_reference_before_the_level_or_the_combination() {
        let long = format!("r1.{}", "a".repeat(126));
        for (cmd, base) in REF_COMMANDS {
            for hostile in [long.as_str(), "r1.é", "", "nonsense"] {
                for beside in [json!({}), json!({ "page": 3 })] {
                    let mut line = beside.clone();
                    line["ref"] = json!(hostile);
                    let error = read_line(line_of(cmd, base, line)).map(|_| ()).expect_err(hostile);
                    assert_eq!((error.code, error.exit), (Code::BAD_REFERENCE, exit::USAGE), "{cmd} {hostile:?} {beside}");
                    assert!(!error.message.contains("nonsense") && !error.message.contains("r1."), "{}", error.message);
                }
            }
            let error = read_line(line_of(cmd, base, json!({ "ref": "r2.040g2081040g2.p1-081040g2081040g2081040g208-0000000" })))
                .map(|_| ())
                .expect_err("r2");
            assert_eq!(error.code, Code::REFERENCE_TOO_NEW, "{cmd}");
        }
    }

    /// Failure: the activity log, or a command printed in a test failure, carries the token or a piece of its checks (which name a page's content) on one of
    /// the six, or loses the page it hints at; or what the window or a test builds prints as a line an agent couldn't send.
    #[test]
    fn every_ref_command_logs_only_the_page_its_reference_hints_at() {
        for (cmd, base) in REF_COMMANDS {
            let command = read_line(line_of(cmd, base, json!({ "ref": EXAMPLE_REF_CHARS }))).expect("example");
            let shown = format!("{:?} {}", command, command.log_details());
            for piece in ["040g2081040g2", "081040g2081040g2081040g208", "0000000", "c1g6"] {
                assert!(!shown.contains(piece), "{cmd}: {shown}");
            }
            assert_eq!(command.log_details()["ref_page"], 1, "{cmd}: {}", command.log_details());
            let again = crate::parse(&command.wire()).unwrap_or_else(|error| panic!("{cmd}: what it prints reads back: {}", error.message));
            assert_eq!(again.log_details(), command.log_details(), "{cmd}");
        }
    }

    /// Failure: a `--ref` line takes no place under the draw cap, or is run on the window's session thread, or reads no pages while it scans a hostile file for
    /// its page (the cap and the gate are by what a line says it does), or a plain `goto 3`, which reads nothing, is held to that. Each `--ref` line draws,
    /// waits and reads pages; `goto` by number draws, waits and reads none.
    #[test]
    fn each_ref_line_draws_waits_and_reads_pages_and_goto_by_number_does_none() {
        for (cmd, base) in REF_COMMANDS {
            let command = read_line(line_of(cmd, base, json!({ "ref": EXAMPLE_REF_CHARS }))).expect("example");
            assert_eq!((command.draws(), command.waits(), command.reads_pages()), (true, true, true), "{cmd}");
        }
        let by_number = read_line(json!({ "cmd": "goto", "page": 3 })).expect("goto 3");
        assert_eq!((by_number.draws(), by_number.waits(), by_number.reads_pages()), (false, false, false));
    }
}
