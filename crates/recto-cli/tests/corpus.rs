//! Optional runs over a real file, read-only. Set `RECTO_BENCH100` to the folder; without it these pass having done nothing, which they say on stderr. `examples/line_order_corpus.rs` in `recto-agent` is the run over every page.

mod common;

use common::State;
use recto_agent::Level;

/// Failure: RFC 9110, the case this repair exists for, reads `A recipient ignore`, the opposite of what it says, or a search for the real words misses them.
#[test]
fn rfc_9110_reads_as_it_is_printed() {
    let Some(folder) = std::env::var_os("RECTO_BENCH100") else {
        eprintln!("RECTO_BENCH100 isn't set, so no corpus files were read.");
        return;
    };
    let file = std::path::PathBuf::from(folder).join("standards_ietf_rfc9110.pdf");
    let state = State::at(Level::Control);
    let _serving = state.serve();
    state.json(&["open", file.to_str().unwrap()]);
    let (read, code) = state.json(&["text", "--page", "110"]);
    assert_eq!(code, 0, "{read}");
    let text = read["data"]["untrusted"]["text"].as_str().unwrap();
    assert!(text.contains("A recipient MUST ignore"), "{text}");
    assert!(text.contains("MUST NOT perform the requested"), "{text}");
    let (found, code) = state.json(&["search", "recipient MUST ignore", "--pages", "110"]);
    assert_eq!(code, 0, "{found}");
    assert!(!found["data"]["hits"].as_array().unwrap().is_empty(), "{found}");
}

/// Failure: a neighbouring table cell is pulled into a line, so `utilizing at least` becomes `utilizing at least utilizing at`, in a table whose gutter is as wide as its word spaces. It's a column landing in another, the one thing this must never do.
#[test]
fn a_real_tables_cells_stay_whole() {
    let Some(folder) = std::env::var_os("RECTO_BENCH100") else {
        eprintln!("RECTO_BENCH100 isn't set, so no corpus files were read.");
        return;
    };
    let file = std::path::PathBuf::from(folder).join("policy_worldbank_wbD34442277.pdf");
    let state = State::at(Level::Control);
    let _serving = state.serve();
    state.json(&["open", file.to_str().unwrap()]);
    let (read, code) = state.json(&["text", "--page", "11"]);
    assert_eq!(code, 0, "{read}");
    let text = read["data"]["untrusted"]["text"].as_str().unwrap();
    assert!(text.contains("utilizing at least"), "{text}");
    assert!(!text.contains("utilizing at least utilizing at") && !text.contains("Learning Grants Learning"), "{text}");
}
