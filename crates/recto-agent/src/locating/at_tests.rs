//! What a character reference's [`Located`] holds a command to: it names characters, which are checked against the text of the command's own read before
//! anything is said of them (`--ref` on `select`, `copy`, `highlight` and `snapshot`).

use super::*;
use recto_core::locator::Locator;
use recto_session::{DiskLoader, Session};
use recto_testpdf::{Page, TempPdf, pages, text_lines};
use std::sync::Mutex;

/// A reference to characters 0 to 6 ("Second") of page 2, found in a tab that holds the file, with its characters still to be checked.
fn found_characters() -> (TempPdf, Mutex<Session<DiskLoader>>, Located) {
    let file = TempPdf::new(
        "two.pdf",
        &pages(&[Page::new(text_lines(&["First page"])), Page::new(text_lines(&["Second page"]))]),
    );
    let mut session = Session::new(DiskLoader);
    session.open(&file.0).unwrap();
    let access = Mutex::new(session);
    let whole = reference(&access, &TabRef::Active, 2, None).expect("the page's reference");
    let made = read_reference(whole["ref"].as_str().unwrap()).expect("a reference");
    let range = locator::characters("Second page", 0, 6).unwrap();
    let chars = Locator::new(made.document(), made.page(), Target::Characters(range));
    let found = page_of(&access, &TabRef::Active, &chars, CharsChecked::AgainstItsText).expect("found");
    (file, access, found)
}

fn text_changed(error: &AgentError) -> bool {
    (error.code, error.message.as_str()) == (Code::REFERENCE_UNRESOLVED, UNRESOLVED_TEXT_CHANGED)
}

/// Failure: a command that read the page's text itself but never checked the reference's characters against it (`checked` false) is let through, so an
/// agent is told words about characters nobody looked at. `attach_checked` refuses a character reference that wasn't checked and attaches nothing, and
/// answers once it was.
#[test]
fn attach_checked_refuses_characters_that_were_never_checked() {
    let (_file, access, found) = found_characters();
    let mut data = json!({});
    let refused = attach_checked(&access, &found, false, &mut data).expect_err("never checked");
    assert!(text_changed(&refused), "{}", refused.message);
    assert_eq!(data, json!({}), "nothing is attached to a refusal");
    attach_checked(&access, &found, true, &mut data).expect("checked");
    assert_eq!(data["ref"]["text"], "same", "{data}");
}

/// Failure: `held_in` lets through a page whose text isn't the words the reference was made of (another program changed them), so characters of another
/// text are selected, copied or highlighted. It refuses changed text, and a page too short to hold the characters, and passes the words that were checked.
#[test]
fn held_in_refuses_text_that_changed_and_passes_the_words_it_was_made_of() {
    let (_file, _access, found) = found_characters();
    found.held_in("Second page").expect("the same words");
    for changed in ["Secone page", "Sec", ""] {
        let refused = found.held_in(changed).expect_err(changed);
        assert!(text_changed(&refused), "{changed:?}: {}", refused.message);
    }
}

/// Failure: the answer's `ref` doesn't say its characters were checked, so an agent can't tell a checked reference from one that wasn't. `told` says
/// `text: same` and the characters for a reference to characters, and neither for a page.
#[test]
fn told_says_the_words_were_the_same_and_which_characters_for_characters_only() {
    let (_file, access, found) = found_characters();
    let said = found.told();
    assert_eq!((&said["text"], &said["chars"]), (&json!("same"), &json!([0, 6])), "{said}");
    let whole = reference(&access, &TabRef::Active, 2, None).expect("the page's reference");
    let page = read_reference(whole["ref"].as_str().unwrap()).expect("a reference");
    let said = page_of(&access, &TabRef::Active, &page, CharsChecked::AgainstItsText).expect("found").told();
    assert!(said.get("text").is_none() && said.get("chars").is_none(), "{said}");
}
