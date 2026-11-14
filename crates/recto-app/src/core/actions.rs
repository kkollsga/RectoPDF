//! **What a menu item, a panel button or a key does,** and whether it can be chosen now: the person's actions, as `AppCore` forwards them and as ⌘K's palette and
//! `recto palette --run` run them, with the menu bar as it stands.

use super::{CoreCall, HostAction, Inner, ReaderView, RecentFile, SidebarRow, lock, menus, updates};
use crate::menus::RecentItem;
use recto_session::{Paper, ViewSettings, record};
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

    /// Does what a menu item, a panel button or a key does, and says what's left for the host: the row `menu_rows` has for its id, run as the person's.
    pub fn perform(self: &Arc<Self>, id: String) -> HostAction {
        // Past a hand-off's last look, what's done here wouldn't come back.
        if self.core.frozen.load(Ordering::SeqCst) {
            return HostAction::None;
        }
        // A window up without the state is made again once it's in, and what was done to it would go with it.
        if !updates::takes_input_now(self.life.window_interactive(), &id) {
            return HostAction::None;
        }
        super::menu_rows::row(self, &id).map_or(HostAction::None, |row| super::menu_rows::by_person(self, row))
    }

    /// Whether a menu item can be chosen now. Asked when a menu opens and on its key. By the item's rule in `menus`: an id without one can't be chosen.
    pub fn menu_item_enabled(&self, id: String) -> bool {
        use menus::Enable;
        let Some(rule) = menus::enable_rule(&id) else { return false };
        if !updates::takes_input_now(self.life.window_interactive(), &id) {
            return false;
        }
        let reader = |pick: fn(&ReaderView) -> bool| pick(&lock(&self.published).reader);
        match rule {
            Enable::Always => true,
            Enable::Document | Enable::Selection => {
                let reading = self.panes.reading_engine(&self.engine);
                menus::enabled_by_reading(rule, reading.points().has_document(), reading.selection().has_any()).unwrap_or(false)
            }
            Enable::Update => self.updates.menu_item().enabled,
            Enable::Save => self.can_save(),
            Enable::Print => reader(|reader| !reader.home && reader.shown_tab != 0) && !self.tools.printing().busy(),
            Enable::SaveAs => reader(|reader| !reader.home && !reader.saving) && self.tools.save_ui().disagreement_now().is_none(),
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
        !reader.home && reader.edited && reader.save_enabled && self.life.sheets_open() == 0
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
        bar.push(super::highlight::mark_up_menu());
        bar.push(crate::snapshot_menu::tools_menu(self.tools.tool().is_on()));
        bar
    }

    /// The reader has dismissed the notice, or its time's up: every notice that goes by itself goes.
    pub fn dismiss_selection_notice(&self) {
        for engine in self.panes.engines(&self.engine) {
            engine.selection().dismiss_notice();
        }
        self.window.notices().take_notice();
        self.tools.tool().dismiss_notice();
        self.tools.printing().dismiss_notice();
        self.updates.dismiss_notice();
        self.core.push(CoreCall::Reconcile);
    }
}
