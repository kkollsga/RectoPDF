//! `compare`: tabs side by side, with separate or synced scroll.

use super::words::OnOff;
use super::{Cmd, Cx, row_access};
use crate::Code;
use crate::human::text;
use crate::table::*;
use crate::*;
use serde_json::{Value, json};

/// Tabs side by side: which, the one being read, and separate or synced scroll. With nothing, only says. One thing at a time, but for tabs put side by
/// side and synced in one go: what can't go together can't be made.
#[derive(Clone, Default, PartialEq, Eq, serde::Serialize)]
#[serde(into = "line::Compare")]
pub enum Compare {
    /// Only says.
    #[default]
    Look,
    /// Back to one tab, the one being read.
    Off,
    /// These tabs side by side, left to right, and their scroll synced or not if said.
    Set { tabs: Panes, sync: Option<OnOff> },
    /// Synced scroll, or separate, for the comparison there is.
    Sync(OnOff),
    /// One tab into the comparison, or out of it, as ⌘-clicking its row does.
    Toggle(TabRef),
    /// The pane to read, of those side by side, as moving the pointer into it does.
    Reading(TabRef),
}

/// `compare` as it's written on the wire.
pub mod line {
    use serde::{Deserialize, Serialize};

    #[derive(Clone, Debug, Default, Deserialize, Serialize)]
    #[serde(deny_unknown_fields)]
    pub struct Compare {
        /// Tabs by selector, with commas between: `#1,#3`.
        #[serde(default)]
        pub tabs: Option<String>,
        /// `on` for synced scroll, `off` for separate.
        #[serde(default)]
        pub sync: Option<String>,
        #[serde(default)]
        pub off: bool,
        #[serde(default)]
        pub toggle: Option<String>,
        #[serde(default)]
        pub reading: Option<String>,
    }
}

/// The most selectors `compare --tabs` reads, so a long list can't be made to look up thousands of names. More than there can be panes, so too many is said in words.
const MAX_COMPARE_SELECTORS: usize = 8;

/// The tabs `compare --tabs` names, left to right. Made by [`Panes::parse`] from the words, or [`Panes::new`] from tabs, each of which keeps to what a list
/// can say: at most [`MAX_COMPARE_SELECTORS`], and no name with a comma or with spaces at its ends, or empty, which the list's words would read back as
/// other tabs (review of R2.5, F1). So a compare that exists prints as a line that reads back as itself.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct Panes(Vec<TabRef>);

impl Panes {
    /// These tabs, if a list can say them: `None` past [`MAX_COMPARE_SELECTORS`], or for a name the list's words can't hold.
    #[must_use]
    pub fn new(tabs: Vec<TabRef>) -> Option<Self> {
        let sayable = |tab: &TabRef| {
            let words = tab.to_string();
            !words.is_empty() && words.trim() == words && !words.contains(',')
        };
        (tabs.len() <= MAX_COMPARE_SELECTORS && tabs.iter().all(sayable)).then_some(Self(tabs))
    }

    /// The tabs a list names, by selector with commas between (`#1,#3`); spaces around each and empty ones are let go. Past [`MAX_COMPARE_SELECTORS`]
    /// is refused in the session's words, having read no more than one past it.
    pub fn parse(list: &str) -> Result<Self, AgentError> {
        let tabs: Vec<TabRef> = list
            .split(',')
            .map(str::trim)
            .filter(|word| !word.is_empty())
            .take(MAX_COMPARE_SELECTORS + 1)
            .map(TabRef::parse)
            .collect();
        if tabs.len() > MAX_COMPARE_SELECTORS {
            return Err(refused(recto_session::compare::CompareError::TooMany));
        }
        Ok(Self(tabs))
    }

    #[must_use]
    pub fn as_slice(&self) -> &[TabRef] {
        &self.0
    }
}

/// Its words in the list: the selectors with commas between.
impl std::fmt::Display for Panes {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for (at, tab) in self.0.iter().enumerate() {
            if at > 0 {
                f.write_str(",")?;
            }
            write!(f, "{tab}")?;
        }
        Ok(())
    }
}

/// Checked whole as it's read, in the order the run checked it before: what goes alone, then the scroll's word, then how many tabs are named.
impl TryFrom<line::Compare> for Compare {
    type Error = AgentError;
    fn try_from(line: line::Compare) -> Result<Self, AgentError> {
        let usage = |words: &str| AgentError::new(Code::USAGE, words, exit::USAGE);
        let line::Compare {
            tabs,
            sync,
            off,
            toggle,
            reading,
        } = line;
        let said = [tabs.is_some(), sync.is_some(), off, toggle.is_some(), reading.is_some()]
            .into_iter()
            .filter(|said| *said)
            .count();
        if off && said > 1 {
            return Err(usage("`--off` goes alone: it ends the comparison."));
        }
        if toggle.is_some() && said > 1 {
            return Err(usage("`--toggle` goes alone: it puts one tab side by side with the others, or takes it out."));
        }
        if reading.is_some() && said > 1 {
            return Err(usage("`--reading` goes alone: it chooses which of the tabs side by side is read."));
        }
        let sync = sync
            .map(|word| OnOff::from_word(&word).ok_or_else(|| usage(&format!("`{}` isn't for `--sync`. It's `on` or `off`.", recto_core::clean_line(&word)))))
            .transpose()?;
        Ok(match (tabs, sync, off, toggle, reading) {
            (Some(list), sync, ..) => Self::Set {
                tabs: Panes::parse(&list)?,
                sync,
            },
            (None, Some(sync), ..) => Self::Sync(sync),
            (None, None, true, ..) => Self::Off,
            (None, None, false, Some(tab), _) => Self::Toggle(TabRef::parse(&tab)),
            (None, None, false, None, Some(tab)) => Self::Reading(TabRef::parse(&tab)),
            (None, None, false, None, None) => Self::Look,
        })
    }
}

impl From<&Compare> for line::Compare {
    fn from(asked: &Compare) -> Self {
        match asked {
            Compare::Look => Self::default(),
            Compare::Off => Self { off: true, ..Self::default() },
            Compare::Set { tabs, sync } => Self {
                tabs: Some(tabs.to_string()),
                sync: sync.map(|sync| sync.word().to_string()),
                ..Self::default()
            },
            Compare::Sync(sync) => Self {
                sync: Some(sync.word().to_string()),
                ..Self::default()
            },
            Compare::Toggle(tab) => Self {
                toggle: Some(tab.to_string()),
                ..Self::default()
            },
            Compare::Reading(tab) => Self {
                reading: Some(tab.to_string()),
                ..Self::default()
            },
        }
    }
}

impl From<Compare> for line::Compare {
    fn from(asked: Compare) -> Self {
        Self::from(&asked)
    }
}

impl std::fmt::Debug for Compare {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        line::Compare::from(self).fmt(f)
    }
}

/// A comparison that can't be as asked, in the session's words.
fn refused(error: recto_session::compare::CompareError) -> AgentError {
    AgentError::new(Code::lower(error.code()), error.message(), exit::FAILED)
}

impl Cmd for Compare {
    const TAG: &'static str = "compare";
    const ROWS: &'static [Spec] = ROWS_COMPARE;
    const DRAWS: bool = false;
    const READS_PAGES: bool = false;
    const PROTECTED_OK: bool = false;
    // A pane's cold file is read in place here, or, on the window's session thread, off it and the command run again once it's in (`later`).
    const WAITS: bool = false;
    fn tab(&self) -> Option<&TabRef> {
        None
    }
    /// Asking is reading. Changing it is moving around.
    fn access(&self) -> Access {
        match self {
            Self::Look => row_access(Self::TAG),
            _ => Access::At(Level::Control),
        }
    }
    fn route(&self) -> Runs {
        Runs::Session
    }
    fn log_details(&self) -> Value {
        let sync = match self {
            Self::Set { sync, .. } => *sync,
            Self::Sync(sync) => Some(*sync),
            _ => None,
        };
        let mut details = json!({ "sync": sync.map(OnOff::word), "off": *self == Self::Off });
        match self {
            Self::Toggle(tab) => details["toggle"] = json!(super::log_tab(tab)),
            Self::Reading(tab) => details["reading"] = json!(super::log_tab(tab)),
            _ => {}
        }
        details
    }
    fn run<A: SessionAccess>(&self, cx: &mut Cx<'_, A>) -> Result<Value, AgentError> {
        compare(cx.access, self, cx.events)
    }
    fn human(data: &Value) -> String {
        match data["panes"].as_array() {
            Some(panes) if !panes.is_empty() => {
                let list: Vec<String> = panes
                    .iter()
                    .map(|pane| {
                        format!(
                            "#{} {} ({}){}",
                            pane["tab"],
                            text(&pane["untrusted"]["name"]),
                            if pane["protected"] == true {
                                "protected".to_string()
                            } else {
                                format!("page {} of {}", pane["page"], pane["pages"])
                            },
                            if pane["reading"] == true { ", read" } else { "" }
                        )
                    })
                    .collect();
                format!(
                    "Side by side: {}. Scroll: {}.{}",
                    list.join(", "),
                    if data["sync"] == "on" { "synced" } else { "separate" },
                    data["note"].as_str().map(|note| format!(" {note}")).unwrap_or_default()
                )
            }
            _ => "One tab showing, no comparison.".to_string(),
        }
    }
}

/// `recto compare`: what the sidebar's ⌘-click, moving into a pane and the right-click menu's scroll items do, for an agent and for the person alike. Owner,
/// 2026-11-08: "Ctrl clicking more then one pdf should enable comparison view, right click menu can enable separate scroll (default) and synced scroll. Zoom is
/// always synced."
///
/// **Run again from the start once a pane's file is in** (`later`), on the window's session thread: so nothing changes before every read it needs is done,
/// and each form is one change however many times it runs before then.
fn compare<A: SessionAccess>(access: &A, asked: &Compare, events: &mut Vec<Event>) -> Result<Value, AgentError> {
    let mut changed = false;
    match asked {
        Compare::Look => {}
        Compare::Off => changed |= access.with(|s| s.end_compare()),
        Compare::Set { tabs, sync } => {
            let ids = access.with(|s| {
                tabs.as_slice()
                    .iter()
                    .map(|selector| select(s, selector))
                    .collect::<Result<Vec<TabId>, AgentError>>()
            })?;
            recto_session::compare::Comparison::check(&ids).map_err(refused)?;
            // Each pane is a live document, so each cold one is read with the session free, as a tap's is, and none with it held.
            let reading = access.with(|s| s.tabs().active().filter(|active| ids.contains(active)).unwrap_or(ids[0]));
            for (id, path) in access.with(|s| s.wake_for_compare(&ids)) {
                read_ahead(access, |_| Some((id, path)))?;
            }
            changed |= access.with(|s| s.compare(&ids, Some(reading))).map_err(refused)?;
            if let Some(sync) = sync {
                changed |= set_synced(access, *sync)?;
            }
        }
        Compare::Sync(sync) => changed |= set_synced(access, *sync)?,
        Compare::Toggle(selector) => {
            let id = access.with(|s| select(s, selector))?;
            // The tab joining, or the pane read next if the one being read leaves.
            read_ahead(access, |s| s.wake_for_compare_click(id))?;
            let change = access.with(|s| s.toggle_compare(id)).map_err(refused)?;
            changed |= change != recto_session::compare::CompareChange::Nothing;
        }
        Compare::Reading(selector) => {
            let id = access.with(|s| {
                let id = select(s, selector)?;
                match s.comparison() {
                    Some(comparison) if comparison.contains(id) => Ok(id),
                    _ => Err(not_side_by_side()),
                }
            })?;
            read_ahead(access, |s| s.wake_for_tap(id))?;
            events.extend(access.with(|s| s.focus_pane(id).then(|| event::tab_activated(s, id))));
        }
    }
    if changed {
        events.push(access.with(|s| event::comparison_changed(s)));
    }
    // The window's own, for the person, draws what the session publishes: SessionAccess::answer_read.
    Ok(if access.answer_read() {
        access.with(|s| compare_json(s))
    } else {
        Value::Null
    })
}

/// Synced scroll or separate, for the comparison there is. Says whether that's a change.
fn set_synced<A: SessionAccess>(access: &A, sync: OnOff) -> Result<bool, AgentError> {
    access.with(|s| s.set_compare_synced(sync.is_on())).ok_or_else(|| {
        AgentError::new(Code::NOT_COMPARING, "There's no comparison to sync.", exit::NOT_FOUND)
            .then("`recto compare --tabs \"#1,#2\"` puts two side by side first.")
    })
}

/// `--reading` of a tab that isn't one of those side by side.
fn not_side_by_side() -> AgentError {
    AgentError::new(
        Code::NOT_SIDE_BY_SIDE,
        "That tab isn't side by side with others, so it can't be the pane read.",
        exit::NOT_FOUND,
    )
    .then("`recto compare` says which are. `recto tap` reads a tab alone.")
}

pub(crate) const ROWS_COMPARE: &[Spec] = &[Spec {
    name: "compare",
    tag: "compare",
    group: GROUP_MOVE,
    access: Access::At(Level::ReadOnly),
    summary: "Tabs side by side, as ⌘-clicking them in the sidebar does: each its own document in its own place, zoom always the same. Separate scroll, or synced, which keeps them page for page from where it went on. One of them is read, for the contents and the page number, as moving the pointer into its pane makes it. A plain `tap` goes back to one. With no flags it only says. To change it needs Control.",
    args: &[
        Arg {
            key: "tabs",
            flag: Some("--tabs"),
            kind: Kind::Text,
            required: false,
            help: "Two or three tabs, with commas between, left to right: `#1,#3`, or parts of their names. The active tab stays the one read if it's one of them, and otherwise the first.",
        },
        Arg {
            key: "sync",
            flag: Some("--sync"),
            kind: Kind::Choice(OnOff::WORDS),
            required: false,
            help: "`on` for synced scroll, from where each pane is now, or `off` for separate, the default.",
        },
        Arg {
            key: "off",
            flag: Some("--off"),
            kind: Kind::Switch,
            required: false,
            help: "Back to one tab, the one being read.",
        },
        Arg {
            key: "toggle",
            flag: Some("--toggle"),
            kind: Kind::Text,
            required: false,
            help: "One tab in or out, as ⌘-clicking its row does: a tab not side by side joins at the right and is read, and one that is leaves. With one tab showing, it and that one go side by side; down to one, that one's alone again. Goes alone.",
        },
        Arg {
            key: "reading",
            flag: Some("--reading"),
            kind: Kind::Text,
            required: false,
            help: "Which of the tabs side by side is read, as moving the pointer into its pane does. Nothing on screen moves. Goes alone.",
        },
    ],
    data: "`{ comparing, panes: [{ tab, page, pages, reading, untrusted: { name } }], sync, sync_from, note, max_panes }`. `note` says when synced documents differ in length.",
    emits: &["view_changed", "tab_activated"],
    examples: &[
        "recto compare",
        "recto compare --tabs \"#1,#2\"",
        "recto compare --tabs \"#1,#2,#4\" --sync on",
        "recto compare --sync off",
        "recto compare --toggle \"#3\"",
        "recto compare --reading \"#2\"",
        "recto compare --off",
    ],
    ..BASE
}];

#[cfg(test)]
mod tests {
    use crate::{Actor, AgentError, Event, Level, execute, parse};
    use recto_core::{DocFacts, OpenError, Toc};
    use recto_session::{DocLoader, Session, TabId};
    use serde_json::Value;
    use std::path::{Path, PathBuf};
    use std::sync::Mutex;

    #[derive(Clone)]
    struct Fake;

    impl DocLoader for Fake {
        fn load(&self, _path: &Path) -> Result<DocFacts, OpenError> {
            Ok(DocFacts {
                page_count: 4,
                byte_len: 1,
                encrypted: false,
                version: "1.7".into(),
                title: None,
                toc: Toc::default(),
                renderer_only: false,
            })
        }
    }

    fn three_tabs() -> Mutex<Session<Fake>> {
        let mut session = Session::new(Fake);
        for name in ["a", "b", "c"] {
            session.open(&PathBuf::from(format!("/d/{name}.pdf"))).expect("opened");
        }
        Mutex::new(session)
    }

    fn run(session: &Mutex<Session<Fake>>, level: Level, line: &str) -> (Result<Value, AgentError>, Vec<Event>) {
        let mut events = Vec::new();
        let result = parse(line).and_then(|command| execute(session, Actor::Agent(level), &command, &mut events));
        (result, events)
    }

    fn side_by_side(session: &Mutex<Session<Fake>>) -> (Option<u32>, Vec<u32>) {
        let session = session.lock().unwrap();
        (
            session.tabs().active().map(|id| id.0),
            session
                .comparison()
                .map(|comparison| comparison.tabs.iter().map(|id| id.0).collect())
                .unwrap_or_default(),
        )
    }

    /// Failure (R2.3, C5e): `--toggle` does something other than the ⌘-click it is for the person (a tab that joins isn't the one read, one that leaves
    /// leaves the rest, or down to one leaves a comparison of one), or says the view changed when the click did nothing: on the only tab showing.
    #[test]
    fn toggle_is_a_command_click_on_a_tabs_row() {
        let session = three_tabs();
        let toggle = |tab: u32| format!(r##"{{"cmd":"compare","toggle":"#{tab}"}}"##);
        let (nothing, said) = run(&session, Level::Control, &toggle(3));
        assert_eq!(nothing.expect("the tab showing")["comparing"], false);
        assert!(said.is_empty(), "nothing changed, so nothing's said: {said:?}");
        let (_, said) = run(&session, Level::Control, &toggle(1));
        assert_eq!(side_by_side(&session), (Some(1), vec![3, 1]), "joins at the right and is read");
        assert_eq!(said.iter().map(Event::name).collect::<Vec<_>>(), ["view_changed"]);
        run(&session, Level::Control, &toggle(2)).0.expect("a third");
        assert_eq!(side_by_side(&session), (Some(2), vec![3, 1, 2]));
        run(&session, Level::Control, &toggle(2)).0.expect("out again");
        assert_eq!(side_by_side(&session), (Some(3), vec![3, 1]), "the one read leaves, and the first is read");
        run(&session, Level::Control, &toggle(1)).0.expect("down to one");
        assert_eq!(side_by_side(&session), (Some(3), vec![]), "alone again");
        let (refused, _) = run(&session, Level::ReadOnly, &toggle(1));
        assert_eq!(refused.expect_err("Read-only").code, "permission_denied");
    }

    /// Failure (R2.3, C5e): `--reading` moves the reader into a tab that isn't a pane (which would end the comparison, as a tap does), or says a tab was
    /// activated when it was read already; or a flag that goes alone is let through with another, so what's done depends on an order nobody asked for.
    #[test]
    fn reading_chooses_a_pane_and_the_new_flags_go_alone() {
        let session = three_tabs();
        run(&session, Level::Control, r##"{"cmd":"compare","tabs":"#1,#2"}"##).0.expect("side by side");
        assert_eq!(side_by_side(&session), (Some(1), vec![1, 2]));
        let (_, said) = run(&session, Level::Control, r##"{"cmd":"compare","reading":"#2"}"##);
        assert_eq!(side_by_side(&session), (Some(2), vec![1, 2]));
        assert!(matches!(said.as_slice(), [Event::TabActivated { tab: TabId(2), .. }]), "{said:?}");
        let (again, said) = run(&session, Level::Control, r##"{"cmd":"compare","reading":"#2"}"##);
        assert!(again.is_ok() && said.is_empty(), "read already: nothing said, {said:?}");
        let (refused, _) = run(&session, Level::Control, r##"{"cmd":"compare","reading":"#3"}"##);
        assert_eq!(refused.expect_err("not a pane").code, "not_side_by_side");
        assert_eq!(side_by_side(&session), (Some(2), vec![1, 2]), "the comparison stands");
        for line in [
            r##"{"cmd":"compare","toggle":"#3","tabs":"#1,#2"}"##,
            r##"{"cmd":"compare","toggle":"#3","sync":"on"}"##,
            r##"{"cmd":"compare","reading":"#1","tabs":"#1,#2"}"##,
            r##"{"cmd":"compare","reading":"#1","toggle":"#3"}"##,
        ] {
            assert_eq!(run(&session, Level::Control, line).0.expect_err(line).code, "usage", "{line}");
        }
        assert_eq!(side_by_side(&session), (Some(2), vec![1, 2]), "none of those did anything");
    }
}
