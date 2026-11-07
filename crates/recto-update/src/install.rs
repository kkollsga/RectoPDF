//! **The installer, `recto-updater`.** Started by the app after a good hand-off, and it outlives it. What it does in order, each step such that being killed right after it leaves a whole app and the whole state:
//!
//! 1. Waits for the one process it was told about to end. A launch waits only 600 ms for the record's lock and then starts empty, so this comes first.
//! 2. Copies the record aside, to put back if the new version goes wrong.
//! 3. Swaps the new app and the running one's bundle in a single step. There's never a moment with no app or half of one.
//! 4. Moves the old app to be kept.
//! 5. Opens the new app, and waits for it to say it's reached its first frame.
//! 6. If it doesn't, stops it, puts the record back, swaps the old app back, and opens that.
//!
//! The installer quits nothing: quitting was the app's own hand-off. What's left if this is killed is put right at the next launch ([`crate::updater`]). It writes a log to the state folder, with words of its own.

use crate::plan::{Files, Outcome, OutcomeKind, Plan, log, new_app_in, previous_app_in};
use crate::process::tool;
use crate::{APP_BUNDLE, APP_EXECUTABLE, RELEASE_BUILD};
use recto_platform::local;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

const OPEN: &str = "/usr/bin/open";
const WAIT_STEP: Duration = Duration::from_millis(5);
const LOOK_STEP: Duration = Duration::from_millis(20);
/// How long a new app that's vanished gets to come back, as `open` can take a moment to show one.
const GONE_GRACE: Duration = Duration::from_secs(3);
/// How long the new app gets to end by being asked, and then by being told.
const STOP_PATIENCE: Duration = Duration::from_secs(5);

/// A test's way to kill the installer right after a step, to prove what's left is fine. Only in a build that isn't a release.
pub const DIE_AT_ENV: &str = "RECTO_UPDATER_DIE_AT";

fn die_if_told(step: &str) {
    if !RELEASE_BUILD && std::env::var(DIE_AT_ENV).is_ok_and(|at| at == step) {
        std::process::abort();
    }
}

/// How an install run ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ended {
    Updated,
    NotInstalled,
    WentBack,
    /// Nothing's running and nothing could be done about it: the log says why.
    Stuck,
}

struct Run<'a> {
    plan: &'a Plan,
    files: Files,
}

impl Run<'_> {
    fn say(&self, line: &str) {
        log(&self.files.log, line);
    }

    /// A step's done: into the log, and killed here if a test says.
    fn step(&self, name: &str) {
        self.say(&format!("installer: {name}"));
        die_if_told(name);
    }

    fn finish_with(&self, kind: OutcomeKind, why: &str) {
        let outcome = Outcome { kind, from: self.plan.from.clone(), to: self.plan.to.clone(), why: why.to_owned() };
        if outcome.write(&self.files.outcome()).is_err() {
            self.say("installer: the outcome couldn't be written");
        }
    }

    /// Everything this run made in the state folder, for when it's gone well.
    fn clear_own_files(&self) {
        let _ = std::fs::remove_dir_all(self.files.before());
        let _ = std::fs::remove_file(self.files.plan());
        let _ = std::fs::remove_file(self.files.launch_ok());
        let _ = std::fs::remove_file(self.files.helper_pid());
    }

    fn wait_for_old_app(&self) -> bool {
        let deadline = Instant::now() + Duration::from_millis(self.plan.quit_limit_ms);
        while local::process_counters(self.plan.pid).is_some() {
            if Instant::now() >= deadline {
                return false;
            }
            std::thread::sleep(WAIT_STEP);
        }
        true
    }

    /// Opens the app at the plan's path: `open`, or `open -g` if it wasn't frontmost. A test's run says what to add. Whether it's a release decides whether any of that's looked at.
    fn open_app(&self) -> bool {
        let test = if RELEASE_BUILD { None } else { self.plan.test.as_ref() };
        if let Some(test) = test
            && test.direct
        {
            return local::spawn_detached(&self.plan.executable(), &test.args, &test.env, &self.files.log).is_ok();
        }
        let args = open_arguments(self.plan, test);
        let words: Vec<&str> = args.iter().map(String::as_str).collect();
        tool(OPEN, &words, Duration::from_secs(30), 64 * 1024).is_ok_and(|(fine, _, _)| fine)
    }

    /// Waits for the new app's first frame. Comes back early if the app's gone and stays gone.
    fn new_app_started(&self) -> bool {
        let began = Instant::now();
        let limit = Duration::from_millis(self.plan.start_limit_ms);
        let executable = self.plan.executable();
        let mut gone_since: Option<Instant> = None;
        let mut next_look = began + GONE_GRACE;
        while began.elapsed() < limit {
            if std::fs::read_to_string(self.files.launch_ok()).is_ok_and(|text| text.split_whitespace().next() == Some(self.plan.to.as_str())) {
                return true;
            }
            if Instant::now() >= next_look {
                next_look = Instant::now() + Duration::from_millis(250);
                if local::processes_running(&executable).is_empty() {
                    let since = *gone_since.get_or_insert_with(Instant::now);
                    if since.elapsed() >= GONE_GRACE {
                        self.say("installer: the new app ended before its first frame");
                        return false;
                    }
                } else {
                    gone_since = None;
                }
            }
            std::thread::sleep(LOOK_STEP);
        }
        self.say("installer: the new app didn't reach its first frame in time");
        false
    }

    /// Ends whatever's running from the app's path. Only ever the new version, which this installer started.
    fn stop_new_app(&self) {
        let executable = self.plan.executable();
        for kill in [false, true] {
            for pid in local::processes_running(&executable) {
                local::signal_process(pid, kill);
            }
            let deadline = Instant::now() + STOP_PATIENCE;
            while !local::processes_running(&executable).is_empty() && Instant::now() < deadline {
                std::thread::sleep(WAIT_STEP * 4);
            }
            if local::processes_running(&executable).is_empty() {
                return;
            }
        }
    }

    /// Puts things back as they were: the old record, the old app, and opens it.
    fn go_back(&self, why: &str) -> Ended {
        self.step("going_back");
        self.stop_new_app();
        self.step("new_app_stopped");
        restore_record(&self.files.before(), &self.files.session());
        self.step("record_restored");
        let (previous, app) = (self.plan.previous_app(), &self.plan.app);
        if local::swap_paths(&previous, app).is_err() {
            self.say("installer: the kept app couldn't be put back");
            self.finish_with(OutcomeKind::WentBack, "couldnt_swap_back");
            return Ended::Stuck;
        }
        self.step("swapped_back");
        // What's left at `previous` is the version that didn't start.
        let _ = std::fs::remove_dir_all(previous.parent().unwrap_or(&previous));
        let _ = std::fs::remove_file(self.plan.beside.join("staged.json"));
        self.finish_with(OutcomeKind::WentBack, why);
        self.step("outcome_written");
        let _ = std::fs::remove_file(self.files.launch_ok());
        let _ = std::fs::remove_file(self.files.helper_pid());
        if !self.open_app() {
            self.say("installer: the previous app couldn't be opened");
            return Ended::Stuck;
        }
        self.say(&format!("installer: went back to {} ({why})", self.plan.from));
        Ended::WentBack
    }
}

/// What `open` is told: `-g` if it wasn't frontmost, and a test's own additions. The values are arguments, never anything a shell reads.
#[must_use]
pub fn open_arguments(plan: &Plan, test: Option<&crate::plan::TestLaunch>) -> Vec<String> {
    let mut args = Vec::new();
    // A test's run never takes the focus from anyone, whichever it's standing in for.
    if plan.background || test.is_some() {
        args.push("-g".to_owned());
    }
    if let Some(test) = test {
        for (name, value) in &test.env {
            args.push("--env".to_owned());
            args.push(format!("{name}={value}"));
        }
    }
    args.push(plan.app.display().to_string());
    if let Some(test) = test
        && !test.args.is_empty()
    {
        args.push("--args".to_owned());
        args.extend(test.args.iter().cloned());
    }
    args
}

/// Runs an install. Takes the plan, as the app left it.
#[must_use]
pub fn run(plan: &Plan) -> Ended {
    let run = Run { plan, files: Files::in_state(&plan.root) };
    let files = &run.files;
    if !crate::rules::is_newer(&plan.to, &plan.from) {
        run.say("installer: that isn't a newer version, so nothing was replaced");
        return Ended::NotInstalled;
    }
    let _ = std::fs::write(files.helper_pid(), std::process::id().to_string());
    run.say(&format!("installer: {} to {}, waiting for process {}, to reopen {}", plan.from, plan.to, plan.pid, if plan.background { "in the background" } else { "in front" }));
    if !run.wait_for_old_app() {
        run.say("installer: the old app didn't end in time, so nothing was replaced");
        run.finish_with(OutcomeKind::NotInstalled, "old_app_stayed");
        return Ended::NotInstalled;
    }
    let old_gone = crate::wall_ms();
    run.step("old_app_gone");
    // The app takes this away if it stayed after all: nothing's to be done then.
    if !files.plan().exists() {
        run.say("installer: the app took the plan back, so nothing was replaced");
        let _ = std::fs::remove_file(files.helper_pid());
        return Ended::NotInstalled;
    }
    // Someone's opened it again in that moment. Their copy's in use, and it isn't this installer's to stop.
    if !local::processes_running(&plan.executable()).is_empty() {
        run.say("installer: the app's been opened again, so nothing was replaced");
        run.finish_with(OutcomeKind::NotInstalled, "app_reopened");
        return Ended::NotInstalled;
    }
    if !plan.new_app().is_dir() {
        run.say("installer: the new app isn't there, so nothing was replaced");
        run.finish_with(OutcomeKind::NotInstalled, "new_app_missing");
        let _ = run.open_app();
        return Ended::NotInstalled;
    }
    let _ = std::fs::remove_file(files.launch_ok());
    copy_record(&files.session(), &files.before());
    run.step("record_copied");
    if local::swap_paths(&plan.new_app(), &plan.app).is_err() {
        run.say("installer: the swap failed, so nothing was replaced");
        run.finish_with(OutcomeKind::NotInstalled, "swap_failed");
        let _ = run.open_app();
        return Ended::NotInstalled;
    }
    let swapped = crate::wall_ms();
    run.step("swapped");
    if keep_old_app(plan).is_err() {
        // The old app's still whole where it landed, and the next launch moves it.
        run.say("installer: the old app couldn't be moved to be kept");
    }
    run.step("old_app_kept");
    if !run.open_app() {
        return run.go_back("couldnt_open");
    }
    run.step("opened");
    if !run.new_app_started() {
        return run.go_back("didnt_start");
    }
    run.say(&format!("installer: {} started. Old app gone {} ms after the quit, swapped {} ms after that, first frame {} ms after that", plan.to, old_gone.saturating_sub(plan.quit_at_ms), swapped.saturating_sub(old_gone), crate::wall_ms().saturating_sub(swapped)));
    run.step("started");
    let _ = std::fs::remove_dir_all(plan.beside.join("new"));
    let _ = std::fs::remove_file(plan.beside.join("staged.json"));
    run.clear_own_files();
    Ended::Updated
}

/// Goes back to the kept app with no install before it: what a launch does when the new version keeps failing with no installer around to see it.
#[must_use]
pub fn run_back(plan: &Plan) -> Ended {
    let run = Run { plan, files: Files::in_state(&plan.root) };
    let _ = std::fs::write(run.files.helper_pid(), std::process::id().to_string());
    run.say(&format!("installer: {} keeps failing, going back to {}", plan.to, plan.from));
    if !run.wait_for_old_app() {
        run.finish_with(OutcomeKind::NotInstalled, "old_app_stayed");
        return Ended::NotInstalled;
    }
    run.go_back("kept_failing")
}

fn keep_old_app(plan: &Plan) -> std::io::Result<()> {
    let keep = previous_app_in(&plan.beside);
    let folder = keep.parent().ok_or_else(|| std::io::Error::other("no folder"))?;
    let _ = std::fs::remove_dir_all(folder);
    std::fs::create_dir_all(folder)?;
    std::fs::rename(new_app_in(&plan.beside), &keep)
}

/// Whether a record file is one to copy: a closed PDF's history isn't read at a launch, and it's what can be large.
fn record_file_to_keep(name: &str) -> bool {
    !name.starts_with("closed-")
}

fn walk_record(from: &Path, to: &Path, found: &mut Vec<(PathBuf, PathBuf)>) -> std::io::Result<()> {
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        let kind = entry.file_type()?;
        let name = entry.file_name();
        if kind.is_dir() {
            walk_record(&entry.path(), &to.join(&name), found)?;
        } else if kind.is_file() && record_file_to_keep(&name.to_string_lossy()) {
            found.push((entry.path(), to.join(&name)));
        }
    }
    Ok(())
}

/// A clone of the record, as it stands. Costs no disk until something changes, and takes moments. A disk that can't clone means no copy, which only means a rollback brings back what's there.
pub fn copy_record(session: &Path, before: &Path) {
    let _ = std::fs::remove_dir_all(before);
    let mut files = Vec::new();
    if walk_record(session, before, &mut files).is_err() {
        return;
    }
    for (from, to) in files {
        let fine = to.parent().is_some_and(|folder| std::fs::create_dir_all(folder).is_ok()) && local::clone_file(&from, &to).is_ok();
        if !fine {
            let _ = std::fs::remove_dir_all(before);
            return;
        }
    }
    let _ = std::fs::write(before.join(".whole"), "");
}

/// Puts the copy back, one file at a time, each by a rename so none's ever half there, and takes away what's been added since. A copy that isn't whole is left alone.
pub fn restore_record(before: &Path, session: &Path) {
    if !before.join(".whole").exists() {
        return;
    }
    let mut kept = Vec::new();
    if walk_record(before, session, &mut kept).is_err() {
        return;
    }
    let mut now = Vec::new();
    let _ = walk_record(session, session, &mut now);
    for (from, to) in &kept {
        if to.file_name().is_some_and(|name| name == ".whole") {
            continue;
        }
        if let Some(folder) = to.parent() {
            let _ = std::fs::create_dir_all(folder);
            let temp = folder.join(".restoring");
            let _ = std::fs::remove_file(&temp);
            if local::clone_file(from, &temp).is_ok() {
                let _ = std::fs::rename(&temp, to);
            }
        }
    }
    let wanted: std::collections::HashSet<PathBuf> = kept.into_iter().map(|(_, to)| to).collect();
    for (path, _) in now {
        if !wanted.contains(&path) {
            let _ = std::fs::remove_file(path);
        }
    }
}

/// Where a bundle's executable is. For the guard at launch, which has only the app.
#[must_use]
pub fn executable_of(app: &Path) -> PathBuf {
    app.join(APP_EXECUTABLE)
}

/// That a path really is a bundle of this app. For what's found lying about.
#[must_use]
pub fn looks_like_app(path: &Path) -> bool {
    path.file_name().is_some_and(|name| name == APP_BUNDLE) && path.join(APP_EXECUTABLE).is_file()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plan() -> Plan {
        Plan { from: "0.1.0".into(), to: "0.2.0".into(), pid: 1, app: "/Apps/a b/RectoPDF.app".into(), beside: "/Apps/a b/.RectoPDF-update".into(), root: "/s".into(), quit_at_ms: 0, background: false, quit_limit_ms: 1, start_limit_ms: 1, test: None }
    }

    /// Failure: an app in the background comes back taking the focus from whatever the user's doing, or a frontmost one comes back behind it. The owner's rule: "an app in the background reopens without taking focus". And a path with spaces and quotes stays one argument.
    #[test]
    fn the_app_comes_back_as_it_was_frontmost_or_not() {
        assert_eq!(open_arguments(&plan(), None), vec!["/Apps/a b/RectoPDF.app".to_string()], "frontmost");
        assert_eq!(open_arguments(&Plan { background: true, ..plan() }, None), vec!["-g".to_string(), "/Apps/a b/RectoPDF.app".to_string()], "in the background");
        let test = crate::plan::TestLaunch { args: vec!["--qa".into()], env: vec![("A".into(), "b c".into())], direct: false };
        assert_eq!(open_arguments(&Plan { background: true, ..plan() }, Some(&test)), ["-g", "--env", "A=b c", "/Apps/a b/RectoPDF.app", "--args", "--qa"].map(str::to_string).to_vec());
    }

    /// Failure: the copy of the record misses part of it, brings back a history of a closed PDF it never took, or leaves behind what a broken new version added.
    #[test]
    fn a_record_comes_back_as_it_was() {
        let dir = std::env::temp_dir().canonicalize().unwrap().join(format!("recto-install-{}-record", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let (session, before) = (dir.join("session"), dir.join("before"));
        std::fs::create_dir_all(session.join("journals")).unwrap();
        std::fs::write(session.join("record.a.json"), "good a").unwrap();
        std::fs::write(session.join("journals/journal-1.log"), "good journal").unwrap();
        std::fs::write(session.join("journals/closed-1.log"), "a closed one").unwrap();
        copy_record(&session, &before);
        assert!(before.join(".whole").exists());
        assert!(!before.join("journals/closed-1.log").exists());
        // What a broken new version does.
        std::fs::write(session.join("record.a.json"), "damaged").unwrap();
        std::fs::remove_file(session.join("journals/journal-1.log")).unwrap();
        std::fs::write(session.join("journals/journal-2.log"), "from the new version").unwrap();
        restore_record(&before, &session);
        assert_eq!(std::fs::read_to_string(session.join("record.a.json")).unwrap(), "good a");
        assert_eq!(std::fs::read_to_string(session.join("journals/journal-1.log")).unwrap(), "good journal");
        assert!(!session.join("journals/journal-2.log").exists());
        assert_eq!(std::fs::read_to_string(session.join("journals/closed-1.log")).unwrap(), "a closed one", "not taken, and not touched");
        // A copy that isn't whole changes nothing.
        std::fs::remove_file(before.join(".whole")).unwrap();
        std::fs::write(session.join("record.a.json"), "newer").unwrap();
        restore_record(&before, &session);
        assert_eq!(std::fs::read_to_string(session.join("record.a.json")).unwrap(), "newer");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
