//! The agreement gate against the real renderer: crafted files the two readers read alike, and ones they read differently. Each runs PDFium in its
//! worker, so it needs `recto-render-service` built (`tools/check.sh` builds it first).

use recto_agent::agreement::look;
use recto_core::agreement::Disagreement;
use recto_testpdf::{Object, TempPdf, build};

fn dict(body: &str) -> Object {
    Object::Dict(body.into())
}

/// Failure: the gate turns Save off on an ordinary file, as a turned page, an inherited box or a crop box read one way by lopdf and another by
/// PDFium would, so no file could be saved. Sizes of every kind agree.
#[test]
fn an_ordinary_file_agrees() {
    let file = TempPdf::new(
        "ordinary.pdf",
        &build(&[
            dict("<< /Type /Catalog /Pages 2 0 R >>"),
            dict("<< /Type /Pages /Kids [3 0 R 4 0 R 5 0 R 6 0 R] /Count 4 /MediaBox [0 0 400 600] >>"),
            dict("<< /Type /Page /Parent 2 0 R >>"),
            dict("<< /Type /Page /Parent 2 0 R /Rotate 90 >>"),
            dict("<< /Type /Page /Parent 2 0 R /CropBox [10 10 110 210] /Rotate 270 >>"),
            dict("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] >>"),
        ]),
    );
    assert_eq!(look(&file.0), None);
}

/// Failure: a page tree whose kid doesn't load is saved from lopdf's reading, which leaves the page out, while PDFium shows a page there from the
/// root's `/Count`. The page counts differ, and the gate says so.
#[test]
fn a_page_the_structure_leaves_out_is_a_disagreement() {
    let file = TempPdf::new(
        "missing-kid.pdf",
        &build(&[
            dict("<< /Type /Catalog /Pages 2 0 R >>"),
            dict("<< /Type /Pages /Kids [3 0 R 9 0 R 4 0 R] /Count 3 >>"),
            dict("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] >>"),
            dict("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 400 400] >>"),
        ]),
    );
    assert_eq!(look(&file.0), Some(Disagreement::PageCount { structure: 2, renderer: 3 }));
}

/// Failure: a page tree the two readers walk in a different order is saved from lopdf's walk, so a turn or a move lands on another page than the
/// one the person saw. A `/Type /Page` with `/Kids` is a page to lopdf and a branch to PDFium: both count two pages, but not the same first one.
#[test]
fn a_page_tree_walked_differently_is_a_disagreement() {
    let file = TempPdf::new(
        "walked-differently.pdf",
        &build(&[
            dict("<< /Type /Catalog /Pages 2 0 R >>"),
            dict("<< /Type /Pages /Kids [3 0 R 5 0 R] /Count 2 >>"),
            dict("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Kids [4 0 R] >>"),
            dict("<< /Type /Page /Parent 3 0 R /MediaBox [0 0 400 400] >>"),
            dict("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] >>"),
        ]),
    );
    assert_eq!(look(&file.0), Some(Disagreement::PageSize { page: 0 }));
}

/// Failure: a file whose cross-reference table is broken where lopdf, reading lazily, never looks, is saved from the table, while PDFium has rebuilt
/// it by scanning and shows the later copy of a page. Here the first object the table lists isn't where it says, which PDFium checks and lopdf
/// never needs, and page 1 has two copies, the table naming the first.
#[test]
fn a_broken_index_one_reader_repairs_is_a_disagreement() {
    let mut out = b"%PDF-1.7\n%\xe2\xe3\xcf\xd3\n".to_vec();
    let mut offsets = Vec::new();
    for (number, body) in [
        (1, "<< /Unused true >>"),
        (2, "<< /Type /Catalog /Pages 3 0 R >>"),
        (3, "<< /Type /Pages /Kids [4 0 R] /Count 1 >>"),
        (4, "<< /Type /Page /Parent 3 0 R /MediaBox [0 0 200 200] >>"),
    ] {
        offsets.push(out.len());
        out.extend(format!("{number} 0 obj\n{body}\nendobj\n").as_bytes());
    }
    // A later copy of page 1, which no table names: only a scan finds it, and a scan takes the last.
    out.extend(b"4 0 obj\n<< /Type /Page /Parent 3 0 R /MediaBox [0 0 400 400] >>\nendobj\n");
    // Object 1's place is wrong: it points into the catalog.
    offsets[0] = offsets[1] + 3;
    let xref = out.len();
    out.extend(b"xref\n0 5\n0000000000 65535 f \n");
    for offset in offsets {
        out.extend(format!("{offset:010} 00000 n \n").as_bytes());
    }
    out.extend(format!("trailer\n<< /Size 5 /Root 2 0 R >>\nstartxref\n{xref}\n%%EOF\n").as_bytes());
    let file = TempPdf::new("broken-index.pdf", &out);
    let structure = recto_core::agreement::structure_view_bytes(out.clone().into()).unwrap();
    assert!(!structure.repaired, "lopdf reads the table as it is");
    assert_eq!(structure.sizes, [Some((200.0, 200.0))]);
    assert_eq!(look(&file.0), Some(Disagreement::Repaired { by_renderer: true }));
}

/// Failure: a file the renderer can't read at all is saved, though what's shown can't be checked against it. Save is off for it.
#[test]
fn a_file_the_renderer_cant_read_is_a_disagreement() {
    let file = TempPdf::new("form-bomb.pdf", &recto_testpdf::form_bomb());
    // The form bomb draws for ever, but sizing its page is cheap: it agrees. A file that isn't there can't be read by either.
    assert_eq!(look(&file.0), None);
    assert_eq!(look(std::path::Path::new("/nowhere/at/all.pdf")), Some(Disagreement::RendererFailed));
}

/// Failure: the look is slow enough on a long document to matter (each page's size is a round trip to the worker). Measures a 2,000-page file and
/// prints it; asserts only that it's bounded, as timings differ between machines.
#[test]
fn a_long_document_is_looked_at_in_bounded_time() {
    let pages: Vec<recto_testpdf::Page> = (0..2_000).map(|_| recto_testpdf::Page::new("")).collect();
    let file = TempPdf::new("long.pdf", &recto_testpdf::pages(&pages));
    let started = std::time::Instant::now();
    assert_eq!(look(&file.0), None);
    let took = started.elapsed();
    eprintln!("2,000 pages looked at in {took:?}");
    assert!(took < std::time::Duration::from_secs(60), "{took:?}");
}
