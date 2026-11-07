//! **The one list of commands.** The CLI's grammar, `recto help`, the level checks, `SKILL.md` and `REFERENCE.md` all come from [`TABLE`], so nothing documented can be missing and nothing can be
//! left out of the checks. Tests walk it: every command has a row, every row runs, every example parses.

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
}

const BASE: Spec = Spec {
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
};

const TAB: Arg = Arg { key: "tab", flag: Some("--tab"), kind: Kind::Tab, required: false, help: "Which tab: `#3`, `active`, or part of its name. The active tab if left out." };
const PAGE: Arg = Arg { key: "page", flag: Some("--page"), kind: Kind::Page, required: true, help: "The page, counted from 1." };
const SCALE: Arg = Arg { key: "scale", flag: Some("--scale"), kind: Kind::Scale, required: false, help: "How large: `2` or `2x` is twice the page's size. 2x if left out." };
const TIMEOUT: Arg = Arg { key: "timeout_ms", flag: Some("--timeout"), kind: Kind::Duration, required: false, help: "How long to wait: `500ms`, `30s`, `5m`. 30s if left out." };
const FIRST: Arg = Arg { key: "first", flag: Some("--first"), kind: Kind::Page, required: false, help: "The page to start from. The first page if left out." };

pub const GROUP_START: &str = "Start here";
pub const GROUP_READ: &str = "Read a document";
pub const GROUP_MOVE: &str = "Open and move around";
pub const GROUP_EDIT: &str = "Change pages (in memory: saving isn't built yet)";
pub const GROUP_SYNC: &str = "Wait instead of polling";
pub const GROUP_ACCESS: &str = "Access and history";
pub const GROUP_SESSION: &str = "The session";
pub const GROUP_SKILL: &str = "Skill files";
pub const GROUP_MEASURE: &str = "Measuring";

const AGENT_EXITS_PAGE: &str = "`{ tab, sel, page, pages, active, edited, structure, problem, note, untrusted: { name } }`.";
const EDIT_STATE: &str = "`{ tab, pages, file_pages, page, edited, saved, can_undo, can_redo, note, page_was_removed, place_note }`. The tab stays on the page it was reading, wherever that goes. If that page was removed, `page_was_removed` says so and it's on the nearest one.";
const STATUS_DATA: &str = "`{ running, pid, version, api, level, uptime_ms, socket, tabs }`. `tabs` needs Read-only.";

/// Every command, in the order `recto help` shows them.
pub static TABLE: &[Spec] = &[
    Spec {
        name: "status",
        tag: "status",
        group: GROUP_START,
        access: Access::At(Level::Off),
        summary: "Whether the session is running, and what access agents have.",
        data: STATUS_DATA,
        examples: &["recto status", "recto status --json"],
        client_wait_s: 5,
        ..BASE
    },
    Spec { name: "help", tag: "help", group: GROUP_START, access: Access::At(Level::Off), route: Route::Here, summary: "This list, or everything about one command.", args: &[Arg { key: "command", flag: None, kind: Kind::Rest, required: false, help: "The command to explain, such as `page move`." }], data: "With `--json`: the command table itself.", examples: &["recto help", "recto help page move", "recto help --json"], ..BASE },
    Spec { name: "version", tag: "version", group: GROUP_START, access: Access::At(Level::Off), route: Route::Here, summary: "This program's version, and the API version it speaks.", data: "`{ version, api }`.", examples: &["recto version --json"], ..BASE },
    Spec {
        name: "tabs",
        tag: "tabs",
        group: GROUP_READ,
        summary: "The open tabs, with each one's page, length and unsaved edits.",
        args: &[Arg { key: "from", flag: Some("--from"), kind: Kind::Count(100_000), required: false, help: "Tabs to skip: `next_from` from the last answer." }, Arg { key: "limit", flag: Some("--limit"), kind: Kind::Positive(1000), required: false, help: "Tabs in this answer. 200 if left out." }],
        data: "`{ tabs: [ ...same as where ], count, next_from }`.",
        examples: &["recto tabs", "recto tabs --from 200 --json"],
        ..BASE
    },
    Spec { name: "where", tag: "where", group: GROUP_READ, summary: "Where one tab is: its page, its length, and whether it has unsaved edits.", args: &[TAB], data: AGENT_EXITS_PAGE, examples: &["recto where", "recto where --tab \"#2\""], ..BASE },
    Spec {
        name: "facts",
        tag: "facts",
        group: GROUP_READ,
        summary: "A document's facts: pages, size, version, and whether it has contents.",
        args: &[TAB],
        data: "`{ tab, pages, file_pages, file_bytes, encrypted, encryption_note, pdf_version, structure, has_contents, contents_entries, edited, note, untrusted: { name, title } }`. `encryption_note` says when an encrypted file opened with no password.",
        examples: &["recto facts", "recto facts --tab report --json"],
        ..BASE
    },
    Spec {
        name: "contents",
        tag: "contents",
        group: GROUP_READ,
        summary: "A document's table of contents, a piece at a time. By default the document's sections and as far down as fits, and it says which depth that is.",
        args: &[
            TAB,
            Arg { key: "from", flag: Some("--from"), kind: Kind::Count(100_000), required: false, help: "Entries to skip: `next_from` from the last answer." },
            Arg { key: "limit", flag: Some("--limit"), kind: Kind::Positive(2000), required: false, help: "Entries in this answer. 200 if left out." },
            Arg { key: "depth", flag: Some("--depth"), kind: Kind::Count(32), required: false, help: "Deepest nesting to include. 0 is the top level, which can be just the title. As much as fits if left out." },
        ],
        data: "`{ tab, available, total, all_entries, entries: [ { depth, page, untrusted: { title } } ], next_from, truncated, depth_shown, deepest, sections_at, depth_rule }`. `total` counts the entries at the depth shown, and `all_entries` every entry. `sections_at` is the first depth whose entries are on more than one page, and `depth_rule` says all this in words. `available: false` means it couldn't be read, not that there's none. `truncated: true` means the file's contents stopped being read at a limit.",
        examples: &["recto contents", "recto contents --depth 1 --limit 50", "recto contents --from 200 --json"],
        ..BASE
    },
    Spec {
        name: "search",
        tag: "search",
        group: GROUP_READ,
        summary: "Finds words across a document. Ignores case and line breaks. Finding nothing is still exit 0, with what was searched.",
        args: &[
            Arg { key: "query", flag: None, kind: Kind::Text, required: true, help: "The words to find." },
            TAB,
            Arg { key: "pages", flag: Some("--pages"), kind: Kind::PagesOpen, required: false, help: "Only these pages: `4`, `4-6`, or `4-` to the end." },
            Arg { key: "limit", flag: Some("--limit"), kind: Kind::Positive(200), required: false, help: "Most hits in this answer. 20 if left out." },
        ],
        data: "`{ tab, hits: [ { page, source_page, from_added_file, line, also_spelled, page_has_tag_chars, hidden_text: { checked, flagged }, overlaps_hidden, untrusted: { snippet, added_file } } ], hidden_text_checked, coverage: { pages_searched, pages_in_scope, pages_with_no_text, no_text_pages, failed_pages, hits_not_shown: [ { page, hits } ], pages_with_tag_chars, changed_files: [ { untrusted: { file } } ], next_page } }`. `source_page` and `from_added_file` say where a hit's page really comes from, once it's been edited. `overlaps_hidden: true` means the words matched include text no eye can see, and `null` that it can't be said. It searches the same text `text` gives. A page in `failed_pages` couldn't be read, or its file changed since its number was recorded (`changed_files`). Only pages with hits are checked for hidden text, as `hidden_text_checked` says. `also_spelled: true` means the hit is another spelling of a word broken across lines, with no line of its own. `next_page` is where to go on from with `--pages`.",
        examples: &["recto search \"force majeure\"", "recto search termination --pages 10-40 --limit 5 --json"],
        client_wait_s: 180,
        ..BASE
    },
    Spec {
        name: "text",
        tag: "text",
        group: GROUP_READ,
        summary: "One page's text, normalised for reading and search, with a flag for text no eye can see. Words a file draws out of order are put back on their lines.",
        args: &[PAGE, TAB],
        data: "`{ page, source_page, from_added_file, chars, hyphens_joined, hyphens_kept, invisible_chars_removed, tag_chars_removed, runs_moved, hidden_text: { checked, flagged, chars }, hidden_spans: [ [start, end] ], hidden_spans_dropped, untrusted: { text, also_spelled, hidden_runs, added_file } }`. `runs_moved` is how many runs drawn late were put back on their lines. `hidden_spans` says where flagged hidden text is in `text`, counted in characters, end not included. In plain output, a line holding any starts with `! `, and it's inside `[hidden: ...]`. `source_page` and `from_added_file` say where the page really comes from, once it's been edited. `hidden_text.checked: false` means it wasn't checked, not that there's none.",
        emits: &["page_drawn", "page_failed"],
        examples: &["recto text --page 3", "recto text --page 3 --tab \"#2\" --json"],
        ..BASE
    },
    Spec {
        name: "render",
        tag: "render",
        group: GROUP_READ,
        access: Access::At(Level::Full),
        summary: "Draws a page to a new PNG file, sharp or not at all. It never replaces a file, and doesn't show an unsaved rotation.",
        args: &[PAGE, Arg { key: "out", flag: Some("--out"), kind: Kind::Path, required: true, help: "A new file ending in `.png`." }, SCALE, TAB],
        data: "`{ page, source_page, from_added_file, scale, out, took_ms, width, height, bytes, kind, unsaved_rotation_degrees, untrusted: { added_file } }`.",
        emits: &["page_drawn", "page_failed"],
        examples: &["recto render --page 1 --out page1.png", "recto render --page 2 --scale 3x --out /tmp/p2.png --json"],
        client_wait_s: 600,
        ..BASE
    },
    Spec {
        name: "signatures",
        tag: "signatures",
        group: GROUP_READ,
        summary: "What the file says about signatures. Facts only: nothing is checked, and nothing here can sign.",
        args: &[TAB],
        data: "`{ tab, structure_read, count, signed, truncated, unread, partial, edited, certification_named, signatures: [ { summary, status, page, file_page, byte_range, bytes_after_range, untrusted: { field, filter, sub_filter, stated_name, stated_reason, stated_location, stated_time } } ], note }`. `status` is `not_checked`, `empty` or `unread`. `structure_read: false` or `partial: true` means there may be more than this.",
        examples: &["recto signatures", "recto signatures --tab contract --json"],
        ..BASE
    },
    Spec { name: "sidebar", tag: "sidebar", group: GROUP_READ, summary: "The sidebar's rows, as the app would draw them.", data: "`{ rows: [ { kind, tab, page, depth, current, untrusted: { text } } ], rail_filled, toc_open, place, edited }`.", examples: &["recto sidebar --json"], ..BASE },
    Spec {
        name: "open",
        tag: "open",
        group: GROUP_MOVE,
        access: Access::At(Level::Control),
        summary: "Opens a PDF as the active tab, or goes to its tab if it's already open. A `--page` it doesn't have opens nothing.",
        args: &[Arg { key: "path", flag: None, kind: Kind::Path, required: true, help: "The PDF." }, Arg { key: "page", flag: Some("--page"), kind: Kind::Page, required: false, help: "The page to land on, counted from 1." }],
        data: AGENT_EXITS_PAGE,
        emits: &["tab_opened", "tab_activated", "page_changed"],
        examples: &["recto open report.pdf", "recto open /Users/me/report.pdf --page 12 --json"],
        ..BASE
    },
    Spec { name: "goto", tag: "goto", group: GROUP_MOVE, access: Access::At(Level::Control), summary: "Goes to a page. A page that isn't there is refused.", args: &[Arg { key: "page", flag: None, kind: Kind::Page, required: true, help: "The page, counted from 1." }, TAB], data: AGENT_EXITS_PAGE, emits: &["page_changed"], examples: &["recto goto 12", "recto goto 12 --tab report"], ..BASE },
    Spec { name: "tap", tag: "tap", group: GROUP_MOVE, access: Access::At(Level::Control), summary: "Taps a tab in the sidebar: switches to it, or folds its contents if it's already active.", args: &[Arg { key: "tab", flag: None, kind: Kind::Tab, required: true, help: "Which tab: `#3`, or part of its name." }], data: "The sidebar, as `sidebar` gives it.", emits: &["tab_activated"], examples: &["recto tap \"#2\"", "recto tap report"], ..BASE },
    Spec { name: "close", tag: "close", group: GROUP_MOVE, access: Access::At(Level::Control), summary: "Closes a tab. Refused while it has unsaved edits, so `recto discard` first, and below Full if undone steps of the user's would go. Undone steps of an agent's own go with it. The refusal says what's at stake and each way on.", args: &[TAB], data: "`{ closed, count, active }`, with `active` as `where` gives it.", emits: &["tab_closed", "tab_activated"], examples: &["recto close", "recto close --tab \"#2\""], ..BASE },
    Spec {
        name: "page move",
        tag: "page_move",
        group: GROUP_EDIT,
        access: Access::At(Level::Control),
        summary: "Moves pages, so the first of them ends up at `--to`.",
        args: &[Arg { key: "pages", flag: None, kind: Kind::Pages, required: true, help: "Which: `4` or `4-6`." }, Arg { key: "to", flag: Some("--to"), kind: Kind::Page, required: true, help: "Where the first of them lands, counted from 1 in the new order." }, TAB],
        data: EDIT_STATE,
        emits: &["edits_changed"],
        examples: &["recto page move 3 --to 1", "recto page move 4-6 --to 10 --tab report"],
        ..BASE
    },
    Spec {
        name: "page rotate",
        tag: "page_rotate",
        group: GROUP_EDIT,
        access: Access::At(Level::Control),
        summary: "Turns pages clockwise.",
        args: &[Arg { key: "pages", flag: None, kind: Kind::Pages, required: true, help: "Which: `4` or `4-6`." }, Arg { key: "degrees", flag: Some("--by"), kind: Kind::Degrees, required: true, help: "`90`, `180`, `270`, or `-90` for the other way." }, TAB],
        data: EDIT_STATE,
        emits: &["edits_changed"],
        examples: &["recto page rotate 2 --by 90", "recto page rotate 1-4 --by -90 --json"],
        ..BASE
    },
    Spec { name: "page delete", tag: "page_delete", group: GROUP_EDIT, access: Access::At(Level::Control), summary: "Removes pages from the edited document. A document keeps at least one page. Below Full, an edit that would clear undone steps of the user's is refused.", args: &[Arg { key: "pages", flag: None, kind: Kind::Pages, required: true, help: "Which: `4` or `4-6`." }, TAB], data: EDIT_STATE, emits: &["edits_changed"], examples: &["recto page delete 5", "recto page delete 2-3 --tab report"], ..BASE },
    Spec {
        name: "page insert",
        tag: "page_insert",
        group: GROUP_EDIT,
        access: Access::At(Level::Control),
        summary: "Adds every page of another PDF.",
        args: &[Arg { key: "path", flag: None, kind: Kind::Path, required: true, help: "The PDF to add." }, Arg { key: "at", flag: Some("--at"), kind: Kind::Page, required: true, help: "Where its first page lands, counted from 1. One past the last page adds to the end." }, TAB],
        data: EDIT_STATE,
        emits: &["edits_changed"],
        examples: &["recto page insert appendix.pdf --at 4"],
        ..BASE
    },
    Spec { name: "undo", tag: "undo", group: GROUP_EDIT, access: Access::At(Level::Control), summary: "Reverses the last page edit. It can be redone until a new edit clears it. Below Full, a new edit may clear only agents' undone steps: if the user's are among them it's refused until you've redone them.", args: &[TAB], data: EDIT_STATE, emits: &["edits_changed"], examples: &["recto undo"], ..BASE },
    Spec { name: "redo", tag: "redo", group: GROUP_EDIT, access: Access::At(Level::Control), summary: "Repeats the last undone page edit.", args: &[TAB], data: EDIT_STATE, emits: &["edits_changed"], examples: &["recto redo"], ..BASE },
    Spec { name: "edits", tag: "edits", group: GROUP_EDIT, summary: "A tab's unsaved edits, as runs of pages.", args: &[TAB], data: "`{ ...same as undo, runs: [ { pages, from_file_pages, rotation_degrees, added, untrusted: { added_file } } ], runs_truncated }`.", examples: &["recto edits", "recto edits --json"], ..BASE },
    Spec { name: "discard", tag: "discard", group: GROUP_EDIT, access: Access::At(Level::Control), summary: "Throws away a tab's edits, undone ones included. They can't be got back. Below Full it's refused, and throws nothing away, if any are the user's.", args: &[TAB], data: EDIT_STATE, emits: &["edits_changed"], examples: &["recto discard --tab report"], ..BASE },
    Spec { name: "save", tag: "save", group: GROUP_EDIT, access: Access::At(Level::Off), summary: "Saving isn't built yet. This says so and writes nothing.", args: &[TAB], data: "Always refuses with `not_built`.", examples: &["recto save"], not_built: true, ..BASE },
    Spec {
        name: "wait tab-open",
        tag: "wait",
        group: GROUP_SYNC,
        summary: "Blocks until a tab matching the selector is open. Exit 7 if the time runs out.",
        args: &[Arg { key: "tab", flag: None, kind: Kind::Tab, required: true, help: "Which tab: `#3`, or part of its name." }, TIMEOUT],
        fixed: &[("condition", "tab_open")],
        data: "`where`'s answer for that tab.",
        examples: &["recto wait tab-open report --timeout 10s"],
        ..BASE
    },
    Spec { name: "wait tab-closed", tag: "wait", group: GROUP_SYNC, summary: "Blocks until no tab matches the selector. Exit 7 if the time runs out.", args: &[Arg { key: "tab", flag: None, kind: Kind::Tab, required: true, help: "Which tab: `#3`, or part of its name." }, TIMEOUT], fixed: &[("condition", "tab_closed")], data: "`{ closed }`.", examples: &["recto wait tab-closed \"#2\" --timeout 10s"], ..BASE },
    Spec { name: "wait page-drawn", tag: "wait", group: GROUP_SYNC, summary: "Blocks until a page has been drawn (by `render`, or by `text`'s check), or failed to be. Exit 7 if the time runs out.", args: &[PAGE, TAB, TIMEOUT], fixed: &[("condition", "page_drawn")], data: "`{ tab, page, drawn }`.", examples: &["recto wait page-drawn --page 3 --timeout 1m"], ..BASE },
    Spec { name: "wait indexing-done", tag: "wait", group: GROUP_SYNC, summary: "The search index isn't built yet, so this says so at once rather than waiting.", args: &[TAB, TIMEOUT], fixed: &[("condition", "indexing_done")], data: "Always refuses with `not_built`.", examples: &["recto wait indexing-done"], not_built: true, ..BASE },
    Spec {
        name: "watch",
        tag: "watch",
        group: GROUP_SYNC,
        summary: "Prints events as they happen, one per line, until the time or count runs out. Stops if access drops below Read-only.",
        args: &[
            Arg { key: "events", flag: Some("--events"), kind: Kind::List(EVENT_GROUPS), required: false, help: "Which: `tabs`, `pages`, `edits`, `draws`, `access`, with commas between. All if left out." },
            Arg { key: "timeout_ms", flag: Some("--timeout"), kind: Kind::Duration, required: false, help: "How long to watch: `30s`, `5m`. 10m if left out." },
            Arg { key: "count", flag: Some("--count"), kind: Kind::Count(1_000_000), required: false, help: "Stop after this many events. Exit 7 if the time runs out first." },
        ],
        data: "One `{ event, ... }` per line, after a first `{ watching, events }`, and last a `{ done, reason, delivered }`.",
        examples: &["recto watch --events tabs,pages --timeout 30s", "recto watch --count 1 --timeout 1m --json"],
        ..BASE
    },
    Spec {
        name: "permission show",
        tag: "permission_show",
        group: GROUP_ACCESS,
        access: Access::At(Level::Off),
        summary: "What access agents have now, and what each level allows.",
        data: "`{ level, label, levels: [ { level, label, allows } ], how_to_change, set_at, pending_request }`.",
        examples: &["recto permission show", "recto permission show --json"],
        client_wait_s: 5,
        ..BASE
    },
    Spec {
        name: "permission request",
        tag: "permission_request",
        group: GROUP_ACCESS,
        access: Access::At(Level::Off),
        summary: "Asks the user for more access. It grants nothing and answers at once: tell the user what it prints, and carry on with what you can. Use `--timeout` only when someone's at the keyboard to answer, since it blocks until they do.",
        args: &[Arg { key: "level", flag: None, kind: Kind::Level, required: true, help: "`read_only`, `control` or `full`." }, Arg { key: "timeout_ms", flag: Some("--timeout"), kind: Kind::Duration, required: false, help: "Only when someone's at the keyboard to answer: how long to block waiting for them, such as `30s` or `5m`. Answers at once if left out, which is nearly always what you want." }],
        data: "`{ level, granted, asked, tell_the_user, command, what_to_do }`.",
        examples: &["recto permission request read_only", "recto permission request control --timeout 5m"],
        ..BASE
    },
    Spec {
        name: "permission set",
        tag: "permission_set",
        group: GROUP_ACCESS,
        access: Access::UserOnly,
        route: Route::Here,
        summary: "For the user: sets what agents may do. Raising it needs a real terminal and a word typed back. No flag, variable or message can do it. Later the app's settings do this.",
        args: &[Arg { key: "level", flag: None, kind: Kind::Level, required: true, help: "`off`, `read_only`, `control` or `full`." }],
        data: "`{ level, label, was }`.",
        examples: &["recto permission set read_only", "recto permission set off"],
        ..BASE
    },
    Spec {
        name: "log",
        tag: "log",
        group: GROUP_ACCESS,
        summary: "What agents have done: time, command, level and outcome, oldest first. Never document text. At a real terminal it needs no session and no level.",
        args: &[Arg { key: "last", flag: Some("--last"), kind: Kind::Positive(5000), required: false, help: "How many entries, from the newest. 50 if left out." }],
        data: "`{ entries: [ { time, t_ms, cmd, level, outcome, details, count, since } ], kept_bytes, file }`. `count` means that many more of the same since `since`: `status`, bad lines and refusals are counted rather than written each time, so a flood of them can't push out what happened.",
        examples: &["recto log", "recto log --last 10 --json"],
        ..BASE
    },
    Spec {
        name: "sign",
        tag: "sign",
        group: GROUP_ACCESS,
        access: Access::Nobody,
        summary: "Signing isn't open to agents at any level. Only the user can sign, in the app. This always refuses, and `signatures` is what reports them.",
        data: "Always refuses with `sign_refused`, exit 5.",
        examples: &["recto sign"],
        ..BASE
    },
    Spec {
        name: "serve",
        tag: "serve",
        group: GROUP_SESSION,
        access: Access::At(Level::Off),
        route: Route::Here,
        summary: "Runs the session, on a socket only this user can reach, with no network listener. Tabs last until it stops.",
        args: &[Arg { key: "background", flag: Some("--background"), kind: Kind::Switch, required: false, help: "Starts it and returns once it's answering. Does nothing if it's already running." }],
        data: "`{ running, pid, socket, already }`.",
        examples: &["recto serve --background", "recto serve"],
        ..BASE
    },
    Spec { name: "stop", tag: "stop", group: GROUP_SESSION, access: Access::At(Level::Control), summary: "Ends the session. Refused while a tab has unsaved or undone edits.", data: "`{ stopping }`.", examples: &["recto stop"], client_wait_s: 15, ..BASE },
    Spec {
        name: "local",
        tag: "local",
        group: GROUP_SESSION,
        access: Access::At(Level::Off),
        route: Route::Here,
        summary: "For tests: runs one command in a throwaway session in this process, with one file open. The same commands, levels and log. Tabs vanish on exit.",
        args: &[Arg { key: "file", flag: None, kind: Kind::Path, required: true, help: "The PDF to open first. Opening it needs Control." }, Arg { key: "command", flag: None, kind: Kind::Rest, required: true, help: "The command to run on it." }],
        data: "The command's answer.",
        examples: &["recto local report.pdf text --page 1 --json", "recto local report.pdf budget render --count 5"],
        ..BASE
    },
    Spec { name: "session", tag: "session", group: GROUP_SESSION, access: Access::At(Level::Off), route: Route::Here, summary: "For QA scripts: a throwaway session in this process, with one JSON command per line on stdin and one reply per line. The same levels and log.", data: "One reply per line.", examples: &["recto session"], ..BASE },
    Spec { name: "skill show", tag: "skill_show", group: GROUP_SKILL, access: Access::At(Level::Off), route: Route::Here, summary: "Prints the generated skill.", args: &[Arg { key: "file", flag: None, kind: Kind::Choice(&["SKILL.md", "REFERENCE.md"]), required: false, help: "`SKILL.md` or `REFERENCE.md`. `SKILL.md` if left out." }], data: "The file's text.", examples: &["recto skill show", "recto skill show REFERENCE.md"], ..BASE },
    Spec {
        name: "skill install",
        tag: "skill_install",
        group: GROUP_SKILL,
        access: Access::UserOnly,
        route: Route::Here,
        summary: "For the user: writes SKILL.md and REFERENCE.md into their agent's files, after a word typed back at a real terminal. Refuses to replace a file someone changed, and says so.",
        args: &[
            Arg { key: "path", flag: Some("--path"), kind: Kind::Path, required: false, help: "The folder to write into." },
            Arg { key: "target", flag: Some("--target"), kind: Kind::Choice(&["claude-code"]), required: false, help: "`claude-code` is Claude Code's own place for skills, `~/.claude/skills/recto/`." },
            Arg { key: "replace", flag: Some("--replace"), kind: Kind::Switch, required: false, help: "Replace files even if someone changed them. There's no backup." },
            Arg { key: "dry_run", flag: Some("--dry-run"), kind: Kind::Switch, required: false, help: "Says what it would write, and writes nothing." },
        ],
        data: "`{ folder, files: [ { path, action } ], dry_run }`.",
        examples: &["recto skill install --target claude-code --dry-run", "recto skill install --path ./skills/recto"],
        ..BASE
    },
    Spec { name: "budget", tag: "budget", group: GROUP_MEASURE, summary: "The session's own footprint, tabs and handles right now.", data: "`{ tabs, footprint_bytes, open_handles }`.", examples: &["recto budget --json"], ..BASE },
    Spec {
        name: "budget tabs",
        tag: "budget_tabs",
        group: GROUP_MEASURE,
        access: Access::At(Level::Control),
        route: Route::Here,
        summary: "Opens one PDF as many tabs, through links in a temp folder, and reports memory and handles. Through the session, or in this process with `--in-process`.",
        args: &[
            Arg { key: "file", flag: Some("--file"), kind: Kind::Path, required: true, help: "The PDF. It's only read." },
            Arg { key: "count", flag: Some("--count"), kind: Kind::Count(10_000), required: true, help: "How many tabs." },
            Arg { key: "in_process", flag: Some("--in-process"), kind: Kind::Switch, required: false, help: "Measure this process, with no session." },
        ],
        data: "`{ route, tabs, file_bytes, start_footprint_bytes, samples, start_handles, end_handles, ms_per_open }`.",
        examples: &["recto budget tabs --file big.pdf --count 1000"],
        client_wait_s: 1800,
        ..BASE
    },
    Spec {
        name: "budget render",
        tag: "budget_render",
        group: GROUP_MEASURE,
        summary: "Measures a worker on a tab's document: idle, pages, peak and retirement. Choosing limits needs Full.",
        args: &[
            Arg { key: "pages", flag: Some("--count"), kind: Kind::Count(1_000_000), required: true, help: "How many pages." },
            FIRST,
            SCALE,
            Arg { key: "fresh", flag: Some("--fresh"), kind: Kind::Switch, required: false, help: "A fresh worker per page." },
            Arg { key: "tiles", flag: Some("--tiles"), kind: Kind::Switch, required: false, help: "512 px tiles, not whole pages." },
            Arg { key: "samples", flag: Some("--samples"), kind: Kind::Switch, required: false, help: "Each page's numbers too." },
            Arg { key: "limit_mib", flag: Some("--limit-mib"), kind: Kind::Count(1_000_000), required: false, help: "The memory limit, in MiB. Needs Full." },
            Arg { key: "retire_mib", flag: Some("--retire-mib"), kind: Kind::Count(1_000_000), required: false, help: "The retirement footprint, in MiB. Needs Full." },
            TAB,
        ],
        data: "Footprints in `*_bytes`, kills, tiles, and `ms: { p50, p95, max }`.",
        examples: &["recto budget render --count 200 --samples", "recto budget render --count 5 --limit-mib 512 --tab \"#1\""],
        client_wait_s: 1800,
        ..BASE
    },
    Spec {
        name: "budget viewport",
        tag: "budget_viewport",
        group: GROUP_MEASURE,
        summary: "Measures a reader's window: first view, one scroll step, one zoom step.",
        args: &[
            Arg { key: "viewport", flag: Some("--viewport"), kind: Kind::Size, required: true, help: "`WIDTHxHEIGHT` in device pixels, such as `3024x1964`." },
            FIRST,
            SCALE,
            Arg { key: "strategy", flag: Some("--strategy"), kind: Kind::Choice(&["per_tile", "region"]), required: false, help: "`per_tile` or `region`. Picked by page if left out." },
            TAB,
        ],
        data: "`{ strategy, largest_image_pixels, viewport, scale, steps: [ { step, ms, tiles, passes, blank, grey, colour, low_res, failed, peak_footprint_bytes } ], spawns, retirements, host_footprint_bytes }`.",
        examples: &["recto budget viewport --viewport 3024x1964", "recto budget viewport --viewport 1600x1000 --strategy region --first 2"],
        client_wait_s: 600,
        ..BASE
    },
    Spec { name: "level changed", tag: "level_changed", group: GROUP_ACCESS, access: Access::At(Level::Off), summary: "Tells the session to read the level's file again. Carries no level.", data: "`{ level }`.", client_wait_s: 5, hidden: true, ..BASE },
];

/// What `watch --events` can ask for.
pub const EVENT_GROUPS: &[&str] = &["tabs", "pages", "edits", "draws", "access"];

/// The group an event belongs to.
pub fn event_group(name: &str) -> &'static str {
    match name {
        "tab_opened" | "tab_closed" | "tab_activated" => "tabs",
        "page_changed" => "pages",
        "edits_changed" => "edits",
        "page_drawn" | "page_failed" => "draws",
        _ => "access",
    }
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
    AgentError::new("usage", message, exit::USAGE).then(next)
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
    let (fields, rest) = read_args(spec, &kept[words..], literal_from.saturating_sub(words), help_flag)?;
    Ok(Invocation { spec, fields, json: json_flag, help: help_flag, rest })
}

/// A line that means one of the rows that runs here, however it was typed.
fn here_row(tag: &str, json: bool, rest: Vec<String>) -> Invocation {
    let spec = by_tag(tag).unwrap_or(&TABLE[0]);
    Invocation { spec, fields: Map::new(), json, help: false, rest }
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
    TABLE.iter().filter(move |spec| !spec.hidden && spec.name.split_once(' ').is_some_and(|(group, _)| group == first))
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
        return usage_error(format!("`{typed}` needs one more word."), format!("Use `recto {}`. `recto help {typed}` lists them.", names.join("`, `recto "))).with_candidates(names);
    }
    let candidates: Vec<String> = TABLE
        .iter()
        .filter(|spec| !spec.hidden)
        .map(|spec| (spec.name, distance(spec.name, &typed).min(if spec.name.starts_with(first) { 0 } else { usize::MAX })))
        .filter(|(_, distance)| *distance <= 3)
        .map(|(name, _)| name.to_string())
        .take(6)
        .collect();
    let next = match candidates.as_slice() {
        [] => "`recto help` lists every command.".to_string(),
        some => format!("Perhaps `recto {}`. `recto help` lists every command.", some.join("`, `recto ")),
    };
    usage_error(format!("`{typed}` isn't a command."), next).with_candidates(candidates)
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

/// `literal_from` is where `--` was: from there on, nothing is a flag, so a search can look for `--force`.
fn read_args(spec: &'static Spec, words: &[&str], literal_from: usize, help: bool) -> Result<(Map<String, Value>, Vec<String>), AgentError> {
    let wrong = |message: String| usage_error(message, format!("`recto help {}` shows how to use it.", spec.name));
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
                return Err(wrong(format!("`recto {}` has no `{}`.", spec.name, recto_core::clean_line(flag))));
            };
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
                if let Some(flag) = page_flag_for(spec, word) {
                    let shown = recto_core::clean_line(word);
                    return Err(usage_error(format!("`recto {}` takes that as `{flag} {shown}`.", spec.name), format!("`recto help {}` shows how to use it.", spec.name)));
                }
                return Err(wrong(format!("`recto {}` doesn't take `{}` here.", spec.name, recto_core::clean_line(word))));
            };
            if arg.kind == Kind::Rest {
                rest = words[index - 1..].iter().map(|word| (*word).to_string()).collect();
                break;
            }
            fields.insert(arg.key.into(), read_value(arg, word).map_err(wrong)?);
        }
    }
    if !help && let Some(missing) = spec.args.iter().find(|arg| arg.required && arg.kind != Kind::Rest && !fields.contains_key(arg.key)) {
        let shown = missing.flag.map_or_else(|| format!("<{}>", missing.key), str::to_string);
        return Err(wrong(format!("`recto {}` needs {shown}.", spec.name)));
    }
    if !help && spec.args.iter().any(|arg| arg.required && arg.kind == Kind::Rest) && rest.is_empty() {
        return Err(wrong(format!("`recto {}` needs a command to run.", spec.name)));
    }
    Ok((fields, rest))
}

/// The flag a stray page number was probably meant for: `recto text 1` for `--page 1`. Measurements count pages rather than name them, so they're left out.
fn page_flag_for(spec: &Spec, word: &str) -> Option<&'static str> {
    if spec.group == GROUP_MEASURE {
        return None;
    }
    // Advice it would then refuse is worse than none: `--page` takes one page, and `--pages` can take a range.
    spec.args.iter().filter(|arg| arg.flag.is_some()).find_map(|arg| match arg.kind {
        Kind::Page if whole_number(word).is_some_and(|page| page > 0) => arg.flag,
        Kind::Pages if crate::parse_pages(word, false).is_ok() => arg.flag,
        Kind::PagesOpen if crate::parse_pages(word, true).is_ok() => arg.flag,
        _ => None,
    })
}

fn read_value(arg: &Arg, value: &str) -> Result<Value, String> {
    match arg.kind {
        Kind::Tab | Kind::Text => Ok(json!(value)),
        Kind::Page => match whole_number(value) {
            Some(0) | None => Err(format!("`{}` isn't a page. Pages count from 1.", recto_core::clean_line(value))),
            Some(page) => Ok(json!(page)),
        },
        Kind::Pages => crate::parse_pages(value, false).map(|_| json!(value)).map_err(|_| format!("`{}` isn't pages. Write 4 or 4-6.", recto_core::clean_line(value))),
        Kind::PagesOpen => crate::parse_pages(value, true).map(|_| json!(value)).map_err(|_| format!("`{}` isn't pages. Write 4, 4-6 or 4-.", recto_core::clean_line(value))),
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
        Kind::Scale => {
            let number: f32 = value.strip_suffix('x').unwrap_or(value).parse().map_err(|_| format!("`{}` isn't a scale. Write 2 or 2x.", recto_core::clean_line(value)))?;
            if number.is_finite() && number > 0.0 { Ok(json!(number)) } else { Err("A scale is more than 0.".to_string()) }
        }
        Kind::Duration => duration_ms(value).map(|ms| json!(ms)),
        Kind::Degrees => {
            let degrees: i32 = value.strip_suffix("deg").unwrap_or(value).parse().map_err(|_| format!("`{}` isn't degrees. Write 90, 180, 270 or -90.", recto_core::clean_line(value)))?;
            if degrees % 90 == 0 && degrees % 360 != 0 { Ok(json!(degrees)) } else { Err("A rotation is 90, 180, 270 or -90 degrees.".to_string()) }
        }
        Kind::Level => Level::parse(value).map(|level| json!(level.name())).ok_or_else(|| format!("`{}` isn't a level. They are off, read_only, control and full.", recto_core::clean_line(value))),
        Kind::Choice(words) => words.iter().find(|word| **word == value).map(|word| json!(word)).ok_or_else(|| format!("`{}` isn't one of: {}.", recto_core::clean_line(value), words.join(", "))),
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
            _ => Err(format!("`{}` isn't a size. Write WIDTHxHEIGHT, such as 3024x1964.", recto_core::clean_line(value))),
        },
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
    if ms > crate::MAX_WAIT_MS { Err("That's longer than the longest wait, 24h.".to_string()) } else { Ok(ms) }
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
    let mut out = String::from("recto: control RectoPDF from the command line.\n\nStart with `recto status`. `recto help <command>` explains one. Every command takes `--json`. Flags take `--flag value` or `--flag=value`. `recto --version` gives the version.\n");
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
    let needs = if spec.not_built { format!("Needs: nothing yet. It isn't built, so it says so and does nothing, even at {}.", spec.access.label()) } else { format!("Needs: {}", spec.access.label()) };
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
                let line = parse_line(example).unwrap_or_else(|error| panic!("{example}: {}", error.message));
                assert_eq!(line.spec.name, spec.name, "{example}");
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

    /// Failure: a row's flags and its command's fields drift apart, so a flag on the docs is refused by the socket as an unknown field. Fills every argument of every row and parses the result.
    #[test]
    fn every_argument_of_every_row_reaches_the_command() {
        for spec in TABLE.iter().filter(|spec| spec.route == Route::Session && !spec.hidden) {
            let mut line = vec![];
            line.extend(spec.name.split(' ').map(str::to_string));
            for arg in spec.args {
                if let Some(flag) = arg.flag {
                    line.push(flag.to_string());
                }
                let sample = match arg.kind {
                    Kind::Tab => "#1",
                    Kind::Page | Kind::Count(_) | Kind::Positive(_) => "1",
                    Kind::Pages | Kind::PagesOpen => "1-2",
                    Kind::Path => "/d/out.png",
                    Kind::Scale => "2x",
                    Kind::Duration => "5s",
                    Kind::Degrees => "90",
                    Kind::Level => "read_only",
                    Kind::Text => "words",
                    Kind::Choice(words) | Kind::List(words) => words[0],
                    Kind::Size => "10x10",
                    Kind::Switch | Kind::Rest => "",
                };
                if !sample.is_empty() {
                    line.push(sample.to_string());
                }
            }
            let parsed = parse_cli(&line).unwrap_or_else(|error| panic!("{line:?}: {}", error.message));
            parse(&parsed.wire()).unwrap_or_else(|_| panic!("{} sends a field its command doesn't know: {}", spec.name, parsed.wire()));
        }
    }

    /// Failure: a typo is taken as something else, a flag is ignored, or one bad value turns into a default. Refused with the help to fix it, as usage, exit 2.
    #[test]
    fn a_wrong_line_is_refused_and_never_guessed_at() {
        for line in ["tabz", "page", "page turn 1", "tabs --nope", "tabs extra", "text", "text --page", "text --page 0", "text --page x", "text --page 1.5", "text --page +1", "goto", "page rotate 1", "page rotate 1 --by 45", "page rotate 4- --by 90", "page delete 4-x-", "search x --limit 0", "contents --limit 0", "tabs --limit 0", "log --last 0", "wait tab-open", "wait", "watch --events nope", "render --page 1", "permission request root", "open", "--level full", "search"] {
            let args: Vec<String> = line.split(' ').map(str::to_string).collect();
            let error = parse_cli(&args).expect_err(line);
            assert_eq!((error.code, error.exit), ("usage", exit::USAGE), "{line}");
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
            assert_eq!(find_row(&[row.name.to_string()]).unwrap().name, row.name, "a quoted name, as the list prints it");
            let as_words: Vec<String> = row.name.split(' ').map(str::to_string).collect();
            assert_eq!(find_row(&as_words).unwrap().name, row.name);
            let mut asked = as_words.clone();
            asked.push("--help".to_string());
            let line = parse_cli(&asked).unwrap();
            assert!(line.help && line.spec.name == row.name, "{}", row.name);
        }
        assert!(family_help(&words("nope")).is_none());
        assert!(family_help(&words("page move")).is_none(), "that's a command");
        assert!(help_for(find_row(&words("budget")).unwrap()).contains("recto budget tabs"), "a command with others under it points at them");
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
        assert_eq!(line("render --page=1 --out=/d/a=b.png --scale=2x").unwrap().fields["out"], "/d/a=b.png", "only the first `=` splits it");
        for wrong in ["text --page=", "text --page=x", "text --nope=1", "serve --background=yes"] {
            assert_eq!(line(wrong).unwrap_err().exit, exit::USAGE, "{wrong}");
        }
        assert_eq!(parse_cli(&["search".into(), "--".into(), "--tab=x".into()]).unwrap().fields["query"], "--tab=x", "after `--`, it's only words");
    }

    /// Failure: a page number given in the wrong place is refused without saying what's wanted, so an agent has to read the help to find `--page`.
    #[test]
    fn a_stray_page_number_says_which_flag_it_wants() {
        let line = |text: &str| parse_cli(&text.split(' ').map(str::to_string).collect::<Vec<_>>());
        for (typed, wanted) in [("text 1", "--page 1"), ("render 2 --out /d/a.png", "--page 2"), ("wait page-drawn 3", "--page 3"), ("open /d/a.pdf 4", "--page 4"), ("search word 5-6", "--pages 5-6"), ("page insert /d/a.pdf 2", "--at 2"), ("page move 3 1", "--to 1")] {
            let error = line(typed).unwrap_err();
            assert_eq!(error.exit, exit::USAGE, "{typed}");
            assert!(error.message.contains(wanted), "{typed}: {}", error.message);
        }
        for typed in ["text 1-3", "render 1-3 --out /d/a.png", "wait page-drawn 1-3", "open /d/a.pdf 1-3", "page insert /d/a.pdf 1-3", "page move 3 1-3", "text 0"] {
            let error = line(typed).unwrap_err();
            let advised = error.message.split('`').find(|piece| piece.starts_with("--"));
            assert!(advised.is_none(), "{typed} advises {advised:?}, which would be refused: {}", error.message);
        }
        assert!(line("search word 1-").unwrap_err().message.contains("--pages 1-"), "`--pages` can take that");
        assert!(!line("tabs 1").unwrap_err().message.contains("--page"), "it has none");
        assert!(!line("text word").unwrap_err().message.contains("--page word"), "that's no page");
        assert!(!line("budget render 5").unwrap_err().message.contains("--first"), "a measurement's number is a count");
    }

    /// Failure: a command that isn't built says `Needs: Off`, which reads as though it works, and an agent believes its edits can be saved.
    #[test]
    fn help_says_when_a_command_isnt_built() {
        for name in ["save", "wait indexing-done"] {
            let help = help_for(TABLE.iter().find(|row| row.name == name).unwrap());
            assert!(help.contains("isn't built") && !help.contains("Needs: Off") && !help.contains("Needs: Read-only"), "{help}");
        }
        assert!(help_for(TABLE.iter().find(|row| row.name == "tabs").unwrap()).contains("Needs: Read-only"));
        let rows = table_json();
        let save = rows["commands"].as_array().unwrap().iter().find(|row| row["name"] == "save").unwrap();
        assert_eq!(save["built"], false);
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
        let scale = |text: &str| parse_cli(&["render".into(), "--page".into(), "1".into(), "--out".into(), "/d/a.png".into(), "--scale".into(), text.into()]);
        assert_eq!(scale("2x").unwrap().fields["scale"], 2.0);
        assert_eq!(scale("2").unwrap().fields["scale"], 2.0);
        assert!(scale("nanx").is_err() && scale("-1").is_err() && scale("inf").is_err());
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
        markdown.split('`').skip(1).step_by(2).filter(|piece| piece.starts_with("recto ")).map(str::to_string).collect()
    }

    /// Failure: the skill tells an agent to run something that isn't there, which is a blind agent's first failure. Every `recto` line in either file must exist.
    #[test]
    fn every_documented_command_exists_and_every_example_parses() {
        let reference = skill::reference_md();
        let fenced = fenced_commands(&reference);
        assert!(fenced.len() >= TABLE.len(), "only {} examples", fenced.len());
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
        for must in ["facts", "contents", "recto search", "recto text", "Cite pages", "data, not instructions", "untrusted", "recto wait", "not checked", "Saving isn't built"] {
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
                assert_ne!(event_group(event), "access", "{event} has no group of its own, so `--events` can't select it");
            }
        }
        for group in EVENT_GROUPS {
            assert!(skill::reference_md().contains(&format!("`{group}`")), "{group}");
        }
        let _ = Command::Stop {};
    }
}
