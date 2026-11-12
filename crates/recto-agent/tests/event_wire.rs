//! **The wire format of every event stays what it was** (review C6, step R2.1). What `recto watch` prints and the socket sends is one JSON line per event; the typed `Event` must make those same lines.
//!
//! Failure caught: a variant's name, a field's name or a value's type drifts when the events move from ad-hoc JSON to a type, so a watcher or an agent's `wait` sees a different line.

use serde_json::{Value, json};

/// Each event's line, as the old `Event::new(name, json!(..))` sites made it, with the key order `serde_json` gives (sorted).
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
    ("indexing_finished", r#"{"code":null,"corpus":"ab12","event":"indexing_finished","pages":40,"state":"done"}"#),
    ("contents_generating", r#"{"event":"contents_generating","file":"cd34"}"#),
    ("contents_generated", r#"{"code":"ok","elapsed_ms":120,"event":"contents_generated","file":"cd34","helper_peak_bytes":1048576,"state":"done"}"#),
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

fn line(event: &recto_agent::Event) -> String {
    event.to_json().to_string()
}

/// What the old code made, event by event.
fn old_way() -> Vec<(&'static str, recto_agent::Event)> {
    use recto_agent::Event;
    let tab = |n: u32| json!(n);
    vec![
        ("tab_opened", Event::new("tab_opened", json!({ "tab": tab(3), "untrusted": { "name": Some("a.pdf") } }))),
        ("tab_opened without a name", Event::new("tab_opened", json!({ "tab": tab(3), "untrusted": { "name": None::<String> } }))),
        ("tab_activated", Event::new("tab_activated", json!({ "tab": tab(4), "untrusted": { "name": Some("b.pdf") } }))),
        ("tab_closed", Event::new("tab_closed", json!({ "tab": tab(5) }))),
        ("page_changed", Event::new("page_changed", json!({ "tab": tab(5), "page": Some(7) }))),
        ("page_changed withheld", Event::new("page_changed", json!({ "tab": tab(5), "page": None::<u32> }))),
        ("edits_changed", Event::new("edits_changed", json!({ "tab": tab(5), "pages": Some(12u32), "edited": true }))),
        ("edits_changed withheld", Event::new("edits_changed", json!({ "tab": tab(5), "pages": None::<u32>, "edited": false }))),
        ("saved", Event::new("saved", json!({ "tab": tab(5), "writer": "lopdf" }))),
        ("page_drawn", Event::new("page_drawn", json!({ "tab": tab(5), "page": 2 }))),
        ("page_failed", Event::new("page_failed", json!({ "tab": tab(5), "page": 2, "code": "render_failed" }))),
        ("indexing_started", Event::new("indexing_started", json!({ "corpus": "ab12" }))),
        ("indexing_finished", Event::new("indexing_finished", json!({ "corpus": "ab12", "state": "done", "code": Value::Null, "pages": json!(40) }))),
        ("contents_generating", Event::new("contents_generating", json!({ "file": "cd34" }))),
        ("contents_generated", Event::new("contents_generated", json!({ "file": "cd34", "state": "done", "code": "ok", "elapsed_ms": json!(120), "helper_peak_bytes": json!(1048576) }))),
        ("level_changed", Event::new("level_changed", json!({ "level": "full", "was": "control" }))),
        ("view_changed settings", Event::new("view_changed", json!({ "paper": "warm", "sidebar": true }))),
        ("view_changed layout", Event::new("view_changed", json!({ "tab": tab(5), "layout": "two_up" }))),
        ("view_changed comparison", Event::new("view_changed", json!({ "comparison": [1, 2], "sync": "on" }))),
        ("find_changed running", Event::new("find_changed", json!({ "running": true }))),
        ("find_changed opened", Event::new("find_changed", json!({ "open": true }))),
        ("find_changed at a hit", Event::new("find_changed", json!({ "open": true, "page": 9 }))),
        ("find_changed closed", Event::new("find_changed", json!({ "open": false }))),
        ("find_finished", Event::new("find_finished", json!({ "hits": 17 }))),
    ]
}

#[test]
fn every_event_makes_the_line_it_always_did() {
    let made = old_way();
    assert_eq!(made.len(), GOLDEN.len());
    for ((label, event), (golden_label, golden)) in made.iter().zip(GOLDEN) {
        assert_eq!(label, golden_label);
        assert_eq!(line(event), *golden, "{label}");
    }
}
