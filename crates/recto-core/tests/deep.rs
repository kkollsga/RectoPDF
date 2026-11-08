//! Hostile depth, run in the process that matters: the structure reader runs inside the session, so a stack overflow here would abort it, unsaved edits and all, and nothing could catch that. Run in debug on a
//! test thread's 2 MiB, which is well under what any real caller has: the session's job threads have 8 MiB, and debug frames are larger than release ones.

mod support;

use recto_core::{inspect, signatures};
use support::{TempPdf, pdf, three_page_pdf};

const DEEP: usize = 100_000;

fn nested_array() -> String {
    format!("{}{}", "[".repeat(DEEP), "]".repeat(DEEP))
}

fn nested_dict() -> String {
    format!("{}1{}", "<< /A ".repeat(DEEP), " >>".repeat(DEEP))
}

/// Everything a session does to a file's structure, which must return, whatever it finds.
fn read_all_of(name: &str, bytes: &[u8]) {
    let file = TempPdf::new(name, bytes);
    let _ = inspect(&file.0);
    let _ = signatures(&file.0);
}

/// Failure: arrays or dictionaries nested a hundred thousand deep overflow the stack in the parser, wherever they sit: in the catalog, in a page, as a title, as a form or as a signature's value. The reader stops at 100 levels.
#[test]
fn deep_nesting_in_any_object_returns() {
    for (kind, deep) in [("array", nested_array()), ("dict", nested_dict())] {
        read_all_of(&format!("{kind}-catalog.pdf"), &three_page_pdf(&format!("/X {deep}"), &[], ""));
        read_all_of(
            &format!("{kind}-form.pdf"),
            &three_page_pdf(
                "/AcroForm 6 0 R",
                &[format!("<< /Fields [7 0 R] /X {deep} >>"), format!("<< /FT /Sig /T (F) /V {deep} >>")],
                "",
            ),
        );
        read_all_of(
            &format!("{kind}-outline.pdf"),
            &three_page_pdf(
                "/Outlines 6 0 R",
                &["<< /First 7 0 R >>".to_string(), format!("<< /Title {deep} /Dest {deep} >>")],
                "",
            ),
        );
        read_all_of(
            &format!("{kind}-info.pdf"),
            &three_page_pdf("", &[format!("<< /Title {deep} >>")], "/Info 6 0 R"),
        );
    }
}

/// Failure: a page tree a hundred thousand levels deep overflows the stack. The reader's own limit is 256 levels.
#[test]
fn a_deep_page_tree_returns() {
    let mut objects = vec![
        "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
        "<< /Type /Pages /Kids [4 0 R] /Count 1 >>".to_string(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] >>".to_string(),
    ];
    for i in 0..DEEP {
        let kid = if i + 1 < DEEP { 5 + i } else { 3 };
        objects.push(format!("<< /Type /Pages /Kids [{kid} 0 R] /Count 1 >>"));
    }
    read_all_of("page-tree.pdf", &pdf(&objects, "/Root 1 0 R"));
}

/// Failure: an outline chain as deep as a file can make overflows the stack. The walk keeps its own stack, for both ways an outline can go.
#[test]
fn a_deep_outline_returns_and_is_capped() {
    for link in ["First", "Next"] {
        let mut extra = vec!["<< /First 7 0 R >>".to_string()];
        for i in 0..DEEP {
            let next = if i + 1 < DEEP { format!("/{link} {} 0 R", 8 + i) } else { String::new() };
            extra.push(format!("<< /Title (t) /Dest [3 0 R /Fit] {next} >>"));
        }
        let file = TempPdf::new(&format!("outline-{link}.pdf"), &three_page_pdf("/Outlines 6 0 R", &extra, ""));
        let toc = inspect(&file.0).unwrap().toc;
        assert!(!toc.is_empty(), "{link}");
    }
}

/// Failure: a field tree as deep as a file can make overflows the stack, or a deep name tree does. Both keep their own stacks.
#[test]
fn deep_kids_and_a_deep_name_tree_return() {
    let mut extra = vec!["<< /Fields [7 0 R] >>".to_string()];
    for i in 0..DEEP {
        let next = if i + 1 < DEEP {
            format!("/Kids [{} 0 R]", 8 + i)
        } else {
            "/FT /Sig".to_string()
        };
        extra.push(format!("<< /T (n) {next} >>"));
    }
    let file = TempPdf::new("kids.pdf", &three_page_pdf("/AcroForm 6 0 R", &extra, ""));
    assert!(signatures(&file.0).unwrap().truncated, "past its depth limit, it must say so");

    let mut extra = vec![
        "<< /First 7 0 R >>".to_string(),
        "<< /Title (t) /Dest (x) >>".to_string(),
        "<< /Dests 9 0 R >>".to_string(),
    ];
    for i in 0..DEEP {
        let next = if i + 1 < DEEP {
            format!("/Kids [{} 0 R]", 10 + i)
        } else {
            "/Names [(x) [3 0 R /Fit]]".to_string()
        };
        extra.push(format!("<< {next} >>"));
    }
    read_all_of("names.pdf", &three_page_pdf("/Outlines 6 0 R /Names 8 0 R", &extra, ""));
}
