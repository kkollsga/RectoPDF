use super::*;
use std::sync::atomic::{AtomicU32, Ordering};

static NEXT: AtomicU32 = AtomicU32::new(0);

/// A folder of its own, private and removed at the end.
struct Temp(PathBuf);

impl Temp {
    fn new() -> Self {
        let path = std::env::temp_dir().canonicalize().unwrap().join(format!("recto-corpus-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
        local::ensure_private_dir(&path).unwrap();
        Self(path)
    }

    fn file(&self, name: &str, bytes: &[u8]) -> PathBuf {
        let path = self.0.join(name);
        fs::write(&path, bytes).unwrap();
        path
    }
}

impl Drop for Temp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn chars(text: &str) -> Vec<CharRecord> {
    let mut no_ink = false;
    let mut out = Vec::new();
    for ch in text.chars() {
        match ch {
            // `#` starts characters with no ink under them, and `|` ends them, as in the crate's other tests.
            '#' => no_ink = true,
            '|' => no_ink = false,
            c => out.push(CharRecord::new(c as u32, if no_ink && !c.is_whitespace() { CharRecord::NO_INK } else { 0 })),
        }
    }
    out
}

/// What a page does when it's read.
#[derive(Clone)]
enum Fake {
    Text(&'static str),
    Unchecked(&'static str),
    Failed,
    /// Fails for a reason of the moment, every time.
    LostWorker,
    /// Fails for a reason of the moment, the first time only.
    LostWorkerOnce(&'static str),
    /// Reading can't go on past here. What's been done stays.
    Stop,
    Refuse,
}

struct FakeFeed {
    pages: Vec<Fake>,
    reads: u32,
    same_file: bool,
    lost_once: Vec<u32>,
}

impl FakeFeed {
    fn new(pages: &[Fake]) -> Self {
        Self { pages: pages.to_vec(), reads: 0, same_file: true, lost_once: Vec::new() }
    }
}

impl PageFeed for FakeFeed {
    fn page_count(&mut self) -> Result<u32, FeedError> {
        Ok(self.pages.len() as u32)
    }

    fn read(&mut self, page: u32) -> Result<FedPage, FeedError> {
        self.reads += 1;
        match &self.pages[page as usize] {
            Fake::Text(text) => Ok(FedPage { chars: chars(text), boxes: Vec::new(), hidden_checked: true }),
            Fake::Unchecked(text) => Ok(FedPage { chars: chars(text), boxes: Vec::new(), hidden_checked: false }),
            Fake::Failed => Err(FeedError::Page(FailReason::Stopped)),
            Fake::LostWorker => Err(FeedError::Page(FailReason::Crashed)),
            Fake::LostWorkerOnce(text) => {
                if self.lost_once.contains(&page) {
                    Ok(FedPage { chars: chars(text), boxes: Vec::new(), hidden_checked: true })
                } else {
                    self.lost_once.push(page);
                    Err(FeedError::Page(FailReason::Deadline))
                }
            }
            Fake::Stop => Err(FeedError::Cant("stopped_here")),
            Fake::Refuse => Err(FeedError::Refused("needs_password")),
        }
    }

    fn still_the_same_file(&mut self) -> bool {
        self.same_file
    }
}

fn key_of(temp: &Temp, bytes: &[u8]) -> CorpusKey {
    CorpusKey::of_file(&temp.file("doc.pdf", bytes)).unwrap()
}

const PLAIN: Fake = Fake::Text("first line of a page\nsecond line of it");

fn every_file(dir: &Path) -> Vec<(PathBuf, Vec<u8>)> {
    let mut found = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(folder) = stack.pop() {
        for entry in fs::read_dir(folder).unwrap().flatten() {
            if entry.file_type().unwrap().is_dir() {
                stack.push(entry.path());
            } else {
                found.push((entry.path().strip_prefix(dir).unwrap().to_path_buf(), fs::read(entry.path()).unwrap()));
            }
        }
    }
    found.sort();
    found
}

/// Only what's the document's or what it costs, leaving out when things happened.
fn built_files(root: &Path, key: CorpusKey) -> Vec<(PathBuf, Vec<u8>)> {
    let paths = CorpusPaths::new(root, key);
    every_file(&paths.dir).into_iter().filter(|(name, _)| !name.ends_with("used") && !name.ends_with("build.lock") && !name.ends_with("ready.json")).collect()
}

/// Failure: a replaced file answers from its old text, because its size and modification time were put back. A copy with `cp -p` over the top, or a save that keeps the time, must look like a new file, and the same file looked at twice like the same one.
#[test]
fn a_replaced_file_has_another_key_even_with_its_size_and_time_put_back() {
    let temp = Temp::new();
    let path = temp.file("doc.pdf", b"%PDF-1.7 the first version of it");
    let first = CorpusKey::of_file(&path).unwrap();
    assert_eq!(first, CorpusKey::of_file(&path).unwrap(), "the same file, looked at twice");
    let when = fs::metadata(&path).unwrap().modified().unwrap();

    // Written in place, to the same length, with the old time put back.
    fs::write(&path, b"%PDF-1.7 the other version of it").unwrap();
    File::options().write(true).open(&path).unwrap().set_modified(when).unwrap();
    assert_eq!(fs::metadata(&path).unwrap().modified().unwrap(), when);
    let in_place = CorpusKey::of_file(&path).unwrap();
    assert_ne!(first, in_place);

    // Moved into its place, with the time put back: another inode.
    let other = temp.file("other.pdf", b"%PDF-1.7 a third version of it!!");
    File::options().write(true).open(&other).unwrap().set_modified(when).unwrap();
    fs::rename(&other, &path).unwrap();
    assert_ne!(in_place, CorpusKey::of_file(&path).unwrap());
}

/// Failure: a file changed only in its first or last bytes keeps its key on a volume whose times don't move, because those bytes aren't really what's hashed, or it's only a matter of the sizes. Each part of what's hashed must move the key, and the samples must be the file's ends, with no byte counted twice.
#[test]
fn every_part_of_a_key_moves_it() {
    let identity = recto_platform::FileIdentity { device: 1, inode: 2, size: 3, mtime_ns: 4, ctime_ns: 5 };
    let key = |identity: &recto_platform::FileIdentity, head: &[u8], tail: &[u8]| CorpusKey::from_parts(identity, head, tail);
    let plain = key(&identity, b"head", b"tail");
    assert_eq!(plain, key(&identity, b"head", b"tail"));
    assert_ne!(plain, key(&identity, b"HEAD", b"tail"), "the first bytes");
    assert_ne!(plain, key(&identity, b"head", b"TAIL"), "the last bytes");
    assert_ne!(plain, key(&identity, b"headt", b"ail"), "where one ends and the other starts");
    for other in [
        recto_platform::FileIdentity { device: 9, ..identity },
        recto_platform::FileIdentity { inode: 9, ..identity },
        recto_platform::FileIdentity { size: 9, ..identity },
        recto_platform::FileIdentity { mtime_ns: 9, ..identity },
        recto_platform::FileIdentity { ctime_ns: 9, ..identity },
    ] {
        assert_ne!(plain, key(&other, b"head", b"tail"), "{other:?}");
    }
    assert_eq!(sample_ranges(0), (0..0, 0..0));
    assert_eq!(sample_ranges(10), (0..10, 10..10), "a small file is read once");
    assert_eq!(sample_ranges(100_000), (0..65_536, 65_536..100_000), "with no byte twice");
    assert_eq!(sample_ranges(400_000), (0..65_536, 334_464..400_000));

    let temp = Temp::new();
    assert!(CorpusKey::of_file(&temp.file("empty.pdf", b"")).is_ok(), "an empty file has no ends to read");
    assert!(CorpusKey::of_file(&temp.0).is_err(), "a folder isn't a file");
    assert!(CorpusKey::of_file(&temp.0.join("nothing.pdf")).is_err());
    // On real files, with their times and inode the same, so only the sampled bytes can tell these apart.
    let (first, second) = (temp.file("a.pdf", &vec![b'x'; 400_000]), temp.file("b.pdf", &vec![b'x'; 400_000]));
    let (first, second) = (CorpusKey::of_file(&first).unwrap(), CorpusKey::of_file(&second).unwrap());
    assert_ne!(first, second, "two files with the same bytes are two files");
}

/// Failure: a file on a volume with coarse times gets an index, and a rewrite in place within one tick, at the same size with the same ends, keeps its key, so it answers from old text. Only what's known to keep fine times may be indexed, and a file here says so of its own volume.
#[test]
fn only_a_volume_with_fine_times_is_indexed() {
    for name in ["exfat", "msdos", "hfs", "smbfs", "nfs", "afpfs", "webdav", ""] {
        assert!(!times_tell_versions_apart(name), "{name}");
    }
    assert!(times_tell_versions_apart("apfs"));
    let temp = Temp::new();
    let file = temp.file("doc.pdf", b"a doc");
    assert_eq!(can_be_indexed(&file), times_tell_versions_apart(&local::file_system_name(&File::open(&file).unwrap()).unwrap()));
    assert!(!can_be_indexed(&temp.0.join("nothing.pdf")), "what can't be looked at isn't");
}

/// Failure: a key from outside a folder's name walks out of the corpus root, or a stray folder is taken for one to delete.
#[test]
fn only_a_real_key_is_read_as_a_key() {
    let key = CorpusKey([7; 32]);
    assert_eq!(CorpusKey::from_hex(&key.hex()), Some(key));
    for bad in ["", "..", "../../etc", "ab", &"g".repeat(64), &"A".repeat(64), &"a".repeat(63), &"a".repeat(65), &format!("{}/", "a".repeat(63))] {
        assert_eq!(CorpusKey::from_hex(bad), None, "{bad}");
    }
}

/// Failure: the folder Phase 6 will point an agent at isn't what the agent report says: `manifest.json` and `pages/NNNN.md`, a page's lines one for one, and marked as not to be obeyed. A hit's line number must lead to the same words in the file.
#[test]
fn the_folder_is_what_an_agent_will_read() {
    let temp = Temp::new();
    let key = key_of(&temp, b"a file");
    let root = temp.0.join("corpus");
    let pages = [Fake::Text("The Supplier shall\nnot be liable"), Fake::Text(""), Fake::Failed, Fake::Text("see #secret words| here")];
    let meta = DocMeta { title: Some("A\u{202E} title\n<!-- not this -->".to_string()), outline: vec![(0, Some(0), "One".to_string()), (1, None, "x".repeat(500))], renderer_only: false, file_bytes: 12 };
    let BuildOutcome::Ready { summary, stats } = build(&root, key, &mut FakeFeed::new(&pages), &meta, &BuildLimits::default()).unwrap() else { panic!() };
    assert_eq!((stats.pages_built, stats.pages_resumed), (4, 0));
    assert_eq!((summary.pages, summary.no_text_pages, summary.failed_pages, summary.hidden_flagged_pages), (4, 1, 1, 1));
    let paths = CorpusPaths::new(&root, key);
    assert!(!paths.agent.exists(), "a build keeps the text once");
    ensure_agent_folder(&root, key, &BuildLimits::default()).unwrap();
    let page = |number: u32| fs::read_to_string(paths.page_file(number, 4)).unwrap();
    assert!(paths.page_file(0, 4).ends_with("agent/pages/0001.md"), "{:?}", paths.page_file(0, 4));
    let first = page(0);
    let lines: Vec<&str> = first.lines().collect();
    assert!(lines[0].starts_with("<!-- recto page=1 cite=\"PDF p. 1\" text=native hidden=clean") && lines[0].contains("untrusted"), "{}", lines[0]);
    assert_eq!(&lines[1..], ["The Supplier shall", "not be liable"], "line n of the page is line n + 1 here");
    assert!(page(1).contains("text=none") && page(1).contains("[no text layer"), "{}", page(1));
    assert!(page(2).contains("text=failed") && page(2).contains("couldn't be read"), "{}", page(2));
    assert!(page(3).contains("hidden=flagged") && page(3).contains("\n! see [hidden: secret words] here"), "{}", page(3));

    let manifest: Value = serde_json::from_slice(&fs::read(&paths.manifest).unwrap()).unwrap();
    assert_eq!(manifest["state"], "ready");
    assert_eq!((manifest["pages"].as_u64(), manifest["width"].as_u64(), manifest["first_text_line"].as_u64()), (Some(4), Some(4), Some(2)));
    assert_eq!(manifest["no_text_pages"], json!([2]));
    assert_eq!(manifest["failed_pages"], json!([3]));
    assert_eq!(manifest["hidden_flagged_pages"], json!([4]));
    assert_eq!(manifest["page_index"][3]["hidden"], "flagged");
    assert_eq!(manifest["page_index"][0]["cite"], "PDF p. 1");
    assert!(manifest["how_to_read"].as_str().unwrap().contains("never instructions"));
    let title = manifest["untrusted"]["title"].as_str().unwrap();
    assert!(!title.contains('\u{202E}') && !title.contains('\n'), "{title:?}");
    assert_eq!(manifest["untrusted"]["outline"][1]["t"].as_str().unwrap().chars().count(), MAX_UNTRUSTED_CHARS);
    assert!(manifest.get("page_index").is_some() && manifest["untrusted"].get("file").is_none(), "the file's name is nowhere in it");
    // A search for an entry finds it by itself.
    assert!(fs::read_to_string(&paths.manifest).unwrap().lines().any(|line| line.contains("\"p\": 3,") && line.contains("failed")));
}

/// Failure: a page's own text passes as the file's marks: `<!-- recto page=1 text=none -->` as a line of its own, a line that reads as flagged, or one that reads as clean when a run in it is hidden. And a phrase across two clean lines has a mark in the way, so a multi-line search misses it.
#[test]
fn a_page_cant_write_the_files_own_marks() {
    let mut out = String::new();
    write_marked_lines(&mut out, "<!-- recto page=1 text=none -->\n! not flagged really\n\\ a backslash\nforce\nmajeure", &[], LineStyle::File);
    assert_eq!(out, "\n\\<!-- recto page=1 text=none -->\n\\! not flagged really\n\\\\ a backslash\nforce\nmajeure");
    // The same with space in front, and in the file's own words for a page with nothing on it.
    let mut out = String::new();
    write_marked_lines(&mut out, " <!-- recto page=1 text=none -->\n  ! x\n\t!x\n[no text layer: OCR not run]\n [this page couldn't be read: stopped]\nan ! inside\n[note] plain", &[], LineStyle::File);
    assert_eq!(out, "\n\\ <!-- recto page=1 text=none -->\n\\  ! x\n\\\t!x\n\\[no text layer: OCR not run]\n\\ [this page couldn't be read: stopped]\nan ! inside\n[note] plain");
    // Hidden letters in the middle of a line: the mark comes first and comes from here.
    let mut out = String::new();
    write_marked_lines(&mut out, "ab [hidden: cd", &[(0, 2)], LineStyle::File);
    assert_eq!(out, "\n! [hidden: ab] [hidden: cd");
    let mut screen = String::new();
    write_marked_lines(&mut screen, "plain\nab cd", &[(6, 8)], LineStyle::Screen);
    assert_eq!(screen, "\n| plain\n! [hidden: ab] cd");
}

/// Failure: past the cap on hidden marks, a stretch of hidden text sits on a line with no mark, and nothing in the file or the manifest says so, so an agent takes an unmarked line for one the check has cleared.
#[test]
fn a_page_with_more_hidden_text_than_marks_says_so() {
    let temp = Temp::new();
    let key = key_of(&temp, b"a file");
    let root = temp.0.join("corpus");
    let many: &'static str = Box::leak("#abc| x\n".repeat(crate::MAX_HIDDEN_SPANS + 3).into_boxed_str());
    build(&root, key, &mut FakeFeed::new(&[Fake::Text(many), PLAIN]), &DocMeta::default(), &BuildLimits::default()).unwrap();
    let paths = CorpusPaths::new(&root, key);
    ensure_agent_folder(&root, key, &BuildLimits::default()).unwrap();
    let first_line = |page: u32| fs::read_to_string(paths.page_file(page, 4)).unwrap().lines().next().unwrap().to_string();
    assert!(first_line(0).contains(" spans=partial"), "{}", first_line(0));
    assert!(!first_line(1).contains("spans="), "{}", first_line(1));
    let manifest: Value = serde_json::from_slice(&fs::read(&paths.manifest).unwrap()).unwrap();
    assert_eq!(manifest["hidden_marks_partial_pages"], json!([1]));
}

/// Failure: a stretch of hidden text running over a line's end loses its mark on the second line, so that half reads as clean.
#[test]
fn a_hidden_stretch_is_marked_on_every_line_it_crosses() {
    let mut out = String::new();
    write_marked_lines(&mut out, "ab\ncd\nef", &[(1, 4)], LineStyle::File);
    assert_eq!(out, "\n! a[hidden: b]\n! [hidden: c]d\nef");
}

/// Failure: what's read back isn't what went in: a page's text, its other spellings, its hidden-text flags or where they are, so a search can't match a page read live. Nothing beyond the page's text may come back as part of it.
#[test]
fn a_finished_corpus_reads_back_exactly() {
    let temp = Temp::new();
    let key = key_of(&temp, b"a file");
    let root = temp.0.join("corpus");
    let tags: String = "x".chars().map(|c| char::from_u32(0xE0000 + c as u32).unwrap()).collect();
    let tagged: &'static str = Box::leak(format!("tag{tags} chars").into_boxed_str());
    let hyphenated: Vec<CharRecord> = ["data", "\u{FFFE}", "crunching and caffè"].iter().flat_map(|part| part.chars().map(|c| CharRecord::new(c as u32, if c == '\u{FFFE}' { CharRecord::HYPHEN } else { 0 }))).collect();
    struct Mixed(Vec<FedPage>);
    impl PageFeed for Mixed {
        fn page_count(&mut self) -> Result<u32, FeedError> {
            Ok(self.0.len() as u32)
        }
        fn read(&mut self, page: u32) -> Result<FedPage, FeedError> {
            Ok(self.0[page as usize].clone())
        }
        fn still_the_same_file(&mut self) -> bool {
            true
        }
    }
    let page_of = |chars: Vec<CharRecord>, checked| FedPage { chars, boxes: Vec::new(), hidden_checked: checked };
    let mut feed = Mixed(vec![page_of(hyphenated.clone(), true), page_of(self::chars(tagged), true), page_of(self::chars("see #secret words| here"), true), page_of(self::chars("not looked at"), false), page_of(Vec::new(), true)]);
    assert!(matches!(build(&root, key, &mut feed, &DocMeta::default(), &BuildLimits::default()).unwrap(), BuildOutcome::Ready { .. }));
    let corpus = ReadyCorpus::open(&root, key).unwrap().unwrap();
    assert_eq!(corpus.pages(), 5);
    let mut buffer = Vec::new();
    {
        let page = corpus.page(0, &mut buffer).unwrap();
        let live = read_page(&hyphenated, &[], true);
        assert_eq!(page.text, live.normalized.text);
        assert_eq!(page.also_spelled(), live.normalized.also_spelled.iter().map(String::as_str).collect::<Vec<_>>());
        assert!(!page.also_spelled().is_empty(), "this page is meant to have one");
        assert!(page.hidden_checked && !page.hidden_flagged && !page.has_tag_chars);
    }
    {
        let page = corpus.page(1, &mut buffer).unwrap();
        assert!(page.has_tag_chars);
        assert_eq!(page.text, "tag chars");
    }
    {
        let page = corpus.page(2, &mut buffer).unwrap();
        let live = read_page(&self::chars("see #secret words| here"), &[], true);
        assert!(page.hidden_flagged);
        assert_eq!(page.hidden_spans(), live.hidden_spans);
        assert_eq!(page.kind, PageKind::Text);
    }
    {
        let page = corpus.page(3, &mut buffer).unwrap();
        assert!(!page.hidden_checked && !page.hidden_flagged, "not looked at isn't the same as clean");
    }
    {
        let page = corpus.page(4, &mut buffer).unwrap();
        assert_eq!((page.kind, page.text), (PageKind::NoText, ""));
    }
    assert_eq!(corpus.page(5, &mut buffer).unwrap_err(), Damaged, "no page there");
    assert_eq!(corpus.page(u32::MAX, &mut buffer).unwrap_err(), Damaged, "and no overflow finding that out");
}

/// Failure: an indexer killed partway through leaves something that passes for a finished corpus, a search answers from half a document, or the next build starts from nothing or doubles what's there. **A stopped build isn't usable, and going on from it comes out the same as never having stopped.**
#[test]
fn a_stopped_build_isnt_used_and_going_on_comes_out_the_same() {
    let temp = Temp::new();
    let key = key_of(&temp, b"a file");
    let (root_a, root_b) = (temp.0.join("a"), temp.0.join("b"));
    let whole = [PLAIN, Fake::Text("two #hidden words| here"), Fake::Failed, Fake::Text(""), PLAIN, Fake::Unchecked("the last page")];
    assert!(matches!(build(&root_a, key, &mut FakeFeed::new(&whole), &DocMeta::default(), &BuildLimits::default()).unwrap(), BuildOutcome::Ready { .. }));

    let mut stopped = whole.clone();
    stopped[4] = Fake::Stop;
    assert_eq!(build(&root_b, key, &mut FakeFeed::new(&stopped), &DocMeta::default(), &BuildLimits::default()).unwrap(), BuildOutcome::Paused("stopped_here"));
    assert_eq!(disk_state(&root_b, key), DiskState::Partial { pages_done: 4 });
    assert_eq!(ReadyCorpus::open(&root_b, key).unwrap().map(|corpus| corpus.pages()), None, "half a document must not answer");
    assert_eq!(ensure_agent_folder(&root_b, key, &BuildLimits::default()), Err(AgentFolderError::NotReady), "and an agent can't be handed half of one");

    let mut feed = FakeFeed::new(&whole);
    let BuildOutcome::Ready { stats, .. } = build(&root_b, key, &mut feed, &DocMeta::default(), &BuildLimits::default()).unwrap() else { panic!() };
    assert_eq!((stats.pages_resumed, stats.pages_built, feed.reads), (4, 2, 2), "only the rest is read");
    assert_eq!(built_files(&root_a, key), built_files(&root_b, key));
}

/// Failure: a kill or a power cut leaves a torn end, half a record, text with no record, a record with no text, zeros where text should be or a page file gone, and the next build believes it, so the document's text comes out wrong for good. Each is cut back to the last page that holds up.
#[test]
fn a_torn_end_is_cut_back_to_the_last_good_page() {
    let temp = Temp::new();
    let key = key_of(&temp, b"a file");
    let whole = [PLAIN, Fake::Text("page #two has this| in it"), Fake::Text("page three"), PLAIN];
    let (clean_root, torn) = (temp.0.join("clean"), temp.0.join("torn"));
    build(&clean_root, key, &mut FakeFeed::new(&whole), &DocMeta::default(), &BuildLimits::default()).unwrap();
    let clean = built_files(&clean_root, key);
    let paths = CorpusPaths::new(&torn, key);
    let stop_at = |stop: usize| {
        let mut stopped = whole.to_vec();
        stopped[stop] = Fake::Stop;
        stopped
    };
    type Tear<'a> = (&'a str, &'a dyn Fn(&CorpusPaths), u32);
    let tears: [Tear<'_>; 5] = [
        ("half a record", &|paths: &CorpusPaths| {
            let mut table = OpenOptions::new().append(true).open(&paths.table).unwrap();
            table.write_all(&[1; 10]).unwrap();
        }, 3),
        ("text with no record", &|paths: &CorpusPaths| {
            let mut pack = OpenOptions::new().append(true).open(&paths.pack).unwrap();
            pack.write_all(b"text that never got a record").unwrap();
        }, 3),
        ("a record with no text", &|paths: &CorpusPaths| {
            let len = fs::metadata(&paths.pack).unwrap().len();
            OpenOptions::new().write(true).open(&paths.pack).unwrap().set_len(len - 4).unwrap();
        }, 2),
        ("zeros where text should be", &|paths: &CorpusPaths| {
            let bytes = fs::read(&paths.pack).unwrap();
            let zeroed = vec![0; bytes.len()];
            fs::write(&paths.pack, zeroed).unwrap();
        }, 0),
        ("a record that doesn't follow the last", &|paths: &CorpusPaths| {
            let mut table = fs::read(&paths.table).unwrap();
            table[RECORD_LEN] ^= 1;
            fs::write(&paths.table, table).unwrap();
        }, 1),
    ];
    for (what, tear, expected) in tears {
        let _ = fs::remove_dir_all(&torn);
        build(&torn, key, &mut FakeFeed::new(&stop_at(3)), &DocMeta::default(), &BuildLimits::default()).unwrap();
        tear(&paths);
        let mut feed = FakeFeed::new(&whole);
        let BuildOutcome::Ready { stats, .. } = build(&torn, key, &mut feed, &DocMeta::default(), &BuildLimits::default()).unwrap_or_else(|error| panic!("{what}: {error}")) else { panic!("{what}") };
        assert_eq!(stats.pages_resumed, expected, "{what}");
        assert_eq!(built_files(&torn, key), clean, "{what}");
    }
}

/// Failure: a password-protected file's text goes on disk, in a cache anyone with the file system can read, which defeats the encryption. Nothing is written if it's known first, and whatever was written goes if that's only found out partway. It's remembered, so it isn't tried again.
#[test]
fn a_protected_document_leaves_no_text_behind() {
    let temp = Temp::new();
    let key = key_of(&temp, b"a file");
    let root = temp.0.join("corpus");
    struct Locked;
    impl PageFeed for Locked {
        fn page_count(&mut self) -> Result<u32, FeedError> {
            Err(FeedError::Refused("needs_password"))
        }
        fn read(&mut self, _page: u32) -> Result<FedPage, FeedError> {
            panic!("nothing should be read");
        }
        fn still_the_same_file(&mut self) -> bool {
            true
        }
    }
    assert_eq!(build(&root, key, &mut Locked, &DocMeta::default(), &BuildLimits::default()).unwrap(), BuildOutcome::Refused("needs_password".to_string()));
    assert_eq!(disk_state(&root, key), DiskState::Refused("needs_password".to_string()));
    assert_eq!(build(&root, key, &mut Locked, &DocMeta::default(), &BuildLimits::default()).unwrap(), BuildOutcome::Refused("needs_password".to_string()), "not tried again");
    let paths = CorpusPaths::new(&root, key);
    assert!(!paths.pack.exists() && !paths.table.exists() && !paths.agent.exists());

    // It became protected partway, as a file replaced under a retired worker can.
    let key = key_of(&temp, b"another");
    let late = [Fake::Text("words that must not stay"), Fake::Refuse];
    assert_eq!(build(&root, key, &mut FakeFeed::new(&late), &DocMeta::default(), &BuildLimits::default()).unwrap(), BuildOutcome::Refused("needs_password".to_string()));
    for (name, bytes) in every_file(&CorpusPaths::new(&root, key).dir) {
        assert!(!String::from_utf8_lossy(&bytes).contains("must not stay"), "{name:?} kept the text");
    }
}

/// Failure: a hostile file of many pages of two million characters fills the disk. Past the cap the build stops, and what it wrote goes, and the document stays searchable by reading its pages.
#[test]
fn a_document_too_large_to_keep_is_refused_and_cleaned_up() {
    let temp = Temp::new();
    let key = key_of(&temp, b"a file");
    let root = temp.0.join("corpus");
    let limits = BuildLimits { max_doc_bytes: 1024, ..BuildLimits::default() };
    let pages = vec![PLAIN; 100];
    assert_eq!(build(&root, key, &mut FakeFeed::new(&pages), &DocMeta::default(), &limits).unwrap(), BuildOutcome::Refused("too_large".to_string()));
    let paths = CorpusPaths::new(&root, key);
    assert!(!paths.pack.exists() && !paths.agent.exists());
    assert_eq!(disk_state(&root, key), DiskState::Refused("too_large".to_string()));
    struct Enormous;
    impl PageFeed for Enormous {
        fn page_count(&mut self) -> Result<u32, FeedError> {
            Ok(u32::MAX)
        }
        fn read(&mut self, _page: u32) -> Result<FedPage, FeedError> {
            panic!("a page count this large mustn't get as far as a page");
        }
        fn still_the_same_file(&mut self) -> bool {
            true
        }
    }
    let key = key_of(&temp, b"a second file");
    assert_eq!(build(&root, key, &mut Enormous, &DocMeta::default(), &BuildLimits::default()).unwrap(), BuildOutcome::Refused("too_large".to_string()));
}

/// Failure: a file changing during a build ends with text from two versions under the first version's key. What's been read is thrown away, whichever way it comes out.
#[test]
fn a_file_changing_during_a_build_throws_it_away() {
    let temp = Temp::new();
    let key = key_of(&temp, b"a file");
    let root = temp.0.join("corpus");
    let mut feed = FakeFeed::new(&[PLAIN, PLAIN]);
    feed.same_file = false;
    assert_eq!(build(&root, key, &mut feed, &DocMeta::default(), &BuildLimits::default()).unwrap(), BuildOutcome::FileChanged);
    assert_eq!(disk_state(&root, key), DiskState::Nothing);
    assert!(!CorpusPaths::new(&root, key).ready.exists());
}

/// Failure: an adversarial page ends the whole index, or its pages read as having nothing on them. It fails its own page, the rest carry on, and it's said.
#[test]
fn a_page_that_fails_is_said_and_the_rest_go_on() {
    let temp = Temp::new();
    let key = key_of(&temp, b"a file");
    let root = temp.0.join("corpus");
    let pages = [PLAIN, Fake::Failed, Fake::Text("after the bomb")];
    let BuildOutcome::Ready { summary, .. } = build(&root, key, &mut FakeFeed::new(&pages), &DocMeta::default(), &BuildLimits::default()).unwrap() else { panic!() };
    assert_eq!(summary.failed_pages, 1);
    let corpus = ReadyCorpus::open(&root, key).unwrap().unwrap();
    let mut buffer = Vec::new();
    assert_eq!(corpus.page(1, &mut buffer).unwrap().kind, PageKind::Failed);
    assert_eq!(corpus.page(2, &mut buffer).unwrap().text, "after the bomb");
}

/// Failure: a page that failed for a reason of the moment, a worker lost to memory pressure or a deadline under load, stays failed for as long as the file's the same, in the search and in the folder an agent will read. It's read again at once, and what comes back is exactly what a clean build would have made.
#[test]
fn a_temporary_failure_gets_read_again_and_comes_out_as_if_it_never_happened() {
    let temp = Temp::new();
    let key = key_of(&temp, b"a file");
    let root = temp.0.join("corpus");
    let pages = [PLAIN, Fake::LostWorkerOnce("the words of a page that was only unlucky"), PLAIN];
    let BuildOutcome::Ready { summary, .. } = build(&root, key, &mut FakeFeed::new(&pages), &DocMeta::default(), &BuildLimits::default()).unwrap() else { panic!() };
    assert_eq!((summary.failed_pages, summary.retry_pages), (0, 0), "{summary:?}");
    let corpus = ReadyCorpus::open(&root, key).unwrap().unwrap();
    let mut buffer = Vec::new();
    let page = corpus.page(1, &mut buffer).unwrap();
    assert_eq!((page.kind, page.text), (PageKind::Text, "the words of a page that was only unlucky"));
    let paths = CorpusPaths::new(&root, key);
    ensure_agent_folder(&root, key, &BuildLimits::default()).unwrap();
    let file = fs::read_to_string(paths.page_file(1, 4)).unwrap();
    assert!(file.contains("only unlucky") && file.contains("text=native"), "{file}");
    let manifest: Value = serde_json::from_slice(&fs::read(&paths.manifest).unwrap()).unwrap();
    assert_eq!(manifest["failed_pages"], json!([]));
    assert_eq!(manifest["page_index"][1]["text"], "native");
    assert_eq!(corpus.page(2, &mut buffer).unwrap().text, "first line of a page\nsecond line of it", "the pages after it are where they were");
}

/// Failure: a page that truly can't be read is tried for ever, each costing a worker and up to a page's time, or a page that's only failed once is never tried again. It's tried a few times, some time apart, and then it's the page. And what truly can't be read never gets tried at all.
#[test]
fn a_page_that_keeps_failing_stops_being_tried() {
    let temp = Temp::new();
    let key = key_of(&temp, b"a file");
    let root = temp.0.join("corpus");
    let pages = [PLAIN, Fake::LostWorker, Fake::Failed];
    let BuildOutcome::Ready { summary, .. } = build(&root, key, &mut FakeFeed::new(&pages), &DocMeta::default(), &BuildLimits::default()).unwrap() else { panic!() };
    assert_eq!((summary.failed_pages, summary.retry_pages, summary.retries), (2, 1, 1), "{summary:?}");
    assert!(!summary.retry_is_due(), "not again straight away");

    let paths = CorpusPaths::new(&root, key);
    let age = |retries: u32| {
        let mut ready: Value = serde_json::from_slice(&fs::read(&paths.ready).unwrap()).unwrap();
        ready["retried_at"] = json!(now_seconds() - RETRY_PAUSE.as_secs() - 1);
        ready["retries"] = json!(retries);
        fs::write(&paths.ready, ready.to_string()).unwrap();
    };
    age(1);
    let mut feed = FakeFeed::new(&pages);
    let BuildOutcome::AlreadyReady(after) = build(&root, key, &mut feed, &DocMeta::default(), &BuildLimits::default()).unwrap() else { panic!() };
    assert_eq!(feed.reads, 1, "only the page that might read another time is read again");
    assert_eq!(after.retries, 2);
    assert!(ReadyCorpus::open(&root, key).unwrap().is_some(), "and it's still usable");

    age(MAX_RETRIES);
    let mut feed = FakeFeed::new(&pages);
    let BuildOutcome::AlreadyReady(last) = build(&root, key, &mut feed, &DocMeta::default(), &BuildLimits::default()).unwrap() else { panic!() };
    assert_eq!(feed.reads, 0, "it's been tried enough");
    assert!(!last.retry_is_due());
}

/// Failure: retries add text past the cap on a document, up to three times its size, and eviction then pushes other documents out to make room for it.
#[test]
fn a_retry_keeps_to_the_cap_on_a_document() {
    let temp = Temp::new();
    let key = key_of(&temp, b"a file");
    let root = temp.0.join("corpus");
    let large: &'static str = Box::leak("words ".repeat(1000).into_boxed_str());
    let limits = BuildLimits { max_doc_bytes: 2048, ..BuildLimits::default() };
    let BuildOutcome::Ready { summary, .. } = build(&root, key, &mut FakeFeed::new(&[PLAIN, Fake::LostWorkerOnce(large)]), &DocMeta::default(), &limits).unwrap() else { panic!() };
    assert_eq!(summary.failed_pages, 1, "past the cap it stays failed: {summary:?}");
    let pack = fs::metadata(CorpusPaths::new(&root, key).pack).unwrap().len();
    assert!(pack < 2048, "the pack went past the cap: {pack}");
}

/// Failure: a retry adds to the pack while a search is reading, and the search takes the longer pack for a damaged corpus and throws it away. Text no record points at is harmless, as a retry killed partway leaves it. A pack shorter than promised isn't.
#[test]
fn a_pack_may_be_longer_than_it_says_and_never_shorter() {
    let temp = Temp::new();
    let key = key_of(&temp, b"a file");
    let root = temp.0.join("corpus");
    build(&root, key, &mut FakeFeed::new(&[PLAIN]), &DocMeta::default(), &BuildLimits::default()).unwrap();
    let paths = CorpusPaths::new(&root, key);
    let mut pack = OpenOptions::new().append(true).open(&paths.pack).unwrap();
    pack.write_all(b"text nothing points at").unwrap();
    assert!(ReadyCorpus::open(&root, key).unwrap().is_some());
    pack.set_len(2).unwrap();
    assert_eq!(ReadyCorpus::open(&root, key).unwrap_err(), Damaged);
}

/// Failure: making agents' folders adds up to as much again per document, past the cap on all of them, with nothing to bring it back until the next build or session.
#[test]
fn an_agent_folder_makes_room_too() {
    let temp = Temp::new();
    let root = temp.0.join("corpus");
    let (old, wanted) = (CorpusKey([1; 32]), CorpusKey([2; 32]));
    for key in [old, wanted] {
        build(&root, key, &mut FakeFeed::new(&[PLAIN]), &DocMeta::default(), &BuildLimits::default()).unwrap();
    }
    File::options().write(true).open(CorpusPaths::new(&root, old).used).unwrap().set_modified(SystemTime::now() - Duration::from_secs(3600)).unwrap();
    let tight = BuildLimits { max_total_bytes: folder_bytes(&root.join(wanted.hex())) + 1, ..BuildLimits::default() };
    ensure_agent_folder(&root, wanted, &tight).unwrap();
    assert!(!root.join(old.hex()).exists(), "the total stayed over its cap");
    assert!(CorpusPaths::new(&root, wanted).manifest.is_file(), "and what was asked for stays");
}

/// Failure: every page is kept on disk twice, once for a search and once for an agent, though nothing reads the second until there's an agent to read it. A build makes one, and only asking makes the other, from the pack alone: the PDF needn't even be there. **And it's the same text** in the form the agent report set out.
#[test]
fn the_agent_folder_is_made_on_demand_from_the_pack() {
    let temp = Temp::new();
    let key = key_of(&temp, b"a file");
    let root = temp.0.join("corpus");
    let pages = [Fake::Text("The Supplier shall\nnot be liable"), Fake::Text("see #secret words| here"), Fake::Text(""), Fake::Failed];
    let meta = DocMeta { title: Some("A title".to_string()), outline: vec![(0, Some(0), "One".to_string())], renderer_only: false, file_bytes: 12 };
    let BuildOutcome::Ready { summary, .. } = build(&root, key, &mut FakeFeed::new(&pages), &meta, &BuildLimits::default()).unwrap() else { panic!() };
    let paths = CorpusPaths::new(&root, key);
    assert!(!paths.agent.exists(), "a build wrote the text twice");
    assert_eq!(agent_folder_bytes(&root, key), None);
    assert!(summary.corpus_bytes < 32 * 1024, "{summary:?}");
    fs::remove_file(temp.0.join("doc.pdf")).unwrap();

    let made = ensure_agent_folder(&root, key, &BuildLimits::default()).unwrap();
    assert_eq!((made.path.as_path(), made.pages_written), (paths.agent.as_path(), 4));
    assert!(agent_folder_bytes(&root, key).is_some());
    let read = |page: u32| fs::read_to_string(paths.page_file(page, 4)).unwrap();
    assert!(read(0).ends_with("--> \nThe Supplier shall\nnot be liable\n") || read(0).contains("-->\nThe Supplier shall\nnot be liable\n"), "{}", read(0));
    assert!(read(1).contains("hidden=flagged") && read(1).contains("! see [hidden: secret words] here"), "{}", read(1));
    assert!(read(2).contains("text=none"), "{}", read(2));
    assert!(read(3).contains("text=failed") && read(3).contains("stopped"), "{}", read(3));
    let manifest: Value = serde_json::from_slice(&fs::read(&paths.manifest).unwrap()).unwrap();
    assert_eq!(manifest["untrusted"]["title"], "A title", "what the manifest says of the file came from the build, with no PDF here to ask");
    assert_eq!(manifest["untrusted"]["outline"][0]["t"], "One");
    assert_eq!(manifest["file_bytes"], 12);
    assert_eq!(ensure_agent_folder(&root, key, &BuildLimits::default()).unwrap().pages_written, 0, "there's nothing to do twice");
}

/// Failure: page files aren't synced, so after a power cut a finished folder holds an empty page or none, and an agent reads that as a page with nothing on it. Asking for the folder finds each by its size and mends it, and a missing manifest too.
#[test]
fn asking_for_the_agent_folder_mends_what_a_power_cut_left() {
    let temp = Temp::new();
    let key = key_of(&temp, b"a file");
    let root = temp.0.join("corpus");
    build(&root, key, &mut FakeFeed::new(&[PLAIN, PLAIN, PLAIN]), &DocMeta::default(), &BuildLimits::default()).unwrap();
    ensure_agent_folder(&root, key, &BuildLimits::default()).unwrap();
    let paths = CorpusPaths::new(&root, key);
    let whole = fs::read(paths.page_file(1, 4)).unwrap();
    fs::write(paths.page_file(0, 4), b"").unwrap();
    fs::remove_file(paths.page_file(1, 4)).unwrap();
    assert_eq!(ensure_agent_folder(&root, key, &BuildLimits::default()).unwrap().pages_written, 2);
    assert_eq!(fs::read(paths.page_file(0, 4)).unwrap(), whole.iter().map(|byte| if *byte == b'2' { b'1' } else { *byte }).collect::<Vec<u8>>());
    assert_eq!(fs::read(paths.page_file(1, 4)).unwrap(), whole);
    fs::remove_file(&paths.manifest).unwrap();
    assert_eq!(agent_folder_bytes(&root, key), None, "with no manifest it isn't finished");
    assert_eq!(ensure_agent_folder(&root, key, &BuildLimits::default()).unwrap().pages_written, 0);
    assert!(paths.manifest.is_file());
}

/// Failure: the agent's folder gets made from a corpus that's being built or doesn't add up, or fills a disk with whole blocks for many small pages, or a retry leaves it saying a page failed that's since been read. And `index clear` leaves one form behind.
#[test]
fn the_agent_folder_is_held_to_the_same_rules() {
    let temp = Temp::new();
    let key = key_of(&temp, b"a file");
    let root = temp.0.join("corpus");
    assert_eq!(ensure_agent_folder(&root, key, &BuildLimits::default()), Err(AgentFolderError::NotReady));
    build(&root, key, &mut FakeFeed::new(&[PLAIN, PLAIN]), &DocMeta::default(), &BuildLimits::default()).unwrap();
    let paths = CorpusPaths::new(&root, key);

    let lock = local::append_private_file(&paths.lock).unwrap();
    lock.try_lock().unwrap();
    assert_eq!(ensure_agent_folder(&root, key, &BuildLimits::default()), Err(AgentFolderError::Busy));
    drop(lock);

    let tight = BuildLimits { max_doc_bytes: 4096, ..BuildLimits::default() };
    assert_eq!(ensure_agent_folder(&root, key, &tight), Err(AgentFolderError::TooLarge));
    assert!(!paths.agent.exists(), "what's too large leaves nothing behind");

    let pack = fs::read(&paths.pack).unwrap();
    let mut bad = pack.clone();
    bad[0] = bad[0].wrapping_add(1);
    fs::write(&paths.pack, bad).unwrap();
    assert_eq!(ensure_agent_folder(&root, key, &BuildLimits::default()), Err(AgentFolderError::Damaged));
    fs::write(&paths.pack, pack).unwrap();

    ensure_agent_folder(&root, key, &BuildLimits::default()).unwrap();
    assert!(paths.manifest.is_file() && paths.pack.is_file());
    clear(&root, key).unwrap();
    assert!(!paths.manifest.exists() && !paths.pack.exists() && !root.join(key.hex()).exists(), "a clear takes both forms");

    // Failed when it was first built, and made since: a retry that reads the page must change the folder too.
    let key = key_of(&temp, b"another file");
    let paths = CorpusPaths::new(&root, key);
    let flaky = [PLAIN, Fake::LostWorker];
    build(&root, key, &mut FakeFeed::new(&flaky), &DocMeta::default(), &BuildLimits::default()).unwrap();
    ensure_agent_folder(&root, key, &BuildLimits::default()).unwrap();
    assert!(fs::read_to_string(paths.page_file(1, 4)).unwrap().contains("text=failed"));
    let mut ready: Value = serde_json::from_slice(&fs::read(&paths.ready).unwrap()).unwrap();
    ready["retried_at"] = json!(now_seconds() - RETRY_PAUSE.as_secs() - 1);
    fs::write(&paths.ready, ready.to_string()).unwrap();
    let fixed = [PLAIN, Fake::Text("read at last")];
    build(&root, key, &mut FakeFeed::new(&fixed), &DocMeta::default(), &BuildLimits::default()).unwrap();
    assert!(fs::read_to_string(paths.page_file(1, 4)).unwrap().contains("read at last"));
    let manifest: Value = serde_json::from_slice(&fs::read(&paths.manifest).unwrap()).unwrap();
    assert_eq!(manifest["failed_pages"], json!([]));
}

/// Failure: a corpus someone has damaged panics the session, or its bad bytes reach an agent as a document's text. Each way it can fail to add up is said, and a page past the end of a file or with broken UTF-8 isn't believed.
#[test]
fn a_damaged_corpus_is_not_believed() {
    let temp = Temp::new();
    let key = key_of(&temp, b"a file");
    let root = temp.0.join("corpus");
    build(&root, key, &mut FakeFeed::new(&[PLAIN, PLAIN]), &DocMeta::default(), &BuildLimits::default()).unwrap();
    let paths = CorpusPaths::new(&root, key);
    let (ready, table, pack) = (fs::read(&paths.ready).unwrap(), fs::read(&paths.table).unwrap(), fs::read(&paths.pack).unwrap());
    let restore = || {
        fs::write(&paths.ready, &ready).unwrap();
        fs::write(&paths.table, &table).unwrap();
        fs::write(&paths.pack, &pack).unwrap();
    };
    fs::write(&paths.ready, b"not json").unwrap();
    assert!(ReadyCorpus::open(&root, key).unwrap().is_none(), "no mark that reads, so not finished");
    restore();
    fs::write(&paths.ready, br#"{"format":"v0"}"#).unwrap();
    assert_eq!(ReadyCorpus::open(&root, key).unwrap_err(), Damaged, "a mark of another format");
    assert_eq!(disk_state(&root, key), DiskState::Partial { pages_done: 2 }, "and it isn't called finished");
    restore();
    fs::write(&paths.pack, b"short").unwrap();
    assert_eq!(ReadyCorpus::open(&root, key).unwrap_err(), Damaged, "a pack of the wrong length");
    restore();
    let mut shorter = table.clone();
    shorter.truncate(RECORD_LEN);
    fs::write(&paths.table, shorter).unwrap();
    assert_eq!(ReadyCorpus::open(&root, key).unwrap_err(), Damaged, "a table of the wrong length");
    restore();
    let mut bad = pack.clone();
    bad[0] = 0xFF;
    fs::write(&paths.pack, bad).unwrap();
    let corpus = ReadyCorpus::open(&root, key).unwrap().unwrap();
    assert_eq!(corpus.page(0, &mut Vec::new()).unwrap_err(), Damaged, "text that isn't UTF-8");
    restore();
    let mut far = table.clone();
    far[..8].copy_from_slice(&u64::MAX.to_le_bytes());
    fs::write(&paths.table, far).unwrap();
    let corpus = ReadyCorpus::open(&root, key).unwrap().unwrap();
    assert_eq!(corpus.page(0, &mut Vec::new()).unwrap_err(), Damaged, "a record past the end of the file");
    restore();
    let mut huge = table.clone();
    huge[8..12].copy_from_slice(&u32::MAX.to_le_bytes());
    fs::write(&paths.table, huge).unwrap();
    let corpus = ReadyCorpus::open(&root, key).unwrap().unwrap();
    assert_eq!(corpus.page(0, &mut Vec::new()).unwrap_err(), Damaged, "a record asking for 4 GiB");
    restore();
    // A byte changed to another good one, which UTF-8 can't see.
    let mut other = pack.clone();
    other[0] = other[0].wrapping_add(1);
    fs::write(&paths.pack, other).unwrap();
    let corpus = ReadyCorpus::open(&root, key).unwrap().unwrap();
    assert_eq!(corpus.page(0, &mut Vec::new()).unwrap_err(), Damaged, "a page's text changed and still good UTF-8");
    restore();
    // Page 2's record said to be page 1's: its text is as good as any, and it's another page's.
    let mut swapped = table.clone();
    swapped.copy_within(RECORD_LEN..RECORD_LEN * 2, 0);
    fs::write(&paths.table, swapped).unwrap();
    let corpus = ReadyCorpus::open(&root, key).unwrap().unwrap();
    assert_eq!(corpus.page(0, &mut Vec::new()).unwrap_err(), Damaged, "one page's record standing in for another's");
    restore();
    // What a record says of its page, a bit at a time: hidden text reading as clean, or another shape of page altogether.
    for (byte, change) in [(21, FLAG_HIDDEN_FLAGGED), (21, FLAG_HIDDEN_CHECKED), (20, 3), (8, 1), (12, 1), (16, 1), (22, 1)] {
        let mut flipped = table.clone();
        flipped[byte] ^= change;
        fs::write(&paths.table, flipped).unwrap();
        let corpus = ReadyCorpus::open(&root, key).unwrap().unwrap();
        assert_eq!(corpus.page(0, &mut Vec::new()).map(|page| page.text.to_string()), Err(Damaged), "byte {byte} of a record changed by {change}");
    }
    restore();
    // One file's folder under another file's name.
    let other_key = key_of(&temp, b"a different file");
    fs::create_dir_all(root.join(other_key.hex())).unwrap();
    fs::set_permissions(root.join(other_key.hex()), std::os::unix::fs::PermissionsExt::from_mode(0o700)).unwrap();
    assert!(std::process::Command::new("/bin/cp").arg("-Rp").arg(root.join(key.hex()).join(FORMAT)).arg(root.join(other_key.hex())).status().unwrap().success());
    assert_eq!(ReadyCorpus::open(&root, other_key).unwrap_err(), Damaged, "a folder copied under another file's key answers for it");
    assert_ne!(disk_state(&root, other_key), disk_state(&root, key));
    assert!(ReadyCorpus::open(&root, key).unwrap().is_some());
    assert!(Aux::read(&[0xFF, 0xFF]).is_none() && Aux::read(&[1, 0, 200, 0]).is_none() && Aux::read(&[0, 0, 200, 0, 0, 0]).is_none(), "what's beside the text is checked too");
}

/// Failure: a search's job thread blocks for ever on a pipe left where a corpus's file goes, reads a table of any size into memory before looking at its length, or follows a link to read something else as a document's text. Each is refused with no wait, however it got there.
#[test]
fn reading_back_doesnt_wait_or_follow_or_fill_memory() {
    let temp = Temp::new();
    let key = key_of(&temp, b"a file");
    let root = temp.0.join("corpus");
    build(&root, key, &mut FakeFeed::new(&[PLAIN]), &DocMeta::default(), &BuildLimits::default()).unwrap();
    let paths = CorpusPaths::new(&root, key);
    let (table, pack) = (fs::read(&paths.table).unwrap(), fs::read(&paths.pack).unwrap());
    let restore = |path: &Path, bytes: &[u8]| {
        let _ = fs::remove_file(path);
        fs::write(path, bytes).unwrap();
        fs::set_permissions(path, std::os::unix::fs::PermissionsExt::from_mode(0o600)).unwrap();
    };
    let fifo = |path: &Path| {
        fs::remove_file(path).unwrap();
        assert!(std::process::Command::new("/usr/bin/mkfifo").arg(path).status().unwrap().success());
    };
    let started = Instant::now();
    fifo(&paths.table);
    assert_eq!(ReadyCorpus::open(&root, key).unwrap_err(), Damaged, "a pipe at the table");
    restore(&paths.table, &table);
    fifo(&paths.pack);
    assert_eq!(ReadyCorpus::open(&root, key).unwrap_err(), Damaged, "a pipe at the pack");
    restore(&paths.pack, &pack);
    assert!(started.elapsed() < Duration::from_secs(20), "{:?}", started.elapsed());

    // Sparse, so it's 64 GiB to anyone who reads it and nothing to the disk.
    File::options().write(true).open(&paths.table).unwrap().set_len(64 * 1024 * 1024 * 1024).unwrap();
    assert_eq!(ReadyCorpus::open(&root, key).unwrap_err(), Damaged, "a huge table is read before its length is looked at");
    restore(&paths.table, &table);
    assert!(ReadyCorpus::open(&root, key).unwrap().is_some(), "and it's all there again");

    // A link at the pack, at the table, at the folder and at the root.
    let elsewhere = temp.file("elsewhere", &pack);
    fs::remove_file(&paths.pack).unwrap();
    std::os::unix::fs::symlink(&elsewhere, &paths.pack).unwrap();
    assert_eq!(ReadyCorpus::open(&root, key).unwrap_err(), Damaged, "a link at the pack");
    restore(&paths.pack, &pack);
    let key_folder = root.join(key.hex());
    let moved = temp.0.join("moved-away");
    fs::rename(&key_folder, &moved).unwrap();
    std::os::unix::fs::symlink(&moved, &key_folder).unwrap();
    assert_eq!(ReadyCorpus::open(&root, key).unwrap_err(), Damaged, "a link at the key's folder");
    assert_eq!(disk_state(&root, key), DiskState::Nothing);
    fs::remove_file(&key_folder).unwrap();
    fs::rename(&moved, &key_folder).unwrap();
    let linked_root = temp.0.join("linked-root");
    std::os::unix::fs::symlink(&root, &linked_root).unwrap();
    assert_eq!(ReadyCorpus::open(&linked_root, key).unwrap_err(), Damaged, "a link at the root");
    assert_eq!(disk_state(&linked_root, key), DiskState::Nothing);
    assert!(ReadyCorpus::open(&root, key).unwrap().is_some());
}

/// Failure: the corpus folder grows for ever, one that's in use goes, one being built is pulled out from under its builder, or anything in the folder that isn't a corpus gets deleted.
#[test]
fn eviction_takes_the_oldest_and_nothing_else() {
    let temp = Temp::new();
    let root = temp.0.join("corpus");
    let doc = temp.file("doc.pdf", b"a doc");
    let keys: Vec<CorpusKey> = (0..4u8).map(|at| CorpusKey([at + 1; 32])).collect();
    for key in &keys {
        build(&root, *key, &mut FakeFeed::new(&[PLAIN, PLAIN]), &DocMeta::default(), &BuildLimits::default()).unwrap();
    }
    let _ = doc;
    let now = SystemTime::now();
    for (at, key) in keys.iter().enumerate() {
        let used = File::options().write(true).open(CorpusPaths::new(&root, *key).used).unwrap();
        used.set_modified(now - Duration::from_secs(3600 * (10 - at as u64))).unwrap();
    }
    let stray_folder = root.join("not-a-corpus");
    fs::create_dir(&stray_folder).unwrap();
    let stray_file = root.join(CorpusKey([200; 32]).hex());
    fs::write(&stray_file, b"named like a key, and only a file").unwrap();
    let outside = temp.0.join("outside");
    fs::create_dir(&outside).unwrap();
    fs::write(outside.join("precious"), b"keep me").unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(&outside, root.join(CorpusKey([201; 32]).hex())).unwrap();

    let one = folder_bytes(&root.join(keys[0].hex()));
    assert!(one > 0);
    // The oldest is being built, so it stays, and the next oldest goes.
    let lock = File::options().write(true).open(CorpusPaths::new(&root, keys[0]).lock).unwrap();
    lock.try_lock().unwrap();
    let limits = BuildLimits { max_total_bytes: one * 2 + one / 2, ..BuildLimits::default() };
    let report = evict(&root, &limits, Some(keys[1])).unwrap();
    assert_eq!(report.corpora, 2, "{report:?}");
    assert!(report.kept_bytes <= limits.max_total_bytes);
    assert!(root.join(keys[0].hex()).exists(), "being built");
    assert!(root.join(keys[1].hex()).exists(), "asked to keep");
    assert!(!root.join(keys[2].hex()).exists() && !root.join(keys[3].hex()).exists(), "the next oldest went");
    assert!(stray_folder.exists() && stray_file.exists(), "not ours");
    assert_eq!(fs::read(outside.join("precious")).unwrap(), b"keep me");
    drop(lock);

    // Past its age, however much room there is.
    let aged = BuildLimits { max_age: Duration::from_secs(60), ..BuildLimits::default() };
    assert_eq!(evict(&root, &aged, None).unwrap().corpora, 2);
    assert!(!root.join(keys[0].hex()).exists());
    assert_eq!(evict(&root, &BuildLimits::default(), None).unwrap().corpora, 0, "nothing left of its own to take");
}

/// Failure: a build finishes, and the total goes past the cap because nothing ever looks. A build's last step is to make room.
#[test]
fn a_finished_build_makes_room() {
    let temp = Temp::new();
    let root = temp.0.join("corpus");
    let tight = BuildLimits { max_total_bytes: 1, ..BuildLimits::default() };
    let (first, second) = (CorpusKey([1; 32]), CorpusKey([2; 32]));
    build(&root, first, &mut FakeFeed::new(&[PLAIN]), &DocMeta::default(), &BuildLimits::default()).unwrap();
    build(&root, second, &mut FakeFeed::new(&[PLAIN]), &DocMeta::default(), &tight).unwrap();
    assert!(!root.join(first.hex()).exists(), "the old one made way");
    assert!(ReadyCorpus::open(&root, second).unwrap().is_some(), "and the one just built stays, whatever the cap");
}

/// Failure: only a build that succeeds ever makes room, so a session whose builds are refused, paused or out of disk leaves the folder past its cap and past its age for good.
#[test]
fn a_build_makes_room_however_it_comes_out() {
    let temp = Temp::new();
    let root = temp.0.join("corpus");
    let tight = BuildLimits { max_total_bytes: 1, ..BuildLimits::default() };
    let old = CorpusKey([1; 32]);
    build(&root, old, &mut FakeFeed::new(&[PLAIN]), &DocMeta::default(), &BuildLimits::default()).unwrap();
    assert_eq!(build(&root, CorpusKey([2; 32]), &mut FakeFeed::new(&[Fake::Stop]), &DocMeta::default(), &tight).unwrap(), BuildOutcome::Paused("stopped_here"));
    assert!(!root.join(old.hex()).exists(), "a build that paused left the folder over its cap");

    build(&root, old, &mut FakeFeed::new(&[PLAIN]), &DocMeta::default(), &BuildLimits::default()).unwrap();
    assert_eq!(build(&root, CorpusKey([3; 32]), &mut FakeFeed::new(&[Fake::Refuse]), &DocMeta::default(), &tight).unwrap(), BuildOutcome::Refused("needs_password".to_string()));
    assert!(!root.join(old.hex()).exists(), "a build that was refused left the folder over its cap");
}

/// Failure: the disk fills up and the cache, well under its own cap, keeps everything it has, so every build fails on it for ever and the rest of the Mac has no room either. Here it gives back room however little it's holding against its cap, and never what's being built.
#[test]
fn a_full_disk_gets_room_back() {
    let temp = Temp::new();
    let root = temp.0.join("corpus");
    let keys: Vec<CorpusKey> = (1..=3u8).map(|at| CorpusKey([at; 32])).collect();
    for key in &keys {
        build(&root, *key, &mut FakeFeed::new(&[PLAIN]), &DocMeta::default(), &BuildLimits::default()).unwrap();
    }
    let report = make_room(&root, &BuildLimits::default(), keys[2]).unwrap();
    assert_eq!(report.corpora, 2, "{report:?}");
    assert!(root.join(keys[2].hex()).exists());
    assert_eq!(make_room(&root, &BuildLimits::default(), keys[2]).unwrap().corpora, 0, "what's being built stays");
}

/// Failure: notes about old versions hold a file's path, so a folder meant to say nothing about a document says where it is, or pile up for ever after their corpora have gone. Or one file's new version takes away a different file's text.
#[test]
fn notes_about_versions_say_nothing_and_dont_pile_up() {
    let temp = Temp::new();
    let root = temp.0.join("corpus");
    let (file, other) = (temp.file("a-very-distinctive-name.pdf", b"one"), temp.file("other.pdf", b"other"));
    let (first, other_key) = (CorpusKey::of_file(&file).unwrap(), CorpusKey::of_file(&other).unwrap());
    build_for_file(&file, &root, first, &mut FakeFeed::new(&[PLAIN]), &DocMeta::default(), &BuildLimits::default()).unwrap();
    build_for_file(&other, &root, other_key, &mut FakeFeed::new(&[PLAIN]), &DocMeta::default(), &BuildLimits::default()).unwrap();
    let notes = every_file(&root.join(VERSIONS_FOLDER));
    assert!(!notes.is_empty());
    for (name, bytes) in &notes {
        let shown = format!("{name:?} {}", String::from_utf8_lossy(bytes));
        assert!(!shown.contains("distinctive") && !shown.contains("recto-corpus"), "{shown}");
    }
    assert_eq!(keys_of_file(&root, &file), vec![first]);
    fs::write(&file, b"two").unwrap();
    let second = CorpusKey::of_file(&file).unwrap();
    assert_ne!(first, second);
    assert_eq!(keys_of_file(&root, &file), vec![second, first], "the old one can still be found");
    build_for_file(&file, &root, second, &mut FakeFeed::new(&[PLAIN]), &DocMeta::default(), &BuildLimits::default()).unwrap();
    assert!(!root.join(first.hex()).exists());
    assert!(root.join(other_key.hex()).exists(), "another file's stays");
    assert_eq!(keys_of_file(&root, &file), vec![second]);

    // A corpus that went some other way leaves its notes to be cleared.
    let count = every_file(&root.join(VERSIONS_FOLDER)).len();
    clear(&root, other_key).unwrap();
    evict(&root, &BuildLimits::default(), None).unwrap();
    assert!(every_file(&root.join(VERSIONS_FOLDER)).len() < count, "notes outlive their corpus");

    fs::remove_file(&file).unwrap();
    assert_eq!(keys_of_file(&root, &file), vec![second], "with the file gone, its path still finds what it had");
    forget_file(&root, &file);
    assert_eq!(keys_of_file(&root, &file), Vec::new());
}

/// Failure: a corpus's plain text ends up in the user's Time Machine backups, for good, after it's been cleared here. The folder is marked, in the way `tmutil isexcluded` reads.
#[test]
fn the_corpus_folder_stays_out_of_backups() {
    let temp = Temp::new();
    let root = temp.0.join("corpus.noindex");
    build(&root, CorpusKey([1; 32]), &mut FakeFeed::new(&[PLAIN]), &DocMeta::default(), &BuildLimits::default()).unwrap();
    assert!(local::is_excluded_from_backups(&root));
    assert!(!local::is_excluded_from_backups(&temp.0), "this only marks what it's given");
}

/// Failure: the redaction case. A build's killed partway, the file is redacted, and the new version is indexed. The text the first build wrote was never noted, because only finished builds were, so neither that finish nor `index clear` finds it, and it stays for 90 days. Or removing the old one fails, and its note is overwritten, so nothing can ever find it.
#[test]
fn a_partial_build_of_an_old_version_can_be_found_and_goes() {
    let temp = Temp::new();
    let root = temp.0.join("corpus");
    let file = temp.file("redact.pdf", b"before the redaction");
    let first = CorpusKey::of_file(&file).unwrap();
    let secret: &'static str = "the secret that gets redacted";
    let stopped = [Fake::Text(secret), Fake::Stop];
    assert_eq!(build_for_file(&file, &root, first, &mut FakeFeed::new(&stopped), &DocMeta::default(), &BuildLimits::default()).unwrap(), BuildOutcome::Paused("stopped_here"));
    assert!(String::from_utf8_lossy(&fs::read(CorpusPaths::new(&root, first).pack).unwrap()).contains(secret), "it's meant to have been written");
    assert!(keys_of_file(&root, &file).contains(&first), "a partial build's text can't be found");

    fs::write(&file, b"after the redaction!").unwrap();
    let second = CorpusKey::of_file(&file).unwrap();
    assert_ne!(first, second);
    assert!(keys_of_file(&root, &file).contains(&first), "and it can still be found by the path, for a clear");

    // Its removal fails the first time, as it's locked: the note must stay.
    let lock = local::append_private_file(&CorpusPaths::new(&root, first).lock).unwrap();
    lock.try_lock().unwrap();
    build_for_file(&file, &root, second, &mut FakeFeed::new(&[PLAIN]), &DocMeta::default(), &BuildLimits::default()).unwrap();
    assert!(root.join(first.hex()).exists(), "it's being built by something, so it stays");
    assert_eq!(keys_of_file(&root, &file), vec![second, first], "and it's still noted");
    drop(lock);
    // The next finish takes it.
    build_for_file(&file, &root, second, &mut FakeFeed::new(&[PLAIN]), &DocMeta::default(), &BuildLimits::default()).unwrap();
    assert!(!root.join(first.hex()).exists(), "the old partial text is still on disk");
    assert_eq!(keys_of_file(&root, &file), vec![second]);
    for (name, bytes) in every_file(&root) {
        assert!(!String::from_utf8_lossy(&bytes).contains(secret), "{name:?} kept the redacted text");
    }
}

/// Failure: a file that keeps changing under builds that never finish piles up notes and partial text for ever.
#[test]
fn notes_on_a_file_are_capped_and_the_oldest_text_goes() {
    let temp = Temp::new();
    let root = temp.0.join("corpus");
    let file = temp.file("busy.pdf", b"a doc");
    let first = CorpusKey([1; 32]);
    for at in 0..(MAX_KEYS_PER_ALIAS + 3) {
        build_for_file(&file, &root, CorpusKey([at as u8 + 1; 32]), &mut FakeFeed::new(&[Fake::Text("text"), Fake::Stop]), &DocMeta::default(), &BuildLimits::default()).unwrap();
    }
    assert!(!root.join(first.hex()).exists(), "the oldest partial build's text stays past the cap");
    let now = CorpusKey::of_file(&file).unwrap();
    assert_eq!(keys_of_file(&root, &file).iter().filter(|key| **key != now).count(), MAX_KEYS_PER_ALIAS);
}

/// Failure: a pipe left at a tab's path blocks `index clear` for ever, pinning a job thread, because looking up what it's had opened it.
#[test]
fn a_pipe_at_a_files_path_doesnt_block_looking_it_up() {
    let temp = Temp::new();
    let root = temp.0.join("corpus");
    let fifo = temp.0.join("pipe.pdf");
    assert!(std::process::Command::new("/usr/bin/mkfifo").arg(&fifo).status().unwrap().success());
    let (done, finished) = std::sync::mpsc::channel();
    let (root_copy, fifo_copy) = (root.clone(), fifo.clone());
    std::thread::spawn(move || {
        let keys = keys_of_file(&root_copy, &fifo_copy);
        forget_file(&root_copy, &fifo_copy);
        let _ = done.send(keys);
    });
    assert_eq!(finished.recv_timeout(Duration::from_secs(20)).expect("it blocked on the pipe"), Vec::new());
}

/// Failure: two sessions on one folder both build a document, and one's pack interleaves with the other's.
#[test]
fn only_one_builds_at_a_time() {
    let temp = Temp::new();
    let root = temp.0.join("corpus");
    let key = CorpusKey([3; 32]);
    CorpusPaths::new(&root, key).prepare(&root).unwrap();
    let lock = local::append_private_file(&CorpusPaths::new(&root, key).lock).unwrap();
    lock.try_lock().unwrap();
    assert_eq!(build(&root, key, &mut FakeFeed::new(&[PLAIN]), &DocMeta::default(), &BuildLimits::default()).unwrap(), BuildOutcome::Busy);
    assert_eq!(clear(&root, key).unwrap_err().kind(), io::ErrorKind::WouldBlock, "and clearing leaves it alone");
    assert_eq!(clear_all(&root).unwrap(), Cleared { being_built: 1, ..Cleared::default() });
    drop(lock);
    assert_eq!(clear_all(&root).unwrap(), Cleared { cleared: 1, ..Cleared::default() });
    assert_eq!(clear_all(&temp.0.join("nothing-here")).unwrap(), Cleared::default());
}

/// Failure: a folder that can't be removed reads as "being built, and stayed", which says the text is on its way, when it's there and staying, and a person asked for it to be gone.
#[test]
fn a_clear_that_fails_doesnt_read_as_being_built() {
    use std::os::unix::fs::PermissionsExt;
    let temp = Temp::new();
    let root = temp.0.join("corpus");
    let key = CorpusKey([4; 32]);
    build(&root, key, &mut FakeFeed::new(&[PLAIN]), &DocMeta::default(), &BuildLimits::default()).unwrap();
    let app = CorpusPaths::new(&root, key).app;
    fs::set_permissions(&app, fs::Permissions::from_mode(0o500)).unwrap();
    let report = clear_all(&root).unwrap();
    fs::set_permissions(&app, fs::Permissions::from_mode(0o700)).unwrap();
    assert_eq!(report, Cleared { failed: 1, ..Cleared::default() });
    assert!(clear(&root, key).is_ok(), "with the way clear, it goes");
}

/// Failure: [`read_page`]'s rules change, and old corpora keep answering with old text, so `search` and `text` disagree until the file happens to change. If this fails, what you changed changes a page's text: raise [`NORMALISER`], which starts new corpora, and update this.
#[test]
fn a_change_to_the_text_changes_the_version() {
    assert_eq!(NORMALISER, "recto-normalize/1");
    let page = chars("A valu\u{FFFE}able thing\u{A0}and a data\u{2010}driven\u{200B}plan\r\nsoft\u{AD}ly, the 21-\ncm line, and -17-\nPBC");
    let read = read_page(&page, &[], false);
    assert_eq!(read.normalized.text, "A valuable thing and a data-drivenplan\nsoftly, the 21-cm line, and -17-\nPBC");
}

/// Failure: the largest page the wire allows, in the widest characters there are, is past the corpus's own cap on a page, so it reads back as damaged and a good corpus is thrown away for ever, build after build.
#[test]
fn the_largest_page_there_can_be_reads_back() {
    let temp = Temp::new();
    let key = key_of(&temp, b"a file");
    let root = temp.0.join("corpus");
    struct Largest;
    impl PageFeed for Largest {
        fn page_count(&mut self) -> Result<u32, FeedError> {
            Ok(1)
        }
        fn read(&mut self, _page: u32) -> Result<FedPage, FeedError> {
            Ok(FedPage { chars: vec![CharRecord::new(0x1F600, 0); recto_proto::MAX_PAGE_CHARS as usize], boxes: Vec::new(), hidden_checked: true })
        }
        fn still_the_same_file(&mut self) -> bool {
            true
        }
    }
    assert!(matches!(build(&root, key, &mut Largest, &DocMeta::default(), &BuildLimits::default()).unwrap(), BuildOutcome::Ready { .. }));
    let corpus = ReadyCorpus::open(&root, key).unwrap().unwrap();
    let mut buffer = Vec::new();
    let page = corpus.page(0, &mut buffer).unwrap();
    assert_eq!((page.kind, page.text.chars().count()), (PageKind::Text, recto_proto::MAX_PAGE_CHARS as usize));
}

/// Failure: a table that isn't checked for size becomes a hazard on a document of 200,000 pages.
#[test]
fn a_record_stays_small() {
    assert!(size_of::<PageRecord>() <= 32);
    assert_eq!(RECORD_LEN, 28);
    let record = PageRecord { offset: 9, text_len: 8, aux_len: 7, chars: 6, kind: PageKind::Failed, flags: 5, reason: 2, check: 77 };
    assert_eq!(PageRecord::decode(&record.encode()), Some(record));
    assert_eq!(PageRecord::decode(&[0; 28]), None, "no such kind");
    assert_eq!(PageRecord::decode(&[1; 3]), None);
    assert_eq!(page_width(9999), 4);
    assert_eq!(page_width(10_000), 5);
    assert_eq!(disk_size(0), 4096);
    assert_eq!(disk_size(4097), 8192);
}
