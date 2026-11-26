//! **Every open of a file by its path in the crates a path from a command reaches goes through the file wall** (`recto_platform::document_file`), or
//! is on a reviewed list with its reason. A new open of a path an agent names, outside the wall, would open `/dev/fd/N`, the person's print, again (S1);
//! this fails it until it goes through the wall or is listed, by a reviewer, as never given such a path. It reads every production line, as the actor
//! scan does ([`crate::policy_guards::production_lines`], [`crate::actor::scan_tests::test_files`]): a test item early in a file hides nothing after it.

use crate::actor::scan_tests::{sources, test_files};
use crate::policy_guards::production_lines;
use std::path::Path;

/// The crates whose code a path from a command, a record, a restore, a drop or an Apple event reaches, and the platform's, but for the wall itself.
const CRATES: &[&str] = &[
    "recto-agent",
    "recto-app",
    "recto-control",
    "recto-core",
    "recto-ffi",
    "recto-index",
    "recto-platform",
    "recto-reader",
    "recto-render-service",
    "recto-session",
];

/// The wall, which is where a document's path is opened.
const THE_WALL: &str = "recto-platform/src/document_file.rs";

/// What opens, reads, copies or clones a file by its path, other than the wall.
const OPENS: &[&str] = &[
    "File::open(",
    "File::options(",
    "OpenOptions::new(",
    "FileSource::open(",
    "fs::read(",
    "fs::read_to_string(",
    "fs::copy(",
    "clonefile",
    "clone_file(",
    // The platform's reads of a path it's handed: a new caller of either is counted where it calls.
    "open_untrusted(",
    "open_private_file(",
    // A descriptor-relative open, and the folder helper's two: a folder by its path, and a file by a name in an open folder.
    "openat(",
    "libc::open(",
    "Folder::open(",
    "open_private_in(",
];

/// **The reviewed list:** each file of those crates that opens a file by its path other than through the wall, how many times, and why none of those
/// paths can be one a caller names as a document to read. Sorted by file.
const OPENS_OUTSIDE_THE_WALL: &[(&str, usize, &str)] = &[
    (
        "recto-agent/src/render.rs",
        4,
        "files this process makes and names itself: an export's temp file (`OwnTemp::create`, `create_new`), the print's file with no name \
         (`create_unnamed`, `create_new`), an output written new (`create_new`, never over a file), and the temp file it made read back to copy it there",
    ),
    (
        "recto-agent/src/skill.rs",
        3,
        "`recto skill install`, the user's own command in `recto`'s process (Route::Here, UserOnly): its own SKILL.md in the folder named, read to compare \
         and written new; never a document, never in the app",
    ),
    (
        "recto-control/src/record_store.rs",
        3,
        "the state folder's own record files, named by the app: flushed to the drive, a kept copy written, and a record that couldn't be read copied aside",
    ),
    ("recto-control/src/record_store/lock.rs", 1, "the state folder's lock file, named by the app"),
    (
        "recto-control/src/record_store/sleep_files.rs",
        1,
        "the state folder's own sleep files, opened (no link followed) by the pass after a launch that removes older ones",
    ),
    (
        "recto-control/src/record_store/went_back.rs",
        1,
        "the state folder's own list kept beside a tab after a copy went back, its first line read (no link followed) to say whose it is",
    ),
    (
        "recto-control/src/removal_store/backups.rs",
        1,
        "the original-backup folder the caller of `survey_backups` names (`pub`, with no production caller yet; the design names the state folder's), opened \
         through the folder helper (no link followed) and only listed: a backup's name is read, never its bytes",
    ),
    (
        "recto-control/src/removal_store/journals.rs",
        2,
        "the `session` and `session-windowless` folders of the root the caller of `survey_journals` names (`pub`, with no production caller yet; the design names \
         the state folder), opened through the folder helper (no link followed), and the `journals` folder in each; the first line of a journal (at most 8 KiB) and \
         a sidecar's 110-byte header read in `session*/journals` through that folder's descriptor (a plain private file only): never a document",
    ),
    (
        "recto-control/src/removal_store/ledger.rs",
        2,
        "the provenance ledger's own file in the state folder, read with a cap (a plain file only; the writer opens it with no link followed): paths and numbers, never a document",
    ),
    (
        "recto-control/src/removal_store/ledger/adopt.rs",
        1,
        "the first line (at most 8 KiB) of a journal in the state folder, a plain file only, read once when the ledger is first made: never a document",
    ),
    (
        "recto-control/src/removal_store/leftovers/act.rs",
        2,
        "Clear leftovers' two `Folder::open` calls, each by a path the surveys pinned (the documents folder of an unfinished save, the app's own state root), \
         never a path from a command: no link is followed at its last part, and the folder's device and inode are then compared with the pin before anything \
         is done in it. Everything below is reached by descriptor",
    ),
    (
        "recto-control/src/removal_store/legacy_session.rs",
        1,
        "the state folder the caller of `survey_legacy_sessions` names (`pub`, with no production caller yet; the design names the state folder), opened through the \
         folder helper (no link followed) and listed: a copy of the session record (`session.v<N>`, `<flavor>.damaged[.k]`) is opened by name from it with no link \
         followed and listed, and its `journals` folder through the journals survey (a journal's first 8 KiB and a sidecar's 110 bytes, a plain private file only). A copy's \
         record slots, `places.log` and every other entry are placed by name and never opened: never a document",
    ),
    (
        "recto-control/src/removal_store/record.rs",
        2,
        "the `session` and `session-windowless` folders of the root the caller of `survey_record` names (`pub`, with no production caller yet; the design names the \
         state folder), opened through the folder helper (no link followed); the two record slots in each read at most 32 MiB + 1 byte, then refused, through that folder's \
         descriptor (a plain private file only): never a document",
    ),
    (
        "recto-control/src/removal_store/update_copy.rs",
        1,
        "the state folder the caller of `survey_update_copy` names (`pub`, with no production caller yet), opened through the folder helper (no link followed), then \
         `update` and `before` opened one from the other with no link followed and listed (the parent is dropped as the next is opened: at most three descriptors, `before`, its `journals` and a journal file), and `before/journals` through the journals \
         survey. The copy's slots, `places.log`, `.whole` and every other entry are placed by name and never opened: never a document",
    ),
    (
        "recto-control/src/removal_store/writer_temps.rs",
        1,
        "the folder of each target-side lineage path the caller of `survey_writer_temps` names (`pub`, with no production caller yet), and of every Save As or Replace destination ever recorded, from any lineage (landed or not): a person's folder, \
         opened through the folder helper (this user's only, no link followed at its last part) and only listed; each entry is looked at by `fstatat` without \
         following a link, and nothing in the folder is opened: never a document",
    ),
    ("recto-control/src/server.rs", 1, "the socket's lock file in the state folder, named by the app"),
    (
        "recto-control/src/terminal.rs",
        1,
        "`/dev/tty`, by that name only: the terminal `recto` asks the person at, in `recto`'s own process",
    ),
    (
        "recto-core/src/save/backups.rs",
        4,
        "`keep_backup`'s clone or copy of the tab's own file, by the path the wall opened it by, resolved, after that open: never a path a command names, \
         but by path where the descriptor the wall admitted would be surer (PLAN, F-A); and its own partial file in the state folder, opened to set its \
         time and flushed before it's renamed into place",
    ),
    (
        "recto-index/src/corpus.rs",
        3,
        "the index's own files under the app's index folder: its table and pack written and read; the pack read again to add up a build's \
         sealed records",
    ),
    (
        "recto-index/src/corpus/agent_life.rs",
        1,
        "the index's own `.session` file in an agent's folder under the app's index folder, read to see whether a live session holds it",
    ),
    (
        "recto-index/src/corpus/identities.rs",
        2,
        "the index's own `ids.bin` under the app's index folder, through the private-file opens (no link, no pipe, an owner's plain file): read back to check \
         the records a build keeps on resume, appended to as pages are built, and read for a stored identity",
    ),
    (
        "recto-index/src/corpus/retry.rs",
        1,
        "the index's own pack under the app's index folder, read again to find the pages a retry reads",
    ),
    (
        "recto-index/src/corpus/sealed.rs",
        1,
        "the index's own pack under the app's index folder, read to say what it keeps of a document: `opened_text`, for the index tests",
    ),
    (
        "recto-platform/src/bin/recto-sandbox-probe.rs",
        4,
        "a test's probe, never in the app: adversarial file attempts, plus capped reads of its own fixture configuration/canary and creation of its exact launch receipt",
    ),
    (
        "recto-platform/src/detached_mac.rs",
        1,
        "the spawned child's app-owned update log; moved from local_mac, never a document path",
    ),
    (
        "recto-platform/src/find_key.rs",
        2,
        "the app's own files and tools, never a document: a test state folder's key file put in place, and the test-only look at a built program for the Keychain's code",
    ),
    (
        "recto-platform/src/folder_mac.rs",
        5,
        "the five opens of `Folder`, which is `pub` and opens any path its caller names: the folder by that path (`OpenOptions`, `O_DIRECTORY | O_NOFOLLOW`), \
         a child folder of it (`open_in`, `openat`), the folder again for each listing (`entries`, `openat` of `.`), and a file in it (`open_private_in`, \
         `openat`, `O_NOFOLLOW | O_NONBLOCK`; a name that is empty, `.`, `..` or holds a `/` is refused, so no name can leave the folder). Used today \
         only by removal's six surveys (`journals.rs`, `backups.rs`, `record.rs`, `writer_temps.rs`, `legacy_session.rs` and `update_copy.rs`); a new caller is counted where it calls",
    ),
    (
        "recto-platform/src/folder_stub.rs",
        1,
        "the folder helper's stand-in on another system: opens nothing",
    ),
    (
        "recto-platform/src/local_mac.rs",
        17,
        "the platform's file primitives, each handed its path by a caller: private state files (links not followed), clones, the untrusted open of \
         `recto`'s update zip in `recto`, a folder flushed, a path opened only to watch it (`O_EVTONLY`, no read). Their reads (`open_untrusted`, `open_private_file`, `clone_file`) \
         are counted again where a caller in these crates makes them",
    ),
    ("recto-platform/src/local_stub.rs", 4, "the same primitives' stand-ins on another system"),
    (
        "recto-platform/src/update_record_lock.rs",
        1,
        "the validated update plan's private state root plus fixed run/session.lock, shared with RecordStore; no document path, links or special files",
    ),
];

/// The opens in a file's production lines, a line comment left out.
fn opens_in(text: &str) -> usize {
    production_lines(text)
        .into_iter()
        .map(|(_, line)| {
            let code = line.split("//").next().unwrap_or_default();
            OPENS.iter().map(|open| code.matches(open).count()).sum::<usize>()
        })
        .sum()
}

/// **Failure (S1 again, by a new route): a new open of a file by its path in these crates, outside the wall,** that a path an agent names could reach:
/// `/dev/fd/N` then opens the person's print, or any descriptor of the app's, again. Every such open is on the reviewed list with its reason, at its
/// count, and nothing else opens a path; the count also falls when one goes, so the list is never wider than the code.
#[test]
fn every_open_of_a_path_goes_through_the_wall_or_is_reviewed() {
    let crates = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let (mut found, mut read) = (Vec::new(), 0);
    for krate in CRATES {
        let src = crates.join(krate).join("src");
        let files = sources(&src);
        let tests = test_files(&src, &files);
        for file in files.iter().filter(|file| !tests.contains(*file)) {
            let name = file.strip_prefix(crates).unwrap().to_string_lossy().replace('\\', "/");
            if name == THE_WALL {
                continue;
            }
            read += 1;
            let opens = opens_in(&std::fs::read_to_string(file).unwrap());
            if opens > 0 {
                found.push((name, opens));
            }
        }
    }
    assert!(read > 300, "the crates' sources were read: {read} files");
    found.sort();
    let listed: Vec<(String, usize)> = OPENS_OUTSIDE_THE_WALL.iter().map(|(name, count, _)| ((*name).to_owned(), *count)).collect();
    assert_eq!(
        found, listed,
        "a file is opened by its path outside the wall (recto_platform::document_file) where OPENS_OUTSIDE_THE_WALL doesn't say why: open a document \
         through the wall, or list a file of the app's own with its reason"
    );
}

/// **Failure: the scan can't see an open**, so the list above passes whatever the code does: one of each kind is counted in code, after an early test
/// item as well as before it, and none in a comment or a test module.
#[test]
fn the_wall_scan_sees_a_seeded_open() {
    for open in OPENS {
        let seeded = format!("fn read(path: &Path) {{\n    let _ = std::{open}path);\n}}\n");
        assert_eq!(opens_in(&seeded), 1, "{open} in: {seeded}");
        let after_a_test_item = format!("#[cfg(test)]\nfn helper() {{\n    let _ = 1;\n}}\n{seeded}");
        assert_eq!(opens_in(&after_a_test_item), 1, "{open} after an early test item");
        let commented: String = seeded.lines().map(|line| format!("// {line}\n")).collect();
        assert_eq!(opens_in(&commented), 0, "{open} in a comment");
        assert_eq!(
            opens_in(&format!("fn f() {{}}\n#[cfg(test)]\nmod tests {{\n{seeded}}}\n")),
            0,
            "{open} in tests"
        );
    }
}
