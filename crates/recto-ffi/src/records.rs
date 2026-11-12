//! **The window's records, from `recto-app`'s plain models.** One conversion for each record the host sees, field for field: no rule, no default that decides anything. The records
//! themselves stay in `lib.rs` because their shape is the Swift contract. The three sheets that ask something (`save_problem`, `link_prompt`, `password_prompt`) are the bridge's
//! own and are set by the caller after the conversion.

use crate::{AppListener, PaneView, QuitAnswer, ReaderView, RowKind, SelectionNotice, SidebarRow, SidebarView};
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
        self.0.quit_answer(answer.into());
    }

    fn hand_off_quit(&self) {
        self.0.hand_off_quit();
    }
}

impl From<app::listener::QuitAnswer> for QuitAnswer {
    fn from(answer: app::listener::QuitAnswer) -> Self {
        Self {
            go: answer.go,
            title: answer.title,
            message: answer.message,
            quit_button: answer.quit_button,
            stay_button: answer.stay_button,
        }
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

impl From<app::PaneView> for PaneView {
    fn from(pane: app::PaneView) -> Self {
        Self {
            tab: pane.tab,
            title: pane.title,
            reading: pane.reading,
            layout: pane.layout,
            note: pane.note,
        }
    }
}

impl From<app::SelectionNotice> for SelectionNotice {
    fn from(notice: app::SelectionNotice) -> Self {
        Self {
            text: notice.text,
            action_title: notice.action_title,
            action_id: notice.action_id,
            fades: notice.fades,
        }
    }
}

impl From<SelectionNotice> for app::SelectionNotice {
    fn from(notice: SelectionNotice) -> Self {
        Self {
            text: notice.text,
            action_title: notice.action_title,
            action_id: notice.action_id,
            fades: notice.fades,
        }
    }
}

/// The reader model with its sheets empty: the caller sets the bridge's own.
impl From<app::ReaderModel> for ReaderView {
    fn from(model: app::ReaderModel) -> Self {
        Self {
            panes: model.panes.into_iter().map(Into::into).collect(),
            compare_synced: model.window.compare_synced,
            home: model.window.home,
            note: model.note,
            sidebar_hidden: model.window.sidebar_hidden,
            layout: model.document.layout,
            edited: model.edit.edited,
            can_undo: model.edit.can_undo,
            can_redo: model.edit.can_redo,
            edit_mode: model.edit.edit_mode,
            can_rotate: model.edit.can_rotate,
            place: model.document.place,
            paper: model.paper.name,
            paper_rgb: model.paper.rgb,
            agent_icon_hidden: model.window.agent_icon_hidden,
            page_tint: model.paper.tint,
            sidebar_width: model.window.sidebar_width,
            highlight_dark_colour: model.paper.highlight_dark_colour,
            highlight_lighten: model.paper.highlight_lighten,
            selection_notice: model.selection_notice.map(Into::into),
            saving: model.save.saving,
            save_tooltip: model.save.tooltip,
            save_problem: None,
            save_as_note: model.save.as_note,
            shown_tab: model.document.shown_tab,
            document_name: model.document.name,
            document_folder: model.document.folder,
            save_button: model.save.button,
            print_ready: model.window.print_ready,
            password_prompt: None,
            save_enabled: model.save.enabled,
            can_go_back: model.edit.can_go_back,
            can_go_forward: model.edit.can_go_forward,
            link_prompt: None,
            page_text: model.document.page_text,
        }
    }
}
