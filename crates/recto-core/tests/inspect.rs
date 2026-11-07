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

/// Failure: a wide name tree under a long outline makes `open` resolve every child for every entry, billions of reads, while the session waits. It must stop at its budget, and say the contents aren't whole.
#[test]
fn a_hostile_name_tree_under_a_long_outline_ends_soon() {
    // 20,000 entries, each naming a destination that isn't in a tree with one node of 200,000 references to itself and to a leaf.
    let refs = "7 0 R ".repeat(200_000);
    let mut extra = vec!["<< /Type /Outlines /First 9 0 R >>".to_string(), format!("<< /Kids [{refs}] >>"), "<< /Dests 7 0 R >>".to_string()];
    let entries = 20_000;
    for i in 0..entries {
        let next = if i + 1 < entries { format!("/Next {} 0 R", 10 + i) } else { String::new() };
        extra.push(format!("<< /Title (Entry) /Dest (nowhere) {next} >>"));
    }
    let file = TempPdf::new("name-bomb.pdf", &three_page_pdf("/Outlines 6 0 R /Names 8 0 R", &extra, ""));
    let started = std::time::Instant::now();
    let toc = inspect(&file.0).unwrap().toc;
    assert!(started.elapsed() < std::time::Duration::from_secs(30), "took {:?}", started.elapsed());
    assert!(toc.truncated, "it must say the contents aren't whole");
}

/// Failure: an ordinary named destination stops being found, because the budget is spent too eagerly, or a tree with a good branch after a bad one loses it.
#[test]
fn an_ordinary_name_tree_is_still_followed_past_a_dead_branch() {
    let extra = vec![
        "<< /Type /Outlines /First 7 0 R >>".to_string(),
        "<< /Title (Named) /Dest (chapter) >>".to_string(),
        "<< /Dests 9 0 R >>".to_string(),
        "<< /Kids [10 0 R 99 0 R 11 0 R] >>".to_string(),
        "<< /Limits [(a) (b)] /Names [(a) [3 0 R /Fit]] >>".to_string(),
        "<< /Limits [(c) (d)] /Names [(chapter) [5 0 R /Fit]] >>".to_string(),
    ];
    let file = TempPdf::new("named-branch.pdf", &three_page_pdf("/Outlines 6 0 R /Names 8 0 R", &extra, ""));
    let toc = inspect(&file.0).unwrap().toc;
    assert_eq!(toc.entries()[0].page, 2);
    assert!(!toc.truncated);
}
