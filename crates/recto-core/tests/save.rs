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

/// Failure: pages added from another PDF bring that PDF's other pages along through a link, or another app's private `/PieceInfo` (on the corpus's USGS map, 156 MB of Illustrator data), or don't come at all. Only the pages added, and what they draw with, are copied in.
#[test]
fn added_pages_are_copied_in_without_the_rest_of_their_file() {
    let dir = Dir::new();
    let path = dir.write("doc.pdf", &marked(&["MINE"]));
    // Page 1 of the other file links to its page 2, which isn't added.
    let other = build(&[
        Raw::Dict("<< /Type /Catalog /Pages 2 0 R >>".into()),
        Raw::Dict("<< /Type /Pages /Kids [3 0 R 5 0 R] /Count 2 /Resources << /Font << /F1 7 0 R >> >> >>".into()),
        Raw::Dict("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Contents 4 0 R /Annots [8 0 R] /PieceInfo << /Illustrator << /Private (PIECEINFOMARK) >> >> >>".into()),
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
    assert!(!contains(&bytes, "PIECEINFOMARK"), "nor another app's private data, which can be a whole second copy of the artwork");
    assert!(contains(&bytes, "/Helvetica"), "the font the added page draws with is");
}

/// Failure: a delete is appended, so the deleted page's objects stay in the file for anyone to read back, in the bytes or an old revision. It's a rewrite: the deleted page's words are nowhere in the new file, plainly or in any stream, its old revisions are gone, and the kept pages are there in order with their turns.
#[test]
fn a_delete_is_a_rewrite_and_the_page_is_gone() {
    let dir = Dir::new();
    let path = dir.write("doc.pdf", &marked(&["KEEPONE", "DELETEDMARK", "KEEPTWO"]));
    // An earlier revision that turned the page to be deleted, so its old version is in the bytes twice.
    save::save(&request(&path, 3, &own(&[(0, 0), (1, 1), (2, 0)]), &[]), Target::Same, None, &mut save::NoHooks).unwrap();
    let order = own(&[(0, 0), (2, 1)]);
    let mut noted = Noted::default();
    let outcome = save::save(&request(&path, 3, &order, &[]), Target::Same, None, &mut noted).unwrap();
    assert_eq!((outcome.writer, outcome.pages_removed, outcome.pages), (Writer::Rewrite, 1, 2));
    let bytes = std::fs::read(&path).unwrap();
    assert!(!contains(&bytes, "DELETEDMARK"), "the deleted page's words are gone");
    assert_eq!(bytes.windows(9).filter(|window| window == b"startxref").count(), 1, "one section: no old revision holds it");
    assert_eq!(read_pages(&path), [("KEEPONE".into(), 0), ("KEEPTWO".into(), 90)]);
    assert_eq!(noted.steps, [SaveStep::Checking, SaveStep::Writing, SaveStep::Prepared, SaveStep::Committing]);
    assert_eq!(outcome.stamp, Some(stamp(&path)));
    assert!(outcome.document_data_kept.is_some(), "a rewrite says what it kept of the document's own");
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
    // Save As makes a new copy with every signature removed (SPEC decision 27), and the original is untouched.
    let copy = path.with_file_name("copy.pdf");
    let outcome = save::save(&request(&path, 1, &order, &[]), Target::NewFile(&copy), None, &mut save::NoHooks).unwrap();
    assert_eq!((outcome.writer, outcome.signatures_removed), (Writer::Rewrite, 1));
    assert_unsigned(&copy);
    assert_eq!(read_pages(&copy)[0].1, 90, "with the turn");
    assert_eq!(std::fs::read(&path).unwrap(), before, "the original keeps its signature");
    // A copy with nothing changed is unsigned too: an appended copy would add a revision its certification can show as a change (review round 2).
    let unchanged = own(&[(0, 0)]);
    let second = path.with_file_name("second.pdf");
    save::save(&request(&path, 1, &unchanged, &[]), Target::NewFile(&second), None, &mut save::NoHooks).unwrap();
    assert_unsigned(&second);
}

/// A copy has no signature of any kind: no field, value, drawn box, `/Perms` or `/SigFlags`, and nothing with a `/ByteRange` anywhere in its bytes.
fn assert_unsigned(path: &Path) {
    let bytes = std::fs::read(path).unwrap();
    for mark in ["/ByteRange", "/Sig ", "/Sig>", "/DocMDP", "/UR3", "/SigFlags", "/Perms", "Signature1"] {
        assert!(!contains(&bytes, mark), "{mark} is still in {}", path.display());
    }
    let facts = save::signature_facts_of(path).unwrap();
    assert!(!facts.has_persons_signature() && !facts.usage_rights, "{facts:?}");
}

/// Failure (SPEC decision 27): a delete on a signed PDF rewrites the signed file, so its signature breaks, or is refused with no way on. Save over it is refused with nothing written, and Save As makes an unsigned copy without the page, saying how many signatures went.
#[test]
fn a_delete_on_a_signed_file_is_an_unsigned_copy() {
    let dir = Dir::new();
    let bytes = build(&[
        Raw::Dict("<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [5 0 R] /SigFlags 3 >> >>".into()),
        Raw::Dict("<< /Type /Pages /Kids [3 0 R 7 0 R] /Count 2 >>".into()),
        Raw::Dict("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Contents 4 0 R /Annots [5 0 R] >>".into()),
        Raw::Stream { dict: String::new(), data: text_lines(&["SIGNEDPAGE"]) },
        Raw::Dict("<< /FT /Sig /T (Signature1) /V 6 0 R /Rect [0 0 50 20] /Subtype /Widget /P 3 0 R >>".into()),
        Raw::Dict("<< /Type /Sig /Filter /Adobe.PPKLite /SubFilter /adbe.pkcs7.detached /ByteRange [0 10 20 30] /Contents <00> >>".into()),
        Raw::Dict("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Contents 8 0 R >>".into()),
        Raw::Stream { dict: String::new(), data: text_lines(&["GOESMARK"]) },
    ]);
    let path = dir.write("signed.pdf", &bytes);
    assert!(save::signature_facts_of(&path).unwrap().signed);
    let order = own(&[(0, 0)]);
    assert_eq!(save::save(&request(&path, 2, &order, &[]), Target::Same, None, &mut save::NoHooks), Err(SaveError::SignedNeedsCopy));
    assert_eq!(std::fs::read(&path).unwrap(), bytes, "the signed original never changes");
    let copy = dir.0.join("copy.pdf");
    let outcome = save::save(&request(&path, 2, &order, &[]), Target::NewFile(&copy), None, &mut save::NoHooks).unwrap();
    assert_eq!((outcome.writer, outcome.signatures_removed, outcome.pages_removed), (Writer::Rewrite, 1, 1));
    assert_unsigned(&copy);
    assert!(!contains(&std::fs::read(&copy).unwrap(), "GOESMARK"));
    assert_eq!(read_pages(&copy), [("SIGNEDPAGE".into(), 0)]);
    assert_eq!(std::fs::read(&path).unwrap(), bytes);
    // A turn is still an addition, which keeps the signature: appended, with every byte it signs.
    let turned = own(&[(0, 1), (1, 0)]);
    assert_eq!(save::save(&request(&path, 2, &turned, &[]), Target::Same, None, &mut save::NoHooks).unwrap().writer, Writer::Append);
    assert!(std::fs::read(&path).unwrap().starts_with(&bytes));
}

/// Failure: a delete on a file whose only signature is usage rights, as IRS forms have, is refused as if a person had signed it, or keeps rights a reader then calls broken. It's rewritten in place with them removed, and says so (owner, 2026-11-08).
#[test]
fn usage_rights_alone_allow_a_rewrite_in_place() {
    let dir = Dir::new();
    let bytes = build(&[
        Raw::Dict("<< /Type /Catalog /Pages 2 0 R /Perms << /UR3 7 0 R >> >>".into()),
        Raw::Dict("<< /Type /Pages /Kids [3 0 R 5 0 R] /Count 2 >>".into()),
        Raw::Dict("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Contents 4 0 R >>".into()),
        Raw::Stream { dict: String::new(), data: text_lines(&["FORMPAGE"]) },
        Raw::Dict("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Contents 6 0 R >>".into()),
        Raw::Stream { dict: String::new(), data: text_lines(&["INSTRUCTIONSMARK"]) },
        Raw::Dict("<< /Type /Sig /Filter /Adobe.PPKLite /SubFilter /adbe.pkcs7.detached /ByteRange [0 10 20 30] /Contents <00> /Reference [<< /TransformMethod /UR3 >>] >>".into()),
    ]);
    let path = dir.write("irs.pdf", &bytes);
    let order = own(&[(0, 0)]);
    let outcome = save::save(&request(&path, 2, &order, &[]), Target::Same, None, &mut save::NoHooks).unwrap();
    assert_eq!((outcome.writer, outcome.usage_rights_removed, outcome.signatures_removed), (Writer::Rewrite, true, 0));
    assert_unsigned(&path);
    assert!(!contains(&std::fs::read(&path).unwrap(), "INSTRUCTIONSMARK"));
}

/// Failure: moves, turns, pages added from another PDF and a delete in one save come out in the wrong order, lose the added page, or keep the deleted one. All in one rewrite.
#[test]
fn a_delete_with_moves_turns_and_added_pages_is_one_rewrite() {
    let dir = Dir::new();
    let path = dir.write("doc.pdf", &marked(&["AAA", "DELETEDMARK", "CCC"]));
    let other = dir.write("other.pdf", &marked(&["ADDEDPAGE"]));
    let added = [AddedFile { path: &other, page_count: 1, stamp: Some(stamp(&other)) }];
    let order = [PageRef { source: 0, page: 2, quarter_turns: 1 }, PageRef { source: 1, page: 0, quarter_turns: 0 }, PageRef { source: 0, page: 0, quarter_turns: 0 }];
    let outcome = save::save(&request(&path, 3, &order, &added), Target::Same, None, &mut save::NoHooks).unwrap();
    assert_eq!((outcome.writer, outcome.pages_removed), (Writer::Rewrite, 1));
    assert_eq!(read_pages(&path), [("CCC".into(), 90), ("ADDEDPAGE".into(), 0), ("AAA".into(), 0)]);
    assert!(!contains(&std::fs::read(&path).unwrap(), "DELETEDMARK"));
}

/// Failure (owner, 2026-11-07): undoing a delete past a Save needs a copy of the whole file, so a big PDF can't, or the copy holds pages nobody can bring back. The copy holds the pages asked for, in order, and nothing of the others.
#[test]
fn a_kept_copy_holds_only_the_pages_asked_for() {
    let dir = Dir::new();
    let path = dir.write("doc.pdf", &marked(&["STAYS1", "GONE1", "STAYS2", "GONE2"]));
    let kept = dir.0.join("kept.pdf");
    let stamp_kept = save::keep_pages(&path, &[1, 3], &kept, u64::MAX).unwrap();
    assert_eq!(stamp_kept, stamp(&kept));
    assert_eq!(read_pages(&kept), [("GONE1".into(), 0), ("GONE2".into(), 0)]);
    let bytes = std::fs::read(&kept).unwrap();
    assert!(!contains(&bytes, "STAYS1") && !contains(&bytes, "STAYS2"));
    assert!(save::keep_pages(&path, &[3, 1], &dir.0.join("other.pdf"), u64::MAX).is_err(), "in order");
    assert!(save::keep_pages(&path, &[9], &dir.0.join("other.pdf"), u64::MAX).is_err(), "pages it has");
    assert!(save::keep_pages(&path, &[1], &kept, u64::MAX).is_err(), "never over a file that's there");
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

/// Failure: a save over a file loses its Finder tags, an attribute longer than 4 KiB, or, worse, the quarantine flag of a download, which Gatekeeper goes by. A Save As keeps where it came from, and nothing that's about the original alone.
#[test]
fn a_saved_file_keeps_its_attributes() {
    let dir = Dir::new();
    let path = dir.write("download.pdf", &marked(&["A"]));
    recto_platform::local::set_attribute(&path, "com.apple.quarantine", b"0083;5f000000;Safari;").unwrap();
    recto_platform::local::set_attribute(&path, "com.apple.metadata:_kMDItemUserTags", b"red-tag").unwrap();
    let long = vec![b'q'; 10_000];
    recto_platform::local::set_attribute(&path, "com.example.long", &long).unwrap();
    let order = own(&[(0, 1)]);
    let copy = dir.0.join("copy.pdf");
    save::save(&request(&path, 1, &order, &[]), Target::NewFile(&copy), None, &mut save::NoHooks).unwrap();
    assert_eq!(recto_platform::local::attribute(&copy, "com.apple.quarantine").as_deref(), Some(&b"0083;5f000000;Safari;"[..]), "a copy of a download is still one");
    assert_eq!(recto_platform::local::attribute(&copy, "com.apple.metadata:_kMDItemUserTags"), None, "the original's tags are its own");
    save::save(&request(&path, 1, &order, &[]), Target::Same, None, &mut save::NoHooks).unwrap();
    assert_eq!(recto_platform::local::attribute(&path, "com.apple.quarantine").as_deref(), Some(&b"0083;5f000000;Safari;"[..]));
    assert_eq!(recto_platform::local::attribute(&path, "com.apple.metadata:_kMDItemUserTags").as_deref(), Some(&b"red-tag"[..]));
    assert_eq!(recto_platform::local::attribute(&path, "com.example.long"), Some(long), "one longer than the 4 KiB the first version read is carried whole (review)");
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

/// Failure: a hostile added page names an object over the save's 1 MiB limit, and the save reads it whole, or quietly leaves it out and changes the page. The reader it saves from refuses it, and the save is refused with nothing written.
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
    assert!(matches!(result, Err(SaveError::AddedFileUnreadable { .. })), "{result:?}");
    assert_eq!(std::fs::read(&path).unwrap(), before);
}

/// Failure (review): an inherited `/Resources` written in place on the root of the tree is copied into every page when one is moved, so 200,000 pages under a 1 MiB dictionary append 200 GB. It goes in once, as an object every page names.
#[test]
fn an_inherited_value_written_in_place_goes_in_once() {
    let dir = Dir::new();
    let pages = 200;
    let padding = format!("/Pad ({})", "x".repeat(50_000));
    let mut objects = vec![
        Raw::Dict("<< /Type /Catalog /Pages 2 0 R >>".into()),
        Raw::Dict(format!("<< /Type /Pages /Kids [{}] /Count {pages} /MediaBox [0 0 100 100] /Resources << /Font << /F1 3 0 R >> {padding} >> >>", (0..pages).map(|at| format!("{} 0 R", 4 + at * 2)).collect::<Vec<_>>().join(" "))),
        Raw::Dict("<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".into()),
    ];
    for at in 0..pages {
        objects.push(Raw::Dict(format!("<< /Type /Page /Parent 2 0 R /Contents {} 0 R >>", 5 + at * 2)));
        objects.push(Raw::Stream { dict: String::new(), data: text_lines(&["P"]) });
    }
    let path = dir.write("doc.pdf", &build(&objects));
    let before = std::fs::metadata(&path).unwrap().len();
    let mut order = own(&(0..pages as u32).map(|page| (page, 0)).collect::<Vec<_>>());
    order.swap(0, 1);
    save::save(&request(&path, pages as u32, &order, &[]), Target::Same, None, &mut save::NoHooks).unwrap();
    let appended = std::fs::metadata(&path).unwrap().len() - before;
    assert!(appended < 2 * 50_000 + pages as u64 * 300, "{appended} bytes appended for one move: the inherited dictionary went in more than once");
}

/// Failure: a signed field with a `/Lock` that allows no changes has its document's pages changed, as a certification would forbid. Refused, untouched.
#[test]
fn a_signature_fields_lock_is_kept() {
    let dir = Dir::new();
    let bytes = build(&[
        Raw::Dict("<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [5 0 R] /SigFlags 3 >> >>".into()),
        Raw::Dict("<< /Type /Pages /Kids [3 0 R] /Count 1 >>".into()),
        Raw::Dict("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Contents 4 0 R >>".into()),
        Raw::Stream { dict: String::new(), data: b"BT ET".to_vec() },
        Raw::Dict("<< /FT /Sig /T (Locked) /V 6 0 R /Lock << /Type /SigFieldLock /Action /All /P 1 >> >>".into()),
        Raw::Dict("<< /Type /Sig /Filter /Adobe.PPKLite /ByteRange [0 10 20 30] /Contents <00> >>".into()),
    ]);
    let path = dir.write("locked.pdf", &bytes);
    let order = own(&[(0, 1)]);
    assert_eq!(save::save(&request(&path, 1, &order, &[]), Target::Same, None, &mut save::NoHooks), Err(SaveError::LockedBySignature));
    assert_eq!(std::fs::read(&path).unwrap(), bytes);
}

/// Failure: a Save As of a document with nothing unsaved writes the copy outside the one writer, with a fixed temporary name (review). It goes through the same atomic append, with the original's bytes first and its pages as they were.
#[test]
fn a_save_as_with_nothing_unsaved_goes_through_the_writer() {
    let dir = Dir::new();
    let path = dir.write("doc.pdf", &marked(&["ONE", "TWO"]));
    let original = std::fs::read(&path).unwrap();
    let copy = dir.0.join("copy.pdf");
    let order = own(&[(0, 0), (1, 0)]);
    save::save(&request(&path, 2, &order, &[]), Target::NewFile(&copy), None, &mut save::NoHooks).unwrap();
    assert!(std::fs::read(&copy).unwrap().starts_with(&original));
    assert_eq!(read_pages(&copy), [("ONE".into(), 0), ("TWO".into(), 0)]);
    assert_eq!(std::fs::read(&path).unwrap(), original, "the original is as it was");
}

/// Failure: a tab with no stamp skips the check that its file is still the one its edits are about, and saves over whatever is there now. It's refused.
#[test]
fn no_stamp_is_no_save() {
    let dir = Dir::new();
    let path = dir.write("doc.pdf", &marked(&["A"]));
    let order = own(&[(0, 1)]);
    let unstamped = SaveRequest { path: &path, stamp: None, file_pages: 1, pages: &order, added: &[] };
    assert_eq!(save::save(&unstamped, Target::Same, None, &mut save::NoHooks), Err(SaveError::FileChanged));
}

/// Failure (review): copying a hostile added page holds a reference for every one it names, so objects that each name all the others reach hundreds of millions, or clones every level of a deep dictionary. The copy is bounded, and refused past it, with little held.
#[test]
fn a_hostile_added_page_is_refused_within_its_bounds() {
    let count = 100;
    let refs: String = (0..count).map(|at| format!("{} 0 R ", 4 + at)).collect::<String>().repeat(20);
    let mut objects = vec![
        Raw::Dict("<< /Type /Catalog /Pages 2 0 R >>".into()),
        Raw::Dict("<< /Type /Pages /Kids [3 0 R] /Count 1 >>".into()),
        Raw::Dict("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 9 9] /Resources << /X 4 0 R >> >>".into()),
    ];
    for _ in 0..count {
        objects.push(Raw::Dict(format!("[{refs}]")));
    }
    let hostile = build(&objects);
    let target = marked(&["MINE"]);
    assert_eq!(save::plan_in_memory_within(&target, &hostile, 100_000), Err(SaveError::TooLarge), "100 objects naming 2,000 references each is past a bound of 100,000");
    assert_eq!(save::plan_in_memory_within(&target, &hostile, 1_000_000), Ok(2), "and within a larger one it copies");
    assert_eq!(save::plan_in_memory(&target, &marked(&["ADDED", "TOO"])), Ok(3), "and an ordinary one copies");
}

/// Failure: a page added from a PDF with layers loses that PDF's layer states, so a layer its own file hides, which can be what it keeps out of sight, shows in the saved file. Found on the corpus: the USGS map's hidden layers made its page need over 1 GiB to draw once added elsewhere. The layers come along, and the hidden ones stay hidden.
#[test]
fn an_added_pages_hidden_layers_stay_hidden() {
    let dir = Dir::new();
    let path = dir.write("doc.pdf", &marked(&["MINE"]));
    let other = build(&[
        Raw::Dict("<< /Type /Catalog /Pages 2 0 R /OCProperties << /OCGs [5 0 R 6 0 R] /D << /OFF [5 0 R] >> >> >>".into()),
        Raw::Dict("<< /Type /Pages /Kids [3 0 R] /Count 1 >>".into()),
        Raw::Dict("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Contents 4 0 R /Resources << /Properties << /H 5 0 R /S 6 0 R >> >> >>".into()),
        Raw::Stream { dict: String::new(), data: b"/OC /H BDC (HIDDENLAYER) Tj EMC /OC /S BDC (SHOWNLAYER) Tj EMC".to_vec() },
        Raw::Dict("<< /Type /OCG /Name (Hidden) >>".into()),
        Raw::Dict("<< /Type /OCG /Name (Shown) >>".into()),
    ]);
    let other_path = dir.write("other.pdf", &other);
    let added = [AddedFile { path: &other_path, page_count: 1, stamp: Some(stamp(&other_path)) }];
    let order = [PageRef { source: 0, page: 0, quarter_turns: 0 }, PageRef { source: 1, page: 0, quarter_turns: 0 }];
    save::save(&request(&path, 1, &order, &added), Target::Same, None, &mut save::NoHooks).unwrap();
    let reader = IndexedReader::open(FileSource::open(&path).unwrap()).unwrap();
    let Some(Object::Dictionary(root)) = reader.trailer_entry_owned(b"Root").unwrap() else { panic!() };
    let properties = root.get(b"OCProperties").and_then(Object::as_dict).expect("the layers came along");
    let named = |list: &Object| -> Vec<String> {
        list.as_array().unwrap().iter().map(|item| match reader.resolve_object(item.as_reference().unwrap()).unwrap() {
            Object::Dictionary(group) => String::from_utf8_lossy(group.get(b"Name").unwrap().as_str().unwrap()).into_owned(),
            _ => String::new(),
        }).collect()
    };
    assert_eq!(named(properties.get(b"OCGs").unwrap()), ["Hidden", "Shown"]);
    assert_eq!(named(properties.get(b"D").and_then(Object::as_dict).unwrap().get(b"OFF").unwrap()), ["Hidden"], "hidden there, hidden here");
}

/// Failure (review round 2): a reference to an object number the file's cross-reference data doesn't have, as real files carry, refuses the save, though ISO 32000 and every viewer read it as null. It goes as nothing, and the save says it left something out.
#[test]
fn a_dangling_reference_in_an_added_page_is_null() {
    let dir = Dir::new();
    let path = dir.write("doc.pdf", &marked(&["MINE"]));
    let other = build(&[
        Raw::Dict("<< /Type /Catalog /Pages 2 0 R >>".into()),
        Raw::Dict("<< /Type /Pages /Kids [3 0 R] /Count 1 >>".into()),
        Raw::Dict("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Contents 4 0 R /Resources << /XObject << /Gone 99 0 R >> >> >>".into()),
        Raw::Stream { dict: String::new(), data: text_lines(&["ADDED"]) },
    ]);
    let other_path = dir.write("other.pdf", &other);
    let added = [AddedFile { path: &other_path, page_count: 1, stamp: Some(stamp(&other_path)) }];
    let order = [PageRef { source: 0, page: 0, quarter_turns: 0 }, PageRef { source: 1, page: 0, quarter_turns: 0 }];
    let outcome = save::save(&request(&path, 1, &order, &added), Target::Same, None, &mut save::NoHooks).unwrap();
    assert_eq!(outcome.objects_missing, 1);
    assert_eq!(read_pages(&path)[1].0, "ADDED");
}

/// A form like the IRS's: one top field holding a text field whose widgets are on both pages.
fn irs_like(marker: &str, value: &str) -> Vec<u8> {
    build(&[
        Raw::Dict("<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [8 0 R] /DR << /Font << /HeBo 11 0 R >> >> /DA (/HeBo 9 Tf 0 g) >> >>".into()),
        Raw::Dict("<< /Type /Pages /Kids [3 0 R 5 0 R] /Count 2 >>".into()),
        Raw::Dict("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Contents 4 0 R /Annots [9 0 R] >>".into()),
        Raw::Stream { dict: String::new(), data: text_lines(&[marker]) },
        Raw::Dict("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Contents 6 0 R /Annots [10 0 R] >>".into()),
        Raw::Stream { dict: String::new(), data: text_lines(&["SECONDPAGE"]) },
        Raw::Dict("<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".into()),
        Raw::Dict("<< /T (topmostSubform[0]) /Kids [12 0 R] >>".into()),
        Raw::Dict("<< /Type /Annot /Subtype /Widget /Rect [10 10 100 30] /P 3 0 R /Parent 12 0 R /F 4 >>".into()),
        Raw::Dict("<< /Type /Annot /Subtype /Widget /Rect [10 10 100 30] /P 5 0 R /Parent 12 0 R /F 4 /OTHERPAGEWIDGET true >>".into()),
        Raw::Dict("<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica-Bold >>".into()),
        Raw::Dict(format!("<< /T (f1_01[0]) /FT /Tx /V ({value}) /Parent 8 0 R /Kids [9 0 R 10 0 R] >>")),
    ])
}

/// Failure (save review round 1, 9 of 139 corpus saves): pages added from a PDF with a form keep their drawn fields, which can't be filled in the saved file, as qpdf says "widget annotation is not reachable from /AcroForm". The widget keeps its field and the field's parent, the top field joins the form's `/Fields`, renamed as its name's taken, the field's widgets on pages not added stay behind, and the form's fonts come along.
#[test]
fn an_added_pages_form_fields_can_be_filled() {
    let dir = Dir::new();
    let path = dir.write("f1040.pdf", &irs_like("TARGETFORM", "mine"));
    let other = dir.write("f1040s1.pdf", &irs_like("ADDEDFORM", "theirs"));
    let added = [AddedFile { path: &other, page_count: 2, stamp: Some(stamp(&other)) }];
    let order = [PageRef { source: 0, page: 0, quarter_turns: 0 }, PageRef { source: 0, page: 1, quarter_turns: 0 }, PageRef { source: 1, page: 0, quarter_turns: 0 }];
    save::save(&request(&path, 2, &order, &added), Target::Same, None, &mut save::NoHooks).unwrap();
    let reader = IndexedReader::open(FileSource::open(&path).unwrap()).unwrap();
    let Some(Object::Dictionary(catalog)) = reader.trailer_entry_owned(b"Root").unwrap() else { panic!() };
    let form = match catalog.get(b"AcroForm").unwrap() {
        Object::Dictionary(form) => form.clone(),
        Object::Reference(id) => reader.resolve_object(*id).unwrap().as_dict().unwrap().clone(),
        _ => panic!(),
    };
    let fields = form.get(b"Fields").unwrap().as_array().unwrap();
    assert_eq!(fields.len(), 2, "the added page's top field joins the form");
    let names: Vec<Vec<u8>> = fields.iter().map(|field| reader.resolve_object(field.as_reference().unwrap()).unwrap().as_dict().unwrap().get(b"T").unwrap().as_str().unwrap().to_vec()).collect();
    assert_eq!(names, [b"topmostSubform[0]".to_vec(), b"topmostSubform[0]_2".to_vec()], "renamed, so the two forms' values stay apart");
    // The added widget reaches its field, which reaches the new top field.
    let page = reader.page_map().unwrap().get(2).unwrap().id();
    let page = reader.resolve_object(page).unwrap();
    let widget = page.as_dict().unwrap().get(b"Annots").unwrap().as_array().unwrap()[0].as_reference().unwrap();
    let widget = reader.resolve_object(widget).unwrap();
    let field = reader.resolve_object(widget.as_dict().unwrap().get(b"Parent").unwrap().as_reference().unwrap()).unwrap();
    assert_eq!(field.as_dict().unwrap().get(b"V").unwrap().as_str().unwrap(), b"theirs");
    assert_eq!(field.as_dict().unwrap().get(b"Kids").unwrap().as_array().unwrap().len(), 1, "its widget on the page that wasn't added stays behind");
    let bytes = std::fs::read(&path).unwrap();
    assert_eq!(bytes.windows(15).filter(|window| window == b"OTHERPAGEWIDGET").count(), 1, "the document's own, and not the added file's");
    let fonts = match form.get(b"DR").unwrap() {
        Object::Dictionary(dr) => dr.get(b"Font").unwrap().as_dict().unwrap().clone(),
        _ => panic!(),
    };
    assert!(fonts.has(b"HeBo"));
    // And qpdf, where it's installed, finds every widget reachable.
    if let Ok(out) = std::process::Command::new("qpdf").arg("--check").arg(&path).output() {
        let said = String::from_utf8_lossy(&out.stdout).to_string() + &String::from_utf8_lossy(&out.stderr);
        assert!(!said.contains("not reachable from /AcroForm"), "{said}");
    }
}

/// Failure (save review round 1, on the corpus): IRS forms added to IRS forms come out with widgets qpdf calls "not reachable from /AcroForm", so their fields can't be filled. Each IRS form in `RECTO_BENCH100`, with the next one's pages added, saved as a copy: qpdf finds every widget reachable, and the copy has every top field of both. Skipped without the corpus or qpdf.
#[test]
fn irs_forms_added_to_irs_forms_stay_fillable() {
    let Some(folder) = std::env::var_os("RECTO_BENCH100") else { return };
    if std::process::Command::new("qpdf").arg("--version").output().is_err() {
        return;
    }
    let mut forms: Vec<PathBuf> = std::fs::read_dir(&folder).unwrap().flatten().map(|entry| entry.path()).filter(|path| path.file_name().is_some_and(|name| name.to_string_lossy().starts_with("forms_irs_"))).collect();
    forms.sort();
    assert!(forms.len() >= 2, "the corpus has its IRS forms");
    let dir = Dir::new();
    for pair in forms.windows(2) {
        let (target, other) = (&pair[0], &pair[1]);
        let pages_of = |path: &Path| IndexedReader::open(FileSource::open(path).unwrap()).unwrap().page_map().unwrap().len() as u32;
        let (mine, theirs) = (pages_of(target), pages_of(other));
        let added = [AddedFile { path: other, page_count: theirs, stamp: Some(stamp(other)) }];
        let mut order: Vec<PageRef> = (0..mine).map(|page| PageRef { source: 0, page, quarter_turns: 0 }).collect();
        order.extend((0..theirs).map(|page| PageRef { source: 1, page, quarter_turns: 0 }));
        let copy = dir.0.join(format!("{}-plus.pdf", target.file_stem().unwrap().to_string_lossy()));
        let outcome = save::save(&request(target, mine, &order, &added), Target::NewFile(&copy), None, &mut save::NoHooks);
        let outcome = outcome.unwrap_or_else(|error| panic!("{}: {error:?}", target.display()));
        assert_eq!(outcome.pages, mine + theirs);
        let out = std::process::Command::new("qpdf").arg("--check").arg(&copy).output().unwrap();
        let said = String::from_utf8_lossy(&out.stdout).to_string() + &String::from_utf8_lossy(&out.stderr);
        assert!(!said.contains("not reachable from /AcroForm"), "{} + {}: {said}", target.display(), other.display());
        eprintln!("{} + {}: qpdf exit {:?}", target.file_name().unwrap().to_string_lossy(), other.file_name().unwrap().to_string_lossy(), out.status.code());
    }
}

/// Failure: the fuzzer's way into the rewrite plans a different save from a real one, or what it writes doesn't pass the fork's verify. On ordinary files it deletes, adds and reads back, and refuses a delete of the only page.
#[test]
fn the_rewrite_in_memory_is_the_real_plan() {
    let target = marked(&["GONE", "STAYS", "ALSO"]);
    let added = marked(&["ADDED"]);
    assert_eq!(save::rewrite_in_memory(&target, &added, false), Ok(2));
    assert_eq!(save::rewrite_in_memory(&target, &added, true), Ok(3));
    assert_eq!(save::rewrite_in_memory(&marked(&["ONLY"]), &added, false), Err(SaveError::NoPagesLeft));
}

/// Failure (review round 1 of stage 2): a page added from a signed document brings its signature widget and value along, outside the form, so this document then reads as signed by its bytes: a later delete is refused as if a person had signed it, though nothing showed Save off, and a Save As "unsigned copy" keeps a `/ByteRange`. The widget and its value stay behind, the page's own drawing comes, and saving, a delete and a Save As all go as for an unsigned document.
#[test]
fn an_added_pages_signature_stays_behind() {
    let dir = Dir::new();
    let path = dir.write("doc.pdf", &marked(&["MINE", "ALSO"]));
    let signed = build(&[
        Raw::Dict("<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [5 0 R] /SigFlags 3 >> >>".into()),
        Raw::Dict("<< /Type /Pages /Kids [3 0 R] /Count 1 >>".into()),
        Raw::Dict("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Contents 4 0 R /Annots [5 0 R 7 0 R] >>".into()),
        Raw::Stream { dict: String::new(), data: text_lines(&["SIGNEDPAGEDRAWING"]) },
        Raw::Dict("<< /FT /Sig /T (Signature1) /V 6 0 R /Rect [0 0 50 20] /Subtype /Widget /P 3 0 R >>".into()),
        Raw::Dict("<< /Type /Sig /Filter /Adobe.PPKLite /SubFilter /adbe.pkcs7.detached /ByteRange [0 10 20 30] /Contents <00> >>".into()),
        Raw::Dict("<< /Type /Annot /Subtype /Text /Rect [0 0 10 10] /Contents (A NOTE THAT STAYS) >>".into()),
    ]);
    let other = dir.write("signed.pdf", &signed);
    let added = [AddedFile { path: &other, page_count: 1, stamp: Some(stamp(&other)) }];
    let order = [PageRef { source: 0, page: 0, quarter_turns: 0 }, PageRef { source: 0, page: 1, quarter_turns: 0 }, PageRef { source: 1, page: 0, quarter_turns: 0 }];
    save::save(&request(&path, 2, &order, &added), Target::Same, None, &mut save::NoHooks).unwrap();
    let bytes = std::fs::read(&path).unwrap();
    assert!(!contains(&bytes, "/ByteRange") && !contains(&bytes, "Signature1"), "no signature came along");
    assert!(contains(&bytes, "SIGNEDPAGEDRAWING") && contains(&bytes, "A NOTE THAT STAYS"), "the page and its other annotations did");
    let reader = IndexedReader::open(FileSource::open(&path).unwrap()).unwrap();
    assert!(!save::persons_signature(&reader), "the fork finds no signature in it");
    // A delete saves over it, as for any unsigned file, and a Save As copy has no signature either.
    let delete = own(&[(0, 0), (2, 0)]);
    assert_eq!(save::save(&request(&path, 3, &delete, &[]), Target::Same, None, &mut save::NoHooks).unwrap().writer, Writer::Rewrite);
    let copy = dir.0.join("copy.pdf");
    save::save(&request(&path, 2, &own(&[(0, 0)]), &[]), Target::NewFile(&copy), None, &mut save::NoHooks).unwrap();
    assert!(!contains(&std::fs::read(&copy).unwrap(), "/ByteRange"));
}

/// Failure (review round 1 of stage 2): a hostile form makes a save's work unbounded, or a rename that runs out settles on a taken name, merging the added field into the document's own and changing its value. Every top name taken, as far as renames go, is refused with nothing written; a form whose `/Fields` names past the limit is refused; a widget chain shared by thousands of widgets is looked at once each.
#[test]
fn hostile_forms_are_bounded() {
    let dir = Dir::new();
    // The target's form already has `F` and `F_2` to `F_1000`.
    let taken: String = (1..=1000).map(|n| if n == 1 { "<< /T (F) >> ".to_string() } else { format!("<< /T (F_{n}) >> ") }).collect();
    let target = build(&[
        Raw::Dict(format!("<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [{taken}] >> >>")),
        Raw::Dict("<< /Type /Pages /Kids [3 0 R] /Count 1 >>".into()),
        Raw::Dict("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Contents 4 0 R >>".into()),
        Raw::Stream { dict: String::new(), data: text_lines(&["TARGET"]) },
    ]);
    let path = dir.write("target.pdf", &target);
    let before = std::fs::read(&path).unwrap();
    // The added page: 2,000 widgets of one field named `F`.
    let widgets: String = (0..2000).map(|at| format!("{} 0 R ", 6 + at)).collect();
    let mut objects = vec![
        Raw::Dict("<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [5 0 R] >> >>".into()),
        Raw::Dict("<< /Type /Pages /Kids [3 0 R] /Count 1 >>".into()),
        Raw::Dict(format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Contents 4 0 R /Annots [{widgets}] >>")),
        Raw::Stream { dict: String::new(), data: text_lines(&["ADDED"]) },
        Raw::Dict(format!("<< /T (F) /FT /Tx /Kids [{widgets}] >>")),
    ];
    for _ in 0..2000 {
        objects.push(Raw::Dict("<< /Type /Annot /Subtype /Widget /Rect [0 0 5 5] /Parent 5 0 R >>".into()));
    }
    let other = dir.write("added.pdf", &build(&objects));
    let added = [AddedFile { path: &other, page_count: 1, stamp: Some(stamp(&other)) }];
    let order = [PageRef { source: 0, page: 0, quarter_turns: 0 }, PageRef { source: 1, page: 0, quarter_turns: 0 }];
    let started = std::time::Instant::now();
    assert_eq!(save::save(&request(&path, 1, &order, &added), Target::Same, None, &mut save::NoHooks), Err(SaveError::FieldNamesTaken));
    assert!(started.elapsed() < std::time::Duration::from_secs(10), "{:?}", started.elapsed());
    assert_eq!(std::fs::read(&path).unwrap(), before, "nothing was written");
    // With one name free, it goes in, its 2,000 widgets under one field.
    let mine = dir.write("free.pdf", &marked(&["FREE"]));
    let outcome = save::save(&request(&mine, 1, &order, &added), Target::Same, None, &mut save::NoHooks).unwrap();
    assert_eq!(outcome.pages, 2);
    // A form listing more fields than a save looks at is refused, not walked.
    let many: String = (0..100_001).map(|_| "4 0 R ").collect();
    let huge = build(&[
        Raw::Dict(format!("<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [{many}] >> >>")),
        Raw::Dict("<< /Type /Pages /Kids [3 0 R] /Count 1 >>".into()),
        Raw::Dict("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] >>".into()),
        Raw::Dict("<< /T (X) >>".into()),
    ]);
    let huge_path = dir.write("huge.pdf", &huge);
    // Refused before it's walked: as too much, or, by the signature look's own bound, as possibly locked, on the side of refusing.
    assert!(matches!(save::save(&request(&huge_path, 1, &order, &added), Target::Same, None, &mut save::NoHooks), Err(SaveError::TooLarge | SaveError::LockedBySignature)));
}

/// Failure (review round 1 of stage 2): a page object the tree lists twice is one page to the rewrite, so a copy kept for "pages 0 and 1" holds one page, and undo past the save puts the wrong pages back. A copy with fewer pages than asked for isn't used, and one estimated past the limit isn't written.
#[test]
fn a_kept_copy_that_isnt_what_was_asked_isnt_used() {
    let dir = Dir::new();
    let twice = build(&[
        Raw::Dict("<< /Type /Catalog /Pages 2 0 R >>".into()),
        Raw::Dict("<< /Type /Pages /Kids [3 0 R 3 0 R 5 0 R] /Count 3 >>".into()),
        Raw::Dict("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Contents 4 0 R >>".into()),
        Raw::Stream { dict: String::new(), data: text_lines(&["TWICE"]) },
        Raw::Dict("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Contents 6 0 R >>".into()),
        Raw::Stream { dict: String::new(), data: text_lines(&["ONCE"]) },
    ]);
    let path = dir.write("twice.pdf", &twice);
    let kept = dir.0.join("kept.keptpages");
    // Page 1 is page 0's object again: keeping page 0 alone removes page 1, which takes that object from both places.
    assert!(save::keep_pages(&path, &[0], &kept, u64::MAX).is_err());
    assert!(!kept.exists(), "and it isn't left there");
    assert_eq!(save::keep_pages(&path, &[0, 1], &kept, 1), Err(SaveError::TooLarge), "past the limit, refused before writing");
    assert!(!kept.exists());
}

/// Failure (review round 1 of stage 2): a blank signature field, the place a person is to sign, goes when usage rights are removed in place or a copy is made unsigned, though nobody signed it. It stays, with the form's `SignaturesExist`. Needs the fork's `work/blank-signature-fields-stay` (`1336343`): until RectoPDF is pinned to it, this is ignored.
#[test]
#[ignore = "needs the lopdf fork's 1336343, not yet pushed"]
fn a_blank_signature_field_stays_through_a_rewrite() {
    let dir = Dir::new();
    let bytes = build(&[
        Raw::Dict("<< /Type /Catalog /Pages 2 0 R /Perms << /UR3 7 0 R >> /AcroForm << /Fields [5 0 R] /SigFlags 3 >> >>".into()),
        Raw::Dict("<< /Type /Pages /Kids [3 0 R 6 0 R] /Count 2 >>".into()),
        Raw::Dict("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Contents 4 0 R /Annots [5 0 R] >>".into()),
        Raw::Stream { dict: String::new(), data: text_lines(&["FORM"]) },
        Raw::Dict("<< /Type /Annot /Subtype /Widget /FT /Sig /T (SIGNHERE) /Rect [0 0 90 20] /P 3 0 R >>".into()),
        Raw::Dict("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] >>".into()),
        Raw::Dict("<< /Type /Sig /Filter /Adobe.PPKLite /SubFilter /adbe.pkcs7.detached /ByteRange [0 10 20 30] /Contents <00> /Reference [<< /TransformMethod /UR3 >>] >>".into()),
    ]);
    let path = dir.write("irs-blank.pdf", &bytes);
    let outcome = save::save(&request(&path, 2, &own(&[(0, 0)]), &[]), Target::Same, None, &mut save::NoHooks).unwrap();
    assert!(outcome.usage_rights_removed);
    let after = std::fs::read(&path).unwrap();
    assert!(contains(&after, "SIGNHERE") && contains(&after, "/SigFlags 1") && !contains(&after, "/UR3"), "the blank field stays");
}
