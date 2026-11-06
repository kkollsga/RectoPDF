//! Its own binary, because it counts this process's open files and other tests running beside it would disturb that.
mod support;

use recto_core::inspect;
use support::{TempPdf, three_page_pdf};

/// Failure: reading a PDF's facts leaves its file open, so each tab costs a handle and many large PDFs run out of them.
#[test]
fn inspect_leaves_no_file_handle_open() {
    let file = TempPdf::new("handles.pdf", &three_page_pdf("", &[], ""));
    let Some(before) = recto_platform::open_handle_count() else { return };
    for _ in 0..50 {
        inspect(&file.0).unwrap();
    }
    assert_eq!(recto_platform::open_handle_count(), Some(before));
}
