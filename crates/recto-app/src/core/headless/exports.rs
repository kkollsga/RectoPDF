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

/// **The table:** everything the bridge exports, as uniffi exports it: every fn of an `#[uniffi::export] impl AppCore` block whatever its visibility, by
/// its name; an exported free function as `fn name`; an exported impl of another type as `Type::name`; an exported trait as `trait Name`. With what it
/// is, and the `UiAction`s it sends through `act`, by kind, if any.
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
    // Not `pub`, and exported all the same, as every fn of an exported impl is.
    ("can_save", View, &[]),
    // Free functions, asked before there's an app.
    ("fn launch_is_quiet", View, &[]),
    ("fn launch_answers_at_once", View, &[]),
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
    ("selection_release", Input, &["LinkClicked", "SnapshotTaken"]),
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
    // Every advertised type is prepared by the copy; the system's callback only takes those bytes.
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
    (
        "trait AppListener",
        HostOnly("the host's listener, which Rust calls to say something changed: the host implements it, and nothing it does reaches a command"),
        &[],
    ),
];

/// **What reaches the person's commands from inside `recto-app`, where reading the bridge can't see it,** with where it's to be fixed. Only shrinks: a new
/// one fails [`every_export_is_in_the_table_and_reaches_the_person_only_through_act`] (2 on 2026-11-14, none since S2's follow-ups (1) and (2)).
const KNOWN: &[(&str, &str)] = &[];

/// What may not appear in an export's body: the ways to a person's command other than `act`.
const FORBIDDEN: &[&str] = &["person::", "submit(", "execute(", ".run(Actor", "run_counted", "printing::start"];

/// One fn of the bridge's: its text, and whether uniffi exports it.
#[derive(Debug)]
struct Method {
    body: String,
    exported: bool,
}

/// Whether the lines before `at` that are attributes (and their doc comments) hold `#[uniffi::export`.
fn exported_at(lines: &[&str], at: usize) -> bool {
    lines[..at]
        .iter()
        .rev()
        .map(|line| line.trim())
        .take_while(|line| line.starts_with("#[") || line.starts_with("///") || line.starts_with("//!"))
        .any(|line| line.starts_with("#[uniffi::export"))
}

/// How deep a line leaves the braces, from `depth` before it.
fn depth_after(line: &str, depth: i32) -> i32 {
    depth + line.matches('{').count() as i32 - line.matches('}').count() as i32
}

/// **What the bridge exports, read from its sources by uniffi's rules:** every fn at the top of an `impl` block that's `#[uniffi::export]`, whatever its
/// visibility (named by itself for `AppCore`, `Type::name` for another type), an exported free fn (`fn name`), and an exported trait (`trait Name`). The
/// fns of `AppCore`'s other `impl` blocks are kept too, not exported, for what an export calls of its own.
fn bridge_methods(sources: &[(String, String)]) -> BTreeMap<String, Method> {
    let mut methods = BTreeMap::new();
    let mut insert = |file: &str, name: String, body: String, exported: bool| {
        let duplicate = methods.insert(name.clone(), Method { body, exported });
        assert!(duplicate.is_none(), "{file}: `{name}` read twice");
    };
    for (file, text) in sources {
        let lines: Vec<&str> = text.lines().collect();
        let mut at = 0;
        while at < lines.len() {
            let line = lines[at];
            let exported = exported_at(&lines, at);
            if let Some(name) = line.strip_prefix("pub trait ").or_else(|| line.strip_prefix("trait ")) {
                let name: String = name.chars().take_while(|c| c.is_alphanumeric() || *c == '_').collect();
                if exported {
                    insert(file, format!("trait {name}"), String::new(), true);
                }
                at += 1;
                continue;
            }
            if let Some(name) = line.strip_prefix("pub fn ").or_else(|| line.strip_prefix("fn ")) {
                let name: String = name.chars().take_while(|c| c.is_alphanumeric() || *c == '_').collect();
                let mut depth = depth_after(line, 0);
                let mut body = format!("{line}\n");
                while depth > 0 && at + 1 < lines.len() {
                    at += 1;
                    depth = depth_after(lines[at], depth);
                    body.push_str(lines[at]);
                    body.push('\n');
                }
                if exported {
                    insert(file, format!("fn {name}"), body, true);
                }
                at += 1;
                continue;
            }
            let Some(owner) = line
                .strip_prefix("impl ")
                .filter(|rest| rest.trim_end().ends_with('{') && !rest.contains(" for "))
            else {
                at += 1;
                continue;
            };
            let owner: String = owner.chars().take_while(|c| c.is_alphanumeric() || *c == '_').collect();
            let mut depth = 1;
            at += 1;
            let mut current: Option<(String, String)> = None;
            while at < lines.len() && depth > 0 {
                let line = lines[at];
                if depth == 1 && current.is_none() {
                    let trimmed = line.trim_start();
                    let named = ["pub fn ", "pub(crate) fn ", "pub(super) fn ", "fn ", "pub async fn ", "async fn "]
                        .iter()
                        .find_map(|head| trimmed.strip_prefix(head));
                    if let Some(name) = named {
                        let name: String = name.chars().take_while(|c| c.is_alphanumeric() || *c == '_').collect();
                        current = Some((name, String::new()));
                    }
                }
                depth = depth_after(line, depth);
                if let Some((_, body)) = current.as_mut() {
                    body.push_str(line);
                    body.push('\n');
                }
                if depth == 1
                    && let Some((name, body)) = current.take()
                {
                    let name = if owner == "AppCore" { name } else { format!("{owner}::{name}") };
                    if exported || owner == "AppCore" {
                        insert(file, name, body, exported);
                    }
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
    assert!(KNOWN.is_empty(), "a new known exception: fix it instead");
}

/// Failure (R2.3, C6; kept beside the table, review of S2, S3): the bridge reaches past the one way a person's action comes in, queuing a person's call on
/// the session thread itself, anywhere in its text, an export or not and however deep: the table's check reads an export's body and what it calls of its
/// own one level down, and this reads everything. The bridge's sources name no `CoreCall` but `Reconcile`, which asks the window to be drawn again and does
/// nothing a person does.
#[test]
fn the_bridge_queues_none_of_a_persons_calls() {
    let named = persons_calls_queued_by(&bridge_sources());
    assert!(named.is_empty(), "{named:#?}");
}

/// Every `CoreCall` but `Reconcile` named anywhere in these bridge sources.
fn persons_calls_queued_by(sources: &[(String, String)]) -> Vec<String> {
    let mut named = Vec::new();
    for (file, text) in sources {
        for (at, _) in text.match_indices("CoreCall::") {
            let call: String = text[at + "CoreCall::".len()..].chars().take_while(|c| c.is_alphanumeric()).collect();
            if call != "Reconcile" {
                named.push(format!("{file}: CoreCall::{call}"));
            }
        }
    }
    named
}

/// Failure (the weak-test review, proven by seeding): the table's check reads an export's body and what it calls of its own one level down, so a person's
/// call queued two private functions deep from an export passes it, and only the search of the whole text sees it. Seeded as it was proven: the table's
/// check says nothing, and the search names the call, so neither is a repeat of the other.
#[test]
fn a_call_queued_two_functions_deep_is_seen_by_the_search_and_not_the_table() {
    let mut sources = bridge_sources();
    let (_, printing) = sources
        .iter_mut()
        .find(|(file, _)| file.ends_with("printing.rs"))
        .expect("the bridge's printing");
    *printing = printing.replace(
        "        self.inner.tools.printing().finished(&path);",
        "        self.inner.tools.printing().finished(&path);\n        seed_a(&self.inner);",
    );
    printing.push_str(
        "\nfn seed_a(inner: &Inner) {\n    seed_b(inner);\n}\n\nfn seed_b(inner: &Inner) {\n    inner.core.push(CoreCall::Find(String::new()));\n}\n",
    );
    assert_eq!(
        persons_calls_queued_by(&sources).len(),
        1,
        "the search names the call two functions deep: {:#?}",
        persons_calls_queued_by(&sources)
    );
    let said = wrong(&bridge_methods(&sources), &walked());
    assert!(
        said.is_empty(),
        "the table's check reads one level down only; if it follows further now, this test and the search can go: {said:#?}"
    );
}

/// **What uniffi really exports, by the bridge's static library,** where one is built from these sources: its `uniffi_recto_ffi_fn_` symbols, the
/// `AppCore` methods and constructors and the free functions. None if there's no library, `nm` can't read it, or it's older than a source.
fn exported_by_the_library(sources: &[(String, String)]) -> Option<BTreeSet<String>> {
    let target = std::env::var_os("CARGO_TARGET_DIR").map_or_else(
        || std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target"),
        std::path::PathBuf::from,
    );
    let library = target.join("debug/librecto_ffi.a");
    let built = std::fs::metadata(&library).and_then(|meta| meta.modified()).ok()?;
    let newest = sources
        .iter()
        .filter_map(|(file, _)| std::fs::metadata(file).and_then(|meta| meta.modified()).ok())
        .max()?;
    if built < newest {
        return None;
    }
    let listed = std::process::Command::new("nm").args(["-gU"]).arg(&library).output().ok()?;
    let text = String::from_utf8_lossy(&listed.stdout).into_owned();
    let names: BTreeSet<String> = text
        .lines()
        .filter_map(|line| line.split_whitespace().last()?.strip_prefix("_uniffi_recto_ffi_fn_").map(str::to_string))
        .filter_map(|symbol| {
            if let Some(name) = symbol.strip_prefix("method_appcore_").or_else(|| symbol.strip_prefix("constructor_appcore_")) {
                Some(name.to_string())
            } else {
                symbol.strip_prefix("func_").map(|name| format!("fn {name}"))
            }
        })
        .collect();
    (!names.is_empty()).then_some(names)
}

/// Failure (review of S2, S2): the reading of the bridge's sources isn't what uniffi exports (a rule of uniffi's it doesn't follow, as `fn can_save`,
/// exported though not `pub`, went unread), so an export can be outside the table with the test passing. Held against the static library built from these
/// sources, where there is one: the app's build makes it, and a run without it says so.
#[test]
fn the_reading_of_the_bridge_is_what_uniffi_exports() {
    let sources = bridge_sources();
    let Some(real) = exported_by_the_library(&sources) else {
        eprintln!("no static library built from these sources: the reading wasn't held against what uniffi exports this run");
        return;
    };
    let read: BTreeSet<String> = bridge_methods(&sources)
        .into_iter()
        .filter(|(name, method)| method.exported && !name.starts_with("trait "))
        .map(|(name, _)| name)
        .collect();
    assert_eq!(read, real, "what the bridge's sources are read as exporting, against what its library exports");
}

/// Failure: the reading of the bridge misses what it's for: an export added with no line, one that reaches the core's person's commands directly, as the
/// bridge's print did, or one that queues a person's call on the session thread itself (`the_bridge_queues_none_of_a_persons_calls`, kept: it sees a call
/// queued deeper than the table reads), passes. Each seeded in the bridge's text as it is now: each is found.
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
        "    pub fn print_now(&self) {\n        recto_app::core::person::submit(&self.inner, todo!());\n    }\n\n    fn reader_twice(&self) {\n        \
         self.inner.window.notices();\n    }\n\n",
    );
    printing.push_str("\n#[uniffi::export]\npub fn print_freely() {\n}\n");
    let methods = bridge_methods(&sources);
    let said = wrong(&methods, &walked());
    assert!(said.iter().any(|line| line.contains("`print_now` is exported and the table")), "{said:#?}");
    assert!(
        said.iter().any(|line| line.contains("`reader_twice` is exported and the table")),
        "not `pub`, and exported: {said:#?}"
    );
    assert!(
        said.iter().any(|line| line.contains("`fn print_freely` is exported and the table")),
        "a free function: {said:#?}"
    );
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
    let anchor = "        recto_app::core::printing::completion::request(";
    assert_eq!(
        printing.matches(anchor).count(),
        1,
        "the seed must reach the actual completion forwarder exactly once"
    );
    *printing = printing.replacen(
        anchor,
        &format!("        recto_app::core::person::submit(&self.inner, todo!());\n        self.inner.core.push(CoreCall::Find(todo!()));\n{anchor}"),
        1,
    );
    let said = wrong(&bridge_methods(&sources), &walked());
    assert!(
        said.iter().any(|line| line.contains("`print_finished` reaches a person's command")),
        "{said:#?}"
    );
    // What the text search this replaced held: a person's call queued on the session thread by the bridge itself.
    assert!(said.iter().any(|line| line.contains("`print_finished` queues `CoreCall::Find`")), "{said:#?}");
}
