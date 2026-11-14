//! **Every call the bridge exports on `AppCore`, and what it is** (S2, the independent review of phase R, 2026-11-14): a view, the host telling Rust a fact,
//! the person's action (which reaches the one executor only through [`crate::core::controls::UiAction`] and `act`), or the host's alone, with why. The table
//! is held against the bridge's own sources, read here: an export without a line, or a line without an export, fails; so does an export whose body reaches
//! the person's commands any other way than the `UiAction`s its line names.
//!
//! What this can't see, said: the check reads the bridge's text, so a call into `recto-app` that sends a person's command from inside it is held by its
//! line naming it ([`KNOWN`]), not found by reading. That list only shrinks.

use std::collections::{BTreeMap, BTreeSet};

/// What an export is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Is {
    /// Reads what's published or a rule's answer, and changes nothing a person did.
    View,
    /// The host telling Rust a fact: a size, a scroll, the pointer, a window's life, the system's state, a field's text before it's acted on.
    Input,
    /// **The one way a person's action comes in,** `act`, which every `UiAction` goes through to the executor as the person.
    TheWayIn,
    /// The person's or the system's answer to something only the host shows or does, with no command, and why there's none.
    HostOnly(&'static str),
}

use Is::{HostOnly, Input, TheWayIn, View};

/// Why an answer to the password sheet is the host's alone.
const PASSWORD: &str = "a password the person typed is theirs alone: no command carries one, so no agent can send or read it";
/// Why the hand-off's calls are the host's alone.
const HAND_OFF: &str = "the updater's hand-off, the host's and the installer's: an agent's update is `update install`, which asks for it";
/// Why the clipboard's calls are the host's alone.
const CLIPBOARD: &str = "the system's clipboard asking for what a copy offered: an agent's copy is `copy` or `snapshot`, which answers with it";
/// Why the system's printing's calls are the host's alone.
const PRINTING: &str = "the system's printing taking the file the person's `print` made, and giving it back: no agent prints on the person's printer";
/// Why QA's answers are the host's alone.
const QA: &str = "the host's answer to a QA request an agent's `qa` command sent: it carries what the window did, not an action";

/// **The table:** every `pub fn` in the bridge's `#[uniffi::export] impl AppCore` blocks, with what it is and the `UiAction`s it sends through `act`, by
/// kind, if any.
const EXPORTS: &[(&str, Is, &[&str])] = &[
    // The one way in.
    ("act", TheWayIn, &[]),
    // Views.
    ("is_qa", View, &[]),
    ("strings", View, &[]),
    ("limits", View, &[]),
    ("sidebar", View, &[]),
    ("sidebar_tabs", View, &[]),
    ("contents_rows", View, &[]),
    ("scroll_thumb", View, &[]),
    ("sidebar_layout", View, &[]),
    ("find_view", View, &[]),
    ("find_rows", View, &[]),
    ("find_report", View, &[]),
    ("contents_is_following", View, &[]),
    ("contents_is_moving", View, &[]),
    ("reader", View, &[]),
    ("pane_count", View, &[]),
    ("row_reveal", View, &[]),
    ("menu_bar", View, &[]),
    ("recent_files", View, &[]),
    ("panel", View, &[]),
    ("palette_view", View, &[]),
    ("menu_item_enabled", View, &[]),
    ("save_panel", View, &[]),
    ("close_prompt", View, &[]),
    ("context_menu", View, &[]),
    ("pointer_look", View, &[]),
    ("page_field", View, &[]),
    ("pointer_is_over_text", View, &[]),
    ("autoscroll_step", View, &[]),
    ("selection_text_now", View, &[]),
    ("selection_can_be_sent", View, &[]),
    ("document_point", View, &[]),
    ("reading_pane", View, &[]),
    ("plain_key_action", View, &[]),
    ("zoom_label", View, &[]),
    ("zoom_after_scroll", View, &[]),
    ("app_comes_back_hidden", View, &[]),
    ("window_should_open", View, &[]),
    ("state_read", View, &[]),
    ("window_interactive", View, &[]),
    ("may_remake_window", View, &[]),
    ("ready_to_show", View, &[]),
    ("window_placement", View, &[]),
    ("focus_view", View, &[]),
    ("appearance_choice", View, &[]),
    ("seconds_since_input", View, &[]),
    ("hand_off_check", View, &[]),
    ("update_timer", View, &[]),
    ("settings_model", View, &[]),
    ("settings_window_was_open", View, &[]),
    ("settings_window_place", View, &[]),
    ("opens_in_background", View, &[]),
    ("print_model", View, &[]),
    ("chrome_look", View, &[]),
    ("qa_policy_nudge_ms", View, &[]),
    ("qa_policy_nudge_in_put_back", View, &[]),
    ("thumb_access", View, &[]),
    ("qa_accessibility_step", View, &[]),
    ("qa_accessibility_refusal", View, &[]),
    ("thumb_contrast", View, &[]),
    ("fold_action_name", View, &[]),
    // The host telling Rust a fact. Those that move the pane being read send it as the person's `compare --reading`, through `act`.
    ("start", Input, &[]),
    ("ready", Input, &[]),
    ("contents_user_scrolled", Input, &[]),
    ("contents_pointer", Input, &[]),
    ("contents_reached", Input, &[]),
    ("contents_step", Input, &[]),
    ("contents_view_changed", Input, &[]),
    ("contents_row_change", Input, &[]),
    ("contents_shifted", Input, &[]),
    ("pane_focused", Input, &["PaneRead"]),
    ("set_viewport", Input, &["PaneRead"]),
    ("drain_ops", Input, &[]),
    ("ops_applied", Input, &[]),
    ("set_dark_appearance", Input, &[]),
    ("palette_typed", Input, &[]),
    ("palette_step", Input, &[]),
    ("palette_close", Input, &[]),
    ("memory_pressure", Input, &[]),
    ("selection_press", Input, &["PaneRead"]),
    ("selection_drag", Input, &[]),
    ("selection_release", Input, &[]),
    // The person's own notices, which no agent sees: their time is up, or the person dismissed them.
    ("dismiss_selection_notice", Input, &[]),
    ("update_alert_answered", Input, &[]),
    // Only `qa appearance` calls it, after the host has set the app's appearance.
    ("set_appearance_choice", Input, &[]),
    ("window_closed", Input, &[]),
    ("window_replaced", Input, &[]),
    ("window_minimised_changed", Input, &[]),
    ("app_hidden_changed", Input, &[]),
    ("window_opened", Input, &[]),
    ("shown_with_state", Input, &[]),
    ("no_window_to_show", Input, &[]),
    ("main_busy", Input, &[]),
    ("main_idle", Input, &[]),
    ("begin_restore", Input, &[]),
    ("restore_patience_over", Input, &[]),
    ("window_shown", Input, &[]),
    ("window_changed", Input, &[]),
    ("set_sidebar_width", Input, &[]),
    ("contents_position", Input, &[]),
    ("focus_changed", Input, &[]),
    ("frontmost_changed", Input, &[]),
    ("sheet_changed", Input, &[]),
    ("note_user_input", Input, &[]),
    ("note_user_nearby", Input, &[]),
    ("set_update_facts", Input, &[]),
    ("update_timer_fired", Input, &[]),
    ("settings_window_changed", Input, &[]),
    ("chrome_changed", Input, &[]),
    ("chrome_pointer", Input, &[]),
    ("chrome_held", Input, &[]),
    ("shutdown", Input, &[]),
    // A contents row's fold, as VoiceOver and the keys reach it: its accessibility step, then the chevron's `contents fold`.
    ("fold_action", Input, &["Chevron"]),
    ("contents_fold_key", Input, &["Chevron"]),
    // The host's alone.
    ("password_answered", HostOnly(PASSWORD), &[]),
    (
        "link_prompt_answered",
        HostOnly("a web or email address from a document, which is hostile, opens only on the person's own answer to the sheet, and the system opens it"),
        &[],
    ),
    (
        "save_problem_answered",
        HostOnly("a sheet's answer that only closes it or opens Save As's, whose answer is `UiAction::SaveAs`"),
        &[],
    ),
    (
        "quit_requested",
        HostOnly("quitting is the person's and the system's: an agent's `stop` is refused in the app (`only_the_user_quits`)"),
        &[],
    ),
    (
        "quit_stayed",
        HostOnly("the quit's question answered with Stay: the person's answer to the host's own alert"),
        &[],
    ),
    ("hand_off", HostOnly(HAND_OFF), &[]),
    ("hand_off_quit_now", HostOnly(HAND_OFF), &[]),
    ("hand_off_cancelled", HostOnly(HAND_OFF), &[]),
    (
        "take_update_alert",
        HostOnly("an update's word, as an alert when there's no window: the person's own, which no agent sees"),
        &[],
    ),
    ("take_clipboard", HostOnly(CLIPBOARD), &[]),
    ("clipboard_data", HostOnly(CLIPBOARD), &[]),
    ("clipboard_released", HostOnly(CLIPBOARD), &[]),
    ("take_print", HostOnly(PRINTING), &[]),
    ("print_finished", HostOnly(PRINTING), &[]),
    (
        "print_qa_target",
        HostOnly("where QA's print through the system's printing writes: the print itself is `UiAction::Print`"),
        &[],
    ),
    ("qa_answer", HostOnly(QA), &[]),
    ("qa_note_step", HostOnly(QA), &[]),
];

/// **What reaches the person's commands from inside `recto-app`, where reading the bridge can't see it,** with where it's to be fixed. Only shrinks: a new
/// one fails [`every_export_is_in_the_table_and_reaches_the_person_only_through_act`] (2 on 2026-11-14).
const KNOWN: &[(&str, &str)] = &[
    (
        "selection_release",
        "a link clicked sends `goto` or `back` as the person through `person::submit` in `reconcile::follow_link`, not `act`; and with the snapshot tool on, \
         the release takes a snapshot through `extract` in `snapshot_tool::start`, outside the executor (PLAN, S2's follow-ups)",
    ),
    (
        "clipboard_data",
        "a promised SVG of the snapshot tool's is made through `extract::snapshot_svg` as a paste asks for it, outside the executor (PLAN, S2's follow-ups)",
    ),
];

/// What may not appear in an export's body: the ways to a person's command other than `act`.
const FORBIDDEN: &[&str] = &["person::", "submit(", "execute(", ".run(Actor", "run_counted", "printing::start"];

/// One method of the bridge's `impl AppCore` blocks: its name, its text, and whether it's exported.
#[derive(Debug)]
struct Method {
    body: String,
    exported: bool,
}

/// **The bridge's `AppCore` methods, read from its sources:** every `fn` at the top of an `impl AppCore` block, exported when it's `pub` in a block that
/// is `#[uniffi::export]`, with its text to its closing brace.
fn bridge_methods(sources: &[(String, String)]) -> BTreeMap<String, Method> {
    let mut methods = BTreeMap::new();
    for (file, text) in sources {
        let lines: Vec<&str> = text.lines().collect();
        let mut at = 0;
        while at < lines.len() {
            if lines[at].trim() != "impl AppCore {" {
                at += 1;
                continue;
            }
            let exported = lines[..at]
                .iter()
                .rev()
                .find(|line| !line.trim().is_empty())
                .is_some_and(|line| line.trim().starts_with("#[uniffi::export"));
            let mut depth = 1i32;
            at += 1;
            let mut current: Option<(String, String, bool)> = None;
            while at < lines.len() && depth > 0 {
                let line = lines[at];
                let trimmed = line.trim_start();
                if depth == 1 && current.is_none() {
                    let public = trimmed.strip_prefix("pub fn ");
                    if let Some(name) = public.or_else(|| trimmed.strip_prefix("fn ")) {
                        let name: String = name.chars().take_while(|c| c.is_alphanumeric() || *c == '_').collect();
                        current = Some((name, String::new(), public.is_some()));
                    }
                }
                depth += line.matches('{').count() as i32 - line.matches('}').count() as i32;
                if let Some((_, body, _)) = current.as_mut() {
                    body.push_str(line);
                    body.push('\n');
                }
                if depth == 1
                    && let Some((name, body, public)) = current.take()
                {
                    let duplicate = methods.insert(
                        name.clone(),
                        Method {
                            body,
                            exported: exported && public,
                        },
                    );
                    assert!(duplicate.is_none(), "{file}: `{name}` read twice");
                }
                at += 1;
            }
        }
    }
    methods
}

/// The bridge's sources, by path.
fn bridge_sources() -> Vec<(String, String)> {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../recto-ffi/src");
    let mut files = vec![root];
    let mut sources = Vec::new();
    while let Some(path) = files.pop() {
        if path.is_dir() {
            files.extend(
                std::fs::read_dir(&path)
                    .expect("the bridge's sources")
                    .map(|entry| entry.expect("an entry").path()),
            );
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            sources.push((path.display().to_string(), std::fs::read_to_string(&path).expect("a source")));
        }
    }
    sources
}

/// What's wrong with the bridge against the table, a line each: an export with no line or a line with no export; a body that reaches a person's command
/// other than through `act`, or sends a `UiAction` its line doesn't name, or names one it doesn't send; a view that sends one; and a kind no control walks.
fn wrong(methods: &BTreeMap<String, Method>, walked: &BTreeSet<String>) -> Vec<String> {
    let mut wrong = Vec::new();
    let table: BTreeMap<&str, (Is, &[&str])> = EXPORTS.iter().map(|(name, is, acts)| (*name, (*is, *acts))).collect();
    assert_eq!(table.len(), EXPORTS.len(), "a name twice in the table");
    for (name, method) in methods.iter().filter(|(_, method)| method.exported) {
        let Some((is, acts)) = table.get(name.as_str()) else {
            wrong.push(format!("`{name}` is exported and the table doesn't say what it is"));
            continue;
        };
        // What it calls of its own, read with it, once: a private helper's or another export's body.
        let mut text = method.body.clone();
        for (other, called) in methods {
            if other != name && method.body.contains(&format!("self.{other}(")) {
                text.push_str(&called.body);
            }
        }
        for forbidden in FORBIDDEN {
            if text.contains(forbidden) {
                wrong.push(format!("`{name}` reaches a person's command other than through `act`: `{forbidden}`"));
            }
        }
        for (at, _) in text.match_indices("CoreCall::") {
            let call: String = text[at + "CoreCall::".len()..].chars().take_while(|c| c.is_alphanumeric()).collect();
            if call != "Reconcile" {
                wrong.push(format!("`{name}` queues `CoreCall::{call}` on the session thread itself"));
            }
        }
        let sent: BTreeSet<String> = text
            .match_indices("UiAction::")
            .map(|(at, found)| text[at + found.len()..].chars().take_while(|c| c.is_alphanumeric()).collect())
            .collect();
        let said: BTreeSet<String> = acts.iter().map(ToString::to_string).collect();
        if sent != said {
            wrong.push(format!("`{name}` sends {sent:?} through `act`, and its line says {said:?}"));
        }
        if !said.is_empty() && !matches!(is, Input) {
            wrong.push(format!("`{name}` is said to be {is:?} and sends {said:?}"));
        }
        if matches!(is, View | HostOnly(_)) && text.contains(".act(") {
            wrong.push(format!("`{name}` is said to be {is:?} and acts"));
        }
        if let HostOnly(why) = is
            && why.is_empty()
        {
            wrong.push(format!("`{name}` needs its reason"));
        }
        for kind in &said {
            if !walked.contains(&format!("UiAction::{kind}")) {
                wrong.push(format!("`{name}` sends `UiAction::{kind}`, which no control's walk reaches the executor with"));
            }
        }
    }
    for name in table.keys() {
        if !methods.get(*name).is_some_and(|method| method.exported) {
            wrong.push(format!("`{name}` is in the table and the bridge doesn't export it"));
        }
    }
    for (name, _) in KNOWN {
        if !table.contains_key(name) {
            wrong.push(format!("`{name}` is a known exception and no export"));
        }
    }
    wrong
}

/// The kinds of `UiAction` the coverage walk sends to the executor, by name.
fn walked() -> BTreeSet<String> {
    let app = super::coverage::app();
    super::coverage::every_kind(&app)
        .into_iter()
        .map(|(name, _)| name.split('(').next().unwrap_or_default().to_string())
        .collect()
}

/// Failure (S2, the independent review of phase R, 2026-11-14): the bridge has a way for a person's action into the core beside `act`, so it runs
/// outside the one executor (the person's print did: it decided its pages in the bridge and called the printing functions itself, with no check of a
/// quiet run's files and no hold on its tab), and the coverage test, which walks the `UiAction`s, can't see it; or an export is added that nobody has
/// said is a view, a fact from the host, the way in, or the host's alone and why. Every export of the bridge, read from its sources, against the table:
/// each has a line, each line an export, and only `act`, and what its line names, reaches a person's command.
#[test]
fn every_export_is_in_the_table_and_reaches_the_person_only_through_act() {
    let methods = bridge_methods(&bridge_sources());
    let exported = methods.values().filter(|method| method.exported).count();
    assert!(exported > 100, "the bridge's exports were read: {exported}");
    let wrong = wrong(&methods, &walked());
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
    assert!(KNOWN.len() <= 2, "a new known exception: fix it instead");
}

/// Failure: the reading of the bridge misses what it's for: an export added with no line, one that reaches the core's person's commands directly, as the
/// bridge's print did, or one that queues a person's call on the session thread itself (what `the_bridge_queues_none_of_a_persons_calls` held before this
/// replaced it), passes. Each seeded in the bridge's text as it is now: each is found.
#[test]
fn a_seeded_export_is_found() {
    let mut sources = bridge_sources();
    let (_, printing) = sources
        .iter_mut()
        .find(|(file, _)| file.ends_with("printing.rs"))
        .expect("the bridge's printing");
    let at = printing.find("    pub fn take_print").expect("an export to put one beside");
    printing.insert_str(
        at,
        "    pub fn print_now(&self) {\n        recto_app::core::person::submit(&self.inner, todo!());\n    }\n\n    pub fn reader_twice(&self) {\n        \
         self.inner.window.notices();\n    }\n\n",
    );
    let methods = bridge_methods(&sources);
    let said = wrong(&methods, &walked());
    assert!(said.iter().any(|line| line.contains("`print_now` is exported and the table")), "{said:#?}");
    assert!(said.iter().any(|line| line.contains("`reader_twice` is exported and the table")), "{said:#?}");
    // Classified, the one calling into the core directly is still found.
    let reached = methods
        .get("print_now")
        .map(|method| FORBIDDEN.iter().any(|forbidden| method.body.contains(forbidden)));
    assert_eq!(reached, Some(true), "a direct call to a person's command is seen in its body");
    let mut sources = bridge_sources();
    let (_, printing) = sources
        .iter_mut()
        .find(|(file, _)| file.ends_with("printing.rs"))
        .expect("the bridge's printing");
    *printing = printing.replace(
        "        self.inner.tools.printing().finished(&path);",
        "        self.inner.tools.printing().finished(&path);\n        recto_app::core::person::submit(&self.inner, todo!());\n        \
         self.inner.core.push(CoreCall::Find(todo!()));",
    );
    let said = wrong(&bridge_methods(&sources), &walked());
    assert!(
        said.iter().any(|line| line.contains("`print_finished` reaches a person's command")),
        "{said:#?}"
    );
    // What the text search this replaced held: a person's call queued on the session thread by the bridge itself.
    assert!(said.iter().any(|line| line.contains("`print_finished` queues `CoreCall::Find`")), "{said:#?}");
}
