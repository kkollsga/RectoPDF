//! What a caller outside the crate can do with each of the session's components.

/// The methods declared `pub fn` at the top level of a component's file, outside its test module, in the order written.
fn public_methods(source: &str) -> Vec<&str> {
    let code = source.split("#[cfg(test)]\nmod ").next().unwrap_or(source);
    code.lines()
        .filter_map(|line| line.strip_prefix("    pub fn "))
        .map(|rest| rest.split(['(', '<']).next().unwrap_or(rest))
        .collect()
}

/// **Failure: a method that has to change two components together becomes public on one of them,** so a caller outside the crate holding
/// `session.lists_mut()` or `session.tabs_mut()` could leave them disagreeing: a list for a tab that's gone, a tab with no folder. Each component's
/// accessor goes out because what's public on it only reads it, or changes it alone (R3.5). A method added here is a choice a reviewer sees in this
/// list: add it only if it can't put this component out of step with another.
#[test]
fn what_a_component_lets_a_caller_outside_the_crate_do_is_listed() {
    let components = [
        (
            "Tabs",
            include_str!("../registry.rs"),
            &[
                "tab",
                "active",
                "path_of",
                "for_path",
                "tab_notice",
                "needs_password",
                "withheld_from_agents",
                "own_stamp",
                "spread_of",
                "trail_of",
                "can_go_back_and_forward",
                "place_to_show",
                "set_view_rotation",
                "set_spread",
                "set_selection",
                "note_password_used",
                "touch_at",
            ][..],
        ),
        (
            "Lists",
            include_str!("../lists.rs"),
            &[
                "get",
                "is_edited",
                "needs_rewrite",
                "has_undone_owner_steps",
                "undo_and_redo",
                "holds_owner_edits",
                "summary",
                "is_asleep",
                "set_agent_at_full",
                "own_pages_needed",
                "list_mark",
                "edits_record",
            ][..],
        ),
        (
            "Viewing",
            include_str!("../viewing.rs"),
            &["view", "set_view", "find", "set_find", "set_find_current", "edit_mode", "set_edit_mode"][..],
        ),
        (
            "PageViewMail",
            include_str!("../mail.rs"),
            &["note_drawn", "draw_note", "waiting_jump", "take_jump", "jump_sent", "take_place_lost"][..],
        ),
        (
            "Journals",
            include_str!("../journals.rs"),
            &["set_journaling", "history", "set_history", "name_of", "has_pending"][..],
        ),
        ("Saves", include_str!("../saves.rs"), &["is_saving", "any"][..]),
        (
            "Dormancy",
            include_str!("../dormancy.rs"),
            &["seconds_now", "hold", "set_sleep_files", "take_woken_from_file", "take_sleep_read"][..],
        ),
        ("Warm", include_str!("../warm.rs"), &["ids", "toc", "holds_read_of"][..]),
        ("Generated", include_str!("../generated.rs"), &["view", "forget", "making"][..]),
        ("Recent", include_str!("../recent.rs"), &["version", "clear"][..]),
        (
            "Pages",
            include_str!("../pages.rs"),
            &["label", "label_differs", "place_words", "for_typed", "position_in_same_file", "count"][..],
        ),
        ("Dormant", include_str!("../dormant.rs"), &["next_at", "due", "oldest_first", "may_go"][..]),
    ];
    for (name, source, listed) in components {
        assert_eq!(public_methods(source), listed, "{name}'s public methods");
    }
}
