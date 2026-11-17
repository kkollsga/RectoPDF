//! Repeated content at a page's edge is still content: exercise the text-to-copy path, not only the chrome classifier.

use crate::select::{CopiedText, PageText, Range, chrome_for};
use recto_proto::{CharRecord, PageFrame};

const HEADINGS: [&str; 3] = ["Core platform services", "Active end users", "Active business users"];

fn table_page(first_cell: &str, number: &str) -> PageText {
    let (mut chars, mut boxes) = (Vec::new(), Vec::new());
    for (words, x, y) in [
        (HEADINGS[0], 72.0, 750.0),
        (HEADINGS[1], 252.0, 750.0),
        (HEADINGS[2], 432.0, 750.0),
        (first_cell, 72.0, 710.0),
        ("People using the service", 252.0, 710.0),
        ("Businesses using it", 432.0, 710.0),
        (number, 280.0, 30.0),
    ] {
        for (at, ch) in words.chars().enumerate() {
            let left = x + at as f32 * 5.0;
            chars.push(CharRecord::new(ch as u32, 0));
            boxes.push(if ch == ' ' {
                [left, y, left + 5.0, y]
            } else {
                [left, y, left + 4.0, y + 10.0]
            });
        }
        for ch in ['\r', '\n'] {
            chars.push(CharRecord::new(ch as u32, CharRecord::GENERATED));
            boxes.push([0.0; 4]);
        }
    }
    PageText::build(
        &chars,
        &boxes,
        PageFrame {
            turns: 0,
            left: 0.0,
            bottom: 0.0,
            right: 612.0,
            top: 792.0,
        },
        true,
        false,
    )
}

/// Failure: copying a table across pages loses its three repeated multi-word column labels as running heads (EU DMA pp65–66), so the copied values have no meaning.
#[test]
fn a_copy_across_table_pages_keeps_the_repeated_column_labels() {
    let pages = [table_page("Search engines", "Page 1"), table_page("Web browsers", "Page 2")];
    let mut copied = CopiedText::default();
    let range = Range::pages(0, 1);
    for (page, text) in pages.iter().enumerate() {
        let chrome = chrome_for(page as u32, text, pages.len() as u32, |other| {
            pages.get(other as usize).map(PageText::edge_lines)
        });
        assert!(copied.add_page(page as u32, text, &range, usize::MAX, &chrome));
    }
    copied.finish();
    for heading in HEADINGS {
        assert_eq!(
            copied.text.matches(heading).count(),
            2,
            "the table label {heading:?} must remain on each page: {:?}",
            copied.text
        );
    }
    assert!(
        copied.text.contains("Search engines") && copied.text.contains("Web browsers"),
        "{:?}",
        copied.text
    );
    assert!(
        !copied.text.contains("Page 1") && !copied.text.contains("Page 2"),
        "the page numbers still leave the body copy: {:?}",
        copied.text
    );
}
