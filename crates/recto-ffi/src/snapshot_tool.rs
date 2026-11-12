//! **The clipboard's calls:** what a copy offers, one type's bytes, and letting it go. The tool and its state are `crate::core::snapshot_tool`'s.

use crate::core::snapshot_tool::Data;
use crate::{AppCore, lock};
use recto_agent::extract::{self, ClipItem};
use recto_reader::Engine;
use std::sync::atomic::Ordering;

/// What the clipboard is offered: its types in the order to offer them, which of those are made only when a paste asks, and the number that names this offer for [`AppCore::clipboard_data`].
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct ClipboardOffer {
    pub serial: u64,
    /// Uniform type identifiers, such as `public.png`, in the order a paste should prefer them.
    pub types: Vec<String>,
    /// The ones made only when a paste asks for them: the host declares them and gives them when asked.
    pub promised: Vec<String>,
}

#[uniffi::export]
impl AppCore {
    /// What a copy has for the clipboard, a snapshot's or the selection's text, as one offer: the types in order, and which are made only when a paste asks. `copy_ready` says when. The bytes come one type at a time from `clipboard_data`, so no more than one is copied across at once.
    pub fn take_clipboard(&self) -> Option<ClipboardOffer> {
        let tool = &self.inner.tools.tool;
        let mut offered = lock(&tool.ready).take().or_else(|| {
            self.inner
                .panes
                .engines(&self.inner.engine)
                .iter()
                .find_map(Engine::take_copied)
                .map(|copied| tool.offer_text(copied.text))
        })?;
        offered.serial = tool.serial.fetch_add(1, Ordering::SeqCst) + 1;
        let types = offered.items.iter().map(|item| item.uti().to_string()).collect();
        let promised = offered
            .items
            .iter()
            .filter(|item| matches!(item, ClipItem::Promised { .. }))
            .map(|item| item.uti().to_string())
            .collect();
        let offer = ClipboardOffer {
            serial: offered.serial,
            types,
            promised,
        };
        // Whatever was offered before is gone from the clipboard now, its promise with it.
        *lock(&tool.offered) = Some(offered);
        Some(offer)
    }

    /// One type's bytes for the offer with this number, once. A type made with the copy is handed over and dropped from here. A promised one is made now, as a paste asks for it, which takes a worker and can take seconds: the system asks on the UI thread and waits for the answer, which is the one time the UI thread waits here, and only for a paste of that type from another app. Nothing for an offer replaced since, or on the way out.
    pub fn clipboard_data(&self, serial: u64, uti: String) -> Option<Vec<u8>> {
        match self.inner.tools.tool.data(serial, &uti)? {
            Data::Ready(bytes) => Some(bytes),
            Data::Make(promise, cap) => extract::snapshot_svg(&promise.path, promise.page, promise.rect, promise.turns, cap).ok(),
        }
    }

    /// The clipboard has something else on it now: what this offer promised is let go.
    pub fn clipboard_released(&self, serial: u64) {
        let mut offered = lock(&self.inner.tools.tool.offered);
        if offered.as_ref().is_some_and(|offered| offered.serial == serial) {
            offered.take();
        }
    }
}
