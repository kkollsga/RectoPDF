use super::*;
use recto_core::TocLine;
use recto_core::nav::{LabelRange, LabelStyle, PageLabels};
use std::cell::Cell;

mod compare_tests;
mod contents;
mod editing;
mod files;
mod journal;
mod pages;
mod restore;
mod saving;
mod sleep_tests;
mod tabs;
mod tiers;
mod trail_tests;
mod view;

struct FakeLoader {
    pages: u32,
    loads: Cell<u32>,
}

impl DocLoader for FakeLoader {
    fn load(&self, path: &Path) -> Result<DocFacts, OpenError> {
        self.loads.set(self.loads.get() + 1);
        if path.to_string_lossy().contains("missing") {
            return Err(OpenError::NotFound);
        }
        Ok(DocFacts {
            page_count: self.pages,
            byte_len: 1,
            encrypted: false,
            version: "1.7".into(),
            title: None,
            toc: Toc::default(),
            renderer_only: false,
        })
    }
}

fn session(pages: u32) -> Session<FakeLoader> {
    Session::new(FakeLoader { pages, loads: Cell::new(0) })
}

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

/// Failure: in a short contents area, a place that's just been brought to its anchor is outside the dead zone, so it moves again at every section, or for ever. Wherever the list is short, landing on the anchor is at rest.
#[test]
fn the_dead_zone_holds_in_a_short_contents_area() {
    for height in [CONTENTS_ROW_HEIGHT, 56.0, 100.0, 168.0, 300.0, 420.0] {
        let mut follow = ContentsFollow::default();
        let view = |scroll_y: f64| ContentsView { scroll_y, height, rows: 2000 };
        follow.reached(Some(0), view(0.0), 0.0, false);
        assert_eq!(follow.reached(Some(500), view(0.0), 1.0, false), FollowMove::Move, "height {height}");
        let landed = *run_frames(&mut follow, 0.0).last().unwrap();
        assert!(!follow.is_moving());
        // The same section again, and the next: nothing moves until the place has really left.
        assert_eq!(follow.reached(Some(500), view(landed), 2.0, false), FollowMove::Stay, "height {height}");
        let place = 500.5 * CONTENTS_ROW_HEIGHT;
        assert!(
            place >= landed + height * FOLLOW_DEAD_ZONE.0 && place <= landed + height * FOLLOW_DEAD_ZONE.1,
            "height {height}: {place} from {landed}"
        );
    }
}

const HEIGHT: f64 = 560.0;
const ROWS: u32 = 1000;

fn contents_view(scroll_y: f64) -> ContentsView {
    ContentsView {
        scroll_y,
        height: HEIGHT,
        rows: ROWS,
    }
}

fn anchor_y(row: u32) -> f64 {
    (f64::from(row) + 0.5) * CONTENTS_ROW_HEIGHT - HEIGHT * FOLLOW_ANCHOR
}

/// Runs a move to its end, at 60 frames a second, and returns each position.
/// Failure: a window's resize counts as a change of place, so a list the reader had scrolled themselves snaps back once they've stopped. It stays where they put it.
#[test]
fn a_resize_leaves_a_list_the_reader_scrolled_alone() {
    let mut follow = ContentsFollow::default();
    let view = ContentsView {
        scroll_y: 0.0,
        height: 400.0,
        rows: 200,
    };
    follow.reached(Some(50), view, 0.0, false);
    follow.user_scrolled(1.0);
    follow.view_changed();
    let later = ContentsView {
        scroll_y: 2000.0,
        height: 300.0,
        rows: 200,
    };
    assert_eq!(
        follow.reached(Some(50), later, 10.0, false),
        FollowMove::Stay,
        "the same section, and it's been left alone"
    );
    assert!(!follow.is_following(), "still theirs until the section changes");
}

/// Failure: folding leaves a move running, so opening the contents again whooshes from the top. Opened, the place is straight where it should be.
#[test]
fn folding_ends_a_move_and_opening_goes_straight_there() {
    let mut follow = ContentsFollow::default();
    let view = |scroll_y: f64| ContentsView {
        scroll_y,
        height: 400.0,
        rows: 200,
    };
    follow.reached(Some(0), view(0.0), 0.0, false);
    assert_eq!(follow.reached(Some(100), view(0.0), 1.0, false), FollowMove::Move);
    assert_eq!(follow.reached(None, view(0.0), 1.1, false), FollowMove::Stay);
    assert!(!follow.is_moving(), "nothing's left running");
    assert_eq!(follow.step(0.0, 1.0 / 60.0), None);
    assert!(matches!(follow.reached(Some(100), view(0.0), 2.0, false), FollowMove::SetTo(_)));
    assert!(!follow.is_moving());
}

/// The share of steps with the place in the view, with sections going by at this many rows a second.
fn place_in_view(rows_per_second: f64, seconds: f64) -> f64 {
    const DT: f64 = 1.0 / 120.0;
    let mut follow = ContentsFollow::default();
    let (height, rows) = (680.0, 100_000);
    let mut position = 0.0;
    follow.reached(
        Some(0),
        ContentsView {
            scroll_y: position,
            height,
            rows,
        },
        0.0,
        false,
    );
    let steps = (seconds / DT) as u32;
    let mut shown = 0;
    for step in 0..steps {
        let now = f64::from(step) * DT;
        let row = (now * rows_per_second) as u32;
        follow.reached(
            Some(row),
            ContentsView {
                scroll_y: position,
                height,
                rows,
            },
            now,
            false,
        );
        if let Some(next) = follow.step(position, DT) {
            position = next;
        }
        let place = (f64::from(row) + 0.5) * CONTENTS_ROW_HEIGHT;
        if (position..=position + height).contains(&place) {
            shown += 1;
        }
    }
    f64::from(shown) / f64::from(steps)
}

/// Failure: scrolling fast, the view trails past the place: the target only moves once the place has passed the last target's zone, and a move never gets quicker, so at 40 pages a second the place is out of the list about one step in ten, on `rfc9110` as measured. The first moment aside, it stays in view at any speed a reader can scroll.
#[test]
fn the_place_stays_in_view_at_any_speed() {
    for rows_per_second in [20.0, 120.0, 400.0] {
        let share = place_in_view(rows_per_second, 5.0);
        assert!(share >= 0.97, "{rows_per_second} rows a second: in view {share}");
    }
}

/// Failure: a chapter closes just after a reposition. The place drops up the list by the rows that went, past the dead zone, and the contents move straight back, so one change of section makes two moves in opposite directions.
#[test]
fn a_chapter_closing_above_the_place_makes_no_second_move() {
    let mut follow = ContentsFollow::default();
    let (height, rows) = (680.0, 40);
    let view = |scroll_y: f64, rows: u32| ContentsView { scroll_y, height, rows };
    assert!(matches!(follow.reached(Some(0), view(0.0, rows), 0.0, false), FollowMove::SetTo(_)));
    assert_eq!(follow.reached(Some(25), view(0.0, rows), 1.0, false), FollowMove::Move);
    let landed = *run_frames(&mut follow, 0.0).last().unwrap();
    // Chapter at row 14 closes its 11 sections, and the next chapter, row 26, becomes row 15.
    let change = ContentsRowChange {
        from: 15,
        removed: 12,
        inserted: 1,
        place_shift: -11,
    };
    let by = follow.row_change(change, landed).expect("the scroll takes it up, with no slide to see");
    assert!(by < 0.0);
    follow.shifted(by);
    assert_eq!(
        follow.reached(Some(15), view(landed + by, rows - 11), 2.0, false),
        FollowMove::Stay,
        "the place is where it was on screen"
    );
}

/// Failure: rows change under a reader who has scrolled the contents themselves, and what they're looking at is taken from them. Only rows above the view move the scroll then, and only by what went.
#[test]
fn a_reader_in_the_contents_keeps_what_they_are_looking_at() {
    let mut follow = ContentsFollow::default();
    follow.reached(
        Some(0),
        ContentsView {
            scroll_y: 0.0,
            height: 680.0,
            rows: 40,
        },
        0.0,
        false,
    );
    follow.user_scrolled(1.0);
    let closes_in_view = ContentsRowChange {
        from: 15,
        removed: 12,
        inserted: 1,
        place_shift: -11,
    };
    assert_eq!(follow.row_change(closes_in_view, 280.0), None, "in the view, they slide");
    assert_eq!(
        follow.row_change(closes_in_view, 28.0 * 30.0),
        Some(-11.0 * 28.0),
        "above the view, it can't be seen"
    );
}

fn run_frames(follow: &mut ContentsFollow, mut position: f64) -> Vec<f64> {
    let mut path = vec![position];
    for _ in 0..2000 {
        let Some(next) = follow.step(position, 1.0 / 60.0) else { return path };
        position = next;
        path.push(position);
    }
    panic!("a move never ended, so frames would run for ever: {path:?}");
}

fn placed_at(row: u32) -> (ContentsFollow, f64) {
    let mut follow = ContentsFollow::default();
    let FollowMove::SetTo(y) = follow.reached(Some(row), contents_view(0.0), 100.0, false) else {
        panic!("the first showing goes straight there")
    };
    (follow, y)
}

/// Failure: the list moves at every page turn, which is what the owner asked not to see. Across a slow scripted scroll, with the place inside the dead zone, it doesn't move by one pixel: asked at every section, the answer is always to stay.
#[test]
fn inside_the_dead_zone_nothing_moves_at_all() {
    let (mut follow, y) = placed_at(100);
    assert!((y - anchor_y(100)).abs() < 0.001);
    // Forward to the zone's edge. 45% of 560 points is 9 rows.
    for row in 101..=108 {
        assert_eq!(follow.reached(Some(row), contents_view(y), 100.0, false), FollowMove::Stay, "row {row}");
        assert!(!follow.is_moving());
    }
    // And back, by a little.
    for row in (97..108).rev() {
        assert_eq!(follow.reached(Some(row), contents_view(y), 100.0, false), FollowMove::Stay, "row {row}");
    }
}

/// Failure: reading on nudges the list at every section, or it never comes back. Leaving the dead zone starts one move, back to the anchor, and then it rests again for a useful stretch.
#[test]
fn leaving_the_dead_zone_makes_one_move_and_then_rests() {
    let (mut follow, mut y) = placed_at(100);
    let mut moves = Vec::new();
    for row in 101..200 {
        if follow.reached(Some(row), contents_view(y), 100.0, false) == FollowMove::Move {
            moves.push(row);
            let path = run_frames(&mut follow, y);
            y = *path.last().unwrap();
            assert!((y - anchor_y(row)).abs() < 0.001, "row {row}: at {y}");
        }
    }
    assert!(moves.len() >= 5, "{moves:?}");
    for pair in moves.windows(2) {
        assert!(pair[1] - pair[0] >= 8, "a rest of only {} rows: {moves:?}", pair[1] - pair[0]);
    }
}

/// Failure: a move starts or stops with a jolt. Over every move, near and far, speed builds from about nothing and fades to about nothing, and never changes much from one frame to the next, as a share of its peak.
#[test]
fn a_move_starts_and_ends_softly() {
    for row in [112, 150, 600] {
        let (mut follow, y) = placed_at(100);
        assert_eq!(follow.reached(Some(row), contents_view(y), 100.0, false), FollowMove::Move, "row {row}");
        let path = run_frames(&mut follow, y);
        let speeds: Vec<f64> = path.windows(2).map(|pair| (pair[1] - pair[0]) * 60.0).collect();
        let peak = speeds.iter().fold(0.0f64, |peak, speed| peak.max(speed.abs()));
        let (first, last) = (speeds[0].abs(), speeds[speeds.len() - 2].abs());
        assert!(first < peak * 0.15, "row {row}: starts at {first} of {peak}");
        assert!(last < peak * 0.15, "row {row}: ends at {last} of {peak}");
        // No step: how much the speed changes doesn't itself change suddenly from one frame to the next.
        let changes: Vec<f64> = speeds.windows(2).map(|pair| pair[1] - pair[0]).collect();
        for pair in changes.windows(2).take(changes.len().saturating_sub(2)) {
            assert!((pair[1] - pair[0]).abs() < peak * 0.2, "row {row}: {} then {}, of {peak}", pair[0], pair[1]);
        }
        assert!(speeds.iter().all(|speed| *speed >= -peak * 0.01), "row {row}: it went backwards");
        let seconds = path.len() as f64 / 60.0;
        assert!(
            seconds >= FOLLOW_SECONDS.0 * 0.7 && seconds <= FOLLOW_SECONDS.1 * 1.25,
            "row {row}: {seconds} s"
        );
    }
}

/// Failure: reading quickly restarts the move at each section, so it stutters to a stop and starts again. A new target mid-move leaves the speed where it was, and many in a row are one long move with no stop in it.
#[test]
fn a_new_target_mid_move_has_no_step_in_speed() {
    let (mut follow, mut y) = placed_at(100);
    assert_eq!(follow.reached(Some(115), contents_view(y), 100.0, false), FollowMove::Move);
    let mut speed = 0.0;
    for _ in 0..12 {
        let next = follow.step(y, 1.0 / 60.0).unwrap();
        speed = (next - y) * 60.0;
        y = next;
    }
    assert!(speed > 50.0, "it's moving by now: {speed}");
    assert_eq!(follow.reached(Some(140), contents_view(y), 100.0, false), FollowMove::Move);
    let next = follow.step(y, 1.0 / 60.0).unwrap();
    let after = (next - y) * 60.0;
    assert!((after - speed).abs() < speed * 0.25 + 30.0, "{speed} then {after}");

    // 40 sections a second for two seconds is one move, not forty.
    let (mut follow, mut y) = placed_at(100);
    let mut stopped_on = 0;
    let mut row = 100.0;
    for _ in 0..120 {
        row += 40.0 / 60.0;
        follow.reached(Some(row as u32), contents_view(y), 100.0, false);
        match follow.step(y, 1.0 / 60.0) {
            Some(next) => y = next,
            None => stopped_on += u32::from(row > 110.0),
        }
    }
    assert_eq!(stopped_on, 0, "the move kept going");
    let place = (row + 0.5) * CONTENTS_ROW_HEIGHT;
    assert!(place >= y && place <= y + HEIGHT, "and the place is showing: {place} in {y}");
}

/// Failure: a far jump ends somewhere near, or stops short. It's allowed to whoosh, and it lands exactly on the anchor, in the bounded time.
#[test]
fn a_far_jump_lands_exactly_on_the_anchor() {
    let (mut follow, y) = placed_at(10);
    assert_eq!(follow.reached(Some(800), contents_view(y), 100.0, false), FollowMove::Move);
    let path = run_frames(&mut follow, y);
    assert_eq!(*path.last().unwrap(), anchor_y(800));
    assert!(path.len() as f64 / 60.0 <= FOLLOW_SECONDS.1 * 1.25, "{} frames", path.len());
}

/// Failure: at the ends of the list, where the anchor can't be reached, a move runs past what can be scrolled, or keeps trying and never ends, or the first sections move at all.
#[test]
fn the_first_and_last_sections_stop_at_the_end_of_the_list() {
    let (mut follow, y) = placed_at(0);
    assert_eq!(y, 0.0, "the top of the list");
    for row in 1..=8 {
        assert_eq!(follow.reached(Some(row), contents_view(0.0), 100.0, false), FollowMove::Stay, "row {row}");
    }
    let furthest = f64::from(ROWS) * CONTENTS_ROW_HEIGHT - HEIGHT;
    let (mut follow, y) = placed_at(900);
    assert_eq!(follow.reached(Some(999), contents_view(y), 100.0, false), FollowMove::Move);
    let path = run_frames(&mut follow, y);
    assert_eq!(*path.last().unwrap(), furthest);
    assert!(path.iter().all(|at| *at <= furthest + 0.5), "it ran past the end");
    assert_eq!(
        follow.reached(Some(998), contents_view(furthest), 100.0, false),
        FollowMove::Stay,
        "and it's done"
    );
}

/// Failure: a slow end never arrives, because a scroll view rounds each position to a whole device pixel, and the small step from there rounds back to the same place. Frames ran for ever, seen in the first build. Given only rounded positions, it must still end, in place.
#[test]
fn rounding_by_the_host_doesnt_stop_a_move() {
    let (mut follow, mut y) = placed_at(100);
    assert_eq!(follow.reached(Some(112), contents_view(y), 100.0, false), FollowMove::Move);
    for _ in 0..2000 {
        let Some(next) = follow.step(y, 1.0 / 60.0) else {
            assert!((y - anchor_y(112)).abs() <= 0.5, "{y}");
            return;
        };
        y = (next * 2.0).round() / 2.0;
    }
    panic!("it never arrived: {y}");
}

/// Failure: reading pulls the list out from under someone looking through it. A scroll stops any move where it is, and nothing moves afterwards, until they've stopped using it.
#[test]
fn a_reader_scrolling_the_contents_pauses_following() {
    let (mut follow, y) = placed_at(100);
    follow.reached(Some(115), contents_view(y), 100.0, false);
    assert!(follow.is_moving());
    follow.user_scrolled(101.0);
    assert!(!follow.is_moving() && !follow.is_following());
    assert_eq!(follow.step(y, 1.0 / 60.0), None, "stopped right there");
    assert_eq!(follow.reached(Some(300), contents_view(y), 102.0, false), FollowMove::Stay, "still using it");
}

/// Failure: following never comes back, or comes back while the pointer is still on the list. It takes the pointer out, the time passed, and a new section, all three.
#[test]
fn following_resumes_only_when_they_have_stopped_and_the_section_changes() {
    let (mut follow, y) = placed_at(100);
    follow.pointer(true);
    follow.user_scrolled(100.0);
    let later = 100.0 + FOLLOW_RESUME_AFTER + 5.0;
    assert_eq!(
        follow.reached(Some(120), contents_view(y), later, false),
        FollowMove::Stay,
        "the pointer's on it"
    );
    follow.pointer(false);
    assert!(!follow.is_following(), "not until the section changes");
    assert_eq!(
        follow.reached(Some(120), contents_view(y), later, false),
        FollowMove::Stay,
        "the same section, so nothing's changed"
    );
    let (mut soon, y) = placed_at(100);
    soon.user_scrolled(100.0);
    assert_eq!(
        soon.reached(Some(120), contents_view(y), 100.0 + FOLLOW_RESUME_AFTER - 0.5, false),
        FollowMove::Stay,
        "too soon"
    );
    assert_eq!(follow.reached(Some(125), contents_view(y), later, false), FollowMove::Move);
    assert!(follow.is_following());
}

/// Failure: a pointer resting on the list has it move from under it, though they haven't scrolled.
#[test]
fn nothing_moves_under_a_resting_pointer() {
    let (mut follow, y) = placed_at(100);
    follow.reached(Some(115), contents_view(y), 100.0, false);
    follow.pointer(true);
    assert!(!follow.is_moving(), "what was moving stops");
    assert_eq!(follow.reached(Some(130), contents_view(y), 100.0, false), FollowMove::Stay);
    follow.pointer(false);
    assert_eq!(
        follow.reached(Some(135), contents_view(y), 100.0, false),
        FollowMove::Move,
        "and picks up on the next section"
    );
}

/// Failure: Reduce Motion gets an animation anyway, or the place is lost. No frames at all, nothing moves while the place shows, and it goes straight there when it doesn't.
#[test]
fn reduce_motion_never_animates() {
    let (mut follow, y) = placed_at(100);
    assert_eq!(
        follow.reached(Some(112), contents_view(y), 100.0, true),
        FollowMove::Stay,
        "past the dead zone, but still showing"
    );
    assert!(matches!(follow.reached(Some(400), contents_view(y), 100.0, true), FollowMove::SetTo(_)));
    assert!(!follow.is_moving());
}

/// Failure: another tab's contents swoop in from wherever the last tab's were. They start in place.
#[test]
fn new_contents_start_in_place() {
    let (mut follow, _) = placed_at(100);
    follow.new_contents();
    assert!(matches!(follow.reached(Some(500), contents_view(0.0), 100.0, false), FollowMove::SetTo(_)));
}

/// Failure: the contents come back where the place would put them, not where they were, or forget the reader had scrolled them. And a hostile number isn't taken.
#[test]
fn the_contents_come_back_as_they_were() {
    let mut follow = ContentsFollow::default();
    follow.resume_at(280.0, false);
    assert_eq!(follow.reached(Some(100), contents_view(0.0), 0.0, false), FollowMove::SetTo(280.0));
    assert!(!follow.is_following(), "they were the reader's");
    let mut follow = ContentsFollow::default();
    follow.resume_at(f64::NAN, false);
    assert!(follow.is_following());
    follow.resume_at(1e12, true);
    let furthest = f64::from(ROWS) * CONTENTS_ROW_HEIGHT - HEIGHT;
    assert_eq!(follow.reached(Some(100), contents_view(0.0), 0.0, false), FollowMove::SetTo(furthest));
}

/// Failure: a shorter window leaves the place below the bottom of the contents, with nothing to bring it back until the section changes. Seen in the first short-window shot.
#[test]
fn a_change_of_height_brings_the_place_back() {
    let (mut follow, y) = placed_at(100);
    follow.view_changed();
    assert!(matches!(
        follow.reached(
            Some(100),
            ContentsView {
                scroll_y: y,
                height: 120.0,
                rows: 1000
            },
            100.0,
            false
        ),
        FollowMove::SetTo(_)
    ));
}

/// Failure: rows opening above the place throw a move off, by the number of rows the host just moved past.
#[test]
fn rows_changing_above_carry_the_move_with_them() {
    let (mut follow, y) = placed_at(100);
    follow.reached(Some(115), contents_view(y), 100.0, false);
    follow.shifted(-112.0);
    let path = run_frames(&mut follow, y - 112.0);
    assert!((path.last().unwrap() - (anchor_y(115) - 112.0)).abs() < 1.0, "{path:?}");
}

/// Failure: a field added carelessly raises the cost of every visible row.
#[test]
fn sidebar_row_stays_small() {
    assert!(std::mem::size_of::<SidebarRow<'_>>() <= 64);
}

fn journaled(pages: u32) -> Session<FakeLoader> {
    let mut s = session(pages);
    s.set_journaling(true);
    s
}

/// What the journal-writing thread does, into plain memory: what comes back from a crash is whatever it had got to.
fn write_journal(s: &mut Session<FakeLoader>, files: &mut HashMap<String, Vec<u8>>) {
    let work = s.journal_work();
    for name in work.retired {
        files.remove(&name);
    }
    let mut outcome = JournalOutcome::default();
    for append in work.appends {
        if let Some(base) = &append.base {
            let header = JournalHeader {
                base_seq: append.through,
                ..append.header.clone()
            };
            files.insert(append.name.clone(), record::journal_start(&header, Some(base)));
            if let Some(old) = append.replaces.as_ref().filter(|old| **old != append.name) {
                files.remove(old);
            }
        } else {
            let file = files.entry(append.name.clone()).or_insert_with(|| record::journal_start(&append.header, None));
            file.extend_from_slice(&record::journal_lines(&append.ops));
        }
        outcome.written.push((append.tab, append.through, append.name));
    }
    s.journal_outcome(outcome);
}

fn back_from(files: &HashMap<String, Vec<u8>>) -> HashMap<String, RestoredEdits> {
    files
        .iter()
        .map(|(name, bytes)| {
            let parsed = record::parse_journal(bytes, record::CURRENT_VERSION).unwrap();
            let (edits, applied) = PageEdits::replay(parsed.header.file_pages, parsed.base.as_ref(), &parsed.ops).unwrap();
            assert_eq!(applied, parsed.ops.len());
            (
                name.clone(),
                RestoredEdits {
                    edits,
                    seq: parsed.header.base_seq + applied as u64,
                    rolled_off: false,
                    lost_lines: parsed.lost_lines,
                },
            )
        })
        .collect()
}

fn kept(name: &str, size: u64) -> edit::AddedSource {
    edit::AddedSource {
        path: PathBuf::from(format!("/state/journals/{name}")).into_boxed_path(),
        page_count: 5,
        stamp: Some(FileStamp {
            size,
            mtime_ns: 1,
            device: 1,
            inode: 7,
        }),
        kept: true,
        protected: false,
        holds: None,
    }
}

const KEPT_A: &str = "kept-00000000000000aa.pdf";
const KEPT_B: &str = "kept-00000000000000bb.pdf";

fn at(path: &str) -> FileStatus {
    FileStatus::Moved(
        PathBuf::from(path),
        FileStamp {
            size: 1,
            mtime_ns: 1,
            device: 1,
            inode: 1,
        },
    )
}

/// Twelve pages: `i` to `iv`, then `1` to `8`.
struct Labelled;

impl DocLoader for Labelled {
    fn load(&self, _: &Path) -> Result<DocFacts, OpenError> {
        let mut toc = Toc::default();
        toc.labels = PageLabels::from_ranges(
            vec![
                LabelRange {
                    start: 0,
                    first: 1,
                    style: LabelStyle::RomanLower,
                    prefix: "".into(),
                },
                LabelRange {
                    start: 4,
                    first: 1,
                    style: LabelStyle::Decimal,
                    prefix: "".into(),
                },
            ],
            false,
        );
        Ok(DocFacts {
            page_count: 12,
            byte_len: 1,
            encrypted: false,
            version: "1.7".into(),
            title: None,
            toc,
            renderer_only: false,
        })
    }
}
