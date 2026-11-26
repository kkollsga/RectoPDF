//! **What a command is, for the person's lane and for a quit** (review of the lane, D7 and D8): two choices every command makes, by its tag, in lists
//! with no default, and a test over every command in the table that each is in exactly one list of each pair. So a new command can't fall through into
//! "not ordered" or "only reads" by being forgotten: the test names it until it's placed.
//!
//! - **Ordered** (`crate::core::person_lane`): the person's command keeps its place among their edits, as it changes a document's state or fixes what's
//!   read of it (every edit but the list and edit mode, a save, a print and a close; and by its arguments the snapshot tool's box and click, [`ordered`]).
//! - **Lands later** (`crate::core::person_lane`): an edit run on a thread of its own whose effect the session publishes only as it lands, so a close
//!   asks and Undo and Save are on while it lands.
//! - **Changes** (the quit's wait, `core::running`): **anything it does can be in what the quit writes** (the record, a journal, a document's file), so a
//!   quit waits for it: a jump, the view, an open, a close, a tab's move, an edit, a save, a session's restore or save. A read, a render, an export or a
//!   print to a new file of its own, a search or a look doesn't. Where a tag is both by its arguments, or it isn't clear, it counts as changing: the cost
//!   of that is only a quit waiting for it.
//!
//! The property belongs on the command (`Cmd`, rule 7) and moves there once the agent crate is free to take it (PLAN); this is the stand-in, and the
//! test is what keeps it true meanwhile.

use recto_agent::Command;

/// The person's commands that keep their order.
const ORDERED: &[&str] = &[
    "undo",
    "redo",
    "page_rotate",
    "page_move",
    "page_delete",
    "page_insert",
    "discard",
    "clear_history",
    "save",
    "highlight",
    "highlight_remove",
    "print",
    "close",
];

/// Every other command: goes as it always has. Read only by the test that every command is placed.
#[cfg(test)]
const NOT_ORDERED: &[&str] = &[
    "back",
    "budget",
    "budget_render",
    "budget_viewport",
    "compare",
    "contents",
    "contents_fold",
    "contents_show",
    "contents_stop",
    "copy",
    "edit_mode",
    "edits",
    "export_svg",
    "facts",
    "find",
    "follow",
    "forward",
    "goto",
    "highlights",
    "image",
    "images",
    "index",
    "index_clear",
    "index_status",
    "level_changed",
    "link",
    "links",
    "log",
    "open",
    "palette",
    "permission_request",
    "permission_show",
    "qa_drive",
    "qa_read",
    "recent",
    "recent_clear",
    "ref",
    "render",
    "resolve",
    "search",
    "select",
    "session_check",
    "session_restore",
    "session_save",
    "session_show",
    "session_sleep",
    "settings",
    "sidebar",
    "sign",
    "signatures",
    "snapshot",
    "status",
    "stop",
    "tabs",
    "tap",
    "text",
    "update_check",
    "update_dev_builds",
    "update_install",
    "update_status",
    "view",
    "wait",
    "watch",
    "watch_stop",
    "where",
    "budget_index",
    "budget_tabs",
    "help",
    "local",
    "permission_set",
    "qa_budget",
    "qa_compare_budget",
    "qa_run",
    "qa_select_budget",
    "qa_start",
    "serve",
    "session",
    "skill_install",
    "skill_show",
    "version",
];

/// Commands nothing of which can be in what a quit writes.
const READS: &[&str] = &[
    "copy",
    "contents",
    "edits",
    "export_svg",
    "facts",
    "highlights",
    "image",
    "images",
    "index",
    "index_clear",
    "index_status",
    "level_changed",
    "link",
    "links",
    "log",
    "permission_show",
    "print",
    "qa_read",
    "recent",
    "ref",
    "render",
    "resolve",
    "search",
    "select",
    "session_check",
    "session_show",
    "sign",
    "signatures",
    "snapshot",
    "status",
    "tabs",
    "text",
    "update_status",
    "wait",
    "watch",
    "watch_stop",
    "where",
    "help",
    "version",
    "skill_show",
];

/// Every other command: what it does can be in what a quit writes. Read only by the test that every command is placed.
#[cfg(test)]
const CHANGES: &[&str] = &[
    "back",
    "budget",
    "budget_render",
    "budget_viewport",
    "clear_history",
    "close",
    "compare",
    "contents_fold",
    "contents_show",
    "contents_stop",
    "discard",
    "edit_mode",
    "find",
    "follow",
    "forward",
    "goto",
    "highlight",
    "highlight_remove",
    "open",
    "page_delete",
    "page_insert",
    "page_move",
    "page_rotate",
    "palette",
    "permission_request",
    "qa_drive",
    "recent_clear",
    "redo",
    "save",
    "session_restore",
    "session_save",
    "session_sleep",
    "settings",
    "sidebar",
    "stop",
    "tap",
    "undo",
    "update_check",
    "update_dev_builds",
    "update_install",
    "view",
    "budget_index",
    "budget_tabs",
    "local",
    "permission_set",
    "qa_budget",
    "qa_compare_budget",
    "qa_run",
    "qa_select_budget",
    "qa_start",
    "serve",
    "session",
    "skill_install",
];

/// The edits that land later: run on a thread of their own, the session publishes what they did only once they land, so a close asks and Undo and Save
/// are on meanwhile (review of the lane, D1 and D3).
const LANDS_LATER: &[&str] = &["highlight", "highlight_remove", "page_insert"];

/// Every other command: done, or refused, by the time the session thread publishes. Read only by the test that every command is placed.
#[cfg(test)]
const LANDS_NOW: &[&str] = &[
    "back",
    "budget",
    "budget_index",
    "budget_render",
    "budget_tabs",
    "budget_viewport",
    "clear_history",
    "close",
    "compare",
    "contents",
    "contents_fold",
    "contents_show",
    "contents_stop",
    "copy",
    "discard",
    "edit_mode",
    "edits",
    "export_svg",
    "facts",
    "find",
    "follow",
    "forward",
    "goto",
    "help",
    "highlights",
    "image",
    "images",
    "index",
    "index_clear",
    "index_status",
    "level_changed",
    "link",
    "links",
    "local",
    "log",
    "open",
    "page_delete",
    "page_move",
    "page_rotate",
    "palette",
    "permission_request",
    "permission_set",
    "permission_show",
    "print",
    "qa_budget",
    "qa_compare_budget",
    "qa_drive",
    "qa_read",
    "qa_run",
    "qa_select_budget",
    "qa_start",
    "recent",
    "recent_clear",
    "redo",
    "ref",
    "render",
    "resolve",
    "save",
    "search",
    "select",
    "serve",
    "session",
    "session_check",
    "session_restore",
    "session_save",
    "session_show",
    "session_sleep",
    "settings",
    "sidebar",
    "sign",
    "signatures",
    "skill_install",
    "skill_show",
    "snapshot",
    "status",
    "stop",
    "tabs",
    "tap",
    "text",
    "undo",
    "update_check",
    "update_dev_builds",
    "update_install",
    "update_status",
    "version",
    "view",
    "wait",
    "watch",
    "watch_stop",
    "where",
];

/// **Whether a person's command keeps its place among their edits** (the module's notes). And, by its arguments, the snapshot tool's box and click to
/// the clipboard, as a print is: each holds its turn until its page is found, so a turn asked before the release is in what's copied, and one asked
/// after isn't (coordinator, 2026-11-14). A plain `snapshot` or `image` is a read, and a paste's SVG isn't ordered, as the system's paste waits for it.
#[must_use]
pub fn ordered(command: &Command) -> bool {
    match recto_agent::clipboard::kind_of(command) {
        Some(kind) => kind != recto_agent::clipboard::ClipboardKind::Paste,
        None => ORDERED.contains(&command.tag()),
    }
}

/// **Whether a command is an edit that lands later** (the list's notes): a highlight, its removal, and an inserted page (which no window path sends today).
#[must_use]
pub fn lands_later(command: &Command) -> bool {
    LANDS_LATER.contains(&command.tag())
}

/// **Whether anything a command does can be in what a quit writes** (the module's notes): a quit waits for it. One not in either list counts as changing.
#[must_use]
pub fn changes(command: &Command) -> bool {
    !READS.contains(&command.tag())
}

#[cfg(test)]
mod tests {
    use super::{CHANGES, LANDS_LATER, LANDS_NOW, NOT_ORDERED, ORDERED, READS};

    /// **Failure (review of the lane, D8): a new command falls through a default** into "not ordered" (an edit that then runs before one asked ahead of
    /// it) or "only reads" (a change a quit doesn't wait for, and loses). Every command in the table is in exactly one list of each pair, and no list names
    /// a command that isn't there.
    #[test]
    fn every_command_is_placed_once_in_each_pair() {
        let mut tags: Vec<&str> = recto_agent::table::TABLE.iter().map(|spec| spec.tag).collect();
        tags.sort_unstable();
        tags.dedup();
        for (pair, (one, other)) in [
            ("ordered", (ORDERED, NOT_ORDERED)),
            ("changes", (READS, CHANGES)),
            ("lands later", (LANDS_LATER, LANDS_NOW)),
        ] {
            let wrong: Vec<String> = tags
                .iter()
                .filter(|tag| one.contains(tag) == other.contains(tag))
                .map(|tag| format!("{tag}: in {} of the two", if one.contains(tag) { "both" } else { "neither" }))
                .chain(
                    one.iter()
                        .chain(other)
                        .filter(|tag| !tags.contains(tag))
                        .map(|tag| format!("{tag}: no such command")),
                )
                .collect();
            assert!(wrong.is_empty(), "{pair}: {wrong:#?}");
        }
    }
}
