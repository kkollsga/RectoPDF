//! Going to a page, links within a document, and the trail through them: `goto`, `goto label`, `links`, `follow`, `back` and `forward`.

#![allow(unused_imports)]
use super::{Cmd, Cx, row_access};
use crate::Code;
use crate::human::*;
use crate::table::*;
use crate::*;
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

/// A jump to a page, by its number or by what the file calls it. Back comes here again.
#[derive(Clone, Debug, Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub struct Goto {
    #[serde(default)]
    pub tab: TabRef,
    #[serde(default)]
    pub page: Option<u32>,
    #[serde(default)]
    pub label: Option<String>,
    /// How far down the page, from 0, its top, to 1, its bottom, as a link's destination says it (`LinkTarget::Page`'s `within`): a click on a link sends
    /// it. Past either end is that end, as a link's is.
    #[serde(default)]
    pub within: Option<f64>,
}

impl Cmd for Goto {
    const TAG: &'static str = "goto";
    const ROWS: &'static [Spec] = ROWS_GOTO;
    const DRAWS: bool = false;
    const READS_PAGES: bool = false;
    const PROTECTED_OK: bool = false;
    const ABOUT: super::About = super::About::Its;
    const WAITS: bool = false;
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
        let Self { tab, page, label, within } = self;
        json!({ "tab": super::log_tab(tab), "page": page, "by_label": label.is_some(), "within": within })
    }
    fn run<A: SessionAccess>(&self, cx: &mut Cx<'_, A>) -> Result<Value, AgentError> {
        cx.access.take_places();
        match (self.page, &self.label) {
            (Some(page), None) => cx.access.with(|s| {
                let id = select(s, &self.tab)?;
                go_to(s, id, page, self.within.unwrap_or(0.0), cx.actor.author())?;
                cx.events.push(page_now(s, id));
                Ok(where_json(s, id))
            }),
            (None, Some(label)) if self.within.is_none() => linking::goto_label(cx.access, cx.actor.author(), &self.tab, label, cx.events),
            (None, Some(_)) => Err(AgentError::new(
                Code::USAGE,
                "`--within` is how far down a page, and goes with a page number, not a label.",
                exit::USAGE,
            )
            .then("`recto goto 12 --within 0.3`.")),
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
        linking::step(access, cx.actor.author(), tab, true, events)
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
        linking::step(access, cx.actor.author(), tab, false, events)
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
        linking::follow(access, cx.actor.author(), tab, *page, *link, events)
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
            Arg {
                key: "within",
                flag: Some("--within"),
                kind: Kind::Fraction,
                required: false,
                help: "How far down the page to land, from 0, its top, to 1, its bottom, as a link's destination says. The top if left out.",
            },
            TAB,
        ],
        data: AGENT_EXITS_PAGE,
        emits: &["page_changed"],
        examples: &["recto goto 12", "recto goto 12 --tab report", "recto goto 9 --within 0.3"],
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
}
