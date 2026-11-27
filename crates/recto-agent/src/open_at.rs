//! Where a page of the saved file stands in a tab now, for `open` with a page (the person's link, the Finder's `#page=N`, `recto open --page`).
//!
//! A page asked for by a link or a number is a page of the file on disk, as the link was made from it; an open tab with unsaved edits has moved,
//! removed or added pages. This maps one to the other from the session's own state, with no disk access, because it runs on the session thread.

use crate::reading::positions_of;
use recto_session::{DocLoader, Session, TabId};
use serde_json::{Value, json};

/// Where a page of the file landed.
pub(crate) enum Landing {
    /// At this page of the tab, counted from 1.
    At(u32),
    /// Not in the tab as it stands: unsaved edits removed it, or its pages can't be shown as they stand.
    NotInTab,
}

/// Where page `file_page` (from 1) of the saved file of tab `id` stands now.
pub(crate) fn landing<L: DocLoader>(s: &Session<L>, id: TabId, file_page: u32) -> Landing {
    if s.notices().pages_withheld(id) {
        return Landing::NotInTab;
    }
    match positions_of(s, id) {
        None => Landing::At(file_page),
        Some(positions) => match file_page.checked_sub(1).and_then(|at| positions.get(at as usize)) {
            Some(place) if *place != u32::MAX => Landing::At(place + 1),
            _ => Landing::NotInTab,
        },
    }
}

/// Says in `data` which page was asked for and whether it landed, with a note, for an agent, when the numbers differ or it didn't land.
pub(crate) fn say(data: &mut Value, file_page: u32, landing: &Landing) {
    data["file_page"] = json!(file_page);
    data["landed"] = json!(matches!(landing, Landing::At(_)));
    match landing {
        Landing::At(page) if *page != file_page => {
            data["note"] = json!(format!(
                "This tab has unsaved edits, so page {file_page} of the file on disk is page {page} here."
            ));
        }
        Landing::At(_) => {}
        Landing::NotInTab => {
            data["note"] = json!(
                "That page of the file on disk isn't in the tab as it stands now (unsaved edits removed it, or the file changed under them), so the tab is shown where it was."
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::{editing::PageDelete, tabs::Open};
    use crate::{Actor, Command, Level, execute, parse};
    use std::sync::Mutex;

    /// A four-page file open in a tab, as the window's session holds one.
    fn four_pages() -> (Mutex<Session<crate::DrawableLoader>>, recto_testpdf::TempPdf) {
        let pages: Vec<_> = (1..=4)
            .map(|n| recto_testpdf::Page::new(recto_testpdf::text_lines(&[&format!("Page {n}")])))
            .collect();
        let file = recto_testpdf::TempPdf::new("four.pdf", &recto_testpdf::pages(&pages));
        let session = Mutex::new(Session::new(crate::DrawableLoader));
        session.lock().unwrap().open(&file.0).unwrap();
        (session, file)
    }

    fn open(session: &Mutex<Session<crate::DrawableLoader>>, path: &std::path::Path, page: Option<u32>) -> Result<Value, crate::AgentError> {
        let command = Command::from(Open {
            path: path.to_path_buf(),
            page,
        });
        execute(session, Actor::Agent(Level::Control), &command, &mut Vec::new())
    }

    fn delete_page_one(session: &Mutex<Session<crate::DrawableLoader>>) {
        let command = Command::from(PageDelete {
            tab: "active".into(),
            pages: "1".into(),
        });
        execute(session, Actor::Agent(Level::Control), &command, &mut Vec::new()).expect("deleted");
    }

    fn active_page(session: &Mutex<Session<crate::DrawableLoader>>) -> u32 {
        let session = session.lock().unwrap();
        let id = session.tabs().active().expect("a tab");
        crate::where_json(&session, Actor::Agent(Level::Control), id)["page"].as_u64().expect("a page") as u32
    }

    /// Failure: a link names a page of the file on disk and `open` lands on that number of the tab, so after an unsaved delete of page 1 a link to page 3
    /// shows the page that was page 4. File page 3 is tab page 2 here, and the answer says both numbers.
    #[test]
    fn a_page_of_the_file_lands_where_the_tab_has_it_now() {
        let (session, file) = four_pages();
        delete_page_one(&session);
        let said = open(&session, &file.0, Some(3)).expect("opened");
        assert_eq!(
            (said["page"].as_u64(), said["file_page"].as_u64(), said["landed"].as_bool()),
            (Some(2), Some(3), Some(true)),
            "{said}"
        );
        assert!(
            said["note"]
                .as_str()
                .is_some_and(|note| note.contains("page 3 of the file on disk is page 2 here")),
            "{said}"
        );
        assert_eq!(active_page(&session), 2);
    }

    /// Failure: a page the unsaved edits removed is guessed at (the nearest page, or the same number), so a link to a deleted page shows another page as if
    /// it were that one. It doesn't land, the tab stays where it was and no page-changed event is made.
    #[test]
    fn a_page_the_edits_removed_does_not_land_and_moves_nothing() {
        let (session, file) = four_pages();
        delete_page_one(&session);
        let before = active_page(&session);
        let mut events = Vec::new();
        let said = execute(
            &session,
            Actor::Agent(Level::Control),
            &Command::from(Open {
                path: file.0.clone(),
                page: Some(1),
            }),
            &mut events,
        )
        .expect("opened");
        assert_eq!((said["landed"].as_bool(), said["file_page"].as_u64()), (Some(false), Some(1)), "{said}");
        assert!(said["note"].as_str().is_some_and(|note| note.contains("shown where it was")), "{said}");
        assert_eq!(active_page(&session), before);
        assert!(
            events
                .iter()
                .all(|event| format!("{event:?}") != format!("{:?}", crate::page_now(&session.lock().unwrap(), TabId(1)))),
            "no page_now event"
        );
    }

    /// Failure: a page past the saved file's count opens a tab or moves one, or is counted against the tab's own (edited) length. It is `no_such_page` and
    /// nothing moves, even with a page deleted so the tab is shorter than the file.
    #[test]
    fn a_page_past_the_saved_file_opens_nothing() {
        let (session, file) = four_pages();
        delete_page_one(&session);
        let before = active_page(&session);
        let error = open(&session, &file.0, Some(5)).expect_err("page 5 of a four-page file");
        assert_eq!(error.code, crate::Code::NO_SUCH_PAGE);
        assert_eq!(active_page(&session), before);
        assert_eq!(session.lock().unwrap().tabs().len(), 1);
        // Page 4 of the file is page 3 of the three-page tab: one the tab, counted by its own length, would have refused as past its end.
        let said = open(&session, &file.0, Some(4)).expect("page 4 of the file exists");
        assert_eq!(said["page"].as_u64(), Some(3), "{said}");
    }

    fn line(text: &str) -> Result<Command, crate::AgentError> {
        parse(text)
    }

    /// Failure: `open` reads both a path and a link, or neither, or a link with a page, and picks one, so what runs isn't what was asked. Each is refused
    /// as the line is read, and a refused link's words never hold the link.
    #[test]
    fn the_open_line_takes_a_path_or_a_link_alone() {
        for text in [
            r#"{"cmd":"open","path":"/d/a.pdf","link":"rectopdf:///d/a.pdf"}"#,
            r#"{"cmd":"open"}"#,
            r#"{"cmd":"open","link":"rectopdf:///d/a.pdf","page":2}"#,
        ] {
            let error = line(text).err().unwrap_or_else(|| panic!("{text} should be refused"));
            assert_eq!(error.code, crate::Code::USAGE, "{text}");
        }
        let error = line(r#"{"cmd":"open","link":"http://MARKERLINK/a.pdf"}"#).err().expect("not a link");
        assert_eq!(error.code, crate::Code::NOT_A_LINK);
        assert!(!error.message.contains("MARKERLINK"), "{}", error.message);
        assert!(line(r#"{"cmd":"open","path":"/d/a.pdf","page":2}"#).is_ok());
    }

    /// Failure: the link text is kept in the command, so it is in the log and the wire an agent's activity shows. What runs is `{path, page}`, the link
    /// read as the line was, in both forms.
    #[test]
    fn a_links_wire_holds_its_path_and_page_and_no_link() {
        for link in ["rectopdf:///d/a%20b.pdf#page=3&zzmarker=1", "file:///d/a%20b.pdf#page=3"] {
            let command = line(&format!(r#"{{"cmd":"open","link":"{link}"}}"#)).expect("a link");
            let wire: Value = serde_json::from_str(&command.wire()).expect("json");
            assert_eq!(wire["path"], "/d/a b.pdf");
            assert_eq!(wire["page"], 3);
            assert!(wire.get("link").is_none() && !command.wire().contains("zzmarker"), "{wire}");
        }
    }

    /// Failure: an agent's route to a link opens a file it could not open by path, so the wall isn't the same. A link is the path, and a path outside
    /// the walls is refused as it is; an agent below Control is refused as for any `open`.
    #[test]
    fn an_agent_below_control_opens_no_link() {
        let command = line(r#"{"cmd":"open","link":"rectopdf:///d/a.pdf#page=1"}"#).expect("a link");
        let session = Mutex::new(Session::new(crate::DrawableLoader));
        let error = execute(&session, Actor::Agent(Level::ReadOnly), &command, &mut Vec::new()).expect_err("refused");
        assert_eq!(error.code, crate::Code::PERMISSION_DENIED);
        assert_eq!(session.lock().unwrap().tabs().len(), 0);
    }
}
