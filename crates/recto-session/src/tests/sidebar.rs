use super::*;

/// Failure: a shortened name has no way to be read in full, or a hostile title takes over the hover: 100,000 characters, newlines and invisible characters. It comes back on one clean line, bounded, and says what it is.
#[test]
fn a_rows_full_text_is_clean_and_bounded() {
    let plain = row_full_text("QUIC: A UDP-Based Multiplexed and Secure Transport", false);
    assert_eq!(plain.text, "QUIC: A UDP-Based Multiplexed and Secure Transport");
    assert!(!plain.is_file_name);
    assert!(row_full_text("report", true).is_file_name);
    let hostile = format!("{}{}", "line\n".repeat(20_000), "\u{202E}\u{E0041}end");
    let shown = row_full_text(&hostile, false);
    assert!(shown.text.chars().count() <= MAX_ROW_FULL_TEXT + 1, "{}", shown.text.chars().count());
    assert!(shown.text.ends_with('…'));
    assert!(!shown.text.chars().any(char::is_control), "one line");
    assert!(!shown.text.contains('\u{202E}'));
    assert_eq!(row_full_text("\n\n", false).text, "");
}

/// Failure: a long outline pushes the other tabs or the bottom bar off screen, the reason the owner asked for this. Holds at every window height down to the smallest, for every mix of tabs and rows: the tabs kept never lose their room, and nothing goes past the room there is unless the tab list itself is too long, when that scrolls.
#[test]
fn a_long_outline_never_pushes_the_other_tabs_away() {
    for room in [0.0, 50.0, 100.0, 208.0, 328.0, 700.0, 1400.0] {
        for tabs in [1u32, 2, 3, 6, 9, 200, 10_000] {
            for rows in [0u32, 1, 3, 40, 10_000, 8_000_000] {
                let heights = sidebar_heights(room, tabs, rows);
                assert!(heights.contents >= 0.0 && heights.contents <= f64::from(rows) * CONTENTS_ROW_HEIGHT);
                assert!(heights.contents <= room * CONTENTS_MAX_SHARE + 1e-9, "{room} {tabs} {rows}: {heights:?}");
                if f64::from(tabs) * TAB_ROW_HEIGHT + heights.contents <= room {
                    assert!(
                        !heights.tabs_scroll,
                        "{room} {tabs} {rows}: all of it fits, so nothing scrolls but the contents"
                    );
                }
            }
        }
    }
}

/// Failure: a long outline pushes the other tabs and the bottom bar off, as a rule of "what the tabs leave" did with one tab: the owner saw no maximum at all. It stops at the maximum however few tabs there are, a short outline takes only what it needs, and the tabs scroll only if they don't fit in what's left.
#[test]
fn the_contents_have_a_true_maximum() {
    let most = 700.0 * CONTENTS_MAX_SHARE;
    assert_eq!(sidebar_heights(700.0, 1, 1000).contents, most, "one tab, and it still stops");
    assert_eq!(sidebar_heights(700.0, 9, 1000).contents, most);
    assert_eq!(sidebar_heights(700.0, 2, 3).contents, 3.0 * CONTENTS_ROW_HEIGHT, "only what it needs");
    assert!(!sidebar_heights(700.0, 5, 1000).tabs_scroll, "what's left holds these");
    assert!(sidebar_heights(700.0, 40, 1000).tabs_scroll, "and these scroll in it");
    assert!(
        sidebar_heights(100.0, 1, 1000).contents <= 100.0 - TAB_ROW_HEIGHT,
        "a short window keeps the tab's own row"
    );
    assert_eq!(sidebar_heights(f64::NAN, 2, 1000).contents, 0.0);
    assert_eq!(sidebar_heights(10.0, 1, 1000).contents, 0.0);
    // A note over them comes out of the same share, so the tabs keep what they had.
    let noted = sidebar_heights_with_note(700.0, 1, 1000, true);
    assert_eq!(noted.note, NOTE_ROW_HEIGHT);
    assert!((noted.contents + noted.note - most).abs() < 1e-9, "{noted:?}");
    assert_eq!(
        sidebar_heights_with_note(700.0, 1, 0, true).contents,
        0.0,
        "a note alone, for a file with no contents"
    );
    assert_eq!(sidebar_heights_with_note(40.0, 1, 10, true).note, 0.0, "no room for it");
}

/// Failure: a field added carelessly raises the cost of every visible row.
#[test]
fn sidebar_row_stays_small() {
    assert!(std::mem::size_of::<SidebarRow<'_>>() <= 64);
}
