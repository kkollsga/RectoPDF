//! **Tests of the app on the headless kit** ([`crate::testkit::headless`]): what the app decides, run in this process on the core the window runs on, a file per
//! feature. Each names the app-launching test it stands in for, where it replaced one.

mod back_after_a_jump;
mod changed_under_edits;
mod closing;
mod compare;
mod contents;
mod controls;
mod coverage;
mod editing;
mod encrypted_sleep;
mod exits;
pub(crate) mod files;
mod find;
mod finding;
mod highlight; // new-feature: highlight
mod inner_guards;
mod input_to_frame;
mod late_lock;
mod late_lock_order;
mod launch;
mod levels;
pub(super) mod life_locks;
mod life_script;
mod menu_agents;
mod menu_twins;
mod navigation;
mod newer_read;
mod palette;
mod passwords;
// The count it reads is a debug build's only.
#[cfg(debug_assertions)]
mod per_tab_work;
mod protected_save;
mod quiet_files;
mod quit;
mod reading_later;
mod recent;
mod record_threads;
mod selection;
mod sidebar;
mod sleep_counts;
mod sleeping;
mod tabs;
mod twins;
mod update;
mod update_guards;
mod update_menu;
mod update_saved;
mod view;
mod window_script;

// The scaffold's template (`tools/new-feature.sh`), compiled with the tests so a change that would break what it makes fails here.
#[path = "../../../../../tools/new-feature/headless.rs"]
mod scaffold_template;
