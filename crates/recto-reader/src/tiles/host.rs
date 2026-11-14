//! **What the host's been told:** the ops on their way, in the order it applies them, the pages' layers placed, and what's gone off a layer, which goes discardable once the host says it's applied what took it off.

use super::Op;
use recto_platform::surface::PixelSurface;
use std::collections::{HashMap, VecDeque};

/// The most entries' room each list of what's gone off a layer keeps once it's empty: a page's tiles in view and more.
pub(super) const KEEP_GONE: usize = 256;
/// The most ops' room the queue keeps once it's empty: a turn's attachments and the pages' layers placed, several times over.
pub(super) const KEEP_OPS: usize = 64;

/// How a page sits.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Placement {
    pub(super) x: f64,
    pub(super) y: f64,
    pub(super) width: f64,
    pub(super) height: f64,
    pub(super) fit: f64,
    pub(super) paper_rgb: u32,
    pub(super) turns: u8,
}

/// Built only in the tiles module ([`HostQueue::new`]), with no `Default`, as each of [`Reader`]'s parts is (see there): the engine reaches it through `Reader`'s field to ask whether there are ops, and to hand it to the cache.
#[derive(Debug)]
pub(crate) struct HostQueue {
    pub(super) shown: HashMap<u32, Placement>,
    pub(super) ops: VecDeque<Op>,
    /// The host has been given ops it hasn't said it's applied.
    pub(super) unapplied: bool,
    /// Gone off a layer, in ops the host hasn't been given yet.
    pub(super) to_discard: Vec<u64>,
    /// Gone off a layer, in ops the host has been given. It's these that "applied" is about.
    pub(super) handed_to_host: Vec<u64>,
    pub(super) more_to_attach: bool,
}

// size: held at what it was on 2026-11-13 (macOS, debug and release alike) before R3.6 moved `Reader`'s methods onto its parts, so the move, or a field added later, can't grow it unseen.
#[cfg(target_os = "macos")]
const _: () = assert!(size_of::<HostQueue>() <= 136, "what the host's told grew");

impl HostQueue {
    pub(super) fn new() -> Self {
        Self {
            shown: HashMap::new(),
            ops: VecDeque::new(),
            unapplied: false,
            to_discard: Vec::new(),
            handed_to_host: Vec::new(),
            more_to_attach: false,
        }
    }

    pub(super) fn take_ops(&mut self) -> Vec<Op> {
        if !self.ops.is_empty() {
            self.unapplied = true;
        }
        self.handed_to_host.append(&mut self.to_discard);
        self.ops.drain(..).collect()
    }

    /// Gives back the room of the lists of what's gone off a layer past [`KEEP_GONE`], once they're empty: a step that took a hostile file's thousands of tiles off doesn't leave that room held.
    pub(super) fn trim(&mut self) {
        for list in [&mut self.to_discard, &mut self.handed_to_host] {
            if list.is_empty() && list.capacity() > KEEP_GONE {
                list.shrink_to(KEEP_GONE);
            }
        }
        if self.ops.is_empty() && self.ops.capacity() > KEEP_OPS {
            self.ops.shrink_to(KEEP_OPS);
        }
    }

    /// The bytes the queue's lists hold, filled or not.
    #[cfg(test)]
    pub(super) fn held_bytes(&self) -> usize {
        (self.to_discard.capacity() + self.handed_to_host.capacity()) * size_of::<u64>() + self.ops.capacity() * size_of::<Op>()
    }

    /// Whether the host should come back for more.
    pub(crate) fn has_ops(&self) -> bool {
        !self.ops.is_empty() || self.more_to_attach
    }

    /// The window closed: nothing's there to take what's waiting.
    pub(super) fn close(&mut self) {
        // Nothing's there to take them. What was waiting is thrown away, and each `Attach` holds a reference to a surface, which goes with it.
        self.release_waiting();
        // Their room too: a closed window holds nothing here.
        self.ops = VecDeque::new();
        self.to_discard = Vec::new();
        self.handed_to_host = Vec::new();
        self.unapplied = false;
        self.more_to_attach = false;
    }
}

impl HostQueue {
    /// Each `Attach` waiting holds a reference to its surface for the host, which only the host or this lets go of.
    fn release_waiting(&mut self) {
        for op in self.ops.drain(..) {
            if let Op::Attach { surface, .. } = op {
                PixelSurface::release_for_host(surface);
            }
        }
    }
}

/// **An `Attach` never outlives its queue unreleased,** however the queue goes: a comparison's pane let go with tiles still waiting for its layers
/// (`Engine::shut_down`, then the page view dropped, while the window's page area had already stopped taking ops), as well as a window closing. Before
/// (2026-11-13), only a closed window gave them back, so a pane let go mid-draw kept each waiting tile's surface for the rest of the app's life: a turn's
/// attachments (8) at up to 1 MiB a tile and 4 MiB a preview, for every pane let go that way. In the one place every way of going passes through.
impl Drop for HostQueue {
    fn drop(&mut self) {
        self.release_waiting();
    }
}

#[cfg(test)]
mod tests {
    /// Failure (2026-11-13, found looking for the headless kit's surface leak in the window): a page view let go with an `Attach` still waiting, as a
    /// comparison's pane is, kept the host's reference to that tile's surface for good. Dropped with one waiting, the surface is held by its maker alone.
    #[test]
    fn a_queue_let_go_with_a_tile_waiting_lets_go_of_its_surface() {
        let surface = recto_platform::surface::PixelSurface::create(16, 16).expect("a surface");
        let before = surface.reference_count();
        let mut queue = super::HostQueue::new();
        queue.ops.push_back(super::Op::Attach {
            id: 1,
            page: 0,
            x: 0.0,
            y: 0.0,
            width: 16.0,
            height: 16.0,
            surface: surface.retained_for_host(),
            rgb: None,
            crop_width: 16.0,
            crop_height: 16.0,
            below: false,
        });
        assert_eq!(surface.reference_count(), before + 1, "the host's reference, waiting");
        drop(queue);
        assert_eq!(surface.reference_count(), before, "let go with its queue");
    }
}
