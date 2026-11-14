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
/// agent reads it once the person turns synced scroll off. Its place stays withheld until the person next sets it themselves; an agent's `goto` on it
/// moves it and keeps it withheld (the third review's D-A), and the person's own page typed in sets it, and it's said again. A tab that never moved while
/// it followed was never placed by it, and says its place.
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
    let goto = control(&app, &format!(r##"{{"cmd":"goto","tab":"#{plain_tab}","page":3}}"##)).expect("goto");
    app.settle();
    assert_eq!(
        (goto["page"].clone(), goto["place_withheld"].clone()),
        (Value::Null, Value::Bool(true)),
        "{goto}"
    );
    assert_eq!(page_of(&app, plain_tab), Value::Null, "an agent's goto kept it withheld");
    app.act(UiAction::Row {
        row: app.tabs().into_iter().find(|row| row.tab == plain_tab).expect("its row"),
        adding: false,
    });
    app.settle();
    assert!(app.act(UiAction::PageTyped { text: "4".into() }).understood);
    app.settle();
    assert_eq!(page_of(&app, plain_tab), 4, "the person's own page typed in set its place");
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

/// What `heard` gives, with each page's `page_drawn` once, in the order first heard: how many of a page's tiles are drawn after a jump follows what the
/// page view had drawn already and how the machine coalesces their notes, not the documents (a remainder, `docs/engineering/opening.md`).
fn pages_heard(watching: &mut crate::testkit::watching::Watching) -> Vec<Value> {
    let mut seen = Vec::new();
    heard(watching)
        .into_iter()
        .filter(|event| {
            event["event"] != "page_drawn" || {
                let page = (event["tab"].clone(), event["page"].clone());
                let first = !seen.contains(&page);
                seen.push(page);
                first
            }
        })
        .collect()
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

/// **The twins' answers don't turn on the machine's speed,** as in `recto-agent`'s walk: every renderer of this test's process gets a page deadline it
/// always meets, so Find reads every page for both twins. Under `--like-ci` one twin's Find once counted a page it couldn't read in time
/// (`failed_pages`), and the walk told them apart by it (2 of 20, 2026-11-14).
pub(super) fn deadlines_met() {
    recto_render_service::page_deadline_for_this_process(std::time::Duration::from_secs(20));
}

/// Failure (security review of S2's round): a plain tab that follows a protected one, or was left by following one, tells an agent that document's
/// place and length by a way the first round didn't close: an edit's, undo's, redo's or save's answer, `edits`, a step of Find from it or the person's
/// ⌘G there, `back` and `forward` after a jump, `session save`'s sizes, `session restore` (which forgets the place was withheld), `wait page-drawn`
/// after its place is let go, a `goto` to the page it's on, the palette's Rotate of this page. Twins whose protected documents differ in length and words
/// are told and heard alike, the protected one leading and the plain one leading.
#[test]
fn an_agent_is_told_nothing_of_where_a_followed_tab_is() {
    deadlines_met();
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

/// The host's view of the first pane, `tab`'s, at its place as the page view has it, `zoom` times its magnification, as the host's UI thread gives
/// it. Gives the view and that pane's page view.
fn first_pane_shown(app: &HeadlessApp, tab: u32, zoom: f64) -> (recto_reader::Viewport, recto_reader::Engine) {
    let inner = std::sync::Arc::clone(app.core());
    assert_eq!(inner.panes.tab_of(0).map(|id| id.0), Some(tab), "the first pane");
    let engine = inner.engine.clone();
    let at = engine.places().1.into_iter().find(|(id, _)| *id == tab).expect("its place").1;
    let view = recto_reader::Viewport {
        x: 0.0,
        y: at.y,
        width: 292.0,
        height: 225.0,
        frame_width: 292.0,
        magnification: at.magnification * zoom,
        backing: 1.0,
        live: false,
        scrolled_to: u64::MAX,
    };
    (view, engine)
}

/// **What an agent is told through sequences of commands beside a protected document of `pages` pages saying `word`** (the second security review): the
/// plain tab `P` left by following it to its end; then side by side with a third plain tab `C` instead, which an agent at Control asks to sync and the
/// person syncs, and scrolls, `P` following (S-1); then, with `P` read, an agent's zoom, a draw of `P`'s pages ended and its word held, and an agent's
/// `goto 8` on `P` (the page following, then the third, left it on beside five pages, another beside nine) before the word is let go (S-3), which keeps
/// its place withheld (D-A); then what's offered and said of `P`'s place and trail (S-2, D-2). At each step, what an agent at Control and Read-only reads
/// and a watcher heard.
fn agent_told_through_sequences(pages: usize, word: &str) -> Vec<String> {
    use recto_control::host::Host;
    let disk = Disk::new();
    let plain = disk.pdf("plain.pdf", 30);
    let third = disk.pdf("third.pdf", 30);
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
    app.act(UiAction::Menu { id: "compare.separate".into() });
    app.settle();
    app.open(&third);
    let third_tab = app
        .tabs()
        .into_iter()
        .map(|row| row.tab)
        .find(|tab| ![plain_tab, locked_tab].contains(tab))
        .expect("the third");
    app.act(UiAction::Row {
        row: app.tabs().into_iter().find(|row| row.tab == plain_tab).expect("its row"),
        adding: false,
    });
    app.settle();
    command_click(&app, third_tab);
    let mut told = Vec::new();
    let look = |app: &HeadlessApp, watching: &mut crate::testkit::watching::Watching, step: &str| {
        let mut told = Vec::new();
        let p = format!("#{plain_tab}");
        told.push(format!("{step} heard: {:?}", pages_heard(watching)));
        for (level, line) in [
            (Level::ReadOnly, vec!["compare"]),
            (Level::ReadOnly, vec!["where", "--tab", &p]),
            (Level::Control, vec!["session", "show"]),
            (Level::Control, vec!["palette", "back"]),
            (Level::Control, vec!["palette", "forward"]),
        ] {
            let mut said = app.recto(level, &line);
            if let Ok(answer) = said.as_mut() {
                for key in ["total_ms", "total_us", "took_ms"] {
                    if answer.get(key).is_some() {
                        answer[key] = Value::Null;
                    }
                }
            }
            told.push(format!("{step} {level:?} {line:?}: {said:?}"));
        }
        told
    };
    told.extend(look(&app, &mut watching, "beside the third"));
    let synced = control(&app, r#"{"cmd":"compare","sync":"on"}"#);
    assert_eq!(
        synced.as_ref().err().map(String::as_str),
        Some("place_withheld"),
        "an agent syncs a pane whose place is withheld: {synced:?}"
    );
    told.push(format!("an agent syncs: {synced:?}"));
    app.act(UiAction::Menu { id: "compare.synced".into() });
    app.act(UiAction::PaneRead { tab: third_tab });
    app.settle();
    person_scrolls(&app, third_tab, 0.1);
    told.extend(look(&app, &mut watching, "synced with the third, which led"));
    app.act(UiAction::Menu { id: "compare.separate".into() });
    app.act(UiAction::PaneRead { tab: plain_tab });
    app.settle();
    told.extend(look(&app, &mut watching, "separate"));
    // A draw of the plain tab's pages, ended and its word held, then an agent's goto, which keeps its place withheld (D-A), then the word.
    let (mut view, engine) = first_pane_shown(&app, plain_tab, 1.0);
    crate::testkit::headless::applied_as_the_host(&engine.set_viewport(view));
    crate::testkit::headless::host_turns(&app, &mut view, true);
    engine.qa().hold_tells(true);
    told.push(format!("zoom: {:?}", control(&app, r#"{"cmd":"view","zoom":"in"}"#).map(|_| ())));
    // The host shows it zoomed, from the same place: the pages there are drawn again.
    let (mut zoomed, _) = first_pane_shown(&app, plain_tab, 2.0);
    crate::testkit::headless::applied_as_the_host(&engine.set_viewport(zoomed));
    assert!(engine.qa().wait_for_held_tell(crate::testkit::headless::WAIT_LIMIT), "a draw's word held");
    told.push(format!(
        "goto 8: {:?}",
        control(&app, &format!(r##"{{"cmd":"goto","tab":"#{plain_tab}","page":8}}"##)).map(|answer| answer["page"].clone())
    ));
    engine.qa().hold_tells(false);
    // The host scrolls where the goto sent it and draws what's there, as its UI thread does.
    crate::testkit::headless::host_turns(&app, &mut zoomed, true);
    app.settle();
    let drawn = |page: u32| {
        app.core().host.check(
            recto_agent::Condition::PageDrawn,
            &recto_agent::TabRef::Id(recto_session::TabId(plain_tab)),
            Some(page),
        )
    };
    // Withheld still, so no page's drawing is the agent's to wait for, the one it went to included.
    told.push(format!("drawn around where following left it: {:?}", [7, 8, 9, 11, 13].map(drawn)));
    told.extend(look(&app, &mut watching, "after the goto"));
    told
}

/// Failure (the second security review, S-1 to S-3 and D-2, in sequence): a plain tab left by following a protected one is synced with a third, which
/// follows it unwithheld, or which it follows with its mark let go; an agent syncs them; a draw asked before an agent's `goto` lets the place go is told
/// after it (`page_drawn` around where following left it, `wait page-drawn`); the place the page view said before it is kept on the page the goto went to
/// (`session show`'s zoom and how far down); Back is offered after a jump elsewhere and not after one to the same place. Twins whose protected documents
/// differ in length and words, so following leaves the plain tab on page 8 in one, are told and hear the same.
#[test]
fn an_agent_is_told_nothing_of_a_followed_place_through_sequences() {
    deadlines_met();
    let (one, two) = (agent_told_through_sequences(5, "Locked"), agent_told_through_sequences(9, "Sealed"));
    let differs: Vec<String> = one.iter().zip(&two).filter(|(a, b)| a != b).map(|(a, b)| format!("  {a}\n  {b}")).collect();
    assert!(
        differs.is_empty(),
        "the agent was told something of the protected document:\n{}",
        differs.join("\n")
    );
    assert_eq!(one.len(), two.len());
}

/// The person has a 30-page plain document left by following a protected one of `pages` pages that says `word` to its end, on its own in the first pane,
/// with the host's view of it where following left it and drawn there; the protected one in another tab. The protected one was compared in the second
/// pane, so the first pane's page view is given no view by the person's scroll there, whose `scrolled_to` says every jump has arrived: the host's view
/// here says the last scroll it applied, as the real host's does. Gives the app, the two tabs (plain, protected), the host's view and the first pane's
/// page view.
fn left_by_following_alone(pages: usize, word: &str) -> (HeadlessApp, u32, u32, recto_reader::Viewport, recto_reader::Engine) {
    left_by_following_alone_in(|disk| disk.pdf("plain.pdf", 30), pages, word)
}

/// [`left_by_following_alone`], with the plain document `plain` writes, named `plain.pdf`.
pub(super) fn left_by_following_alone_in(
    plain: impl FnOnce(&Disk) -> std::path::PathBuf,
    pages: usize,
    word: &str,
) -> (HeadlessApp, u32, u32, recto_reader::Viewport, recto_reader::Engine) {
    let disk = Disk::new();
    let plain = plain(&disk);
    let locked = protected(&disk, pages, word);
    let app = disk.start();
    person_unlocks(&app, &locked);
    app.open(&plain);
    let tab = |name: &str| app.tabs().into_iter().find(|row| row.text == name).expect(name).tab;
    let (plain_tab, locked_tab) = (tab("plain"), tab("protected"));
    command_click(&app, locked_tab);
    app.act(UiAction::Menu { id: "compare.synced".into() });
    app.act(UiAction::PaneRead { tab: locked_tab });
    app.settle();
    person_scrolls(&app, locked_tab, 1.0);
    app.act(UiAction::Menu { id: "compare.separate".into() });
    app.settle();
    // The protected one out of the comparison: the plain one alone.
    command_click(&app, locked_tab);
    let inner = std::sync::Arc::clone(app.core());
    assert_eq!(inner.panes.tab_of(0).map(|id| id.0), Some(plain_tab), "the plain one alone in the first pane");
    assert_eq!(inner.panes.count(), 1);
    let (view, engine) = shown_by_the_host(&app, plain_tab);
    (app, plain_tab, locked_tab, view, engine)
}

/// The host's view of the first pane's tab `tab` at its place as the page view has it, applied and drawn, as the host's UI thread gives it: the view and
/// the first pane's page view. Settled.
fn shown_by_the_host(app: &HeadlessApp, tab: u32) -> (recto_reader::Viewport, recto_reader::Engine) {
    let engine = app.core().engine.clone();
    let at = engine.places().1.into_iter().find(|(id, _)| *id == tab).expect("its place").1;
    let mut view = recto_reader::Viewport {
        x: 0.0,
        y: at.y,
        width: 292.0,
        height: 225.0,
        frame_width: 292.0,
        magnification: at.magnification,
        backing: 1.0,
        live: false,
        scrolled_to: 0,
    };
    crate::testkit::headless::applied_as_the_host(&engine.set_viewport(view));
    crate::testkit::headless::host_turns(app, &mut view, true);
    (view, engine)
}

/// What an agent at Control reads of the plain tab in `session show`: its page, zoom and how far down.
pub(super) fn shown_of(app: &HeadlessApp, plain_tab: u32) -> Value {
    let shown = app.recto(Level::Control, &["session", "show"]).expect("session show");
    let tab = shown["tabs"]
        .as_array()
        .expect("tabs")
        .iter()
        .find(|tab| tab["tab"] == plain_tab)
        .cloned()
        .expect("the plain tab");
    serde_json::json!([tab["page"], tab["zoom"], tab["within_page"]])
}

/// Failure (the third security review, D-B 1): the person's own `goto` lets the place of a tab left by following go, and before the session thread's look
/// sends the page view the jump, the person goes to another tab: the page view still has the plain tab where following left it, says that place at the
/// next read and then keeps the tab warm there in the background, and it's taken in, on the page the person went to; so `session show` gives its zoom and
/// how far down only when that page is where following left the tab. Twins whose protected documents differ in length, the person going in both to the
/// page following left the first twin on, are told the same, and nothing of the old place.
#[test]
fn a_warm_page_view_doesnt_tell_the_place_the_persons_jump_let_go() {
    let told = |pages: usize, word: &str, to: Option<u32>| {
        let (app, plain_tab, locked_tab, _, _) = left_by_following_alone(pages, word);
        let left_on = app
            .core()
            .host
            .session
            .with_session(|s| s.tabs().tab(recto_session::TabId(plain_tab)).expect("plain").page + 1);
        let to = to.unwrap_or(left_on);
        let held = app.hold_session_thread_aside();
        let host = &app.core().host;
        let person = |line: String| host.run_counted(recto_agent::Actor::Person, &recto_agent::parse(&line).expect("a command"), &mut Vec::new());
        person(format!(r##"{{"cmd":"goto","tab":"#{plain_tab}","page":{to}}}"##)).expect("the person's goto");
        person(format!(r##"{{"cmd":"tap","tab":"#{locked_tab}"}}"##)).expect("the person's tap");
        held.let_go();
        app.settle();
        // Looked at again, as every look reads the views.
        app.core().take_places();
        (shown_of(&app, plain_tab), to, left_on)
    };
    let (five, to, left_on) = told(5, "Locked", None);
    let (nine, _, left_on_too) = told(9, "Sealed", Some(to));
    assert_ne!(left_on, left_on_too, "following left the twins on different pages");
    assert_eq!(five, nine, "the old place told on the page the person went to");
    assert_eq!(
        five,
        serde_json::json!([to, null, null]),
        "the page the person went to, and nothing of the place before it"
    );
}

/// Failure (the third security review, D-B 2): the cutoff for a draw's news after the person's own jump lets a withheld place go was the jump's send, but
/// the view stays where it was until the host arrives there: what the host's view asks for in that gap (a pinch, a resize) is planned at the old place
/// with a stamp after the send, and its `page_drawn` names the pages around where following left the tab. The host here holds its turn between the send
/// and the arrival and zooms the view where it was. Twins whose protected documents differ in length hear the same: the page the person went to.
#[test]
fn nothing_drawn_before_the_view_arrives_at_the_persons_jump_is_heard() {
    for hold in [false, true] {
        let heard_of = |pages: usize, word: &str| {
            let (app, _, _, view, engine) = left_by_following_alone(pages, word);
            let mut watching = app.agent_watches(Level::Control);
            let _ = jump_with_a_pinch_in_its_gap(&app, view, &engine, "20", hold);
            pages_heard(&mut watching)
                .into_iter()
                .filter(|event| event["event"] == "page_drawn")
                .collect::<Vec<_>>()
        };
        let (five, nine) = (heard_of(5, "Locked"), heard_of(9, "Sealed"));
        assert_eq!(
            five, nine,
            "pages drawn where following left the tab were heard (the gap's news held across the arrival: {hold})"
        );
        assert!(!five.is_empty(), "the page gone to is drawn, and heard");
    }
}

/// **The person types `page` in the go-to field, and the host's UI thread zooms the view where it is before it scrolls where the jump sent it** (a pinch
/// in the gap): what's there is drawn at the new size, planned while the jump is on its way. With `hold`, the view's news of that draw is held until the
/// view has arrived (`qa().hold_tells`), so it's told after the arrival and among the first of what's drawn there (the fourth review's item 3). Then the
/// host scrolls there and draws. Settled. Gives the host's view as it's left.
fn jump_with_a_pinch_in_its_gap(
    app: &HeadlessApp,
    view: recto_reader::Viewport,
    engine: &recto_reader::Engine,
    page: &str,
    hold: bool,
) -> recto_reader::Viewport {
    assert!(app.act(UiAction::PageTyped { text: page.into() }).understood);
    assert!(app.core().core.flush(crate::testkit::headless::WAIT_LIMIT), "the look sent the jump");
    // The window made wider as well, which lays the pages out again where the view is: a zoom alone waits for the jump's arrival.
    let mut pinched = recto_reader::Viewport {
        magnification: view.magnification * 1.5,
        width: view.width * 1.5,
        frame_width: view.frame_width * 1.5,
        ..view
    };
    let sent = std::cell::Cell::new(None);
    let take = |ops: Vec<recto_reader::Op>| {
        crate::testkit::headless::applied_as_the_host(&ops);
        for op in ops {
            if let recto_reader::Op::ScrollTo { y, seq } = op {
                sent.set(Some((y, seq)));
            }
        }
    };
    // The jump is on its way to the host before the pinch: the look's sent it (a command's look can come after the flush).
    app.wait_until("the jump's scroll, sent the host", || {
        take(engine.drain());
        sent.get().is_some()
    });
    engine.qa().hold_tells(hold);
    take(engine.set_viewport(pinched));
    if hold {
        assert!(
            engine.qa().wait_for_held_tell(crate::testkit::headless::WAIT_LIMIT),
            "a draw in the gap, its news held"
        );
        take(engine.drain());
        engine.applied();
    } else {
        app.wait_until("the view drew where it was", || {
            take(engine.drain());
            engine.applied();
            engine.qa().at_rest().is_some()
        });
        app.settle();
    }
    let (y, seq) = sent.get().expect("the jump's scroll, for the host");
    (pinched.y, pinched.scrolled_to) = (y, seq);
    crate::testkit::headless::applied_as_the_host(&engine.set_viewport(pinched));
    engine.qa().hold_tells(false);
    crate::testkit::headless::host_turns(app, &mut pinched, true);
    pinched
}

/// Failure (the fourth security review, item 3): the cutoff a release set was never reset, so in a tab whose place the person let go, every later jump's
/// draws planned while it's on its way (stamped before every send) were dropped for as long as the tab was hot or warm, and an agent's `wait page-drawn`
/// there could miss for good. After the release and its arrival, a later jump with a pinch in its gap is heard as in a tab never withheld: the same pages
/// as the same steps in a control app whose plain tab never followed anything.
#[test]
fn a_later_jump_in_a_tab_once_let_go_is_heard_as_in_a_tab_never_withheld() {
    let heard = |withheld: bool| {
        let (app, view, engine) = if withheld {
            let (app, _, _, view, engine) = left_by_following_alone(5, "Locked");
            (app, view, engine)
        } else {
            let disk = Disk::new();
            let plain = disk.pdf("plain.pdf", 30);
            let app = disk.start();
            app.open(&plain);
            let tab = app.tabs()[0].tab;
            let (view, engine) = shown_by_the_host(&app, tab);
            (app, view, engine)
        };
        let plain_tab = app.tabs().into_iter().find(|row| row.text == "plain").expect("plain").tab;
        // The person's own jump: in the first app it lets the place go, and the view arrives there.
        let view = jump_with_a_pinch_in_its_gap(&app, view, &engine, "10", false);
        let mut watching = app.agent_watches(Level::Control);
        let _ = jump_with_a_pinch_in_its_gap(&app, view, &engine, "20", false);
        let mut pages: Vec<Value> = heard(&mut watching)
            .into_iter()
            .filter(|event| event["event"] == "page_drawn" && event["tab"] == plain_tab)
            .map(|event| event["page"].clone())
            .collect();
        pages.dedup();
        (
            withheld.then(|| {
                app.core()
                    .host
                    .session
                    .with_session(|s| recto_session::compare::place_withheld(s, recto_session::TabId(plain_tab)))
            }),
            pages,
        )
    };
    let ((let_go, once_let_go), (_, never_withheld)) = (heard(true), heard(false));
    assert_eq!(let_go, Some(false), "the person's go-to let the place go");
    assert_eq!(
        once_let_go, never_withheld,
        "a later jump's draws in a tab once let go, against one never withheld"
    );
    // What's drawn in the gap is where the view was, laid out again for the wider window: pages before the one gone to.
    assert!(
        once_let_go.iter().any(|page| page.as_u64().is_some_and(|page| page < 20)),
        "what's drawn in the later jump's gap is heard: {once_let_go:?}"
    );
}

/// Failure (the third security review; the coordinator's D-A): a menu id an agent runs (`recto palette --run`) lets the place of a tab left by following
/// a protected one go, as the person's own would: Go to Page's way, a contents row's, Back's, a Find hit's. Every menu id, run for an agent at Full by
/// the palette's way on the plain tab being read, on twins whose protected documents differ in length: after each the plain tab's place is still
/// withheld while it's open, and what each run says is the same. What closes or empties goes last.
#[test]
fn no_menu_id_an_agent_runs_lets_a_withheld_place_go() {
    deadlines_met();
    let ids = super::menu_agents::every_id();
    let (last, first): (Vec<&String>, Vec<&String>) = ids.iter().partition(|id| matches!(id.as_str(), "file.close" | "recent.clear" | "app.quit"));
    let twins = [5, 9].map(|pages| {
        let (app, plain_tab, locked_tab) = synced_beside_protected(pages);
        person_scrolls(&app, locked_tab, 1.0);
        app.act(UiAction::Menu { id: "compare.separate".into() });
        app.act(UiAction::PaneRead { tab: plain_tab });
        app.settle();
        (app, plain_tab)
    });
    let mut differs = Vec::new();
    let mut let_go = None;
    for id in first.into_iter().chain(last) {
        let [one, two] = [&twins[0], &twins[1]].map(|(app, plain_tab)| {
            let core = app.core();
            let ran = core.host.run_menu_id(recto_agent::Actor::Agent(Level::Full), core, id, None, &mut Vec::new());
            app.settle();
            let kept = core.host.session.with_session(|s| {
                let plain = recto_session::TabId(*plain_tab);
                s.tabs().tab(plain).is_none() || recto_session::compare::place_withheld(s, plain)
            });
            (format!("{:?}", ran.map_err(|error| error.code)), kept)
        });
        if one.0 != two.0 {
            differs.push(format!("{id}:\n  {}\n  {}", one.0, two.0));
        }
        if !(one.1 && two.1) && let_go.is_none() {
            let_go = Some(id.clone());
        }
    }
    assert_eq!(let_go, None, "the first menu id an agent ran that let the withheld place go");
    assert!(differs.is_empty(), "told something of the protected document:\n{}", differs.join("\n"));
}
