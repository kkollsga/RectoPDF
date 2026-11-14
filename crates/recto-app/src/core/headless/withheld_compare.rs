//! **A comparison with a document the person opened with its password, as an agent sees it** (security review of S1, its S2): `compare`'s note and
//! where synced scroll started, and a plain pane that follows the protected one, give away the protected document's length and place.

use super::withheld_events::{person_unlocks, protected};
use crate::core::controls::UiAction;
use crate::testkit::headless::{Disk, HeadlessApp};
use recto_agent::Level;
use serde_json::Value;

/// The person has a 30-page plain document beside a protected one of `pages` pages, synced, the protected one read, both laid out in their panes.
/// Gives the app and the two tabs: (plain, protected).
fn synced_beside_protected(pages: usize) -> (HeadlessApp, u32, u32) {
    let disk = Disk::new();
    let plain = disk.pdf("plain.pdf", 30);
    let locked = protected(&disk, pages, "Locked");
    let app = disk.start();
    app.open(&plain);
    person_unlocks(&app, &locked);
    let (plain_tab, locked_tab) = (app.tabs()[0].tab, app.tabs()[1].tab);
    let row = app.tabs().into_iter().find(|row| row.tab == plain_tab).expect("its row");
    app.act(UiAction::Row { row, adding: true });
    app.settle();
    app.act(UiAction::Menu { id: "compare.synced".into() });
    app.act(UiAction::PaneRead { tab: locked_tab });
    app.settle();
    (app, plain_tab, locked_tab)
}

/// The person scrolls `tab`'s pane down by `documents` of its whole height from where it is (up if less than 0), as their hand does; a synced pane
/// follows. Settled.
fn person_scrolls(app: &HeadlessApp, tab: u32, documents: f64) {
    let inner = std::sync::Arc::clone(app.core());
    let pane = (0..inner.panes.count())
        .find(|pane| inner.panes.tab_of(*pane).map(|id| id.0) == Some(tab))
        .expect("in a pane");
    let engine = inner.panes.engine(&inner.engine, pane).expect("its page view");
    let scroll = |y: f64, magnification: f64| recto_reader::Viewport {
        y,
        magnification,
        backing: 2.0,
        scrolled_to: u64::MAX - 1,
        ..recto_reader::Viewport::default()
    };
    let place = |engine: &recto_reader::Engine| engine.places().1.into_iter().find(|(id, _)| *id == tab).expect("its place").1;
    let at = place(&engine);
    drop(inner.panes.viewport(&inner.engine, pane, false, scroll(at.y, at.magnification)));
    let at = place(&engine);
    drop(
        inner
            .panes
            .viewport(&inner.engine, pane, true, scroll(at.y + documents * at.document_height, at.magnification)),
    );
    app.settle();
}

/// What an agent at Read-only reads of the comparison and of the plain tab.
fn agent_reads(app: &HeadlessApp, plain_tab: u32) -> (Value, Value) {
    let compared = app.recto(Level::ReadOnly, &["compare"]).expect("compare");
    let plain = app.recto(Level::ReadOnly, &["where", "--tab", &format!("#{plain_tab}")]).expect("where");
    (compared, plain)
}

/// Failure (security review of phase R, S2): beside a document the person opened with its password, `compare` gives an agent at Read-only its length
/// in the note ("Different lengths: N and 30 pages") and the page synced scroll started it from (`sync_from`), and the plain pane that follows it moves
/// with it and stops where it ends, so `where` on the plain tab, and a watcher's `page_changed` for it, say the protected one's place and length. What
/// an agent reads and hears is the same whatever the protected document's length, before and after the person scrolls it to its end.
#[test]
fn a_comparison_tells_an_agent_nothing_of_a_protected_pane() {
    let told = |pages: usize| {
        let (app, plain_tab, locked_tab) = synced_beside_protected(pages);
        let mut watching = app.agent_watches(Level::ReadOnly);
        let before = agent_reads(&app, plain_tab);
        // To the protected document's end and past it: the plain pane follows as far as it goes.
        person_scrolls(&app, locked_tab, 1.0);
        let after = agent_reads(&app, plain_tab);
        format!("before: {before:?}\nafter: {after:?}\nheard: {:?}", watching.heard())
    };
    let (five, nine) = (told(5), told(9));
    assert_eq!(five, nine, "an agent told the protected document's length or place");
}

/// A ⌘-click on `tab`'s row, as the person puts a tab side by side or takes it out. Settled.
fn command_click(app: &HeadlessApp, tab: u32) {
    let row = app.tabs().into_iter().find(|row| row.tab == tab).expect("its row");
    app.act(UiAction::Row { row, adding: true });
    app.settle();
}

/// An agent at Control, at `line`: its answer, or its refusal's code.
fn control(app: &HeadlessApp, line: &str) -> Result<Value, String> {
    app.agent(Level::Control, line).map_err(|error| error.code.to_string())
}

/// Failure (security review of phase R, S2): an agent at Control makes for itself what the person's synced comparison gave away: a protected tab synced
/// beside a plain one, then reads `compare`'s note, or makes a plain tab follow a protected one. Each way to a synced comparison with a protected pane is
/// refused as `protected` (put side by side and synced at once; synced afterwards; a tab added to one the person synced); side by side with separate
/// scroll is allowed, as nothing follows.
#[test]
fn an_agent_cant_make_a_synced_comparison_with_a_protected_pane() {
    let disk = Disk::new();
    let (plain, other) = (disk.pdf("plain.pdf", 30), disk.pdf("other.pdf", 30));
    let locked = protected(&disk, 5, "Locked");
    let app = disk.start();
    app.open(&plain);
    app.open(&other);
    person_unlocks(&app, &locked);
    let tab = |name: &str| app.tabs().into_iter().find(|row| row.text == name).expect(name).tab;
    let (plain_tab, other_tab, locked_tab) = (tab("plain"), tab("other"), tab("protected"));
    assert_eq!(
        control(&app, &format!(r##"{{"cmd":"compare","tabs":"#{plain_tab},#{locked_tab}","sync":"on"}}"##))
            .err()
            .as_deref(),
        Some("protected"),
        "side by side and synced at once"
    );
    control(&app, &format!(r##"{{"cmd":"compare","tabs":"#{plain_tab},#{locked_tab}"}}"##)).expect("side by side, separate");
    assert_eq!(
        control(&app, r#"{"cmd":"compare","sync":"on"}"#).err().as_deref(),
        Some("protected"),
        "synced afterwards"
    );
    app.act(UiAction::Menu { id: "compare.synced".into() });
    app.settle();
    assert_eq!(
        control(&app, &format!(r##"{{"cmd":"compare","toggle":"#{other_tab}"}}"##)).err().as_deref(),
        Some("protected"),
        "a tab added to the person's synced comparison would follow the protected one"
    );
}

/// Failure (S2, 3b): a plain tab left by following a protected one keeps the page it was left on, which says how far the protected one goes, and an
/// agent reads it once the person turns synced scroll off. Its place stays withheld until it's next set otherwise; an agent's `goto` on it sets it, and
/// says it again. A tab that never moved while it followed was never placed by it, and says its place.
#[test]
fn a_tab_left_by_following_a_protected_one_keeps_its_place_withheld_until_moved() {
    let page_of = |app: &HeadlessApp, tab: u32| app.recto(Level::ReadOnly, &["where", "--tab", &format!("#{tab}")]).expect("where")["page"].clone();
    let (app, plain_tab, _) = synced_beside_protected(5);
    app.act(UiAction::Menu { id: "compare.separate".into() });
    app.settle();
    assert_eq!(page_of(&app, plain_tab), 1, "never moved while it followed: its own place");
    let (app, plain_tab, locked_tab) = synced_beside_protected(5);
    person_scrolls(&app, locked_tab, 1.0);
    app.act(UiAction::Menu { id: "compare.separate".into() });
    app.settle();
    assert_eq!(page_of(&app, plain_tab), Value::Null, "left where following put it");
    command_click(&app, plain_tab);
    assert_eq!(page_of(&app, plain_tab), Value::Null, "and after the comparison ends");
    control(&app, &format!(r##"{{"cmd":"goto","tab":"#{plain_tab}","page":3}}"##)).expect("goto");
    app.settle();
    assert_eq!(page_of(&app, plain_tab), 3, "an agent's goto set its place");
}

/// **What an agent is told while the person compares a plain document with a protected one of `pages` pages that says `word`:** side by side; synced, the protected one
/// leading to its end; the plain one leading back up; separate; and an agent at Control asking for synced scroll and setting a separate comparison of
/// its own. At each step, a watch on every group and what `compare`, `where` on the plain tab, `tabs`, `session show` and `sidebar` say.
fn agent_told_of_a_comparison(pages: usize, word: &str) -> Vec<String> {
    let disk = Disk::new();
    let plain = disk.pdf("plain.pdf", 30);
    let locked = protected(&disk, pages, word);
    let app = disk.start();
    let mut watching = app.agent_watches(Level::Control);
    app.open(&plain);
    person_unlocks(&app, &locked);
    let (plain_tab, locked_tab) = (app.tabs()[0].tab, app.tabs()[1].tab);
    let mut told = Vec::new();
    let mut look = |app: &HeadlessApp, step: &str| {
        let mut told = vec![format!("{step} heard: {:?}", watching.heard())];
        for words in [
            vec!["compare".to_string()],
            vec!["where".into(), "--tab".into(), format!("#{plain_tab}")],
            vec!["tabs".into()],
            vec!["session".into(), "show".into()],
            vec!["sidebar".into()],
        ] {
            let words: Vec<&str> = words.iter().map(String::as_str).collect();
            told.push(format!("{step} {words:?}: {:?}", app.recto(Level::Control, &words)));
        }
        told
    };
    let row = app.tabs().into_iter().find(|row| row.tab == plain_tab).expect("its row");
    app.act(UiAction::Row { row, adding: true });
    app.act(UiAction::PaneRead { tab: locked_tab });
    app.settle();
    told.extend(look(&app, "side by side"));
    app.act(UiAction::Menu { id: "compare.synced".into() });
    app.settle();
    told.extend(look(&app, "synced"));
    person_scrolls(&app, locked_tab, 1.0);
    told.extend(look(&app, "the protected one led to its end"));
    app.act(UiAction::PaneRead { tab: plain_tab });
    person_scrolls(&app, plain_tab, -0.05);
    told.extend(look(&app, "the plain one led back up"));
    app.act(UiAction::Menu { id: "compare.separate".into() });
    app.settle();
    told.extend(look(&app, "separate"));
    told.push(format!("an agent syncs: {:?}", control(&app, r#"{"cmd":"compare","sync":"on"}"#).map(|_| ())));
    command_click(&app, plain_tab);
    told.push(format!(
        "an agent's own: {:?}",
        control(&app, &format!(r##"{{"cmd":"compare","tabs":"#{locked_tab},#{plain_tab}"}}"##)).map(|_| ())
    ));
    app.settle();
    told.extend(look(&app, "an agent's own, separate"));
    // A record really written, with the protected tab's place and length in it.
    app.advance(60_000);
    app.settle();
    told.extend(look(&app, "the record written"));
    told
}

/// Failure (S2; the walk of a comparison beside the events' walk in `withheld_events`): something of a protected document reaches an agent through a
/// comparison: its length in `compare`'s note, where synced scroll started it, the plain pane's place as it follows (where, compare's panes, `tabs`,
/// `session show`, `sidebar`, `page_changed`), the place it was left on, or the record's sizes in `session show`. An agent watching at Control, and
/// looking at each step, is told the same of a five-page and a nine-page protected document.
#[test]
fn an_agent_is_told_the_same_of_two_compared_protected_documents() {
    let (one, two) = (agent_told_of_a_comparison(5, "Locked"), agent_told_of_a_comparison(9, "Sealed"));
    let differs: Vec<String> = one.iter().zip(&two).filter(|(a, b)| a != b).map(|(a, b)| format!("  {a}\n  {b}")).collect();
    assert!(
        differs.is_empty(),
        "the agent was told something of the protected document:\n{}",
        differs.join("\n")
    );
    assert_eq!(one.len(), two.len());
}

/// What a watcher heard, less Find's word that a search is running and has finished: each edit to the plain tab finds the person's query again, and how
/// many of those runs are told apart follows how a busy machine's session thread takes them, not the documents (under `--like-ci` the twins differed in it in each of 10 rounds, 2026-11-14). Find's
/// news about a tab whose place is withheld is held back whole ([`recto_agent::withheld`]); its hits are the plain document's.
fn heard(watching: &mut crate::testkit::watching::Watching) -> Vec<Value> {
    let mut heard = watching.heard();
    heard.retain(|event| !(event["event"] == "find_finished" || (event["event"] == "find_changed" && event.get("running").is_some())));
    heard
}

/// What an agent at Control is told, and hears, of the plain tab `plain` (`#P` below), which follows or followed the protected one, at `step`: what reads
/// its place (`where`, `edits`, `tabs`, `session show`, `sidebar`, `compare`), what starts from it (a step of Find and its jump, `back`, `forward`, the
/// palette's Rotate of this page in edit mode), an edit's, undo's, redo's and save's answers, the record written and restored, and `wait page-drawn` on
/// page 5, where following leaves the plain tab beside a five-page protected one. The person's ⌘G in the plain pane is heard too. The plain pane is
/// made the one being read first, as Find and the palette's Rotate go by it.
fn probe(app: &HeadlessApp, watching: &mut crate::testkit::watching::Watching, plain: u32, step: &str) -> Vec<String> {
    use recto_control::host::Host;
    app.act(UiAction::PaneRead { tab: plain });
    app.settle();
    let mut told = vec![format!("{step} heard before: {:?}", heard(watching))];
    let tab = format!("#{plain}");
    for line in [
        format!(r#"{{"cmd":"where","tab":"{tab}"}}"#),
        format!(r#"{{"cmd":"edits","tab":"{tab}"}}"#),
        r#"{"cmd":"tabs"}"#.to_string(),
        r#"{"cmd":"session_show"}"#.to_string(),
        r#"{"cmd":"sidebar"}"#.to_string(),
        r#"{"cmd":"compare"}"#.to_string(),
        r#"{"cmd":"find","query":"page"}"#.to_string(),
        r#"{"cmd":"find","next":true}"#.to_string(),
        r#"{"cmd":"find","next":true,"jump":true}"#.to_string(),
        r#"{"cmd":"find"}"#.to_string(),
        format!(r#"{{"cmd":"back","tab":"{tab}"}}"#),
        format!(r#"{{"cmd":"forward","tab":"{tab}"}}"#),
        format!(r#"{{"cmd":"page_rotate","tab":"{tab}","pages":"2","degrees":90}}"#),
        format!(r#"{{"cmd":"undo","tab":"{tab}"}}"#),
        format!(r#"{{"cmd":"redo","tab":"{tab}"}}"#),
        format!(r#"{{"cmd":"save","tab":"{tab}"}}"#),
        r#"{"cmd":"edit_mode","set":"on"}"#.to_string(),
        r#"{"cmd":"palette","query":"rotate this page right","run":true}"#.to_string(),
        r#"{"cmd":"edit_mode","set":"off"}"#.to_string(),
        r#"{"cmd":"session_save"}"#.to_string(),
        r#"{"cmd":"session_restore"}"#.to_string(),
    ] {
        let mut said = app.agent(Level::Control, &line);
        // What's measured as it runs differs from run to run whatever the documents.
        if let Ok(answer) = said.as_mut() {
            for key in ["total_ms", "total_us", "write_us", "force_us", "verify_us", "took_ms", "first_hit_ms"] {
                if answer.get(key).is_some() {
                    answer[key] = Value::Null;
                }
            }
        }
        app.settle();
        told.push(format!("{step} {line}: {said:?}"));
    }
    let drawn = app
        .core()
        .host
        .check(recto_agent::Condition::PageDrawn, &recto_agent::TabRef::from(tab.as_str()), Some(5));
    told.push(format!("{step} wait page-drawn 5: {drawn:?}"));
    // The person's ⌘G in the plain pane, from where it is.
    app.act(UiAction::FindTyped { text: "page".into() });
    app.settle();
    app.act(UiAction::Menu { id: "find.next".into() });
    app.settle();
    told.push(format!("{step} heard after: {:?}", heard(watching)));
    told
}

/// **What an agent is told while the person compares a plain document with a protected one of `pages` pages that says `word`** (security review of
/// S2's round): synced, the protected one leading to its end (and, `plain_leads`, the plain one then leading back up); then separate; then an agent's
/// `goto 5` on the plain tab, which is where following left it beside a five-page document; then the person's Back. At each step, [`probe`].
fn agent_told_beside_a_followed_tab(pages: usize, word: &str, plain_leads: bool) -> Vec<String> {
    let disk = Disk::new();
    let plain = disk.pdf("plain.pdf", 30);
    let locked = protected(&disk, pages, word);
    let app = disk.start();
    let mut watching = app.agent_watches(Level::Control);
    app.open(&plain);
    person_unlocks(&app, &locked);
    let (plain_tab, locked_tab) = (app.tabs()[0].tab, app.tabs()[1].tab);
    command_click(&app, plain_tab);
    app.act(UiAction::Menu { id: "compare.synced".into() });
    app.act(UiAction::PaneRead { tab: locked_tab });
    app.settle();
    person_scrolls(&app, locked_tab, 1.0);
    if plain_leads {
        app.act(UiAction::PaneRead { tab: plain_tab });
        person_scrolls(&app, plain_tab, -0.05);
    }
    let mut told = probe(&app, &mut watching, plain_tab, "following");
    app.act(UiAction::Menu { id: "compare.separate".into() });
    app.settle();
    told.extend(probe(&app, &mut watching, plain_tab, "left by following"));
    told.push(format!(
        "goto 5: {:?}",
        control(&app, &format!(r##"{{"cmd":"goto","tab":"#{plain_tab}","page":5}}"##))
    ));
    app.settle();
    told.extend(probe(&app, &mut watching, plain_tab, "after a goto"));
    app.act(UiAction::Menu { id: "go.back".into() });
    app.settle();
    told.extend(probe(&app, &mut watching, plain_tab, "the person went back"));
    told
}

/// Failure (security review of S2's round): a plain tab that follows a protected one, or was left by following one, tells an agent that document's
/// place and length by a way the first round didn't close: an edit's, undo's, redo's or save's answer, `edits`, a step of Find from it or the person's
/// ⌘G there, `back` and `forward` after a jump, `session save`'s sizes, `session restore` (which forgets the place was withheld), `wait page-drawn`
/// after its place is let go, a `goto` to the page it's on, the palette's Rotate of this page. Twins whose protected documents differ in length and words
/// are told and heard alike, the protected one leading and the plain one leading.
#[test]
fn an_agent_is_told_nothing_of_where_a_followed_tab_is() {
    for plain_leads in [false, true] {
        let (one, two) = (
            agent_told_beside_a_followed_tab(5, "Locked", plain_leads),
            agent_told_beside_a_followed_tab(9, "Sealed", plain_leads),
        );
        let differs: Vec<String> = one.iter().zip(&two).filter(|(a, b)| a != b).map(|(a, b)| format!("  {a}\n  {b}")).collect();
        assert!(
            differs.is_empty(),
            "the agent was told something of the protected document (the plain one leading: {plain_leads}):\n{}",
            differs.join("\n")
        );
        assert_eq!(one.len(), two.len());
    }
}

/// Failure (S2's round): refusing an agent the palette's Rotate of this page while the page being read's place is withheld refuses the person's too, or
/// turns another page: the person's Rotate in edit mode, in a plain tab left by following a protected one, turns the page they're on, as it always did,
/// and an agent's is refused as `place_withheld`.
#[test]
fn the_persons_rotate_of_this_page_turns_it_where_an_agents_is_refused() {
    let (app, plain_tab, locked_tab) = synced_beside_protected(5);
    person_scrolls(&app, locked_tab, 1.0);
    app.act(UiAction::Menu { id: "compare.separate".into() });
    app.act(UiAction::PaneRead { tab: plain_tab });
    app.settle();
    control(&app, r#"{"cmd":"edit_mode","set":"on"}"#).expect("edit mode");
    // The palette's rows are the menus as published, which edit mode names "Rotate this page".
    app.settle();
    assert_eq!(
        control(&app, r#"{"cmd":"palette","query":"rotate this page right","run":true}"#)
            .err()
            .as_deref(),
        Some("place_withheld"),
        "an agent's starts from where the tab is"
    );
    app.act(UiAction::Menu { id: "rotate.right".into() });
    app.settle();
    let turns = app.core().host.session.with_session(|s| {
        let id = recto_session::TabId(plain_tab);
        let page = s.tabs().tab(id).expect("the plain tab").page;
        (page, s.page_source(id, page).map(|source| source.quarter_turns))
    });
    assert_eq!(turns, (4, Some(1)), "the person's turned page 5, where following left the tab, a quarter turn");
}
