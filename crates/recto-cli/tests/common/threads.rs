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
