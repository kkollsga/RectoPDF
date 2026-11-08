//! A file's name tree against the memory limit. In its own binary, so the count of live bytes is this test's alone.

use recto_fuzz::guard::peak_of;
use recto_testpdf::{Object, build};
use std::sync::Arc;

/// A catalog, one page, one outline entry naming a destination, and a name tree whose only node lists itself `width` times.
fn pdf_with_a_self_listing_name_tree(width: usize) -> Vec<u8> {
    let kids = "6 0 R ".repeat(width);
    build(&[
        Object::Dict("<< /Type /Catalog /Pages 2 0 R /Outlines 4 0 R /Names 7 0 R >>".into()),
        Object::Dict("<< /Type /Pages /Kids [3 0 R] /Count 1 >>".into()),
        Object::Dict("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] >>".into()),
        Object::Dict("<< /Type /Outlines /First 5 0 R >>".into()),
        Object::Dict("<< /Title (A) /Dest (nowhere) >>".into()),
        Object::Dict(format!("<< /Kids [{kids}] >>")),
        Object::Dict("<< /Dests 6 0 R >>".into()),
    ])
}

/// Failure: walking a name tree that lists itself keeps every child of every visit waiting on a stack, 65,535 more each time round, so a 400 KB file holds gigabytes in the session's own process until the 5 s limit, or the Mac runs out.
#[test]
fn a_name_tree_that_lists_itself_stays_small() {
    let bytes: Arc<[u8]> = pdf_with_a_self_listing_name_tree(65_536).into();
    let (facts, peak) = peak_of(|| recto_core::inspect_bytes(Arc::clone(&bytes)));
    assert!(
        facts.expect("the file itself is fine to open").toc.truncated,
        "it left children out, so it has to say the contents may not be whole"
    );
    // The file as read, a page map and the child list a few times over. Nothing like a child list per visit.
    assert!(peak < 64 * 1024 * 1024, "{peak} bytes held for a {} byte file", bytes.len());
}
