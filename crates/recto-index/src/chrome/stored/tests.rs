use super::*;

/// A piece whose every field says which piece it is and which field: no two fields of any piece, or of two pieces, share a value, and the flags take every combination across 16 pieces, `number` present on the odd ones.
fn piece(at: u32) -> EdgeLine {
    let base = at as f32 * 100.0;
    EdgeLine {
        top: at & 1 != 0,
        from_edge: base + 0.25,
        size: base + 1.5,
        key: 0x1111_0000_0000 + u64::from(at),
        row_key: 0x2222_0000_0000 + u64::from(at),
        row_numbers: [at * 10 + 1, at * 10 + 2],
        numbers: [at * 10 + 3, if at == 3 { NO_NUMBER } else { at * 10 + 4 }],
        framed_number: 500 + at as u16,
        start: at * 7,
        end: at * 7 + 5,
        number: (at % 2 == 1).then_some(at * 11),
        relative: base + 0.75,
        words: at & 2 != 0,
        digits_only: at & 4 != 0,
        isolated: at & 8 != 0,
        dated: at.is_multiple_of(3),
        substantial: at.is_multiple_of(5),
        from_left: base + 3.5,
        from_right: base + 4.5,
        place: InRow {
            row: 300 + at as u16,
            alone: at % 3 == 1,
            last: at % 4 == 2,
        },
    }
}

/// Failure: a field is lost or swapped in the codec (`from_left` written as `from_right`, a flag in another's place, `number` absent read as present), so a page's stored chrome differs from what `recto text` marks.
#[test]
fn stored_edges_open_as_they_were_kept() {
    let lines: Vec<EdgeLine> = (0..MAX_EDGE_LINES as u32).map(piece).collect();
    assert!(lines.iter().any(|line| line.number.is_none()) && lines.iter().any(|line| line.number.is_some()));
    let flags: [fn(&EdgeLine) -> bool; 8] = [
        |line| line.top,
        |line| line.words,
        |line| line.digits_only,
        |line| line.isolated,
        |line| line.dated,
        |line| line.substantial,
        |line| line.place.alone,
        |line| line.place.last,
    ];
    for flag in flags {
        assert!(lines.iter().any(flag) && !lines.iter().all(flag), "every flag has to be both ways");
    }
    let mut bytes = Vec::new();
    write(&lines, &mut bytes);
    assert_eq!(bytes.len(), MAX_REGION_LEN, "70 B a piece and a count");
    assert!(length_fits(bytes.len()));
    assert_eq!(read(&bytes, 10_000), Some(lines.clone()));
    assert!(check(&bytes, 10_000));
    // None at all is a region of one byte, and says so.
    let mut none = Vec::new();
    write(&[], &mut none);
    assert_eq!((none.len(), read(&none, 0)), (1, Some(Vec::new())));
}

/// Failure: a region that doesn't add up (a count the bytes don't have, a length that isn't whole pieces, a span past its page, a NaN) is read as pieces, so a hostile page's chunk steers what `get` leaves out.
#[test]
fn a_region_that_doesnt_add_up_is_refused() {
    let mut good = Vec::new();
    write(&[piece(1), piece(2)], &mut good);
    assert!(check(&good, 100));
    assert!(!check(&good, 11), "the second piece ends at 19, past a page of 11 characters");
    let mut counted = good.clone();
    counted[0] = 3;
    assert!(!check(&counted, 100), "a count the bytes don't have");
    assert!(!check(&good[..good.len() - 1], 100) && !length_fits(good.len() - 1));
    assert!(!check(&[good.clone(), vec![0]].concat(), 100) && !length_fits(good.len() + 1));
    assert!(!check(&[], 100) && !length_fits(0));
    let mut backwards = piece(1);
    (backwards.start, backwards.end) = (9, 8);
    let mut bytes = Vec::new();
    write(&[backwards], &mut bytes);
    assert!(!check(&bytes, 100), "start after end");
    for spoil in [f32::NAN, f32::INFINITY] {
        let mut odd = piece(1);
        odd.relative = spoil;
        bytes.clear();
        write(&[odd], &mut bytes);
        assert!(!check(&bytes, 100), "{spoil}");
    }
    let mut unknown_flag = good.clone();
    unknown_flag[2] |= 0x80;
    assert!(!check(&unknown_flag, 100), "a flag no one wrote");
    assert!(!length_fits(1 + LINE_LEN * (MAX_EDGE_LINES + 1)), "17 pieces");
}

/// Failure: walking a document's pages in order reads each page's stored pieces five times, once for each page it is compared with (a rule 1 cost on every `get`), or holds more than a window of them.
#[test]
fn walking_pages_in_order_reads_each_pages_edges_once() {
    let mut asked = Vec::new();
    let mut window = StoredChrome::new(200, |page| {
        asked.push(page);
        Some(Vec::new())
    });
    let mut most_held = 0;
    for page in 10..=73 {
        assert!(window.spans(page).is_some());
        most_held = most_held.max(window.held.len());
    }
    drop(window);
    assert!(asked.len() <= 64 + 4, "{} reads for 64 pages", asked.len());
    assert!(most_held <= 2 * NEIGHBOURS as usize + 1, "{most_held} pages held at once");
    let mut sorted = asked.clone();
    sorted.sort_unstable();
    sorted.dedup();
    assert_eq!(sorted.len(), asked.len(), "a page asked for twice");
}

/// Failure: the window's ends are mistaken (a first page asks for pages before it, a last page for ones past the end), or a page beyond the document is answered.
#[test]
fn the_window_stays_inside_the_document() {
    let mut asked = Vec::new();
    let mut window = StoredChrome::new(3, |page| {
        asked.push(page);
        Some(Vec::new())
    });
    assert!(window.spans(0).is_some() && window.spans(2).is_some());
    assert!(window.spans(3).is_none() && window.spans(u32::MAX).is_none());
    drop(window);
    assert_eq!(asked, [0, 1, 2]);
}
