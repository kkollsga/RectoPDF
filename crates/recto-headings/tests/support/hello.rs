//! The helper's first line is its hello (`{"state":"hello","walled":...}`), said before it reads its job. Shared by the test files that read the helper's output, so they skip it the same way.

use serde_json::Value;

/// What the helper said after its hello: the hello is skipped by what it says (a first line whose `state` is `hello`), not by its place. Without one, all of it.
#[allow(dead_code)]
pub fn after_hello(said: &str) -> &str {
    match said.split_once('\n') {
        Some((first, rest)) if serde_json::from_str::<Value>(first).is_ok_and(|line| line["state"] == "hello") => rest,
        _ => said,
    }
}
