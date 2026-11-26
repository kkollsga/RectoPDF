//! `recto link <page>`: the standard link to a page of the tab's file, `name.pdf#page=N`. The form is `recto_core::locator::page_link`; this is the
//! command around it (guide §2.9): the same one the window sends later for Copy Link, as the person. It reads no file and writes no clipboard.

use super::{Cmd, Cx, row_access};
use crate::human::text;
use crate::table::*;
use crate::*;
use recto_core::locator::{LinkError, address_of, link_of};
use serde_json::{Value, json};

const ADDED_PAGE: &str = "That page was added from another PDF and isn't in this file yet, so no link can name it. Save first, or ask for a page of the file.";
const NAME_REFUSED: &str = "This file's name can't be made into a link.";
const NOT_FOR_AGENTS: &str = "The file's address is the person's alone, for their clipboard. Ask for the link, which names no folder.";
/// What a refusal to make an address says, which the window tells from the added page's, as both are `not_referenceable`.
pub const ADDRESS_REFUSED: &str = "This file's location can't be made into a link.";
const UNSAVED_NOTE: &str =
    "This tab has unsaved edits, so the link names the page's place in the file on disk, not in the tab. The link's page is `file_page`.";

/// `recto link <page>`: a link to a page of the tab, by the file's own name. Not `links`, which lists what a page's own links point to.
#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub struct Link {
    pub page: u32,
    #[serde(default)]
    pub tab: TabRef,
    /// **The person's Copy Link:** the file's whole address, with the page, as text for the window's clipboard ([`crate::clipboard`]). Wire-only: no flag, no
    /// row argument. Anyone but the actor at the window is refused, before any tab is read, and the answer holds no address and no path.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub to_clipboard: bool,
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
        // The person's alone, said before any tab is read, so an agent learns nothing of the tabs by asking.
        let sink = if self.to_clipboard {
            Some(
                cx.access
                    .clipboard(cx.actor)
                    .ok_or_else(|| AgentError::new(Code::PERMISSION_DENIED, NOT_FOR_AGENTS, exit::PERMISSION))?,
            )
        } else {
            None
        };
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
        if let Some(sink) = sink {
            let address = address_of(file.path(), file_page + 1).map_err(|why| match why {
                LinkError::Page => crate::no_such_page(),
                LinkError::Name => AgentError::new(Code::NOT_REFERENCEABLE, ADDRESS_REFUSED, exit::FAILED),
            })?;
            sink.text(address);
            return Ok(json!({
                "tab": file.id(),
                "page": self.page,
                "file_page": file_page + 1,
                "as": "clipboard",
                "note": (file_page != wanted).then_some(UNSAVED_NOTE),
            }));
        }
        let link = link_of(file.path(), file_page + 1).map_err(|why| match why {
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
        if data["as"] == "clipboard" {
            return format!("Copied a link to page {} to the clipboard.", crate::human::number(&data["file_page"]));
        }
        format!(
            "A link to page {}: {}",
            crate::human::number(&data["file_page"]),
            text(&data["untrusted"]["link"])
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clipboard::ClipboardSink;
    use crate::extract::{Clip, ClipStop};
    use std::sync::Mutex;

    /// A window's clipboard that keeps the text given it and counts everything else.
    #[derive(Default)]
    struct Kept {
        texts: Mutex<Vec<String>>,
    }

    impl ClipboardSink for Kept {
        fn begin(&self) -> ClipStop {
            ClipStop::default()
        }
        fn begin_paste(&self) -> ClipStop {
            ClipStop::default()
        }
        fn offer(&self, _clip: Clip) {}
        fn made(&self, _bytes: Vec<u8>) {}
        fn text(&self, text: String) {
            self.texts.lock().unwrap().push(text);
        }
    }

    /// A session whose clipboard is the person's alone, as the host gives it.
    struct AtTheWindow {
        session: Mutex<recto_session::Session<crate::DrawableLoader>>,
        clipboard: Kept,
    }

    impl SessionAccess for AtTheWindow {
        type Loader = crate::DrawableLoader;
        fn with<R>(&self, step: impl FnOnce(&mut recto_session::Session<crate::DrawableLoader>) -> R) -> R {
            self.session.with(step)
        }
        fn clipboard(&self, actor: Actor) -> Option<&dyn ClipboardSink> {
            (actor == Actor::Person).then_some(&self.clipboard as &dyn ClipboardSink)
        }
    }

    fn at_the_window(name: &str) -> (AtTheWindow, recto_testpdf::TempPdf) {
        let file = recto_testpdf::TempPdf::new(name, &recto_testpdf::pages(&[recto_testpdf::Page::new(recto_testpdf::text_lines(&["Mine"]))]));
        let access = AtTheWindow {
            session: Mutex::new(recto_session::Session::new(crate::DrawableLoader)),
            clipboard: Kept::default(),
        };
        access.session.lock().unwrap().open(&file.0).unwrap();
        (access, file)
    }

    fn run(access: &AtTheWindow, actor: Actor, tab: TabRef, to_clipboard: bool) -> Result<Value, AgentError> {
        Cmd::run(
            &Link { page: 1, tab, to_clipboard },
            &mut Cx {
                access,
                actor,
                events: &mut Vec::new(),
            },
        )
    }

    /// Failure: the person's Copy Link gives the clipboard another text than the file's whole address and page (a name with no folder, a raw `#`), or puts
    /// the address or the folder in the command's answer, where the log and an agent's lane could read it. The sink gets exactly the address; the answer holds
    /// neither it nor the folder, and says it went to the clipboard.
    #[test]
    fn the_person_gets_the_address_on_the_clipboard_and_the_answer_holds_none() {
        let (access, file) = at_the_window("a b#1.pdf");
        let said = run(&access, Actor::Person, TabRef::Active, true).expect("copied");
        let folder = file.0.parent().unwrap().to_string_lossy().into_owned();
        let texts = access.clipboard.texts.lock().unwrap().clone();
        assert_eq!(texts.len(), 1);
        assert!(texts[0].starts_with("file:///") && texts[0].ends_with("/a%20b%231.pdf#page=1"), "{texts:?}");
        let said_text = said.to_string();
        assert_eq!(said["as"], "clipboard");
        assert!(
            !said_text.contains(&folder) && !said_text.contains("file://") && !said_text.contains("untrusted"),
            "{said_text}"
        );
    }

    /// Failure: an agent at any level asking for the clipboard gets the address, or reads the tabs first (a `no_such_tab` before the refusal tells it which
    /// tabs exist), or touches the person's clipboard. All four levels are refused as `permission_denied`, before the tab is looked for, and the sink is untouched.
    #[test]
    fn an_agent_is_refused_at_every_level_before_any_tab_is_read() {
        let (access, _file) = at_the_window("plain.pdf");
        for level in [Level::Off, Level::ReadOnly, Level::Control, Level::Full] {
            let error = run(&access, Actor::Agent(level), TabRef::Id(recto_session::TabId(9999)), true).expect_err("refused");
            assert_eq!((error.code, error.exit), (Code::PERMISSION_DENIED, exit::PERMISSION), "{level:?}: {error:?}");
        }
        assert!(access.clipboard.texts.lock().unwrap().is_empty());
        // A plain link is as it was: the name only, and the sink never given it.
        let plain = run(&access, Actor::Agent(Level::ReadOnly), TabRef::Active, false).expect("a link");
        assert_eq!(plain["untrusted"]["link"], "plain.pdf#page=1");
        assert!(access.clipboard.texts.lock().unwrap().is_empty());
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
