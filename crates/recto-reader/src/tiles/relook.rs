//! **A page's look changed on dark paper** (`engine/look/heal.rs`: a look kept for want of an answer was asked again and came out different): the page view's side of drawing it again with no blank frame and never two looks at once.
//!
//! What it does, in order, is the engine's turn ([`stand_in_plan`], then a draw, then [`drop_look`] and `finish` under one hold of the page view's lock):
//! 1. a plan for the page's preview, which is the stand-in while its tiles are drawn again ([`stand_in_plan`]);
//! 2. the engine draws it, which goes by the new look and makes no walk;
//! 3. under the one lock, every drawn thing of that file page on dark paper, in every held tab, is dropped ([`drop_look`], told to the host as any entry that goes is, and as the shown tab's `Detach`es), and the preview is taken in and attached to the bare page. The host hears "old tiles off, new preview on" as one batch.
//!
//! Where no preview is possible (zoomed out past one, so the page is a few tiles) nothing is asked and nothing dropped: the page keeps its look until it is zoomed in.

use super::{DrawKind, JobPlan, PageMap, PlanKind, Reader};
use crate::paper::PaperKind;
use std::path::{Path, PathBuf};

/// The pages on show of the tab showing, as the file and page a renderer knows them by, with a generation of their own, when the paper is dark: the pages whose look may be asked again at rest.
pub(crate) fn on_dark_paper(reader: &mut Reader) -> Option<(Vec<(PathBuf, u32)>, u64)> {
    if reader.paper != PaperKind::Dark {
        return None;
    }
    super::image_pick::pages_on_show(reader)
}

/// **The plan for a page's preview, drawn again by its new look,** for the first position of the tab showing that is `source_page` of `file` and in view: as the preview of a page in view is planned, whether or not one is kept. `None` when no preview is possible there (zoomed out past one, too large a picture) or the page isn't in view.
pub(crate) fn stand_in_plan(reader: &mut Reader, file: &Path, source_page: u32) -> Option<JobPlan> {
    let tab = reader.docs.hot?;
    let doc = reader.docs.views.get(&tab)?;
    let scale = doc.drawing.tile_scale;
    if scale <= 0.0 || doc.drawing.problem.is_some() {
        return None;
    }
    let position = reader
        .visible_pages(doc)
        .find(|&position| doc.map.file_of(position).is_some_and(|(of, page)| of == file && page == source_page))?;
    let (preview_scale, rect) = reader.preview_rect(doc, position, scale)?;
    let paper = reader.paper;
    let generation = reader.work.fresh_generation();
    Some(JobPlan {
        tab,
        position,
        file: file.to_path_buf(),
        source_page,
        generation,
        asked: super::place::asked_now(&reader.place),
        kind: PlanKind::Draw {
            scale: preview_scale,
            rect,
            what: DrawKind::Preview,
            paper,
        },
    })
}

/// **Drops what's drawn of a file's page on dark paper, in every tab the page view holds,** previews and tiles, attached or kept, so nothing of the old look is left anywhere: the shown tab's attached ones are taken off their layer (`Detach`, a tile of one colour too), and the rest forgotten. Done under the same hold of the lock as the new preview's `finish`, which attaches it to the bare page. A page with another paper's pictures keeps them: their look isn't this one's.
pub(crate) fn drop_look(reader: &mut Reader, file: &Path, source_page: u32) {
    let going: Vec<(u64, u32, bool)> = reader
        .docs
        .views
        .iter()
        .flat_map(|(&tab, doc)| {
            let positions = positions_of(&doc.map, file, source_page);
            reader
                .cache
                .entries
                .values()
                .filter(move |entry| entry.tab == tab && entry.paper == PaperKind::Dark && positions.contains(&entry.page))
                .map(|entry| (entry.id, entry.page, entry.attached))
                .collect::<Vec<_>>()
        })
        .collect();
    for (id, page, attached) in going {
        if attached {
            reader.detach(id, page);
        }
        reader.cache.forget(id, &mut reader.host);
    }
}

/// The positions of a tab that show `page` of `file`.
fn positions_of(map: &PageMap, file: &Path, page: u32) -> Vec<u32> {
    match &map.slots {
        None => map
            .files
            .first()
            .filter(|first| first.as_path() == file)
            .map(|_| vec![page])
            .unwrap_or_default(),
        Some(slots) => slots
            .iter()
            .enumerate()
            .filter(|(_, (of, there))| *there == page && map.files.get(usize::from(*of)).is_some_and(|of| of.as_path() == file))
            .filter_map(|(at, _)| u32::try_from(at).ok())
            .collect(),
    }
}
