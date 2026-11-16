//! **Where a tab is, for an agent's answer** (security review of S2's round): the one way every answer gets a tab's place, and the one gate for what
//! starts from it.
//!
//! A tab's place can be withheld from agents while its document isn't: it follows a protected document in a synced comparison, or was left by
//! following one, so where it is says where that one is and how far it goes ([`recto_session::compare::place_withheld`]). The rule was first asked
//! answer by answer, and the review found answers that forgot it (an edit's `page`, `back` after a jump, Find's step, a restore). So an answer gets a
//! tab's place only through [`place_for`], which says `None` to an agent while it's withheld and always gives the person theirs, and a scan of the crate
//! ([`tests::every_read_of_a_tabs_place_goes_through_place_for`]) fails on any other read of it that isn't on its reviewed list. What starts from the
//! place ([`crate::commands::Cmd::starts_from`]) is refused an agent's in [`crate::execute`], by [`refuses`].

use crate::commands::FromPlace;
use crate::{Actor, AgentError, Code, exit};
use recto_session::record::PlaceRecord;
use recto_session::{DocLoader, Session, TabId, TabRecord, compare};
use serde_json::Value;

/// **Where a tab is, as an answer may say it:** its page, what the file calls that page, and exactly where on it the page view last said. Got only from
/// [`place_for`]; each part is worked out when it's asked for, so an answer that says only the page pays for nothing else.
pub(crate) struct Place<'a, L: DocLoader> {
    session: &'a Session<L>,
    tab: &'a TabRecord,
}

impl<L: DocLoader> Place<'_, L> {
    /// The page, counted from 1.
    pub(crate) fn page(&self) -> u32 {
        self.tab.page + 1
    }

    /// What the file calls the page, where that isn't its number.
    pub(crate) fn label(&self) -> Option<String> {
        self.session.pages().label_differs(self.tab.id, self.tab.page)
    }

    /// The exact place the page view last said, while it's on the page the tab is.
    fn shown(&self) -> Option<PlaceRecord> {
        recto_session::place_shown(self.tab)
    }

    /// How far down the page, from 0 to 1, when the place is anchored on it.
    pub(crate) fn within(&self) -> Option<f64> {
        self.shown().filter(|place| place.anchor_page == place.page).map(|place| place.within)
    }

    /// The zoom it's read at.
    pub(crate) fn zoom(&self) -> Option<f64> {
        self.shown().map(|place| place.zoom)
    }
}

/// **A tab's place for `actor`'s answer:** `None` for an agent while it's withheld ([`compare::place_withheld_of`]), and always the person's own. Takes
/// the tab's record, which an answer has in hand, so nothing searches the tabs for it again.
pub(crate) fn place_for<'a, L: DocLoader>(session: &'a Session<L>, actor: Actor, tab: &'a TabRecord) -> Option<Place<'a, L>> {
    (actor.sees_protected() || !compare::place_withheld_of(session, tab)).then_some(Place { session, tab })
}

/// The refusal of an agent's command that starts from a tab's withheld place. Exit 8, as a protected document's refusal is.
pub fn place_withheld_refusal() -> AgentError {
    AgentError::new(
        Code::PLACE_WITHHELD,
        "This tab's place follows a document the user opened with its password, so nothing that starts from where it is can be done by an agent.",
        exit::PROTECTED,
    )
    .then(
        "Its place is given again once the user has gone to a page in that tab themselves, by its number or the contents. Nothing an agent does gives it back.",
    )
}

/// **The gate:** whether `actor`'s command that starts `from` the place of tab `id` is refused: an agent's, while the tab's place is withheld, or Back or
/// Forward would land on a stop made from such a place ([`compare::trail_step_withheld`]), or Find's step would go on from a hit stepped to from one.
/// The one rule: `execute`'s gate, a link that goes Back or Forward, the palette's Rotate of this page and an agent's palette listing Back and Forward ask it.
pub fn refuses<L: DocLoader>(session: &Session<L>, actor: Actor, from: FromPlace, id: TabId) -> bool {
    if actor.sees_protected() || from == FromPlace::No {
        return false;
    }
    let withheld = session.tabs().tab(id).is_some_and(|tab| compare::place_withheld_of(session, tab));
    withheld
        || match from {
            FromPlace::Trail { back } => compare::trail_step_withheld(session, id, back),
            FromPlace::PlaceOrHit => session.viewing().find_current_withheld(),
            FromPlace::No | FromPlace::Place => false,
        }
}

/// **Whether Back and Forward are there, for `actor`:** the person's as the trail has them; an agent's as [`refuses`] would let it go, so whether
/// they're there says nothing of a place withheld from it, or of whether a jump from one went to where it was (the second review, D-2: a jump to it
/// leaves a mark only, which the person's Back goes past, and one elsewhere a stop). `where`'s answer and an agent's palette ask it.
pub(crate) fn trail_offers<L: DocLoader>(session: &Session<L>, actor: Actor, id: TabId) -> (bool, bool) {
    let (back, forward) = session.tabs().can_go_back_and_forward(id);
    let refused = |back| refuses(session, actor, FromPlace::Trail { back }, id);
    (back && !refused(true), forward && !refused(false))
}

/// **The palette's Back and Forward rows left out for `actor`,** by their menu ids: an agent's while the gate would refuse its own `back` or `forward` in
/// the tab being read ([`refuses`]), so whether they're listed says nothing of a withheld place (D-2). A protected document's are refused as
/// `protected` by their command, as every command about it is, and listed. The gate's rule, asked here for the app's palette.
pub fn trail_rows_hidden<L: DocLoader>(session: &Session<L>, actor: Actor) -> Vec<&'static str> {
    let Some(id) = session.tabs().active().filter(|id| !session.tabs().withheld_from_agents(*id)) else {
        return Vec::new();
    };
    [(true, "go.back"), (false, "go.forward")]
        .into_iter()
        .filter(|(back, _)| refuses(session, actor, FromPlace::Trail { back: *back }, id))
        .map(|(_, menu)| menu)
        .collect()
}

/// The state record's sizes and counts, which an answer gives as the host says them.
const RECORD_NUMBERS: [&str; 6] = ["record_bytes", "journal_bytes", "writes", "appends", "wrote_bytes", "edited_tabs"];

/// **The record's numbers, left out of an agent's answer while any tab's place is withheld** (`session show`, `session save`): the record holds every
/// tab's place, a protected one's length and edits, and a followed one's page, so its sizes move with them, and how many tabs are edited counts a
/// protected one. Each is nulled where the answer has it; with no place withheld, the answer is as it was.
pub(crate) fn withhold_record_numbers<L: DocLoader>(session: &Session<L>, actor: Actor, answer: &mut Value) {
    if actor.sees_protected() || !session.tabs().iter().any(|tab| compare::place_withheld_of(session, tab)) {
        return;
    }
    for key in RECORD_NUMBERS {
        if answer.get(key).is_some() {
            answer[key] = Value::Null;
        }
    }
}

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

    /// Failure (the second review, D-4): `wait page-drawn` on a plain tab whose place is withheld is refused as `protected`, though its document isn't:
    /// the refusal names the place rule, `place_withheld`, as every other refusal that comes of it does.
    #[test]
    fn wait_page_drawn_on_a_withheld_place_is_refused_by_the_place_rule() {
        let mut session = Session::new(Chapters);
        let locked = session.open(Path::new("/d/locked.pdf")).unwrap();
        let plain = session.open(Path::new("/d/plain.pdf")).unwrap();
        session.tabs_mut().note_password_used(locked);
        session.compare(&[locked, plain], Some(plain)).unwrap();
        session.set_compare_synced(true);
        session.go_to(plain, 6).unwrap();
        let access = Mutex::new(session);
        let state = crate::check_condition(&access, crate::Condition::PageDrawn, &crate::TabRef::Id(plain), Some(7));
        assert!(
            matches!(&state, crate::ConditionState::Failed(error) if error.code == crate::Code::PLACE_WITHHELD),
            "{state:?}"
        );
    }

    /// What reads a tab's place, as the scan below looks for it: its page, counted from 0 or 1, and what the page view or the file says of it there; and
    /// what's derived from it (the second review, D-5): the place the page view said, a jump waiting for the view, where synced scroll started each pane,
    /// and the notes of pages drawn.
    const READS: [&str; 11] = [
        ".page + 1",
        "tab.page",
        "place_to_show(",
        "label_differs(",
        ".here(",
        "reading_line(",
        "place_shown(",
        "reading.place",
        "waiting_jump(",
        "sync_from",
        "draw_note(",
    ];

    /// **Each read of a place that isn't a tab's place through [`super::place_for`], with why,** by its file and function. A page an agent named, a hit's,
    /// a draw's or a link's is the document's, not where the tab is; an event is judged where events leave ([`crate::withheld`]).
    const READS_ALLOWED: &[(&str, &str, &str)] = &[
        ("place.rs", "page", "the one way: a `Place` is only had from `place_for`"),
        ("place.rs", "label", "the one way: a `Place` is only had from `place_for`"),
        ("place.rs", "shown", "the one way: a `Place` is only had from `place_for`"),
        (
            "selecting.rs",
            "run_counted",
            "where a selection starts: the page an agent named or the person picked, not where the tab is",
        ),
        ("render.rs", "budget", "a draw's page in a measure of pages an agent named"),
        ("lib.rs", "describe_source", "the file's page behind a page an agent named"),
        ("lib.rs", "edits_json", "the edit list's runs of pages, which say nothing of where the tab is"),
        ("lib.rs", "label_hits", "what the file calls each hit's page: the hits are the document's"),
        (
            "lib.rs",
            "compare_json",
            "where synced scroll started each pane: left out while the comparison is withheld (`compare::comparison_withheld`)",
        ),
        (
            "lib.rs",
            "check_condition",
            "a draw's note of a page an agent named, after the place rule's refusal and the mail's cutoff (S-3)",
        ),
        (
            "event.rs",
            "page_now",
            "an event, judged where events leave: `withheld::for_agents` drops it while the place is withheld",
        ),
        ("linking.rs", "link_json", "where a link goes, and what that page is called: the document's"),
        ("linking.rs", "links", "what the file calls the page an agent named"),
        (
            "find.rs",
            "step",
            "where a step of Find starts: an agent's is refused while the place is withheld (`Cmd::starts_from`); the person's is theirs",
        ),
        ("find.rs", "heard_going_to", "events, judged where events leave"),
        (
            "find/command.rs",
            "answer",
            "each hit's page, the document's; where a step went, nulled while `place_for` says none",
        ),
        ("export.rs", "run", "the file's page behind a page an agent named, to draw or read"),
        ("text.rs", "run", "the file's page behind a page an agent named, to read"),
    ];

    /// **The same for `recto-app`,** whose answers to agents (the palette, the menus' rows, the app's own commands) and whose taking in of what the page
    /// views say read a tab's place too (D-5). Each with why it isn't an agent's answer, or goes through the place rule.
    const APP_READS_ALLOWED: &[(&str, &str, &str)] = &[
        (
            "panes.rs",
            "follow",
            "synced scroll's follow between the host's panes: the person's window, never an agent's answer",
        ),
        (
            "model.rs",
            "reading_facts",
            "the window's model of the pane being read, drawn for the person; `qa state` reads it only in a quiet run",
        ),
        ("model.rs", "pane_snapshot", "what a pane's page view is given to show: the person's window"),
        (
            "core/places.rs",
            "take_in:tab.page",
            "the page the session has, against which a view's word is its own, and its `page_changed`, judged where events leave; only `tab.page`, so a \
             write of `reading.place` here isn't let through (the third review)",
        ),
        (
            "core/places.rs",
            "take_in_at_the_look",
            "the jumps the session thread's look sends the page views",
        ),
        (
            "core/editing.rs",
            "rotate_pages_now",
            "the page Rotate turns: an agent's is refused first by the gate's rule (`place::refuses`, D-6)",
        ),
        ("core/highlight.rs", "pick_of", "the pages of the person's selection, not where the tab is"),
        (
            "core/image_select.rs",
            "command",
            "the page the person clicked to pick an image, not where the tab is",
        ),
    ];

    /// **The same for `recto-control`.**
    const CONTROL_READS_ALLOWED: &[(&str, &str, &str)] = &[(
        "record_store.rs",
        "claim",
        "the record's page held to the pages its journal's edits have, as a restore takes it in: never an answer",
    )];

    /// Each read in `text`'s production lines (test items left out to their end, so production code after a test module is read), as (its line, the
    /// function it's in, the line's words).
    fn place_reads(text: &str) -> Vec<(usize, String, String)> {
        let mut function = String::new();
        let mut found = Vec::new();
        for (at, line) in crate::policy_guards::production_lines(text) {
            // Words in a string are an answer's or a row's text, not a read.
            let mut in_string = false;
            let mut last = ' ';
            let code: String = line
                .split("//")
                .next()
                .unwrap_or_default()
                .chars()
                .filter(|c| {
                    if *c == '"' && last != '\\' {
                        in_string = !in_string;
                    }
                    last = *c;
                    !in_string && *c != '"'
                })
                .collect();
            let code = code.as_str();
            if let Some((_, after)) = code.split_once("fn ") {
                let name: String = after.chars().take_while(|c| c.is_alphanumeric() || *c == '_').collect();
                if !name.is_empty() {
                    function = name;
                }
            }
            let reads = READS.iter().any(|read| {
                // `tab.page`, and not `tab.page_count`: a read that ends in a word ends where the word does.
                let word = |c: char| c.is_alphanumeric() || c == '_';
                code.match_indices(read)
                    .any(|(start, _)| !read.ends_with(word) || !code[start + read.len()..].starts_with(word))
            });
            if reads {
                found.push((at + 1, function.clone(), line.trim().to_string()));
            }
        }
        found
    }

    /// Whether an entry `named` allows a read in `function` of a line's `words`: the function's name, or `name:read`, which allows only lines whose one
    /// read of [`READS`] is `read`.
    fn allows(named: &str, function: &str, words: &str) -> bool {
        match named.split_once(':') {
            None => named == function,
            Some((name, only)) => name == function && READS.iter().all(|read| *read == only || !words.contains(read)),
        }
    }

    /// The reads in a crate's production sources not on `allowed`, and which entries of `allowed` were met: a folder named `tests`, `headless` or
    /// `testkit` and files named for tests are test-only, and so are `skip`.
    fn unlisted(root: &std::path::Path, crate_name: &str, allowed: &[(&str, &str, &str)], skip: &[&str]) -> (usize, Vec<String>, Vec<usize>) {
        let root = root.to_path_buf();
        let mut folders = vec![root.clone()];
        let (mut read, mut found, mut used) = (0, Vec::new(), Vec::new());
        while let Some(folder) = folders.pop() {
            for path in std::fs::read_dir(&folder).unwrap().map(|entry| entry.unwrap().path()) {
                let name = path.file_name().unwrap().to_string_lossy().into_owned();
                if path.is_dir() {
                    if !matches!(name.as_str(), "tests" | "headless" | "testkit") {
                        folders.push(path);
                    }
                    continue;
                }
                if !name.ends_with(".rs") || name.ends_with("tests.rs") || skip.contains(&name.as_str()) {
                    continue;
                }
                read += 1;
                let file = path.strip_prefix(&root).unwrap().to_string_lossy().replace("commands/", "");
                for (line, function, words) in place_reads(&std::fs::read_to_string(&path).unwrap()) {
                    match allowed
                        .iter()
                        .position(|(allowed, named, _)| *allowed == file && allows(named, &function, &words))
                    {
                        Some(at) => used.push(at),
                        None => found.push(format!("{crate_name}/{file}:{line} in `{function}`: {words}")),
                    }
                }
            }
        }
        (read, found, used)
    }

    /// Failure (security review of S2's round, and the second review's D-5): an answer reads where a tab is in a way of its own, as `edits` and every
    /// edit's answer did, or what's derived from it (`session show`'s zoom from the page view's place), and gives an agent a place that follows a
    /// protected document's. Every production line of `recto-agent`, `recto-app` and `recto-control` that reads a tab's place or what came of it is in
    /// [`super::place_for`] or on the crate's reviewed list with why; and each entry there is still a read, so a list doesn't keep what's gone.
    #[test]
    fn every_read_of_a_tabs_place_goes_through_place_for() {
        for (crate_name, allowed, skip, least) in [
            (
                "recto-agent",
                READS_ALLOWED,
                &["policy_guards.rs", "registry_golden.rs", "event_agreement.rs"][..],
                50,
            ),
            ("recto-app", APP_READS_ALLOWED, &["testkit.rs"][..], 50),
            ("recto-control", CONTROL_READS_ALLOWED, &[][..], 10),
        ] {
            let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join(crate_name).join("src");
            let (read, found, used) = unlisted(&root, crate_name, allowed, skip);
            assert!(read > least, "{crate_name}'s sources were read: {read} files");
            assert!(
                found.is_empty(),
                "a tab's place read other than through `place::place_for`: use it, or put the read on {crate_name}'s list with why it isn't where the tab is:\n{}",
                found.join("\n")
            );
            let gone: Vec<_> = allowed
                .iter()
                .enumerate()
                .filter(|(at, _)| !used.contains(at))
                .map(|(_, entry)| entry)
                .collect();
            assert!(gone.is_empty(), "no longer a read, so taken off {crate_name}'s list: {gone:?}");
        }
    }

    /// Failure (D-5): the scan of a crate misses a read in a folder below its sources, or one of a name it looks for, or counts a test-only folder: one
    /// read of each name seeded in a crate's nested folder is found, and the same in `headless` isn't.
    #[test]
    fn the_scan_of_a_crate_finds_a_read_of_each_name_seeded_in_it() {
        let root = std::env::temp_dir().join(format!("recto-place-scan-{}", std::process::id()));
        for folder in ["core", "headless"] {
            std::fs::create_dir_all(root.join(folder)).unwrap();
        }
        for read in READS {
            let text = format!("fn seeded() {{\n    let _ = (x{read} 0);\n}}\n");
            std::fs::write(root.join("core").join("seeded.rs"), &text).unwrap();
            std::fs::write(root.join("headless").join("seeded.rs"), &text).unwrap();
            let (read_files, found, _) = unlisted(&root, "seeded", &[], &[]);
            assert_eq!((read_files, found.len()), (1, 1), "{read}: {found:?}");
        }
        std::fs::remove_dir_all(&root).unwrap();
    }

    /// Failure (the third review): an entry that lets a function read only the page (`take_in:tab.page`) lets every read in it through, so a later write
    /// of the page view's place there, which a release must not take in, goes unseen.
    #[test]
    fn an_entry_for_one_read_lets_only_that_read_through() {
        assert!(allows("take_in:tab.page", "take_in", "let was = session.tabs().tab(id).map(|tab| tab.page);"));
        assert!(!allows("take_in:tab.page", "take_in", "tab.reading.place = Some(place);"));
        assert!(!allows("take_in:tab.page", "other", "tab.page"));
        assert!(
            allows("take_in", "take_in", "tab.reading.place = Some(place);"),
            "a whole function's entry, as before"
        );
    }

    /// Failure: the scan misses a read it names, or one after a test module, as another scan in this crate once lost a renderer by stopping at the first
    /// `#[cfg(test)]`.
    #[test]
    fn the_place_scan_sees_each_read_and_what_follows_a_test_module() {
        for read in READS {
            let text = format!("fn f() {{\n    let _ = (x{read} 0);\n}}\n");
            assert_eq!(place_reads(&text).len(), 1, "{read}: {text}");
        }
        let after_tests = "fn f() {}\n#[cfg(test)]\nmod tests {\n    fn g() { let _ = tab.page; }\n}\nfn h(tab: &T) -> u32 {\n    tab.page + 1\n}\n";
        assert_eq!(place_reads(after_tests), [(7, "h".to_string(), "tab.page + 1".to_string())]);
        assert!(
            place_reads("fn f() {\n    let _ = tab.page_count;\n}\n").is_empty(),
            "a word that starts the same isn't one"
        );
    }
}
