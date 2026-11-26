//! Going to a page, links within a document, and the trail through them: `goto`, `goto label`, `links`, `follow`, `back` and `forward`.

#![allow(unused_imports)]
use super::locating::{GOTO_REF_EXAMPLE, REF_GO};
use super::{Cmd, Cx, row_access};
use crate::Code;
use crate::human::*;
use crate::table::*;
use crate::*;
use recto_core::locator::Locator;
use recto_session::trail::Chosen;
use serde::Deserialize;
use serde_json::{Value, json};
use std::fmt::Write;
use std::path::PathBuf;

/// A page's links, and where each goes.
#[derive(Clone, Debug, Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub struct Links {
    #[serde(default)]
    pub tab: TabRef,
    pub page: u32,
}

impl Cmd for Links {
    const TAG: &'static str = "links";
    const ROWS: &'static [Spec] = ROWS_LINKS;
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

/// A jump to a page, by its number, by what the file calls it, or by a reference to it. Back comes here again. Read from a `line`, so a token that can't be a
/// reference, or one beside a page, a label, an entry or a link, is refused as the line is read, before the level or the tab is asked.
#[derive(Clone, serde::Serialize)]
#[serde(into = "line::Goto")]
pub struct Goto {
    pub tab: TabRef,
    pub page: Option<u32>,
    pub label: Option<String>,
    /// How far down the page, from 0, its top, to 1, its bottom, as a link's destination says it (`LinkTarget::Page`'s `within`): a click on a link sends
    /// it. Past either end is that end, as a link's is.
    pub within: Option<f64>,
    /// The contents entry this goes to, by its number as `contents` gives it, as a click on its row does: it's the one marked as being read from when the
    /// view lands until the reader scrolls or jumps again, even with another entry at the same place. Refused unless it's an entry of the contents the
    /// sidebar shows for the tab, on that page. Left out of what's sent when there's none.
    pub entry: Option<u32>,
    /// **A link's jump** (the window's click on a link to a page of this document): where it goes is tied to the page it was clicked on, so it's chosen
    /// from where the tab is, and a place withheld from agents stays withheld, whoever sends it ([`recto_session::trail::Chosen::FromPlace`], the fourth
    /// review's item 2). Anyone may send it: it only ever keeps a place withheld. Left out of what's sent when false.
    pub link: bool,
    /// The page a reference to it points at now, in place of `page`. Never with a page, a label, an entry or a link.
    pub reference: Option<Locator>,
}

/// `goto` as it's written on the wire.
pub mod line {
    use crate::TabRef;
    use serde::{Deserialize, Serialize};

    #[derive(Clone, Deserialize, Serialize)]
    #[serde(deny_unknown_fields)]
    pub struct Goto {
        #[serde(default)]
        pub tab: TabRef,
        #[serde(default)]
        pub page: Option<u32>,
        #[serde(default)]
        pub label: Option<String>,
        #[serde(default)]
        pub within: Option<f64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub entry: Option<u32>,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        pub link: bool,
        /// Last, so a positional line that has always read `[tab, page, label, within, entry, link]` still does.
        #[serde(default, rename = "ref", skip_serializing_if = "Option::is_none")]
        pub reference: Option<String>,
    }
}

impl TryFrom<line::Goto> for Goto {
    type Error = AgentError;
    fn try_from(line: line::Goto) -> Result<Self, AgentError> {
        let line::Goto {
            tab,
            page,
            label,
            within,
            entry,
            link,
            reference,
        } = line;
        let others = page.is_some() || label.is_some() || entry.is_some() || link;
        let reference = super::locating::reference_alone(reference.as_deref(), others, super::locating::GOTO_REF_ALONE)?;
        if page.is_none() && label.is_none() && reference.is_none() {
            return Err(AgentError::new(Code::USAGE, super::locating::GOTO_NEITHER, exit::USAGE)
                .then("`recto goto 12`, `recto goto label iv` or `recto goto --ref <token>`."));
        }
        Ok(Self {
            tab,
            page,
            label,
            within,
            entry,
            link,
            reference,
        })
    }
}

impl From<Goto> for line::Goto {
    fn from(asked: Goto) -> Self {
        let Goto {
            tab,
            page,
            label,
            within,
            entry,
            link,
            reference,
        } = asked;
        Self {
            tab,
            page,
            label,
            within,
            entry,
            link,
            reference: reference.as_ref().map(recto_core::locator::encode),
        }
    }
}

/// Said as it's sent: `entry` and `link` only when there's one, so a `goto` that names neither reads as it always has, in a log and the wire's golden alike.
/// A reference shows only the page it hints at, never the token.
impl std::fmt::Debug for Goto {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let Self {
            tab,
            page,
            label,
            within,
            entry,
            link,
            reference,
        } = self;
        let mut said = f.debug_struct("Goto");
        said.field("tab", tab).field("page", page).field("label", label).field("within", within);
        if entry.is_some() {
            said.field("entry", entry);
        }
        if *link {
            said.field("link", link);
        }
        if let Some(reference) = reference {
            said.field("ref_page", &reference.page().number());
        }
        said.finish()
    }
}

impl Cmd for Goto {
    const TAG: &'static str = "goto";
    const ROWS: &'static [Spec] = ROWS_GOTO;
    const DRAWS: bool = false;
    const READS_PAGES: bool = false;
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
        let Self {
            tab,
            page,
            label,
            within,
            entry,
            link,
            reference,
        } = self;
        let mut details = json!({ "tab": super::log_tab(tab), "page": page, "by_label": label.is_some(), "within": within });
        if let Some(entry) = entry {
            details["entry"] = json!(entry);
        }
        if *link {
            details["link"] = json!(true);
        }
        // The page it hints at, a number, and never the token or any part of its checks.
        if let Some(reference) = reference {
            details["ref_page"] = json!(reference.page().number());
        }
        details
    }
    /// A jump by reference scans the file's pages (so it takes a place under the draw cap, reads pages, and isn't run on the window's session thread);
    /// one by number or label reads nothing.
    fn draws_as_asked(&self) -> bool {
        self.reference.is_some()
    }
    fn waits_as_asked(&self) -> bool {
        self.reference.is_some()
    }
    fn reads_pages_as_asked(&self) -> bool {
        self.reference.is_some()
    }
    fn run<A: SessionAccess>(&self, cx: &mut Cx<'_, A>) -> Result<Value, AgentError> {
        if let Some(reference) = &self.reference {
            return self.go_by_reference(cx, reference);
        }
        cx.access.take_places();
        match (self.page, &self.label) {
            (Some(page), None) => cx.access.with(|s| {
                let id = select(s, &self.tab)?;
                // A page that's there, and the entry not on it: a page that isn't there is `go_to`'s to refuse.
                let at = page.checked_sub(1).filter(|at| *at < s.pages().count(id));
                if let (Some(entry), Some(at)) = (self.entry, at)
                    && !s.tabs().tab(id).is_some_and(|tab| recto_session::entry_is_on(s, tab, entry, at))
                {
                    return Err(not_that_entry(entry, page));
                }
                let chosen = if self.link { Chosen::FromPlace } else { Chosen::ByItself };
                go_to(s, id, page, self.within.unwrap_or(0.0), cx.actor.author(), chosen)?;
                if let Some(entry) = self.entry {
                    s.mail_mut().jump_is_to_entry(id, entry);
                }
                cx.events.push(page_now(s, id));
                Ok(where_json(s, cx.actor, id))
            }),
            (None, Some(label)) if self.within.is_none() && self.entry.is_none() && !self.link => {
                linking::goto_label(cx.access, cx.actor, &self.tab, label, cx.events)
            }
            (None, Some(_)) => Err(AgentError::new(
                Code::USAGE,
                "`--within`, `--entry` and `--link` go with a page number, not a label.",
                exit::USAGE,
            )
            .then("`recto goto 12 --within 0.3`.")),
            (None, None) => Err(AgentError::new(Code::USAGE, super::locating::GOTO_NEITHER, exit::USAGE)
                .then("`recto goto 12`, `recto goto label iv` or `recto goto --ref <token>`.")),
            _ => Err(AgentError::new(
                Code::USAGE,
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

impl Goto {
    /// Goes to the page a reference points at now, as the tab has it among its edits. Anything but `found` and `moved` is `page_of`'s refusal, and nothing
    /// is done then: no jump, no notice.
    fn go_by_reference<A: SessionAccess>(&self, cx: &mut Cx<'_, A>, reference: &Locator) -> Result<Value, AgentError> {
        let found = locating::page_of(cx.access, &self.tab, reference, locating::CharsChecked::First)?;
        locating::still_located(cx.access, &found)?;
        let within = self.within.unwrap_or(0.0);
        // Taken in last, after a scan that can take seconds, so the trail marks the page showing when the jump is made.
        cx.access.take_places();
        let (actor, events) = (cx.actor, &mut *cx.events);
        cx.access.with(|s| {
            let id = found.tab;
            let source = s.page_source(id, found.page - 1).ok_or_else(no_such_page)?;
            locating::agree(&found, &source)?;
            go_to(s, id, found.page, within, actor.author(), Chosen::ByItself)?;
            events.push(page_now(s, id));
            let mut data = where_json(s, actor, id);
            data["ref"] = found.told();
            Ok(data)
        })
    }
}

/// The refusal of `goto --entry` for an entry that isn't one of the contents shown, on that page.
fn not_that_entry(entry: u32, page: u32) -> AgentError {
    AgentError::new(
        Code::NO_SUCH_ENTRY,
        format!("Entry {entry} isn't one of the contents the sidebar shows on page {page}."),
        exit::NOT_FOUND,
    )
    .then("`recto contents` gives each entry's number and page (`contents --generated` when generated contents are shown).")
}

#[derive(Clone, Debug, Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub struct Back {
    #[serde(default)]
    pub tab: TabRef,
}

impl Cmd for Back {
    const TAG: &'static str = "back";
    const ROWS: &'static [Spec] = ROWS_BACK;
    const DRAWS: bool = false;
    const READS_PAGES: bool = false;
    const PROTECTED_OK: bool = false;
    const ABOUT: super::About = super::About::Its;
    const WAITS: bool = false;
    fn starts_from(&self) -> super::FromPlace {
        super::FromPlace::Trail { back: true }
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
        let Self { tab } = self;
        json!({ "tab": super::log_tab(tab) })
    }
    fn run<A: SessionAccess>(&self, cx: &mut Cx<'_, A>) -> Result<Value, AgentError> {
        let Self { tab } = self;
        let (access, events) = (cx.access, &mut *cx.events);
        access.take_places();
        linking::step(access, cx.actor, tab, true, events)
    }
    fn human(data: &Value) -> String {
        place(data)
    }
}

#[derive(Clone, Debug, Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub struct Forward {
    #[serde(default)]
    pub tab: TabRef,
}

impl Cmd for Forward {
    const TAG: &'static str = "forward";
    const ROWS: &'static [Spec] = ROWS_FORWARD;
    const DRAWS: bool = false;
    const READS_PAGES: bool = false;
    const PROTECTED_OK: bool = false;
    const ABOUT: super::About = super::About::Its;
    const WAITS: bool = false;
    fn starts_from(&self) -> super::FromPlace {
        super::FromPlace::Trail { back: false }
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
        let Self { tab } = self;
        json!({ "tab": super::log_tab(tab) })
    }
    fn run<A: SessionAccess>(&self, cx: &mut Cx<'_, A>) -> Result<Value, AgentError> {
        let Self { tab } = self;
        let (access, events) = (cx.access, &mut *cx.events);
        access.take_places();
        linking::step(access, cx.actor, tab, false, events)
    }
    fn human(data: &Value) -> String {
        place(data)
    }
}

/// Follows a link within the document, as a click does. Never a web address.
#[derive(Clone, Debug, Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub struct Follow {
    #[serde(default)]
    pub tab: TabRef,
    pub page: u32,
    pub link: u32,
}

impl Cmd for Follow {
    const TAG: &'static str = "follow";
    const ROWS: &'static [Spec] = ROWS_FOLLOW;
    const DRAWS: bool = false;
    const READS_PAGES: bool = true;
    const PROTECTED_OK: bool = false;
    const ABOUT: super::About = super::About::Its;
    // It reads the page's links from its file in place.
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
        let Self { tab, page, link } = self;
        json!({ "tab": super::log_tab(tab), "page": page, "link": link })
    }
    fn run<A: SessionAccess>(&self, cx: &mut Cx<'_, A>) -> Result<Value, AgentError> {
        let Self { tab, page, link } = self;
        let (access, events) = (cx.access, &mut *cx.events);
        linking::follow(access, cx.actor, tab, *page, *link, events)
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
        summary: "Goes to a page. A page that isn't there is refused. `recto back` comes back to where it was. With `--ref` it goes to the page a reference points at now, even if another program moved it, and `ref` in the answer says how it was found; anything but a page that was found or moved is refused, and nothing moves.",
        args: &[
            Arg {
                key: "page",
                flag: None,
                kind: Kind::Page,
                required: false,
                help: "The page, counted from 1. Give this or `--ref`, not both.",
            },
            REF_GO,
            Arg {
                key: "within",
                flag: Some("--within"),
                kind: Kind::Fraction,
                required: false,
                help: "How far down the page to land, from 0, its top, to 1, its bottom, as a link's destination says. The top if left out.",
            },
            Arg {
                key: "entry",
                flag: Some("--entry"),
                kind: Kind::Count(100_000),
                required: false,
                help: "The contents entry this goes to, by its number as `contents` gives it, as a click on its row does: it's the one marked as being read until the tab is scrolled or jumps again. It has to be on that page.",
            },
            TAB,
        ],
        data: AGENT_EXITS_PAGE,
        emits: &["page_changed"],
        examples: &["recto goto 12", "recto goto 12 --tab report", "recto goto 9 --within 0.3", GOTO_REF_EXAMPLE],
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

#[cfg(test)]
mod tests {
    use crate::*;
    use recto_core::{DocFacts, OpenError, Toc};
    use recto_session::DiskLoader;
    use std::path::Path;
    use std::sync::Mutex;

    #[derive(Clone)]
    struct Fake;

    impl DocLoader for Fake {
        fn load(&self, _path: &Path) -> Result<DocFacts, OpenError> {
            Ok(DocFacts {
                page_count: 12,
                byte_len: 1,
                encrypted: false,
                version: "1.7".into(),
                title: None,
                toc: Toc::default(),
                renderer_only: false,
            })
        }
    }

    /// A session with a window whose page view has scrolled to a page the session hasn't heard of yet, which it gives when asked.
    struct Scrolled {
        session: Mutex<Session<Fake>>,
        showing: Mutex<Option<(TabId, u32)>>,
    }

    impl SessionAccess for Scrolled {
        type Loader = Fake;
        fn with<R>(&self, step: impl FnOnce(&mut Session<Fake>) -> R) -> R {
            self.session.with(step)
        }
        fn take_places(&self) {
            if let Some((id, page)) = self.showing.lock().unwrap().take() {
                self.session.lock().unwrap().go_to(id, page).unwrap();
            }
        }
    }

    impl Scrolled {
        /// The reader has scrolled to this page (counted from 1), and the session's not heard.
        fn scroll_to(&self, id: TabId, page: u32) {
            *self.showing.lock().unwrap() = Some((id, page - 1));
        }

        fn run(&self, line: &str) -> Value {
            execute(self, Actor::Agent(Level::Control), &parse(line).unwrap(), &mut Vec::new()).unwrap_or_else(|error| panic!("{line}: {error:?}"))
        }
    }

    /// Failure (R2.3, accident A1): an agent's `goto`, `goto label`, `back`, `forward` or `follow` marks the trail with where the session last heard the
    /// reader was, not the page showing, so Back after it returns to a stale place the reader had scrolled away from. Each takes in the page view's place
    /// first, as the window's own jumps do, and the trail holds the page that was showing.
    #[test]
    fn an_agents_jump_marks_the_page_that_was_showing() {
        let file = recto_testpdf::TempPdf::new("linked.pdf", &recto_testpdf::linked_document());
        let access = Scrolled {
            session: Mutex::new(Session::new(Fake)),
            showing: Mutex::new(None),
        };
        let id = access.session.lock().unwrap().open(&file.0).unwrap();
        let page = |answer: Value| answer["page"].as_u64().unwrap();
        for jump in [
            r#"{"cmd":"goto","page":9}"#,
            r#"{"cmd":"goto","label":"9"}"#,
            r#"{"cmd":"follow","page":1,"link":1}"#,
        ] {
            access.run(r#"{"cmd":"goto","page":1}"#);
            access.scroll_to(id, 5);
            access.run(jump);
            assert_eq!(page(access.run(r#"{"cmd":"back"}"#)), 5, "{jump}: Back comes back to the page that was showing");
        }
        // Back from 9 with the reader scrolled on to 11: Forward goes to 11, where they were.
        access.run(r#"{"cmd":"goto","page":9}"#);
        access.scroll_to(id, 11);
        access.run(r#"{"cmd":"back"}"#);
        assert_eq!(
            page(access.run(r#"{"cmd":"forward"}"#)),
            11,
            "back: the way forward is the page that was showing"
        );
        // Forward from there with the reader scrolled back to 3: Back goes to 3.
        access.run(r#"{"cmd":"back"}"#);
        access.scroll_to(id, 3);
        access.run(r#"{"cmd":"forward"}"#);
        assert_eq!(page(access.run(r#"{"cmd":"back"}"#)), 3, "forward: the way back is the page that was showing");
    }

    /// A session on real files that notes, in order, each time a command reads the session and each time it takes in the page view's place.
    struct Noting {
        session: Mutex<Session<DiskLoader>>,
        calls: Mutex<Vec<&'static str>>,
    }

    impl SessionAccess for Noting {
        type Loader = DiskLoader;
        fn with<R>(&self, step: impl FnOnce(&mut Session<DiskLoader>) -> R) -> R {
            self.calls.lock().unwrap().push("with");
            self.session.with(step)
        }
        fn take_places(&self) {
            self.calls.lock().unwrap().push("take_places");
        }
    }

    /// Failure: `goto --ref` takes in the page view's place before it scans the file for the page, which can take seconds, so a reader who scrolls during
    /// the scan is marked on the trail as where they were before it, and Back returns there instead of to the page that was showing when it jumped.
    /// The place is taken in after the scan's reads of the tab.
    #[test]
    fn a_goto_by_reference_takes_in_the_page_showing_just_before_it_jumps_not_before_its_scan() {
        let pages: Vec<recto_testpdf::Page> = (1..=9)
            .map(|page| recto_testpdf::Page::new(recto_testpdf::text_lines(&[&format!("Mine page {page}")])))
            .collect();
        let file = recto_testpdf::TempPdf::new("nine.pdf", &recto_testpdf::pages(&pages));
        let access = Noting {
            session: Mutex::new(Session::new(DiskLoader)),
            calls: Mutex::new(Vec::new()),
        };
        access.session.lock().unwrap().open(&file.0).unwrap();
        let made = crate::locating::reference(&access, &TabRef::Active, 7, None).expect("a reference to page 7");
        let line = format!(r#"{{"cmd":"goto","ref":"{}"}}"#, made["ref"].as_str().expect("its token"));
        // Where the place is taken in, counted in reads of the session before it: a plain jump reads nothing for a scan.
        let reads_before_taking_in = |line: &str| {
            access.calls.lock().unwrap().clear();
            execute(&access, Actor::Agent(Level::Control), &parse(line).unwrap(), &mut Vec::new()).expect("went");
            let calls = access.calls.lock().unwrap().clone();
            calls
                .iter()
                .position(|call| *call == "take_places")
                .unwrap_or_else(|| panic!("the place is taken in: {calls:?}"))
        };
        let plain = reads_before_taking_in(r#"{"cmd":"goto","page":7}"#);
        let by_reference = reads_before_taking_in(&line);
        assert!(
            by_reference >= plain + 2,
            "the scan's reads of the tab (2) come before the place is taken in: {by_reference} reads, a plain jump {plain}"
        );
    }

    /// Failure: `goto --within` takes a place on the page from a link's destination, which comes from the PDF, so a place past the page's end lands off
    /// it, or one with a label lands somewhere unsaid; or the CLI lets through what the wire would clamp without a word. It lands that far down, clamped
    /// to the page as a link's is; with a label it's refused; and the CLI refuses a share outside 0 to 1.
    #[test]
    fn goto_lands_that_far_down_the_page_and_no_further() {
        let mut session = Session::new(Fake);
        let id = session.open(Path::new("/d/a.pdf")).unwrap();
        let access = Mutex::new(session);
        let run = |line: &str| execute(&access, Actor::Agent(Level::Control), &parse(line).unwrap(), &mut Vec::new());
        // Where the page view's told to land.
        let here = || {
            let stop = access.lock().unwrap().mail_mut().take_jump(id).unwrap();
            (stop.page, stop.within)
        };
        run(r#"{"cmd":"goto","page":3,"within":0.4}"#).unwrap();
        assert_eq!(here(), (2, 0.4));
        run(r#"{"cmd":"goto","page":4,"within":7}"#).unwrap();
        assert_eq!(here(), (3, 1.0), "past the bottom is the bottom");
        assert_eq!(run(r#"{"cmd":"goto","label":"2","within":0.5}"#).unwrap_err().code, "usage");
        let words = |words: &[&str]| table::parse_cli(&words.iter().map(|word| (*word).to_string()).collect::<Vec<_>>()).map(|line| line.wire());
        assert!(words(&["goto", "3", "--within", "1.5"]).is_err());
        assert!(words(&["goto", "3", "--within", "0.25"]).unwrap().contains("0.25"));
    }

    /// Failure: `goto` with no page, no label and no reference moves the tab somewhere (page 1, say) or fails with a message that isn't about what's missing,
    /// now that the line is read with none of the three required. It is usage, with what to type, and nothing moves.
    #[test]
    fn goto_with_nothing_to_go_to_is_usage_and_moves_nothing() {
        let mut session = Session::new(Fake);
        let id = session.open(Path::new("/d/a.pdf")).unwrap();
        let access = Mutex::new(session);
        // As the line is read, in its own words, before any level is asked.
        let error = parse(r#"{"cmd":"goto"}"#).map(|_| ()).unwrap_err();
        assert_eq!(
            (error.code.as_str(), error.exit, error.message.as_str()),
            ("usage", exit::USAGE, crate::commands::locating::GOTO_NEITHER)
        );
        // One built in Rust, which never passes through the line, is refused when it runs.
        let bare = super::Goto {
            tab: TabRef::default(),
            page: None,
            label: None,
            within: None,
            entry: None,
            link: false,
            reference: None,
        };
        let error = crate::commands::Cmd::run(
            &bare,
            &mut crate::commands::Cx {
                access: &access,
                actor: Actor::Agent(Level::Control),
                events: &mut Vec::new(),
            },
        )
        .unwrap_err();
        assert_eq!(
            (error.code.as_str(), error.exit, error.message.as_str()),
            ("usage", exit::USAGE, crate::commands::locating::GOTO_NEITHER),
            "in the same words as the line's refusal, not \"and not both\""
        );
        assert!(access.lock().unwrap().mail_mut().take_jump(id).is_none(), "nothing was told to move");
    }
}
