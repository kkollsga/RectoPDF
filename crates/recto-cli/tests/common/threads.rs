//! A process's threads by name, so a test that counts them says on failure which they were, on a machine nobody can look at (CI, 2026-11-12: "12 then 15 threads" and "13 then 17", with nothing to tell a thread per tab from a command's on its way out).

use std::collections::BTreeMap;

/// The names and how many have each, as `13: recto-session, 3 × unnamed, …`, or why they couldn't be read.
pub fn named(pid: u32) -> String {
    let Some(names) = recto_platform::local::thread_names(pid) else {
        return format!("(process {pid}'s threads couldn't be read)");
    };
    let mut counted: BTreeMap<&str, usize> = BTreeMap::new();
    for name in &names {
        *counted.entry(name.as_str()).or_default() += 1;
    }
    let listed: Vec<String> = counted
        .iter()
        .map(|(name, &count)| if count == 1 { (*name).to_string() } else { format!("{count} × {name}") })
        .collect();
    format!("{}: {}", names.len(), listed.join(", "))
}

/// The threads the app made itself, by name: every thread its code starts is named (`named_threads::every_thread_the_code_starts_is_named`), and so is AppKit's event thread. The unnamed ones are the system's libdispatch pool (and the main thread), sized by the system's load and reaped when it likes, so a count of all of them measures the machine. Empty if the process can't be read.
pub fn own(pid: u32) -> Vec<String> {
    recto_platform::local::thread_names(pid)
        .unwrap_or_default()
        .into_iter()
        .filter(|name| name != "unnamed")
        .collect()
}

/// How many of these have this name.
pub fn count(names: &[String], name: &str) -> usize {
    names.iter().filter(|found| *found == name).count()
}

/// These without any of the names given: what's left once the threads that come and go at a moment's asking are taken out.
pub fn without(names: &[String], passing: &[&str]) -> Vec<String> {
    names.iter().filter(|name| !passing.contains(&name.as_str())).cloned().collect()
}

/// The app's threads that come and go at a moment's asking, whatever the number of tabs: a command's on its way out once it's answered (`recto-job`), the one look at the tabs' files, the one read or open of the tab on screen's, and the one sleep-file writer. Each is checked on its own; the rest are what's kept.
pub const PASSING: [&str; 5] = ["recto-job", "recto-look", "recto-read", "recto-open", "recto-sleep"];

/// The passing ones there's at most one of at a time, by the app's design: one look, one read, one writer. Not a command's, of which there's one per request on its way out.
pub const ONE_AT_A_TIME: [&str; 4] = ["recto-look", "recto-read", "recto-open", "recto-sleep"];

/// What's wrong with these threads against those at the moment compared with: more of the kept ones, or more than one of a kind there's one at a time of. `None` if nothing.
pub fn grew(before: &[String], now: &[String]) -> Option<String> {
    let (kept_before, kept_now) = (without(before, &PASSING), without(now, &PASSING));
    if kept_now.len() > kept_before.len() {
        return Some(format!(
            "the app's own threads grew from {} to {}: {kept_before:?} then {kept_now:?}",
            kept_before.len(),
            kept_now.len()
        ));
    }
    ONE_AT_A_TIME
        .iter()
        .find(|name| count(now, name) > 1)
        .map(|name| format!("{} threads named {name}, where there's one at a time: {now:?}", count(now, name)))
}

/// Waits, up to `limit` (a bound for a thread that stays, not a guess at how long one takes to end on a busy machine), for the app's own threads to be back to what they were (`grew` says nothing), as a thread that does one thing ends by itself once it's done: a check, a look, a pane's page view. What was still wrong at the limit, with every thread's name, or `None` once they're back.
pub fn back_to(pid: u32, before: &[String], limit: std::time::Duration) -> Option<String> {
    let began = std::time::Instant::now();
    loop {
        let wrong = grew(before, &own(pid))?;
        if began.elapsed() > limit {
            return Some(format!("{wrong}\nall of them: {}", named(pid)));
        }
        std::thread::sleep(std::time::Duration::from_millis(50)); // ratchet: poll
    }
}
