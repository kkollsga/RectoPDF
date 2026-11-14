//! Waiting instead of polling: `wait`, `watch` and `watch stop`. The server's own loop answers them, since it holds the connections that wait.

use super::{Cmd, Cx, needs_server, row_access};
use crate::TabRef;
use crate::human::{QUOTED_NAMES, count, number, tab_line};
use crate::table::*;
use crate::{AgentError, Condition, Runs, SessionAccess};
use serde::Deserialize;
use serde_json::Value;

/// Blocks until a condition holds, or the time runs out.
#[derive(Clone, Debug, Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub struct Wait {
    pub condition: Condition,
    #[serde(default)]
    pub tab: TabRef,
    #[serde(default)]
    pub page: Option<u32>,
    #[serde(default = "crate::default_wait_ms")]
    pub timeout_ms: u64,
}

impl Cmd for Wait {
    const TAG: &'static str = "wait";
    const ROWS: &'static [Spec] = ROWS_WAIT;
    const DRAWS: bool = false;
    const READS_PAGES: bool = false;
    const PROTECTED_OK: bool = false;
    const ABOUT: super::About = super::About::Its;
    const WAITS: bool = true;
    fn tab(&self) -> Option<&TabRef> {
        Some(&self.tab)
    }
    fn access(&self) -> Access {
        row_access(Self::TAG)
    }
    fn route(&self) -> Runs {
        Runs::Server
    }
    fn log_details(&self) -> Value {
        Value::Null
    }
    fn run<A: SessionAccess>(&self, _: &mut Cx<'_, A>) -> Result<Value, AgentError> {
        Err(needs_server())
    }
    fn human(data: &Value) -> String {
        if data["generated"] == true {
            crate::human::contents(data)
        } else if data["folder"].is_string() || data["files_ready"].is_u64() {
            crate::human::index(data)
        } else if data["closed"] == true {
            "That tab is closed.".to_string()
        } else if data["drawn"] == true {
            format!("Page {} is drawn.", number(&data["page"]))
        } else {
            format!("Open: {}\n{QUOTED_NAMES}", tab_line(data))
        }
    }
}

/// Streams events, one line each.
#[derive(Clone, Debug, Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub struct Watch {
    #[serde(default)]
    pub events: Option<Vec<String>>,
    #[serde(default)]
    pub timeout_ms: Option<u64>,
    #[serde(default)]
    pub count: Option<u32>,
}

impl Cmd for Watch {
    const TAG: &'static str = "watch";
    const ROWS: &'static [Spec] = ROWS_WATCH;
    const DRAWS: bool = false;
    const READS_PAGES: bool = false;
    const PROTECTED_OK: bool = false;
    const ABOUT: super::About = super::About::Nothing;
    const WAITS: bool = true;
    fn tab(&self) -> Option<&TabRef> {
        None
    }
    fn access(&self) -> Access {
        row_access(Self::TAG)
    }
    fn route(&self) -> Runs {
        Runs::Server
    }
    fn log_details(&self) -> Value {
        Value::Null
    }
    fn run<A: SessionAccess>(&self, _: &mut Cx<'_, A>) -> Result<Value, AgentError> {
        Err(needs_server())
    }
    fn human(data: &Value) -> String {
        crate::human::pretty(data)
    }
}

/// Ends every watch running on this session.
#[derive(Clone, Debug, Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub struct WatchStop {}

impl Cmd for WatchStop {
    const TAG: &'static str = "watch_stop";
    const ROWS: &'static [Spec] = ROWS_WATCH_STOP;
    const DRAWS: bool = false;
    const READS_PAGES: bool = false;
    const PROTECTED_OK: bool = false;
    const ABOUT: super::About = super::About::Nothing;
    const WAITS: bool = true;
    fn tab(&self) -> Option<&TabRef> {
        None
    }
    fn access(&self) -> Access {
        row_access(Self::TAG)
    }
    fn route(&self) -> Runs {
        Runs::Server
    }
    fn log_details(&self) -> Value {
        Value::Null
    }
    fn run<A: SessionAccess>(&self, _: &mut Cx<'_, A>) -> Result<Value, AgentError> {
        Err(needs_server())
    }
    fn human(data: &Value) -> String {
        format!("Ended {}.", count(number(&data["stopped"]), "watch", "watches"))
    }
}

pub(crate) const ROWS_WAIT: &[Spec] = &[
    Spec {
        name: "wait tab-open",
        tag: "wait",
        group: GROUP_SYNC,
        summary: "Blocks until a tab matching the selector is open. Exit 7 if the time runs out.",
        args: &[
            Arg {
                key: "tab",
                flag: None,
                kind: Kind::Tab,
                required: true,
                help: "Which tab: `#3`, or part of its name.",
            },
            TIMEOUT,
        ],
        fixed: &[("condition", "tab_open")],
        data: "`where`'s answer for that tab.",
        examples: &["recto wait tab-open report --timeout 10s"],
        ..BASE
    },
    Spec {
        name: "wait tab-closed",
        tag: "wait",
        group: GROUP_SYNC,
        summary: "Blocks until no tab matches the selector. Exit 7 if the time runs out.",
        args: &[
            Arg {
                key: "tab",
                flag: None,
                kind: Kind::Tab,
                required: true,
                help: "Which tab: `#3`, or part of its name.",
            },
            TIMEOUT,
        ],
        fixed: &[("condition", "tab_closed")],
        data: "`{ closed }`.",
        examples: &["recto wait tab-closed \"#2\" --timeout 10s"],
        ..BASE
    },
    Spec {
        name: "wait page-drawn",
        tag: "wait",
        group: GROUP_SYNC,
        summary: "Blocks until a page has been drawn (by `render`, or by `text`'s check), or failed to be. Exit 7 if the time runs out.",
        args: &[PAGE, TAB, TIMEOUT],
        fixed: &[("condition", "page_drawn")],
        data: "`{ tab, page, drawn }`.",
        examples: &["recto wait page-drawn --page 3 --timeout 1m"],
        ..BASE
    },
    Spec {
        name: "wait contents-generated",
        tag: "wait",
        group: GROUP_SYNC,
        summary: "Waits for the tab's generated contents to be made, asking for them if nobody has.",
        args: &[TAB, TIMEOUT],
        fixed: &[("condition", "contents_generated")],
        data: "What `contents --generated` gives, once it's ready. Exit 8 with `not_generated` if the file needs a password, exit 1 with `not_generated` or `generating_failed` if they won't or can't be made, and exit 7 if the time runs out.",
        examples: &["recto wait contents-generated", "recto wait contents-generated --tab #2 --timeout 2m"],
        ..BASE
    },
    Spec {
        name: "wait indexing-done",
        tag: "wait",
        group: GROUP_SYNC,
        summary: "Waits for the tab's search index to be finished, asking for it to be built if nobody has.",
        args: &[TAB, TIMEOUT],
        fixed: &[("condition", "indexing_done")],
        data: "What `index status` gives, once it says `ready`. Exit 8 with `not_indexed` if the file needs a password, whose text is never written to disk, exit 1 with `not_indexed` or `index_failed` if it won't or can't be indexed, and exit 7 if the time runs out.",
        examples: &["recto wait indexing-done", "recto wait indexing-done --tab #2 --timeout 5m --json"],
        ..BASE
    },
];

pub(crate) const ROWS_WATCH: &[Spec] = &[Spec {
    name: "watch",
    tag: "watch",
    group: GROUP_SYNC,
    summary: "Prints events from now on, one per line. It reports only what happens after it starts, and nothing from before: `recto tabs` and `recto edits` say how things stand. It ends when `--timeout` or `--count` runs out, if access drops below Read-only, if the session stops, or when `recto watch stop` is run. Killing the process ends it too, at once.",
    args: &[
        Arg {
            key: "events",
            flag: Some("--events"),
            kind: Kind::List(EVENT_GROUPS),
            required: false,
            help: "Which: `tabs`, `pages`, `edits`, `draws`, `indexing`, `contents` (generated contents being made), `access`, `view`, `find` (Find's hits coming in, and done), with commas between. All if left out.",
        },
        Arg {
            key: "timeout_ms",
            flag: Some("--timeout"),
            kind: Kind::Duration,
            required: false,
            help: "How long to watch: `30s`, `5m`. 10m if left out.",
        },
        Arg {
            key: "count",
            flag: Some("--count"),
            kind: Kind::Count(1_000_000),
            required: false,
            help: "Stop after this many events. Exit 7 if the time runs out first.",
        },
    ],
    data: "One `{ event, ... }` per line, after a first `{ watching, events }`, and last a `{ done, reason, delivered }`.",
    examples: &["recto watch --events tabs,pages --timeout 30s", "recto watch --count 1 --timeout 1m --json"],
    ..BASE
}];

pub(crate) const ROWS_WATCH_STOP: &[Spec] = &[Spec {
    name: "watch stop",
    tag: "watch_stop",
    group: GROUP_SYNC,
    summary: "Ends every watch running on this session, each with its last line as usual. For a watch left running in the background.",
    data: "`{ stopped }`: how many.",
    examples: &["recto watch stop"],
    client_wait_s: 5,
    ..BASE
}];
