use super::*;

/// Failure: each way of zooming has its own step and limits, or one goes past them, or "actual size" is the same as "fit".
#[test]
fn every_way_of_zooming_is_one_path_with_one_set_of_limits() {
    let mut reader = reader_on(10);
    settle(&mut reader);
    assert_eq!(reader.zoom(Zoom::In), Some(ZOOM_STEP));
    assert!(
        reader
            .host
            .take_ops()
            .iter()
            .any(|op| matches!(op, Op::Magnify { value } if *value == ZOOM_STEP))
    );
    assert_eq!(reader.zoom(Zoom::Out), Some(1.0));
    assert_eq!(reader.zoom(Zoom::To(1e9)), Some(MAX_ZOOM));
    assert_eq!(reader.zoom(Zoom::To(0.0)), Some(MIN_ZOOM));
    assert_eq!(reader.zoom(Zoom::To(f64::NAN)), Some(MIN_ZOOM), "a nonsense number changes nothing");
    assert_eq!(reader.zoom(Zoom::Fit), Some(1.0));
    let actual = reader.zoom(Zoom::Actual).unwrap();
    assert!(
        (actual * reader.docs.shown().unwrap().layout.fit() - 1.0).abs() < 1e-9 || actual == MIN_ZOOM || actual == MAX_ZOOM,
        "{actual}"
    );
    assert!(zoom_after_scroll(1.0, 100.0) > 1.0 && zoom_after_scroll(1.0, -100.0) < 1.0, "upwards is in");
    assert_eq!(zoom_after_scroll(1.0, 1e9), MAX_ZOOM);
    assert_eq!(zoom_after_scroll(1.0, -1e9), MIN_ZOOM);
    assert_eq!(zoom_after_scroll(2.0, f64::NAN), 2.0);
}

/// Failure: the zoom level says one thing and does another: 100% isn't the page's real size, a typed level goes past the limits, something that isn't a number zooms, or (review of f05c998, finding 6) `1,000` typed with a Mac set to `.` is 1%. Owner, 2026-11-07: "clicking the zoom value enables you to type in zoom level or reset".
#[test]
fn a_zoom_level_is_a_share_of_the_real_size() {
    let mut reader = reader_on(10);
    settle(&mut reader);
    let fit = reader.docs.shown().unwrap().layout.fit();
    let actual = reader.zoom(Zoom::Percent(100.0)).unwrap();
    assert!((actual - reader.zoom(Zoom::Actual).unwrap()).abs() < 1e-9);
    assert!((zoom_percent(actual, fit) - 100.0).abs() < 1e-9);
    assert!((zoom_percent(1.0, fit) - fit * 100.0).abs() < 1e-9, "the fitted width says what it is");
    assert_eq!(reader.zoom(Zoom::Percent(1e6)), Some(MAX_ZOOM));
    assert_eq!(reader.zoom(Zoom::Percent(0.001)), Some(MIN_ZOOM));
    for (typed, decimal, wanted) in [
        ("150", '.', 150.0),
        ("150%", '.', 150.0),
        (" 87.5 % ", '.', 87.5),
        ("1,000", '.', 1000.0),
        ("1 000", '.', 1000.0),
        ("80,5", ',', 80.5),
        ("1.000", ',', 1000.0),
        ("1.250,5 %", ',', 1250.5),
    ] {
        assert_eq!(typed_zoom(typed, decimal), Some(Zoom::Percent(wanted)), "{typed} with {decimal}");
    }
    for nonsense in ["", "%", "abc", "-50", "0", "NaN", "inf", "1e400", "9999999999999", "1%%"] {
        assert_eq!(typed_zoom(nonsense, '.'), None, "{nonsense}");
    }
}

/// Failure (PLAN, side-by-side follow-ups): typing "1.5" in the zoom field on a Mac set to a `,` decimal, as the owner's Norwegian one is, reads as 15%, since `.` groups thousands there; or "1,5" on a Mac set to `.` reads as 15%. A number says which mark it means wherever it can, and the Mac's own decimal sign decides only `1,000` and `1.000`.
#[test]
fn a_typed_zoom_reads_either_decimal_sign_on_any_mac() {
    for decimal in ['.', ','] {
        for (typed, wanted) in [
            ("1.5", 1.5),
            ("1,5", 1.5),
            ("87.5%", 87.5),
            ("87,5 %", 87.5),
            ("150.25", 150.25),
            ("150,25", 150.25),
            ("1250.5", 1250.5),
            ("1,250.5", 1250.5),
            ("1.250,5", 1250.5),
            ("1.000.000", 1_000_000.0),
            ("1,000,000", 1_000_000.0),
            (".5", 0.5),
            (",5", 0.5),
            ("150.", 150.0),
            ("1 000", 1000.0),
        ] {
            assert_eq!(typed_zoom(typed, decimal), Some(Zoom::Percent(wanted)), "{typed} with {decimal}");
        }
        for nonsense in [".", ",", "1.2,3.4", "1,2.3,4", "1..5", "5.1,2"] {
            assert_eq!(typed_zoom(nonsense, decimal), None, "{nonsense} with {decimal}");
        }
    }
    // Only these can be either, and the Mac's sign decides.
    assert_eq!(typed_zoom("1,000", '.'), Some(Zoom::Percent(1000.0)));
    assert_eq!(typed_zoom("1.000", ','), Some(Zoom::Percent(1000.0)));
    assert_eq!(typed_zoom("1.000", '.'), Some(Zoom::Percent(1.0)));
    assert_eq!(typed_zoom("1,000", ','), Some(Zoom::Percent(1.0)));
}
