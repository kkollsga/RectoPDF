//! `get` through the app core, with no window: the text a reference points at, at the grains there are before blocks, as text and Markdown. One executor
//! (`recto_agent::getting`); the window has no control for it, so everything here is what an agent sees and what it may not do.

use super::changed_under_edits::{look, pdf_bytes, replace};
use super::files::locked_saying;
use super::locating::{hostile_chars, page_text, pages_saying, reference};
use super::withheld_events::{PASSWORD, person_unlocks};
use crate::testkit::headless::{Disk, HeadlessApp};
use recto_agent::{AgentError, Level, exit};
use serde_json::{Value, json};

/// A valid reference token that points at nothing in any file: what a hostile line could carry.
const NOWHERE: &str = "r1.040g2081040g2.p1-081040g2081040g2081040g208-0000000";

fn get(app: &HeadlessApp, level: Level, token: &str, rest: Value) -> Result<Value, AgentError> {
    let mut line = json!({ "cmd": "get", "ref": token, "tab": "#1" });
    if let (Some(line), Some(rest)) = (line.as_object_mut(), rest.as_object()) {
        line.extend(rest.clone());
    }
    app.agent(level, &line.to_string())
}

fn chars_ref(app: &HeadlessApp, page: u32, start: u32, end: u32) -> String {
    let made = app
        .agent(
            Level::ReadOnly,
            &json!({ "cmd": "ref", "page": page, "chars": format!("{start}-{end}"), "tab": "#1" }).to_string(),
        )
        .expect("a reference to characters");
    made["ref"].as_str().expect("its token").to_owned()
}

fn resolves(app: &HeadlessApp, token: &str) -> Value {
    super::locating::resolve(app, Level::ReadOnly, token).expect("resolve")
}

/// Failure: a page whose lines start like Markdown (`#`, `>`, a `~~~` fence, a list, a number and a dot) becomes a heading, a quote or a code block in the answer, so
/// a file writes the structure an agent reads; or a hit gives more than its words, or a page less than all of them; or `up`, `next` or `prev` name the wrong
/// page. The three-line page comes back with a backslash before every line's first mark (the dot, for a number), `hit` is exactly the characters named, `page` all of them, and the
/// page's `up`, `next` and `prev` are references that resolve to the pages they are.
#[test]
fn get_gives_the_hit_or_the_page_and_markdown_cannot_be_forged() {
    let app = HeadlessApp::start();
    let lines = ["# not a heading", "> not a quote", "~~~", "1. not a list", "plain words"];
    let pages = [
        recto_testpdf::Page::new(recto_testpdf::text_lines(&["Mine page 1"])),
        recto_testpdf::Page::new(recto_testpdf::text_lines(&lines)),
        recto_testpdf::Page::new(recto_testpdf::text_lines(&["Mine page 3"])),
    ];
    let file = app.disk().file("doc.pdf", &recto_testpdf::pages(&pages));
    app.open(&file);
    let folder = app.disk().files().to_string_lossy().into_owned();
    let whole = page_text(&app, 2);
    let token = chars_ref(&app, 2, 2, 5);
    let hit = get(&app, Level::ReadOnly, &token, json!({})).expect("a hit");
    assert_eq!(
        (hit["grain"].as_str(), hit["unit"]["kind"].as_str(), hit["widened"].is_null()),
        (Some("hit"), Some("hit"), true),
        "{hit}"
    );
    assert_eq!(hit["untrusted"]["text"], "not a heading".chars().take(3).collect::<String>(), "{hit}");
    assert_eq!(hit["untrusted"]["link"], "doc.pdf#page=2", "{hit}");
    assert_eq!(hit["refs"]["self"], token.as_str(), "{hit}");
    assert_eq!(resolves(&app, hit["refs"]["up"].as_str().expect("up"))["page"], 2, "{hit}");
    assert!(
        hit["refs"]["next"].is_null() && hit["refs"]["prev"].is_null(),
        "no search, no neighbouring hits: {hit}"
    );
    let page = get(&app, Level::ReadOnly, &token, json!({ "grain": "page" })).expect("the page");
    assert_eq!(page["untrusted"]["text"].as_str().map(str::trim), Some(whole.as_str()), "{page}");
    assert_eq!(page["ref"]["chars"], json!([2, 5]), "the characters were checked: {page}");
    for (which, number) in [("next", 3), ("prev", 1), ("self", 2)] {
        let said = resolves(&app, page["refs"][which].as_str().unwrap_or_else(|| panic!("{which}: {page}")));
        assert_eq!(
            (said["status"].as_str(), said["page"].as_u64()),
            (Some("found"), Some(number)),
            "{which}: {said}"
        );
    }
    let markdown = get(&app, Level::ReadOnly, &token, json!({ "grain": "page", "as": "markdown" })).expect("markdown");
    let body = markdown["untrusted"]["markdown"].as_str().expect("markdown");
    let shown: Vec<&str> = body.lines().filter(|line| !line.is_empty()).collect();
    assert_eq!(shown.len(), lines.len(), "{body}");
    for line in &shown[..3] {
        assert!(
            line.starts_with('\\'),
            "a line of the page that starts like Markdown is escaped: {line:?} in {body}"
        );
    }
    assert_eq!(shown[3], "1\\. not a list", "a number and a dot is escaped at the dot: {body}");
    assert!(
        !body
            .lines()
            .any(|line| line.starts_with('#') || line.starts_with('>') || line.starts_with("~~~")),
        "{body}"
    );
    // No folder at any level, and the same answer at each that answers at all.
    for level in [Level::ReadOnly, Level::Control, Level::Full] {
        let said = get(&app, level, &token, json!({ "grain": "page" })).expect("an answer");
        assert!(!said.to_string().contains(&folder), "no folder at {level:?}: {said}");
    }
}

/// Failure: an agent at Off reads a document through `get`, or an agent gains a way to touch the owner's edits by it. Off is refused and every level above
/// answers; reading with a person's unsaved edit on the tab changes nothing of the tab (the edit is still the owner's, and the page asked for is the file's).
#[test]
fn get_is_a_read_and_leaves_the_owners_edits_alone() {
    let app = HeadlessApp::start();
    let file = app.disk().file("doc.pdf", &pdf_bytes("Mine", 3));
    app.open(&file);
    let token = reference(&app, 2);
    let refused = get(&app, Level::Off, &token, json!({})).expect_err("nothing at Off");
    assert_eq!(refused.exit, exit::PERMISSION, "{refused:?}");
    app.person(r##"{"cmd":"page_rotate","tab":"#1","pages":"2","degrees":90}"##);
    app.settle();
    let before = app.session_json();
    for level in [Level::ReadOnly, Level::Control, Level::Full] {
        let said = get(&app, level, &token, json!({ "grain": "page" })).expect("a read");
        assert!(
            said["untrusted"]["text"].as_str().is_some_and(|text| text.contains("Mine page 2")),
            "{level:?}: {said}"
        );
    }
    assert_eq!(app.session_json()["tabs"], before["tabs"], "a read changed the tab");
    let discard = app.agent(Level::Control, r#"{"cmd":"discard"}"#).expect_err("still the owner's edit");
    assert_eq!(discard.exit, exit::PERMISSION, "{discard:?}");
}

/// Failure: a protected document is read through `get`, or its refusal comes after the reference was resolved (so an agent learns whether a page is in it
/// from the words of the refusal). The refusal is the one `text` gives, with a reference that points nowhere: it is the document's, not the reference's.
#[test]
fn a_protected_document_is_refused_before_the_reference_is_resolved() {
    let disk = Disk::new();
    let locked = disk.file("locked.pdf", &locked_saying(PASSWORD, 2, "Locked", lopdf::encryption::Permissions::all()));
    let app = disk.start();
    person_unlocks(&app, &locked);
    let text = app
        .agent(Level::Full, &json!({ "cmd": "text", "page": 1, "tab": "#1" }).to_string())
        .expect_err("text is refused");
    let refused = get(&app, Level::Full, NOWHERE, json!({})).expect_err("get is refused");
    assert_eq!((refused.code, refused.exit, &refused.message), (text.code, text.exit, &text.message));
    assert_eq!(refused.exit, exit::PROTECTED);
}

/// Failure: a page another program moved is refused, or read as the page number it had; a page it removed is answered with another page; a reference to
/// characters whose text changed is answered. `moved` answers the page the words are on now, saying so; `gone` and a changed text are `reference_unresolved`.
#[test]
fn get_follows_a_moved_page_and_refuses_a_gone_one() {
    let app = HeadlessApp::start();
    let file = app.disk().file("doc.pdf", &pdf_bytes("Mine", 5));
    app.open(&file);
    let page = reference(&app, 3);
    let words = chars_ref(&app, 3, 0, 4);
    replace(
        &file,
        &pages_saying(&["Cover", "Mine page 1", "Mine page 2", "Mine page 3", "Mine page 4", "Mine page 5"]),
    );
    look(&app);
    let moved = get(&app, Level::ReadOnly, &page, json!({ "grain": "page" })).expect("moved answers");
    assert_eq!(moved["ref"]["status"], "moved", "{moved}");
    assert!(moved["untrusted"]["text"].as_str().is_some_and(|text| text.contains("Mine page 3")), "{moved}");
    assert_eq!(moved["untrusted"]["link"], "doc.pdf#page=4", "{moved}");
    let hit = get(&app, Level::ReadOnly, &words, json!({})).expect("the words moved with their page");
    assert_eq!(hit["untrusted"]["text"], "Mine", "{hit}");
    // A page found whose text at the characters isn't what the reference was made of: refused, and no text of it given.
    let hostile = hostile_chars(&words, 0, 4);
    for grain in ["hit", "page"] {
        let changed = get(&app, Level::ReadOnly, &hostile, json!({ "grain": grain })).expect_err("text changed");
        assert_eq!(
            (changed.code.as_str(), changed.exit),
            ("reference_unresolved", exit::NOT_FOUND),
            "{grain}: {changed:?}"
        );
    }
    replace(&file, &pages_saying(&["Mine page 1", "Mine page 2", "Mine page 4", "Mine page 5"]));
    look(&app);
    let gone = get(&app, Level::ReadOnly, &page, json!({})).expect_err("gone");
    assert_eq!((gone.code.as_str(), gone.exit), ("reference_unresolved", exit::NOT_FOUND), "{gone:?}");
}

/// Failure: the cap is one off. At the limit a unit is given whole; one character under it the answer is cut and says so, with the next page to go on
/// with; and a cap past 65,536 is refused as usage, as the line is read.
#[test]
fn max_chars_is_held_at_the_limit_and_one_over() {
    let app = HeadlessApp::start();
    let file = app.disk().file("doc.pdf", &pdf_bytes("Mine", 2));
    app.open(&file);
    let token = reference(&app, 1);
    let size = page_text(&app, 1).chars().count();
    let at = get(&app, Level::ReadOnly, &token, json!({ "grain": "page", "max_chars": size })).expect("at the limit");
    assert_eq!(
        (at["truncated"].as_bool(), at["counts"]["chars_given"].as_u64()),
        (Some(false), Some(size as u64)),
        "{at}"
    );
    let under = get(&app, Level::ReadOnly, &token, json!({ "grain": "page", "max_chars": size - 1 })).expect("one under");
    assert_eq!(under["truncated"], true, "{under}");
    assert_eq!(under["counts"]["chars_given"].as_u64(), Some(size as u64 - 1), "{under}");
    assert_eq!(resolves(&app, under["refs"]["next"].as_str().expect("a way on"))["page"], 2, "{under}");
    for bad in [0, 65_537] {
        let error = get(&app, Level::ReadOnly, &token, json!({ "max_chars": bad })).expect_err("out of range");
        assert_eq!(error.exit, exit::USAGE, "{bad}: {error:?}");
    }
    let most = get(&app, Level::ReadOnly, &token, json!({ "max_chars": 65_536 })).expect("the most");
    assert_eq!(most["truncated"], false, "{most}");
}

/// Failure: the text of record is read from the index when there is one, and the answer differs from the page's own text (a page read two ways reads two
/// ways), or the index is believed after the file changed. With a finished index the answer says `index` and gives the page's words as `text` does; after
/// the file is replaced the index is no longer this file's, and the answer is read from the file.
#[test]
fn get_reads_the_index_when_it_is_this_files_and_the_file_when_not() {
    let disk = Disk::new();
    let file = disk.file("doc.pdf", &pdf_bytes("Mine", 3));
    let app = disk.builder().indexing(true).start();
    app.open(&file);
    app.recto(Level::Full, &["index"]).expect("asked for");
    app.wait_until("the index is built", || {
        app.recto(Level::Full, &["index", "status"]).is_ok_and(|status| status["state"] == "ready")
    });
    let token = reference(&app, 2);
    let indexed = get(&app, Level::ReadOnly, &token, json!({ "grain": "page" })).expect("from the index");
    assert_eq!(indexed["read_from"], "index", "{indexed}");
    assert_eq!(
        indexed["untrusted"]["text"].as_str().map(str::trim),
        Some(page_text(&app, 2).as_str()),
        "{indexed}"
    );
    replace(&file, &pdf_bytes("Mine", 3));
    look(&app);
    let after = get(&app, Level::ReadOnly, &token, json!({ "grain": "page" })).expect("from the file");
    assert_eq!(after["read_from"], "file", "{after}");
}

/// Failure: a grain that needs blocks gives nothing, or a pretence of one. Paragraph, section and chapter answer the page and say why they didn't give
/// more; a hit of a reference to a page says that too.
#[test]
fn a_grain_that_needs_blocks_answers_the_page_and_says_so() {
    let app = HeadlessApp::start();
    let file = app.disk().file("doc.pdf", &pdf_bytes("Mine", 2));
    app.open(&file);
    let token = reference(&app, 1);
    for grain in ["paragraph", "section", "chapter"] {
        let said = get(&app, Level::ReadOnly, &token, json!({ "grain": grain })).expect("widened");
        assert_eq!(said["widened"], json!({ "to": "page", "why": "blocks_not_made" }), "{grain}: {said}");
        assert_eq!((said["grain"].as_str(), said["unit"]["kind"].as_str()), (Some(grain), Some("page")), "{said}");
    }
    let hit = get(&app, Level::ReadOnly, &token, json!({})).expect("a page for a hit of a page");
    assert_eq!(hit["widened"], json!({ "to": "page", "why": "ref_is_page" }), "{hit}");
}

/// Failure: a page whose stored copy was never checked for hidden text (the indexer stops checking after repeated failures) is given from the index with
/// its hidden words unmarked, where `text --ref` marks them, so an agent reads hidden instructions as visible text. The page is read live unless the index
/// checked it and dropped no spans; either way the answer says `hidden_text`, and as Markdown the hidden words are inside `[hidden: ...]`.
#[test]
fn a_page_the_index_did_not_check_for_hidden_text_is_read_live_and_marked() {
    let disk = Disk::new();
    let file = disk.file(
        "hiding.pdf",
        &recto_testpdf::pages(&[recto_testpdf::Page::new(recto_testpdf::hidden_text_page())]),
    );
    let app = disk.builder().indexing(true).start();
    app.open(&file);
    app.recto(Level::Full, &["index"]).expect("asked for");
    app.wait_until("the index is built", || {
        app.recto(Level::Full, &["index", "status"]).is_ok_and(|status| status["state"] == "ready")
    });
    let token = reference(&app, 1);
    let said = get(&app, Level::ReadOnly, &token, json!({ "grain": "page", "as": "markdown" })).expect("an answer");
    let body = said["untrusted"]["markdown"].as_str().expect("markdown");
    assert!(body.contains("[hidden: "), "hidden words are marked, from whichever copy was read: {said}");
    assert!(
        said["hidden_text"]["checked"].is_boolean() && said["hidden_text"]["partial"].is_boolean(),
        "{said}"
    );
    assert!(
        !body.lines().any(|line| line.contains("HIDDEN-WHITE") && !line.contains("[hidden: ")),
        "no hidden line is shown as plain text: {body}"
    );
}

/// A page of about 70,000 characters: 700 lines of 100 letters, drawn one point high so they fit the page.
fn long_page() -> Vec<u8> {
    let mut content = String::from("BT /F1 1 Tf 1 TL 5 785 Td\n");
    for line in 0..700 {
        content.push_str(&format!("({}) Tj T*\n", format!("{:04}", line).repeat(25)));
    }
    recto_testpdf::pages(&[recto_testpdf::Page::new(content.into_bytes())])
}

/// Failure (S2's review): a page over 65,536 characters can't be read to its end, as `refs.more` is missing; or the cut is in another place for Markdown than for text, so a `more` reference means
/// two things. A page of about 70,000 characters at the most an answer holds: two calls, the second with `--from-here` on the first's `refs.more`, give the whole page once. At a small cap text and Markdown
/// give the same `refs.more` and `counts.chars_given`.
#[test]
fn a_page_over_65536_characters_is_read_to_its_end() {
    let app = HeadlessApp::start();
    let file = app.disk().file("long.pdf", &long_page());
    app.open(&file);
    let whole = app.recto(Level::ReadOnly, &["text", "--page", "1"]).expect("its text")["untrusted"]["text"]
        .as_str()
        .unwrap_or_default()
        .to_owned();
    let total = whole.chars().count();
    assert!(total > 65_536, "the fixture is a page over the most an answer holds: {total}");
    let token = reference(&app, 1);
    let first = get(&app, Level::ReadOnly, &token, json!({ "grain": "page", "max_chars": 65_536 })).expect("the first part");
    // Cut at the last line end inside the cap, so a little under it.
    let given = first["counts"]["chars_given"].as_u64().unwrap_or(0);
    assert!((60_000..=65_536).contains(&given) && first["truncated"] == true, "{first}");
    let more = first["refs"]["more"].as_str().expect("a way on").to_owned();
    let second = get(&app, Level::ReadOnly, &more, json!({ "grain": "page", "max_chars": 65_536, "from_here": true })).expect("the rest");
    assert_eq!(second["truncated"], false, "{second}");
    let joined = format!(
        "{}{}",
        first["untrusted"]["text"].as_str().unwrap_or_default(),
        second["untrusted"]["text"].as_str().unwrap_or_default()
    );
    assert_eq!(joined.chars().count(), total, "the whole page once: nothing lost, nothing twice");
    assert_eq!(joined.trim(), whole.trim());

    let small = |form: &str| get(&app, Level::ReadOnly, &token, json!({ "grain": "page", "max_chars": 10, "as": form })).expect("a small cap");
    let (text, markdown) = (small("text"), small("markdown"));
    assert_eq!(text["refs"]["more"], markdown["refs"]["more"], "one cut whatever the form: {text} {markdown}");
    assert_eq!(text["counts"]["chars_given"], markdown["counts"]["chars_given"]);
    assert!(text["refs"]["more"].is_string());
}

/// Failure: `get --grain section` reads the contents and the pages of a tab whose own pages are withheld (its file replaced under unsaved edits), which `contents` refuses, so an agent gets a
/// section of a file the edits' page numbers aren't about. The refusal is `contents`'s, in the same code.
#[test]
fn a_section_on_a_tab_whose_pages_are_withheld_is_refused_as_contents_is() {
    let app = HeadlessApp::start();
    let file = app.disk().file("doc.pdf", &pdf_bytes("Mine", 10));
    app.open(&file);
    let token = reference(&app, 2);
    app.recto(Level::Control, &["page", "rotate", "9", "--by", "90"]).expect("rotated");
    app.settle();
    replace(&file, &pdf_bytes("Theirs", 5));
    look(&app);
    let contents = app.recto(Level::ReadOnly, &["contents"]).expect_err("contents refuses");
    for grain in ["section", "chapter", "paragraph", "page", "hit"] {
        let said = get(&app, Level::ReadOnly, &token, json!({ "grain": grain })).expect_err("the pages are withheld");
        assert_eq!(said.code, contents.code, "{grain}: {said:?}");
    }
}
