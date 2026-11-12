use super::*;

/// Failure: warm paper is too dim to read on, by WCAG AA, or it's lost its warmth and reads as white, or gone yellow or orange. Holds the value: text at the page's usual dark greys stays above AA, and each channel falls a little more than the last without a big drop in blue.
#[test]
fn warm_paper_keeps_body_text_readable_and_reads_as_warm() {
    for ink in [0x00_00_00, 0x22_22_22, 0x44_44_44, 0x59_59_59] {
        assert!(
            contrast_ratio(ink, WARM_PAPER) >= 4.5,
            "{ink:06x} on {WARM_PAPER:06x}: {}",
            contrast_ratio(ink, WARM_PAPER)
        );
    }
    let (red, green, blue) = ((WARM_PAPER >> 16) & 0xFF, (WARM_PAPER >> 8) & 0xFF, WARM_PAPER & 0xFF);
    assert!(red >= green && green >= blue, "warm means red over green over blue");
    assert!(red - blue >= 12, "or it's white");
    assert!(red - blue <= 40, "or it's yellow");
    assert!(red >= 0xEE, "or it's dim");
}

/// Failure: the paper follows the window once it's been chosen, or doesn't when it hasn't. Every combination works: with nothing chosen it goes with the window, and a choice holds in either window.
#[test]
fn all_six_papers_and_windows_work() {
    assert_eq!(Paper::resolved(None, false), Paper::White);
    assert_eq!(Paper::resolved(None, true), Paper::Dark);
    for paper in Paper::ALL {
        assert_eq!(Paper::resolved(Some(paper), false), paper);
        assert_eq!(Paper::resolved(Some(paper), true), paper);
        assert_eq!(Paper::from_name(paper.name()), Some(paper));
        assert!(!paper.label().is_empty());
    }
    assert_eq!(Paper::from_name("sepia"), None);
}
