//! The page digest `recto-page/1` (`recto_core::page_id`) through what is done to files: our own Save (a turn appended, a page deleted by a rewrite, a
//! highlight added) and, where they're on this machine, the real corpus and qpdf's rewrite. Each test names the failure it catches.

use recto_core::page_id::{self, Caps, PageIdentity};
use recto_core::save::{self, NewHighlight, PageRef, SaveRequest, Target};
use recto_testpdf::{Object as Raw, Page, TempDir, build, pages_with, text_lines};
use std::path::{Path, PathBuf};

/// The file as it is now, opened as the app opens one for its pages: for this version, checked by what's opened.
fn open(path: &Path) -> page_id::IndexedReader {
    let stamp = recto_platform::file_stamp(path).expect("its version");
    page_id::open_version_for_identity(path, stamp).expect("it opens")
}

fn identities(path: &Path) -> Vec<PageIdentity> {
    let reader = open(path);
    let map = reader.page_map().expect("its pages");
    let pages: Vec<u32> = (0..map.len() as u32).collect();
    page_id::identify_pages(&reader, &map, &pages)
        .into_iter()
        .map(|id| id.expect("a page it has"))
        .collect()
}

/// Every page's digest, `None` for one with no identity (past a cap).
fn digests(path: &Path) -> Vec<Option<String>> {
    let reader = open(path);
    let map = reader.page_map().expect("its pages");
    let pages: Vec<u32> = (0..map.len() as u32).collect();
    page_id::identify_pages(&reader, &map, &pages)
        .into_iter()
        .map(|id| id.map(|id| hex(&id.digest)))
        .collect()
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// Four pages: words on each, and an image on the third.
fn document(dir: &Path) -> PathBuf {
    let pages: Vec<Page> = (1..=4).map(|page| Page::new(text_lines(&[&format!("Page {page} of the document")]))).collect();
    let image = Raw::Stream {
        dict: "/Type /XObject /Subtype /Image /Width 1 /Height 1 /BitsPerComponent 8 /ColorSpace /DeviceGray /Filter /DCTDecode".into(),
        data: b"\xff\xd8 pixels \xff\xd9".to_vec(),
    };
    let first = recto_testpdf::first_extra_object(4);
    let path = dir.join("doc.pdf");
    std::fs::write(&path, pages_with(&pages, &format!("/XObject << /Im1 {first} 0 R >>"), vec![image])).unwrap();
    path
}

fn own(order: &[(u32, u8)]) -> Vec<PageRef> {
    order
        .iter()
        .map(|(page, quarter_turns)| PageRef {
            source: 0,
            page: *page,
            quarter_turns: *quarter_turns,
        })
        .collect()
}

fn saved(path: &Path, file_pages: u32, pages: &[PageRef], highlights: &[NewHighlight<'_>]) {
    let request = SaveRequest {
        path,
        stamp: Some(recto_platform::file_stamp(path).unwrap()),
        file_pages,
        pages,
        added: &[],
        highlights,
        highlights_gone: &[],
    };
    save::save(&request, Target::Same, None, &mut save::NoHooks).expect("it saves");
}

/// Failure caught: our own Save changes the identity of the pages it keeps, so the person's next edits, recorded against the saved file, would be set
/// aside the first time another program touched it. A turn (appended; the page keeps its object number too, so the matcher's fast try holds), a
/// highlight added (appended), and a page deleted (a rewrite, which may renumber): the digests of the pages kept are what they were.
#[test]
fn our_own_saves_keep_the_identity_of_every_page_kept() {
    let dir = TempDir::new("recto-page-id-");
    let path = document(&dir);
    let before = identities(&path);
    assert_eq!(before.len(), 4);
    assert_ne!(before[0].digest, before[1].digest, "pages saying other words are other pages");

    saved(&path, 4, &own(&[(0, 0), (1, 1), (2, 0), (3, 0)]), &[]);
    let turned = identities(&path);
    for (at, (was, now)) in before.iter().zip(&turned).enumerate() {
        assert_eq!(hex(&was.digest), hex(&now.digest), "page {at} after a turn");
        assert_eq!(was.object, now.object, "page {at}'s number after an appended turn");
    }
    assert_ne!(before[1].frame, turned[1].frame, "the turn shows in the frame, not in the digest");

    saved(
        &path,
        4,
        &own(&[(0, 0), (1, 0), (2, 0), (3, 0)]),
        &[NewHighlight {
            position: 2,
            rects: &[[72.0, 700.0, 200.0, 714.0]],
        }],
    );
    let highlighted = identities(&path);
    assert_eq!(hex(&highlighted[2].digest), hex(&before[2].digest), "a highlight added");

    saved(&path, 4, &own(&[(1, 0), (2, 0), (3, 0)]), &[]);
    let rewritten = identities(&path);
    assert_eq!(rewritten.len(), 3);
    for (at, now) in rewritten.iter().enumerate() {
        assert_eq!(hex(&now.digest), hex(&before[at + 1].digest), "page {} after page 1 was deleted", at + 2);
    }
}

const BENCH: &str = "/Volumes/EksternalHome/Koding/Rust/distillPDF/benchmarking/bench100";

/// Small files of the corpus, of several makers, for the round trips below.
const CORPUS_SAMPLE: &[&str] = &[
    "forms_irs_f1040.pdf",
    "standards_ietf_rfc9457.pdf",
    "policy_worldbank_wbD34466295.pdf",
    "math_AG_2607.28368.pdf",
    "cs_CL_2607.28576.pdf",
    "eu_law_eu_aiact.pdf",
];

fn corpus() -> Option<PathBuf> {
    let bench = PathBuf::from(BENCH);
    if bench.is_dir() {
        Some(bench)
    } else {
        println!("no corpus at {BENCH}: skipped");
        None
    }
}

/// **Golden vectors on real files.** Failure caught: a change to `recto-page/1` that the made pages of the unit tests don't exercise (real fonts,
/// predictors, forms, object streams), which would make every digest an older build kept stop matching. Page 1 of three corpus files, by the first 16
/// bytes (what a sidecar keeps). Only where the corpus is on the machine.
#[test]
fn golden_vectors_on_real_files() {
    let Some(bench) = corpus() else { return };
    let golden = [
        ("eu_law_eu_aiact.pdf", "7a817f0126c6be91f49bac6b85847016"),
        ("cs_CV_2607.28611.pdf", "f2782d8acb05bd6fb4e96e7d7684bbd1"),
        ("gov_nasa_ntrs20220005969.pdf", "aa3f093bba9db27ce0829f7fa2917914"),
    ];
    let mut got = Vec::new();
    for (name, _) in golden {
        let reader = open(&bench.join(name));
        let map = reader.page_map().unwrap();
        got.push(hex(&page_id::identify_pages(&reader, &map, &[0])[0].unwrap().digest[..16]));
    }
    let want: Vec<String> = golden.iter().map(|(_, want)| (*want).to_string()).collect();
    assert_eq!(got, want, "{:?}", golden.map(|(name, _)| name));
}

/// Failure caught (research §3, data table C): a re-save that keeps every page as it was changes their digests, so every edit would be set aside the
/// first time another program re-saved the file. Every page of a sample of the corpus, through qpdf's rewrite with object streams (which renumbers
/// almost every page) where qpdf is on the machine, and through our own turn and save, keeps its digest. Within each file, pages that share a digest
/// are the same blank or separator page, as the research found (24 of 5,872 in the whole corpus).
#[test]
fn real_pages_keep_their_identity_through_a_rewrite_and_our_save() {
    let Some(bench) = corpus() else { return };
    let dir = TempDir::new("recto-page-id-corpus-");
    let qpdf = Path::new("/opt/homebrew/bin/qpdf");
    for name in CORPUS_SAMPLE {
        let path = dir.join(name);
        std::fs::copy(bench.join(name), &path).unwrap();
        let before = digests(&path);
        if qpdf.exists() {
            let out = dir.join(format!("qpdf-{name}"));
            let status = std::process::Command::new(qpdf)
                .args(["--object-streams=generate"])
                .arg(&path)
                .arg(&out)
                .status()
                .unwrap();
            assert!(status.success() || status.code() == Some(3), "qpdf on {name}: {status}");
            let rewritten = digests(&out);
            assert_eq!(rewritten.len(), before.len());
            for (at, (was, now)) in before.iter().zip(&rewritten).enumerate() {
                assert_eq!(was, now, "{name} page {} through qpdf", at + 1);
            }
        }
        let count = before.len() as u32;
        let order: Vec<(u32, u8)> = (0..count).map(|page| (page, u8::from(page == 0))).collect();
        saved(&path, count, &own(&order), &[]);
        let after = digests(&path);
        for (at, (was, now)) in before.iter().zip(&after).enumerate() {
            assert_eq!(was, now, "{name} page {} through our turn and save", at + 1);
        }
    }
}

/// What a page holds at once, in live heap bytes counted on the reading thread (the process footprint can't tell held from freed here).
#[allow(unsafe_code)]
mod held {
    use std::alloc::{GlobalAlloc, Layout, System};
    use std::cell::Cell;

    struct Counting;

    thread_local! {
        static ON: Cell<bool> = const { Cell::new(false) };
        static LIVE: Cell<i64> = const { Cell::new(0) };
        static PEAK: Cell<i64> = const { Cell::new(0) };
    }

    fn grew(by: i64) {
        // A thread on its way out has no counter any more.
        let _ = ON.try_with(|on| {
            if on.get() {
                let live = LIVE.with(|live| {
                    live.set(live.get() + by);
                    live.get()
                });
                PEAK.with(|peak| peak.set(peak.get().max(live)));
            }
        });
    }

    // SAFETY: forwards to the system allocator, counting live bytes on the thread that makes them while counting is on.
    unsafe impl GlobalAlloc for Counting {
        unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
            grew(layout.size() as i64);
            // SAFETY: the caller's layout, passed on.
            unsafe { System.alloc(layout) }
        }

        unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
            grew(layout.size() as i64);
            // SAFETY: the caller's layout, passed on.
            unsafe { System.alloc_zeroed(layout) }
        }

        unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
            grew(new_size as i64 - layout.size() as i64);
            // SAFETY: the caller's block and sizes, passed on.
            unsafe { System.realloc(ptr, layout, new_size) }
        }

        unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
            grew(-(layout.size() as i64));
            // SAFETY: given back as it was given.
            unsafe { System.dealloc(ptr, layout) }
        }
    }

    #[global_allocator]
    static COUNTING: Counting = Counting;

    /// The most live bytes `work` held at once on this thread, past what it held when it started.
    pub fn peak_of(work: impl FnOnce()) -> i64 {
        LIVE.with(|live| live.set(0));
        PEAK.with(|peak| peak.set(0));
        ON.with(|on| on.set(true));
        work();
        ON.with(|on| on.set(false));
        PEAK.with(Cell::get)
    }
}

/// The most live bytes reading page 0 held at once.
fn held_reading(bytes: &[u8], caps: &Caps) -> i64 {
    let reader = page_id::open_bytes_for_identity(std::sync::Arc::from(bytes)).expect("it opens");
    let map = reader.page_map().expect("its pages");
    let entry = map.get(0).expect("a page");
    let mut identity = None;
    let peak = held::peak_of(|| identity = page_id::read_page(&reader, entry, caps).identity);
    assert!(identity.is_some(), "an identity");
    peak
}

const JPEG: &str = "/Subtype /Image /Width 1 /Height 1 /BitsPerComponent 8 /ColorSpace /DeviceGray /Filter /DCTDecode";

/// A dictionary entry big enough to be a parsed object worth counting.
fn junk() -> String {
    format!("/Junk [{}]", "0 ".repeat(20_000))
}

/// A page drawing `n` forms, each drawing `/K`, a small image, looked up in its own resources as `shape` writes them (object 6 + 2i is the form, 7 + 2i its
/// other object).
fn forms_page(n: usize, shape: &str) -> Vec<u8> {
    let names: String = (0..n).map(|i| format!("/F{i} {} 0 R ", 6 + 2 * i)).collect();
    let draws: String = (0..n).map(|i| format!("/F{i} Do ")).collect();
    let mut all = vec![
        Raw::Dict("<< /Type /Catalog /Pages 2 0 R >>".into()),
        Raw::Dict("<< /Type /Pages /Kids [3 0 R] /Count 1 >>".into()),
        Raw::Dict(format!(
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 1 1] /Contents 4 0 R /Resources << /XObject << {names} >> >> >>"
        )),
        Raw::Stream {
            dict: String::new(),
            data: draws.into_bytes(),
        },
        Raw::Stream {
            dict: JPEG.into(),
            data: b"\xff\xd8\xff\xd9".to_vec(),
        },
    ];
    for i in 0..n {
        let other = 7 + 2 * i;
        let (resources, object) = match shape {
            "own resources" => (format!("{other} 0 R"), format!("<< /XObject << /K 5 0 R >> {} >>", junk())),
            "own table" => (format!("<< /XObject {other} 0 R >>"), format!("<< /K 5 0 R {} >>", junk())),
            "inline resources" => (format!("<< /XObject << /K 5 0 R >> {} >>", junk()), "null".into()),
            _ => unreachable!(),
        };
        all.push(Raw::Stream {
            dict: format!("/Subtype /Form /BBox [0 0 1 1] /Resources {resources}"),
            data: b"/K Do".to_vec(),
        });
        all.push(Raw::Dict(object));
    }
    build(&all)
}

/// A page drawing `n` images whose decode fails, each under two names, each image's dictionary holding junk.
fn gave_up_page(n: usize) -> Vec<u8> {
    let names: String = (0..n)
        .flat_map(|i| [format!("/A{i} {} 0 R ", 5 + i), format!("/B{i} {} 0 R ", 5 + i)])
        .collect();
    let draws: String = (0..n).map(|i| format!("/A{i} Do /B{i} Do ")).collect();
    let mut all = vec![
        Raw::Dict("<< /Type /Catalog /Pages 2 0 R >>".into()),
        Raw::Dict("<< /Type /Pages /Kids [3 0 R] /Count 1 >>".into()),
        Raw::Dict(format!(
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 1 1] /Contents 4 0 R /Resources << /XObject << {names} >> >> >>"
        )),
        Raw::Stream {
            dict: String::new(),
            data: draws.into_bytes(),
        },
    ];
    for _ in 0..n {
        all.push(Raw::Stream {
            dict: format!("/Subtype /Image /Filter /FlateDecode {}", junk()),
            data: b"not flate".to_vec(),
        });
    }
    build(&all)
}

/// A page keeps what each form it draws resolved (its `/Resources`, its `/XObject` table, a gave-up image's descriptor) in whole, for the whole page, and
/// a file of a few hundred KB draws a hundred forms that each own a megabyte-scale `/Resources`: tens of GiB held at once where one at a time is a few
/// MiB. Eight such forms must hold under twice what one holds.
#[test]
fn a_page_holds_one_resolved_object_at_a_time() {
    for shape in ["own resources", "own table", "inline resources"] {
        let (one, eight) = (
            held_reading(&forms_page(1, shape), &Caps::STANDARD),
            held_reading(&forms_page(8, shape), &Caps::STANDARD),
        );
        assert!(eight < 2 * one, "{shape}: eight forms held {eight} bytes at once, one held {one}");
    }
    let small = Caps {
        xobject_bytes: 1 << 16,
        page_bytes: 1 << 16,
        ..Caps::STANDARD
    };
    let (one, eight) = (held_reading(&gave_up_page(1), &small), held_reading(&gave_up_page(8), &small));
    assert!(eight < 2 * one, "gave up: eight images held {eight} bytes at once, one held {one}");
}
