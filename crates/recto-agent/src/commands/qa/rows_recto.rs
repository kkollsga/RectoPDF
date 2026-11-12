//! The QA verbs' rows, part of [`super::ROWS_QA`]: `recto`'s own QA verbs: starting a QA run, running a script, and the budgets it measures.

use crate::Level;
use crate::table::*;

pub(crate) const ROWS: &[Spec] = &[
    Spec {
        name: "qa start",
        tag: "qa_start",
        group: GROUP_QA,
        access: Access::At(Level::Off),
        route: Route::Here,
        summary: "Starts the app in a quiet run on this state folder, which must be set with RECTO_STATE_DIR: off every screen, with no Dock icon and never active. The level there starts at Full, because the folder is throwaway. Blocks until the app is ready.",
        args: &[
            Arg {
                key: "app",
                flag: Some("--app"),
                kind: Kind::Path,
                required: false,
                help: "The app's executable, if it isn't the one built beside this `recto`.",
            },
            Arg {
                key: "memory_limit_mib",
                flag: Some("--memory-limit-mib"),
                kind: Kind::Positive(4096),
                required: false,
                help: "A worker's memory limit, and turns off the larger limit, to reach a lower-resolution page on purpose.",
            },
            Arg {
                key: "time_limit_ms",
                flag: Some("--time-limit"),
                kind: Kind::Duration,
                required: false,
                help: "When the run ends by itself. 10m if left out.",
            },
            Arg {
                key: "owner_pid",
                flag: Some("--owner"),
                kind: Kind::Positive(u32::MAX),
                required: false,
                help: "A process the run ends with, as a test or script that started it: RECTO_QA_OWNER_PID if left out. It also ends when its state folder or its program goes.",
            },
        ],
        data: "`{ running, pid, socket, state, owner }`.",
        examples: &[
            "recto qa start",
            "recto qa start --memory-limit-mib 64 --time-limit 2m",
            "recto qa start --owner 4242",
        ],
        client_wait_s: 30,
        ..BASE
    },
    Spec {
        name: "qa run",
        tag: "qa_run",
        group: GROUP_QA,
        access: Access::At(Level::Off),
        route: Route::Here,
        summary: "Runs a QA script against the running app, and prints PASS or FAIL for it and each line that fails. A line is a `recto` command without the word `recto`, or `expect PATH VALUE` on the last answer. A stall of 2 s fails the script.",
        args: &[
            Arg {
                key: "script",
                flag: None,
                kind: Kind::Path,
                required: true,
                help: "The script.",
            },
            Arg {
                key: "time_limit_ms",
                flag: Some("--time-limit"),
                kind: Kind::Duration,
                required: false,
                help: "When the script fails for taking too long. 2m if left out.",
            },
        ],
        data: "`{ script, passed, steps, failed: [ { line, why } ] }`.",
        examples: &["recto qa run app/qa/reader.qa"],
        client_wait_s: 30,
        ..BASE
    },
    Spec {
        name: "qa budget",
        tag: "qa_budget",
        group: GROUP_QA,
        access: Access::At(Level::Off),
        route: Route::Here,
        summary: "Measures the app against the budgets on a release build, starting and ending its own quiet runs: cold start, one document, scrolling, idle, a closed window, many tabs.",
        args: &[
            Arg {
                key: "file",
                flag: Some("--file"),
                kind: Kind::Path,
                required: true,
                help: "A text PDF to measure with.",
            },
            Arg {
                key: "heavy",
                flag: Some("--heavy"),
                kind: Kind::Path,
                required: false,
                help: "A page-heavy PDF, for first view, scroll and zoom.",
            },
            Arg {
                key: "app",
                flag: Some("--app"),
                kind: Kind::Path,
                required: false,
                help: "The app's executable, if it isn't the one built beside this `recto`.",
            },
            Arg {
                key: "runs",
                flag: Some("--runs"),
                kind: Kind::Positive(100),
                required: false,
                help: "Cold starts to measure. 11 if left out.",
            },
        ],
        data: "`{ build, cold_start_ms: { p50, p95 }, one_document: { footprint_mib }, scroll: { .. }, idle: { .. }, closed_window: { .. }, tabs: { .. }, heavy: { .. } }`.",
        examples: &["recto qa budget --file report.pdf --runs 3"],
        client_wait_s: 30,
        ..BASE
    },
    Spec {
        name: "qa compare-budget",
        tag: "qa_compare_budget",
        group: GROUP_QA,
        access: Access::At(Level::Off),
        route: Route::Here,
        summary: "Measures comparison view against the budgets, starting and ending its own quiet runs: memory at rest and after scrolling, idle wake-ups, the UI thread's step in a sweep, and starts from the record with the comparison in it. With one pane, the same files open and one showing, as before comparison view.",
        args: &[
            Arg {
                key: "files",
                flag: Some("--files"),
                kind: Kind::Text,
                required: true,
                help: "Two or three whole paths, with commas between, left to right.",
            },
            Arg {
                key: "panes",
                flag: Some("--panes"),
                kind: Kind::Positive(3),
                required: false,
                help: "How many side by side: 1 for none. As many as files if left out.",
            },
            Arg {
                key: "sync",
                flag: Some("--sync"),
                kind: Kind::Choice(&["on", "off"]),
                required: false,
                help: "Synced scroll while sweeping.",
            },
            Arg {
                key: "app",
                flag: Some("--app"),
                kind: Kind::Path,
                required: false,
                help: "The app's executable, if it isn't the one built beside this `recto`.",
            },
            Arg {
                key: "runs",
                flag: Some("--runs"),
                kind: Kind::Positive(100),
                required: false,
                help: "Starts to measure. 5 if left out.",
            },
        ],
        data: "`{ build, panes, synced, at_rest: { tree_mib, app_mib, worker_mib, workers, threads }, idle, scroll: { ui_step_us }, after_scroll, cold_start_ms }`.",
        examples: &["recto qa compare-budget --files /tmp/a.pdf,/tmp/b.pdf --sync on"],
        client_wait_s: 30,
        ..BASE
    },
    Spec {
        name: "qa select-budget",
        tag: "qa_select_budget",
        group: GROUP_QA,
        access: Access::At(Level::Off),
        route: Route::Here,
        summary: "Measures selecting on one file, starting and ending its own quiet run: a first double-click, a long drag over several pages, select all, and a copy of all of it, with the footprint before, during and after, and the UI thread's worst step.",
        args: &[
            Arg {
                key: "file",
                flag: Some("--file"),
                kind: Kind::Path,
                required: true,
                help: "A PDF with text near its start.",
            },
            Arg {
                key: "app",
                flag: Some("--app"),
                kind: Kind::Path,
                required: false,
                help: "The app's executable, if it isn't the one built beside this `recto`.",
            },
        ],
        data: "`{ build, pages, footprint_mib: { before, after_drag, after_select_all, peak_during_copy, worker_peak_during_copy, after }, text_held_kib, threads, handles, first_double_click_to_highlight_ms, drag: { steps, round_trip_ms, total_ms }, ui_thread: { worst_pointer_step_us, worst_stall_ms }, select_all_ms, copy: { ms, chars, notice } }`.",
        examples: &["recto qa select-budget --file report.pdf"],
        client_wait_s: 30,
        ..BASE
    },
];
