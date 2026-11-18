//! A transient mark is observed in the callback that applied the action, before its timer can run. The ordinary report still describes the later, flushed state.

use serde_json::Value;

pub(super) fn report(verb: &str, delivered: Value, mut current: Value) -> Value {
    if matches!(verb, "scroll" | "zoom" | "corner_pointer") {
        if let (Some(chrome), Some(report)) = (delivered.get("chrome"), current.as_object_mut()) {
            report.insert("chrome_at_delivery".into(), chrome.clone());
        }
    }
    current
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// Failure: observing the second callback after a timer expired falsely says the action never showed its mark, or keeping that observation replaces the flushed report or exposes unrelated callback contents.
    #[test]
    fn transient_chrome_keeps_its_delivery_observation_and_the_current_report() {
        let applied = json!({"chrome": {"place_shown": true, "place_alpha": 1}, "unrelated": "first callback"});
        let current = json!({"chrome": {"place_shown": false, "place_alpha": 0}, "flushed": true});
        for verb in ["scroll", "zoom", "corner_pointer"] {
            let observed = report(verb, applied.clone(), current.clone());
            assert_eq!(observed["chrome_at_delivery"], applied["chrome"]);
            assert_eq!(observed["chrome"], current["chrome"]);
            assert_eq!(observed["flushed"], true);
            assert!(observed.get("unrelated").is_none());
        }
        assert_eq!(report("button", applied, current.clone()), current);
    }
}
