//! Where the view is across as the zoom changes, against a model of the host's scroll view: rows centred in the view while they fit, the zoom's point
//! kept still past that, and a place across kept as a point on a page.

use super::*;

const FRAME: f64 = 1168.0;

/// The host's scroll view as far as across goes, as `PageArea` and AppKit's `NSScrollView` have it: a zoom about a point keeps that point of the document
/// under it, and the view is held to the document's width, or centred on a document narrower than it (`CenteringClipView`). Where it draws a page is
/// where Rust last told it to ([`drawn`]).
struct Host {
    /// The window's reading area, in its points: [`FRAME`] until a resize or the sidebar changes it.
    frame: f64,
    x: f64,
    y: f64,
    magnification: f64,
    document_width: f64,
    scrolled_to: u64,
}

impl Host {
    fn new() -> Self {
        Self {
            frame: FRAME,
            x: 0.0,
            y: 0.0,
            magnification: 1.0,
            document_width: FRAME,
            scrolled_to: u64::MAX,
        }
    }

    fn view_width(&self) -> f64 {
        self.frame / self.magnification
    }

    /// The window resized, or the sidebar shown or hidden: AppKit keeps the view's left edge, and holds it to the document as it was.
    fn resize(&mut self, frame: f64) {
        self.frame = frame;
        self.hold();
    }

    fn hold(&mut self) {
        let width = self.view_width();
        self.x = if width > self.document_width {
            (self.document_width - width) / 2.0
        } else {
            self.x.clamp(0.0, self.document_width - width)
        };
    }

    /// Zooms about a point of the window, `across` points from the view's left edge, as a pinch or ⌘-scroll does.
    fn zoom_at(&mut self, magnification: f64, across: f64) {
        let point = self.x + across / self.magnification;
        self.magnification = magnification;
        self.x = point - across / magnification;
        self.hold();
    }

    fn apply(&mut self, ops: Vec<Op>) {
        for op in ops {
            match op {
                Op::Document { width, .. } => {
                    self.document_width = width;
                    self.hold();
                }
                Op::Magnify { value } => {
                    let middle = self.view_width() / 2.0;
                    self.zoom_at(value, middle * self.magnification);
                }
                Op::ScrollTo { y, seq } => {
                    self.y = y;
                    self.scrolled_to = seq;
                }
                Op::ScrollAcross { x } => {
                    self.x = x;
                    self.hold();
                }
                _ => {}
            }
        }
    }

    fn viewport(&self) -> Viewport {
        self.viewport_live(false)
    }

    /// The view as the host tells it, `live` while a pinch or ⌘-scroll is under way.
    fn viewport_live(&self, live: bool) -> Viewport {
        Viewport {
            x: self.x,
            y: self.y,
            width: self.view_width(),
            height: 900.0 / self.magnification,
            frame_width: self.frame,
            magnification: self.magnification,
            backing: 2.0,
            live,
            scrolled_to: self.scrolled_to,
        }
    }

    /// Tells the reader where the view is, applying what comes back, until it's said nothing more: what `PageArea.sendViewport` and `apply` do.
    fn tell(&mut self, reader: &mut Reader) {
        self.tell_live(reader, false);
    }

    /// [`Self::tell`], `live` as a step of a pinch or ⌘-scroll under way is.
    fn tell_live(&mut self, reader: &mut Reader, live: bool) {
        for _ in 0..8 {
            let ops = reader.set_viewport(self.viewport_live(live));
            reader.applied();
            let before = (self.x, self.y);
            self.apply(ops);
            if (self.x, self.y) == before {
                return;
            }
        }
        panic!("the view kept moving");
    }

    /// Which point of a page, in its own points, is under a point of the window `across` from the view's left edge, as the host draws it.
    fn page_point_under(&self, reader: &Reader, page: u32, across: f64) -> f64 {
        let fit = reader.docs.shown().unwrap().layout.fit();
        (self.x + across / self.magnification - drawn(reader, page)) / fit
    }
}

/// Where the host draws a page across: where its last `ShowPage` put it, as the reader keeps it.
fn drawn(reader: &Reader, page: u32) -> f64 {
    reader.host.shown[&page].x
}

/// A letter document settled in the view, with the view on page 3.
fn on_page_three() -> (Reader, Host) {
    let mut reader = reader_on(10);
    settle(&mut reader);
    let mut host = Host::new();
    host.y = reader.docs.shown().unwrap().layout.top(3);
    host.tell(&mut reader);
    settle(&mut reader);
    (reader, host)
}

/// Failure (owner, 2026-11-16): "when I zoom into the page only the right margin is reduced. Why? The page should always be centered (if there is no scrollbar)". Zoomed in by ⌘+, the menu or an agent, about the view's middle, from the fit to well past the window's width: while the page is no wider than the window it's centred, with equal margins, and the document is no wider than the view, so there's nothing to scroll across; past that, the middle of the page stays in the middle of the window, so the zoom never throws the reader to one side as the page passes the room or the window. And at every step a point on the page as it's drawn is the same point to Rust, which a click, a selection, a link and a highlight go by.
#[test]
fn a_zoom_about_the_middle_keeps_the_page_centred_until_it_is_wider_then_keeps_its_middle() {
    let (mut reader, mut host) = on_page_three();
    let mut magnification: f64 = 1.0;
    while magnification < 4.0 {
        magnification *= 1.04;
        let value = reader.zoom(Zoom::To(magnification)).unwrap();
        host.apply(reader.host.take_ops());
        host.tell(&mut reader);
        let doc = reader.docs.shown().unwrap();
        let fit = doc.layout.fit();
        let (left, width) = (drawn(&reader, 3), doc.layout.rect(3).width);
        // On the window, in its points: the margins either side of the page.
        let margins = ((left - host.x) * value, FRAME - (left + width - host.x) * value);
        // What's drawn is put on whole device pixels: half of one, in the window's points.
        let pixel = 0.25 + 1e-9;
        if width * value <= FRAME {
            assert!(
                (margins.0 - margins.1).abs() <= 2.0 * pixel && host.document_width <= host.view_width() + 1e-9,
                "at {value}: margins {margins:?}, {} across a view {}",
                host.document_width,
                host.view_width()
            );
        } else {
            assert!(
                (host.page_point_under(&reader, 3, FRAME / 2.0) - 306.0).abs() * fit * value <= pixel,
                "at {value}: the page's middle went to {}",
                host.page_point_under(&reader, 3, FRAME / 2.0)
            );
            assert!(
                margins.0 <= pixel && margins.1 <= pixel,
                "at {value}: the page runs past both edges: {margins:?}"
            );
        }
        let under = doc.point_on_page(host.x + host.view_width() / 2.0, host.y + 10.0).unwrap();
        assert!(
            under.page == 3 && (f64::from(under.x) - host.page_point_under(&reader, 3, FRAME / 2.0)).abs() * fit * value <= pixel,
            "at {value}: drawn under the middle {}, read as {under:?}",
            host.page_point_under(&reader, 3, FRAME / 2.0)
        );
    }
}

/// Failure: a zoom about the pointer, a pinch or ⌘-scroll, moves the point under it: once the page is wider than the window, the point the reader zoomed on slides away, or as the page passes the window's width it jumps, because the view across wasn't moved with the rows, which a zoom moves across. Zoomed in small steps about a point left of the page's middle, from the fit to well past the window: the point under the pointer moves only a little a step, as the page is centred, until the page is wider than the window, and from the step the view leaves the document's left edge it stays under the pointer; and one long step from the fit to past the window keeps it there.
#[test]
fn a_zoom_about_the_pointer_keeps_its_point_once_the_page_is_wider_and_never_jumps() {
    let (mut reader, mut host) = on_page_three();
    let pointer = 350.0;
    let mut was = host.page_point_under(&reader, 3, pointer);
    let mut held_since: Option<f64> = None;
    let mut magnification: f64 = 1.0;
    while magnification < 4.0 {
        let step = 1.01;
        magnification *= step;
        let x_was = host.x;
        host.zoom_at(magnification, pointer);
        host.tell(&mut reader);
        let fit = reader.docs.shown().unwrap().layout.fit();
        let now = host.page_point_under(&reader, 3, pointer);
        // On the window: how far the point that was under the pointer has gone.
        let moved = (now - was).abs() * fit * magnification;
        assert!(
            moved <= FRAME * (step - 1.0),
            "at {magnification}: the point under the pointer went {moved} points in one step, from {was} to {now}"
        );
        if x_was > 0.0 && host.x > 0.0 {
            held_since.get_or_insert(magnification);
            assert!(
                moved <= 0.5,
                "at {magnification}: the point under the pointer slid {moved} points, from {was} to {now}"
            );
        }
        was = now;
    }
    assert!(held_since.is_some_and(|at| at < 2.0), "the view left the left edge: {held_since:?}");

    // One long ⌘-scroll from the fit to well past the window, about a point right of the page's middle: the rows moved 204 points across under the host's
    // zoom, and the view goes with them, so the point stays under the pointer.
    let (mut reader, mut host) = on_page_three();
    let pointer = 750.0;
    let was = host.page_point_under(&reader, 3, pointer);
    host.zoom_at(2.5, pointer);
    host.tell(&mut reader);
    let fit = reader.docs.shown().unwrap().layout.fit();
    let now = host.page_point_under(&reader, 3, pointer);
    assert!((now - was).abs() * fit * 2.5 <= 0.5, "in one step past the window: from {was} to {now}");
}

/// Failure: a tab restored at a zoom wider than the window, by a restart, an update or a window reopened, comes back at another point across its page, as the place across is a point in the document, which a zoom moves. Kept and restored at the same window and zoom, the point under the middle of the window is the same point of the same page; and a tab restored at a zoom whose page fits is centred, whatever was kept across.
#[test]
fn a_tab_restored_at_a_zoom_comes_back_to_the_same_point_on_its_page() {
    for (magnification, across) in [(2.0, 90.0), (1.2, 40.0)] {
        let (mut reader, mut host) = on_page_three();
        reader.zoom(Zoom::To(magnification));
        host.apply(reader.host.take_ops());
        host.tell(&mut reader);
        host.x = across;
        host.hold();
        host.tell(&mut reader);
        reader.place.take_moved_page();
        let kept = host.page_point_under(&reader, 3, FRAME / 2.0);
        let saved = reader.places()[0].1;

        let mut again = Reader::new();
        let mut back = Host::new();
        again.set_viewport(back.viewport());
        again.activate(1, 10, saved.page, map(), &[], Some(saved), Spread::Single);
        back.apply(again.host.take_ops());
        back.tell(&mut again);
        settle(&mut again);
        back.apply(again.host.take_ops());
        back.tell(&mut again);
        let fit = again.docs.shown().unwrap().layout.fit();
        let now = back.page_point_under(&again, 3, FRAME / 2.0);
        assert!(
            (now - kept).abs() * fit * magnification <= 0.5 && (back.y - host.y).abs() < 1.0,
            "at {magnification}: kept {kept} at {}, back at {now} at {}",
            host.y,
            back.y
        );
        if again.docs.shown().unwrap().layout.rect(3).width * magnification <= FRAME {
            assert_eq!(back.x, 0.0, "at {magnification}: a page that fits is centred, with nothing across");
            assert!((now - 306.0).abs() * fit * magnification <= 0.5, "at {magnification}: centred: {now}");
        }
    }
}

/// Failure (the read of 2026-11-16 before 0.1.2): a zoom in small steps, as a slow pinch or a fine ⌘-scroll makes them, ends with the document up to half a
/// point narrower or wider than the view, as a step's change of width under half a point isn't told: the host's own centring of a narrower document then
/// holds the page instead of Rust's, up to a quarter point times the zoom off it, or there's a sliver to scroll across. Once the zoom ends, whichever way it
/// went, the document is exactly the view's width and the page exactly in its middle.
#[test]
fn once_a_zoom_ends_the_document_is_exactly_as_wide_as_the_view() {
    // A step changes the view's width by about 0.3 points here: one, three and 41 of them leave a change under half a point untold.
    for (step, steps) in [1.0003, 1.0 / 1.0003].into_iter().flat_map(|step| [1, 3, 41].map(|steps| (step, steps))) {
        let (mut reader, mut host) = on_page_three();
        let mut magnification = 1.2;
        host.zoom_at(magnification, FRAME / 2.0 + 100.0);
        host.tell(&mut reader);
        for _ in 0..steps {
            magnification *= step;
            host.zoom_at(magnification, FRAME / 2.0 + 100.0);
            host.tell_live(&mut reader, true);
        }
        host.tell(&mut reader);
        let (left, width) = (drawn(&reader, 3), reader.docs.shown().unwrap().layout.rect(3).width);
        assert!(
            host.document_width == host.view_width() && host.x == 0.0,
            "{steps} steps of {step} to {magnification}: {} across a view {} wide, at {}",
            host.document_width,
            host.view_width(),
            host.x
        );
        assert!(
            ((left - host.x) - (host.x + host.view_width() - left - width)).abs() * magnification <= 0.5,
            "by {step}: margins {} and {}",
            left - host.x,
            host.x + host.view_width() - left - width
        );
    }
}

/// Failure (the read of 2026-11-16): the window resized, or the sidebar shown or hidden, while zoomed in moves the page across under the reader: the host
/// keeps the view's left edge, and the move across was measured after the layout had already taken the new width, or not at all. Zoomed past the window,
/// the point of the page at the middle of the view stays at its middle through a narrower window (the sidebar shown, which also makes the fit smaller) and a
/// wider one; zoomed in but still fitting, the page stays centred.
#[test]
fn a_resize_or_the_sidebar_at_a_zoom_keeps_the_page_centred_or_its_middle_still() {
    for (magnification, centred) in [(2.0, false), (1.2, true)] {
        let (mut reader, mut host) = on_page_three();
        reader.zoom(Zoom::To(magnification));
        host.apply(reader.host.take_ops());
        host.tell(&mut reader);
        for frame in [900.0, 1400.0, FRAME] {
            let was = host.page_point_under(&reader, 3, host.frame / 2.0);
            host.resize(frame);
            host.tell(&mut reader);
            let fit = reader.docs.shown().unwrap().layout.fit();
            let now = host.page_point_under(&reader, 3, frame / 2.0);
            if centred && reader.docs.shown().unwrap().layout.rect(3).width * magnification <= frame {
                assert!(
                    host.x == 0.0 && host.document_width == host.view_width() && (now - 306.0).abs() * fit * magnification <= 0.5,
                    "at {magnification}, {frame} wide: {now} at the middle, across {}",
                    host.x
                );
            } else {
                assert!(
                    (now - was).abs() * fit * magnification <= 0.5,
                    "at {magnification}, {frame} wide: {was} at the middle, then {now}"
                );
            }
        }
    }
}
