use super::*;
use crate::nav::Frame;

const PAGE_TOKEN: &str = "r1.040g2081040g2-081040g208104.p42-0c1g60r30c1g60r30c1g60r30c-28t5cy0";

fn identity() -> crate::page_id::PageIdentity {
    crate::page_id::PageIdentity {
        digest: [3; 32],
        object: (7, 0),
        frame: 0x1234_5678,
    }
}

fn sample(target: Target) -> Locator {
    Locator::new(DocumentTags::new([1; 8], Some([2; 8])), PageRef::new(42, &identity()).unwrap(), target)
}

fn frame(turns: u8) -> Frame {
    Frame {
        turns,
        left: 10.0,
        bottom: 20.0,
        right: 610.0,
        top: 820.0,
    }
}

/// Failure: references acquire pointers, strings or collections and their value size grows, or token checks leak through ordinary debug logging.
#[test]
fn locator_values_are_fixed_size_and_debug_does_not_print_checks() {
    assert_eq!(std::mem::size_of::<DocumentTags>(), 17);
    assert_eq!(std::mem::size_of::<PageRef>(), 24);
    assert_eq!(std::mem::size_of::<CharacterRange>(), 16);
    assert_eq!(std::mem::size_of::<Spot>(), 10);
    assert_eq!(std::mem::size_of::<Locator>(), 64);
    let page = sample(Target::Page);
    assert!(!format!("{page:?}").contains("040g"));
    assert!(!format!("{:?} {:?} {:?}", page.document(), page.page(), CharacterRange::new(1, 3, [4; 8]).unwrap()).contains("["));
}

/// Failure: the token's fixed byte order or alphabet changes and stored references silently acquire different checks or frames.
#[test]
fn locator_tokens_have_one_canonical_byte_definition() {
    let page = sample(Target::Page);
    assert_eq!(encode(&page), PAGE_TOKEN);
    assert_eq!(decode(PAGE_TOKEN).unwrap(), page);
    let characters = sample(Target::Characters(CharacterRange::new(1203, 1219, [4; 8]).unwrap()));
    assert_eq!(encode(&characters), format!("{PAGE_TOKEN}.c1203-1219-0g2081040g208"));
    let spot = sample(Target::Spot(Spot::area(72, 350, 200, 90).unwrap()));
    assert_eq!(encode(&spot), format!("{PAGE_TOKEN}.x72,350,200,90"));
}

/// Failure: high/low bits of a page, document or text check are dropped by base32 conversion, or a valid optional-ID/target form cannot be read back.
#[test]
fn locator_codec_round_trips_every_check_bit_and_target() {
    for bit in 0..128 {
        let mut identity = identity();
        identity.digest[bit / 8] ^= 1 << (bit % 8);
        let mut tag = [0; 8];
        tag[(bit % 64) / 8] = 1 << (bit % 8);
        let documents = [DocumentTags::new(tag, None), DocumentTags::new([0xff; 8], Some(tag))];
        for document in documents {
            let page = PageRef::new(MAX_PAGE, &identity).unwrap();
            let targets = [
                Target::Page,
                Target::Characters(CharacterRange::new(0, MAX_CHARACTER, tag).unwrap()),
                Target::Spot(Spot::point(MAX_POINT, MAX_POINT).unwrap()),
                Target::Spot(Spot::area(0, 0, MAX_POINT, MAX_POINT).unwrap()),
            ];
            for target in targets {
                let locator = Locator::new(document, page, target);
                let token = encode(&locator);
                assert!(token.len() <= MAX_TOKEN_BYTES);
                assert_eq!(decode(&token).unwrap(), locator, "bit {bit}, {target:?}");
            }
        }
    }
    let longest = Locator::new(
        DocumentTags::new([0xff; 8], Some([0xff; 8])),
        PageRef::new(MAX_PAGE, &identity()).unwrap(),
        Target::Characters(CharacterRange::new(MAX_CHARACTER - 1, MAX_CHARACTER, [0xff; 8]).unwrap()),
    );
    assert_eq!(encode(&longest).len(), 104);
}

/// Failure: path-like input, partial/repeated targets, signed/overflowing numbers or noncanonical spellings are interpreted as a valid place.
#[test]
fn locator_parser_refuses_the_whole_malformed_token() {
    let bad = [
        String::new(),
        "file:///tmp/document.pdf#page=42".into(),
        PAGE_TOKEN.to_uppercase(),
        PAGE_TOKEN.replace(".p42-", ".p042-"),
        PAGE_TOKEN.replace(".p42-", ".p+42-"),
        PAGE_TOKEN.replace(".p42-", ".p42949672960-"),
        format!("{PAGE_TOKEN}.x0,0,1"),
        format!("{PAGE_TOKEN}.x0,0,1,1,1"),
        format!("{PAGE_TOKEN}.x0,0.c0-1-0g2081040g208"),
        format!("{PAGE_TOKEN}.c0-1-0g2081040g208.x0,0"),
        format!("{PAGE_TOKEN}.s1-abc"),
        format!("{PAGE_TOKEN}."),
        format!("{PAGE_TOKEN}\n"),
        PAGE_TOKEN.replace("040g", "o40g"),
        PAGE_TOKEN.replace("040g", "i40g"),
        PAGE_TOKEN.replace("040g", "l40g"),
    ];
    for token in bad {
        assert!(decode(&token).is_err(), "accepted {token:?}");
    }
    assert_eq!(decode(&format!("{PAGE_TOKEN}é")), Err(Error::NonAscii));
    assert_eq!(decode(&"x".repeat(MAX_TOKEN_BYTES)), Err(Error::Malformed));
    assert_eq!(decode(&"x".repeat(MAX_TOKEN_BYTES + 1)), Err(Error::TooLong));
    assert_eq!(decode(&PAGE_TOKEN.replacen("r1.", "r2.", 1)), Err(Error::UnsupportedVersion));
}

/// Failure: ignored base32 padding bits or a prefix read allow two distinct tokens to name the same value, or a truncated token is partly accepted.
#[test]
fn locator_parser_refuses_nonzero_padding_and_truncation() {
    for changed in [
        PAGE_TOKEN.replace("040g2081040g2", "040g2081040g3"),
        PAGE_TOKEN.replace("0c1g60r30c-", "0c1g60r30d-"),
        format!("{}1", &PAGE_TOKEN[..PAGE_TOKEN.len() - 1]),
    ] {
        assert_eq!(decode(&changed), Err(Error::Malformed));
    }
    for end in 0..PAGE_TOKEN.len() {
        assert!(decode(&PAGE_TOKEN[..end]).is_err(), "accepted truncation at {end}");
    }
}

/// Failure: parsing bypasses validated constructors, accepting absent pages, reversed/too-long ranges, empty boxes or overflowing extents.
#[test]
fn locator_limits_hold_at_the_boundary_and_one_past_it() {
    assert!(PageRef::new(MAX_PAGE, &identity()).is_ok());
    assert!(PageRef::new(0, &identity()).is_err());
    assert!(PageRef::new(MAX_PAGE + 1, &identity()).is_err());
    assert!(CharacterRange::new(0, MAX_CHARACTER, [0; 8]).is_ok());
    for (start, end) in [(0, MAX_CHARACTER + 1), (1, 1), (2, 1)] {
        assert!(CharacterRange::new(start, end, [0; 8]).is_err());
    }
    for suffix in [
        ".x14401,0",
        ".x65535,0,1,1",
        ".x14400,0,1,1",
        ".x0,0,0,1",
        ".c0-2097153-0000000000000",
        ".c1-1-0000000000000",
    ] {
        assert!(decode(&format!("{PAGE_TOKEN}{suffix}")).is_err(), "{suffix}");
    }
    for page in ["0", "200001"] {
        assert!(decode(&PAGE_TOKEN.replace(".p42-", &format!(".p{page}-"))).is_err());
    }
}

/// Failure: a cropped or turned PDF box acquires a second coordinate convention instead of the existing top-left shown frame.
#[test]
fn locator_boxes_use_the_existing_crop_and_four_turns() {
    let expected = [
        [20.0, 700.0, 120.0, 750.0],
        [50.0, 20.0, 100.0, 120.0],
        [480.0, 50.0, 580.0, 100.0],
        [700.0, 480.0, 750.0, 580.0],
    ];
    for turns in 0..4 {
        let frame = frame(turns);
        let spot = Spot::from_pdf_box([30.0, 70.0, 130.0, 120.0], frame).unwrap();
        assert_eq!(spot.checked_shown(frame).unwrap(), expected[usize::from(turns)]);
    }
    let frame = Frame { top: 1020.0, ..frame(0) };
    for top in [0, 1, 499, 500, 501, 999, 1000] {
        let spot = Spot::from_thousandths(top, frame).unwrap();
        assert_eq!(spot.thousandths(frame).unwrap(), top);
        assert_eq!(spot.origin().1, top);
    }
    assert!(Spot::from_thousandths(crate::TOP_SCALE + 1, frame).is_err());

    for turns in 0..4 {
        let frame = Frame {
            turns,
            left: 10.0,
            bottom: 20.0,
            right: 605.28,
            top: 861.89,
        };
        let (width, height) = frame.shown_size().unwrap();
        let point = Spot::from_shown_point(width, height, frame).expect("a valid fractional-page corner is representable inward");
        let corner = point.checked_shown(frame).unwrap();
        assert!((0.0..1.0).contains(&(width - corner[0])) && (0.0..1.0).contains(&(height - corner[1])));
        let box_at = Spot::from_pdf_box([frame.left, frame.bottom, frame.right, frame.top], frame).expect("the full fractional page");
        let shown = box_at.checked_shown(frame).unwrap();
        assert_eq!(&shown[..2], &[0.0, 0.0]);
        assert!((0.0..1.0).contains(&(width - shown[2])) && (0.0..1.0).contains(&(height - shown[3])));
        let foot = Spot::from_thousandths(crate::TOP_SCALE, frame).expect("the foot of a fractional page");
        assert!((0.0..1.0).contains(&(height - foot.checked_shown(frame).unwrap()[1])));
        assert!(
            Spot::from_shown_point(width + 0.01, height, frame).is_err(),
            "an out-of-page point is not rounded inward"
        );
    }
}

/// Failure: hostile/unknown frames or off-page targets are silently clamped, NaNs reach a cast, or a tiny positive box collapses into a point.
#[test]
fn locator_geometry_rejects_invalid_and_off_page_targets() {
    let frame = frame(0);
    for rect in [
        [130.0, 70.0, 30.0, 120.0],
        [30.0, 120.0, 130.0, 70.0],
        [30.0, 70.0, 30.0, 120.0],
        [30.0, 70.0, 130.0, 70.0],
    ] {
        assert!(Spot::from_pdf_box(rect, frame).is_err(), "PDF box must be ordered and positive: {rect:?}");
    }
    for (x, y) in [(f32::NAN, 0.0), (0.0, f32::INFINITY), (-0.1, 0.0), (600.1, 0.0), (0.0, 800.1)] {
        assert!(Spot::from_shown_point(x, y, frame).is_err());
    }
    for rect in [
        [1.0, 1.0, 0.0, 2.0],
        [0.0, 0.0, 601.0, 1.0],
        [0.1, 0.1, 0.2, 0.2],
        [0.0, 0.0, f32::NAN, 1.0],
        [-1.0, 0.0, 1.0, 1.0],
    ] {
        assert!(Spot::from_shown_box(rect, frame).is_err(), "{rect:?}");
    }
    for bad in [
        Frame { turns: 4, ..frame },
        Frame { top: f32::INFINITY, ..frame },
        Frame { right: frame.left, ..frame },
    ] {
        assert!(Spot::point(0, 0).unwrap().checked_shown(bad).is_err());
    }
    let parsed = decode(&format!("{PAGE_TOKEN}.x14400,14400")).unwrap();
    let Target::Spot(spot) = parsed.target() else { panic!("a spot") };
    assert!(spot.checked_shown(frame).is_err(), "a parsed global bound does not establish page bounds");
    assert_eq!(Spot::from_shown_point(12.4, 42.6, frame).unwrap().origin(), (12, 43));
}

/// Failure: the locator introduces another page hash or discards the frame, so a reference made from the real identity cannot be checked later.
#[test]
fn locator_copies_the_real_existing_page_identity() {
    use crate::page_id;
    use recto_testpdf::{Page, pages_with, text_lines};
    let bytes = pages_with(&[Page::new(text_lines(&["A reference to this page"]))], "", vec![]);
    let reader = page_id::open_bytes_for_identity(std::sync::Arc::from(bytes)).unwrap();
    let map = reader.page_map().unwrap();
    let identity = page_id::identify_page(&reader, map.get(0).unwrap(), &page_id::Caps::STANDARD).unwrap();
    let reference = PageRef::new(1, &identity).unwrap();
    assert_eq!(reference.page_check().as_slice(), &identity.digest[..16]);
    assert_eq!(reference.frame_tag(), identity.frame);
    let value = Locator::new(DocumentTags::new([1; 8], None), reference, Target::Page);
    assert_eq!(decode(&encode(&value)).unwrap(), value);
}

/// Failure: the text check drifts from the frozen `recto-text/1` (another hash, another prefix, bytes or normalised text instead of the characters), so
/// every reference to characters made before the drift reads as changed text.
#[test]
fn the_text_check_is_recto_text_1_over_the_utf8_characters() {
    // printf 'recto-text/1\0%s' 'Mine page 3' | shasum -a 256
    assert_eq!(text_check("Mine page 3"), [0x87, 0x5f, 0xb1, 0xc4, 0x68, 0x09, 0xd7, 0x88]);
    let range = characters("xx Mine page 3 yy", 3, 14).unwrap();
    assert_eq!(range.text_check(), text_check("Mine page 3"));
    assert_eq!(range.offsets(), (3, 14));
}

/// Failure: offsets count bytes, or UTF-16 units, so a range over text with accents or an emoji names other characters than `recto text`'s offsets do.
#[test]
fn characters_count_unicode_scalars_not_bytes() {
    let text = "a\u{e9}\u{1F600}b\u{e9}";
    let range = characters(text, 1, 4).unwrap();
    assert_eq!(range.text_check(), text_check("\u{e9}\u{1F600}b"));
    assert_eq!(check_characters(range, text), TextMatch::Same);
}

/// Failure: an empty range, one past the cap, or one past the text's end is made into a reference (so it names nothing, or text that isn't there), or the
/// limit itself is refused.
#[test]
fn a_range_empty_past_the_text_or_past_the_cap_is_refused() {
    let text = "abcdef";
    assert!(characters(text, 0, 6).is_ok(), "end at the count is held");
    assert!(characters(text, 5, 6).is_ok());
    for (start, end) in [(0, 7), (6, 7), (3, 3), (4, 2), (0, 0)] {
        assert_eq!(characters(text, start, end).unwrap_err(), Error::OutOfRange, "{start}-{end}");
    }
    let long: String = "x".repeat(MAX_CHARACTER as usize);
    assert!(characters(&long, 0, MAX_CHARACTER).is_ok(), "the cap itself is held");
    assert_eq!(characters(&long, 0, MAX_CHARACTER + 1).unwrap_err(), Error::OutOfRange);
}

/// Failure: a changed page's range is vouched for by finding the words elsewhere (a search), a range past a short text panics or is `Same`, or a hostile
/// range of 2^21 over a short text walks more than the text.
#[test]
fn check_is_same_only_at_the_same_offsets_and_never_searches_elsewhere() {
    let range = characters("one two three", 4, 7).unwrap();
    assert_eq!(check_characters(range, "one two three"), TextMatch::Same);
    assert_eq!(check_characters(range, "one  two three"), TextMatch::Changed, "the words moved by one");
    assert_eq!(check_characters(range, "one"), TextMatch::Changed, "past the text");
    let hostile = CharacterRange::new(0, MAX_CHARACTER, [0; 8]).unwrap();
    assert_eq!(check_characters(hostile, "ten chars!!"), TextMatch::Changed);
}

/// Failure: what lies past a range's `end` in a long page (100,000 characters) changes the answer for the range, so a check is about more than its own
/// characters. Two texts that differ only past `end` give the same range and the same match, and a change inside the range is `Changed`. This holds the
/// answer, not the cost: a walk that read on and ignored the rest would pass, and the bound on the walk is by `span` stopping at `end`, which no
/// timing-free test sees.
#[test]
fn a_small_range_over_a_long_text_is_decided_by_what_is_before_its_end() {
    let head = "Mine page 3 ";
    let (a, b) = (format!("{head}{}", "x".repeat(100_000)), format!("{head}{}\u{1F600}", "y".repeat(100_000)));
    let made = characters(&a, 5, 9).unwrap();
    assert_eq!(made.text_check(), text_check("page"));
    assert_eq!(characters(&b, 5, 9).unwrap().text_check(), made.text_check());
    assert_eq!((check_characters(made, &a), check_characters(made, &b)), (TextMatch::Same, TextMatch::Same));
    let changed = format!("Mine pace 3 {}", "x".repeat(100_000));
    assert_eq!(check_characters(made, &changed), TextMatch::Changed);
    assert_eq!(
        characters(&a, 0, 100_000 + head.len() as u32).map(|range| range.offsets().1),
        Ok(100_000 + head.len() as u32)
    );
}
