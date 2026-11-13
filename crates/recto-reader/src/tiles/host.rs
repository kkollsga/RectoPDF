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

#[derive(Debug, Default)]
pub(super) struct HostQueue {
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

impl HostQueue {
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
    pub(super) fn has_ops(&self) -> bool {
        !self.ops.is_empty() || self.more_to_attach
    }

    /// The window closed: nothing's there to take what's waiting.
    pub(super) fn close(&mut self) {
        // Nothing's there to take them. What was waiting is thrown away, and each `Attach` holds a reference to a surface, which goes with it.
        for op in self.ops.drain(..) {
            if let Op::Attach { surface, .. } = op {
                PixelSurface::release_for_host(surface);
            }
        }
        // Their room too: a closed window holds nothing here.
        self.ops = VecDeque::new();
        self.to_discard = Vec::new();
        self.handed_to_host = Vec::new();
        self.unapplied = false;
        self.more_to_attach = false;
    }
}
