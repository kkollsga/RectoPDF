//! A place brought back through a window that wasn't the one it was kept in: the window that went up without a late state, at its own size, then the one
//! made again from the state (`record.remade`).

use super::*;

/// The last scroll the reader sent the host, with its number.
fn sent(reader: &mut Reader) -> (f64, u64) {
    reader
        .host
        .take_ops()
        .into_iter()
        .rev()
        .find_map(|op| if let Op::ScrollTo { y, seq } = op { Some((y, seq)) } else { None })
        .expect("a scroll sent")
}

/// A viewport `frame` wide at `y`, said after the scroll numbered `seq` was applied.
fn at(frame: f64, y: f64, seq: u64) -> Viewport {
    Viewport {
        width: frame,
        frame_width: frame,
        scrolled_to: seq,
        ..view(y)
    }
}

/// Failure (CI run 43751284816, main at 504f5926, `everything_comes_back_when_the_state_is_read_late` 1 failure in 5 at a load of 46 on 3 cores: both
/// panes came back 0.21 and 0.02 points from where they were, at the same document height, so not to the pixel): a place is kept as the host's pixel with
/// its anchor up to [`HOST_ROUNDING`] from it, and brought back to that pixel while the document is as tall as it was. Shown first in a window of another
/// width, where the pixel doesn't apply and the view goes to the anchor, the next word from that window that wasn't an arrival (it settling its own height)
/// was taken for the reader moving, and the pixel was dropped: in the window of the kept width it came back to the anchor instead. Nobody moved, so the pixel
/// is kept for when the layout it's about comes back.
#[test]
fn a_place_shown_first_at_another_width_comes_back_to_the_pixel_at_its_own() {
    let (kept_width, other_width) = (1168.0, 800.0);
    // Launch 1: sent part way down a page, arrived on the host's half-point pixel above it, and a word from the host after, as a pane in a comparison is.
    let mut first = reader_on(100);
    settle(&mut first);
    first.place.scrolled_to = 0;
    first.jump_to(40, 0.4221);
    let (exact, seq) = sent(&mut first);
    let pixel = (exact * 2.0).floor() / 2.0 - 0.5;
    assert!((exact - pixel).abs() < HOST_ROUNDING && exact != pixel, "{exact} against {pixel}");
    first.set_viewport(at(kept_width, pixel, seq));
    first.set_viewport(Viewport {
        height: 901.0,
        ..at(kept_width, pixel, seq)
    });
    let (_, saved) = first.places()[0];
    assert_eq!(saved.y, pixel, "kept as the host's pixel: {saved:?}");
    assert_ne!(
        first.docs.shown().unwrap().layout.anchor_y(saved.anchor),
        saved.y,
        "an anchor within rounding of it, not on it"
    );

    // Launch 2: the window that went up without the state, at its own width, shows the tab when the state comes in.
    let mut second = Reader::new();
    second.window_opened();
    second.place.scrolled_to = 0;
    second.set_viewport(at(other_width, 0.0, 0));
    second.activate(1, 100, 40, map(), &[], Some(saved), Spread::Single);
    let (there, seq) = sent(&mut second);
    second.set_viewport(at(other_width, there, seq));
    // Its next word, which isn't an arrival: the window settling its own height.
    second.set_viewport(Viewport {
        height: 901.0,
        ..at(other_width, there, seq)
    });
    assert!(!second.place.take_user_scrolled(), "nobody scrolled");
    // It's made again from the state, at the width the place was kept at.
    second.window_closed();
    second.place.scrolled_to = 0;
    second.set_viewport(at(kept_width, 0.0, 0));
    second.window_opened();
    let (back, seq) = sent(&mut second);
    second.set_viewport(at(kept_width, back, seq));
    let (_, place) = second.places()[0];
    assert_eq!(place.document_height, saved.document_height, "the layout the place was kept in");
    assert_eq!((back, place.y), (saved.y, saved.y), "to the pixel, as the on-time launch brings it back");
}
