//! **What a menu item, a panel button or a key does,** and whether it can be chosen now: the person's actions, as `AppCore` forwards them and as ⌘K's palette and
//! `recto palette --run` run them, with the menu bar as it stands.

use super::{CoreCall, HostAction, Inner, ReaderView, RecentFile, SidebarRow, find, lock, menus, saving, updates};
use crate::menus::RecentItem;
use recto_session::{Paper, TabId, ViewSettings, record};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::Ordering;

impl Inner {
    /// The tabs, one row each. Worth pulling only when `tabs_version` has moved: this copies all of them.
    pub fn sidebar_tabs(&self) -> Vec<SidebarRow> {
        let tabs = Arc::clone(&lock(&self.published).tabs);
        tabs.iter().cloned().collect()
    }

    /// **Files opened lately, newest first,** for ⌘K and anything else that offers them. From what the session thread last published: it never waits.
    pub fn recent_files(&self) -> Vec<RecentFile> {
        lock(&self.published).recent.as_ref().clone()
    }

    /// Opens a file from that list by its key, as File > Open Recent does: the very file that was offered, however the list has moved since, and nothing if it's left the list. Its tab if it's open, and otherwise it's read as any open is, and says so if it's gone.
    pub fn open_recent(&self, key: String) {
        let path = lock(&self.published)
            .recent
            .iter()
            .find(|file| file.key == key)
            .map(|file| PathBuf::from(&file.path));
        if let Some(path) = path {
            self.core.push(CoreCall::Open(path));
        }
    }

    /// Does what a menu item, a panel button or a key does, and says what's left for the host. The same methods as `recto view`.
    pub fn perform(self: &Arc<Self>, id: String) -> HostAction {
        // Past a hand-off's last look, what's done here wouldn't come back.
        if self.core.frozen.load(Ordering::SeqCst) {
            return HostAction::None;
        }
        // A window up without the state is made again once it's in, and what was done to it would go with it.
        if !updates::takes_input_now(self.window_interactive(), &id) {
            return HostAction::None;
        }
        // Features that answer ids of their own: the snapshot tool, and printing.
        if super::snapshot_tool::perform(self, &id) || super::printing::perform(self, &id) {
            return HostAction::None;
        }
        let Some(action) = menus::action(&id) else { return HostAction::None };
        match action {
            menus::Action::Host(host) => return host,
            menus::Action::Copy { anyway } => self.panes.reading_engine(&self.engine).copy_selection(anyway),
            menus::Action::SelectAll => self.panes.reading_engine(&self.engine).select_all(),
            menus::Action::SetSynced(on) => self.core.push(CoreCall::SetSynced(on)),
            menus::Action::PaletteOpen => {
                // The palette is in the sidebar, so ⌘K shows it.
                if lock(&self.published).reader.sidebar_hidden {
                    self.core.push(CoreCall::SetSidebarHidden(false));
                }
                self.tools.palette.open();
                self.listener.model_changed();
                return HostAction::FocusPalette;
            }
            menus::Action::OpenRecent(key) => self.open_recent(key),
            menus::Action::ClearRecent => self.core.push(CoreCall::ClearRecent),
            menus::Action::FindOpen => {
                self.core.push(CoreCall::Find(find::FindCall::Open));
                return HostAction::FocusFind;
            }
            menus::Action::FindStep(forward) => self.core.push(CoreCall::Find(find::FindCall::Step(forward, recto_agent::find::Going::Step))),
            menus::Action::LeaveEditMode => self.core.push(CoreCall::SetEditMode(false)),
            menus::Action::Step(back) => self.core.push(CoreCall::Step(back)),
            menus::Action::ClearEditHistory => self.core.push(CoreCall::ClearEditHistory),
            menus::Action::Zoom(zoom) => {
                self.panes.zoom_all(&self.engine, zoom);
            }
            menus::Action::ToggleSidebar => self.toggle_sidebar(),
            // The pane being read as it's chosen, which a right-click on a pane has just made it, whatever the session's still to hear.
            menus::Action::SetSpread(spread) => self.core.push(CoreCall::SetSpread(
                (self.panes.count() > 1).then(|| self.panes.tab_of(self.panes.reading())).flatten(),
                spread,
            )),
            menus::Action::Rotate(turns) => self.core.push(CoreCall::Rotate(turns)),
            menus::Action::Undo => self.core.push(CoreCall::UndoOrRedo(true)),
            menus::Action::Redo => self.core.push(CoreCall::UndoOrRedo(false)),
            menus::Action::ToggleAgentIcon => self.core.push(CoreCall::ToggleAgentIcon),
            menus::Action::SetPaper(paper) => self.core.push(CoreCall::SetPaper(paper)),
            menus::Action::TogglePanel => {
                self.window.panel_open.fetch_xor(true, Ordering::SeqCst);
                self.listener.model_changed();
            }
            // Off this thread by itself, and it says how it went.
            menus::Action::Update => self.updates.menu_chosen(),
            menus::Action::Save => {
                let reader = lock(&self.published).reader.clone();
                if self.can_save() && reader.shown_tab != 0 {
                    saving::start_save(self, TabId(reader.shown_tab), None, false);
                }
            }
            menus::Action::CloseDocument => {
                let reader = lock(&self.published).reader.clone();
                if reader.home || reader.saving {
                    return HostAction::None;
                }
                // Unsaved edits, or the owner's undone ones, which closing would lose: it asks, as any Mac app does.
                if reader.edited || lock(&self.published).sidebar.undone_owner_steps {
                    return HostAction::ConfirmClose;
                }
                if reader.shown_tab != 0 {
                    self.core.push(CoreCall::CloseTab(TabId(reader.shown_tab), false));
                }
            }
        }
        HostAction::None
    }

    /// Whether a menu item can be chosen now. Asked when a menu opens and on its key. By the item's rule in `menus`: an id without one can't be chosen.
    pub fn menu_item_enabled(&self, id: String) -> bool {
        use menus::Enable;
        let Some(rule) = menus::enable_rule(&id) else { return false };
        if !updates::takes_input_now(self.window_interactive(), &id) {
            return false;
        }
        let reader = |pick: fn(&ReaderView) -> bool| pick(&lock(&self.published).reader);
        match rule {
            Enable::Always => true,
            Enable::Document | Enable::Selection => {
                let reading = self.panes.reading_engine(&self.engine);
                menus::enabled_by_reading(rule, reading.has_document(), reading.has_selection()).unwrap_or(false)
            }
            Enable::Update => self.updates.menu_item().enabled,
            Enable::Save => self.can_save(),
            Enable::Print => reader(|reader| !reader.home && reader.shown_tab != 0) && !self.tools.printing.busy(),
            Enable::SaveAs => reader(|reader| !reader.home && !reader.saving) && self.tools.save_ui.disagreement_now().is_none(),
            Enable::Close => reader(|reader| !reader.home && !reader.saving),
            Enable::ClearRecent => !lock(&self.published).recent.is_empty(),
            Enable::Undo => reader(|reader| reader.can_undo),
            Enable::Redo => reader(|reader| reader.can_redo),
            Enable::Back => reader(|reader| reader.can_go_back),
            Enable::Forward => reader(|reader| reader.can_go_forward),
        }
    }

    /// Whether Save does anything now: the tab showing has unsaved edits and isn't being saved.
    pub fn can_save(&self) -> bool {
        let reader = lock(&self.published).reader.clone();
        // Not under a sheet: the close prompt's own Save is how to save then.
        !reader.home && reader.edited && reader.save_enabled && self.life.sheets.load(Ordering::SeqCst) == 0
    }

    /// The sidebar's button, beside the window's controls: shows the sidebar if it's hidden, and hides it if not.
    pub fn toggle_sidebar(&self) {
        let hidden = !lock(&self.published).reader.sidebar_hidden;
        self.core.push(CoreCall::SetSidebarHidden(hidden));
    }

    /// Whether the window takes what a person does: once it's been made from the state, or once the state's been waited for long enough. Until then it's shown and takes nothing, as it's made again from the state when that comes, and what was done to it would go with it.
    pub fn window_interactive(&self) -> bool {
        updates::takes_input(self.life.shown_with_state.load(Ordering::SeqCst), self.life.started.elapsed())
    }

    /// The menu bar as it stands, as recto-app's lists: what `AppCore::menu_bar` converts, and the palette lists.
    pub fn bar_now(&self) -> Vec<crate::menus::MenuModel> {
        let reader = lock(&self.published).reader.clone();
        let recent = Arc::clone(&lock(&self.published).recent);
        let items: Vec<RecentItem> = recent
            .iter()
            .map(|file| RecentItem {
                key: &file.key,
                name: &file.name,
            })
            .collect();
        let mut bar = menus::menu_bar_with(
            ViewSettings {
                paper: Paper::from_name(&reader.paper),
                sidebar_hidden: reader.sidebar_hidden,
                agent_icon_hidden: reader.agent_icon_hidden,
            },
            record::Spread::from_name(&reader.layout),
            &self.updates.menu_item().title,
            &items,
        );
        bar.push(crate::snapshot_menu::tools_menu(self.tools.tool.is_on()));
        bar
    }
}
