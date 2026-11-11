//! **The installer, `recto-updater`.** Started by the app after a good hand-off, and it outlives it. What it does in order, each step such that being killed right after it leaves a whole app and the whole state:
//!
//! 0. Takes a lock for its whole life, so only one is ever at work on a state folder, and its plan has a name, so one whose plan's been replaced or taken back stands down.
//! 1. Waits for the one process it was told about to end. A launch waits only 600 ms for the record's lock and then starts empty, so this comes first.
//! 2. Looks once more at what it's about to swap in: the very bytes that were verified, however long ago that was.
//! 3. Copies the record aside, to put back if the new version goes wrong.
//! 4. Swaps the new app and the running one's bundle in a single step. There's never a moment with no app or half of one.
//! 5. Moves the old app to be kept.
//! 6. Opens the new app, and waits for it to say it's come up: see [`crate::updater::Updater::first_frame`] for what that means.
//! 7. If it doesn't, stops it, puts the record back, swaps the old app back, and opens that.
//!
//! The installer quits nothing: quitting was the app's own hand-off. What's left if this is killed is put right at the next launch ([`crate::updater`]). It writes a log to the state folder, with words of its own.

use crate::plan::{Files, Outcome, OutcomeKind, Plan, log, new_app_in, previous_app_in};
use crate::process::tool;
use crate::{APP_BUNDLE, APP_EXECUTABLE, RELEASE_BUILD, TEAM_ID};
use recto_platform::local;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

const OPEN: &str = "/usr/bin/open";
const WAIT_STEP: Duration = Duration::from_millis(5);
const LOOK_STEP: Duration = Duration::from_millis(20);
/// How long a new app that's been seen and has gone gets to come back.
const GONE_GRACE: Duration = Duration::from_secs(3);
/// How often the installer looks at whether its plan's still its own, while it waits for the old app.
const PLAN_LOOK: Duration = Duration::from_millis(100);
/// How long the new app gets to end by being asked, and then by being told.
const STOP_PATIENCE: Duration = Duration::from_secs(5);

/// A test's way to kill the installer right after a step, to prove what's left is fine. Only in a build that isn't a release.
pub const DIE_AT_ENV: &str = "RECTO_UPDATER_DIE_AT";
/// A test's way to hold the installer right after a step, as a racer would need it held, until a file says go on. Only in a build that isn't a release.
pub const PAUSE_AT_ENV: &str = "RECTO_UPDATER_PAUSE_AT";

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
        if !RELEASE_BUILD && std::env::var(PAUSE_AT_ENV).is_ok_and(|at| at == name) {
            let _ = std::fs::write(self.files.work.join("paused"), name);
            let began = Instant::now();
            while !self.files.work.join("go-on").exists() && began.elapsed() < Duration::from_secs(30) {
                std::thread::sleep(LOOK_STEP);
            }
        }
    }

    fn finish_with(&self, kind: OutcomeKind, why: &str) {
        let outcome = Outcome {
            kind,
            from: self.plan.from.clone(),
            to: self.plan.to.clone(),
            why: why.to_owned(),
        };
        if outcome.write(&self.files.outcome()).is_err() {
            self.say("installer: the outcome couldn't be written");
        }
        // So nothing that comes after, another installer or the next launch, takes this as still to do.
        let _ = std::fs::remove_file(self.files.plan());
    }

    /// Everything this run made in the state folder, for when it's gone well. Not the record's copy: see [`crate::updater::KEEP_RECORD_COPY_MS`].
    fn clear_own_files(&self) {
        let _ = std::fs::remove_file(self.files.plan());
        let _ = std::fs::remove_file(self.files.launch_ok());
        let _ = std::fs::remove_file(self.files.launch_failed());
        let _ = std::fs::remove_file(self.files.helper_pid());
    }

    /// Whether the plan on disk is still this run's. It isn't if the app took it back, or if a later install has replaced it.
    fn plan_is_mine(&self) -> bool {
        Plan::read(&self.files.plan()).is_some_and(|on_disk| on_disk.id == self.plan.id)
    }

    fn wait_for_old_app(&self) -> OldApp {
        let deadline = Instant::now() + Duration::from_millis(self.plan.quit_limit_ms);
        let mut next_look = Instant::now() + PLAN_LOOK;
        while local::process_counters(self.plan.pid).is_some() {
            if Instant::now() >= deadline {
                return OldApp::Stayed;
            }
            // Noticed now rather than once the old app's gone, so a later install isn't left waiting on the lock.
            if Instant::now() >= next_look {
                next_look = Instant::now() + PLAN_LOOK;
                if !self.plan_is_mine() {
                    return OldApp::PlanGone;
                }
            }
            std::thread::sleep(WAIT_STEP);
        }
        OldApp::Gone
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

    /// Waits for the new app to say it's come up. Comes back early if it says it hasn't, or it's gone and stays gone. A process that's never been seen gets a good while before that counts, as the system can spend some time checking an app it hasn't run before, and it isn't a failure.
    fn new_app_started(&self) -> Result<(), &'static str> {
        let began = Instant::now();
        let limit = Duration::from_millis(self.plan.start_limit_ms);
        let never_seen_limit = limit / 4;
        let executable = self.plan.executable();
        let mut seen = false;
        let mut gone_since: Option<Instant> = None;
        let mut next_look = began;
        while began.elapsed() < limit {
            if std::fs::read_to_string(self.files.launch_ok()).is_ok_and(|text| text.split_whitespace().next() == Some(self.plan.to.as_str())) {
                return Ok(());
            }
            if self.files.launch_failed().exists() {
                self.say("installer: the new app says it can't draw");
                return Err("couldnt_draw");
            }
            if Instant::now() >= next_look {
                next_look = Instant::now() + Duration::from_millis(100);
                if local::processes_running(&executable).is_empty() {
                    let since = *gone_since.get_or_insert_with(Instant::now);
                    if (seen && since.elapsed() >= GONE_GRACE) || (!seen && began.elapsed() >= never_seen_limit) {
                        self.say("installer: the new app ended before it came up");
                        return Err("didnt_start");
                    }
                } else {
                    seen = true;
                    gone_since = None;
                }
            }
            std::thread::sleep(LOOK_STEP);
        }
        self.say("installer: the new app didn't come up in time");
        Err("didnt_start")
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

    /// Where the old app is, if it's anywhere: where it's kept, or still in the swap's other half if keeping it didn't work. Only ever the version this plan came from.
    fn old_app(&self) -> Option<PathBuf> {
        [self.plan.previous_app(), self.plan.new_app()]
            .into_iter()
            .find(|path| crate::stage::app_version(path).as_deref() == Some(self.plan.from.as_str()))
    }

    /// Puts things back as they were: the old record, the old app, and opens it. With no old app it can only say so, and whatever's there stays, as it's all there is.
    fn go_back(&self, why: &str) -> Ended {
        self.step("going_back");
        self.stop_new_app();
        self.step("new_app_stopped");
        restore_record(&self.files.before(), &self.files.session());
        self.step("record_restored");
        let Some(old) = self.old_app() else {
            self.say("installer: there's no earlier version to go back to");
            self.finish_with(OutcomeKind::CouldntGoBack, "nothing_to_go_back_to");
            return Ended::Stuck;
        };
        if local::swap_paths(&old, &self.plan.app).is_err() {
            self.say("installer: the kept app couldn't be put back");
            self.finish_with(OutcomeKind::CouldntGoBack, "couldnt_swap_back");
            return Ended::Stuck;
        }
        self.step("swapped_back");
        // What's left where the old app was is the version that didn't start.
        let _ = std::fs::remove_dir_all(old.parent().unwrap_or(&old));
        let _ = std::fs::remove_file(self.plan.beside.join("staged.json"));
        self.finish_with(OutcomeKind::WentBack, why);
        self.step("outcome_written");
        let _ = std::fs::remove_file(self.files.launch_ok());
        let _ = std::fs::remove_file(self.files.launch_failed());
        let _ = std::fs::remove_file(self.files.helper_pid());
        if !self.open_app() {
            self.say("installer: the previous app couldn't be opened");
            return Ended::Stuck;
        }
        self.say(&format!("installer: went back to {} ({why})", self.plan.from));
        Ended::WentBack
    }

    /// This installer's whole-life lock. Another may be finishing, or standing down now that this plan's replaced its own, so it's worth a wait, and no longer than the wait for the old app is. The system lets go of it however this ends.
    fn take_lock(&self) -> Option<std::fs::File> {
        let file = local::open_private_for_writing(&self.files.installer_lock(), true).ok()?;
        let deadline = Instant::now() + Duration::from_millis(self.plan.quit_limit_ms);
        loop {
            if file.try_lock().is_ok() {
                return Some(file);
            }
            if Instant::now() >= deadline {
                return None;
            }
            std::thread::sleep(LOOK_STEP);
        }
    }
}

/// How the wait for the old app came out.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum OldApp {
    Gone,
    Stayed,
    PlanGone,
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

/// Runs an install. Takes the plan, as the app left it. Nothing's touched, not even what this run's files are called, without the lock and a plan that's its own.
#[must_use]
pub fn run(plan: &Plan) -> Ended {
    let run = Run {
        plan,
        files: Files::in_state(&plan.root),
    };
    let files = &run.files;
    if !may_replace(plan) {
        run.say("installer: that isn't a newer version, so nothing was replaced");
        return Ended::NotInstalled;
    }
    let Some(_lock) = run.take_lock() else {
        // It's the app that's told about this at its next launch, and nothing's been moved. The plan on disk is this one's, or there's a later one.
        run.say("installer: another installer's at work, so this one stands down");
        if run.plan_is_mine() {
            run.finish_with(OutcomeKind::NotInstalled, "another_installer");
        }
        return Ended::NotInstalled;
    };
    if !run.plan_is_mine() {
        run.say("installer: the plan isn't this one's any more, so this one stands down");
        return Ended::NotInstalled;
    }
    let _ = std::fs::write(files.helper_pid(), std::process::id().to_string());
    run.say(&format!(
        "installer: {} to {}, waiting for process {}, to reopen {}",
        plan.from,
        plan.to,
        plan.pid,
        if plan.background { "in the background" } else { "in front" }
    ));
    match run.wait_for_old_app() {
        OldApp::Gone => {}
        OldApp::Stayed => {
            run.say("installer: the old app didn't end in time, so nothing was replaced");
            run.finish_with(OutcomeKind::NotInstalled, "old_app_stayed");
            return Ended::NotInstalled;
        }
        OldApp::PlanGone => {
            run.say("installer: the app took the plan back, or another's replaced it, so nothing was replaced");
            return Ended::NotInstalled;
        }
    }
    let old_gone = crate::wall_ms();
    run.step("old_app_gone");
    // The app takes this away if it stayed after all, and it may have gone just as this was looking.
    if !run.plan_is_mine() {
        run.say("installer: the app took the plan back, or another's replaced it, so nothing was replaced");
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
    let _ = std::fs::remove_file(files.launch_failed());
    if !copy_record(&files.session(), &files.before(), &mut |step| die_if_told(step)) {
        // Not a reason to stop: it costs only what a going back would bring back.
        run.say("installer: the record couldn't be copied aside, so going back would bring back what's there");
    }
    run.step("record_copied");
    // The last look, and the last thing before the swap, so there's nothing slow between it and the swap for anyone to use. The release's own team, always, in a real build, whatever the plan says: a plan's only a file. Except a development build, which is signed ad hoc and is held to an intact signature, and only with the owner's switch on, which is read here again rather than taken from the plan.
    let team = if plan.development_build && builds_allowed(files) {
        None
    } else if RELEASE_BUILD || plan.team_required {
        Some(TEAM_ID)
    } else {
        None
    };
    if !crate::stage::still_what_was_verified(&plan.new_app(), &plan.cdhash, &plan.to, team) {
        run.say("installer: what's waiting isn't what was verified, so nothing was replaced");
        run.finish_with(OutcomeKind::NotInstalled, "changed_since_checked");
        let _ = std::fs::remove_dir_all(files.before());
        let _ = std::fs::remove_dir_all(plan.beside.join("new"));
        let _ = std::fs::remove_file(plan.beside.join("staged.json"));
        let _ = run.open_app();
        return Ended::NotInstalled;
    }
    let verified = crate::wall_ms();
    run.step("still_verified");
    if local::swap_paths(&plan.new_app(), &plan.app).is_err() {
        run.say("installer: the swap failed, so nothing was replaced");
        run.finish_with(OutcomeKind::NotInstalled, "swap_failed");
        let _ = run.open_app();
        return Ended::NotInstalled;
    }
    let swapped = crate::wall_ms();
    run.step("swapped");
    if keep_old_app(plan).is_err() {
        // The old app's still whole where it landed, and that's where going back or the next launch looks.
        run.say("installer: the old app couldn't be moved to be kept");
    }
    run.step("old_app_kept");
    if !run.open_app() {
        return run.go_back("couldnt_open");
    }
    run.step("opened");
    if let Err(why) = run.new_app_started() {
        return run.go_back(why);
    }
    run.say(&format!(
        "installer: {} started. Old app gone {} ms after the quit, the record's copy and the last look {} ms, swapped {} ms after that, up {} ms after that",
        plan.to,
        old_gone.saturating_sub(plan.quit_at_ms),
        verified.saturating_sub(old_gone),
        swapped.saturating_sub(verified),
        crate::wall_ms().saturating_sub(swapped)
    ));
    run.step("started");
    if !new_app_holds_the_old_one(plan) {
        let _ = std::fs::remove_dir_all(plan.beside.join("new"));
    }
    let _ = std::fs::remove_file(plan.beside.join("staged.json"));
    run.clear_own_files();
    Ended::Updated
}

/// *Allow development builds*, as the owner's switch says in the updater's own file.
fn builds_allowed(files: &Files) -> bool {
    crate::saved::Saved::read(&files.saved).dev_builds == Some(true)
}

/// Whether the plan's version may replace the one it came from: a newer one, or with [`Plan::any_version`] any other, but never itself.
fn may_replace(plan: &Plan) -> bool {
    if plan.any_version {
        crate::version::parse(&plan.to).is_some() && crate::version::parse(&plan.to) != crate::version::parse(&plan.from)
    } else {
        crate::rules::is_newer(&plan.to, &plan.from)
    }
}

/// Keeping the old app didn't work, so it's still in the swap's other half. It's the only way back, so it stays.
fn new_app_holds_the_old_one(plan: &Plan) -> bool {
    crate::stage::app_version(&plan.new_app()).as_deref() == Some(plan.from.as_str())
}

/// Goes back to the kept app with no install before it: what a launch does when the new version keeps failing with no installer around to see it.
#[must_use]
pub fn run_back(plan: &Plan) -> Ended {
    let run = Run {
        plan,
        files: Files::in_state(&plan.root),
    };
    let Some(_lock) = run.take_lock() else {
        run.say("installer: another installer's at work, so this one stands down");
        return Ended::NotInstalled;
    };
    if !run.plan_is_mine() {
        return Ended::NotInstalled;
    }
    let _ = std::fs::write(run.files.helper_pid(), std::process::id().to_string());
    run.say(&format!("installer: {} keeps failing, going back to {}", plan.to, plan.from));
    if run.wait_for_old_app() != OldApp::Gone {
        run.finish_with(OutcomeKind::NotInstalled, "old_app_stayed");
        return Ended::NotInstalled;
    }
    run.go_back("kept_failing")
}

fn keep_old_app(plan: &Plan) -> std::io::Result<()> {
    let keep = previous_app_in(&plan.beside);
    let folder = keep.parent().ok_or_else(|| std::io::Error::other("no folder"))?;
    if std::fs::remove_dir_all(folder).is_err() {
        let _ = std::fs::remove_file(folder);
    }
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

/// Most a copy of the record may come to when it has to be a real copy, on a disk that can't clone. Past this it's done without.
const MAX_REAL_COPY_BYTES: u64 = 256 * 1024 * 1024;

/// A clone of the record, as it stands. Costs no disk until something changes, and takes moments. On a disk that can't clone it's a copy, up to a limit, and if that's no good either it says so: no copy only means a going back brings back what's there.
pub fn copy_record(session: &Path, before: &Path, after_each: &mut dyn FnMut(&str)) -> bool {
    let _ = std::fs::remove_dir_all(before);
    let mut files = Vec::new();
    if walk_record(session, before, &mut files).is_err() {
        return false;
    }
    let mut copied = 0u64;
    for (from, to) in files {
        let made = to.parent().is_some_and(|folder| std::fs::create_dir_all(folder).is_ok());
        let fine = made
            && (local::clone_file(&from, &to).is_ok() || {
                copied = copied.saturating_add(std::fs::metadata(&from).map_or(u64::MAX, |meta| meta.len()));
                copied <= MAX_REAL_COPY_BYTES && std::fs::copy(&from, &to).is_ok()
            });
        if !fine {
            let _ = std::fs::remove_dir_all(before);
            return false;
        }
        after_each("copying_record");
    }
    std::fs::write(before.join(".whole"), "").is_ok()
}

/// Puts the copy back, one file at a time, each by a rename so none's ever half there, and takes away what's been added since. A copy that isn't whole is left alone. Stopped partway, it's the copy that still says what to do, and it's only cleared once a version's come up.
pub fn restore_record(before: &Path, session: &Path) {
    restore_record_with(before, session, &mut |step| die_if_told(step));
}

fn restore_record_with(before: &Path, session: &Path, after_each: &mut dyn FnMut(&str)) {
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
            if local::clone_file(from, &temp).is_ok() || std::fs::copy(from, &temp).is_ok() {
                let _ = std::fs::rename(&temp, to);
            }
        }
        after_each("restoring_record");
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
        Plan {
            id: "1-1".into(),
            cdhash: String::new(),
            team_required: false,
            any_version: false,
            development_build: false,
            team_signed: false,
            from: "0.1.0".into(),
            to: "0.2.0".into(),
            pid: 1,
            app: "/Apps/a b/RectoPDF.app".into(),
            beside: "/Apps/a b/.RectoPDF-update".into(),
            root: "/s".into(),
            quit_at_ms: 0,
            background: false,
            quit_limit_ms: 1,
            start_limit_ms: 1,
            test: None,
        }
    }

    /// Failure: an app in the background comes back taking the focus from whatever the user's doing, or a frontmost one comes back behind it. The owner's rule: "an app in the background reopens without taking focus". And a path with spaces and quotes stays one argument.
    #[test]
    fn the_app_comes_back_as_it_was_frontmost_or_not() {
        assert_eq!(open_arguments(&plan(), None), vec!["/Apps/a b/RectoPDF.app".to_string()], "frontmost");
        assert_eq!(
            open_arguments(&Plan { background: true, ..plan() }, None),
            vec!["-g".to_string(), "/Apps/a b/RectoPDF.app".to_string()],
            "in the background"
        );
        let test = crate::plan::TestLaunch {
            args: vec!["--qa".into()],
            env: vec![("A".into(), "b c".into())],
            direct: false,
        };
        assert_eq!(
            open_arguments(&Plan { background: true, ..plan() }, Some(&test)),
            ["-g", "--env", "A=b c", "/Apps/a b/RectoPDF.app", "--args", "--qa"]
                .map(str::to_string)
                .to_vec()
        );
    }

    /// Failure: the copy of the record misses part of it, brings back a history of a closed PDF it never took, or leaves behind what a broken new version added.
    #[test]
    fn a_record_comes_back_as_it_was() {
        let dir = std::env::temp_dir()
            .canonicalize()
            .unwrap()
            .join(format!("recto-install-{}-record", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let (session, before) = (dir.join("session"), dir.join("before"));
        std::fs::create_dir_all(session.join("journals")).unwrap();
        std::fs::write(session.join("record.a.json"), "good a").unwrap();
        std::fs::write(session.join("journals/journal-1.log"), "good journal").unwrap();
        std::fs::write(session.join("journals/closed-1.log"), "a closed one").unwrap();
        assert!(copy_record(&session, &before, &mut |_| {}));
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
        assert_eq!(
            std::fs::read_to_string(session.join("journals/closed-1.log")).unwrap(),
            "a closed one",
            "not taken, and not touched"
        );
        // A copy that isn't whole changes nothing.
        std::fs::remove_file(before.join(".whole")).unwrap();
        std::fs::write(session.join("record.a.json"), "newer").unwrap();
        restore_record(&before, &session);
        assert_eq!(std::fs::read_to_string(session.join("record.a.json")).unwrap(), "newer");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// Failure: the installer takes an older version over a newer one from a plan that didn't ask for that, or refuses the one plan that does (a development build handed over, or the way back to the release), or installs a version over itself.
    #[test]
    fn only_a_plan_for_any_version_goes_back_a_version() {
        assert!(may_replace(&plan()), "newer");
        let older = Plan {
            from: "0.1.0.3".into(),
            to: "0.1.0".into(),
            ..plan()
        };
        assert!(!may_replace(&older), "older, unasked");
        assert!(
            may_replace(&Plan {
                any_version: true,
                ..older.clone()
            }),
            "the way back"
        );
        assert!(may_replace(&Plan {
            from: "0.1.0".into(),
            to: "0.1.0.1".into(),
            ..plan()
        }));
        for same in [false, true] {
            assert!(!may_replace(&Plan {
                from: "0.1.0.3".into(),
                to: "0.1.0.3".into(),
                any_version: same,
                ..plan()
            }));
        }
    }
}
