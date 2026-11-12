//! **Many tabs across a quit:** a record of a thousand comes back cold, with no thread, handle or document for each.

use super::{Run, State, long_document};
use crate::common::threads;
use std::time::Instant;

/// Failure: a record of 1,000 tabs comes back slowly, or with a handle or a thread for each, or wakes them all. Only the active tab holds a document, and it has no more handles or threads than 2 tabs do, but for the one read of the active tab's file and the one look at the others, which may not have ended yet. A count over says which threads there were, by name, at both moments.
#[test]
fn a_thousand_tabs_come_back_cold() {
    let state = State::new();
    let Some(run) = Run::on(&state) else { return };
    let file = long_document("many", 3);
    let folder = state.root.join("tabs");
    std::fs::create_dir_all(&folder).unwrap();
    let link = |at: usize| folder.join(format!("t{at}.pdf"));
    // Files of their own, as a link is the same PDF.
    for at in 0..1000 {
        recto_platform::local::clone_file(&file.0, &link(at)).unwrap();
    }
    run.ok(&["open", link(0).to_str().unwrap()]);
    run.ok(&["open", link(1).to_str().unwrap()]);
    run.settled();
    let few = run.report();
    let few_named = threads::named(run.pid);
    run.quit();
    // Those two came back. The rest go in as a record, as though they'd been open.
    let run = Run::on(&state).unwrap();
    run.settled();
    let two = run.report();
    assert_eq!(two["session"]["tabs"].as_array().unwrap().len(), 2);
    for at in 2..1000 {
        run.ok(&["open", link(at).to_str().unwrap()]);
    }
    run.settled();
    let opened = run.report();
    run.quit();
    let started = Instant::now();
    let run = Run::on(&state).unwrap();
    run.settled();
    let took = started.elapsed();
    let back = run.report();
    let back_named = threads::named(run.pid);
    assert_eq!(back["session"]["tabs"].as_array().unwrap().len(), 1000);
    assert_eq!(back["reader"]["documents_held"], 1, "only the active tab");
    assert_eq!(back["session"]["active"], opened["session"]["active"]);
    let (threads_few, threads_back) = (few["memory"]["threads"].as_u64().unwrap(), back["memory"]["threads"].as_u64().unwrap());
    assert!(
        threads_back <= threads_few + 2,
        "{threads_few} then {threads_back} threads\nwith two tabs, just after: {few_named}\nwith 1,000, just after: {back_named}"
    );
    let (handles_few, handles_back) = (
        few["memory"]["open_handles"].as_u64().unwrap(),
        back["memory"]["open_handles"].as_u64().unwrap(),
    );
    assert!(handles_back <= handles_few + 2, "{handles_few} then {handles_back} handles");
    eprintln!("1,000 tabs came back and settled in {took:?}; threads {threads_few} then {threads_back}: {few_named}; then {back_named}");
}
