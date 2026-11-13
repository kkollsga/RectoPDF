//! `edit mode on|off`: the window's edit mode, in which the sidebar shows its tools for editing pages and its Rotate turns the page being read, not every page
//! (R2.3, C5d). It changes no page and throws nothing away: what an agent may edit is the edit commands' to say, at their levels, as with edit mode off.

use super::words::OnOff;
use super::{Cmd, Cx, row_access};
use crate::TabRef;
use crate::table::{Access, Arg, BASE, GROUP_EDIT, Kind, Spec};
use crate::{AgentError, Level, Runs, SessionAccess, exit};
use serde_json::{Value, json};

/// Turns the window's edit mode on or off.
#[derive(Clone, PartialEq, Eq, serde::Serialize)]
#[serde(into = "line::EditMode")]
pub struct EditMode {
    pub set: OnOff,
}

/// `edit mode` as it's written on the wire.
pub mod line {
    use serde::{Deserialize, Serialize};

    #[derive(Clone, Debug, Deserialize, Serialize)]
    #[serde(deny_unknown_fields)]
    pub struct EditMode {
        /// `on` or `off`.
        pub set: String,
    }
}

impl TryFrom<line::EditMode> for EditMode {
    type Error = AgentError;
    fn try_from(line: line::EditMode) -> Result<Self, AgentError> {
        let set = OnOff::from_word(&line.set).ok_or_else(|| {
            AgentError::new(
                "usage",
                format!("`{}` isn't a way edit mode can be. It's `on` or `off`.", recto_core::clean_line(&line.set)),
                exit::USAGE,
            )
        })?;
        Ok(Self { set })
    }
}

impl From<&EditMode> for line::EditMode {
    fn from(asked: &EditMode) -> Self {
        Self {
            set: asked.set.word().to_string(),
        }
    }
}

impl From<EditMode> for line::EditMode {
    fn from(asked: EditMode) -> Self {
        Self::from(&asked)
    }
}

impl std::fmt::Debug for EditMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        line::EditMode::from(self).fmt(f)
    }
}

impl Cmd for EditMode {
    const TAG: &'static str = "edit_mode";
    const ROWS: &'static [Spec] = ROWS_EDIT_MODE;
    const DRAWS: bool = false;
    const READS_PAGES: bool = false;
    // It reads nothing of a document, the one being read included.
    const PROTECTED_OK: bool = true;
    const WAITS: bool = false;
    fn tab(&self) -> Option<&TabRef> {
        None
    }
    fn access(&self) -> Access {
        row_access(Self::TAG)
    }
    fn route(&self) -> Runs {
        Runs::Session
    }
    fn log_details(&self) -> Value {
        json!({ "set": self.set.word() })
    }
    fn run<A: SessionAccess>(&self, cx: &mut Cx<'_, A>) -> Result<Value, AgentError> {
        let on = self.set.is_on();
        Ok(cx.access.with(|s| {
            let was = s.viewing().edit_mode();
            s.viewing_mut().set_edit_mode(on);
            json!({ "edit_mode": on, "changed": was != on })
        }))
    }
    fn human(data: &Value) -> String {
        match (data["edit_mode"] == true, data["changed"] == true) {
            (true, true) => "Edit mode is on: the sidebar shows its tools for editing pages, and its Rotate turns the page being read.",
            (true, false) => "Edit mode was on already.",
            (false, true) => "Edit mode is off: the sidebar's Rotate turns every page.",
            (false, false) => "Edit mode was off already.",
        }
        .to_string()
    }
}

pub(crate) const ROWS_EDIT_MODE: &[Spec] = &[Spec {
    name: "edit mode",
    tag: "edit_mode",
    group: GROUP_EDIT,
    access: Access::At(Level::Control),
    summary: "Turns the window's edit mode on or off, as the user does: on, the sidebar shows its tools for editing pages and its Rotate turns the page being read rather than every page. It changes no page, and lets no edit through that its own command's level wouldn't. `recto session show` says which it is.",
    args: &[Arg {
        key: "set",
        flag: None,
        kind: Kind::Choice(&["on", "off"]),
        required: true,
        help: "`on` or `off`.",
    }],
    data: "`{ edit_mode, changed }`: how it is now, and whether that's a change.",
    examples: &["recto edit mode on", "recto edit mode off"],
    ..BASE
}];

#[cfg(test)]
mod tests {
    use crate::{Actor, Command, DocLoader, Level, execute, parse};
    use recto_core::{DocFacts, OpenError, Toc};
    use recto_session::Session;
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

    /// A session with one tab, with an edit of the owner's that the owner has undone: there to redo, and the owner's.
    fn with_the_owners_undone_step() -> Mutex<Session<Fake>> {
        let mut session = Session::new(Fake);
        session.open(&PathBuf::from("/d/a.pdf")).expect("opened");
        let session = Mutex::new(session);
        let run = |line: &str| execute(&session, Actor::Person, &parse(line).expect("a command"), &mut Vec::new());
        run(r#"{"cmd":"page_rotate","pages":"all","degrees":90}"#).expect("the owner's edit");
        run(r#"{"cmd":"undo"}"#).expect("undone");
        session
    }

    /// Failure (R2.3, C5d): edit mode, which an agent at Control can now turn on, lets an agent below Full past the owner's-edits rules: a new edit that
    /// throws away the owner's undone step, a discard, a clear of the history or a close that loses it. With edit mode on, each is refused as with it off,
    /// and the owner's step is still there to redo.
    #[test]
    fn edit_mode_lets_an_agent_below_full_do_nothing_the_owners_edits_rules_refuse() {
        let refusals = |session: &Mutex<Session<Fake>>| {
            [
                r#"{"cmd":"page_rotate","pages":"1","degrees":90}"#,
                r#"{"cmd":"discard"}"#,
                r#"{"cmd":"clear_history"}"#,
                r#"{"cmd":"close"}"#,
            ]
            .map(|line| {
                execute(session, Actor::Agent(Level::Control), &parse(line).expect("a command"), &mut Vec::new())
                    .err()
                    .map(|error| error.code)
            })
        };
        let session = with_the_owners_undone_step();
        let off = refusals(&session);
        assert!(off.iter().all(Option::is_some), "each refused with edit mode off: {off:?}");
        let on = execute(
            &session,
            Actor::Agent(Level::Control),
            &parse(r#"{"cmd":"edit_mode","set":"on"}"#).expect("a command"),
            &mut Vec::new(),
        )
        .expect("an agent at Control may turn it on");
        assert_eq!(on["edit_mode"], true);
        assert!(session.lock().unwrap().viewing().edit_mode());
        assert_eq!(refusals(&session), off, "and each refused the same with it on");
        let session = session.into_inner().unwrap();
        let id = session.tabs().active().expect("a tab");
        assert!(session.lists().has_undone_owner_steps(id), "the owner's step is still there to redo");
    }

    /// Failure: edit mode is turned by an agent below Control, or by a word that isn't on or off.
    #[test]
    fn edit_mode_needs_control_and_on_or_off() {
        let session = with_the_owners_undone_step();
        let line = |set: &str| parse(&format!(r#"{{"cmd":"edit_mode","set":"{set}"}}"#)).expect("a command");
        let refused = execute(&session, Actor::Agent(Level::ReadOnly), &line("on"), &mut Vec::new()).expect_err("Read-only");
        assert_eq!(refused.code, "permission_denied");
        // A word that isn't on or off can't be a command at all (R2.5): refused as it's read, at every level.
        let refused = parse(r#"{"cmd":"edit_mode","set":"sideways"}"#).expect_err("no such way");
        assert_eq!(refused.code, "usage");
        assert!(!session.lock().unwrap().viewing().edit_mode(), "left off");
        let answer = execute(&session, Actor::Person, &line("off"), &mut Vec::new()).expect("off again");
        assert_eq!(answer["changed"], false);
        assert!(matches!(line("on"), Command::Edit(_)));
    }
}
