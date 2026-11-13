//! **The one list of commands.** The CLI's grammar, `recto help`, the level checks, `SKILL.md` and `REFERENCE.md` all come from [`TABLE`], so nothing documented can be missing and nothing can be
//! left out of the checks. Tests walk it: every command has a row, every row runs, every example parses.

use crate::Code;
use crate::{AgentError, Level, exit};
use serde_json::{Map, Value, json};
use std::path::PathBuf;

/// Who may run a command.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Access {
    /// An agent at this level or above.
    At(Level),
    /// The user alone, at their own terminal. No level reaches this, and nothing over the socket does.
    UserOnly,
    /// Nobody through this interface, at any level.
    Nobody,
}

impl Access {
    pub fn label(self) -> &'static str {
        match self {
            Self::At(level) => level.label(),
            Self::UserOnly => "the user, at a terminal",
            Self::Nobody => "nobody, at any level",
        }
    }
}

/// Where a command runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Route {
    /// In the session, over the socket.
    Session,
    /// In the `recto` process itself.
    Here,
}

/// What a command-line value is, and so what it accepts. Units are the same everywhere.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// `#3`, `active`, or part of a tab's name.
    Tab,
    /// A page, counted from 1.
    Page,
    /// `4` or `4-6`.
    Pages,
    /// `4`, `4-6`, or `4-` to the end.
    PagesOpen,
    /// A whole number no larger than this.
    Count(u32),
    /// A whole number from 1 up to this.
    Positive(u32),
    /// A file. Made whole here, because the session has no folder of its own.
    Path,
    /// `2` or `2x`.
    Scale,
    /// `2`, `2x`, or `max` for the largest that fits.
    ScaleOrMax,
    /// `500ms`, `30s`, `5m`, `1h`. A bare number could mean anything, so it's refused.
    Duration,
    /// `90`, `180`, `270` or `-90`.
    Degrees,
    /// `off`, `read_only`, `control`, `full`.
    Level,
    /// There or not.
    Switch,
    Text,
    /// One of these words.
    Choice(&'static [&'static str]),
    /// Some of these words, with commas between.
    List(&'static [&'static str]),
    /// `WIDTHxHEIGHT`.
    Size,
    /// `x,y,width,height` in points.
    Rect,
    /// A share of something, from `0` to `1`, as `0.3`.
    Fraction,
    /// Everything left on the line.
    Rest,
}

/// One argument of one command.
#[derive(Clone, Copy, Debug)]
pub struct Arg {
    /// The field it fills in the command's JSON.
    pub key: &'static str,
    /// Its flag, or `None` for a value in its place.
    pub flag: Option<&'static str>,
    pub kind: Kind,
    pub required: bool,
    pub help: &'static str,
}

/// One command.
#[derive(Clone, Copy, Debug)]
pub struct Spec {
    /// What's typed, such as `page move`.
    pub name: &'static str,
    /// Its `cmd` on the wire. Rows sharing one are different faces of one command.
    pub tag: &'static str,
    pub group: &'static str,
    pub access: Access,
    pub route: Route,
    pub summary: &'static str,
    pub args: &'static [Arg],
    /// Fields every use of this row sets.
    pub fixed: &'static [(&'static str, &'static str)],
    /// What `data` holds.
    pub data: &'static str,
    /// What it tells watchers when it succeeds.
    pub emits: &'static [&'static str],
    pub examples: &'static [&'static str],
    /// How long the client waits for an answer, in seconds, before saying the session isn't answering. Waits add their own time.
    pub client_wait_s: u64,
    /// Not for people or the docs, though it's a real command.
    pub hidden: bool,
    /// It isn't built yet, and only says so.
    pub not_built: bool,
    /// A bare whole number on the line means its `--page`, because three agents in three rounds tried it. Ranges stay refused, since it takes one page.
    pub bare_page: bool,
}

/// Most lines of context a search gives each side of a hit.
pub(crate) const MAX_SEARCH_CONTEXT: u32 = 20;

pub(crate) const BASE: Spec = Spec {
    name: "",
    tag: "",
    group: "",
    access: Access::At(Level::ReadOnly),
    route: Route::Session,
    summary: "",
    args: &[],
    fixed: &[],
    data: "",
    emits: &[],
    examples: &[],
    client_wait_s: 120,
    hidden: false,
    not_built: false,
    bare_page: false,
};

/// What `save` and `save-as` answer.
pub(crate) const SAVED: &str =
    "`{ ...same as undo, did, written, writer: append|rewrite|copy, bytes_before, bytes, notes: [..], backup_kept, backup (Full only), untrusted: { file } }`.";

pub(crate) const TAB: Arg = Arg {
    key: "tab",
    flag: Some("--tab"),
    kind: Kind::Tab,
    required: false,
    help: "Which tab: `#3`, `active`, or part of its name. The active tab if left out.",
};
pub(crate) const PAGE: Arg = Arg {
    key: "page",
    flag: Some("--page"),
    kind: Kind::Page,
    required: true,
    help: "The page, counted from 1.",
};
pub(crate) const PAGE_OR_BARE: Arg = Arg {
    key: "page",
    flag: Some("--page"),
    kind: Kind::Page,
    required: true,
    help: "The page, counted from 1. A bare number means the same: `60` or `--page 60`.",
};
pub(crate) const PICK_PAGE: Arg = Arg {
    key: "page",
    flag: Some("--page"),
    kind: Kind::Page,
    required: false,
    help: "The page it starts on, counted from 1. A bare number means the same. With nothing else, the whole page.",
};
pub(crate) const PICK_TO_PAGE: Arg = Arg {
    key: "to_page",
    flag: Some("--to-page"),
    kind: Kind::Page,
    required: false,
    help: "The page it ends on, if that's a later one.",
};
pub(crate) const PICK_CHARS: Arg = Arg {
    key: "chars",
    flag: Some("--chars"),
    kind: Kind::Text,
    required: false,
    help: "`START-END` in characters of `recto text`, the end not included. With `--to-page`, the start is on the first page and the end on the last.",
};
pub(crate) const PICK_AT: Arg = Arg {
    key: "at",
    flag: Some("--at"),
    kind: Kind::Text,
    required: false,
    help: "`X,Y` on the page as it's shown, in points from its top left: where a click lands, whatever the page's rotation.",
};
pub(crate) const PICK_TO: Arg = Arg {
    key: "to",
    flag: Some("--to"),
    kind: Kind::Text,
    required: false,
    help: "`X,Y` where a drag from `--at` ends, on `--to-page` if that's given.",
};
pub(crate) const PICK_UNIT: Arg = Arg {
    key: "unit",
    flag: Some("--unit"),
    kind: Kind::Choice(&["char", "word", "line"]),
    required: false,
    help: "What `--at` takes: `word` as a double-click does and `line` as a triple-click does, going by those if there's a drag too. `word` for a click and `char` for a drag if left out.",
};
pub(crate) const PICK_ALL: Arg = Arg {
    key: "all",
    flag: Some("--all"),
    kind: Kind::Switch,
    required: false,
    help: "The whole document, as ⌘A does.",
};
/// `--anyway` on an export: owner, 2026-11-08, exports of a file that asks not to be copied get the same step as Copy.
pub(crate) const EXPORT_ANYWAY: Arg = Arg {
    key: "anyway",
    flag: Some("--anyway"),
    kind: Kind::Switch,
    required: false,
    help: "Export a file whose permissions ask that it isn't copied. Only once the user has said to. Each use goes in the activity log.",
};
pub(crate) const PICK_ANYWAY: Arg = Arg {
    key: "anyway",
    flag: Some("--anyway"),
    kind: Kind::Switch,
    required: false,
    help: "Give the text of a file whose permissions ask that it isn't copied. Only once the user has said to. Each use goes in the activity log.",
};
pub(crate) const SCALE_OR_MAX: Arg = Arg {
    key: "scale",
    flag: Some("--scale"),
    kind: Kind::ScaleOrMax,
    required: false,
    help: "How large: `2` or `2x` is twice the page's size, and `max` is the largest that fits, which the answer names. 2x if left out.",
};
pub(crate) const SCALE: Arg = Arg {
    key: "scale",
    flag: Some("--scale"),
    kind: Kind::Scale,
    required: false,
    help: "How large: `2` or `2x` is twice the page's size. 2x if left out.",
};
pub(crate) const TIMEOUT: Arg = Arg {
    key: "timeout_ms",
    flag: Some("--timeout"),
    kind: Kind::Duration,
    required: false,
    help: "How long to wait: `500ms`, `30s`, `5m`. 30s if left out.",
};
pub(crate) const FIRST: Arg = Arg {
    key: "first",
    flag: Some("--first"),
    kind: Kind::Page,
    required: false,
    help: "The page to start from. The first page if left out.",
};

pub const GROUP_START: &str = "Start here";
pub const GROUP_READ: &str = "Read a document";
pub const GROUP_MOVE: &str = "Open and move around";
pub const GROUP_EDIT: &str = "Change pages, then save";
pub const GROUP_SYNC: &str = "Wait instead of polling";
pub const GROUP_ACCESS: &str = "Access and history";
pub const GROUP_SESSION: &str = "The session";
pub const GROUP_APP: &str = "The app itself";
pub const GROUP_SKILL: &str = "Skill files";
pub const GROUP_MEASURE: &str = "Measuring";
/// Testing the Mac app. Quiet runs on temp state: these reach only an app started with `recto qa start`, and the skill for the user's agents leaves them out.
pub const GROUP_QA: &str = "Testing the app";

pub(crate) const AGENT_EXITS_PAGE: &str = "`{ tab, sel, page, has_label, pages, file_pages, can_go_back, can_go_forward, active, edited, user_edits_undone, structure, problem, note, untrusted: { name, label } }`. `untrusted.label` is what the file calls the page, such as `iv`, where that isn't its number, and `has_label` says there is one. `user_edits_undone: true` means the user's edits are undone and can still be redone, so the page matches the file but closing would lose them.";
pub(crate) const EDIT_STATE: &str = "`{ did, tab, pages, file_pages, page, edited, saved, user_edits_undone, can_undo, can_redo, note, page_was_removed, place_note }`. `did` says what was just done, in words. `pages` is its length with edits, and `file_pages` the file's. The tab stays on the page it was reading, wherever that goes. If that page was removed, `page_was_removed` says so and it's on the nearest one.";
pub(crate) const STATUS_DATA: &str =
    "`{ running, pid, version, api, level, uptime_ms, socket, tabs }`. `tabs` is how many PDFs are open, at every level, and never their names.";

/// Every command, in the order `recto help` shows them: each command's rows from its module, joined by [`crate::commands`].
pub static TABLE: &[Spec] = &crate::commands::ROWS;

pub(crate) const CLI_HELP: &[Spec] = &[
    Spec {
        name: "help",
        tag: "help",
        group: GROUP_START,
        access: Access::At(Level::Off),
        route: Route::Here,
        summary: "This list, or everything about one command.",
        args: &[Arg {
            key: "command",
            flag: None,
            kind: Kind::Rest,
            required: false,
            help: "The command to explain, such as `page move`.",
        }],
        data: "With `--json`: the command table itself.",
        examples: &["recto help", "recto help page move", "recto help --json"],
        ..BASE
    },
    Spec {
        name: "version",
        tag: "version",
        group: GROUP_START,
        access: Access::At(Level::Off),
        route: Route::Here,
        summary: "This program's version, and the API version it speaks.",
        data: "`{ version, api }`.",
        examples: &["recto version --json"],
        ..BASE
    },
];

pub(crate) const CLI_PERMISSION_SET: &[Spec] = &[Spec {
    name: "permission set",
    tag: "permission_set",
    group: GROUP_ACCESS,
    access: Access::UserOnly,
    route: Route::Here,
    summary: "For the user: sets what agents may do. Raising it needs a real terminal and a word typed back. No flag, variable or message can do it. Later the app's settings do this.",
    args: &[Arg {
        key: "level",
        flag: None,
        kind: Kind::Level,
        required: true,
        help: "`off`, `read_only`, `control` or `full`.",
    }],
    data: "`{ level, label, was }`.",
    examples: &["recto permission set read_only", "recto permission set off"],
    ..BASE
}];

pub(crate) const CLI_SERVE: &[Spec] = &[Spec {
    name: "serve",
    tag: "serve",
    group: GROUP_SESSION,
    access: Access::At(Level::Off),
    route: Route::Here,
    summary: "Runs the session, on a socket only this user can reach, with no network listener. Tabs last until it stops.",
    args: &[Arg {
        key: "background",
        flag: Some("--background"),
        kind: Kind::Switch,
        required: false,
        help: "Starts it and returns once it's answering. Does nothing if it's already running.",
    }],
    data: "`{ running, pid, socket, already }`.",
    examples: &["recto serve --background", "recto serve"],
    ..BASE
}];

pub(crate) const CLI_LOCAL: &[Spec] = &[
    Spec {
        name: "local",
        tag: "local",
        group: GROUP_SESSION,
        access: Access::At(Level::Off),
        route: Route::Here,
        summary: "For tests: runs one command in a throwaway session in this process, with one file open. The same commands, levels and log. Tabs vanish on exit.",
        args: &[
            Arg {
                key: "file",
                flag: None,
                kind: Kind::Path,
                required: true,
                help: "The PDF to open first. Opening it needs Control.",
            },
            Arg {
                key: "command",
                flag: None,
                kind: Kind::Rest,
                required: true,
                help: "The command to run on it.",
            },
        ],
        data: "The command's answer.",
        examples: &["recto local report.pdf text --page 1 --json", "recto local report.pdf budget render --count 5"],
        ..BASE
    },
    Spec {
        name: "session",
        tag: "session",
        group: GROUP_SESSION,
        access: Access::At(Level::Off),
        route: Route::Here,
        summary: "For QA scripts: a throwaway session in this process, with one JSON command per line on stdin and one reply per line. The same levels and log.",
        data: "One reply per line.",
        examples: &["recto session"],
        ..BASE
    },
    Spec {
        name: "skill show",
        tag: "skill_show",
        group: GROUP_SKILL,
        access: Access::At(Level::Off),
        route: Route::Here,
        summary: "Prints the generated skill.",
        args: &[Arg {
            key: "file",
            flag: None,
            kind: Kind::Choice(&["SKILL.md", "REFERENCE.md"]),
            required: false,
            help: "`SKILL.md` or `REFERENCE.md`. `SKILL.md` if left out.",
        }],
        data: "The file's text.",
        examples: &["recto skill show", "recto skill show REFERENCE.md"],
        ..BASE
    },
    Spec {
        name: "skill install",
        tag: "skill_install",
        group: GROUP_SKILL,
        access: Access::UserOnly,
        route: Route::Here,
        summary: "For the user: writes SKILL.md and REFERENCE.md into their agent's files, after a word typed back at a real terminal. Refuses to replace a file someone changed, and says so.",
        args: &[
            Arg {
                key: "path",
                flag: Some("--path"),
                kind: Kind::Path,
                required: false,
                help: "The folder to write into.",
            },
            Arg {
                key: "target",
                flag: Some("--target"),
                kind: Kind::Choice(&["claude-code"]),
                required: false,
                help: "`claude-code` is Claude Code's own place for skills, `~/.claude/skills/recto/`.",
            },
            Arg {
                key: "replace",
                flag: Some("--replace"),
                kind: Kind::Switch,
                required: false,
                help: "Replace files even if someone changed them. There's no backup.",
            },
            Arg {
                key: "dry_run",
                flag: Some("--dry-run"),
                kind: Kind::Switch,
                required: false,
                help: "Says what it would write, and writes nothing.",
            },
        ],
        data: "`{ folder, files: [ { path, action } ], dry_run }`.",
        examples: &[
            "recto skill install --target claude-code --dry-run",
            "recto skill install --path ./skills/recto",
        ],
        ..BASE
    },
];

pub(crate) const CLI_BUDGET_TABS: &[Spec] = &[
    Spec {
        name: "budget tabs",
        tag: "budget_tabs",
        group: GROUP_MEASURE,
        access: Access::At(Level::Control),
        route: Route::Here,
        summary: "Opens one PDF as many tabs, through links in a temp folder, and reports memory and handles. Through the session, or in this process with `--in-process`.",
        args: &[
            Arg {
                key: "file",
                flag: Some("--file"),
                kind: Kind::Path,
                required: true,
                help: "The PDF. It's only read.",
            },
            Arg {
                key: "count",
                flag: Some("--count"),
                kind: Kind::Count(10_000),
                required: true,
                help: "How many tabs.",
            },
            Arg {
                key: "in_process",
                flag: Some("--in-process"),
                kind: Kind::Switch,
                required: false,
                help: "Measure this process, with no session.",
            },
        ],
        data: "`{ route, tabs, file_bytes, start_footprint_bytes, samples, start_handles, end_handles, ms_per_open }`.",
        examples: &["recto budget tabs --file big.pdf --count 1000"],
        client_wait_s: 1800,
        ..BASE
    },
    Spec {
        name: "budget index",
        tag: "budget_index",
        group: GROUP_MEASURE,
        access: Access::At(Level::Control),
        route: Route::Here,
        summary: "Builds one PDF's index in the system's temp folder and reports what that costs, and search times with and without it. Removes the folder after.",
        args: &[
            Arg {
                key: "file",
                flag: Some("--file"),
                kind: Kind::Path,
                required: true,
                help: "The PDF. It's only read.",
            },
            Arg {
                key: "queries",
                flag: Some("--queries"),
                kind: Kind::Text,
                required: false,
                help: "Searches to time, with commas between. A word nothing has, a common one and a phrase if left out.",
            },
            Arg {
                key: "runs",
                flag: Some("--runs"),
                kind: Kind::Positive(1000),
                required: false,
                help: "Times each search runs. 7 if left out.",
            },
            Arg {
                key: "foreground",
                flag: Some("--foreground"),
                kind: Kind::Switch,
                required: false,
                help: "Build at this program's own priority, not the helpers' utility, to measure the work apart from the waiting.",
            },
        ],
        data: "`{ file_bytes, build: { state, pages, elapsed_ms, corpus_bytes, pack_bytes, helper_peak_bytes, worker_peak_bytes, worker_spawns, worker_retirements, memory_kills, deadline_kills, crashes, hidden_check_failures, failed_pages, no_text_pages, hidden_flagged_pages, hidden_unchecked_pages }, searches: [ { query, index: { p50_ms, p95_ms, hits_total }, scan: { p50_ms, p95_ms } } ], corpus_folder, this_process_peak_bytes }`.",
        examples: &[
            "recto budget index --file big.pdf",
            "recto budget index --file big.pdf --queries \"force majeure,the\" --runs 21 --foreground",
        ],
        client_wait_s: 1800,
        ..BASE
    },
];

/// What `watch --events` can ask for.
pub const EVENT_GROUPS: &[&str] = &["tabs", "pages", "edits", "draws", "indexing", "contents", "access", "view", "find"];

/// Every event a session tells watchers of, with the `watch --events` group it's in, in the order of [`EVENT_GROUPS`]. **No default** (review C6): the `Event` type names every event's group in an exhaustive `match`, and a test holds this list to the type.
pub const EVENTS: &[(&str, &str)] = &[
    ("tab_opened", "tabs"),
    ("tab_closed", "tabs"),
    ("tab_activated", "tabs"),
    ("page_changed", "pages"),
    ("edits_changed", "edits"),
    ("saved", "edits"),
    ("page_drawn", "draws"),
    ("page_failed", "draws"),
    ("indexing_started", "indexing"),
    ("indexing_finished", "indexing"),
    ("contents_generating", "contents"),
    ("contents_generated", "contents"),
    ("level_changed", "access"),
    ("view_changed", "view"),
    ("find_changed", "find"),
    ("find_finished", "find"),
];

/// The group an event belongs to, if it's listed.
pub fn event_group(name: &str) -> Option<&'static str> {
    EVENTS.iter().find(|(event, _)| *event == name).map(|(_, group)| *group)
}

/// The row for a wire name. Rows sharing a tag share their access, which a test holds them to.
pub fn by_tag(tag: &str) -> Option<&'static Spec> {
    TABLE.iter().find(|spec| spec.tag == tag)
}

/// What to call a wire name to a person.
pub fn display_name(tag: &str) -> &'static str {
    by_tag(tag).map_or("that", |spec| spec.name)
}

/// A parsed command line.
#[derive(Debug)]
pub struct Invocation {
    pub spec: &'static Spec,
    /// The command's fields, as they go on the wire.
    pub fields: Map<String, Value>,
    pub json: bool,
    /// `--help` was on the line.
    pub help: bool,
    /// What `Kind::Rest` caught.
    pub rest: Vec<String>,
}

impl Invocation {
    /// The command this line means, parsed exactly as the socket parses one.
    pub fn command(&self) -> Result<crate::Command, AgentError> {
        crate::parse(&self.wire())
    }

    /// The line sent to the session.
    pub fn wire(&self) -> String {
        let mut fields = self.fields.clone();
        fields.insert("cmd".into(), json!(self.spec.tag));
        Value::Object(fields).to_string()
    }
}

fn usage_error(message: impl Into<String>, next: impl Into<String>) -> AgentError {
    AgentError::new(Code::USAGE, message, exit::USAGE).then(next)
}

/// The rows whose value is a secret read from stdin rather than the command line, under this field: a password typed for a test (review O6).
pub fn stdin_secret_field(spec: &Spec) -> Option<&'static str> {
    (spec.name == "qa password").then_some("value")
}

/// Reads a command line. Unknown commands and flags are refused, with the closest real ones, never guessed at.
pub fn parse_cli(args: &[String]) -> Result<Invocation, AgentError> {
    let mut json_flag = false;
    let mut help_flag = false;
    let mut kept: Vec<&str> = Vec::with_capacity(args.len());
    let mut words_only = false;
    let mut literal_from = usize::MAX;
    for arg in args {
        match arg.as_str() {
            "--" if !words_only => {
                words_only = true;
                literal_from = kept.len();
            }
            "--json" if !words_only => json_flag = true,
            "--help" | "-h" if !words_only => help_flag = true,
            // What grep and ripgrep call it, which is what anyone reaching for it will type.
            "-C" if !words_only => kept.push("--context"),
            other => kept.push(other),
        }
    }
    if let ["--version" | "-V"] = kept.as_slice() {
        return Ok(here_row("version", json_flag, Vec::new()));
    }
    let Some(spec) = find_spec(&kept) else {
        if help_flag {
            // `recto --help` is the list, and `recto page --help` the group's.
            match kept.as_slice() {
                [] => return Ok(here_row("help", json_flag, Vec::new())),
                [group] if family_rows(group).next().is_some() => return Ok(here_row("help", json_flag, vec![(*group).to_string()])),
                _ => {}
            }
        }
        if kept.is_empty() {
            return Err(usage_error("Which command? `recto help` lists them.", "Try `recto status`, or `recto help`.").with_candidates(Vec::new()));
        }
        return Err(unknown_command(&kept));
    };
    let words = spec.name.split(' ').count();
    let (fields, rest) = read_args(spec, &kept[words..], literal_from.saturating_sub(words), help_flag, json_flag)?;
    Ok(Invocation {
        spec,
        fields,
        json: json_flag,
        help: help_flag,
        rest,
    })
}

/// A line that means one of the rows that runs here, however it was typed.
fn here_row(tag: &str, json: bool, rest: Vec<String>) -> Invocation {
    let spec = by_tag(tag).unwrap_or(&TABLE[0]);
    Invocation {
        spec,
        fields: Map::new(),
        json,
        help: false,
        rest,
    }
}

impl AgentError {
    #[must_use]
    fn with_candidates(mut self, candidates: Vec<String>) -> Self {
        self.candidates = candidates;
        self
    }
}

/// The row a line of words names, for `recto help page move`, or `recto help "page move"` as its own list prints it. Hidden rows aren't found.
pub fn find_row(words: &[String]) -> Option<&'static Spec> {
    let words: Vec<&str> = words.iter().flat_map(|word| word.split_whitespace()).collect();
    find_spec(&words)
}

/// The rows named by a first word and something more, such as `page move` for `page`.
fn family_rows(first: &str) -> impl Iterator<Item = &'static Spec> {
    TABLE
        .iter()
        .filter(move |spec| !spec.hidden && spec.name.split_once(' ').is_some_and(|(group, _)| group == first))
}

/// `recto help page`: the commands of a group.
pub fn family_help(words: &[String]) -> Option<String> {
    let words: Vec<&str> = words.iter().flat_map(|word| word.split_whitespace()).collect();
    let [group] = words.as_slice() else { return None };
    let rows: Vec<&Spec> = family_rows(group).collect();
    if rows.is_empty() {
        return None;
    }
    let mut out = format!("`recto {group}` needs one more word:\n\n");
    for spec in rows {
        out.push_str(&format!("  {:<28} {}\n", usage_name(spec), first_sentence(spec.summary)));
    }
    out.push_str(&format!("\n`recto help {group} <word>` explains one.\n"));
    Some(out)
}

fn usage_name(spec: &Spec) -> String {
    format!("recto {}", spec.name)
}

/// The longest row whose name starts the line.
fn find_spec(words: &[&str]) -> Option<&'static Spec> {
    TABLE
        .iter()
        .filter(|spec| !spec.hidden)
        .filter(|spec| {
            let name: Vec<&str> = spec.name.split(' ').collect();
            words.len() >= name.len() && name.iter().zip(words).all(|(a, b)| a == b)
        })
        .max_by_key(|spec| spec.name.len())
}

fn unknown_command(words: &[&str]) -> AgentError {
    // Only short plain words go in the reply: this echoes what was typed.
    let typed: String = words.iter().take(2).copied().collect::<Vec<_>>().join(" ");
    let typed = recto_core::clean_line(&typed);
    let first = words.first().copied().unwrap_or("");
    if let [group] = words
        && family_rows(group).next().is_some()
    {
        let names: Vec<String> = family_rows(group).map(|spec| spec.name.to_string()).collect();
        return usage_error(
            format!("`{typed}` needs one more word."),
            format!("Use `recto {}`. `recto help {typed}` lists them.", names.join("`, `recto ")),
        )
        .with_candidates(names);
    }
    let candidates = nearest_commands(first, &typed);
    let next = match candidates.as_slice() {
        [] => "`recto help` lists every command.".to_string(),
        some => format!("Perhaps `recto {}`. `recto help` lists every command.", some.join("`, `recto ")),
    };
    usage_error(format!("`{typed}` isn't a command."), next).with_candidates(candidates)
}

/// The rows a mistyped command most likely meant, nearest first. By how close the name is, then whether its summary's first sentence uses the word: `recto doc` is near nothing by spelling, and `facts` is about "a document". All from the table, so there's no list of synonyms to keep up.
fn nearest_commands(first: &str, typed: &str) -> Vec<String> {
    const MOST: usize = 4;
    let first_lower = first.to_lowercase();
    // Testing the app is nobody's near miss but someone who typed `qa`.
    let offered = |spec: &&Spec| !spec.hidden && (spec.group != GROUP_QA || first_lower == "qa");
    let mut close: Vec<(usize, &str)> = TABLE
        .iter()
        .filter(offered)
        .filter_map(|spec| {
            let same_family = spec.name.split(' ').next() == Some(first);
            let spelled = distance(spec.name, typed)
                .min(distance(spec.name, first))
                .min(distance(spec.name.split(' ').next().unwrap_or(spec.name), first));
            // A short word is close to every other short word. Only as close as half its letters counts.
            let near = spelled <= 3 && spelled * 2 <= first.chars().count().max(spec.name.chars().count());
            (same_family || near).then_some((if same_family { 0 } else { spelled }, spec.name))
        })
        .collect();
    close.sort_by_key(|(spelled, _)| *spelled);
    close.truncate(MOST);
    let mut names: Vec<String> = close.into_iter().map(|(_, name)| name.to_string()).collect();
    if first_lower.chars().count() >= 3 {
        for spec in TABLE.iter().filter(offered) {
            if names.len() >= MOST {
                break;
            }
            let mentions = first_sentence(spec.summary)
                .split(|c: char| !c.is_alphanumeric())
                .any(|word| word.to_lowercase().starts_with(&first_lower));
            if mentions && !names.iter().any(|name| name == spec.name) {
                names.push(spec.name.to_string());
            }
        }
    }
    names
}

/// Edit distance, for suggesting a command. Short strings only.
fn distance(a: &str, b: &str) -> usize {
    let (a, b): (Vec<char>, Vec<char>) = (a.chars().take(32).collect(), b.chars().take(32).collect());
    let mut row: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.iter().enumerate() {
        let mut previous = row[0];
        row[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let cost = previous + usize::from(ca != cb);
            previous = row[j + 1];
            row[j + 1] = cost.min(row[j] + 1).min(row[j + 1] + 1);
        }
    }
    row[b.len()]
}

/// What's wrong with a line, for whoever can do something about it.
enum Wrong<'a> {
    /// A flag this row doesn't have.
    Flag {
        at: usize,
        flag: &'a str,
        inline: Option<&'a str>,
    },
    /// A word with no place to go, and what the line had filled by then.
    Stray {
        at: usize,
        fields: Map<String, Value>,
    },
    Other(AgentError),
}

fn read_args(spec: &'static Spec, words: &[&str], literal_from: usize, help: bool, json: bool) -> Result<(Map<String, Value>, Vec<String>), AgentError> {
    scan(spec, words, literal_from, help).map_err(|wrong| {
        let advice = || advised_line(spec, words, literal_from, json);
        match wrong {
            Wrong::Other(error) => error,
            Wrong::Flag { flag, .. } => unknown_flag(spec, flag, advice()),
            Wrong::Stray { at, fields } => stray_word(spec, words[at], &fields, advice()),
        }
    })
}

/// `literal_from` is where `--` was: from there on, nothing is a flag, so a search can look for `--force`.
fn scan<'a>(spec: &'static Spec, words: &[&'a str], literal_from: usize, help: bool) -> Result<(Map<String, Value>, Vec<String>), Wrong<'a>> {
    let wrong = |message: String| Wrong::Other(usage_error(message, format!("`recto help {}` shows how to use it.", spec.name)));
    let mut fields = Map::new();
    let mut rest = Vec::new();
    for (key, value) in spec.fixed {
        fields.insert((*key).into(), json!(value));
    }
    let mut places = spec.args.iter().filter(|arg| arg.flag.is_none());
    let mut index = 0;
    while let Some(&word) = words.get(index) {
        index += 1;
        if word.starts_with("--") && word.len() > 2 && index <= literal_from {
            // `--page=3` and `--page 3` are the same thing.
            let (flag, inline) = word.split_once('=').map_or((word, None), |(flag, value)| (flag, Some(value)));
            let Some(arg) = spec.args.iter().find(|arg| arg.flag == Some(flag)) else {
                return Err(Wrong::Flag { at: index - 1, flag, inline });
            };
            // The last one winning would be a guess at which was meant.
            if fields.contains_key(arg.key) {
                return Err(wrong(format!("`recto {}` has `{flag}` twice, or its value in its place as well.", spec.name)));
            }
            if arg.kind == Kind::Switch {
                if inline.is_some() {
                    return Err(wrong(format!("`{flag}` is there or it isn't, and doesn't take a value.")));
                }
                fields.insert(arg.key.into(), json!(true));
                continue;
            }
            let value = match inline {
                Some(value) => value,
                None => {
                    let Some(&value) = words.get(index) else {
                        return Err(wrong(format!("`{flag}` needs a value.")));
                    };
                    index += 1;
                    value
                }
            };
            fields.insert(arg.key.into(), read_value(arg, value).map_err(wrong)?);
        } else {
            let Some(arg) = places.next() else {
                if spec.bare_page
                    && !fields.contains_key("page")
                    && let Some(page) = spec.args.iter().find(|arg| arg.key == "page").filter(|_| whole_number(word).is_some())
                {
                    fields.insert(page.key.into(), read_value(page, word).map_err(wrong)?);
                    continue;
                }
                return Err(Wrong::Stray { at: index - 1, fields });
            };
            if arg.kind == Kind::Rest {
                rest = words[index - 1..].iter().map(|word| (*word).to_string()).collect();
                break;
            }
            fields.insert(arg.key.into(), read_value(arg, word).map_err(wrong)?);
        }
    }
    if !help
        && let Some(missing) = spec
            .args
            .iter()
            .find(|arg| arg.required && arg.kind != Kind::Rest && !fields.contains_key(arg.key))
    {
        let shown = missing.flag.map_or_else(|| format!("<{}>", missing.key), str::to_string);
        return Err(wrong(format!("`recto {}` needs {shown}.", spec.name)));
    }
    if !help && spec.args.iter().any(|arg| arg.required && arg.kind == Kind::Rest) && rest.is_empty() {
        return Err(wrong(format!("`recto {}` needs a command to run.", spec.name)));
    }
    Ok((fields, rest))
}

/// What's said after any wrong word on a line: how the command goes, from its row.
fn how_it_goes(spec: &Spec) -> String {
    format!("It goes `{}`. `recto help {}` shows how to use it.", usage_line(spec), spec.name)
}

/// Longest word advice will quote back.
pub(crate) const MAX_QUOTED_WORD: usize = 256;

/// A word as it has to be typed for a shell to pass it on exactly, or `None` if advice shouldn't print it at all: too long, or holding a line break or anything else `clean_line` would take out, which could make a line of advice into two commands. What's typed can come from a file's name, and whoever reads this may run it. Anything outside a short safe set goes in quotes, with double quotes where nothing inside them means anything to a shell, as the help writes `"#1"`, and single quotes otherwise, where nothing means anything at all.
pub(crate) fn shell_quote(word: &str) -> Option<String> {
    if word.is_empty() || word.chars().count() > MAX_QUOTED_WORD || recto_core::clean_line(word) != word {
        return None;
    }
    if word.chars().all(|c| c.is_alphanumeric() || "_./:=@%+,-".contains(c)) {
        return Some(word.to_string());
    }
    if !word.contains(['$', '`', '"', '\\', '!']) {
        return Some(format!("\"{word}\""));
    }
    Some(format!("'{}'", word.replace('\'', "'\\''")))
}

/// How near a flag someone typed is to one that exists, by its spelling, what's after the `--` being what counts. `--page` isn't near `--tab`, though only 3 letters apart: that's all of them.
fn flag_distance(typed: &str, real: &str) -> Option<usize> {
    let (typed, real) = (typed.trim_start_matches('-'), real.trim_start_matches('-'));
    if typed.len() >= 3 && real.len() >= 3 && (typed.starts_with(real) || real.starts_with(typed)) {
        return Some(1);
    }
    let apart = distance(typed, real);
    (apart <= 2 && apart < typed.chars().count().min(real.chars().count())).then_some(apart)
}

/// The argument a flag that doesn't exist most likely meant: one named that, whatever its flag is, or else one spelled nearly that, a value with no flag going by its name.
fn nearest_arg(spec: &Spec, flag: &str) -> Option<&'static Arg> {
    let named = flag.trim_start_matches('-').replace('-', "_");
    let by_name = spec.args.iter().find(|arg| arg.key == named || arg.key.trim_end_matches("_ms") == named);
    by_name.or_else(|| {
        spec.args
            .iter()
            .filter_map(|arg| Some((flag_distance(flag, arg.flag.unwrap_or(arg.key))?, arg)))
            .min_by_key(|(apart, _)| *apart)
            .map(|(_, arg)| arg)
    })
}

/// The most times advice will mend one line.
pub(crate) const MAX_MENDS: usize = 8;

/// The whole line as it should have been typed: this line, with every wrong word mended and every other word where it was, ready to run. `page delete --pages 3 --tab report` must come back with its `--tab`, or the advice deletes page 3 of whichever tab is active. So nothing is offered unless all of the line can be mended and put back, and each mend goes through the same reading as a real line. Anything still missing stays missing, as it was typed, and is refused for that when it's run.
fn advised_line(spec: &'static Spec, words: &[&str], literal_from: usize, json: bool) -> Option<String> {
    // Past `--`, nothing can be moved or renamed without changing what it means.
    if literal_from <= words.len() {
        return None;
    }
    let mut line: Vec<String> = words.iter().map(|word| (*word).to_string()).collect();
    for _ in 0..MAX_MENDS {
        let view: Vec<&str> = line.iter().map(String::as_str).collect();
        let (range, mended) = match scan(spec, &view, usize::MAX, true) {
            Ok(_) => {
                let mut typed = vec!["recto".to_string()];
                typed.extend(spec.name.split(' ').map(str::to_string));
                for word in &line {
                    typed.push(shell_quote(word)?);
                }
                if json {
                    typed.push("--json".to_string());
                }
                return Some(typed.join(" "));
            }
            Err(Wrong::Flag { at, flag, inline }) => mend_flag(spec, &view, at, flag, inline)?,
            Err(Wrong::Stray { at, .. }) => (
                at..at + 1,
                flag_for_stray_word(spec, view[at])
                    .filter(|(flag, _)| !view.contains(flag))
                    .map(|(flag, value)| vec![flag.to_string(), value])?,
            ),
            Err(Wrong::Other(_)) => return None,
        };
        line.splice(range, mended);
    }
    None
}

/// What should replace a flag this row doesn't have, and the value after it if that goes too. A value is only moved if its argument takes it.
fn mend_flag(spec: &Spec, line: &[&str], at: usize, flag: &str, inline: Option<&str>) -> Option<(std::ops::Range<usize>, Vec<String>)> {
    let arg = nearest_arg(spec, flag)?;
    if arg.flag.is_some_and(|real| line.iter().any(|word| word.split('=').next() == Some(real))) {
        return None;
    }
    if arg.kind == Kind::Switch {
        return inline.is_none().then(|| (at..at + 1, vec![arg.flag.unwrap_or(flag).to_string()]));
    }
    let (value, taken) = match inline {
        Some(value) => (value, 1),
        None => (line.get(at + 1).copied().filter(|next| !next.starts_with("--"))?, 2),
    };
    read_value(arg, value).ok()?;
    let mended = arg
        .flag
        .map_or_else(|| vec![value.to_string()], |real| vec![real.to_string(), value.to_string()]);
    Some((at..at + taken, mended))
}

/// Refuses a flag the row doesn't have, with the closest form that would work: the whole line mended if it can be, and otherwise the flag or the place a value was probably meant for.
fn unknown_flag(spec: &'static Spec, flag: &str, advice: Option<String>) -> AgentError {
    let typed_flag = recto_core::clean_line(flag);
    let mut message = format!("`recto {}` has no `{typed_flag}`.", spec.name);
    let mut candidates = Vec::new();
    match (advice, nearest_arg(spec, flag)) {
        (Some(line), _) => {
            message.push_str(&format!(" Perhaps `{line}`."));
            candidates.push(line);
        }
        (None, Some(arg)) => match arg.flag {
            Some(real) => {
                message.push_str(&format!(" The nearest is `{real}`: {}", arg.help));
                candidates.push(real.to_string());
            }
            None => message.push_str(&format!(" `<{}>` goes in its place, with no flag: {}", arg.key, arg.help)),
        },
        (None, None) => {
            let flags: Vec<&str> = spec.args.iter().filter_map(|arg| arg.flag).collect();
            if !flags.is_empty() {
                message.push_str(&format!(" It has `{}`.", flags.join("`, `")));
            }
        }
    }
    usage_error(message, how_it_goes(spec)).with_candidates(candidates)
}

/// Refuses a word with no place to go, and says the form that would take it, if there's a good guess: a page number for its flag, a tab for `--tab`, more words for quotes around the words. Measurements count pages rather than name them, so they're left out.
fn stray_word(spec: &'static Spec, word: &str, fields: &Map<String, Value>, advice: Option<String>) -> AgentError {
    let shown = recto_core::clean_line(word);
    let mut message = format!("`recto {}` doesn't take `{shown}` here.", spec.name);
    let mut candidates = Vec::new();
    if spec.bare_page && crate::parse_pages(word, false).is_ok() {
        if fields.contains_key("page") {
            message = format!("`recto {}` has its page already, so `{shown}` can't be one too.", spec.name);
        } else {
            message = format!(
                "`recto {}` does one page at a time, so `{shown}` isn't a page. Run it once for each.",
                spec.name
            );
        }
    } else if spec.group != GROUP_MEASURE {
        let text_taken = spec
            .args
            .iter()
            .find(|arg| arg.kind == Kind::Text && arg.flag.is_none())
            .and_then(|arg| fields.get(arg.key))
            .and_then(Value::as_str);
        if let Some((flag, value)) = flag_for_stray_word(spec, word).and_then(|(flag, value)| Some((flag, shell_quote(&value)?))) {
            message = format!("`recto {}` takes that as `{flag} {value}`.", spec.name);
            match advice {
                Some(line) => {
                    message.push_str(&format!(" Perhaps `{line}`."));
                    candidates.push(line);
                }
                None => candidates.push(format!("{flag} {value}")),
            }
        } else if let Some(words) = text_taken.and_then(|before| shell_quote(&format!("{before} {word}"))) {
            message = format!(
                "`recto {}` takes one piece of text. Several words go in quotes: `recto {} {words}`.",
                spec.name, spec.name
            );
        } else if word.to_ascii_lowercase().ends_with(".pdf") && spec.args.iter().any(|arg| arg.kind == Kind::Tab) {
            let open = shell_quote(word).map_or_else(|| "`recto open`".to_string(), |quoted| format!("`recto open {quoted}`"));
            message = format!("`recto {}` works on a tab, not a file. {open} opens it, and `--tab` picks it.", spec.name);
        }
    }
    usage_error(message, how_it_goes(spec)).with_candidates(candidates)
}

/// The flag a stray word was probably meant for, and the value as it means it: `recto text 1` for `--page 1`, and `recto close "#1"` for `--tab "#1"`. Advice it would then refuse is worse than none: `--page` takes one page, and `--pages` can take a range. Measurements count pages rather than name them, so they're left out.
fn flag_for_stray_word(spec: &Spec, word: &str) -> Option<(&'static str, String)> {
    if spec.group == GROUP_MEASURE {
        return None;
    }
    let flagged = || spec.args.iter().filter(|arg| arg.flag.is_some());
    let for_a_page = flagged().find_map(|arg| match arg.kind {
        Kind::Page if whole_number(word).is_some_and(|page| page > 0) => arg.flag,
        Kind::Pages if crate::parse_pages(word, false).is_ok() => arg.flag,
        Kind::PagesOpen if crate::parse_pages(word, true).is_ok() => arg.flag,
        _ => None,
    });
    if let Some(flag) = for_a_page {
        return Some((flag, word.to_string()));
    }
    // Another word after a piece of text is more of that text, and quotes are the answer.
    if spec.args.iter().any(|arg| arg.kind == Kind::Text && arg.flag.is_none()) {
        return None;
    }
    let tab_flag = flagged().find(|arg| arg.kind == Kind::Tab)?.flag?;
    let has_pages = spec.args.iter().any(|arg| matches!(arg.kind, Kind::Page | Kind::Pages | Kind::PagesOpen));
    // A bare number is a tab's only where a page can't be what's meant. A word is a tab's if it could be part of a name, and not a file or a flag.
    let value = match whole_number(word) {
        Some(number) if number > 0 && !has_pages => format!("#{number}"),
        Some(_) => return None,
        None if word.ends_with('-') || word.starts_with('-') || word.contains('/') || word.to_ascii_lowercase().ends_with(".pdf") => return None,
        None if word.chars().any(char::is_alphabetic) || word.strip_prefix('#').and_then(whole_number).is_some() => word.to_string(),
        None => return None,
    };
    Some((tab_flag, value))
}

fn read_value(arg: &Arg, value: &str) -> Result<Value, String> {
    match arg.kind {
        Kind::Tab | Kind::Text => Ok(json!(value)),
        Kind::Page => match whole_number(value) {
            Some(0) | None => Err(format!("`{}` isn't a page. Pages count from 1.", recto_core::clean_line(value))),
            Some(page) => Ok(json!(page)),
        },
        Kind::Pages if value == "all" => Ok(json!(value)),
        Kind::Pages => crate::parse_pages(value, false)
            .map(|_| json!(value))
            .map_err(|_| format!("`{}` isn't pages. Write 4 or 4-6.", recto_core::clean_line(value))),
        Kind::PagesOpen => crate::parse_pages(value, true)
            .map(|_| json!(value))
            .map_err(|_| format!("`{}` isn't pages. Write 4, 4-6 or 4-.", recto_core::clean_line(value))),
        Kind::Count(max) => match whole_number(value) {
            Some(number) if number <= max => Ok(json!(number)),
            _ => Err(format!("`{}` should be a whole number up to {max}.", recto_core::clean_line(value))),
        },
        Kind::Positive(max) => match whole_number(value) {
            Some(number) if (1..=max).contains(&number) => Ok(json!(number)),
            _ => Err(format!("`{}` should be a whole number from 1 to {max}.", recto_core::clean_line(value))),
        },
        Kind::Path => {
            let path = std::path::absolute(PathBuf::from(value)).map_err(|_| "That path couldn't be read.".to_string())?;
            path.to_str().map(|path| json!(path)).ok_or_else(|| "That path isn't valid text.".to_string())
        }
        Kind::ScaleOrMax if value == "max" => Ok(json!("max")),
        Kind::Scale | Kind::ScaleOrMax => {
            let number: f32 = value
                .strip_suffix('x')
                .unwrap_or(value)
                .parse()
                .map_err(|_| format!("`{}` isn't a scale. Write 2 or 2x.", recto_core::clean_line(value)))?;
            if number.is_finite() && number > 0.0 {
                Ok(json!(number))
            } else {
                Err("A scale is more than 0.".to_string())
            }
        }
        Kind::Duration => duration_ms(value).map(|ms| json!(ms)),
        Kind::Fraction => match value.parse::<f64>() {
            Ok(share) if (0.0..=1.0).contains(&share) => Ok(json!(share)),
            _ => Err(format!("`{}` should be a number from 0 to 1, such as 0.3.", recto_core::clean_line(value))),
        },
        Kind::Degrees => {
            let degrees: i32 = value
                .strip_suffix("deg")
                .unwrap_or(value)
                .parse()
                .map_err(|_| format!("`{}` isn't degrees. Write 90, 180, 270 or -90.", recto_core::clean_line(value)))?;
            if degrees % 90 == 0 && degrees % 360 != 0 {
                Ok(json!(degrees))
            } else {
                Err("A rotation is 90, 180, 270 or -90 degrees.".to_string())
            }
        }
        Kind::Level => Level::parse(value)
            .map(|level| json!(level.name()))
            .ok_or_else(|| format!("`{}` isn't a level. They are off, read_only, control and full.", recto_core::clean_line(value))),
        Kind::Choice(words) => words
            .iter()
            .find(|word| **word == value)
            .map(|word| json!(word))
            .ok_or_else(|| format!("`{}` isn't one of: {}.", recto_core::clean_line(value), words.join(", "))),
        Kind::List(words) => {
            let mut chosen = Vec::new();
            for word in value.split(',') {
                let Some(known) = words.iter().find(|known| **known == word) else {
                    return Err(format!("`{}` isn't one of: {}.", recto_core::clean_line(word), words.join(", ")));
                };
                chosen.push(json!(known));
            }
            Ok(Value::Array(chosen))
        }
        Kind::Size => match value.split_once('x').map(|(width, height)| (whole_number(width), whole_number(height))) {
            Some((Some(_), Some(_))) => Ok(json!(value)),
            _ => Err(format!(
                "`{}` isn't a size. Write WIDTHxHEIGHT, such as 3024x1964.",
                recto_core::clean_line(value)
            )),
        },
        Kind::Rect => {
            let bad = || {
                format!(
                    "`{}` isn't a box. Write x,y,width,height in points, such as 72,72,300,200.",
                    recto_core::clean_line(value)
                )
            };
            let numbers = value.split(',').map(str::parse::<f32>).collect::<Result<Vec<_>, _>>().map_err(|_| bad())?;
            match numbers[..] {
                [x, y, width, height]
                    if [x, y, width, height].iter().all(|number| number.is_finite()) && x >= 0.0 && y >= 0.0 && width > 0.0 && height > 0.0 =>
                {
                    Ok(json!(numbers))
                }
                _ => Err(bad()),
            }
        }
        Kind::Switch | Kind::Rest => Err("That doesn't take a value.".to_string()),
    }
}

/// Digits only, so `+3`, `0x3` and `3.0` aren't numbers here.
fn whole_number(text: &str) -> Option<u32> {
    if text.is_empty() || !text.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    text.parse().ok()
}

/// A duration in milliseconds, from `500ms`, `30s`, `5m` or `1h`. Without a unit it's refused rather than guessed at.
pub fn duration_ms(text: &str) -> Result<u64, String> {
    let bad = || format!("`{}` isn't a time. Write 500ms, 30s, 5m or 1h.", recto_core::clean_line(text));
    let (digits, unit) = text.split_at(text.bytes().position(|byte| !byte.is_ascii_digit()).ok_or_else(bad)?);
    let number: u64 = digits.parse().map_err(|_| bad())?;
    let per = match unit {
        "ms" => 1,
        "s" => 1000,
        "m" => 60_000,
        "h" => 3_600_000,
        _ => return Err(bad()),
    };
    let ms = number.checked_mul(per).ok_or_else(bad)?;
    if ms > crate::MAX_WAIT_MS {
        Err("That's longer than the longest wait, 24h.".to_string())
    } else {
        Ok(ms)
    }
}

/// A row's usage line.
pub fn usage_line(spec: &Spec) -> String {
    let mut out = format!("recto {}", spec.name);
    for arg in spec.args {
        let shown = match (arg.flag, arg.kind) {
            (Some(flag), Kind::Switch) => flag.to_string(),
            (Some(flag), _) => format!("{flag} <{}>", arg.key.trim_end_matches("_ms").replace('_', "-")),
            (None, Kind::Rest) => format!("<{}...>", arg.key),
            (None, _) => format!("<{}>", arg.key),
        };
        if arg.required {
            out.push(' ');
            out.push_str(&shown);
        } else {
            out.push_str(&format!(" [{shown}]"));
        }
    }
    out
}

/// `recto help`: every command, by group.
pub fn help_text() -> String {
    let mut out = String::from(
        "recto: control RectoPDF from the command line.\n\nStart with `recto status`. `recto help <command>` explains one. Every command takes `--json`. Flags take `--flag value` or `--flag=value`. `recto --version` gives the version.\n",
    );
    let mut group = "";
    for spec in TABLE.iter().filter(|spec| !spec.hidden) {
        if spec.group != group {
            group = spec.group;
            out.push_str(&format!("\n{group}:\n"));
        }
        let name = format!("{:<20}", spec.name);
        out.push_str(&format!("  {name} {}\n", first_sentence(spec.summary)));
    }
    out.push_str("\nExit codes: 0 ok, 1 failed, 2 usage, 3 not found, 4 ambiguous, 5 permission denied, 6 not running, 7 timed out, 8 protected, 10 busy.\n");
    out
}

fn first_sentence(text: &str) -> &str {
    text.find(". ").map_or(text, |end| &text[..=end])
}

/// `recto help <command>`.
pub fn help_for(spec: &Spec) -> String {
    let needs = if spec.not_built {
        format!(
            "Needs: nothing yet. It isn't built, so it says so and does nothing, even at {}.",
            spec.access.label()
        )
    } else {
        format!("Needs: {}", spec.access.label())
    };
    let mut out = format!("{}\n\n{}\n\n{needs}\n", usage_line(spec), spec.summary);
    if !spec.args.is_empty() {
        out.push('\n');
        for arg in spec.args {
            let name = arg.flag.map_or_else(|| format!("<{}>", arg.key), str::to_string);
            out.push_str(&format!("  {name:<14} {}\n", arg.help));
        }
    }
    if !spec.data.is_empty() {
        out.push_str(&format!("\nWith --json, `data` is: {}\n", spec.data));
    }
    if !spec.examples.is_empty() {
        out.push_str("\nExamples:\n");
        for example in spec.examples {
            out.push_str(&format!("  {example}\n"));
        }
    }
    let related: Vec<String> = family_rows(spec.name).map(usage_name).collect();
    if !related.is_empty() {
        out.push_str(&format!("\nSee also: `{}`.\n", related.join("`, `")));
    }
    out
}

/// The table as JSON, for `recto help --json`.
pub fn table_json() -> Value {
    let rows: Vec<Value> = TABLE
        .iter()
        .filter(|spec| !spec.hidden)
        .map(|spec| {
            json!({
                "name": spec.name,
                "usage": usage_line(spec),
                "needs": spec.access.label(),
                "built": !spec.not_built,
                "level": match spec.access { Access::At(level) => Some(level.name()), _ => None },
                "summary": spec.summary,
                "data": spec.data,
                "examples": spec.examples,
                "args": spec.args.iter().map(|arg| json!({ "name": arg.flag.unwrap_or(arg.key), "required": arg.required, "help": arg.help })).collect::<Vec<_>>(),
            })
        })
        .collect();
    json!({ "api": crate::VERSION, "commands": rows, "exit_codes": exit_codes() })
}

/// What each exit code means, the one list.
pub fn exit_codes() -> Value {
    json!([
        { "code": exit::OK, "means": "It worked. A search that finds nothing is still 0, with what was searched." },
        { "code": exit::FAILED, "means": "It failed. `error.code` says how." },
        { "code": exit::USAGE, "means": "The command line was wrong." },
        { "code": exit::NOT_FOUND, "means": "No such tab, page or file." },
        { "code": exit::AMBIGUOUS, "means": "More than one tab matches. `error.candidates` lists them." },
        { "code": exit::PERMISSION, "means": "Access is too low. `error.ask` is the one command that asks for more." },
        { "code": exit::NOT_RUNNING, "means": "No session is running. `recto serve --background` starts one." },
        { "code": exit::TIMED_OUT, "means": "The time ran out." },
        { "code": exit::PROTECTED, "means": "The document needs a password, which only the user can give, in the app." },
        { "code": exit::BUSY, "means": "The session has too many connections or waiting clients for this right now, or it's stopping. Try again. `status` always gets through." },
    ])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::skill::shell_words;
    use crate::{Command, parse, skill};

    fn parse_line(line: &str) -> Result<Invocation, AgentError> {
        let words = shell_words(line);
        assert_eq!(words[0], "recto", "{line}");
        parse_cli(&words[1..])
    }

    /// Failure: a documented example doesn't run, which is the first thing an agent copies. Every example of every row must parse to that row, and a session row to a real command.
    #[test]
    fn every_example_parses_to_its_own_row() {
        for spec in TABLE.iter().filter(|spec| !spec.hidden) {
            assert!(!spec.examples.is_empty(), "{} has no example", spec.name);
            assert!(!spec.summary.is_empty() && !spec.data.is_empty(), "{}", spec.name);
            for example in spec.examples {
                let mut line = parse_line(example).unwrap_or_else(|error| panic!("{example}: {}", error.message));
                assert_eq!(line.spec.name, spec.name, "{example}");
                // What `recto` reads from stdin for this row, as it puts it in.
                if let Some(field) = stdin_secret_field(spec) {
                    line.fields.insert(field.into(), json!("from stdin"));
                }
                if spec.route == Route::Session {
                    let command = line.command().unwrap_or_else(|error| panic!("{example}: {}", error.message));
                    assert_eq!(command.tag(), spec.tag, "{example}");
                }
                if spec.name == "local" {
                    let inner = parse_cli(&line.rest).unwrap_or_else(|error| panic!("{example}: {}", error.message));
                    assert_eq!(inner.spec.route, Route::Session, "{example}");
                    inner.command().unwrap();
                }
            }
        }
    }

    /// A value each kind of argument takes.
    fn sample_of(kind: Kind) -> &'static str {
        match kind {
            Kind::Tab => "#1",
            Kind::Page | Kind::Count(_) | Kind::Positive(_) => "1",
            Kind::Pages | Kind::PagesOpen => "1-2",
            Kind::Path => "/d/out.png",
            Kind::Scale | Kind::ScaleOrMax => "2x",
            Kind::Duration => "5s",
            Kind::Fraction => "0.3",
            Kind::Degrees => "90",
            Kind::Level => "read_only",
            Kind::Text => "words",
            Kind::Choice(words) | Kind::List(words) => words[0],
            Kind::Size => "10x10",
            Kind::Rect => "1,2,3,4",
            Kind::Switch | Kind::Rest => "",
        }
    }

    /// The line a row makes from these of its arguments, each with its value, and its required ones with a sample, as `recto` sends it.
    fn line_of(spec: &Spec, given: &[(&Arg, &str)]) -> Result<String, AgentError> {
        let mut words: Vec<String> = spec.name.split(' ').map(str::to_string).collect();
        let required = spec
            .args
            .iter()
            .filter(|arg| arg.required && !given.iter().any(|(each, _)| each.key == arg.key));
        for (arg, value) in required.map(|arg| (arg, sample_of(arg.kind))).chain(given.iter().copied()) {
            words.extend(arg.flag.map(str::to_string));
            if !value.is_empty() {
                words.push(value.to_string());
            }
        }
        let mut parsed = parse_cli(&words).unwrap_or_else(|error| panic!("{words:?}: {}", error.message));
        // What's read from stdin rather than the line, as `recto` itself puts it in.
        if let Some(field) = stdin_secret_field(spec) {
            parsed.fields.insert(field.into(), json!("from stdin"));
        }
        let wire = parsed.wire();
        parse(&wire).map(|_| wire)
    }

    /// Failure: a row's flags and its command's fields drift apart, so a flag on the docs is refused by the socket as an unknown field; or a word a row
    /// offers is refused by its command's type (R2.5). Each argument of every row alone, with each word it offers, makes a command with no refusal at all
    /// (a switch that only goes with another is refused alone only for that); every argument at once does too, or is refused only for flags that can't go
    /// together.
    #[test]
    fn every_argument_of_every_row_reaches_the_command() {
        for spec in TABLE.iter().filter(|spec| spec.route == Route::Session && !spec.hidden) {
            for arg in spec.args {
                let values: Vec<&str> = match arg.kind {
                    Kind::Choice(words) | Kind::List(words) => words.to_vec(),
                    other => vec![sample_of(other)],
                };
                for value in values {
                    // A switch that only goes with another (`find --jump` needs a step) is refused alone for that, and only for that.
                    match line_of(spec, &[(arg, value)]) {
                        Err(error) if arg.kind == Kind::Switch && crate::commands::refused_for_its_combination(spec.tag, &error) => {}
                        Err(error) => panic!("{} --{} {value}: refused, {:?}: {}", spec.name, arg.key, error.code, error.message),
                        Ok(_) => {}
                    }
                }
            }
            let all: Vec<(&Arg, &str)> = spec.args.iter().map(|arg| (arg, sample_of(arg.kind))).collect();
            if let Err(error) = line_of(spec, &all) {
                assert!(
                    crate::commands::refused_for_its_combination(spec.tag, &error),
                    "{} with every argument: refused, {:?}: {}",
                    spec.name,
                    error.code,
                    error.message
                );
            }
        }
    }

    /// Failure: a typo is taken as something else, a flag is ignored, or one bad value turns into a default. Refused with the help to fix it, as usage, exit 2.
    #[test]
    fn a_wrong_line_is_refused_and_never_guessed_at() {
        for line in [
            "tabz",
            "page",
            "page turn 1",
            "tabs --nope",
            "tabs extra",
            "text",
            "text --page",
            "text --page 0",
            "text --page x",
            "text --page 1.5",
            "text --page +1",
            "goto",
            "page rotate 1",
            "page rotate 1 --by 45",
            "page rotate 4- --by 90",
            "page delete 4-x-",
            "search x --limit 0",
            "contents --limit 0",
            "tabs --limit 0",
            "log --last 0",
            "wait tab-open",
            "wait",
            "watch --events nope",
            "render --page 1",
            "permission request root",
            "open",
            "--level full",
            "search",
        ] {
            let args: Vec<String> = line.split(' ').map(str::to_string).collect();
            let error = parse_cli(&args).expect_err(line);
            assert_eq!((error.code.as_str(), error.exit), ("usage", exit::USAGE), "{line}");
            assert!(error.next.contains("recto"), "{line}: {}", error.next);
        }
    }

    /// Failure: a mistyped command gets no help towards the right one.
    #[test]
    fn a_near_miss_suggests_the_real_command() {
        let error = parse_cli(&["statuss".to_string()]).unwrap_err();
        assert!(error.candidates.contains(&"status".to_string()), "{:?}", error.candidates);
        let error = parse_cli(&["page".to_string(), "mov".to_string(), "1".to_string()]).unwrap_err();
        assert!(error.candidates.iter().any(|name| name.starts_with("page")), "{:?}", error.candidates);
    }

    /// Failure: `--json` works on some commands and not others, or a word after `--` is read as a flag, so a search can't look for `--force`.
    #[test]
    fn json_is_taken_anywhere_and_a_double_dash_ends_flags() {
        for spec in TABLE.iter().filter(|spec| !spec.hidden) {
            for example in spec.examples {
                let mut words = shell_words(example);
                words.remove(0);
                words.push("--json".to_string());
                assert!(parse_cli(&words).unwrap().json, "{example}");
            }
        }
        let line = parse_cli(&["search".to_string(), "--".to_string(), "--force".to_string()]).unwrap();
        assert_eq!(line.fields["query"], "--force");
    }

    /// Failure: a tool's first move, `recto --help`, says "Which command?", and the help a person asks for by group, or copies from the help's own list, isn't there. Every spelling of asking must work.
    #[test]
    fn every_way_of_asking_for_help_works() {
        let words = |line: &str| -> Vec<String> { line.split(' ').map(str::to_string).collect() };
        let list = parse_cli(&words("--help")).unwrap();
        assert_eq!((list.spec.tag, list.help, list.rest.len()), ("help", false, 0));
        assert_eq!(parse_cli(&words("-h")).unwrap().spec.tag, "help");
        for group in ["page", "wait", "permission", "skill"] {
            let line = parse_cli(&words(&format!("{group} --help"))).unwrap();
            assert_eq!((line.spec.tag, line.rest.as_slice()), ("help", [group.to_string()].as_slice()), "{group}");
            let help = family_help(&line.rest).unwrap();
            assert!(help.contains(&format!("recto {group} ")), "{help}");
        }
        for row in TABLE.iter().filter(|row| !row.hidden) {
            assert_eq!(
                find_row(&[row.name.to_string()]).unwrap().name,
                row.name,
                "a quoted name, as the list prints it"
            );
            let as_words: Vec<String> = row.name.split(' ').map(str::to_string).collect();
            assert_eq!(find_row(&as_words).unwrap().name, row.name);
            let mut asked = as_words.clone();
            asked.push("--help".to_string());
            let line = parse_cli(&asked).unwrap();
            assert!(line.help && line.spec.name == row.name, "{}", row.name);
        }
        assert!(family_help(&words("nope")).is_none());
        assert!(family_help(&words("page move")).is_none(), "that's a command");
        assert!(
            help_for(find_row(&words("budget")).unwrap()).contains("recto budget tabs"),
            "a command with others under it points at them"
        );
    }

    /// Failure: there's no `--version`, and only `recto version`, so the commonest check of a tool fails.
    #[test]
    fn the_version_flag_is_the_version_row() {
        for flag in ["--version", "-V"] {
            assert_eq!(parse_cli(&[flag.to_string()]).unwrap().spec.tag, "version");
        }
        assert!(parse_cli(&["tabs".to_string(), "--version".to_string()]).is_err(), "only on its own");
    }

    /// Failure: `--page=3` is refused where `--page 3` works, the most common way of writing a flag in other tools, so an agent loses a try on each.
    #[test]
    fn a_flag_can_be_written_with_an_equals_sign() {
        let line = |text: &str| parse_cli(&text.split(' ').map(str::to_string).collect::<Vec<_>>());
        assert_eq!(line("text --page=3").unwrap().fields["page"], 3);
        assert_eq!(line("text --tab=#2 --page=3").unwrap().fields["tab"], "#2");
        assert_eq!(
            line("render --page=1 --out=/d/a=b.png --scale=2x").unwrap().fields["out"],
            "/d/a=b.png",
            "only the first `=` splits it"
        );
        for wrong in ["text --page=", "text --page=x", "text --nope=1", "serve --background=yes"] {
            assert_eq!(line(wrong).unwrap_err().exit, exit::USAGE, "{wrong}");
        }
        assert_eq!(
            parse_cli(&["search".into(), "--".into(), "--tab=x".into()]).unwrap().fields["query"],
            "--tab=x",
            "after `--`, it's only words"
        );
    }

    /// Failure: a page number given in the wrong place is refused without saying what's wanted, so an agent has to read the help to find `--page`.
    #[test]
    fn a_stray_page_number_says_which_flag_it_wants() {
        let line = |text: &str| parse_cli(&text.split(' ').map(str::to_string).collect::<Vec<_>>());
        for (typed, wanted) in [
            ("wait page-drawn 3", "--page 3"),
            ("open /d/a.pdf 4", "--page 4"),
            ("search word 5-6", "--pages 5-6"),
            ("page insert /d/a.pdf 2", "--at 2"),
            ("page move 3 1", "--to 1"),
        ] {
            let error = line(typed).unwrap_err();
            assert_eq!(error.exit, exit::USAGE, "{typed}");
            assert!(error.message.contains(wanted), "{typed}: {}", error.message);
        }
        for typed in [
            "text 1-3",
            "render 1-3 --out /d/a.png",
            "wait page-drawn 1-3",
            "open /d/a.pdf 1-3",
            "page insert /d/a.pdf 1-3",
            "page move 3 1-3",
            "text 0",
        ] {
            let error = line(typed).unwrap_err();
            let advised = error.message.split('`').find(|piece| piece.starts_with("--"));
            assert!(advised.is_none(), "{typed} advises {advised:?}, which would be refused: {}", error.message);
        }
        assert!(line("search word 1-").unwrap_err().message.contains("--pages 1-"), "`--pages` can take that");
        assert!(!line("tabs 1").unwrap_err().message.contains("--page"), "it has none");
        assert!(!line("text word").unwrap_err().message.contains("--page word"), "that's no page");
        assert!(line("text 1-3").unwrap_err().message.contains("one page at a time"), "it says why");
        assert!(line("text 1-3").unwrap_err().candidates.is_empty() && line("render 1-3 --out /d/a.png").unwrap_err().candidates.is_empty());
        assert!(
            !line("budget render 5").unwrap_err().message.contains("--first"),
            "a measurement's number is a count"
        );
    }

    /// Failure: an agent types `text --pages 19` or `close #1` and is told only that it's wrong, so it loses a turn reading the help. Any unknown flag or stray word, on any row, gets the closest valid form, and that form must itself parse. Driven by the table, so a new command has this from the start.
    #[test]
    fn a_near_miss_gets_the_closest_form_that_works() {
        let line = |words: &[&str]| parse_cli(&words.iter().map(|word| (*word).to_string()).collect::<Vec<_>>());
        for (typed, wanted) in [
            (&["text", "--pages", "19"][..], "--page 19"),
            (&["search", "word", "--page", "3"][..], "--pages 3"),
            (&["text", "--page", "1", "--tabs", "#2"][..], "--tab \"#2\""),
            (&["contents", "--dept", "1"][..], "--depth 1"),
            (&["render", "--page", "1", "--output", "/d/a.png"][..], "--out /d/a.png"),
            (&["wait", "tab-open", "a", "--time", "5s"][..], "--timeout 5s"),
            (&["page", "rotate", "1", "--degrees", "90"][..], "--by 90"),
            (&["close", "#1"][..], "--tab \"#1\""),
            (&["where", "2"][..], "--tab \"#2\""),
            (&["facts", "report"][..], "--tab report"),
            (&["goto", "--page", "5"][..], "recto goto 5"),
            (&["search", "--query", "word"][..], "recto search word"),
        ] {
            let error = line(typed).unwrap_err();
            assert_eq!((error.code.as_str(), error.exit), ("usage", exit::USAGE), "{typed:?}");
            assert!(error.message.contains(wanted), "{typed:?} should say {wanted}: {}", error.message);
            assert!(error.next.contains("recto help"), "{typed:?}: {}", error.next);
        }
        // Advice that would be refused is worse than none.
        let range = line(&["text", "--pages", "1-3"]).unwrap_err();
        assert!(range.message.contains("`--page`") && !range.message.contains("--page 1-3"), "{}", range.message);
        assert_eq!(range.candidates, ["--page"], "only the flag, since a whole line would be refused");
        let far = line(&["tabs", "--nope", "1"]).unwrap_err();
        assert!(
            far.message.contains("`--from`, `--limit`") && far.candidates.is_empty(),
            "with nothing close, it says what there is: {}",
            far.message
        );
        assert!(
            !line(&["close", "--page", "3"]).unwrap_err().message.contains("--tab 3"),
            "`--page` isn't near `--tab`"
        );
        assert!(
            line(&["search", "two", "words"]).unwrap_err().message.contains("\"two words\""),
            "more words want quotes"
        );
        assert!(
            line(&["text", "report.pdf", "--page", "1"])
                .unwrap_err()
                .message
                .contains("recto open report.pdf")
        );
        for level_attempt in [&["tabs", "--level", "full"][..], &["tabs", "--force"][..], &["tabs", "--yes"][..]] {
            assert!(
                line(level_attempt).unwrap_err().candidates.is_empty(),
                "{level_attempt:?}: there's nothing to offer"
            );
        }
    }

    /// The fields a whole line of advice comes to, as if it were run.
    fn fields_of(advice: &str) -> Map<String, Value> {
        let words = shell_words(advice);
        assert_eq!(words[0], "recto", "{advice}");
        parse_cli(&words[1..])
            .unwrap_or_else(|refused| panic!("{advice} is refused: {}", refused.message))
            .fields
    }

    /// Failure: advice drops the rest of the line. `page delete --pages 3 --tab report` advised `recto page delete 3`, which parses, and deletes page 3 of the active tab, the wrong document, and `page rotate --pages 1 --by 90` advised `page rotate 1`, which is refused. Every flag and every value of every row, mistyped with all the others present: what's advised must parse, and mean what the right line means, field for field.
    #[test]
    fn advice_keeps_the_rest_of_the_line_and_means_the_same() {
        let advice = |line: &[&str]| {
            parse_cli(&line.iter().map(|word| (*word).to_string()).collect::<Vec<_>>())
                .unwrap_err()
                .candidates
                .first()
                .filter(|candidate| candidate.starts_with("recto "))
                .cloned()
        };
        assert_eq!(
            advice(&["page", "delete", "--pages", "3", "--tab", "report"]).as_deref(),
            Some("recto page delete 3 --tab report")
        );
        assert_eq!(
            advice(&["page", "rotate", "--pages", "1", "--by", "90"]).as_deref(),
            Some("recto page rotate 1 --by 90")
        );
        assert_eq!(
            advice(&["page", "rotate", "--page", "1", "--degrees", "90"]).as_deref(),
            Some("recto page rotate 1 --by 90"),
            "more than one wrong word on a line"
        );
        assert_eq!(
            advice(&["text", "--pages", "19", "--json"]).as_deref(),
            Some("recto text --page 19 --json"),
            "a line that wanted JSON still gets it"
        );
        assert_eq!(advice(&["search", "--", "--force", "stray"]), None, "past `--`, nothing can be moved");
        assert_eq!(
            advice(&["text", "--page", "1", "--pages", "2"]),
            None,
            "it can't have the page twice, and choosing would be a guess"
        );
        assert_eq!(
            advice(&["page", "delete", "3", "4"]),
            None,
            "that's not a tab, and a guess here deletes something"
        );
        let mut lines_checked = 0;
        for spec in TABLE.iter().filter(|spec| !spec.hidden && spec.args.iter().all(|arg| arg.kind != Kind::Rest)) {
            let value_of = |arg: &Arg| sample_of(arg.kind).to_string();
            let right: Vec<(Vec<String>, &Arg)> = spec
                .args
                .iter()
                .map(|arg| {
                    let mut words: Vec<String> = arg.flag.iter().map(|flag| (*flag).to_string()).collect();
                    if arg.kind != Kind::Switch {
                        words.push(value_of(arg));
                    }
                    (words, arg)
                })
                .collect();
            let name: Vec<String> = spec.name.split(' ').map(str::to_string).collect();
            let whole: Vec<String> = name.iter().cloned().chain(right.iter().flat_map(|(words, _)| words.iter().cloned())).collect();
            let meant = parse_cli(&whole).unwrap_or_else(|refused| panic!("{whole:?}: {}", refused.message)).fields;
            for (wrong, (_, arg)) in right.iter().enumerate() {
                let typo = arg.flag.map_or_else(|| format!("--{}", arg.key.replace('_', "-")), |flag| format!("{flag}x"));
                let mut typed = name.clone();
                for (at, (words, _)) in right.iter().enumerate() {
                    if at == wrong {
                        typed.push(typo.clone());
                        if arg.kind != Kind::Switch {
                            typed.push(value_of(arg));
                        }
                    } else {
                        typed.extend(words.iter().cloned());
                    }
                }
                let error = parse_cli(&typed).unwrap_err();
                assert!(error.next.contains(&usage_line(spec)), "{typed:?}: the usage line goes with it: {}", error.next);
                let advised = error.candidates.first().unwrap_or_else(|| panic!("{typed:?} got no advice: {}", error.message));
                assert_eq!(fields_of(advised), meant, "{typed:?} was told {advised}, which means something else");
                lines_checked += 1;
            }
        }
        assert!(lines_checked > 40, "{lines_checked}");
    }

    /// Runs what a shell would make of a line of advice, and returns the words it would pass on. `$(...)` and the like print a mark if they run, and the words are compared with what was typed, so a shell that did anything but pass them on fails this.
    fn as_a_shell_reads_it(line: &str) -> Vec<String> {
        let output = std::process::Command::new("/bin/sh")
            .args(["-c", &format!("set -- {line}; for word; do printf '%s\\0' \"$word\"; done")])
            .output()
            .unwrap();
        assert!(output.status.success(), "{line}: {output:?}");
        String::from_utf8(output.stdout).unwrap().split_terminator('\0').map(str::to_string).collect()
    }

    /// Failure: `--tabs 'R&D'` advises `--tab R&D`, which a shell runs as a background job, and a tab's name can come from a file: one named `$(...)` is a command an agent was told to run. Whatever comes back must reach `recto` as the same word, or not be offered at all.
    #[test]
    fn advice_quotes_what_a_shell_would_act_on() {
        for hostile in [
            "R&D",
            "a;echo pwned",
            "$(echo pwned)",
            "`echo pwned`",
            "a\"b",
            "a'b",
            "it's $(echo pwned)",
            "a|b",
            "a>b",
            "a b",
            "#1",
            "*",
            "~",
            "!x",
            "back\\slash",
            "(x)",
            "ca\u{308}fe",
        ] {
            let error = parse_cli(&["facts".to_string(), "--tabs".to_string(), hostile.to_string()]).unwrap_err();
            let advice = error.candidates.first().unwrap_or_else(|| panic!("{hostile} got no advice: {}", error.message));
            assert_eq!(as_a_shell_reads_it(advice), ["recto", "facts", "--tab", hostile], "{hostile}: {advice}");
            assert!(error.message.contains(advice.as_str()), "{}", error.message);
            assert_eq!(
                shell_words(advice),
                ["recto", "facts", "--tab", hostile],
                "{hostile}: the tests' own reading of a line agrees"
            );
        }
        for not_printable in ["a\nb", "a\u{202E}b", &"x".repeat(MAX_QUOTED_WORD + 1)] {
            let error = parse_cli(&["facts".to_string(), "--tabs".to_string(), not_printable.to_string()]).unwrap_err();
            assert_eq!(
                error.candidates,
                ["--tab"],
                "{not_printable:?} would have changed on the way, or broken the line, so only the flag is offered"
            );
            assert!(!error.message.contains('\n'), "{}", error.message);
        }
        let stray = parse_cli(&["facts".to_string(), "$(echo pwned)".to_string()]).unwrap_err();
        assert_eq!(as_a_shell_reads_it(&stray.candidates[0]), ["recto", "facts", "--tab", "$(echo pwned)"]);
        let file = parse_cli(&["facts".to_string(), "$(echo pwned).pdf".to_string()]).unwrap_err();
        assert!(file.message.contains("recto open '$(echo pwned).pdf'"), "{}", file.message);
        let quoted = parse_cli(&["search".to_string(), "a".to_string(), "$(echo pwned)".to_string()]).unwrap_err();
        assert!(quoted.message.contains("recto search 'a $(echo pwned)'"), "{}", quoted.message);
    }

    /// Failure: `recto doc` lists what happens to be spelled alike in table order, or nothing, rather than what's nearest, and a flag after the word (`recto doc --page 1`) loses even that.
    #[test]
    fn a_mistyped_command_lists_the_nearest_first() {
        let line = |words: &[&str]| parse_cli(&words.iter().map(|word| (*word).to_string()).collect::<Vec<_>>());
        let doc = line(&["doc"]).unwrap_err();
        assert!(
            doc.candidates.iter().any(|name| name == "facts"),
            "`facts` is about a document: {:?}",
            doc.candidates
        );
        assert!(doc.candidates.len() <= 4 && !doc.candidates.is_empty(), "{:?}", doc.candidates);
        assert_eq!(
            line(&["doc", "--page", "1"]).unwrap_err().candidates,
            doc.candidates,
            "what follows the word doesn't change what it's near"
        );
        assert_eq!(line(&["statuss"]).unwrap_err().candidates[0], "status", "the nearest comes first");
        assert_eq!(line(&["serach", "word"]).unwrap_err().candidates[0], "search");
        assert!(line(&["zzzzzzzz"]).unwrap_err().candidates.is_empty(), "nothing near isn't made up");
    }

    /// Failure: an agent starts `watch` expecting to hear what's already happened, or looks for a command that ends one, since nothing says it reports only from now on or how it ends.
    #[test]
    fn watch_says_what_it_reports_and_how_it_ends() {
        let help = help_for(TABLE.iter().find(|row| row.name == "watch").unwrap());
        for must in [
            "from now on",
            "nothing from before",
            "--timeout",
            "--count",
            "recto watch stop",
            "Killing the process",
        ] {
            assert!(help.contains(must), "{must}: {help}");
        }
        assert!(help_text().contains("from now on"), "and in the list, which shows only the first sentence");
    }

    /// Failure: `recto text 60` is refused, though three agents in three rounds tried it. It means `--page 60`, wherever it is on the line, and nothing else is taken for one: a range, a page given twice, no page at all, or a number on a row that doesn't say so.
    #[test]
    fn a_bare_page_number_means_its_page() {
        let line = |text: &str| parse_cli(&text.split(' ').map(str::to_string).collect::<Vec<_>>());
        for (typed, flagged) in [
            ("text 60", "text --page 60"),
            ("text 60 --tab #2", "text --page 60 --tab #2"),
            ("text --tab #2 60", "text --page 60 --tab #2"),
            ("render 60 --out /d/a.png", "render --page 60 --out /d/a.png"),
            ("render --out /d/a.png --scale 2 60", "render --page 60 --out /d/a.png --scale 2"),
        ] {
            assert_eq!(line(typed).unwrap().fields, line(flagged).unwrap().fields, "{typed}");
        }
        assert_eq!(line("text 0").unwrap_err().exit, exit::USAGE, "pages count from 1");
        assert!(line("text 0").unwrap_err().message.contains("count from 1"));
        for refused in [
            "text 1-3",
            "text 60 61",
            "text 60 --page 61",
            "text --page 61 60",
            "render 1-3 --out /d/a.png",
            "text x1",
        ] {
            assert_eq!(line(refused).unwrap_err().exit, exit::USAGE, "{refused}");
        }
        assert!(line("text 60 61").unwrap_err().message.contains("page already"));
        assert!(
            TABLE
                .iter()
                .filter(|spec| spec.bare_page)
                .all(|spec| spec.args.iter().any(|arg| arg.key == "page" && arg.kind == Kind::Page)),
            "it needs a page to fill"
        );
        assert_eq!(line("goto 60").unwrap().fields["page"], 60, "that row's own, and unchanged");
        assert!(line("wait page-drawn 3").is_err(), "it's only where the row says");
    }

    /// Failure: a command that isn't built says `Needs: Off`, which reads as though it works, and an agent believes its edits can be saved.
    #[test]
    fn help_says_when_a_command_isnt_built() {
        let unbuilt = Spec {
            name: "example",
            tag: "example",
            summary: "Not here yet.",
            access: Access::At(Level::Off),
            not_built: true,
            ..BASE
        };
        let help = help_for(&unbuilt);
        assert!(
            help.contains("isn't built") && !help.contains("Needs: Off") && !help.contains("Needs: Read-only"),
            "{help}"
        );
        assert!(help_for(TABLE.iter().find(|row| row.name == "tabs").unwrap()).contains("Needs: Read-only"));
        assert!(
            help_for(TABLE.iter().find(|row| row.name == "save").unwrap()).contains("Needs: Control"),
            "saving is built now"
        );
        let rows = table_json();
        for row in rows["commands"].as_array().unwrap() {
            let spec = TABLE.iter().find(|spec| spec.name == row["name"]).unwrap();
            assert_eq!(row["built"], !spec.not_built, "{}", spec.name);
        }
    }

    /// Failure: nothing tells an agent the plain form returns at once and to carry on, so it adds `--timeout` and blocks for minutes waiting for a person who isn't there.
    #[test]
    fn asking_for_access_says_when_to_wait() {
        let help = help_for(TABLE.iter().find(|row| row.name == "permission request").unwrap());
        assert!(help.contains("carry on") && help.contains("at the keyboard"), "{help}");
    }

    /// Failure: units read differently in different places, so `--timeout 30` means who knows what.
    #[test]
    fn units_are_read_one_way() {
        assert_eq!(duration_ms("500ms"), Ok(500));
        assert_eq!(duration_ms("30s"), Ok(30_000));
        assert_eq!(duration_ms("5m"), Ok(300_000));
        assert_eq!(duration_ms("1h"), Ok(3_600_000));
        for bad in ["30", "", "s", "-1s", "1.5s", "1d", "99999999999999h", "25h"] {
            assert!(duration_ms(bad).is_err(), "{bad}");
        }
        let scale = |text: &str| {
            parse_cli(&[
                "render".into(),
                "--page".into(),
                "1".into(),
                "--out".into(),
                "/d/a.png".into(),
                "--scale".into(),
                text.into(),
            ])
        };
        assert_eq!(scale("2x").unwrap().fields["scale"], 2.0);
        assert_eq!(scale("2").unwrap().fields["scale"], 2.0);
        assert!(scale("nanx").is_err() && scale("-1").is_err() && scale("inf").is_err());
        // Failure: `max` reads as a scale anywhere one's taken, or only on the command line, so the socket refuses it.
        let max = scale("max").unwrap();
        assert_eq!(max.fields["scale"], "max");
        assert!(
            matches!(max.command().unwrap(), Command::Read(crate::commands::Read::Render(crate::commands::export::Render { scale, .. })) if scale == crate::render::LARGEST_THAT_FITS)
        );
        assert!(
            parse_cli(&["budget".into(), "render".into(), "--count".into(), "1".into(), "--scale".into(), "max".into()]).is_err(),
            "a measurement goes by a scale it's given"
        );
        for wire in [
            r#"{"cmd":"render","page":1,"out":"/d/a.png","scale":"big"}"#,
            r#"{"cmd":"render","page":1,"out":"/d/a.png","scale":1e999}"#,
            r#"{"cmd":"render","page":1,"out":"/d/a.png","scale":1e300}"#,
        ] {
            assert!(parse(wire).is_err(), "{wire}: a number too big isn't `max`");
        }
    }

    /// Failure: a relative path goes to the session as it is, and is read from the session's folder rather than the one the agent is in.
    #[test]
    fn paths_are_made_whole_before_they_leave() {
        let line = parse_cli(&["open".to_string(), "some/report.pdf".to_string()]).unwrap();
        let path = std::path::PathBuf::from(line.fields["path"].as_str().unwrap());
        assert!(path.is_absolute() && path.ends_with("some/report.pdf"));
    }

    /// Failure: a command's help misses what it needs or how to use it.
    #[test]
    fn help_for_a_command_says_how_and_at_what_level() {
        for spec in TABLE.iter().filter(|spec| !spec.hidden) {
            let help = help_for(spec);
            assert!(help.contains(spec.access.label()), "{}", spec.name);
            assert!(help.contains(&format!("recto {}", spec.name)), "{}", spec.name);
            assert!(help_text().contains(spec.name), "{} isn't in the list", spec.name);
        }
    }

    fn fenced_commands(markdown: &str) -> Vec<String> {
        let mut inside = false;
        let mut lines = Vec::new();
        for line in markdown.lines() {
            if line.starts_with("```") {
                inside = line == "```sh" || (inside && line != "```");
                continue;
            }
            if inside {
                lines.push(line.to_string());
            }
        }
        lines
    }

    fn quoted_commands(markdown: &str) -> Vec<String> {
        markdown
            .split('`')
            .skip(1)
            .step_by(2)
            .filter(|piece| piece.starts_with("recto "))
            .map(str::to_string)
            .collect()
    }

    /// Failure: the skill tells an agent to run something that isn't there, which is a blind agent's first failure. Every `recto` line in either file must exist.
    #[test]
    fn every_documented_command_exists_and_every_example_parses() {
        let reference = skill::reference_md();
        let fenced = fenced_commands(&reference);
        // At least one for each command the reference is about. QA's rows aren't in it, as they're for whoever builds the app, and with enough of them the count against the whole table failed on two branches' worth of rows that had their examples.
        let documented = TABLE.iter().filter(|spec| !spec.hidden && spec.group != GROUP_QA).count();
        assert!(fenced.len() >= documented, "only {} examples for {documented} commands", fenced.len());
        for line in &fenced {
            parse_line(line).unwrap_or_else(|error| panic!("{line}: {}", error.message));
        }
        for file in [skill::skill_md(), reference] {
            for quoted in quoted_commands(&file) {
                // Usage lines hold `<placeholders>`, `[options]` and `...`. Everything up to them must still name a command.
                let plain: Vec<String> = shell_words(&quoted).into_iter().take_while(|word| !word.starts_with(['<', '['])).collect();
                let names: Vec<&str> = plain.iter().skip(1).map(String::as_str).collect();
                assert!(find_spec(&names).is_some(), "the docs mention a command that isn't there: {quoted}");
                // A name mentioned in a sentence may leave out what it needs. Anything else on the line must be right.
                if !quoted.contains(['<', '[', '.'])
                    && let Err(error) = parse_line(&quoted)
                {
                    assert!(error.message.contains("needs"), "{quoted}: {}", error.message);
                }
            }
        }
    }

    /// Failure: a row is left out of the reference, so an agent can't find a command that's there.
    #[test]
    fn every_row_is_documented() {
        let (skill_text, reference) = (skill::skill_md(), skill::reference_md());
        for spec in TABLE.iter().filter(|spec| !spec.hidden) {
            // Testing the app is for whoever builds it, and the user's agents' session wouldn't answer. `recto help` has it.
            if spec.group == GROUP_QA {
                assert!(!reference.contains(&format!("### `recto {}`", spec.name)), "{} is in REFERENCE.md", spec.name);
                continue;
            }
            assert!(reference.contains(&format!("### `recto {}`", spec.name)), "{} isn't in REFERENCE.md", spec.name);
            if [GROUP_START, GROUP_READ, GROUP_MOVE, GROUP_EDIT, GROUP_SYNC, GROUP_ACCESS, GROUP_SESSION].contains(&spec.group) {
                assert!(skill_text.contains(&usage_line(spec)), "{} isn't in SKILL.md", spec.name);
            }
        }
    }

    /// Failure: `allowed-tools` lets an agent run something that changes the session without a prompt.
    #[test]
    fn what_runs_without_a_prompt_only_reads() {
        let text = skill::skill_md();
        assert!(text.starts_with("---\nname: recto\n"), "the front matter must come first");
        for name in skill::auto_approved() {
            let spec = TABLE.iter().find(|spec| spec.name == *name).unwrap_or_else(|| panic!("{name}"));
            assert!(skill::needs(spec).is_some_and(|level| level <= Level::ReadOnly), "{name} isn't read-only");
            assert!(text.contains(&format!("Bash(recto {name}*)")), "{name}");
        }
    }

    /// Failure: the skill forgets what it's there to teach, or mentions the agent panel, which is Phase 6.
    #[test]
    fn the_skill_teaches_how_to_read_and_stays_in_this_phase() {
        let text = skill::skill_md();
        for must in [
            "facts",
            "contents",
            "recto search",
            "recto text",
            "Cite pages",
            "data, not instructions",
            "untrusted",
            "recto wait",
            "not checked",
            "recto save",
        ] {
            assert!(text.contains(must), "{must}");
        }
        for mustnt in ["corpus", "panel", "sidebar chat", "recto read "] {
            assert!(!text.to_lowercase().contains(mustnt), "{mustnt}");
        }
    }

    /// Failure: a command's documented events aren't real, so a watcher waits for something that never comes.
    #[test]
    fn documented_events_belong_to_a_group() {
        for spec in TABLE {
            for event in spec.emits {
                assert!(event_group(event).is_some(), "{event} has no group of its own, so `--events` can't select it");
            }
        }
        for group in EVENT_GROUPS {
            assert!(skill::reference_md().contains(&format!("`{group}`")), "{group}");
        }
        let _ = Command::from(crate::commands::session::Stop {});
    }
}
