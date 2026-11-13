//! **The lists a scroll step fills and empties again,** kept between steps so a step over pages already drawn allocates nothing for them (rule 1). Each is emptied after the step that used it and keeps its room, up to a cap: a hostile file's step can fill one far past what a real view needs, and that room isn't held for ever.

use super::cache::TileKey;
use super::host::Placement;

/// The most pages' worth of room a page list keeps between steps. More than a real view has in it at once: a hostile file's tiny pages can put thousands in view, and past this the room goes after the step.
pub(super) const KEEP_PAGES: usize = 64;
/// The most tiles' worth of room a tile list keeps between steps: a page's tiles in view, which a 5K screen's window at 2x fills to about 270, as the view is cut in 256-pixel tiles.
pub(super) const KEEP_TILES: usize = 256;

/// What [`super::Reader::refresh`] works with, one list for each thing it lists.
#[derive(Debug, Default)]
pub(super) struct Scratch {
    /// Pages in view whose size is known.
    pub(super) visible: Vec<u32>,
    /// Pages in view that are on their paper.
    pub(super) on_paper: Vec<u32>,
    pub(super) placements: Vec<(u32, Placement)>,
    /// A page's tiles in view, nearest the middle first.
    pub(super) cells: Vec<(u16, u16)>,
    pub(super) keys: Vec<TileKey>,
    /// The entries a page's tiles in view are.
    pub(super) wanted: Vec<u64>,
    /// Pages or entries that are going: pages off the view, tiles no longer wanted.
    pub(super) going: Vec<u64>,
    pub(super) going_pages: Vec<u32>,
}

// size: held at what it was on 2026-11-13 (macOS, debug and release alike) before R3.6 moved `Reader`'s methods onto its parts, so the move, or a field added later, can't grow it unseen.
#[cfg(target_os = "macos")]
const _: () = assert!(size_of::<Scratch>() <= 192, "the lists a scroll step fills grew");

impl Scratch {
    /// Empties every list for the next step, giving back room past the caps.
    pub(super) fn reset(&mut self) {
        fn trim<T>(list: &mut Vec<T>, keep: usize) {
            list.clear();
            if list.capacity() > keep {
                list.shrink_to(keep);
            }
        }
        trim(&mut self.visible, KEEP_PAGES);
        trim(&mut self.on_paper, KEEP_PAGES);
        trim(&mut self.placements, KEEP_PAGES);
        trim(&mut self.going_pages, KEEP_PAGES);
        trim(&mut self.cells, KEEP_TILES);
        trim(&mut self.keys, KEEP_TILES);
        trim(&mut self.wanted, KEEP_TILES);
        trim(&mut self.going, KEEP_TILES);
    }

    /// The bytes the lists hold, filled or not.
    #[cfg(test)]
    pub(super) fn held_bytes(&self) -> usize {
        // A list's room is what it holds, so it's the vector that's asked.
        #[allow(clippy::ptr_arg)]
        fn bytes<T>(list: &Vec<T>) -> usize {
            list.capacity() * size_of::<T>()
        }
        bytes(&self.visible)
            + bytes(&self.on_paper)
            + bytes(&self.placements)
            + bytes(&self.going_pages)
            + bytes(&self.cells)
            + bytes(&self.keys)
            + bytes(&self.wanted)
            + bytes(&self.going)
    }

    /// The most [`Self::held_bytes`] can be between steps.
    #[cfg(test)]
    pub(super) const fn most_held_bytes() -> usize {
        KEEP_PAGES * (3 * size_of::<u32>() + size_of::<(u32, Placement)>())
            + KEEP_TILES * (size_of::<(u16, u16)>() + size_of::<TileKey>() + 2 * size_of::<u64>())
    }
}
