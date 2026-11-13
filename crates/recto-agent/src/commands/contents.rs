//! **The sidebar's contents as the person uses them** (step R2.3, C5f): `contents fold`, a section folded or opened as its chevron does, and `contents show`
//! and `contents stop`, what the note over a tab's contents does: the file's own shown, or generated ones, made if they aren't yet, or their making stopped.
//!
//! Generated contents come from the text of an untrusted PDF, through a helper (rule 6). These commands only start the helper, stop it and choose which list
//! the sidebar shows: none answers with a heading's words, and none widens what the helper reads, which is what `contents --generated` already asks it for.

use super::{Cmd, Cx, row_access};
use crate::generated;
use crate::table::*;
use crate::*;
use recto_session::GeneratedView;
use serde::Deserialize;
use serde_json::{Value, json};

/// A contents section folded, or opened, as its chevron does.
#[derive(Clone, Debug, Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub struct ContentsFold {
    /// The entry's number, as `contents` gives it.
    pub entry: u32,
    #[serde(default = "crate::active_tab")]
    pub tab: String,
}

impl Cmd for ContentsFold {
    const TAG: &'static str = "contents_fold";
    const ROWS: &'static [Spec] = ROWS_CONTENTS_FOLD;
    const DRAWS: bool = false;
    const READS_PAGES: bool = false;
    // Which entries have sections under them is the shape of a document's contents.
    const PROTECTED_OK: bool = false;
    const WAITS: bool = false;
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
        json!({ "tab": super::log_tab(&self.tab), "entry": self.entry })
    }
    fn run<A: SessionAccess>(&self, cx: &mut Cx<'_, A>) -> Result<Value, AgentError> {
        let Self { entry, tab } = self;
        cx.access.with(|s| {
            let id = select(s, tab)?;
            let open = s.toggle_contents_section(id, *entry).ok_or_else(|| {
                AgentError::new(
                    "not_a_section",
                    "That entry has nothing under it in the contents the sidebar shows, so there's nothing to fold or open.",
                    exit::NOT_FOUND,
                )
                .then("`recto contents` numbers the entries, and an entry with deeper ones after it is a section.")
            })?;
            Ok(json!({ "tab": id.0, "entry": entry, "open": open }))
        })
    }
    fn human(data: &Value) -> String {
        format!(
            "{} entry {}'s section.",
            if data["open"] == true { "Opened" } else { "Folded" },
            crate::human::number(&data["entry"])
        )
    }
}

/// Which contents the sidebar shows for a tab: the file's own, or generated ones, made if they aren't yet.
#[derive(Clone, Debug, Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub struct ContentsShow {
    /// `own` or `generated`.
    pub which: String,
    #[serde(default = "crate::active_tab")]
    pub tab: String,
}

impl Cmd for ContentsShow {
    const TAG: &'static str = "contents_show";
    const ROWS: &'static [Spec] = ROWS_CONTENTS_SHOW;
    const DRAWS: bool = false;
    const READS_PAGES: bool = false;
    const PROTECTED_OK: bool = false;
    // Generated ones not shown yet are looked for: the file's key, and the folder they're kept in.
    const WAITS: bool = true;
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
        json!({ "tab": super::log_tab(&self.tab), "which": recto_core::clean_line(&self.which) })
    }
    fn run<A: SessionAccess>(&self, cx: &mut Cx<'_, A>) -> Result<Value, AgentError> {
        show(cx.access, &self.tab, &self.which)
    }
    fn human(data: &Value) -> String {
        match (data["shown"].as_str(), data["state"].as_str()) {
            (Some("own"), _) => "The sidebar shows the file's own contents.".to_string(),
            (_, Some("ready")) => "The sidebar shows generated contents.".to_string(),
            (_, Some("making")) => "The sidebar will show generated contents once they're made, in the background.".to_string(),
            (_, Some("none_found")) => "Generated contents were made, and there were no headings to list.".to_string(),
            _ => "Generated contents couldn't be made just now. The sidebar's note says why.".to_string(),
        }
    }
}

/// What the note over a tab's contents comes to once generated ones are asked to be shown: where they stand.
fn view_word(view: Option<&GeneratedView>) -> &'static str {
    match view {
        Some(GeneratedView::Ready(_)) => "ready",
        Some(GeneratedView::Making) => "making",
        Some(GeneratedView::NoneFound) => "none_found",
        Some(GeneratedView::Failed(_)) => "failed",
        Some(GeneratedView::NotKept) | None => "not_made",
    }
}

/// `contents show`, the note's Show and Generate: the file's own chosen, or generated ones, looked for with the session free if they aren't shown yet and
/// asked for if they aren't made, as the note's Generate always did.
fn show<A: SessionAccess>(access: &A, tab: &str, which: &str) -> Result<Value, AgentError> {
    let generated = match which {
        "own" => false,
        "generated" => true,
        other => {
            return Err(AgentError::new(
                "usage",
                format!("`{}` isn't a list of contents. It's `own` or `generated`.", recto_core::clean_line(other)),
                exit::USAGE,
            ));
        }
    };
    let generating = if generated {
        Some(access.generating().ok_or_else(generated::no_generating)?)
    } else {
        None
    };
    let (id, look) = access.with(|s| {
        let id = select(s, tab)?;
        s.choose_generated(id, generated);
        let shown = matches!(s.generated().view(id), Some(GeneratedView::Ready(_) | GeneratedView::Making));
        let look = (generated && !shown)
            .then(|| Some((s.tabs().path_of(id)?, s.tabs().tab(id)?.page_count, s.tabs().own_stamp(id))))
            .flatten();
        Ok::<_, AgentError>((id, look))
    })?;
    if let (Some(generating), Some((path, pages, version))) = (generating, look)
        && let Some(view) = generated::where_they_stand(generating, &path, pages, true)
    {
        access.with(|s| s.set_generated_view_for(id, view, version));
    }
    Ok(access
        .with(|s| json!({ "tab": id.0, "shown": if generated { "generated" } else { "own" }, "state": generated.then(|| view_word(s.generated().view(id))) })))
}

/// Generated contents on their way stopped, and the file's own shown, as the note's Stop does.
#[derive(Clone, Debug, Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub struct ContentsStop {
    #[serde(default = "crate::active_tab")]
    pub tab: String,
}

impl Cmd for ContentsStop {
    const TAG: &'static str = "contents_stop";
    const ROWS: &'static [Spec] = ROWS_CONTENTS_STOP;
    const DRAWS: bool = false;
    const READS_PAGES: bool = false;
    const PROTECTED_OK: bool = false;
    const WAITS: bool = false;
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
        json!({ "tab": super::log_tab(&self.tab) })
    }
    fn run<A: SessionAccess>(&self, cx: &mut Cx<'_, A>) -> Result<Value, AgentError> {
        let access = cx.access;
        let (id, path, making) = access.with(|s| {
            let id = select(s, &self.tab)?;
            Ok::<_, AgentError>((id, s.tabs().path_of(id), s.generated().view(id) == Some(&GeneratedView::Making)))
        })?;
        // Stopped: the helper goes, and the offer's back. Nothing's read for it.
        if let (Some(generating), Some(path)) = (access.generating(), path) {
            generating.cancel_files(Some(&[path]));
        }
        access.with(|s| s.stop_generated_contents(id));
        Ok(json!({ "tab": id.0, "stopped": making }))
    }
    fn human(data: &Value) -> String {
        if data["stopped"] == true {
            "Stopped making generated contents. The sidebar shows the file's own."
        } else {
            "Nothing was being made. The sidebar shows the file's own contents."
        }
        .to_string()
    }
}

pub(crate) const ROWS_CONTENTS_FOLD: &[Spec] = &[Spec {
    name: "contents fold",
    tag: "contents_fold",
    group: GROUP_MOVE,
    access: Access::At(Level::Control),
    summary: "Folds a section of the contents the sidebar shows, or opens it if it's folded, as the chevron beside it does. What's folded is kept with the tab. Changes nothing in the file.",
    args: &[
        Arg {
            key: "entry",
            flag: None,
            kind: Kind::Count(100_000),
            required: true,
            help: "The entry's number, as `contents` gives it (`contents --generated` when generated contents are shown). It has to have deeper entries after it.",
        },
        TAB,
    ],
    data: "`{ tab, entry, open }`: whether the section's open now.",
    examples: &["recto contents fold 3", "recto contents fold 0 --tab \"#2\""],
    ..BASE
}];

pub(crate) const ROWS_CONTENTS_SHOW: &[Spec] = &[Spec {
    name: "contents show",
    tag: "contents_show",
    group: GROUP_MOVE,
    access: Access::At(Level::Control),
    summary: "Which contents the sidebar shows for a tab, as the note over them does: the file's own, or generated ones, made from the headings on its pages if they aren't yet. Making them starts the helper in the background, as `contents --generated` does, and this answers at once. The choice is kept with the tab, and the file never changes.",
    args: &[
        Arg {
            key: "which",
            flag: None,
            kind: Kind::Choice(&["own", "generated"]),
            required: true,
            help: "`own` or `generated`.",
        },
        TAB,
    ],
    data: "`{ tab, shown, state }`. With `generated`, `state` is `ready`, `making`, `none_found` (made, with no headings to list), `failed` or `not_made`; `recto contents --generated` gives them.",
    examples: &["recto contents show generated", "recto contents show own --tab \"#2\""],
    ..BASE
}];

pub(crate) const ROWS_CONTENTS_STOP: &[Spec] = &[Spec {
    name: "contents stop",
    tag: "contents_stop",
    group: GROUP_MOVE,
    access: Access::At(Level::Control),
    summary: "Stops generated contents being made for a tab and shows the file's own, as the note's Stop does. Nothing's read for it.",
    args: &[TAB],
    data: "`{ tab, stopped }`: whether some were being made.",
    examples: &["recto contents stop"],
    ..BASE
}];

#[cfg(test)]
mod tests {
    use crate::{Actor, DocLoader, Level, execute, parse};
    use recto_core::{DocFacts, NONE, OpenError, Toc, TocLine};
    use recto_session::Session;
    use std::path::{Path, PathBuf};
    use std::sync::Mutex;

    #[derive(Clone)]
    struct Sections;

    /// Two chapters, the first with two sections under it.
    impl DocLoader for Sections {
        fn load(&self, _path: &Path) -> Result<DocFacts, OpenError> {
            let entry = |depth: u8, parent: u32, page: u32| TocLine {
                title: "A heading",
                page,
                parent,
                depth,
            };
            Ok(DocFacts {
                page_count: 8,
                byte_len: 1,
                encrypted: false,
                version: "1.7".into(),
                title: None,
                toc: Toc::from_lines(&[entry(0, NONE, 0), entry(1, 0, 1), entry(1, 0, 2), entry(0, NONE, 4)], false),
                renderer_only: false,
            })
        }
    }

    fn one_tab() -> Mutex<Session<Sections>> {
        let mut session = Session::new(Sections);
        session.open(&PathBuf::from("/d/a.pdf")).expect("opened");
        Mutex::new(session)
    }

    fn run(session: &Mutex<Session<Sections>>, actor: Actor, line: &str) -> Result<serde_json::Value, crate::AgentError> {
        execute(session, actor, &parse(line).expect("a command"), &mut Vec::new())
    }

    /// Failure (R2.3, C5f): `contents fold` folds a section other than the one `contents` numbered, says it did something to an entry with nothing under it,
    /// or lets an agent below Control fold the person's contents.
    #[test]
    fn fold_folds_the_section_contents_numbered() {
        let session = one_tab();
        let listed = run(&session, Actor::Agent(Level::ReadOnly), r#"{"cmd":"contents"}"#).expect("listed");
        let numbers: Vec<_> = listed["entries"]
            .as_array()
            .expect("entries")
            .iter()
            .map(|entry| entry["entry"].clone())
            .collect();
        assert_eq!(numbers, [0, 1, 2, 3], "each entry has its number: {listed}");
        let first = run(&session, Actor::Agent(Level::Control), r#"{"cmd":"contents_fold","entry":0}"#).expect("a section");
        let again = run(&session, Actor::Agent(Level::Control), r#"{"cmd":"contents_fold","entry":0}"#).expect("a section");
        assert_ne!(first["open"], again["open"], "a fold toggles: {first} then {again}");
        let refused = run(&session, Actor::Agent(Level::Control), r#"{"cmd":"contents_fold","entry":1}"#).expect_err("nothing under it");
        assert_eq!(refused.code, "not_a_section");
        let refused = run(&session, Actor::Agent(Level::ReadOnly), r#"{"cmd":"contents_fold","entry":0}"#).expect_err("Read-only");
        assert_eq!(refused.code, "permission_denied");
    }

    /// Failure (R2.3, C5f): with nothing to make generated contents (a session in one process), `contents show generated` changes what's shown and leaves
    /// the note waiting for them for good; or `own` and `stop` need a helper to go back to the file's own; or a word other than own or generated is taken.
    #[test]
    fn showing_contents_needs_a_helper_only_for_generated_ones() {
        let session = one_tab();
        let generated = run(&session, Actor::Person, r#"{"cmd":"contents_show","which":"generated"}"#).expect_err("no helper here");
        assert_eq!(generated.code, "no_generated_contents");
        let id = session.lock().unwrap().tabs().active().expect("a tab");
        assert!(!session.lock().unwrap().tabs().tab(id).expect("its tab").generated_contents, "nothing chosen");
        assert!(session.lock().unwrap().generated().view(id).is_none(), "and nothing waited for");
        let own = run(&session, Actor::Person, r#"{"cmd":"contents_show","which":"own"}"#).expect("the file's own");
        assert_eq!(own["shown"], "own");
        let stopped = run(&session, Actor::Person, r#"{"cmd":"contents_stop"}"#).expect("nothing to stop");
        assert_eq!(stopped["stopped"], false);
        let wrong = run(&session, Actor::Person, r#"{"cmd":"contents_show","which":"theirs"}"#).expect_err("no such list");
        assert_eq!(wrong.code, "usage");
    }
}
