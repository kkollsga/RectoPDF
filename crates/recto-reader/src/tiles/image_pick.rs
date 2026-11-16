//! **The page view's side of an image picked with a click** ([`crate::image_pick`]): what's waiting, what's found, and its box told to the host, as free
//! functions of the page view, as its marks and withheld pages are.

use super::{Op, Reader};
use crate::image_pick::ImagePick;
use recto_index::select::{PagePoint, Rect};

/// The box told to the host as it should be now, on the pages shown. Part of [`Reader::highlight_ops`], so it goes wherever the selection's layer does.
pub(super) fn box_ops(reader: &mut Reader) {
    let shown = &reader.host.shown;
    let ops = reader.pointer.selecting.image.update(|page| shown.contains_key(&page));
    for (page, rect) in ops.into_iter().flatten() {
        reader.host.ops.push_back(Op::ImageBox { page, rect });
    }
}

/// The click waiting for its image, on the tab showing.
pub(crate) fn wanted(reader: &Reader) -> Option<(u32, PagePoint)> {
    Some((reader.docs.hot?, reader.pointer.selecting.image.wanted()?))
}

/// **The image found for the click at `point` on `tab`,** held if that click is still the one waiting on the tab showing and nothing's been selected
/// since: picked, or waiting for its page's hidden-text check to say it isn't on text that shows. Says whether it's held.
pub(crate) fn found(reader: &mut Reader, tab: u32, point: PagePoint, number: u32, rect: Rect) -> bool {
    if reader.docs.hot != Some(tab) {
        return false;
    }
    let Some(size) = reader.docs.shown().and_then(|doc| doc.page_size_shown(point.page)) else {
        return false;
    };
    let heavy = reader.docs.heavy_pages().get(&point.page).copied().unwrap_or(false);
    let selecting = &mut reader.pointer.selecting;
    if selecting.has_selection() {
        selecting.image.not_found(point);
        return false;
    }
    let held = selecting.image.found(point, number, rect, size, heavy);
    if held {
        reader.highlight_ops();
    }
    held
}

/// No image for the click at `point` on `tab`, or none could be looked for.
pub(crate) fn not_found(reader: &mut Reader, tab: u32, point: PagePoint) {
    if reader.docs.hot == Some(tab) {
        reader.pointer.selecting.image.not_found(point);
    }
}

/// The image picked on the tab showing, with that tab.
pub(crate) fn picked(reader: &Reader) -> Option<(u32, ImagePick)> {
    Some((reader.docs.hot?, reader.pointer.selecting.image.picked()?))
}

/// The image picked, and any click waiting, let go, and its box taken off. Says whether there was either.
pub(crate) fn let_go(reader: &mut Reader) -> bool {
    let image = &mut reader.pointer.selecting.image;
    let was = image.picked().is_some() || image.wanted().is_some();
    image.let_go();
    reader.highlight_ops();
    was
}
