//! **The window's records, from `recto-app`'s plain models.** One conversion for each record the host sees, field for field: no rule, no default that decides anything. The records
//! themselves stay in `lib.rs` because their shape is the Swift contract. The three sheets that ask something (`save_problem`, `link_prompt`, `password_prompt`) are the bridge's
//! own and are set by the caller after the conversion.

use crate::{AppListener, RowKind, SidebarRow, SidebarView};
use recto_app as app;
use std::sync::Arc;

/// The app core's listener is the host's: each call forwards, one for one and on the thread it was made on, to the UniFFI listener. The promises are `recto_app::Listener`'s.
pub(crate) struct HostListener(pub(crate) Arc<dyn AppListener>);

impl app::Listener for HostListener {
    fn model_changed(&self) {
        self.0.model_changed();
    }

    fn ops_ready(&self) {
        self.0.ops_ready();
    }

    fn qa_request(&self, id: u64, verb: String, value: String) {
        self.0.qa_request(id, verb, value);
    }

    fn session_ended(&self) {
        self.0.session_ended();
    }

    fn copy_ready(&self) {
        self.0.copy_ready();
    }

    fn restore_ready(&self) {
        self.0.restore_ready();
    }

    fn quit_answer(&self, answer: app::listener::QuitAnswer) {
        self.0.quit_answer(answer);
    }

    fn hand_off_quit(&self) {
        self.0.hand_off_quit();
    }
}

impl From<app::RowKind> for RowKind {
    fn from(kind: app::RowKind) -> Self {
        match kind {
            app::RowKind::Tab => Self::Tab,
            app::RowKind::Contents => Self::Contents,
            app::RowKind::Note => Self::Note,
        }
    }
}

impl From<app::SidebarRow> for SidebarRow {
    fn from(row: app::SidebarRow) -> Self {
        Self {
            kind: row.kind.into(),
            tab: row.tab,
            text: row.text,
            page: row.page,
            depth: row.depth,
            current: row.current,
            rail_filled: row.rail_filled,
            rail_first: row.rail_first,
            rail_last: row.rail_last,
            notice: row.notice,
            action: row.action,
            expandable: row.expandable,
            expanded: row.expanded,
            in_comparison: row.in_comparison,
        }
    }
}

impl From<app::SidebarView> for SidebarView {
    fn from(view: app::SidebarView) -> Self {
        Self {
            tabs_version: view.tabs_version,
            tab_count: view.tab_count,
            active_index: view.active_index,
            contents_count: view.contents_count,
            contents_version: view.contents_version,
            contents_changed_from: view.contents_changed_from,
            contents_removed: view.contents_removed,
            contents_inserted: view.contents_inserted,
            contents_place_shift: view.contents_place_shift,
            contents_current: view.contents_current,
            toc_open: view.toc_open,
            edited: view.edited,
            undone_owner_steps: view.undone_owner_steps,
            contents_note: view.contents_note.map(Into::into),
        }
    }
}
