//! **The wire format of every event stays what it was** (review C6, step R2.1). What `recto watch` prints and the socket sends is one JSON line per event; the typed `Event` must make those same lines.
//!
//! Failure caught: a variant's name, a field's name or a value's type drifts when the events move from ad-hoc JSON to a type, so a watcher or an agent's `wait` sees a different line.

use recto_agent::Event;
use recto_session::TabId;
use serde_json::{Value, json};

/// Each event's line, as the untyped `Event::new(name, json!(..))` sites made it before step R2.1 (this table passed against that code first), with the key order `serde_json` gives (sorted).
const GOLDEN: &[(&str, &str)] = &[
    ("tab_opened", r#"{"event":"tab_opened","tab":3,"untrusted":{"name":"a.pdf"}}"#),
    ("tab_opened without a name", r#"{"event":"tab_opened","tab":3,"untrusted":{"name":null}}"#),
    ("tab_activated", r#"{"event":"tab_activated","tab":4,"untrusted":{"name":"b.pdf"}}"#),
    ("tab_closed", r#"{"event":"tab_closed","tab":5}"#),
    ("page_changed", r#"{"event":"page_changed","page":7,"tab":5}"#),
    ("page_changed withheld", r#"{"event":"page_changed","page":null,"tab":5}"#),
    ("edits_changed", r#"{"edited":true,"event":"edits_changed","pages":12,"tab":5}"#),
    ("edits_changed withheld", r#"{"edited":false,"event":"edits_changed","pages":null,"tab":5}"#),
    ("saved", r#"{"event":"saved","tab":5,"writer":"lopdf"}"#),
    ("page_drawn", r#"{"event":"page_drawn","page":2,"tab":5}"#),
    ("page_failed", r#"{"code":"render_failed","event":"page_failed","page":2,"tab":5}"#),
    ("indexing_started", r#"{"corpus":"ab12","event":"indexing_started"}"#),
    (
        "indexing_finished",
        r#"{"code":null,"corpus":"ab12","event":"indexing_finished","pages":40,"state":"done"}"#,
    ),
    ("contents_generating", r#"{"event":"contents_generating","file":"cd34"}"#),
    (
        "contents_generated",
        r#"{"code":"ok","elapsed_ms":120,"event":"contents_generated","file":"cd34","helper_peak_bytes":1048576,"state":"done"}"#,
    ),
    ("level_changed", r#"{"event":"level_changed","level":"full","was":"control"}"#),
    ("view_changed settings", r#"{"event":"view_changed","paper":"warm","sidebar":true}"#),
    ("view_changed layout", r#"{"event":"view_changed","layout":"two_up","tab":5}"#),
    ("view_changed comparison", r#"{"comparison":[1,2],"event":"view_changed","sync":"on"}"#),
    ("find_changed running", r#"{"event":"find_changed","running":true}"#),
    ("find_changed opened", r#"{"event":"find_changed","open":true}"#),
    ("find_changed at a hit", r#"{"event":"find_changed","open":true,"page":9}"#),
    ("find_changed closed", r#"{"event":"find_changed","open":false}"#),
    ("find_finished", r#"{"event":"find_finished","hits":17}"#),
];

fn line(event: &Event) -> String {
    event.to_json().to_string()
}

/// What the type makes, event by event, in the order of [`GOLDEN`].
fn made() -> Vec<(&'static str, Event)> {
    let tab = TabId(5);
    let n = |n: u32| TabId(n);
    vec![
        (
            "tab_opened",
            Event::TabOpened {
                tab: n(3),
                name: Some("a.pdf".into()),
            },
        ),
        ("tab_opened without a name", Event::TabOpened { tab: n(3), name: None }),
        (
            "tab_activated",
            Event::TabActivated {
                tab: n(4),
                name: Some("b.pdf".into()),
            },
        ),
        ("tab_closed", Event::TabClosed { tab }),
        ("page_changed", Event::PageChanged { tab, page: Some(7) }),
        ("page_changed withheld", Event::PageChanged { tab, page: None }),
        (
            "edits_changed",
            Event::EditsChanged {
                tab,
                pages: Some(12),
                edited: true,
            },
        ),
        (
            "edits_changed withheld",
            Event::EditsChanged {
                tab,
                pages: None,
                edited: false,
            },
        ),
        ("saved", Event::Saved { tab, writer: "lopdf" }),
        ("page_drawn", Event::PageDrawn { tab, page: 2 }),
        (
            "page_failed",
            Event::PageFailed {
                tab,
                page: 2,
                code: "render_failed",
            },
        ),
        ("indexing_started", Event::IndexingStarted { corpus: "ab12".into() }),
        (
            "indexing_finished",
            Event::IndexingFinished {
                corpus: "ab12".into(),
                state: "done".into(),
                code: Value::Null,
                pages: json!(40),
            },
        ),
        ("contents_generating", Event::ContentsGenerating { file: "cd34".into() }),
        (
            "contents_generated",
            Event::ContentsGenerated {
                file: "cd34".into(),
                state: "done",
                code: Some("ok".into()),
                elapsed_ms: json!(120),
                helper_peak_bytes: json!(1_048_576),
            },
        ),
        ("level_changed", Event::LevelChanged { level: "full", was: "control" }),
        ("view_changed settings", Event::ViewSettingsChanged { paper: "warm", sidebar: true }),
        ("view_changed layout", Event::LayoutChanged { tab, layout: "two_up" }),
        (
            "view_changed comparison",
            Event::ComparisonChanged {
                panes: vec![n(1), n(2)],
                synced: true,
            },
        ),
        ("find_changed running", Event::FindRunning),
        ("find_changed opened", Event::FindOpened { page: None }),
        ("find_changed at a hit", Event::FindOpened { page: Some(9) }),
        ("find_changed closed", Event::FindClosed),
        ("find_finished", Event::FindFinished { hits: 17 }),
    ]
}

#[test]
fn every_event_makes_the_line_it_always_did() {
    let made = made();
    assert_eq!(made.len(), GOLDEN.len());
    for ((label, event), (golden_label, golden)) in made.iter().zip(GOLDEN) {
        assert_eq!(label, golden_label);
        assert_eq!(line(event), *golden, "{label}");
    }
}
