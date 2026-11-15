//! **The Find text, sealed, in the app as it runs** (owner, 2026-11-15: "Find text is a indexed copy of the pdf text? Lets store it encrypted"): the headless app with indexing, as `start` makes it, on its own state folder.

use crate::testkit::headless::Disk;
use recto_agent::Level;
use recto_index::corpus::CorpusKey;
use recto_platform::find_key::FindKey;

/// Every byte under a folder, read as text.
fn every_byte_under(folder: &std::path::Path) -> String {
    let mut found = String::new();
    let mut stack = vec![folder.to_path_buf()];
    while let Some(next) = stack.pop() {
        for entry in std::fs::read_dir(&next).into_iter().flatten().flatten() {
            if entry.file_type().is_ok_and(|kind| kind.is_dir()) {
                stack.push(entry.path());
            } else if let Ok(bytes) = std::fs::read(entry.path()) {
                found.push_str(&String::from_utf8_lossy(&bytes));
            }
        }
    }
    found
}

/// Failure: after the app builds a document's index and searches it, a word of the document is in a file under its state folder as it is, so any program running as the person reads the Find text there (the record, the logs, the corpus, a sidecar, anything); or the index isn't what the search read, so the test would pass with nothing kept. The words are kept, sealed: the state folder's own key opens them, and `qa state` says where the key came from.
#[test]
fn no_word_of_a_document_is_on_disk_as_it_is() {
    let disk = Disk::new();
    let words = ["quixotic zephyr marmalade", "perihelion obsidian lantern"];
    let pages: Vec<recto_testpdf::Page> = words.iter().map(|line| recto_testpdf::Page::new(recto_testpdf::text_lines(&[line]))).collect();
    let file = disk.file("sealed.pdf", &recto_testpdf::pages(&pages));
    let app = disk.builder().indexing(true).start();
    app.open(&file);
    app.recto(Level::Full, &["index"]).expect("asked for");
    app.wait_until("the index is built", || {
        app.recto(Level::Full, &["index", "status"]).is_ok_and(|status| status["state"] == "ready")
    });
    let found = app.recto(Level::Full, &["search", "zephyr"]).expect("a search");
    assert_eq!(
        (found["route"].as_str(), found["coverage"]["hits_total"].as_u64()),
        (Some("index"), Some(1)),
        "{found}"
    );
    app.settle();
    let paths = app.disk().paths().clone();
    let kept = every_byte_under(&paths.root);
    for word in words.iter().flat_map(|line| line.split(' ')) {
        assert!(!kept.contains(word), "`{word}` is in the state folder as it is");
    }
    let key = FindKey::from_state_file(&paths.root.join("find.key")).expect("the state folder's key");
    let opened = recto_index::corpus::opened_text(&key, &paths.corpus, CorpusKey::of_file(&file).unwrap()).join("\n");
    assert!(
        opened.contains("zephyr") && opened.contains("obsidian"),
        "the index the search read isn't the one on disk: {opened}"
    );
    assert_eq!(app.core().host.session.find_key_report()["source"], "state_file");
    // Built by the real helper with the state folder's key, which lasts: its header says so, and a run on a key in memory leaves it (the security review of slices 2 and 3, S1: a key handed to the helper said it didn't last, so every such run swept the app's corpora).
    let corpus = recto_index::corpus::CorpusPaths::new(&paths.corpus, CorpusKey::of_file(&file).unwrap());
    let header = std::fs::read(corpus.app.join("seal.bin")).expect("the corpus's seal");
    assert_eq!(
        recto_index::seal::CorpusSeal::lasting(&header),
        Some(true),
        "the helper sealed with a key it said doesn't last"
    );
    recto_index::corpus::remove_unreadable(&FindKey::in_memory().unwrap(), &paths.corpus);
    assert!(
        corpus.app.join("seal.bin").is_file(),
        "a run on a key in memory swept a corpus the lasting key sealed"
    );
}
