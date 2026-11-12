use super::*;
use recto_core::TocLine;
use recto_core::nav::{LabelRange, LabelStyle, PageLabels};
use std::cell::Cell;

mod contents;
mod editing;
mod journal;
mod pages;
mod saving;
mod sleep_tests;
mod tabs;
mod tiers;
mod trail_tests;

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

/// Failure: a field added carelessly raises the cost of every remembered draw.
#[test]
fn draw_note_stays_small() {
    assert!(std::mem::size_of::<DrawNote>() <= 32);
}

/// Failure: wait says a page is drawn when a different page now has that number, because notes are by position and outlive edits.
#[test]
fn draw_notes_go_when_the_pages_move() {
    let mut s = session(5);
    let id = s.open(Path::new("/d/a.pdf")).unwrap();
    let note = DrawNote {
        tab: id,
        page: 0,
        failure: None,
    };
    s.note_drawn(note);
    s.edit_pages(id, |e| e.move_pages(0, 1, 2)).unwrap();
    assert_eq!(s.draw_note(id, 0), None, "after an edit");
    s.note_drawn(note);
    assert!(s.undo_edit(id));
    assert_eq!(s.draw_note(id, 0), None, "after an undo");
    s.note_drawn(note);
    assert!(s.redo_edit(id));
    assert_eq!(s.draw_note(id, 0), None, "after a redo");
    s.note_drawn(note);
    s.discard_edits(id);
    assert_eq!(s.draw_note(id, 0), None, "after a discard");
    s.note_drawn(note);
    assert_eq!(s.edit_pages(id, |e| e.rotate(99, 1, 1)), Err(SessionError::Edit(EditError::OutOfRange)));
    assert_eq!(s.draw_note(id, 0), Some(note), "a refused edit changes nothing");
}

/// Failure: the draw notes grow with a long session, or an old result survives its tab, so a waiter is told a closed tab's page is drawn.
#[test]
fn draw_notes_are_capped_and_go_with_their_tab() {
    let mut s = session(5);
    let a = s.open(Path::new("/d/a.pdf")).unwrap();
    for page in 0..(MAX_DRAWS_KEPT as u32 * 2) {
        s.note_drawn(DrawNote { tab: a, page, failure: None });
    }
    assert_eq!(s.draws.len(), MAX_DRAWS_KEPT);
    assert!(s.draw_note(a, 0).is_none());
    let last = MAX_DRAWS_KEPT as u32 * 2 - 1;
    assert!(s.draw_note(a, last).is_some());
    s.close(a).unwrap();
    assert!(s.draw_note(a, last).is_none());
}

/// Failure: a field added carelessly raises the cost of every visible row.
#[test]
fn sidebar_row_stays_small() {
    assert!(std::mem::size_of::<SidebarRow<'_>>() <= 64);
}

/// Failure: a PDF moved away after opening crashes or disappears, rather than staying open and saying why.
#[test]
fn a_tab_whose_file_vanished_stays_and_says_why() {
    let mut s = session(10);
    let gone = s.open(Path::new("/d/missing-later.pdf"));
    assert!(gone.is_err());
    let ids: Vec<_> = (0..5).map(|i| s.open(Path::new(&format!("/d/{i}.pdf"))).unwrap()).collect();
    // Rename the first tab's file underneath the session, then wake it from cold.
    s.tab_mut(ids[0]).unwrap().file_name = OsStr::new("missing.pdf").into();
    s.tap(ids[0]).unwrap();
    assert_eq!(s.tab(ids[0]).unwrap().problem, Some(OpenError::NotFound));
    assert_eq!(s.active(), Some(ids[0]));
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

fn with_owner_and_agent_edits(s: &mut Session<FakeLoader>) -> (TabId, TabId) {
    let a = s.open(Path::new("/d/a.pdf")).unwrap();
    let b = s.open(Path::new("/d/b.pdf")).unwrap();
    s.edit_pages(a, |e| e.rotate(0, 1, 1)).unwrap();
    s.edit_pages_as(a, Author::Agent, |e| e.delete(4, 1)).unwrap();
    s.edit_pages(a, |e| e.move_pages(0, 1, 1)).unwrap();
    s.edit_pages_as(a, Author::Agent, |e| e.rotate(2, 1, 3)).unwrap();
    assert!(s.undo_edit(a));
    assert!(s.undo_edit(a));
    s.go_to(a, 2).unwrap();
    s.tap(a).unwrap();
    s.tap(a).unwrap();
    (a, b)
}

/// Failure: a record from a build with focus view, written with it on, comes back with the sidebar showing, or the next record still says focus, so it can't be turned off. Owner, 2026-11-08: focus view became the sidebar's button.
#[test]
fn an_older_records_focus_view_comes_back_as_a_hidden_sidebar() {
    let old: record::ViewRecord = serde_json::from_str(r#"{"paper":null,"focus":true,"agent_icon_hidden":false}"#).unwrap();
    let mut saved = journaled(1).to_record();
    saved.view = old;
    let mut back = journaled(1);
    back.restore(&saved, HashMap::new()).unwrap();
    assert!(back.view().sidebar_hidden);
    let written = serde_json::to_string(&back.to_record().view).unwrap();
    assert!(!written.contains("focus") && written.contains("\"sidebar_hidden\":true"), "{written}");
    back.set_view(ViewSettings {
        sidebar_hidden: false,
        ..back.view()
    });
    let mut again = journaled(1);
    again.restore(&back.to_record(), HashMap::new()).unwrap();
    assert!(!again.view().sidebar_hidden, "shown again stays shown");
}

/// Failure: anything about the session is different after going through its record and journals: a tab, its order, the active one, a place, a fold, unsaved edits, the steps undone, or who made them. Compares the whole of both.
#[test]
fn a_session_comes_back_whole_from_its_record_and_journals() {
    let mut s = journaled(5);
    let (a, _b) = with_owner_and_agent_edits(&mut s);
    s.note_place(
        a,
        PlaceRecord {
            page: 2,
            anchor_page: 2,
            within: 0.25,
            x: 3.5,
            y: 1234.5,
            document_height: 9000.0,
            zoom: 1.5625,
            page_width: 612.0,
            page_height: 792.0,
        },
    );
    s.set_find(true, "ran", "typed but not run");
    s.set_edit_mode(true);
    s.set_view_rotation(a, 1);
    assert!(s.set_spread(a, record::Spread::TwoUpCover));
    assert!(!s.set_spread(a, record::Spread::TwoUpCover), "the same again changes nothing");
    s.set_selection(
        a,
        Some(Selection {
            from_page: 1,
            from_char: 2,
            to_page: 2,
            to_char: 9,
        }),
    );
    s.set_view(ViewSettings {
        paper: Some(Paper::Warm),
        sidebar_hidden: true,
        agent_icon_hidden: true,
    });
    let mut files = HashMap::new();
    write_journal(&mut s, &mut files);
    let bytes = record::encode_record(&s.to_record(), 7);
    let read = record::decode_record(&bytes, record::CURRENT_VERSION).unwrap();
    let mut back = journaled(5);
    back.restore(&read.record, back_from(&files)).unwrap();
    assert_eq!(back.to_record(), s.to_record());
    assert_eq!(back.edits_record(a), s.edits_record(a));
    assert_eq!(back.view(), s.view());
    assert_eq!(back.spread_of(a), Some(record::Spread::TwoUpCover), "a tab's layout comes back with it");
    assert_eq!(back.active(), s.active());
    assert_eq!(back.tier(a), Tier::Hot, "it's the active one");
    assert!(back.warm_tabs().next().is_none(), "and nothing's been opened");
    assert_eq!(back.page_edits(a).unwrap().slots(), s.page_edits(a).unwrap().slots());
    // Redo works, after a restart, and whose each step is holds for what's then allowed.
    assert!(back.page_edits(a).unwrap().can_redo());
    assert!(back.has_undone_owner_steps(a), "the owner's move is undone, and still theirs");
    assert!(back.redo_edit(a));
    assert!(!back.has_undone_owner_steps(a));
    assert!(back.redo_edit(a));
    assert!(s.redo_edit(a) && s.redo_edit(a));
    assert_eq!(back.page_edits(a).unwrap().slots(), s.page_edits(a).unwrap().slots());
    // What comes next goes on the same journal, with its count following on.
    back.edit_pages(a, |e| e.rotate(1, 1, 1)).unwrap();
    s.edit_pages(a, |e| e.rotate(1, 1, 1)).unwrap();
    write_journal(&mut back, &mut files);
    let mut third = journaled(5);
    third.restore(&back.to_record(), back_from(&files)).unwrap();
    assert_eq!(third.edits_record(a), s.edits_record(a));
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

/// Failure (review, against decision 23): a changed file with no edits stops saying so as soon as it's been read, as the stamp's moved on and being the same says nothing. With edits, it's about what they were made on, so being back at that is news.
#[test]
fn a_changed_file_goes_on_saying_so_when_its_read() {
    let now = FileStamp {
        size: 2,
        mtime_ns: 2,
        device: 1,
        inode: 3,
    };
    let mut s = session(5);
    let a = s.open(Path::new("/d/a.pdf")).unwrap();
    s.apply_file_status(a, FileStatus::Changed(now));
    assert_eq!(s.tab_notice(a).map(FileNotice::code), Some("file_changed"));
    s.apply_file_status(a, FileStatus::Same(now));
    assert_eq!(
        s.tab_notice(a).map(FileNotice::code),
        Some("file_changed"),
        "it's the new one, and that's what it's saying"
    );
    // Left, it's been said.
    s.open(Path::new("/d/b.pdf")).unwrap();
    s.tap(a).unwrap();
    s.tap(s.tabs()[1].id).unwrap();
    assert_eq!(s.tab_notice(a), None);
    // With edits, being the same again is the news.
    s.tap(a).unwrap();
    s.edit_pages(a, |e| e.rotate(0, 1, 1)).unwrap();
    let was = s.own_stamp(a);
    s.apply_file_status(a, FileStatus::Changed(FileStamp { size: 9, ..now }));
    assert_eq!(s.tab_notice(a).map(FileNotice::code), Some("file_changed"));
    s.apply_file_status(a, FileStatus::Same(was.unwrap_or(now)));
    assert_eq!(s.tab_notice(a), None);
}

/// Failure: restoring a thousand tabs reads a file for each, or holds their contents, or a restored tab costs what an opened one doesn't. Every one of them comes back cold with no load at all.
#[test]
fn restoring_opens_no_file() {
    let mut s = session(5);
    let ids: Vec<TabId> = (0..1000).map(|at| s.open(Path::new(&format!("/d/{at}.pdf"))).unwrap()).collect();
    s.tap(ids[500]).unwrap();
    let record = s.to_record();
    let mut back = session(5);
    back.restore(&record, HashMap::new()).unwrap();
    assert_eq!(back.loader.loads.get(), 0, "nothing read");
    assert_eq!(back.tabs().len(), 1000);
    assert_eq!(back.active(), Some(ids[500]));
    assert!(ids.iter().filter(|id| **id != ids[500]).all(|id| back.tier(*id) == Tier::Cold));
    assert_eq!(back.to_record(), record);
    assert_eq!(back.warm_tabs().count(), 0);
}

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

/// Failure: what became of a file isn't said, or its edits go, or the tab's still pointing at where the file isn't. Each case, with the words a reader sees and the edits kept. A moved file with edits keeps its journal's name, as that's by its identity.
#[test]
fn a_tab_says_what_became_of_its_file_and_keeps_its_edits() {
    let mut s = journaled(5);
    let a = s.open(Path::new("/d/a.pdf")).unwrap();
    s.edit_pages(a, |e| e.rotate(0, 1, 1)).unwrap();
    let now = FileStamp {
        size: 9,
        mtime_ns: 9,
        device: 9,
        inode: 9,
    };
    s.apply_file_status(a, at("/e/moved.pdf"));
    assert_eq!(s.path_of(a).unwrap(), Path::new("/e/moved.pdf"));
    assert_eq!(&*s.tab(a).unwrap().name, "moved");
    assert!(s.tab_notice_message(a).unwrap().contains("moved"));
    assert!(s.is_edited(a));
    s.apply_file_status(a, FileStatus::Changed(now));
    assert_eq!(s.tab_notice(a).unwrap().code(), "file_changed");
    assert_ne!(s.own_stamp(a), Some(now), "edits keep the version they were made on");
    s.apply_file_status(a, FileStatus::Missing);
    assert!(s.tab_notice_message(a).unwrap().contains("edits"));
    assert!(s.is_edited(a));
    assert_eq!(s.sidebar().rows[0].notice, s.tab_notice_message(a));
    s.apply_file_status(a, FileStatus::Same(now));
    assert_eq!(s.tab_notice(a), None, "it came back");
    assert_eq!(s.own_stamp(a), Some(now));
    s.apply_file_status(a, FileStatus::NotLooked);
    assert_eq!(s.own_stamp(a), Some(now), "that says nothing");
    // Clean: changed takes the new version, and it's said once, on the way past.
    let mut clean = session(5);
    let b = clean.open(Path::new("/d/b.pdf")).unwrap();
    clean.open(Path::new("/d/c.pdf")).unwrap();
    clean.apply_file_status(b, FileStatus::Changed(now));
    assert_eq!(clean.own_stamp(b), Some(now));
    clean.tap(b).unwrap();
    assert!(clean.tab_notice(b).is_some());
    let c = clean.tabs()[1].id;
    clean.tap(c).unwrap();
    assert_eq!(clean.tab_notice(b), None, "left, so it's been said");
    // Moved onto a file another tab has: this one's the one that's gone.
    clean.apply_file_status(b, FileStatus::Moved(PathBuf::from("/d/c.pdf"), now));
    assert_eq!(clean.tab_notice(b).unwrap().code(), "file_missing");
    assert_eq!(clean.path_of(b).unwrap(), Path::new("/d/b.pdf"));
}

/// Failure: a journal that can't be read costs the tab silently. The tab comes back and says so, with no edits it can't vouch for.
#[test]
fn a_tab_whose_journal_cant_be_read_says_so() {
    let mut s = journaled(5);
    let a = s.open(Path::new("/d/a.pdf")).unwrap();
    s.edit_pages(a, |e| e.rotate(0, 1, 1)).unwrap();
    let mut files = HashMap::new();
    write_journal(&mut s, &mut files);
    let mut back = journaled(5);
    back.restore(&s.to_record(), HashMap::new()).unwrap();
    assert_eq!(back.tab_notice(a).unwrap().code(), "edits_unreadable");
    assert!(!back.is_edited(a));
    let mut torn = back_from(&files);
    torn.values_mut().next().unwrap().lost_lines = true;
    let mut partly = journaled(5);
    partly.restore(&s.to_record(), torn).unwrap();
    assert_eq!(partly.tab_notice(a).unwrap().code(), "edits_unreadable");
    assert!(partly.is_edited(a), "what could be read is here");
}

/// Failure: restoring turns a hostile record into something that panics later: a page past the end, or an active tab that isn't there. Or a record's tabs with no journal cost it a tab.
#[test]
fn a_restored_page_is_held_to_the_document() {
    let mut s = session(5);
    let a = s.open(Path::new("/d/a.pdf")).unwrap();
    let mut saved = s.to_record();
    saved.tabs[0].page = u32::MAX;
    let mut back = session(5);
    back.restore(&saved, HashMap::new()).unwrap();
    assert_eq!(back.tab(a).unwrap().page, 4);
    saved.active = Some(999);
    assert!(back.restore(&saved, HashMap::new()).is_err());
}

/// Failure: ⌘-click doesn't build a comparison the way Finder's does: a second tab doesn't join, one in it doesn't leave, the last one left isn't alone, a fourth gets in, or a plain click doesn't return to one. And the pane being read isn't the one clicked.
#[test]
fn command_click_builds_a_comparison_and_a_plain_click_ends_it() {
    let mut s = session(10);
    let ids: Vec<TabId> = ["a", "b", "c", "d"]
        .iter()
        .map(|name| s.open(&PathBuf::from(format!("/d/{name}.pdf"))).unwrap())
        .collect();
    s.tap(ids[0]).unwrap();
    assert_eq!(s.toggle_compare(ids[0]), Ok(compare::CompareChange::Nothing), "the only tab showing");
    assert_eq!(s.panes(), vec![ids[0]]);
    assert_eq!(s.toggle_compare(ids[1]), Ok(compare::CompareChange::Added));
    assert_eq!(s.panes(), vec![ids[0], ids[1]]);
    assert_eq!(s.active(), Some(ids[1]), "the one clicked is read");
    assert_eq!(s.toggle_compare(ids[2]), Ok(compare::CompareChange::Added));
    assert_eq!(s.toggle_compare(ids[3]), Err(compare::CompareError::TooMany));
    assert_eq!(s.panes().len(), compare::MAX_PANES);
    assert!(s.focus_pane(ids[0]));
    assert_eq!(s.active(), Some(ids[0]));
    assert!(!s.focus_pane(ids[3]), "not a pane");
    assert_eq!(s.toggle_compare(ids[0]), Ok(compare::CompareChange::Removed));
    assert_eq!(s.panes(), vec![ids[1], ids[2]]);
    assert_eq!(s.active(), Some(ids[1]), "reading moves to a pane that's left");
    assert_eq!(s.toggle_compare(ids[2]), Ok(compare::CompareChange::Ended));
    assert_eq!(s.panes(), vec![ids[1]]);
    assert!(s.comparison().is_none());
    s.compare(&[ids[0], ids[3]], None).unwrap();
    s.tap(ids[2]).unwrap();
    assert!(s.comparison().is_none(), "a plain click returns to one");
    assert_eq!(s.panes(), vec![ids[2]]);
    assert!(s.tab(ids[2]).unwrap().toc_open, "and doesn't fold its contents");
    s.compare(&[ids[0], ids[1]], None).unwrap();
    s.open(&PathBuf::from("/d/e.pdf")).unwrap();
    assert!(s.comparison().is_none(), "opening another PDF shows it alone");
    assert_eq!(s.panes().len(), 1);
}

/// Failure: a tab on screen in a comparison pane goes dormant, by time, budget or pressure, and its pane goes blank or its edits leave memory under it. Panes are on screen, and only what's off screen goes.
#[test]
fn panes_on_screen_dont_go_dormant() {
    let mut s = session(10);
    let ids: Vec<TabId> = ["a", "b", "c"]
        .iter()
        .map(|name| s.open(&PathBuf::from(format!("/d/{name}.pdf"))).unwrap())
        .collect();
    s.compare(&[ids[0], ids[1]], Some(ids[0])).unwrap();
    assert!(!s.can_go_dormant(ids[1]), "a pane not being read is still on screen");
    assert!(!s.dormant_oldest_first().contains(&ids[1]));
    assert!(s.dormant_oldest_first().contains(&ids[2]), "the tab off screen can");
    assert!(!s.go_dormant(ids[1]));
}

/// Failure: closing a pane's tab leaves a pane for a tab that's gone, or a comparison of one, or the reader on a tab that's off screen rather than a pane that's left.
#[test]
fn closing_a_panes_tab_takes_its_pane() {
    let mut s = session(10);
    let ids: Vec<TabId> = ["a", "b", "c", "d"]
        .iter()
        .map(|name| s.open(&PathBuf::from(format!("/d/{name}.pdf"))).unwrap())
        .collect();
    s.compare(&[ids[1], ids[2], ids[3]], Some(ids[2])).unwrap();
    s.close(ids[2]).unwrap();
    assert_eq!(s.panes(), vec![ids[1], ids[3]]);
    assert_eq!(s.active(), Some(ids[1]), "to a pane, not the next tab along");
    s.close(ids[1]).unwrap();
    assert!(s.comparison().is_none(), "one left is alone");
    assert_eq!(s.panes(), vec![ids[3]]);
    s.compare(&[ids[0], ids[3]], Some(ids[0])).unwrap();
    s.close(ids[3]).unwrap();
    assert_eq!(s.panes(), vec![ids[0]]);
}

/// Failure: a comparison doesn't come back after a restart exactly: its tabs, their order, synced scroll and each pane's start; or a record naming a tab that isn't there, or of one, or of four, gets in.
#[test]
fn a_comparison_comes_back_from_the_record_exactly() {
    let mut s = session(40);
    let ids: Vec<TabId> = ["a", "b", "c"]
        .iter()
        .map(|name| s.open(&PathBuf::from(format!("/d/{name}.pdf"))).unwrap())
        .collect();
    s.go_to(ids[2], 7).unwrap();
    s.go_to(ids[0], 3).unwrap();
    s.compare(&[ids[2], ids[0]], Some(ids[0])).unwrap();
    assert!(s.set_compare_synced(true).is_some());
    let bytes = record::encode_record(&s.to_record(), 3);
    let read = record::decode_record(&bytes, record::CURRENT_VERSION).unwrap();
    let mut back = session(40);
    back.restore(&read.record, HashMap::new()).unwrap();
    assert_eq!(back.comparison(), s.comparison());
    assert_eq!(back.comparison().unwrap().sync_from, vec![7, 3]);
    assert_eq!(back.active(), Some(ids[0]));
    assert_eq!(back.to_record(), s.to_record());
    for tabs in [vec![ids[0].0], vec![ids[0].0, 99], vec![ids[0].0, ids[0].0], vec![1, 2, 3, 3]] {
        let mut bad = s.to_record();
        bad.comparison = Some(record::ComparisonRecord {
            tabs,
            synced: false,
            sync_from: Vec::new(),
        });
        assert!(record::validate_record(&bad).is_err(), "{:?}", bad.comparison);
    }
    let mut uneven = s.to_record();
    uneven.comparison.as_mut().unwrap().sync_from = vec![1];
    assert!(record::validate_record(&uneven).is_err(), "a start for each pane, or none");
    // An older record has none, and reads.
    let mut older = s.to_record();
    older.comparison = None;
    let json = serde_json::to_string(&older).unwrap();
    assert!(!json.contains("comparison"), "nothing written for an older app to trip on");
}

/// Failure (review 1): a comparison with cold tabs leaves more live documents than SPEC §2 allows, as only the pane being read was made warm and the tabs warm before stayed; or making them warm reads their files with the session held. Each pane is one of the warm tabs, the rest go cold, and what's read ahead is all that's read.
#[test]
fn a_comparison_of_cold_tabs_is_the_live_documents_and_reads_nothing_held() {
    let mut s = session(10);
    let ids: Vec<TabId> = ["a", "b", "c", "d", "e"]
        .iter()
        .map(|name| s.open(&PathBuf::from(format!("/d/{name}.pdf"))).unwrap())
        .collect();
    assert_eq!(s.active(), Some(ids[4]));
    let panes = [ids[0], ids[1], ids[2]];
    let cold = s.wake_for_compare(&panes);
    assert_eq!(
        cold.iter().map(|(id, _)| *id).collect::<Vec<_>>(),
        vec![ids[0], ids[1]],
        "the warm one needs no read"
    );
    for (id, _) in cold {
        let facts = s.loader().load(Path::new("/d/x.pdf"));
        s.give_woken(id, facts, None);
    }
    let loads = s.loader().loads.get();
    s.compare(&panes, None).unwrap();
    assert_eq!(s.loader().loads.get(), loads, "nothing read with the session held");
    let mut warm: Vec<TabId> = s.warm_tabs().collect();
    warm.sort();
    assert_eq!(warm, panes.to_vec(), "the panes are the live documents, and only they");
    assert!(warm.len() <= MAX_WARM);
}

/// Failure (review 1): the pane read next, when the one being read leaves by closing or a ⌘-click, is read from disk with the session held, as nobody read it ahead. Both say which file to read first.
#[test]
fn the_pane_read_next_is_read_ahead() {
    let mut s = session(10);
    let ids: Vec<TabId> = ["a", "b", "c"]
        .iter()
        .map(|name| s.open(&PathBuf::from(format!("/d/{name}.pdf"))).unwrap())
        .collect();
    s.compare(&[ids[0], ids[1], ids[2]], Some(ids[1])).unwrap();
    // Come back from a record, every pane cold.
    let record = s.to_record();
    let mut back = session(10);
    back.restore(&record, HashMap::new()).unwrap();
    assert_eq!(back.active(), Some(ids[1]));
    back.wake_active();
    assert_eq!(
        back.wake_for_close(ids[1]).map(|(id, _)| id),
        Some(ids[0]),
        "closing the read pane wakes the first left, not the next tab along"
    );
    assert_eq!(
        back.wake_for_compare_click(ids[1]).map(|(id, _)| id),
        Some(ids[0]),
        "and so does ⌘-clicking it out"
    );
    assert_eq!(back.wake_for_compare_click(ids[2]), None, "a pane that isn't read leaving wakes nothing");
    let loads = back.loader().loads.get();
    let (next, _) = back.wake_for_compare_click(ids[1]).unwrap();
    let facts = back.loader().load(Path::new("/d/a.pdf"));
    back.give_woken(next, facts, None);
    let read_ahead = back.loader().loads.get();
    back.toggle_compare(ids[1]).unwrap();
    assert_eq!(back.loader().loads.get(), read_ahead, "nothing read with the session held");
    assert!(read_ahead > loads);
    assert_eq!(back.active(), Some(ids[0]));
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
