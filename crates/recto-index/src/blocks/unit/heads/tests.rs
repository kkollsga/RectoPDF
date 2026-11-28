//! What a page's left-out stretches do to the text and the numbers kept with it. Each test names the failure it catches.

use super::*;

fn page() -> Vec<char> {
    "HEAD body one\nbody two 7".chars().collect()
}

/// Failure: a left-out stretch is given, or takes room, or a hidden stretch isn't moved with the text it's in. Spans at the start and the end of a 24-character page: the copy has neither, the lengths agree, a
/// hidden stretch after the head is in characters of what is given, and one inside the head is gone.
#[test]
fn what_is_left_out_is_not_copied_and_hidden_stretches_follow_the_rest() {
    let chars = page();
    let heads = Heads::of(Some(&[(0, 5), (22, 24)]), 24, None, 1024);
    assert_eq!(heads.state, State::Known);
    let (text, end) = heads.copy(&chars, 0, 24, 1000);
    assert_eq!((text.iter().collect::<String>().as_str(), end), ("body one\nbody two", 24));
    assert_eq!(heads.kept_len(0, 24), text.len() as u32);
    assert_eq!(heads.remap(&[(2, 3), (5, 9), (14, 18)], 0, 24), vec![(0, 4), (9, 13)]);
    // Some of it, and where it goes on: the first 4 given characters, and the next given one after the head.
    let (some, stopped) = heads.copy(&chars, 0, 24, 4);
    assert_eq!((some.iter().collect::<String>().as_str(), stopped), ("body", 9));
    assert_eq!(heads.next_kept(0), 5);
    assert_eq!(heads.next_kept(7), 7);
}

/// Failure: spans that overlap, touch or run past the page are left out wrongly (a character counted twice, or an offset past the page).
#[test]
fn spans_are_put_in_order_joined_and_kept_inside_the_page() {
    let heads = Heads::of(Some(&[(10, 30), (2, 5), (4, 8), (8, 9)]), 20, None, 1024);
    assert_eq!(
        (heads.left_in(0, 20), heads.left_in(5, 12)),
        (7 + 10, 4 + 2),
        "(2..9) and (10..20), each character once"
    );
}

/// Failure: a reference that asks for a head gets nothing, or one into body text gets the page number after it (the more-ref of a cut names 32 characters, which may run into the foot).
#[test]
fn only_a_reference_that_begins_in_chrome_names_it() {
    let raw = [(0, 5), (22, 24)];
    let named_head = Heads::of(Some(&raw), 24, Some((0, 4)), 1024);
    assert_eq!(
        (named_head.left_in(0, 5), named_head.left_in(22, 24)),
        (1, 2),
        "the four characters it names stay"
    );
    let body_start = Heads::of(Some(&raw), 24, Some((15, 24)), 1024);
    assert_eq!(body_start.left_in(22, 24), 2, "begins in body text: the foot is still left out");
}

/// Failure: a page with nothing known is taken as having no chrome silently, or its state is mistaken for known.
#[test]
fn unknown_is_not_known_and_over_the_cap_is_given_whole() {
    assert_eq!(Heads::of(None, 24, None, 1024).state, State::Unknown);
    let over = Heads::of(Some(&[(0, 5)]), 24, None, 4);
    assert_eq!((over.state, over.left_in(0, 24)), (State::OverCap, 0));
}

/// Failure: a reference that starts one character before a running head still gets it (so a body reference near the head names it), or one that starts at its first character gets nothing. The head is
/// characters 5 to 23: a range from 4 leaves it out whole, a range from 5 gives the characters it names (the rest of the head stays out), and one from 22 gives its last.
#[test]
fn a_reference_names_chrome_only_from_its_first_character_in_it() {
    let raw = [(5, 23)];
    let left = |named: (u32, u32)| Heads::of(Some(&raw), 40, Some(named), 1024).left_in(0, 40);
    assert_eq!(left((4, 30)), 18, "starts before it: all of it is left out");
    assert_eq!(left((5, 12)), 18 - 7, "starts in it: the seven characters it names are given");
    assert_eq!(left((22, 30)), 18 - 1);
    assert_eq!(left((23, 30)), 18, "starts after it");
}

/// Failure: a left-out stretch that covers part of a hidden stretch shows the rest of it unmarked, or moves the mark. The hidden stretch 8 to 14 with the head 5 to 10 over its start: what is given of it is
/// marked, as the four characters that are still there (given text's 5 to 9); one inside the head is gone.
#[test]
fn a_left_out_stretch_only_shortens_a_hidden_stretch() {
    let heads = Heads::of(Some(&[(5, 10)]), 20, None, 1024);
    assert_eq!(
        heads.remap(&[(8, 14), (6, 9)], 0, 20),
        vec![(5, 9)],
        "four characters are left of the first, none of the one inside the head"
    );
}
