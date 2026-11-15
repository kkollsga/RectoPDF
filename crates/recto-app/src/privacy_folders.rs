//! **The notice's claim about what stays out of Time Machine and Spotlight, held to the folders the app makes.** `PRIVACY.md` says which folders are kept
//! out of Time Machine (an attribute the stores set) and which out of Spotlight (a name ending `.noindex`). The other tests hold the notice's numbers and
//! phrases; nothing held this kind of claim, which is how a folder could go unmarked without anyone noticing. This test makes every folder by the app's
//! own routes, in the headless app on a temp state, and reads each back.

use crate::testkit::headless::{Disk, HeadlessApp};
use recto_agent::Level;
use recto_platform::local::is_excluded_from_backups;
use std::path::{Path, PathBuf};

/// One folder the app makes under its state (and, in the notice, cache) root: where, whether the notice says it's kept out of Time Machine and out of
/// Spotlight, and the words of the notice's sentence that name it. A folder below a marked one is covered by it (a backup tool excludes a whole subtree); below an unmarked one it needs a row of its own.
struct Folder {
    /// Relative to the state root. (The real app's page text and generated contents are under `~/Library/Caches/RectoPDF/`, beside it in a quiet run.)
    at: &'static str,
    /// The notice says it's kept out of Time Machine.
    claimed_out_of_backups: bool,
    /// The notice says it's kept out of Spotlight.
    claimed_out_of_spotlight: bool,
    /// What the notice calls what's in it: in the sentence about Time Machine when claimed, and not in it when not.
    named_in_notice_as: &'static str,
}

/// Every folder the app makes. **A new folder fails the test until it's listed here, one way or the other**, and a listed one that the notice doesn't
/// promise stays unpromised until the notice and this row change together.
const FOLDERS: &[Folder] = &[
    Folder {
        at: "session",
        claimed_out_of_backups: false,
        claimed_out_of_spotlight: false,
        named_in_notice_as: "",
    },
    Folder {
        at: "session/journals",
        claimed_out_of_backups: true,
        claimed_out_of_spotlight: false,
        named_in_notice_as: "edit history",
    },
    Folder {
        at: "backups.noindex",
        claimed_out_of_backups: true,
        claimed_out_of_spotlight: true,
        named_in_notice_as: "the backups",
    },
    Folder {
        at: "corpus.noindex",
        claimed_out_of_backups: true,
        claimed_out_of_spotlight: true,
        named_in_notice_as: "the page text",
    },
    Folder {
        at: "contents.noindex",
        claimed_out_of_backups: true,
        claimed_out_of_spotlight: true,
        named_in_notice_as: "the generated contents",
    },
    Folder {
        at: "update",
        claimed_out_of_backups: false,
        claimed_out_of_spotlight: false,
        named_in_notice_as: "",
    },
    Folder {
        at: "update/before",
        claimed_out_of_backups: true,
        claimed_out_of_spotlight: false,
        named_in_notice_as: "copy of your session kept for an update",
    },
    // The server's lock: no document, nothing of a person's.
    Folder {
        at: "run",
        claimed_out_of_backups: false,
        claimed_out_of_spotlight: false,
        named_in_notice_as: "",
    },
];

/// The notice's two sentences, from `PRIVACY.md`: what's kept out of Time Machine, and what out of Spotlight.
fn sentences() -> (String, String) {
    let text = std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../PRIVACY.md")).expect("PRIVACY.md");
    let paragraph = text
        .lines()
        .find(|line| line.starts_with("Kept out of Time Machine:"))
        .expect("a paragraph that starts `Kept out of Time Machine:`");
    assert!(
        paragraph.contains("The rest of your session") && paragraph.contains("is not kept out of Time Machine"),
        "the notice says the session's own folder isn't kept out, which is what the table says of `session`: {paragraph}"
    );
    let (backups, spotlight) = paragraph.split_once("Out of Spotlight:").expect("`Out of Spotlight:` in it");
    (backups.to_string(), spotlight.to_string())
}

/// Every folder under `root`, relative, links not followed.
fn folders_under(root: &Path) -> Vec<PathBuf> {
    fn walk(root: &Path, at: &Path, found: &mut Vec<PathBuf>) {
        for entry in std::fs::read_dir(root.join(at)).into_iter().flatten().flatten() {
            if entry.file_type().is_ok_and(|kind| kind.is_dir()) {
                let child = at.join(entry.file_name());
                found.push(child.clone());
                walk(root, &child, found);
            }
        }
    }
    let mut found = Vec::new();
    walk(root, Path::new(""), &mut found);
    found
}

/// Makes every folder by the routes that make it: a session with an edit, a save that removes a page (its backup and the pages kept for the history), a
/// Find index, generated contents, and the copy an update keeps of the session.
fn make_every_folder(app: &HeadlessApp, file: &Path) {
    app.open(file);
    app.recto(Level::Control, &["page", "rotate", "2", "--by", "90"]).expect("rotated");
    app.recto(Level::Control, &["page", "delete", "4"]).expect("deleted");
    app.recto(Level::Full, &["save"])
        .expect("saved over the file, which makes its backup and keeps the page");
    app.settle();
    // The journals are written a moment after the change, on the clock.
    app.advance(6_000);
    app.settle();
    app.recto(Level::ReadOnly, &["search", "doc"]).expect("searched, which makes the page text");
    app.person(r##"{"cmd":"contents_show","which":"generated","tab":"#1"}"##);
    let id = recto_session::TabId(app.tabs()[0].tab);
    app.wait_until("the generated contents kept", || {
        app.core().host.session.with_session(|session| {
            !matches!(
                session.generated().view(id),
                None | Some(recto_session::GeneratedView::Making | recto_session::GeneratedView::NotKept)
            )
        })
    });
    app.settle();
    let files = recto_update::plan::Files::in_state(&app.disk().paths().root);
    assert!(
        recto_update::install::copy_record(&files.session(), &files.before(), &mut |_| {}),
        "the update's copy of the session"
    );
}

/// Failure: a folder the notice says is kept out of Time Machine isn't (a store stops marking it, or a new route makes it unmarked), or one that it says
/// is out of Spotlight doesn't end `.noindex`; or the app makes a folder this table doesn't know, so nobody has said whether the notice should promise it.
#[test]
fn every_folder_the_notice_keeps_out_of_backups_is_marked_and_no_folder_is_unaccounted_for() {
    let disk = Disk::new();
    let file = disk.pdf("doc.pdf", 4);
    let app = disk.builder().indexing(true).start();
    make_every_folder(&app, &file);
    let root = app.disk().paths().root.clone();
    let (backups_sentence, spotlight_sentence) = sentences();
    let mut seen: Vec<&str> = Vec::new();
    for folder in folders_under(&root) {
        let relative = folder.to_str().expect("a name");
        let covered_by = FOLDERS
            .iter()
            .filter(|row| relative == row.at || (row.claimed_out_of_backups && relative.starts_with(&format!("{}/", row.at))))
            .max_by_key(|row| row.at.len());
        let row = covered_by.unwrap_or_else(|| {
            panic!("the app made `{relative}`, which the table doesn't know: say whether PRIVACY.md keeps it out of Time Machine, and add its row")
        });
        if relative == row.at {
            seen.push(row.at);
        }
        // Below a marked folder is covered by it; below an unmarked one, it's no more promised than that one.
        if row.claimed_out_of_backups {
            let marked = std::iter::successors(Some(folder.as_path()), |path| path.parent())
                .take_while(|path| !path.as_os_str().is_empty())
                .any(|path| is_excluded_from_backups(&root.join(path)));
            assert!(
                marked,
                "`{relative}` is kept out of Time Machine by PRIVACY.md and isn't marked (or any folder above it)"
            );
        }
    }
    for row in FOLDERS {
        let named_in_backups = !row.named_in_notice_as.is_empty() && backups_sentence.contains(row.named_in_notice_as);
        let named_in_spotlight = !row.named_in_notice_as.is_empty() && spotlight_sentence.contains(row.named_in_notice_as);
        if row.claimed_out_of_backups {
            assert!(
                named_in_backups,
                "`{}`: the notice's Time Machine sentence doesn't say \"{}\"",
                row.at, row.named_in_notice_as
            );
        }
        assert_eq!(
            row.claimed_out_of_spotlight, named_in_spotlight,
            "`{}`: the Spotlight sentence and the table differ on \"{}\"",
            row.at, row.named_in_notice_as
        );
        // Spotlight leaves out a folder by its name, and by nothing else here.
        assert_eq!(
            row.claimed_out_of_spotlight,
            row.at.ends_with(".noindex"),
            "`{}`: Spotlight skips a folder named `.noindex` and no other",
            row.at
        );
        // Every row is a folder this test made: one that's never made is a row that holds nothing.
        assert!(seen.contains(&row.at), "no route made `{}`: the test makes it, or the row goes", row.at);
    }
}
