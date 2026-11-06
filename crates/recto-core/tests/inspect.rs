mod support;

use recto_core::{NONE, OpenError, inspect};
use support::{TempPdf, three_page_pdf};

fn outline_objects() -> Vec<String> {
    vec![
        // 6: outline root. 7: "One" with child 8. 9: "Two".
        "<< /Type /Outlines /First 7 0 R /Last 9 0 R /Count 2 >>".to_string(),
        "<< /Title (One) /Parent 6 0 R /Next 9 0 R /First 8 0 R /Last 8 0 R /Dest [3 0 R /Fit] >>".to_string(),
        "<< /Title (One point one) /Parent 7 0 R /A << /S /GoTo /D [4 0 R /Fit] >> >>".to_string(),
        "<< /Title (Two) /Parent 6 0 R /Prev 7 0 R /Dest [5 0 R /Fit] >>".to_string(),
    ]
}

/// Failure: page count, title or version are read wrongly, or a plain PDF fails to open.
#[test]
fn reads_basic_facts() {
    let mut extra = outline_objects();
    extra.push("<< /Title (Harbour Survey) >>".to_string());
    let file = TempPdf::new("facts.pdf", &three_page_pdf("/Outlines 6 0 R", &extra, "/Info 10 0 R"));
    let facts = inspect(&file.0).unwrap();
    assert_eq!(facts.page_count, 3);
    assert_eq!(facts.title.as_deref(), Some("Harbour Survey"));
    assert!(!facts.encrypted);
    assert_eq!(facts.byte_len, std::fs::metadata(&file.0).unwrap().len());
}

/// Failure: contents lose their order or nesting, or a destination points at the wrong page.
#[test]
fn reads_contents_in_order_with_nesting_and_pages() {
    let file = TempPdf::new("toc.pdf", &three_page_pdf("/Outlines 6 0 R", &outline_objects(), ""));
    let toc = inspect(&file.0).unwrap().toc;
    let rows: Vec<_> = toc.entries().iter().map(|e| (toc.title(e), e.depth, e.page, e.parent)).collect();
    assert_eq!(rows, vec![("One", 0, 0, NONE), ("One point one", 1, 1, 0), ("Two", 0, 2, NONE)]);
}

/// Failure: the section being read is wrong, so the rail marks the wrong row.
#[test]
fn finds_the_section_for_a_page() {
    let file = TempPdf::new("section.pdf", &three_page_pdf("/Outlines 6 0 R", &outline_objects(), ""));
    let toc = inspect(&file.0).unwrap().toc;
    assert_eq!(toc.section_at(0), Some(0));
    assert_eq!(toc.section_at(1), Some(1));
    assert_eq!(toc.section_at(2), Some(2));
}

/// Failure: a hostile outline that loops back on itself hangs the reader or grows without limit.
#[test]
fn survives_an_outline_that_loops() {
    let extra = vec![
        "<< /Type /Outlines /First 7 0 R >>".to_string(),
        "<< /Title (Loop) /Next 7 0 R /First 7 0 R /Dest [3 0 R /Fit] >>".to_string(),
    ];
    let file = TempPdf::new("loop.pdf", &three_page_pdf("/Outlines 6 0 R", &extra, ""));
    let toc = inspect(&file.0).unwrap().toc;
    assert_eq!(toc.entries().len(), 1);
}

/// Failure: a named destination isn't followed through the catalog's name tree, so the contents can't jump.
#[test]
fn follows_named_destinations_through_the_name_tree() {
    let extra = vec![
        "<< /Type /Outlines /First 7 0 R >>".to_string(),
        "<< /Title (Named) /Dest (chapter) >>".to_string(),
        "<< /Dests 9 0 R >>".to_string(),
        "<< /Names [(chapter) [5 0 R /Fit]] >>".to_string(),
    ];
    let file = TempPdf::new("named.pdf", &three_page_pdf("/Outlines 6 0 R /Names 8 0 R", &extra, ""));
    let toc = inspect(&file.0).unwrap().toc;
    assert_eq!(toc.entries()[0].page, 2);
}

/// Failure: a missing or non-PDF file panics or reports the wrong reason.
#[test]
fn reports_plain_reasons_for_bad_files() {
    let missing = std::env::temp_dir().join("recto-core-test-does-not-exist.pdf");
    assert_eq!(inspect(&missing).unwrap_err(), OpenError::NotFound);
    let not_pdf = TempPdf::new("not.pdf", b"this is not a pdf at all");
    assert_eq!(inspect(&not_pdf.0).unwrap_err(), OpenError::NotPdf);
    assert!(!OpenError::NotPdf.message().contains("os error"));
}
