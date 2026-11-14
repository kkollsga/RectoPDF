//! **What the sleeper writes and when, step by step, as a golden** (R3.7 S6's guard, before the sleeper's methods left `Inner`): a scripted run through tabs
//! pushed out of the warm ones by the person and by an agent, the cool-down, a tab handed back inside it, an agent's `session sleep`, memory pressure, a tab
//! left alone and a quit's flush, with `qa state`'s `record.sleep` counts and the queue's after each step. No bytes (a file's size goes with the temp folder's
//! path) and no timings: only what was decided, by whom, and what's on disk.

use crate::testkit::headless::{Disk, HeadlessApp};
use recto_agent::Level;
use serde_json::Value;

/// The tab whose file ends with `name`.
fn tab(app: &HeadlessApp, name: &str) -> String {
    let id = app.session_json()["tabs"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|tab| tab["path"].as_str().is_some_and(|path| path.ends_with(name)))
        .and_then(|tab| tab["id"].as_u64())
        .expect("the tab");
    format!("#{id}")
}

/// A person's command, as the window runs it, settled.
fn person(app: &HeadlessApp, words: &[&str]) {
    let words: Vec<String> = words.iter().map(|word| (*word).to_owned()).collect();
    app.person(&recto_agent::table::parse_cli(&words).expect("words").wire());
    app.settle();
}

/// An agent's command at Full, settled.
fn agent(app: &HeadlessApp, words: &[&str]) {
    app.recto(Level::Full, words).unwrap_or_else(|error| panic!("{words:?}: {error:?}"));
    app.settle();
}

/// Waits till nothing's queued, in flight or being written, the clock standing still.
fn written(app: &HeadlessApp) {
    app.wait_until("the queue written and its writer gone", || {
        let leaving = app.record_json()["sleep"]["leaving"].clone();
        leaving["queued"] == 0 && leaving["in_flight"] == false && leaving["writer"] == false
    });
}

/// The sleep files in the state folder.
fn on_disk(app: &HeadlessApp) -> usize {
    fn walk(dir: &std::path::Path) -> usize {
        std::fs::read_dir(dir)
            .into_iter()
            .flatten()
            .flatten()
            .map(|entry| {
                let path = entry.path();
                if path.is_dir() {
                    walk(&path)
                } else {
                    usize::from(path.file_name().and_then(|name| name.to_str()).is_some_and(|name| name.starts_with("sleep-")))
                }
            })
            .sum()
    }
    walk(&app.disk().paths().root)
}

/// One step's line: what was decided, what agents' share has counted, what the queue did, what's on disk.
fn line(app: &HeadlessApp, step: &str, agent_bytes: &mut u64) -> String {
    let sleep = app.record_json()["sleep"].clone();
    let n = |value: &Value| value.as_u64().unwrap_or_else(|| panic!("a count: {sleep}"));
    let gone = &sleep["gone_dormant"];
    let agents = n(&sleep["agent_bytes_this_hour"]);
    let by_agents = match (agents, agents > *agent_bytes) {
        (0, _) => "none",
        (_, true) => "rose",
        (_, false) => "same",
    };
    *agent_bytes = agents;
    let leaving = &sleep["leaving"];
    format!(
        "{step} | dormant left_alone {} budget {} pressure {} asked {} | files {} put_off {} paper {} place {} skipped_busy {} | agents' bytes {by_agents}, \
         share used {} | woken {} pressure_events {} system {}/{} | queue queued {} handed_back {} hurried {} retired {} dropped {} replaced {} \
         pictures_dropped {} writer_panics {} | on disk {}",
        n(&gone["left_alone"]),
        n(&gone["budget"]),
        n(&gone["pressure"]),
        n(&gone["asked"]),
        n(&sleep["sleep_files_written"]),
        n(&sleep["writes_put_off"]),
        n(&sleep["writes_for_paper"]),
        n(&sleep["writes_for_place"]),
        n(&sleep["writes_skipped_busy"]),
        n(&sleep["agent_share_used"]),
        n(&sleep["woken_from_file"]),
        n(&sleep["pressure_events"]),
        n(&sleep["system_pressure"]["warning"]),
        n(&sleep["system_pressure"]["critical"]),
        n(&leaving["queued"]),
        n(&leaving["handed_back"]),
        n(&leaving["hurried"]),
        n(&leaving["retired"]),
        n(&leaving["dropped"]),
        n(&leaving["replaced"]),
        n(&leaving["pictures_dropped"]),
        n(&leaving["writer_panics"]),
        on_disk(app),
    )
}

/// What the run below said on 2026-11-14, before the sleeper's methods left `Inner`: one line a step.
const GOLDEN: &str = "\
the person opens six | dormant left_alone 0 budget 0 pressure 0 asked 0 | files 0 put_off 0 paper 0 place 0 skipped_busy 0 | agents' bytes none, share used 0 | woken 0 pressure_events 0 system 0/0 | queue queued 3 handed_back 0 hurried 0 retired 0 dropped 0 replaced 0 pictures_dropped 0 writer_panics 0 | on disk 0
the person shows t0 inside its wait | dormant left_alone 0 budget 0 pressure 0 asked 0 | files 0 put_off 0 paper 0 place 0 skipped_busy 0 | agents' bytes none, share used 0 | woken 0 pressure_events 0 system 0/0 | queue queued 3 handed_back 1 hurried 0 retired 0 dropped 0 replaced 0 pictures_dropped 0 writer_panics 0 | on disk 0
the wait passes | dormant left_alone 0 budget 0 pressure 0 asked 0 | files 3 put_off 0 paper 0 place 0 skipped_busy 0 | agents' bytes none, share used 0 | woken 0 pressure_events 0 system 0/0 | queue queued 0 handed_back 1 hurried 0 retired 0 dropped 0 replaced 0 pictures_dropped 0 writer_panics 0 | on disk 3
an agent opens two, and the wait passes | dormant left_alone 0 budget 0 pressure 0 asked 0 | files 5 put_off 0 paper 0 place 0 skipped_busy 0 | agents' bytes rose, share used 0 | woken 0 pressure_events 0 system 0/0 | queue queued 0 handed_back 1 hurried 0 retired 0 dropped 0 replaced 0 pictures_dropped 0 writer_panics 0 | on disk 5
an agent's session sleep of t6 | dormant left_alone 0 budget 0 pressure 0 asked 1 | files 6 put_off 0 paper 0 place 0 skipped_busy 0 | agents' bytes rose, share used 0 | woken 0 pressure_events 0 system 0/0 | queue queued 0 handed_back 1 hurried 0 retired 0 dropped 0 replaced 0 pictures_dropped 0 writer_panics 0 | on disk 6
memory pressure | dormant left_alone 0 budget 0 pressure 1 asked 1 | files 7 put_off 0 paper 0 place 0 skipped_busy 0 | agents' bytes same, share used 0 | woken 0 pressure_events 1 system 0/0 | queue queued 0 handed_back 1 hurried 0 retired 0 dropped 0 replaced 0 pictures_dropped 0 writer_panics 0 | on disk 7
the person shows t1 then t2 | dormant left_alone 0 budget 0 pressure 1 asked 1 | files 7 put_off 0 paper 0 place 0 skipped_busy 0 | agents' bytes same, share used 0 | woken 2 pressure_events 1 system 0/0 | queue queued 0 handed_back 1 hurried 0 retired 0 dropped 0 replaced 0 pictures_dropped 0 writer_panics 0 | on disk 7
left alone | dormant left_alone 2 budget 0 pressure 1 asked 1 | files 8 put_off 0 paper 0 place 0 skipped_busy 0 | agents' bytes same, share used 0 | woken 2 pressure_events 1 system 0/0 | queue queued 0 handed_back 1 hurried 0 retired 0 dropped 0 replaced 0 pictures_dropped 0 writer_panics 0 | on disk 8
the person opens four more | dormant left_alone 2 budget 0 pressure 1 asked 1 | files 8 put_off 0 paper 0 place 0 skipped_busy 0 | agents' bytes same, share used 0 | woken 2 pressure_events 1 system 0/0 | queue queued 1 handed_back 1 hurried 0 retired 0 dropped 0 replaced 0 pictures_dropped 0 writer_panics 0 | on disk 8
a quit's flush | dormant left_alone 2 budget 0 pressure 1 asked 1 | files 10 put_off 0 paper 0 place 0 skipped_busy 0 | agents' bytes same, share used 0 | woken 2 pressure_events 1 system 0/0 | queue queued 0 handed_back 1 hurried 0 retired 0 dropped 0 replaced 0 pictures_dropped 0 writer_panics 0 | on disk 10
";

/// Failure: a move of the sleeper's code changes what's written or when, or who it's counted against: a tab pushed out and not queued, one handed back and
/// written anyway, a queue hurried or not, an agent's push-out or `session sleep` counted as the person's (or the other way round), a tab left alone or under
/// pressure not going, a quit's flush not writing the tab on screen's file. Each step's counts are compared with the golden, and the first step that differs
/// is named with both lines.
#[test]
fn what_the_sleeper_writes_and_when_is_as_it_was() {
    let wait = crate::sleeper::LEAVING_WAIT_MS;
    let disk = Disk::new();
    let docs: Vec<_> = (0..12).map(|at| disk.pdf(&format!("t{at}.pdf"), 3)).collect();
    let app = disk.builder().leaving_wait(wait).start();
    let mut agents = 0;
    let mut lines = Vec::new();
    for doc in &docs[..6] {
        app.open(doc);
    }
    lines.push(line(&app, "the person opens six", &mut agents));
    person(&app, &["tap", &tab(&app, "t0.pdf")]);
    lines.push(line(&app, "the person shows t0 inside its wait", &mut agents));
    app.advance(wait);
    written(&app);
    lines.push(line(&app, "the wait passes", &mut agents));
    for doc in &docs[6..8] {
        agent(&app, &["open", &doc.display().to_string()]);
    }
    app.advance(wait);
    written(&app);
    lines.push(line(&app, "an agent opens two, and the wait passes", &mut agents));
    agent(&app, &["session", "sleep", "--tab", &tab(&app, "t6.pdf")]);
    written(&app);
    lines.push(line(&app, "an agent's session sleep of t6", &mut agents));
    app.core().qa_session_verb("memory_pressure", "warning").expect("a QA verb").expect("pressure");
    app.settle();
    written(&app);
    lines.push(line(&app, "memory pressure", &mut agents));
    person(&app, &["tap", &tab(&app, "t1.pdf")]);
    person(&app, &["tap", &tab(&app, "t2.pdf")]);
    written(&app);
    lines.push(line(&app, "the person shows t1 then t2", &mut agents));
    app.core().qa_session_verb("dormant_after", "0").expect("a QA verb").expect("set");
    app.wait_until("every tab off screen gone dormant", || {
        app.core().host.session.with_session(|session| session.dormant().oldest_first().is_empty())
    });
    app.core().qa_session_verb("dormant_after", "86400").expect("a QA verb").expect("set");
    app.settle();
    written(&app);
    lines.push(line(&app, "left alone", &mut agents));
    for doc in &docs[8..] {
        app.open(doc);
    }
    lines.push(line(&app, "the person opens four more", &mut agents));
    app.core().saver.flush(false, crate::testkit::headless::WAIT_LIMIT).expect("the state written");
    lines.push(line(&app, "a quit's flush", &mut agents));
    drop(app.quit());
    let now = lines.join("\n") + "\n";
    if now != GOLDEN {
        let golden: Vec<&str> = GOLDEN.lines().collect();
        let first = lines
            .iter()
            .zip(golden.iter().chain(std::iter::repeat(&"(none)")))
            .find(|(now, was)| now != *was);
        panic!(
            "the sleeper's counts changed; first step that differs:\n  was: {}\n  now: {}\nall now:\n{now}",
            first.map_or("", |pair| pair.1),
            first.map_or("", |pair| pair.0.as_str())
        );
    }
}
