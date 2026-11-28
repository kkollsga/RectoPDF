//! `get`'s own tests, over a real session with a real file open.

use super::*;
use crate::locating::read_reference;
use recto_platform::slow::{self, Seam};
use recto_session::Session;
use std::sync::Mutex;

/// Failure: a stored page is given though the index never checked it for hidden text, or dropped some of its stretches, so its hidden words show as
/// plain text. Only a checked page with no dropped stretch is enough.
#[test]
fn only_a_checked_page_with_no_dropped_stretch_is_taken_from_the_index() {
    assert!(index_is_enough(true, false));
    assert!(!index_is_enough(false, false));
    assert!(!index_is_enough(true, true));
}

fn pdf(who: &str, pages: usize) -> Vec<u8> {
    let pages: Vec<recto_testpdf::Page> = (1..=pages)
        .map(|page| recto_testpdf::Page::new(recto_testpdf::text_lines(&[&format!("{who} page {page}")])))
        .collect();
    recto_testpdf::pages(&pages)
}

fn open(bytes: &[u8]) -> (recto_testpdf::TempDir, std::path::PathBuf, Mutex<Session<crate::DrawableLoader>>) {
    let dir = recto_testpdf::TempDir::new("recto-getting-");
    let path = dir.join("doc.pdf");
    std::fs::write(&path, bytes).unwrap();
    let mut session = Session::new(crate::DrawableLoader);
    session.open(&path).unwrap();
    (dir, path, Mutex::new(session))
}

/// What a plain `get` of page `page` asks for: the page's own reference.
fn page_ask(access: &Mutex<Session<crate::DrawableLoader>>, page: u32) -> Locator {
    let made = crate::locating::reference(access, &TabRef::Active, page, None).expect("a reference");
    read_reference(made["ref"].as_str().expect("its token")).expect("a reference")
}

fn asked<'a>(reference: &'a Locator, grain: Grain) -> Asked<'a> {
    Asked {
        tab: &TabRef::Active,
        reference,
        grain,
        form: Form::Text,
        max_chars: DEFAULT_MAX_CHARS,
        from_here: false,
        entry: None,
    }
}

/// Failure: the file is replaced by another program after `get` has read the page and before it answers, and the answer is given as the old file's words
/// (or its index key isn't looked at again): an agent is told text of a version the file no longer has. `get` held at its end check ([`Seam::Get`]) while
/// the file is replaced says `file_changed` and gives nothing; the same call with the file left alone answers.
#[test]
fn a_file_replaced_after_get_has_read_it_is_refused_not_answered() {
    let (_dir, path, access) = open(&pdf("Mine", 3));
    let reference = page_ask(&access, 2);
    let alone = get(&access, &mut Vec::new(), &asked(&reference, Grain::Page)).expect("the file is as it was");
    assert!(alone["untrusted"]["text"].as_str().is_some_and(|text| text.contains("Mine page 2")), "{alone}");

    slow::gate::hold(&[Seam::Get]);
    let outcome = std::thread::scope(|scope| {
        let call = scope.spawn(|| get(&access, &mut Vec::new(), &asked(&reference, Grain::Page)));
        let began = std::time::Instant::now();
        while slow::gate::waiting(Seam::Get) == 0 {
            assert!(began.elapsed() < std::time::Duration::from_secs(30), "get never reached its end check");
            std::thread::yield_now();
        }
        // Another program's save, of another length, under the read.
        let incoming = path.with_extension("incoming");
        std::fs::write(&incoming, pdf("Theirs", 5)).unwrap();
        std::fs::rename(&incoming, &path).unwrap();
        slow::gate::open(Seam::Get);
        call.join().expect("get's thread")
    });
    let error = outcome.expect_err("an answer about a file that is gone is refused");
    assert_eq!((error.code, error.message.as_str()), (Code::FILE_CHANGED, CHANGED_WHILE_READ));
}
