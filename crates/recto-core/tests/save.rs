//! The save path's writer, on files built here, so every expectation comes from bytes these tests wrote. Each test names the failure it catches.

use lopdf::{FileSource, IndexedReader, Object};
use recto_core::save::{self, AddedFile, PageRef, SaveError, SaveHooks, SaveRequest, SaveStep, Target, Writer};
use recto_platform::FileStamp;
use recto_testpdf::{Object as Raw, Page, build, pages, text_lines};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

static NEXT: AtomicU32 = AtomicU32::new(0);

/// A folder of the test's own, removed at the end.
struct Dir(PathBuf);

impl Dir {
    fn new() -> Self {
        let dir = std::env::temp_dir().join(format!("recto-save-test-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir.canonicalize().unwrap())
    }

    fn write(&self, name: &str, bytes: &[u8]) -> PathBuf {
        let path = self.0.join(name);
        std::fs::write(&path, bytes).unwrap();
        path
    }
}

impl Drop for Dir {
    fn drop(&mut self) {
        let _ = std::process::Command::new("/bin/chmod").args(["-R", "u+w"]).arg(&self.0).status();
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A document whose pages each say their own marker.
fn marked(markers: &[&str]) -> Vec<u8> {
    let pages: Vec<Page> = markers.iter().map(|marker| Page::new(text_lines(&[marker]))).collect();
    recto_testpdf::pages(&pages)
}

fn stamp(path: &Path) -> FileStamp {
    recto_platform::file_stamp(path).unwrap()
}

fn own(order: &[(u32, u8)]) -> Vec<PageRef> {
    order.iter().map(|(page, turns)| PageRef { source: 0, page: *page, quarter_turns: *turns }).collect()
}

fn request<'a>(path: &'a Path, file_pages: u32, pages: &'a [PageRef], added: &'a [AddedFile<'a>]) -> SaveRequest<'a> {
    SaveRequest { path, stamp: Some(stamp(path)), file_pages, pages, added }
}

/// Each page of a file, in order: its marker and its turn, read with the fork's own reader.
fn read_pages(path: &Path) -> Vec<(String, i64)> {
    let reader = IndexedReader::open(FileSource::open(path).unwrap()).unwrap();
    let map = reader.page_map().unwrap();
    map.iter()
        .map(|entry| {
            let Object::Dictionary(page) = reader.resolve_object(entry.id()).unwrap() else { panic!("a page that isn't a dictionary") };
            let rotate = match entry.inherited().rotate() {
                Some(owner) => match reader.resolve_object(owner).unwrap() {
                    Object::Dictionary(dict) => dict.get(b"Rotate").and_then(Object::as_i64).unwrap_or(0),
                    _ => 0,
                },
                None => 0,
            };
            let contents = page.get(b"Contents").and_then(Object::as_reference).unwrap();
            let Object::Stream(stream) = reader.resolve_object(contents).unwrap() else { panic!("contents that aren't a stream") };
            let text = String::from_utf8_lossy(&stream.content).into_owned();
            let marker = text.split('(').nth(1).and_then(|rest| rest.split(')').next()).unwrap_or("").to_string();
            (marker, rotate)
        })
        .collect()
}

fn contains(haystack: &[u8], needle: &str) -> bool {
    haystack.windows(needle.len()).any(|window| window == needle.as_bytes())
}

#[derive(Default)]
struct Noted {
    steps: Vec<SaveStep>,
    prepared: Option<FileStamp>,
}

impl SaveHooks for Noted {
    fn step(&mut self, step: SaveStep) {
        self.steps.push(step);
    }
    fn prepared(&mut self, _written: &Path, stamp: FileStamp) -> std::io::Result<()> {
        self.prepared = Some(stamp);
        Ok(())
    }
}

/// Failure: a turn rewrites the file, so a signature over its bytes breaks, or the turn doesn't reach the file. A turn is appended: every original byte stays, in place, and the page reads back turned.
#[test]
fn a_turn_is_appended_and_every_original_byte_stays() {
    let dir = Dir::new();
    let path = dir.write("doc.pdf", &marked(&["ONE", "TWO", "THREE"]));
    let before = std::fs::read(&path).unwrap();
    let order = own(&[(0, 0), (1, 1), (2, 3)]);
    let mut noted = Noted::default();
    let outcome = save::save(&request(&path, 3, &order, &[]), Target::Same, None, &mut noted).unwrap();
    assert_eq!(outcome.writer, Writer::Append);
    let after = std::fs::read(&path).unwrap();
    assert!(after.starts_with(&before) && after.len() > before.len(), "the original bytes are a prefix");
    assert_eq!(read_pages(&path), [("ONE".into(), 0), ("TWO".into(), 90), ("THREE".into(), 270)]);
    assert_eq!(noted.steps, [SaveStep::Checking, SaveStep::Writing, SaveStep::Prepared, SaveStep::Committing]);
    assert_eq!(noted.prepared, Some(stamp(&path)), "what the hook was told is what the file is once it's in place");
    assert_eq!(outcome.stamp, Some(stamp(&path)));
}

/// Failure: a page already turned in the file loses its own turn, or a turn past a whole one comes out as 360 or more.
#[test]
fn a_turn_adds_to_the_pages_own() {
    let dir = Dir::new();
    let mut turned = Page::new(text_lines(&["TURNED"]));
    turned.page_extra = "/Rotate 270".into();
    let path = dir.write("doc.pdf", &pages(&[turned, Page::new(text_lines(&["PLAIN"]))]));
    let order = own(&[(0, 2), (1, 0)]);
    save::save(&request(&path, 2, &order, &[]), Target::Same, None, &mut save::NoHooks).unwrap();
    assert_eq!(read_pages(&path), [("TURNED".into(), 90), ("PLAIN".into(), 0)]);
}

/// Failure: moved pages come out in the old order, or a page that took its size from the tree loses it once the tree is new. Moves are appended, as a new tree.
#[test]
fn moved_pages_come_out_in_their_new_order() {
    let dir = Dir::new();
    // The media box is on the tree, not the pages, so a new tree has to give it to each page.
    let bytes = build(&[
        Raw::Dict("<< /Type /Catalog /Pages 2 0 R >>".into()),
        Raw::Dict("<< /Type /Pages /Kids [3 0 R 5 0 R] /Count 2 /MediaBox [0 0 300 400] /Resources << /Font << /F1 7 0 R >> >> /Rotate 90 >>".into()),
        Raw::Dict("<< /Type /Page /Parent 2 0 R /Contents 4 0 R >>".into()),
        Raw::Stream { dict: String::new(), data: text_lines(&["FIRST"]) },
        Raw::Dict("<< /Type /Page /Parent 2 0 R /Contents 6 0 R >>".into()),
        Raw::Stream { dict: String::new(), data: text_lines(&["SECOND"]) },
        Raw::Dict("<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".into()),
    ]);
    let path = dir.write("doc.pdf", &bytes);
    let order = own(&[(1, 0), (0, 1)]);
    save::save(&request(&path, 2, &order, &[]), Target::Same, None, &mut save::NoHooks).unwrap();
    assert_eq!(read_pages(&path), [("SECOND".into(), 90), ("FIRST".into(), 180)], "the tree's own turn is kept, and added to");
    let reader = IndexedReader::open(FileSource::open(&path).unwrap()).unwrap();
    for entry in reader.page_map().unwrap().iter() {
        let Object::Dictionary(page) = reader.resolve_object(entry.id()).unwrap() else { panic!() };
        assert!(page.has(b"MediaBox") || entry.inherited().media_box().is_some(), "each page still has its size");
        assert!(page.has(b"Resources") || entry.inherited().resources().is_some(), "and its fonts");
    }
}

/// Failure: pages added from another PDF bring that PDF's other pages along through a link, or don't come at all. Only the pages added, and what they draw with, are copied in.
#[test]
fn added_pages_are_copied_in_without_the_rest_of_their_file() {
    let dir = Dir::new();
    let path = dir.write("doc.pdf", &marked(&["MINE"]));
    // Page 1 of the other file links to its page 2, which isn't added.
    let other = build(&[
        Raw::Dict("<< /Type /Catalog /Pages 2 0 R >>".into()),
        Raw::Dict("<< /Type /Pages /Kids [3 0 R 5 0 R] /Count 2 /Resources << /Font << /F1 7 0 R >> >> >>".into()),
        Raw::Dict("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Contents 4 0 R /Annots [8 0 R] >>".into()),
        Raw::Stream { dict: String::new(), data: text_lines(&["ADDED"]) },
        Raw::Dict("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Contents 6 0 R >>".into()),
        Raw::Stream { dict: String::new(), data: text_lines(&["NOTADDED"]) },
        Raw::Dict("<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".into()),
        Raw::Dict("<< /Type /Annot /Subtype /Link /Rect [0 0 10 10] /P 3 0 R /Dest [5 0 R /Fit] >>".into()),
    ]);
    let other_path = dir.write("other.pdf", &other);
    let added = [AddedFile { path: &other_path, page_count: 2, stamp: Some(stamp(&other_path)) }];
    let order = [PageRef { source: 0, page: 0, quarter_turns: 0 }, PageRef { source: 1, page: 0, quarter_turns: 1 }];
    save::save(&request(&path, 1, &order, &added), Target::Same, None, &mut save::NoHooks).unwrap();
    assert_eq!(read_pages(&path), [("MINE".into(), 0), ("ADDED".into(), 90)]);
    let bytes = std::fs::read(&path).unwrap();
    assert!(!contains(&bytes, "NOTADDED"), "the page that wasn't added isn't in the file");
    assert!(contains(&bytes, "/Helvetica"), "the font the added page draws with is");
}

/// Failure: a delete is appended, so the deleted page's objects stay in the file for anyone to read back. It needs the rewrite, and until that's built the save is refused with nothing written.
#[test]
fn a_delete_isnt_appended() {
    let dir = Dir::new();
    let path = dir.write("doc.pdf", &marked(&["KEEP", "DELETED"]));
    let before = std::fs::read(&path).unwrap();
    let order = own(&[(0, 0)]);
    assert_eq!(save::save(&request(&path, 2, &order, &[]), Target::Same, None, &mut save::NoHooks), Err(SaveError::NeedsRewrite));
    assert_eq!(std::fs::read(&path).unwrap(), before);
    assert_eq!(save::writer_for(2, &order), Writer::Rewrite);
    assert_eq!(save::writer_for(2, &own(&[(0, 0), (0, 0)])), Writer::Rewrite, "a page twice and another gone is still a page gone");
    assert_eq!(save::writer_for(2, &own(&[(1, 2), (0, 0)])), Writer::Append);
}

/// Failure: a forged journal claims a page the file doesn't have, or a length it isn't, and the save writes a page tree naming nothing. Every claim is checked against the real file first.
#[test]
fn a_journals_claims_are_checked_against_the_real_file() {
    let dir = Dir::new();
    let path = dir.write("doc.pdf", &marked(&["A", "B"]));
    let before = std::fs::read(&path).unwrap();
    let past_the_end = own(&[(0, 0), (1, 0), (2, 0)]);
    assert_eq!(save::save(&request(&path, 3, &past_the_end, &[]), Target::Same, None, &mut save::NoHooks), Err(SaveError::EditsDontMatch { file_pages: 2 }), "a length the file isn't");
    let beyond = own(&[(0, 0), (7, 0)]);
    assert_eq!(save::save(&request(&path, 2, &beyond, &[]), Target::Same, None, &mut save::NoHooks), Err(SaveError::EditsDontMatch { file_pages: 2 }), "a page the file hasn't got");
    let wild_turn = own(&[(0, 9), (1, 0)]);
    assert!(save::save(&request(&path, 2, &wild_turn, &[]), Target::Same, None, &mut save::NoHooks).is_err(), "a turn no edit makes");
    let no_source = [PageRef { source: 4, page: 0, quarter_turns: 0 }, PageRef { source: 0, page: 0, quarter_turns: 0 }, PageRef { source: 0, page: 1, quarter_turns: 0 }];
    assert!(save::save(&request(&path, 2, &no_source, &[]), Target::Same, None, &mut save::NoHooks).is_err(), "a PDF that was never added");
    assert_eq!(std::fs::read(&path).unwrap(), before, "and none of it wrote anything");
}

/// Failure: an added PDF that's changed since its pages were added is saved from, so the document gets pages it never showed. Its stamp is checked, and one never stamped isn't trusted.
#[test]
fn an_added_pdf_that_changed_isnt_saved_from() {
    let dir = Dir::new();
    let path = dir.write("doc.pdf", &marked(&["MINE"]));
    let other_path = dir.write("other.pdf", &marked(&["OLD"]));
    let known = stamp(&other_path);
    std::thread::sleep(std::time::Duration::from_millis(20));
    std::fs::write(&other_path, marked(&["NEW", "LONGER"])).unwrap();
    let order = [PageRef { source: 0, page: 0, quarter_turns: 0 }, PageRef { source: 1, page: 0, quarter_turns: 0 }];
    let changed = [AddedFile { path: &other_path, page_count: 1, stamp: Some(known) }];
    assert!(matches!(save::save(&request(&path, 1, &order, &changed), Target::Same, None, &mut save::NoHooks), Err(SaveError::AddedFileChanged { .. })));
    let unstamped = [AddedFile { path: &other_path, page_count: 2, stamp: None }];
    assert!(matches!(save::save(&request(&path, 1, &order, &unstamped), Target::Same, None, &mut save::NoHooks), Err(SaveError::AddedFileChanged { .. })));
}

/// Failure: a file changed on disk since it was opened is saved over, losing the other app's change. Both before writing, and in the moment between writing and the move.
#[test]
fn a_file_changed_meanwhile_isnt_saved_over() {
    let dir = Dir::new();
    let path = dir.write("doc.pdf", &marked(&["A"]));
    let known = stamp(&path);
    std::thread::sleep(std::time::Duration::from_millis(20));
    std::fs::write(&path, marked(&["B"])).unwrap();
    let order = own(&[(0, 1)]);
    let stale = SaveRequest { path: &path, stamp: Some(known), file_pages: 1, pages: &order, added: &[] };
    assert_eq!(save::save(&stale, Target::Same, None, &mut save::NoHooks), Err(SaveError::FileChanged), "changed before");

    /// Replaces the file while the save has it written beside it.
    struct Replacer(PathBuf);
    impl SaveHooks for Replacer {
        fn prepared(&mut self, _written: &Path, _stamp: FileStamp) -> std::io::Result<()> {
            std::fs::write(&self.0, marked(&["SOMEONE ELSE'S"]))
        }
    }
    let path = dir.write("during.pdf", &marked(&["A"]));
    let result = save::save(&request(&path, 1, &order, &[]), Target::Same, None, &mut Replacer(path.clone()));
    assert_eq!(result, Err(SaveError::FileChanged), "changed during");
    assert_eq!(read_pages(&path), [("SOMEONE ELSE'S".into(), 0)], "and the other version stays");
}

/// Failure: a step that has to happen before the file's in place, such as noting the save in the journal, fails and the file is replaced anyway. Nothing changes, and nothing's left beside it.
#[test]
fn a_failed_note_leaves_the_file_as_it_was() {
    struct Refuse;
    impl SaveHooks for Refuse {
        fn prepared(&mut self, _written: &Path, _stamp: FileStamp) -> std::io::Result<()> {
            Err(std::io::Error::other("the journal couldn't be written"))
        }
    }
    let dir = Dir::new();
    let path = dir.write("doc.pdf", &marked(&["A"]));
    let before = std::fs::read(&path).unwrap();
    let order = own(&[(0, 1)]);
    assert_eq!(save::save(&request(&path, 1, &order, &[]), Target::Same, None, &mut Refuse), Err(SaveError::PrepareFailed));
    assert_eq!(std::fs::read(&path).unwrap(), before);
    let left: Vec<_> = std::fs::read_dir(&dir.0).unwrap().filter_map(Result::ok).map(|entry| entry.file_name()).collect();
    assert_eq!(left.len(), 1, "no temporary file left: {left:?}");
}

/// A document with a signature field, signed or certified.
fn signed(certified: bool, usage_rights_only: bool) -> Vec<u8> {
    let perms = match (certified, usage_rights_only) {
        (true, _) => "/Perms << /DocMDP 6 0 R >>",
        (false, true) => "/Perms << /UR3 6 0 R >>",
        _ => "",
    };
    let form = if usage_rights_only { String::new() } else { "/AcroForm << /Fields [5 0 R] /SigFlags 3 >>".to_string() };
    build(&[
        Raw::Dict(format!("<< /Type /Catalog /Pages 2 0 R {form} {perms} >>")),
        Raw::Dict("<< /Type /Pages /Kids [3 0 R] /Count 1 >>".into()),
        Raw::Dict("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Contents 4 0 R >>".into()),
        Raw::Stream { dict: String::new(), data: b"BT ET".to_vec() },
        Raw::Dict("<< /FT /Sig /T (Signature1) /V 6 0 R /Rect [0 0 0 0] /Subtype /Widget /P 3 0 R >>".into()),
        Raw::Dict("<< /Type /Sig /Filter /Adobe.PPKLite /SubFilter /adbe.pkcs7.detached /ByteRange [0 10 20 30] /Contents <00> /Reference [<< /TransformMethod /DocMDP /TransformParams << /P 2 >> >>] >>".into()),
    ])
}

/// Failure: a turn on a signed PDF changes the bytes its signature covers. Appended, they're all still there, byte for byte (SPEC decision 27: additions keep the signature).
#[test]
fn a_signed_files_bytes_never_change() {
    let dir = Dir::new();
    let path = dir.write("signed.pdf", &signed(false, false));
    let before = std::fs::read(&path).unwrap();
    let order = own(&[(0, 1)]);
    let outcome = save::save(&request(&path, 1, &order, &[]), Target::Same, None, &mut save::NoHooks).unwrap();
    assert!(!outcome.usage_rights_removed);
    assert!(std::fs::read(&path).unwrap().starts_with(&before));
}

/// Failure: a certified PDF's pages are changed, which its certification forbids at every level. Refused, with the file untouched.
#[test]
fn a_certified_files_pages_arent_changed() {
    let dir = Dir::new();
    let path = dir.write("certified.pdf", &signed(true, false));
    let before = std::fs::read(&path).unwrap();
    let order = own(&[(0, 1)]);
    assert_eq!(save::save(&request(&path, 1, &order, &[]), Target::Same, None, &mut save::NoHooks), Err(SaveError::BreaksCertification));
    assert_eq!(std::fs::read(&path).unwrap(), before);
    assert!(save::signature_facts_of(&path).unwrap().certified);
    let copy = path.with_file_name("copy.pdf");
    assert_eq!(save::save(&request(&path, 1, &order, &[]), Target::NewFile(&copy), None, &mut save::NoHooks), Err(SaveError::UnsignedCopyNeedsRewrite), "an unsigned copy is the rewrite's, and nothing says otherwise");
    assert!(!copy.exists());
}

/// Failure: a file with usage rights alone, as IRS forms have, can't be saved, or is saved with rights a reader then calls broken. Saved, with them dropped from the new revision, and said (owner, 2026-11-08).
#[test]
fn usage_rights_go_quietly() {
    let dir = Dir::new();
    let path = dir.write("irs.pdf", &signed(false, true));
    let before = std::fs::read(&path).unwrap();
    assert!(save::signature_facts_of(&path).unwrap().usage_rights);
    let order = own(&[(0, 1)]);
    let outcome = save::save(&request(&path, 1, &order, &[]), Target::Same, None, &mut save::NoHooks).unwrap();
    assert!(outcome.usage_rights_removed);
    assert!(std::fs::read(&path).unwrap().starts_with(&before), "appended, as ever");
    assert!(!save::signature_facts_of(&path).unwrap().usage_rights, "the new revision has none");
}

/// Failure: a save over a file loses its Finder tags and, worse, the quarantine flag of a download, which Gatekeeper goes by. A Save As keeps where it came from, and nothing that's about the original alone.
#[test]
fn a_saved_file_keeps_its_attributes() {
    let dir = Dir::new();
    let path = dir.write("download.pdf", &marked(&["A"]));
    recto_platform::local::set_attribute(&path, "com.apple.quarantine", b"0083;5f000000;Safari;").unwrap();
    recto_platform::local::set_attribute(&path, "com.apple.metadata:_kMDItemUserTags", b"red-tag").unwrap();
    let order = own(&[(0, 1)]);
    let copy = dir.0.join("copy.pdf");
    save::save(&request(&path, 1, &order, &[]), Target::NewFile(&copy), None, &mut save::NoHooks).unwrap();
    assert_eq!(recto_platform::local::attribute(&copy, "com.apple.quarantine").as_deref(), Some(&b"0083;5f000000;Safari;"[..]), "a copy of a download is still one");
    assert_eq!(recto_platform::local::attribute(&copy, "com.apple.metadata:_kMDItemUserTags"), None, "the original's tags are its own");
    save::save(&request(&path, 1, &order, &[]), Target::Same, None, &mut save::NoHooks).unwrap();
    assert_eq!(recto_platform::local::attribute(&path, "com.apple.quarantine").as_deref(), Some(&b"0083;5f000000;Safari;"[..]));
    assert_eq!(recto_platform::local::attribute(&path, "com.apple.metadata:_kMDItemUserTags").as_deref(), Some(&b"red-tag"[..]));
}

/// Failure: a save over a file with other hard links leaves those names with the old version, silently; a read-only file is replaced as readily as any other; Save As replaces a file that's there. Each is refused, with nothing written.
#[test]
fn what_cant_be_saved_over_is_refused() {
    let dir = Dir::new();
    let order = own(&[(0, 1)]);
    let linked = dir.write("linked.pdf", &marked(&["A"]));
    std::fs::hard_link(&linked, dir.0.join("other-name.pdf")).unwrap();
    assert_eq!(save::save(&request(&linked, 1, &order, &[]), Target::Same, None, &mut save::NoHooks), Err(SaveError::HardLinks));
    let locked = dir.write("locked.pdf", &marked(&["A"]));
    let mut permissions = std::fs::metadata(&locked).unwrap().permissions();
    permissions.set_readonly(true);
    std::fs::set_permissions(&locked, permissions).unwrap();
    assert_eq!(save::save(&request(&locked, 1, &order, &[]), Target::Same, None, &mut save::NoHooks), Err(SaveError::ReadOnly));
    let there = dir.write("there.pdf", b"someone's file");
    let path = dir.write("doc.pdf", &marked(&["A"]));
    assert_eq!(save::save(&request(&path, 1, &order, &[]), Target::NewFile(&there), None, &mut save::NoHooks), Err(SaveError::TargetExists));
    assert_eq!(std::fs::read(&there).unwrap(), b"someone's file");
}

/// Failure: a read-only folder stops a save halfway, leaving a temporary file or a broken one. It's refused, and the file is as it was.
#[test]
fn a_read_only_folder_is_refused_whole() {
    let dir = Dir::new();
    let folder = dir.0.join("locked");
    std::fs::create_dir(&folder).unwrap();
    let path = folder.join("doc.pdf");
    std::fs::write(&path, marked(&["A"])).unwrap();
    let before = std::fs::read(&path).unwrap();
    std::process::Command::new("/bin/chmod").arg("a-w").arg(&folder).status().unwrap();
    let order = own(&[(0, 1)]);
    let result = save::save(&request(&path, 1, &order, &[]), Target::Same, None, &mut save::NoHooks);
    std::process::Command::new("/bin/chmod").arg("u+w").arg(&folder).status().unwrap();
    assert!(matches!(result, Err(SaveError::ReadOnly | SaveError::Failed)), "{result:?}");
    assert_eq!(std::fs::read(&path).unwrap(), before);
    assert_eq!(std::fs::read_dir(&folder).unwrap().count(), 1);
}

/// Failure: the backup of an original is taken on every save, so it ends up being the last version and not the original, or isn't taken at all. One, on the first save, of the bytes as they were.
#[test]
fn the_first_save_keeps_one_backup_of_the_original() {
    let dir = Dir::new();
    let backups = dir.0.join("backups");
    let path = dir.write("doc.pdf", &marked(&["A", "B"]));
    let original = std::fs::read(&path).unwrap();
    let order = own(&[(0, 1), (1, 0)]);
    let first = save::save(&request(&path, 2, &order, &[]), Target::Same, Some(&backups), &mut save::NoHooks).unwrap();
    let kept = first.backup.expect("the first save keeps one");
    assert_eq!(std::fs::read(&kept).unwrap(), original);
    assert!(recto_platform::local::is_excluded_from_backups(&backups), "it's out of Time Machine, as the history is");
    let second = save::save(&request(&path, 2, &order, &[]), Target::Same, Some(&backups), &mut save::NoHooks).unwrap();
    assert_eq!(second.backup, None, "and only one");
    assert_eq!(std::fs::read(&kept).unwrap(), original, "still the original");
    assert_eq!(save::backup_of(&path, &backups), Some(kept));
    let copy = dir.0.join("copy.pdf");
    assert_eq!(save::save(&request(&path, 2, &order, &[]), Target::NewFile(&copy), Some(&backups), &mut save::NoHooks).unwrap().backup, None, "a Save As changes no original");
}

/// Failure: backups grow for ever. Past 30 days they go, and past 1 GiB the oldest go first.
#[test]
fn old_backups_go() {
    let dir = Dir::new();
    let folder = dir.0.join("backups");
    std::fs::create_dir(&folder).unwrap();
    let old = folder.join("original-aa.pdf");
    std::fs::write(&old, b"old").unwrap();
    let new = folder.join("original-bb.pdf");
    std::fs::write(&new, b"new").unwrap();
    let other = folder.join("not-a-backup.txt");
    std::fs::write(&other, b"left alone").unwrap();
    let later = std::time::SystemTime::now() + std::time::Duration::from_secs((save::BACKUP_DAYS + 1) * 24 * 60 * 60);
    assert_eq!(save::prune_backups(&folder, std::time::SystemTime::now()), 0);
    assert_eq!(save::prune_backups(&folder, later), 2);
    assert!(other.exists());
}

/// Failure: a hostile added page names an object over the save's 1 MiB limit, and the save reads it whole. The reader it saves from refuses it, and nothing is written.
#[test]
fn a_huge_object_in_an_added_file_is_refused() {
    let dir = Dir::new();
    let path = dir.write("doc.pdf", &marked(&["MINE"]));
    let before = std::fs::read(&path).unwrap();
    let huge = format!("[{}]", "1 ".repeat(700_000));
    let other = build(&[
        Raw::Dict("<< /Type /Catalog /Pages 2 0 R >>".into()),
        Raw::Dict("<< /Type /Pages /Kids [3 0 R] /Count 1 >>".into()),
        Raw::Dict("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Contents 4 0 R /Big 5 0 R >>".into()),
        Raw::Stream { dict: String::new(), data: text_lines(&["ADDED"]) },
        Raw::Dict(huge),
    ]);
    let other_path = dir.write("other.pdf", &other);
    let added = [AddedFile { path: &other_path, page_count: 1, stamp: Some(stamp(&other_path)) }];
    let order = [PageRef { source: 0, page: 0, quarter_turns: 0 }, PageRef { source: 1, page: 0, quarter_turns: 0 }];
    let result = save::save(&request(&path, 1, &order, &added), Target::Same, None, &mut save::NoHooks);
    // Read as nothing, as viewers read what they can't: the page is saved without it, and that's counted.
    match result {
        Ok(outcome) => assert_eq!(outcome.objects_missing, 1),
        Err(error) => assert!(matches!(error, SaveError::AddedFileUnreadable { .. }), "{error:?}"),
    }
    let _ = before;
}
