//! **Going dormant and waking, held as one rule:** a tab is warm, cold with a sleep file on its way, or cold, and never two of them at once; and a list
//! asleep reads back from its journal as what's kept of it, unless its journal's gone, when the tab says so. Driven through every way a tab goes dormant
//! and wakes at the session's seam, the way the app's sleeper and the executor call it.

use super::*;
use std::fmt::Write as _;
use std::sync::Mutex;

/// What the state record's store holds, in memory: journals by name, and sleep files by the file they're about.
#[derive(Default)]
struct Store {
    journals: Mutex<HashMap<String, Vec<u8>>>,
    sleeps: Mutex<Vec<(FileStamp, PathBuf, Vec<u8>)>>,
}

impl HistorySource for Store {
    fn history_of(&self, _stamp: Option<FileStamp>, _path: &Path) -> Option<TakenHistory> {
        None
    }

    fn read_list(&self, name: &str) -> Option<RestoredEdits> {
        let journals = self.journals.lock().unwrap();
        let parsed = record::parse_journal(journals.get(name)?, record::CURRENT_VERSION).ok()?;
        let (edits, applied) = PageEdits::replay(parsed.header.file_pages, parsed.base.as_ref(), &parsed.ops).ok()?;
        Some(RestoredEdits {
            edits,
            seq: parsed.header.base_seq + applied as u64,
            lost_lines: false,
            rolled_off: false,
        })
    }

    fn read_sleep(&self, stamp: FileStamp, path: &Path) -> Option<sleep::SleepFile> {
        let sleeps = self.sleeps.lock().unwrap();
        let (_, _, bytes) = sleeps.iter().find(|(at, named, _)| *at == stamp && named == path)?;
        sleep::decode_sleep(bytes, record::CURRENT_VERSION).ok()
    }
}

impl Store {
    /// A sleep file written as the app's sleeper writes one from a tab's facts, with no pictures.
    fn write_sleep(&self, facts: &SleepFacts) {
        let mut sleeps = self.sleeps.lock().unwrap();
        sleeps.retain(|(at, named, _)| !(*at == facts.sleep.file && *named == facts.path));
        sleeps.push((facts.sleep.file, facts.path.clone(), sleep::encode_sleep(&facts.sleep)));
    }

    /// The session's journal work, written.
    fn write_journals<L: DocLoader>(&self, s: &mut Session<L>) {
        let mut plain = self.journals.lock().unwrap().clone();
        write_journal(s, &mut plain);
        *self.journals.lock().unwrap() = plain;
    }
}

/// Five pages, with contents titled `title`, whatever the file.
struct Titled(&'static str);

impl DocLoader for Titled {
    fn load(&self, _path: &Path) -> Result<DocFacts, OpenError> {
        Ok(DocFacts {
            page_count: 5,
            byte_len: 1,
            encrypted: false,
            version: "1.7".into(),
            title: None,
            toc: Toc::from_lines(
                &[TocLine {
                    title: self.0,
                    page: 1,
                    parent: recto_core::NONE,
                    depth: 0,
                }],
                false,
            ),
            renderer_only: false,
        })
    }
}

/// Real files, so tabs have stamps and so sleep files: `names` in a folder of its own.
fn files(names: &[&str]) -> (recto_testpdf::TempDir, Vec<PathBuf>) {
    let dir = recto_testpdf::TempDir::new_in(&std::env::temp_dir().canonicalize().unwrap(), "recto-sleeper-");
    std::fs::create_dir_all(&dir).unwrap();
    let paths = names
        .iter()
        .map(|name| {
            let path = dir.join(name);
            std::fs::write(&path, name.as_bytes()).unwrap();
            path
        })
        .collect();
    (dir, paths)
}

/// A session that keeps journals and sleep files, in `store`.
fn sleeping_session(store: &Arc<Store>, title: &'static str) -> Session<Titled> {
    let mut s = Session::new(Titled(title));
    s.journals_mut().set_journaling(true);
    s.journals_mut().set_history(Arc::clone(store) as Arc<dyn HistorySource>);
    s.dormancy_mut().set_sleep_files(true);
    s
}

/// **The rule, after a step:** every tab is in exactly one of warm, cold with a sleep file on its way, and cold; and every list asleep reads back
/// from its journal as its summary, except those in `unreadable`, whose journals the step took away, which say so on the tab once a wake has tried.
fn holds<L: DocLoader>(s: &Session<L>, store: &Store, unreadable: &[TabId], step: &str) {
    for tab in s.tabs().iter() {
        let warm = s.warm.contains(tab.id);
        let leaving = s.dormancy.holding(tab.id).iter().any(|(part, holds)| *part == "left_warm" && *holds);
        assert!(
            !(warm && leaving),
            "{step}: tab {} is warm and cold with a sleep file on its way at once",
            tab.id.0
        );
    }
    for (id, summary) in s.lists.asleep() {
        if unreadable.contains(id) {
            continue;
        }
        let name = s
            .journals()
            .name_of(*id)
            .unwrap_or_else(|| panic!("{step}: tab {}'s list is asleep with no journal", id.0));
        let back = store
            .read_list(name)
            .unwrap_or_else(|| panic!("{step}: tab {}'s list is asleep and its journal doesn't read", id.0));
        assert_eq!(back.edits.summary(), *summary, "{step}: tab {}'s journal is behind its list", id.0);
    }
}

/// The app's sleeper sending these to sleep: what's waiting goes to the journals first, its sleep file is written, its list goes if it's all on disk,
/// and one that doesn't go is touched, to be looked at again next time it's left alone.
fn go_dormant<L: DocLoader>(s: &mut Session<L>, store: &Store, tabs: &[TabId]) -> usize {
    store.write_journals(s);
    let mut gone = 0;
    for &id in tabs {
        if let Some(facts) = s.sleep_facts(id) {
            store.write_sleep(&facts);
        }
        let _ = s.list_to_sleep(id);
        if s.go_dormant(id) {
            gone += 1;
        } else {
            s.touch(id);
        }
    }
    gone
}

/// The app showing a tab, as an agent's `tab` command does: its sleep file if it has one, its PDF read with the session free if not, then the tap.
fn show<L: DocLoader>(s: &mut Session<L>, id: TabId) {
    if let Some((tab, path)) = s.wake_for_tap(id) {
        let read = s.loader().load(&path);
        s.give_woken(tab, read, file_stamp(&path).ok());
    }
    s.tap(id).unwrap();
}

/// **Failure: a tab is warm and cold with a sleep file on its way at once, or a list goes to sleep with some of it not on disk.** The first writes a
/// sleep file for a tab that's on screen (wasted writes on the person's drive, the owner's "reduce SSD wear as much as we can"), and after a file's
/// found to be another version, can write what was read of the old one into a new sleep file; the second loses edits after a crash. Driven through
/// every way of going dormant (left alone, pushed out of the warm ones, the budget and memory pressure, asked, at quit) and of waking (shown, an edit,
/// a command that reads pages, and a wake that fails on a journal that's gone), with the rule asserted after each.
#[test]
fn a_tab_is_warm_cold_with_a_sleep_file_on_its_way_or_cold_and_a_list_asleep_is_on_disk() {
    let store = Arc::new(Store::default());
    let (_dir, paths) = files(&["a.pdf", "b.pdf", "c.pdf", "d.pdf", "e.pdf"]);
    let mut s = sleeping_session(&store, "Contents");
    let a = s.open(&paths[0]).unwrap();
    s.edit_pages(a, |e| e.rotate(0, 1, 1)).unwrap();
    store.write_journals(&mut s);
    holds(&s, &store, &[], "a opened and edited");

    // Pushed out of the warm ones: a is cold, its sleep file on its way.
    let others: Vec<TabId> = paths[1..=MAX_WARM].iter().map(|path| s.open(path).unwrap()).collect();
    assert!(!s.warm.contains(a));
    holds(&s, &store, &[], "a pushed out of the warm ones");

    // Shown again before its sleep file's been taken, as two of an agent's commands before the window catches up are: it's warm, and nothing's on
    // its way for it any more.
    show(&mut s, a);
    holds(&s, &store, &[], "a shown again before its sleep file was taken");
    let left: Vec<TabId> = s.take_left_warm().iter().map(|facts| facts.id).collect();
    assert!(!left.contains(&a), "no sleep file is written for the tab on screen: {left:?}");
    holds(&s, &store, &[], "what's left taken");

    // Left alone: whatever's due goes.
    let due = s.dormant().due_at(1_000, 300);
    assert!(!due.is_empty() && !due.contains(&a), "a's on screen: {due:?}");
    go_dormant(&mut s, &store, &due);
    holds(&s, &store, &[], "left alone");

    // Shown: b wakes, from its sleep file.
    show(&mut s, others[0]);
    holds(&s, &store, &[], "b shown");

    // The budget, and memory pressure: oldest first, whatever's there. a's list goes to disk.
    let oldest = s.dormant().oldest_first();
    assert!(oldest.contains(&a));
    go_dormant(&mut s, &store, &oldest);
    assert!(s.lists().is_asleep(a));
    holds(&s, &store, &[], "the budget");

    // Asked: `recto session sleep` on one tab.
    show(&mut s, others[1]);
    go_dormant(&mut s, &store, &[others[0]]);
    holds(&s, &store, &[], "asked");

    // At quit: the tab on screen's sleep file, and nothing goes dormant.
    let active = s.tabs().active().unwrap();
    if let Some(facts) = s.sleep_facts(active) {
        store.write_sleep(&facts);
    }
    holds(&s, &store, &[], "at quit");

    // Woken by an edit, which goes on from its journal.
    s.edit_pages(a, |e| e.rotate(1, 1, 1)).unwrap();
    assert!(!s.lists().is_asleep(a));
    store.write_journals(&mut s);
    holds(&s, &store, &[], "a woken by an edit");
    go_dormant(&mut s, &store, &[a]);
    assert!(s.lists().is_asleep(a));
    holds(&s, &store, &[], "a asleep again");

    // Woken by a command that reads pages, as the executor wakes every tab it's about first.
    assert!(s.wake_list(a));
    holds(&s, &store, &[], "a woken to read its pages");
    go_dormant(&mut s, &store, &[a]);
    holds(&s, &store, &[], "a asleep once more");

    // A wake that fails: its journal's gone from the disk. It stays asleep, under its name, and the tab says so; showing it doesn't change that.
    let name = s.journals().name_of(a).unwrap().to_string();
    store.journals.lock().unwrap().remove(&name);
    assert!(!s.wake_list(a));
    assert!(s.lists().is_asleep(a));
    assert!(matches!(s.tabs().tab_notice(a), Some(FileNotice::EditsUnreadable)));
    holds(&s, &store, &[a], "a's journal unreadable");
    show(&mut s, a);
    assert!(s.lists().is_asleep(a));
    holds(&s, &store, &[a], "a shown with its journal unreadable");

    // Shown again among the rest, pushing others out and back.
    for &id in others.iter().chain([&a]) {
        show(&mut s, id);
        holds(&s, &store, &[a], "shown in turn");
    }
    s.open(&paths[MAX_WARM + 1]).unwrap();
    holds(&s, &store, &[a], "one more opened");
}

/// **Failure: what was read of a file found to be another version is written into a new sleep file** (a correctness fault, not only wear). A tab woken
/// from its sleep file and pushed out of the warm ones before the worker opens its file: the worker says it's another version, so what the sleep file
/// said goes and the PDF's to be read ([`Session::file_length_is`]). Before 2026-11-13 the contents it was pushed out with were still on their way to a
/// sleep file, and went into a new one under the same stamp, so the next wake showed the old version's contents again.
#[test]
fn a_file_found_to_be_another_version_doesnt_write_the_old_ones_contents_into_a_sleep_file() {
    let store = Arc::new(Store::default());
    let (_dir, paths) = files(&["x.pdf", "1.pdf", "2.pdf", "3.pdf", "4.pdf", "5.pdf", "6.pdf"]);
    let mut s = sleeping_session(&store, "New");
    let stamp = file_stamp(&paths[0]).unwrap();
    // The sleep file an earlier version of x left: same stamp, as a rewrite that keeps size, time and number does, and other contents.
    let old = sleep::SleepFile {
        file: stamp,
        file_pages: 5,
        pages: 5,
        list_seq: 0,
        renderer_only: false,
        version: Some(7),
        toc: Titled("Old").load(&paths[0]).unwrap().toc,
        usual: None,
        sizes: Vec::new(),
        pictures: Vec::new(),
    };
    store.sleeps.lock().unwrap().push((stamp, paths[0].clone(), sleep::encode_sleep(&old)));
    let x = s.open(&paths[0]).unwrap();
    for path in &paths[1..=MAX_WARM] {
        s.open(path).unwrap();
    }
    s.take_left_warm();
    // Woken from its sleep file, and shown.
    show(&mut s, x);
    assert!(s.tabs().tab(x).unwrap().from_sleep.is_some(), "woken from the sleep file");
    // Pushed out before the worker's opened it.
    for path in &paths[MAX_WARM + 1..] {
        s.open(path).unwrap();
    }
    assert!(!s.warm.contains(x));
    // The worker: another version.
    assert!(s.file_length_is(x, 5, 8), "the file's to be read");
    holds(&s, &store, &[], "x found to be another version");
    let title = |facts: &SleepFacts| facts.sleep.toc.entries().first().map(|entry| facts.sleep.toc.title(entry).to_string());
    for facts in s.take_left_warm() {
        assert!(
            facts.id != x,
            "a sleep file's on its way for x with what was read of the old version: {:?}",
            title(&facts)
        );
    }
}

/// Stamps and the temporary folder written as names, so a run reads the same on any machine.
struct Names(Vec<(String, String)>);

impl Names {
    fn of(dir: &Path, paths: &[PathBuf]) -> Self {
        let mut names = Vec::new();
        for path in paths {
            let name = path.file_name().unwrap().to_string_lossy().into_owned();
            let stamp = file_stamp(path).unwrap();
            names.push((format!("{stamp:?}"), format!("<{name}>")));
            names.push((format!("{:?}", record::FileRecord::from(stamp)), format!("<{name}>")));
            names.push((record::journal_file_name(Some(stamp), path, None), format!("<journal of {name}>")));
        }
        names.push((dir.to_string_lossy().into_owned(), "<dir>".into()));
        Self(names)
    }

    fn say(&self, what: impl std::fmt::Debug) -> String {
        let mut said = format!("{what:?}");
        for (raw, name) in &self.0 {
            said = said.replace(raw, name);
        }
        said
    }
}

/// What a sleep file would say of a tab, with its contents by title.
fn facts_said(names: &Names, facts: &SleepFacts) -> String {
    let sleep = &facts.sleep;
    let titles: Vec<&str> = sleep.toc.entries().iter().map(|entry| sleep.toc.title(entry)).collect();
    names.say(format_args!(
        "tab {} {} file {:?} file_pages {} pages {} list_seq {} renderer_only {} version {:?} toc {titles:?} usual {:?} sizes {:?} pictures {} pictures_allowed {}",
        facts.id.0,
        facts.path.display(),
        sleep.file,
        sleep.file_pages,
        sleep.pages,
        sleep.list_seq,
        sleep.renderer_only,
        sleep.version,
        sleep.usual,
        sleep.sizes,
        sleep.pictures.len(),
        facts.pictures_allowed
    ))
}

/// Everything going dormant and waking leaves on a tab: its tier, length, whether it came from a sleep file, the read held for it, what dormancy
/// holds of it, its list and its notice.
fn state_of<L: DocLoader>(s: &Session<L>, id: TabId) -> String {
    let Some(tab) = s.tabs().tab(id) else {
        return format!("tab {}: closed", id.0);
    };
    format!(
        "tab {}: {:?} page_count {} from_sleep {:?} read_held {} dormancy {:?} asleep {} in_memory {} notice {:?}",
        id.0,
        s.tier(id),
        tab.page_count,
        tab.from_sleep,
        s.warm().holds_read_of(id),
        s.dormancy
            .holding(id)
            .iter()
            .filter(|(_, holds)| *holds)
            .map(|(part, _)| *part)
            .collect::<Vec<_>>(),
        s.lists().is_asleep(id),
        s.lists().get(id).is_some(),
        s.tabs().tab_notice(id).map(FileNotice::code)
    )
}

/// The scripted run, each step's facts and states written down.
fn sleeper_run() -> String {
    let store = Arc::new(Store::default());
    let (dir, paths) = files(&["a.pdf", "b.pdf", "c.pdf", "d.pdf", "e.pdf"]);
    let names = Names::of(&dir, &paths);
    let mut s = sleeping_session(&store, "Contents");
    let mut out = String::new();
    let tabs = |s: &Session<Titled>| s.tabs().iter().map(|tab| tab.id).collect::<Vec<_>>();
    let step = |s: &Session<Titled>, out: &mut String, what: &str| {
        writeln!(out, "== {what}").unwrap();
        for id in tabs(s) {
            writeln!(out, "{}", state_of(s, id)).unwrap();
            match s.sleep_facts(id) {
                Some(facts) => writeln!(out, "  facts {}", facts_said(&names, &facts)).unwrap(),
                None => writeln!(out, "  facts none").unwrap(),
            }
            writeln!(
                out,
                "  to read {}  awake {}",
                names.say(s.sleep_to_read(id)),
                names.say(s.sleep_to_read_awake(id))
            )
            .unwrap();
        }
        let mut asleep: Vec<(String, EditsSummary)> = s.asleep_by_journal().into_iter().collect();
        asleep.sort_by(|x, y| x.0.cmp(&y.0));
        writeln!(out, "asleep by journal {}", names.say(&asleep)).unwrap();
    };
    let take_left = |s: &mut Session<Titled>, out: &mut String| {
        for facts in s.take_left_warm() {
            writeln!(out, "left warm: {}", facts_said(&names, &facts)).unwrap();
            store.write_sleep(&facts);
        }
    };

    let a = s.open(&paths[0]).unwrap();
    s.edit_pages(a, |e| e.rotate(0, 1, 1)).unwrap();
    store.write_journals(&mut s);
    step(&s, &mut out, "a opened and edited");

    let others: Vec<TabId> = paths[1..=MAX_WARM].iter().map(|path| s.open(path).unwrap()).collect();
    take_left(&mut s, &mut out);
    step(&s, &mut out, "a pushed out of the warm ones");

    let to_sleep = s.list_to_sleep(a);
    writeln!(out, "list to sleep {}", names.say(&to_sleep)).unwrap();
    writeln!(out, "a goes dormant: {}", s.go_dormant(a)).unwrap();
    step(&s, &mut out, "a dormant");

    writeln!(out, "wake for tap {}", names.say(s.wake_for_tap(a))).unwrap();
    step(&s, &mut out, "a woken from its sleep file");
    s.tap(a).unwrap();
    let read = s.dormancy_mut().take_sleep_read(a).map(|sleep| (sleep.pages, sleep.list_seq, sleep.version));
    writeln!(out, "sleep read {read:?}, woken from files {:?}", s.dormancy_mut().take_woken_from_file()).unwrap();
    take_left(&mut s, &mut out);
    step(&s, &mut out, "a shown");
    writeln!(out, "file length is 5, version 0: {}", s.file_length_is(a, 5, 0)).unwrap();
    step(&s, &mut out, "a's file as its sleep file said");

    s.dormancy_mut().hold(others[1]);
    let due = s.dormant().due_at(1_000, 300);
    writeln!(out, "due with c held {due:?}").unwrap();
    writeln!(out, "gone {}", go_dormant(&mut s, &store, &due)).unwrap();
    s.let_go(others[1]);
    writeln!(out, "due once let go {:?}", s.dormant().due_at(1_000, 300)).unwrap();
    step(&s, &mut out, "left alone, c held then let go");

    writeln!(out, "wake for tap {}", names.say(s.wake_for_tap(others[0]))).unwrap();
    s.tap(others[0]).unwrap();
    writeln!(out, "file length is 4, version 3: {}", s.file_length_is(others[0], 4, 3)).unwrap();
    take_left(&mut s, &mut out);
    step(&s, &mut out, "b woken and found another length");

    let oldest = s.dormant().oldest_first();
    writeln!(out, "oldest first {oldest:?}, gone {}", go_dormant(&mut s, &store, &oldest)).unwrap();
    step(&s, &mut out, "the budget");

    s.forget_read(others[0]);
    let e = s.open(&paths[MAX_WARM + 1]).unwrap();
    s.give_woken(others[2], s.loader().load(&paths[3]), file_stamp(&paths[3]).ok());
    take_left(&mut s, &mut out);
    step(&s, &mut out, "b's read forgotten, e opened, d's read given");

    s.edit_pages(a, |e| e.rotate(1, 1, 1)).unwrap();
    store.write_journals(&mut s);
    step(&s, &mut out, "a woken by an edit");
    writeln!(out, "gone {}", go_dormant(&mut s, &store, &[a])).unwrap();
    let name = s.journals().name_of(a).unwrap().to_string();
    store.journals.lock().unwrap().remove(&name);
    writeln!(out, "a's list wakes with its journal gone: {}", s.wake_list(a)).unwrap();
    s.touch(e);
    step(&s, &mut out, "a's journal unreadable");
    out
}

/// What the scripted run said on `main` before the sleeper moved (2026-11-13, after the fix to what's leaving the warm ones), and must go on saying.
const SLEEPER_GOLDEN: &str = include_str!("sleeper_facts.txt");

/// **Failure: what a sleep file would say, or what a tab is left as by going dormant and waking, changes when the code that does it moves** (the
/// sleeper, R3.5): a sleep file's facts of another length, list, version or contents, one offered for a tab that may not have one (on screen, woken
/// from a sleep file and not yet checked, its list asleep), a wake that keeps another read or none, a tab left in another tier or with another
/// notice, a hold that doesn't hold. Each is a first frame wrong or a file read again for nothing. The whole log is compared, so a failure shows where.
/// `RECTO_SLEEPER_GOLDEN=write` writes it afresh, for a change of behaviour reviewed as one.
#[test]
fn going_dormant_and_waking_say_and_leave_the_same() {
    let run = sleeper_run();
    assert_eq!(run, sleeper_run(), "the run is the same twice, or a golden of it holds nothing");
    if std::env::var("RECTO_SLEEPER_GOLDEN").as_deref() == Ok("write") {
        std::fs::write(Path::new(env!("CARGO_MANIFEST_DIR")).join("src/tests/sleeper_facts.txt"), &run).unwrap();
        return;
    }
    if run != SLEEPER_GOLDEN {
        let first = run.lines().zip(SLEEPER_GOLDEN.lines()).position(|(now, then)| now != then);
        panic!("what going dormant and waking say changed, first at line {first:?}.\nNow:\n{run}\nThen:\n{SLEEPER_GOLDEN}");
    }
}
