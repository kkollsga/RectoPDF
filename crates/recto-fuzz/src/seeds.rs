//! What each target starts from. Generated here, or read from the benchmark folders by path where they exist: no third-party PDF goes in this repository. Everything still works without those folders, with less to start from.

use crate::targets;
use recto_agent::table::TABLE;
use std::path::{Path, PathBuf};

/// Where the owner's real PDFs are on this Mac.
pub const PDF_FOLDERS: &[&str] = &[
    "/Volumes/EksternalHome/Koding/Rust/distillPDF/tests/fixtures_pdf",
    "/Volumes/EksternalHome/Koding/Rust/distillPDF/benchmarking/bench100",
];
/// Largest real file used as a seed. Larger ones spend the time in streams nothing here reads.
pub const MAX_SEED_BYTES: u64 = 256 * 1024;

fn pdfs_under(folder: &Path, found: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(folder) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            pdfs_under(&path, found);
        } else if path.extension().is_some_and(|extension| extension == "pdf") && entry.metadata().is_ok_and(|meta| meta.len() <= MAX_SEED_BYTES) {
            found.push(path);
        }
    }
}

/// Real PDFs, smallest first, up to a count. Empty if the folders aren't there, as on CI.
pub fn real_pdfs(most: usize) -> Vec<Vec<u8>> {
    let mut paths = Vec::new();
    for folder in PDF_FOLDERS {
        pdfs_under(Path::new(folder), &mut paths);
    }
    paths.sort_by_key(|path| std::fs::metadata(path).map_or(u64::MAX, |meta| meta.len()));
    paths.truncate(most);
    paths.iter().filter_map(|path| std::fs::read(path).ok()).collect()
}

/// PDFs made here, with each structure the walks look at, whole, so mutation starts from what works.
pub fn made_pdfs() -> Vec<Vec<u8>> {
    use recto_testpdf::{Object, Page, build, pages, pages_with, text_lines};
    let plain = pages(&[Page::new(text_lines(&["One line", "Another"])), Page::new(text_lines(&["Second page"]))]);
    let outlined = build(&[
        Object::Dict("<< /Type /Catalog /Pages 2 0 R /Outlines 5 0 R /Names 8 0 R /Dests 11 0 R >>".into()),
        Object::Dict("<< /Type /Pages /Kids [3 0 R 4 0 R] /Count 2 >>".into()),
        Object::Dict("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] >>".into()),
        Object::Dict("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] >>".into()),
        Object::Dict("<< /Type /Outlines /First 6 0 R >>".into()),
        Object::Dict("<< /Title (Named) /Dest (chapter) /Next 7 0 R /First 12 0 R >>".into()),
        Object::Dict("<< /Title <FEFF00C6> /A << /S /GoTo /D [4 0 R /Fit] >> >>".into()),
        Object::Dict("<< /Dests 9 0 R >>".into()),
        Object::Dict("<< /Kids [10 0 R] >>".into()),
        Object::Dict("<< /Limits [(a) (z)] /Names [(chapter) [3 0 R /Fit]] >>".into()),
        Object::Dict("<< /legacy [4 0 R /Fit] >>".into()),
        Object::Dict("<< /Title (Child) /Dest /legacy >>".into()),
    ]);
    let signed = build(&[
        Object::Dict("<< /Type /Catalog /Pages 2 0 R /AcroForm 4 0 R /Perms << /DocMDP 6 0 R >> >>".into()),
        Object::Dict("<< /Type /Pages /Kids [3 0 R] /Count 1 >>".into()),
        Object::Dict("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] >>".into()),
        Object::Dict("<< /Fields [5 0 R 7 0 R] >>".into()),
        Object::Dict("<< /FT /Sig /T (Sig1) /V 6 0 R /P 3 0 R >>".into()),
        Object::Dict("<< /Filter /Adobe.PPKLite /SubFilter /ETSI.CAdES.detached /Name (Someone) /M (D:20260101) /ByteRange [0 10 20 30] >>".into()),
        Object::Dict("<< /T (Parent) /FT /Sig /Kids [8 0 R] >>".into()),
        Object::Dict("<< /T (Child) /P 3 0 R >>".into()),
    ]);
    let extras = pages_with(
        &[Page::new(recto_testpdf::busy_page())],
        "",
        vec![Object::Stream {
            dict: "/Type /ObjStm /N 1 /First 4".into(),
            data: b"9 0 << /A 1 >>".to_vec(),
        }],
    );
    vec![plain, outlined, signed, extras]
}

/// Bytes for the structure-building target. Any bytes make a PDF, and these make varied ones.
pub fn tree_seeds() -> Vec<Vec<u8>> {
    (0..16u8).map(|i| (0..512u32).map(|at| (u32::from(i) * 37 + at * 11) as u8).collect()).collect()
}

/// One real shape of each kind of bytes a worker sends.
pub fn proto_seeds() -> Vec<Vec<u8>> {
    use recto_proto::*;
    let job = RenderJob {
        generation: 1,
        job_id: 2,
        page: 0,
        scale: 2.0,
        rect: TileRect {
            x: 512,
            y: 0,
            width: 1024,
            height: 512,
        },
        stride: 4096,
        tile_edge: 512,
    };
    let tile = TileReply {
        generation: 1,
        job_id: 2,
        width: 512,
        height: 512,
        stride: 2048,
        status: TileStatus::Done(TileKind::Blank { rgb: 0xFF_FFFF }),
        render_us: 9,
    };
    let region = RegionReply {
        tile,
        kinds: vec![TileKind::Grey, TileKind::Colour, TileKind::Blank { rgb: 1 }],
    };
    let text = TextReply {
        job_id: 3,
        page: 4,
        hidden_checked: true,
        chars: vec![CharRecord::new('a' as u32, CharRecord::NO_INK), CharRecord::new('b' as u32, 0)],
        boxes: vec![[1.0, 2.0, 3.0, 4.0], [4.0, 2.0, 5.0, 4.0]],
        ..TextReply::default()
    };
    let frames: Vec<Vec<u8>> = vec![
        encode_hello(Hello { pid: 7, walled: false }),
        encode_opened(Opened {
            pages: 12,
            version: 1,
            repaired: true,
        }),
        job.encode(),
        tile.encode(),
        region.encode(),
        PageSize {
            job_id: 1,
            width: 612.0,
            height: 792.0,
        }
        .encode(),
        PageTraits {
            job_id: 1,
            largest_image_pixels: 1 << 30,
        }
        .encode(),
        // One the right way round and one inside out, as a hostile worker can send.
        PageImages {
            job_id: 1,
            boxes: vec![[0, 0, 65_535, 30_000], [50_000, 100, 1_000, 40_000]],
        }
        .encode(),
        TextJob {
            job_id: 1,
            page: 2,
            boxes: true,
            hidden_check: true,
            generation: 3,
        }
        .encode(),
        text.encode().expect("a small page"),
        encode_failed(9, FailCode::TooLarge),
    ];
    let mut seeds = Vec::new();
    for frame in &frames {
        // Whole, down the channel, and each route's view of its payload.
        seeds.push([&[0u8][..], frame].concat());
        seeds.push([&[1u8, 0, 1, 0, 1, 0, 0, 2][..], &frame[HEADER_LEN..]].concat());
        seeds.push([&[2u8][..], &frame[HEADER_LEN..]].concat());
    }
    seeds.push([&[0u8][..], &frames.concat()].concat());
    seeds.push(vec![3; 32]);
    seeds
}

/// Pages that read like text, and ones that don't.
pub fn index_seeds() -> Vec<Vec<u8>> {
    let mut seeds = Vec::new();
    let body = |salt: u32| (0..360u32).map(move |at| (at.wrapping_mul(at).wrapping_mul(salt * 2 + 1).wrapping_add(at) >> 2) as u8);
    for mode in [0u8, 2, 0xC0, 0xD0, 0xE0, 0xF0, 1, 0xD1] {
        for salt in 0..4 {
            seeds.push(std::iter::once(mode).chain(body(salt)).collect());
        }
    }
    // One of each at the wire's cap: each takes a second or more, and more of them would be all a run does.
    seeds.push(std::iter::once(0xFE).chain(body(1)).collect());
    seeds.push(std::iter::once(0xFF).chain(body(1)).collect());
    seeds
}

/// Steps of every kind, on documents of several lengths.
pub fn sequence_seeds() -> Vec<Vec<u8>> {
    let mut seeds = Vec::new();
    for file_pages_choice in [0u8, 2, 4, 8] {
        let mut seed = vec![file_pages_choice];
        for op in 0..64u32 {
            seed.extend_from_slice(&[(op % 3) as u8, (op * 5) as u8, (op % 4) as u8, 1, 0, 0, (op * 3) as u8, 0, 10]);
        }
        seeds.push(seed);
    }
    seeds
}

/// JSON lines for each command, and hostile forms of them.
pub fn json_lines() -> Vec<String> {
    let mut lines: Vec<String> = TABLE.iter().map(|spec| format!(r#"{{"cmd":"{}"}}"#, spec.tag)).collect();
    lines.extend(
        [
            r#"{"cmd":"open","path":"/fuzz/p12.pdf","page":3}"#,
            r#"{"cmd":"open","path":"/fuzz/p12.pdf","page":4294967295}"#,
            r#"{"cmd":"tabs","from":4294967295,"limit":0}"#,
            r##"{"cmd":"tap","tab":"#1"}"##,
            r##"{"cmd":"goto","tab":"#99999999999999999999","page":1}"##,
            r#"{"cmd":"goto","tab":"active","page":0}"#,
            r#"{"cmd":"page_move","pages":"1-2","to":3}"#,
            r#"{"cmd":"page_move","pages":"4294967295-1","to":4294967295}"#,
            r#"{"cmd":"page_rotate","pages":"1","degrees":-2147483648}"#,
            r#"{"cmd":"page_rotate","pages":"1-","degrees":90}"#,
            r#"{"cmd":"page_delete","pages":"1-4294967295"}"#,
            r#"{"cmd":"page_insert","path":"/fuzz/p200000.pdf","at":1}"#,
            r#"{"cmd":"contents","depth":255,"from":0,"limit":4294967295}"#,
            r#"{"cmd":"search","query":"","pages":"1-","limit":0}"#,
            r#"{"cmd":"wait","condition":"tab_open","tab":"canary-in-a-line","timeout_ms":18446744073709551615}"#,
            r#"{"cmd":"watch","events":["page_drawn","x"],"timeout_ms":0,"count":0}"#,
            r#"{"cmd":"permission_request","level":"full","timeout_ms":1}"#,
            r#"{"cmd":"log","last":4294967295}"#,
            r#"{"cmd":"unknown","extra":[[[[[[{"a":1e999}]]]]]]}"#,
        ]
        .map(str::to_string),
    );
    lines
}

pub fn agent_seeds() -> Vec<Vec<u8>> {
    let mut seeds = Vec::new();
    for line in json_lines() {
        seeds.push([&[0u8, 2][..], line.as_bytes()].concat());
    }
    for spec in TABLE {
        for example in spec.examples {
            let words: Vec<&str> = example.split(' ').skip(1).collect();
            seeds.push([&[1u8, 0][..], words.join("\0").as_bytes()].concat());
            // Mistyped flags, which is where the advice has to mend a line.
            seeds.push([&[1u8, 0][..], words.join("\0").replace("--", "--x").as_bytes()].concat());
        }
    }
    for grammar in ["1h", "90s", "5ms", "18446744073709551615h", "1-2", "4-", "page move", "#1", "active", "é"] {
        seeds.push([&[2u8, 0][..], grammar.as_bytes()].concat());
    }
    for file in [
        r#"{"level":"control","set_at_ms":1}"#,
        r#"{"level":"full"}"#,
        r#"{"level":["full"]}"#,
        r#"{"level":"read_only","set_at_ms":-1}"#,
        "[",
        "",
    ] {
        seeds.push([&[3u8, 0][..], file.as_bytes()].concat());
    }
    seeds
}

pub fn control_seeds() -> Vec<Vec<u8>> {
    let mut seeds: Vec<Vec<u8>> = json_lines().iter().map(|line| [&[0u8][..], line.as_bytes(), b"\n"].concat()).collect();
    seeds.push([&[0u8][..], json_lines().join("\n").as_bytes(), b"\n"].concat());
    seeds.push([&[0u8][..], &[b'\n'; 4096][..]].concat());
    seeds.push([&[0u8][..], &vec![b'x'; 70 * 1024][..], b"\n"].concat());
    seeds.push([&[3u8][..], b"{\"cmd\":\"tabs\"}\r\n \t\n{\"cmd\":\"sidebar\"}"].concat());
    seeds
}

/// A target's starting points.
/// A real `latest.json`, and a small zip made the way the reader takes them, of files stored and a folder.
fn update_seeds() -> Vec<Vec<u8>> {
    let latest = format!(
        r#"{{"version":"0.2.0","tag":"v0.2.0","minimum_macos":"15.0","zip":"RectoPDF-0.2.0.zip","sha256":"{}"}}"#,
        "a".repeat(64)
    )
    .into_bytes();
    let plan = br#"{"id":"a1","from":"0.1.0","to":"0.2.0","cdhash":"","pid":7,"app":"/Applications/RectoPDF.app","beside":"/Applications/.RectoPDF-update","root":"/tmp/state","quit_at_ms":1,"background":false,"quit_limit_ms":30000,"open":["/tmp/a.pdf"]}"#.to_vec();
    let outcome = br#"{"kind":"went_back","from":"0.2.0","to":"0.1.0","why":"kept_ending"}"#.to_vec();
    let saved =
        br#"{"last_check_ms":1,"failures":0,"offered":"0.2.0","install_failures":0,"quits_for_nothing":0,"failed":["0.1.9"],"proven":"0.1.0","unstarted":0}"#
            .to_vec();
    let launched = b"0.2.0 tabs=3\n".to_vec();
    let dump = b"--------\npath:   /Applications/RectoPDF.app (0x3b1f4)\nidentifier: io.github.kkollsga.RectoPDF\n--------\npath: /x.app\nidentifier: io.github.kkollsga.RectoPDFX\n".to_vec();
    vec![latest, update_seed_zip(), plan, outcome, saved, launched, dump]
}

fn update_seed_zip() -> Vec<u8> {
    let crc = crc_of(b"data");
    let name = b"RectoPDF.app/x";
    let mut zip = Vec::new();
    zip.extend_from_slice(&0x0403_4b50u32.to_le_bytes());
    zip.extend_from_slice(&[20, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
    zip.extend_from_slice(&crc.to_le_bytes());
    zip.extend_from_slice(&4u32.to_le_bytes());
    zip.extend_from_slice(&4u32.to_le_bytes());
    zip.extend_from_slice(&(name.len() as u16).to_le_bytes());
    zip.extend_from_slice(&[0, 0]);
    zip.extend_from_slice(name);
    zip.extend_from_slice(b"data");
    let list_at = zip.len() as u32;
    let mut list = Vec::new();
    list.extend_from_slice(&0x0201_4b50u32.to_le_bytes());
    list.extend_from_slice(&[20, 3, 20, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
    list.extend_from_slice(&crc.to_le_bytes());
    list.extend_from_slice(&4u32.to_le_bytes());
    list.extend_from_slice(&4u32.to_le_bytes());
    list.extend_from_slice(&(name.len() as u16).to_le_bytes());
    list.extend_from_slice(&[0; 8]);
    list.extend_from_slice(&(0o100_644u32 << 16).to_le_bytes());
    list.extend_from_slice(&0u32.to_le_bytes());
    list.extend_from_slice(name);
    zip.extend_from_slice(&list);
    zip.extend_from_slice(&0x0605_4b50u32.to_le_bytes());
    zip.extend_from_slice(&[0, 0, 0, 0, 1, 0, 1, 0]);
    zip.extend_from_slice(&(list.len() as u32).to_le_bytes());
    zip.extend_from_slice(&list_at.to_le_bytes());
    zip.extend_from_slice(&[0, 0]);
    zip
}

fn crc_of(bytes: &[u8]) -> u32 {
    let mut crc = u32::MAX;
    for byte in bytes {
        crc ^= u32::from(*byte);
        for _ in 0..8 {
            crc = if crc & 1 == 1 { (crc >> 1) ^ 0xEDB8_8320 } else { crc >> 1 };
        }
    }
    !crc
}

/// Samples of the state record's files, and their bodies alone, which the target seals itself.
fn record_seeds() -> Vec<Vec<u8>> {
    let record = include_bytes!("../../recto-session/tests/fixtures/record-v1.json").to_vec();
    let journal = include_bytes!("../../recto-session/tests/fixtures/journal-v1.log").to_vec();
    let body = record.split(|byte| *byte == b'\n').nth(1).unwrap_or_default().to_vec();
    let lines: Vec<u8> = journal
        .split(|byte| *byte == b'\n')
        .skip(1)
        .filter(|line| line.len() > 9)
        .flat_map(|line| line[9..].iter().copied().chain(std::iter::once(b'\n')))
        .collect();
    // A comparison, which a record has had since comparison view.
    let compared = String::from_utf8_lossy(&body)
        .replacen(
            "\"next_id\"",
            "\"comparison\":{\"tabs\":[1,2],\"synced\":true,\"sync_from\":[3,0]},\"next_id\"",
            1,
        )
        .into_bytes();
    vec![
        record,
        journal,
        body,
        compared,
        lines,
        br#"{"seq":1,"active":1,"places":[[1,3,{"page":3,"anchor_page":3,"within":0.5,"x":0,"y":10,"document_height":900,"zoom":1.5}]]}"#.to_vec(),
    ]
}

/// Generated contents as they're kept, and as the helper says them.
fn generated_seeds() -> Vec<Vec<u8>> {
    let kept = br#"{"format":"recto-generated-contents","version":1,"key":"abababababababababababababababababababababababababababababababab","pages":12,"truncated":false,"hidden_dropped":1,"pages_not_checked":0,"pages_failed":0,"entries":[[0,0,80,"1 Introduction"],[1,2,400,"1.1 Background"],[0,11,0,"References"]]}"#.to_vec();
    let mut answer = br#"{"state":"ready","found":4,"kept":3,"elapsed_ms":120}"#.to_vec();
    answer.push(b'\n');
    answer.extend_from_slice(&kept);
    vec![
        kept,
        answer,
        br#"{"state":"refused","code":"couldnt_read"}"#.to_vec(),
        (0..=255u8).collect(),
        br#"{"state":"ready","pages":12,"pages_built":3,"pages_resumed":9,"elapsed_ms":40,"corpus_bytes":900,"pack_bytes":300}"#.to_vec(),
        br#"{"state":"paused","code":"too_busy"}"#.to_vec(),
    ]
}

/// Each way into `chrome`: text alone, edge pieces made from the bytes, and pages of rows, a few of each, with a page of thousands of rows.
fn chrome_seeds() -> Vec<Vec<u8>> {
    let mut seeds = vec![[&[0u8][..], "Page 12 of 40 · L 119/6 · xii · 3/14/2019".as_bytes()].concat()];
    for salt in 0..4u8 {
        let body: Vec<u8> = (0..400u32).map(|at| (at.wrapping_mul(u32::from(salt) * 2 + 7) >> 1) as u8).collect();
        seeds.push([&[1u8][..], &body].concat());
        seeds.push([&[2u8, salt, salt * 3, 4, salt][..], &body].concat());
    }
    seeds.push([&[2u8, 0, 1, 4, 0, 0xF0, 0x10, 0x00][..], &[3u8; 64]].concat());
    seeds
}

/// Records as the bytes say, made records of every kind, and JPEGs to scan.
fn svg_seeds() -> Vec<Vec<u8>> {
    let mut seeds = Vec::new();
    for salt in 0..6u8 {
        let body: Vec<u8> = (0..512u32)
            .map(|at| (at.wrapping_mul(u32::from(salt) * 4 + 3).wrapping_add(at >> 3)) as u8)
            .collect();
        seeds.push([&[1u8, salt & 1, salt % 4][..], &body].concat());
        seeds.push([&[3u8, 0, 0][..], &body].concat());
    }
    // Every kind of record in turn, then a frame break.
    seeds.push(
        [
            &[2u8, 1, 0, 0, 0, 0, 0][..],
            &(0..13u8).flat_map(|kind| [kind, 1, 2, 3, 4, 5, 6, 7]).collect::<Vec<u8>>(),
        ]
        .concat(),
    );
    seeds
}

/// An open history and a closed one, each with the kept file as it was, removed, replaced by a link, and made old.
fn kept_seeds() -> Vec<Vec<u8>> {
    vec![
        vec![0, 0, 0, 1, 0, 1, 0, 0, 0, 0, 0, 0, 7, 0],
        vec![1, 1, 1, 1, 0, 1, 0, 0, 0, 0, 0, 0, 7, 1, 0],
        vec![0, 0, 2, 1, 0, 1, 0, 0, 0, 0, 0, 0, 7, 1, 4],
        vec![1, 2, 0, 1, 0, 1, 0, 0, 0, 0, 0, 0, 0, 3, 7, 9, 6],
        vec![0, 1, 0, 1, 0, 1, 0, 0, 0, 0, 0, 0, 1, 2, 1, 0, 2],
    ]
}

/// Damage to a corpus, as `corpus` reads it: the first byte says whether checksums are put right after, then each step is what, which file and how. Nothing, a record pointed past the pack, a table cut short, a mark replaced, a page's text overwritten, a link, and a file others can write.
fn corpus_seeds() -> Vec<Vec<u8>> {
    vec![
        Vec::new(),
        vec![0],
        vec![1, 6, 0, 0, 0xff, 0xff, 0xff, 0x7f, 1],
        vec![0, 1, 5, 30, 0],
        [&[1u8, 3, 4][..], br#"{"format":"v3"}"#].concat(),
        vec![1, 0, 6, 0, 0, 8, b'X', b'Y', b'Z', 0xff, 0xfe, 0, 1, 2],
        vec![0, 8, 3, 6],
        vec![0, 9, 1],
        vec![1, 6, 0, 0, 255, 255, 255, 255, 2, 0, 9, 0, 0],
    ]
}

pub fn for_target(name: &str) -> Vec<Vec<u8>> {
    match name {
        "pdf_bytes" => {
            let mut seeds = made_pdfs();
            seeds.extend(real_pdfs(usize::MAX));
            seeds
        }
        "pdf_tree" | "save_copy" => tree_seeds(),
        "proto" => proto_seeds(),
        "index" => index_seeds(),
        "select" => select_seeds(),
        "agent" => agent_seeds(),
        "control" => control_seeds(),
        "edits" => sequence_seeds(),
        "record" => record_seeds(),
        "generated" => generated_seeds(),
        "corpus" => corpus_seeds(),
        "update" => update_seeds(),
        "chrome" => chrome_seeds(),
        "svg" => svg_seeds(),
        "kept" => kept_seeds(),
        _ => Vec::new(),
    }
}

/// The pages `index` starts from, each after what says how it sits and where the pointer goes: a page as it comes, and one turned, with a pointer on it.
pub fn select_seeds() -> Vec<Vec<u8>> {
    let mut seeds = Vec::new();
    for page in index_seeds() {
        for head in [
            [0u8; 64],
            [
                1, 0, 0, 2, 3, 1, 1, 4, 4, 2, 0, 3, 3, 1, 2, 2, 1, 4, 3, 0, 0, 2, 2, 1, 3, 4, 4, 0, 1, 1, 2, 3, 1, 0, 4, 2, 2, 3, 0, 1, 4, 1, 3, 2, 0, 0, 1, 2,
                3, 4, 1, 3, 0, 2, 4, 1, 2, 0, 3, 1, 4, 2, 0, 1,
            ],
        ] {
            let mut seed = head.to_vec();
            seed.extend_from_slice(&page);
            seeds.push(seed);
        }
    }
    seeds
}

/// Every target's name.
pub const TARGET_NAMES: &[&str] = &[
    "pdf_bytes",
    "pdf_tree",
    "save_copy",
    "proto",
    "index",
    "select",
    "agent",
    "control",
    "edits",
    "record",
    "update",
    "generated",
    "corpus",
    "chrome",
    "svg",
    "kept",
];

/// The check a target runs, by name.
pub fn check_for(name: &str) -> Option<fn(&[u8])> {
    Some(match name {
        "pdf_bytes" => targets::pdf::check_bytes,
        "pdf_tree" => targets::pdf::check_tree,
        "save_copy" => targets::pdf::check_save,
        "proto" => targets::proto::check,
        "index" => targets::index::check,
        "select" => targets::select::check,
        "agent" => targets::agent::check,
        "control" => targets::control::check,
        "edits" => targets::edits::check,
        "record" => targets::record::check,
        "generated" => targets::generated::check,
        "corpus" => targets::corpus::check,
        "update" => targets::update::check,
        "chrome" => targets::chrome::check,
        "svg" => targets::svg::check,
        "kept" => targets::kept::check,
        _ => return None,
    })
}
