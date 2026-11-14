//! **Where a tab is, for an agent's answer** (security review of S2's round): the one way every answer gets a tab's place.

#[cfg(test)]
mod tests {
    use crate::{Actor, Command, Level, commands, execute};
    use recto_core::{DocFacts, NONE, OpenError, Toc, TocLine};
    use recto_session::{DocLoader, Session};
    use std::path::Path;
    use std::sync::Mutex;

    /// A protected file of 9 pages, and a plain one of 10 whose contents have two chapters of one section each.
    #[derive(Clone)]
    struct Chapters;

    impl DocLoader for Chapters {
        fn load(&self, path: &Path) -> Result<DocFacts, OpenError> {
            let lines = [("One", 1, NONE, 0), ("One, a", 2, 0, 1), ("Two", 6, NONE, 0), ("Two, a", 7, 2, 1)].map(|(title, page, parent, depth)| TocLine {
                title,
                page,
                parent,
                depth,
            });
            let plain = path.ends_with("plain.pdf");
            Ok(DocFacts {
                page_count: if plain { 10 } else { 9 },
                byte_len: 1,
                encrypted: !plain,
                version: "1.7".into(),
                title: None,
                toc: if plain { Toc::from_lines(&lines, false) } else { Toc::default() },
                renderer_only: false,
            })
        }
    }

    /// Failure (S2's round, the sidebar): the plain tab being read was left by following a protected one, and an agent's `sidebar` lists its contents
    /// rows, whose sections open where the reader is, so which are open, and which rows are there, say where following left it. An agent's sidebar has
    /// the tab's row with no page and none of its contents rows; the person's has them all.
    #[test]
    fn an_agents_sidebar_lists_no_contents_of_a_tab_whose_place_is_withheld() {
        let mut session = Session::new(Chapters);
        let locked = session.open(Path::new("/d/locked.pdf")).unwrap();
        let plain = session.open(Path::new("/d/plain.pdf")).unwrap();
        session.tabs_mut().note_password_used(locked);
        session.compare(&[locked, plain], Some(plain)).unwrap();
        session.set_compare_synced(true);
        session.go_to(plain, 6).unwrap();
        session.set_compare_synced(false);
        let access = Mutex::new(session);
        let sidebar = |actor: Actor| execute(&access, actor, &Command::from(commands::reading::Sidebar {}), &mut Vec::new()).expect("sidebar");
        let contents = |said: &serde_json::Value| said["rows"].as_array().unwrap().iter().filter(|row| row["kind"] == "contents").count();
        let agents = sidebar(Actor::Agent(Level::Full));
        assert_eq!(contents(&agents), 0, "{agents}");
        let row = agents["rows"]
            .as_array()
            .unwrap()
            .iter()
            .find(|row| row["tab"] == plain.0 && row["kind"] == "tab")
            .cloned();
        assert!(row.is_some_and(|row| row["page"].is_null()));
        assert!(contents(&sidebar(Actor::Person)) > 0, "the person's sidebar is theirs");
    }
}
