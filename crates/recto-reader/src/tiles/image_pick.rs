//! **The page view's side of an image picked with a click** ([`crate::image_pick`]): the click, what its page's text and the tab's own worker are asked
//! for it, the answers, and its box told to the host, as free functions of the page view, as its marks and withheld pages are. Nothing here starts a
//! worker of its own: the text read, the hidden-text check and the image boxes are jobs of the thread that draws, for the worker it has.

use super::{JobPlan, Op, Outcome, PlanKind, Reader};
use crate::image_pick::{ImagePick, Under, Verdict, in_parts, verdict_at};
use crate::selection::Urgency;
use recto_index::select::{PagePoint, Rect};

/// The box told to the host as it should be now, on the pages shown. Part of [`Reader::highlight_ops`], so it goes wherever the selection's layer does.
pub(super) fn box_ops(reader: &mut Reader) {
    let shown = &reader.host.shown;
    let ops = reader.pointer.selecting.image.update(|page| shown.contains_key(&page));
    for (page, rect) in ops.into_iter().flatten() {
        reader.host.ops.push_back(Op::ImageBox { page, rect });
    }
}

/// Whether a page of the tab showing is known to be heavy, which is never drawn for a hidden-text check.
fn heavy(reader: &Reader, page: u32) -> bool {
    reader.docs.shown().and_then(|doc| doc.drawing.heavy.get(&page).copied()).unwrap_or(false)
}

/// **The button came up with `still`,** where a still single click off any link landed ([`crate::links::Linking::release`]): its image is wanted unless
/// something's selected now, with what the page's text says there, as the selection clock's next serial. Nothing's sent from here: the thread that draws
/// asks what's needed next ([`plan`]).
pub(super) fn clicked(reader: &mut Reader, still: Option<PagePoint>) {
    let Some(point) = still else { return };
    if reader.pointer.selecting.has_selection() {
        return;
    }
    let heavy = heavy(reader, point.page);
    let selecting = &mut reader.pointer.selecting;
    let verdict = selecting
        .held_text(point.page)
        .map_or(Verdict::Unknown, |(text, settled)| verdict_at(Some(text), settled || heavy, point));
    let serial = selecting.tick();
    selecting.image.clicked(point, serial, verdict);
}

/// What a click waiting for its image needs next of the thread that draws.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Need {
    /// What its page's text says is known now: taken with no job.
    Settle(Verdict),
    /// What its page's text says after the hidden-text check is known now (tried, or the page heavy): taken with no job.
    Checked(Verdict),
    /// Its page's text, read, or with `check` drawn for the hidden-text check too.
    Read { page: u32, check: bool },
    /// The worker's boxes, at `at` in 65535ths of the page.
    Boxes { page: u32, serial: u32, at: [u16; 2] },
    /// Its page has no size to ask about: nothing's there to pick.
    Nothing(u32),
}

/// What the text held for the page of `point` says there, if any is held: a heavy page's, or one whose check was tried, is taken as showing.
fn verdict_here(reader: &Reader, point: PagePoint) -> Verdict {
    let held = reader.pointer.selecting.held_text(point.page);
    verdict_at(
        held.map(|(text, _)| text),
        held.is_some_and(|(_, settled)| settled) || heavy(reader, point.page),
        point,
    )
}

/// **What a click waiting on the tab showing needs,** if anything: its text read when none is held; the worker's boxes once no text that shows is there,
/// or text not checked; and only where a picture may be under text not checked, the hidden-text check.
fn need(reader: &Reader) -> Option<Need> {
    let doc = reader.docs.shown()?;
    let image = &reader.pointer.selecting.image;
    if let Some(point) = image.text_wanted() {
        if point.page >= doc.layout.count() {
            return Some(Need::Settle(Verdict::Text));
        }
        return Some(match verdict_here(reader, point) {
            Verdict::Unknown => Need::Read {
                page: point.page,
                check: false,
            },
            verdict => Need::Settle(verdict),
        });
    }
    if let Some(point) = image.check_wanted() {
        if point.page >= doc.layout.count() {
            return Some(Need::Checked(Verdict::Text));
        }
        return Some(match verdict_here(reader, point) {
            Verdict::Unknown | Verdict::Unchecked => Need::Read { page: point.page, check: true },
            verdict => Need::Checked(verdict),
        });
    }
    let (point, serial) = image.boxes_wanted()?;
    Some(match doc.page_size_shown(point.page).and_then(|size| in_parts(point, size)) {
        Some(at) => Need::Boxes { page: point.page, serial, at },
        None => Need::Nothing(serial),
    })
}

/// **The next job for a click waiting on `tab`,** at this urgency: its text read and the worker's boxes are quick, and go before tiles; the hidden-text
/// check draws the page, and goes when there's nothing better to do, as a selection's does. What's known with no job is taken here, and what it makes the
/// app's news is told as the thread that draws plans (`engine/run.rs`).
pub(super) fn plan(reader: &mut Reader, tab: u32, urgency: Urgency) -> Option<JobPlan> {
    let kind = loop {
        match need(reader)? {
            Need::Settle(verdict) => {
                let image = &mut reader.pointer.selecting.image;
                let page = image.text_wanted()?.page;
                image.text_read(page, |_| verdict);
            }
            Need::Checked(verdict) => {
                let image = &mut reader.pointer.selecting.image;
                let page = image.check_wanted()?.page;
                image.check_read(page, |_| verdict);
            }
            Need::Nothing(serial) => reader.pointer.selecting.image.boxes_said(serial, Under::No),
            Need::Read { page, check } if check == (urgency == Urgency::WhenIdle) => break (page, PlanKind::Text { check }),
            Need::Boxes { page, serial, at } if urgency == Urgency::Now => break (page, PlanKind::ImageBoxes { serial, at }),
            Need::Read { .. } | Need::Boxes { .. } => return None,
        }
    };
    let (page, kind) = kind;
    let (file, source_page) = reader.docs.views.get(&tab)?.map.source(page)?;
    let generation = reader.work.fresh_generation();
    Some(JobPlan {
        tab,
        position: page,
        file,
        source_page,
        generation,
        asked: super::place::asked_now(&reader.place),
        kind,
    })
}

/// Whether a click waiting for its image has something left for the thread that draws: the view isn't settled till it's said.
pub(super) fn busy(reader: &Reader) -> bool {
    need(reader).is_some()
}

/// A click waiting on its page's hidden-text check, decided now if the text held says (its check in, or the page heavy). After a text job and the
/// worker's boxes, so what it decides is told with that job's end.
pub(super) fn text_came(reader: &mut Reader) {
    if let Some(point) = reader.pointer.selecting.image.check_wanted() {
        let verdict = verdict_here(reader, point);
        reader.pointer.selecting.image.check_read(point.page, |_| verdict);
    }
}

/// **What the worker said of the `serial`th click on `tab`:** taken if it's still the one waiting for it. A job stopped is asked again; any other end (the
/// page's drawing failed or was stopped, withheld, its file refused) is no picture there, and the click picks nothing.
pub(super) fn boxes_said(reader: &mut Reader, tab: u32, serial: u32, outcome: Outcome) {
    if reader.docs.hot != Some(tab) {
        return;
    }
    let under = match outcome {
        Outcome::ImageUnder(under) => under,
        Outcome::Cancelled => return,
        _ => Under::No,
    };
    reader.pointer.selecting.image.boxes_said(serial, under);
    text_came(reader);
}

/// The click the app was told to look for, on the tab showing: the tab, the point, and its serial.
pub(crate) fn looking(reader: &Reader) -> Option<(u32, PagePoint, u32)> {
    let (point, serial) = reader.pointer.selecting.image.looking()?;
    Some((reader.docs.hot?, point, serial))
}

/// Whether a click on the tab showing is waiting for its image, at whatever stage.
pub(crate) fn finding(reader: &Reader) -> bool {
    reader.docs.hot.is_some() && reader.pointer.selecting.image.finding()
}

/// **The image found for the `serial`th click on `tab`,** held if that click is still the one looked for on the tab showing and nothing's been selected
/// since. Says whether it's picked.
pub(crate) fn found(reader: &mut Reader, tab: u32, serial: u32, number: u32, rect: Rect) -> bool {
    if reader.docs.hot != Some(tab) {
        return false;
    }
    let Some((point, _)) = reader.pointer.selecting.image.looking().filter(|(_, looking)| *looking == serial) else {
        return false;
    };
    let Some(size) = reader.docs.shown().and_then(|doc| doc.page_size_shown(point.page)) else {
        return false;
    };
    let selecting = &mut reader.pointer.selecting;
    if selecting.has_selection() {
        selecting.image.not_found(serial);
        return false;
    }
    let held = selecting.image.found(serial, number, rect, size);
    if held {
        reader.highlight_ops();
    }
    held
}

/// No image for the `serial`th click on `tab`, or none could be looked for.
pub(crate) fn not_found(reader: &mut Reader, tab: u32, serial: u32) {
    if reader.docs.hot == Some(tab) {
        reader.pointer.selecting.image.not_found(serial);
    }
}

/// The image picked on the tab showing, with that tab.
pub(crate) fn picked(reader: &Reader) -> Option<(u32, ImagePick)> {
    Some((reader.docs.hot?, reader.pointer.selecting.image.picked()?))
}

/// The image picked, and any click waiting, let go, and its box taken off. Says whether there was either.
pub(crate) fn let_go(reader: &mut Reader) -> bool {
    let image = &mut reader.pointer.selecting.image;
    let was = image.picked().is_some() || image.finding();
    image.let_go();
    reader.highlight_ops();
    was
}
