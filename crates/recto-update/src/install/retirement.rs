//! Before rollback, retire only an owned direct launch and prove the executable absent.
use recto_platform::local;
use std::cell::{Cell, RefCell};
use std::io;
use std::path::Path;
use std::process::Child;
use std::time::{Duration, Instant};

#[derive(Default)]
pub(super) struct Launched {
    child: RefCell<Option<Child>>,
    pending: Cell<bool>,
}

impl Launched {
    pub(super) fn start(&self, program: &Path, args: &[String], env: &[(String, String)], log: &Path) -> io::Result<()> {
        let mut child = self.child.borrow_mut();
        if child.is_some() {
            return Err(io::Error::other("a direct launch is already retained"));
        }
        *child = Some(local::spawn_detached_child(program, args, env, log)?);
        Ok(())
    }

    pub(super) fn requested(&self) {
        self.pending.set(true);
    }

    pub(super) fn running(&self, program: &Path) -> io::Result<bool> {
        let live = local::update_app_is_running(program)?;
        if live {
            self.pending.set(false);
        }
        Ok(live)
    }

    pub(super) fn recover(&self, root: &Path, program: &Path, patience: Duration) -> io::Result<RecordLock> {
        let mut child = self.child.borrow_mut();
        let held = recover_within(
            child.as_mut(),
            root,
            || self.running(program),
            self.pending.get(),
            patience,
            Instant::now,
            std::thread::sleep,
        )?;
        *child = None;
        Ok(held)
    }
}

/// The whole rollback wait: retire the owned launch, prove the app absent, then take the record's lock and recheck.
/// The recheck ends at the deadline of the absence proof, so the observation has one patience in all, after the
/// owned child's own wait.
fn recover_within<C: Owned>(
    child: Option<&mut C>,
    root: &Path,
    mut present: impl FnMut() -> io::Result<bool>,
    pending: bool,
    patience: Duration,
    mut now: impl FnMut() -> Instant,
    mut pause: impl FnMut(Duration),
) -> io::Result<RecordLock> {
    let deadline = retire(child, &mut present, pending, patience, &mut now, &mut pause)?;
    record_lock(root, present, deadline, now, pause)
}

#[derive(Default)]
pub(super) struct Startup {
    seen: bool,
    gone_since: Option<Instant>,
    unknown: Option<io::Error>,
}

impl Startup {
    pub(super) fn check(
        &mut self,
        observed: io::Result<bool>,
        now: Instant,
        elapsed: Duration,
        never_seen: Duration,
        grace: Duration,
    ) -> Result<(), &'static str> {
        match observed {
            Err(error) => {
                self.gone_since = None;
                self.unknown = Some(error);
                // Unknown is neither an ended app nor a break in its known-live history.
                Ok(())
            }
            Ok(true) => {
                self.unknown = None;
                self.seen = true;
                self.gone_since = None;
                Ok(())
            }
            Ok(false) => {
                self.unknown = None;
                let since = *self.gone_since.get_or_insert(now);
                if (self.seen && now.duration_since(since) >= grace) || (!self.seen && elapsed >= never_seen) {
                    Err("didnt_start")
                } else {
                    Ok(())
                }
            }
        }
    }

    pub(super) fn last_unknown(&self) -> Option<&io::Error> {
        self.unknown.as_ref()
    }
}

pub(super) fn before_swap(program: &Path, patience: Duration) -> io::Result<bool> {
    known(|| local::update_app_is_running(program), patience, Instant::now, std::thread::sleep)
}

fn known(
    mut probe: impl FnMut() -> io::Result<bool>,
    patience: Duration,
    mut now: impl FnMut() -> Instant,
    mut pause: impl FnMut(Duration),
) -> io::Result<bool> {
    let deadline = now() + patience;
    let mut answer = None;
    let mut unknown = None;
    until_deadline(deadline, &mut now, &mut pause, || match probe() {
        Ok(live) => {
            answer = Some(live);
            Ok(true)
        }
        Err(error) => {
            unknown = Some(error);
            Ok(false)
        }
    })
    .map_err(|error| unknown.unwrap_or(error))?;
    answer.ok_or_else(|| io::Error::other("no definite app observation"))
}

pub(super) struct RecordLock(std::fs::File);

impl Drop for RecordLock {
    fn drop(&mut self) {
        let _ = self.0.unlock();
    }
}

/// Takes the record's lock and rechecks, until `deadline` (the one the exit wait's absence proof had), that no app is
/// running. The lock is held only while an observation is made: on a definite absence it is kept and returned; on an
/// unknown it is released before the pause and taken again for the next look, so the app's own short lock patience
/// (600 ms) is never spent against rollback's long one. An unknown is "not yet known" as in `known`, so one transient
/// unknown is not reported as "couldn't go back" (`app_exit_unproven`). A live app, a lock another writer holds, or no
/// definite answer by the deadline, refuses.
pub(super) fn record_lock(
    root: &Path,
    mut present: impl FnMut() -> io::Result<bool>,
    deadline: Instant,
    mut now: impl FnMut() -> Instant,
    mut pause: impl FnMut(Duration),
) -> io::Result<RecordLock> {
    loop {
        let held = RecordLock(local::try_update_record_lock(root)?);
        let unknown = match present() {
            Ok(false) => return Ok(held),
            Ok(true) => return Err(io::Error::other("an app appeared before rollback acquired the record")),
            Err(error) => error,
        };
        drop(held);
        let left = deadline.saturating_duration_since(now());
        if left.is_zero() {
            return Err(unknown);
        }
        pause(left.min(Duration::from_millis(20)));
    }
}

trait Owned {
    fn exited(&mut self) -> io::Result<bool>;
    fn kill(&mut self) -> io::Result<()>;
}

impl Owned for Child {
    fn exited(&mut self) -> io::Result<bool> {
        self.try_wait().map(|status| status.is_some())
    }
    fn kill(&mut self) -> io::Result<()> {
        Child::kill(self)
    }
}

fn retire<C: Owned>(
    child: Option<&mut C>,
    mut present: impl FnMut() -> io::Result<bool>,
    pending: bool,
    patience: Duration,
    mut now: impl FnMut() -> Instant,
    mut pause: impl FnMut(Duration),
) -> io::Result<Instant> {
    let mut awaiting_launch = pending && child.is_none();
    if let Some(child) = child {
        // An error, including lost ownership, never authorizes a signal.
        if !child.exited()? {
            child.kill()?;
        }
        until(patience, &mut now, &mut pause, || child.exited())?;
    }
    // Retry only OS-observation uncertainty, within the same deadline. Child ownership errors above are final.
    let deadline = now() + patience;
    let mut unknown = None;
    let observed = until_deadline(deadline, &mut now, &mut pause, || match present() {
        Ok(true) => {
            awaiting_launch = false;
            Ok(false)
        }
        Ok(false) if awaiting_launch => Err(io::Error::other("the requested app launch has not yet been observed")),
        Ok(false) => Ok(true),
        Err(error) => {
            unknown = Some(error);
            Ok(false)
        }
    });
    observed.map(|()| deadline).map_err(|error| unknown.unwrap_or(error))
}

fn until(patience: Duration, now: &mut impl FnMut() -> Instant, pause: &mut impl FnMut(Duration), done: impl FnMut() -> io::Result<bool>) -> io::Result<()> {
    let deadline = now() + patience;
    until_deadline(deadline, now, pause, done)
}

fn until_deadline(
    deadline: Instant,
    now: &mut impl FnMut() -> Instant,
    pause: &mut impl FnMut(Duration),
    mut done: impl FnMut() -> io::Result<bool>,
) -> io::Result<()> {
    loop {
        if done()? {
            return Ok(());
        }
        let left = deadline.saturating_duration_since(now());
        if left.is_zero() {
            return Err(io::Error::new(io::ErrorKind::TimedOut, "app exit could not be established"));
        }
        pause(left.min(Duration::from_millis(20)));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(super) struct Fake {
        error: bool,
        reaped: bool,
        kills: u32,
        kill_completes: bool,
    }
    impl Owned for Fake {
        fn exited(&mut self) -> io::Result<bool> {
            if self.error {
                Err(io::Error::other("ownership unknown"))
            } else {
                Ok(self.reaped)
            }
        }
        fn kill(&mut self) -> io::Result<()> {
            self.kills += 1;
            self.reaped = self.kill_completes;
            Ok(())
        }
    }

    /// Failure: one unrelated process becoming unclassifiable aborts an otherwise valid install before a definite answer.
    #[test]
    fn pre_swap_unknown_waits_for_a_definite_answer() {
        for live in [false, true] {
            let clock = Cell::new(Instant::now());
            let mut answers = [Err(io::Error::other("transient unknown")), Ok(live)].into_iter();
            assert_eq!(
                known(
                    || answers.next().unwrap(),
                    Duration::from_millis(1),
                    || clock.get(),
                    |step| clock.set(clock.get() + step)
                )
                .unwrap(),
                live
            );
        }
        let clock = Cell::new(Instant::now());
        let began = clock.get();
        assert!(
            known(
                || Err(io::Error::other("persistent unknown")),
                Duration::from_millis(1),
                || clock.get(),
                |step| clock.set(clock.get() + step)
            )
            .is_err()
        );
        assert!(clock.get().duration_since(began) <= Duration::from_millis(1));
    }

    /// Failure: a transient unknown ends startup, forgets a prior live app, or counts unknown time as proven absence.
    #[test]
    fn startup_unknown_preserves_live_history_until_known_gone() {
        let start = Instant::now();
        let never = Duration::from_millis(1);
        let grace = Duration::from_millis(3);
        let mut state = Startup::default();
        let mut check = |observed, ms| state.check(observed, start + Duration::from_millis(ms), Duration::from_millis(ms), never, grace);
        assert!(check(Ok(true), 0).is_ok());
        assert!(check(Err(io::Error::other("unknown")), 1).is_ok());
        assert!(check(Ok(false), 2).is_ok(), "the previously live app keeps its full gone grace");
        assert!(check(Err(io::Error::other("unknown again")), 4).is_ok());
        assert!(check(Ok(false), 5).is_ok(), "unknown time is not proven absence");
        assert!(check(Ok(false), 7).is_ok());
        assert_eq!(check(Ok(false), 8), Err("didnt_start"));
    }

    /// Failure: an observed live/unknown app is treated as stopped, or a lost owned Child is signalled.
    #[test]
    fn only_reaped_owned_children_and_observed_absence_allow_rollback() {
        for live in [true, false] {
            let result = retire::<Fake>(None, || Ok(live), false, Duration::ZERO, Instant::now, |_| panic!("zero patience waits"));
            assert_eq!(result.is_ok(), !live);
        }
        assert!(
            retire::<Fake>(
                None,
                || Err(io::Error::other("unknown")),
                false,
                Duration::ZERO,
                Instant::now,
                |_| panic!("unknown waits")
            )
            .is_err()
        );
        assert!(retire::<Fake>(None, || Ok(false), true, Duration::ZERO, Instant::now, |_| panic!("pending waits")).is_err());
        let mut observations = [true, false].into_iter();
        let observed_clock = Cell::new(Instant::now());
        assert!(
            retire::<Fake>(
                None,
                || Ok(observations.next().unwrap()),
                true,
                Duration::from_millis(1),
                || observed_clock.get(),
                |step| observed_clock.set(observed_clock.get() + step)
            )
            .is_ok()
        );
        let clock = Cell::new(Instant::now());
        let mut answers = [Err(io::Error::other("transient unknown")), Ok(false)].into_iter();
        assert!(
            retire::<Fake>(
                None,
                || answers.next().unwrap(),
                false,
                Duration::from_millis(1),
                || clock.get(),
                |step| clock.set(clock.get() + step)
            )
            .is_ok(),
            "a later complete absence query resolves transient uncertainty"
        );
        let began = clock.get();
        assert!(
            retire::<Fake>(
                None,
                || Err(io::Error::other("persistent unknown")),
                false,
                Duration::from_millis(1),
                || clock.get(),
                |step| clock.set(clock.get() + step)
            )
            .is_err()
        );
        assert!(clock.get().duration_since(began) <= Duration::from_millis(1));
        let mut lost = Fake {
            error: true,
            reaped: false,
            kills: 0,
            kill_completes: true,
        };
        assert!(retire(Some(&mut lost), || Ok(false), false, Duration::ZERO, Instant::now, |_| panic!("lost waits")).is_err());
        assert_eq!(lost.kills, 0);
        let mut owned = Fake {
            error: false,
            reaped: false,
            kills: 0,
            kill_completes: true,
        };
        assert!(retire(Some(&mut owned), || Ok(false), false, Duration::ZERO, Instant::now, |_| panic!("reaped waits")).is_ok());
        assert_eq!(owned.kills, 1);
        let mut stubborn = Fake {
            error: false,
            reaped: false,
            kills: 0,
            kill_completes: false,
        };
        assert!(
            retire(
                Some(&mut stubborn),
                || Ok(false),
                false,
                Duration::ZERO,
                Instant::now,
                |_| panic!("stubborn waits")
            )
            .is_err()
        );
        assert_eq!(stubborn.kills, 1);
    }
}

#[cfg(all(test, target_os = "macos"))]
mod recovery_tests {
    use super::*;
    use crate::install::{Ended, Run};
    use crate::plan::{Files, Outcome, OutcomeKind, TestLaunch};
    use crate::saved::{Probation, Saved};
    use crate::updater::{AtLaunch, Facts, HandOffRefused, HandedOff, Host, InstallerMode, Look, Updater};
    use std::sync::{
        Arc,
        atomic::{AtomicU32, Ordering},
    };

    #[derive(Default)]
    struct Quiet(AtomicU32);
    impl Host for Quiet {
        fn look(&self) -> Look {
            Look::default()
        }
        fn hand_off(&self, _: Duration) -> Result<HandedOff, HandOffRefused> {
            Ok(HandedOff::default())
        }
        fn quit_now(&self) -> bool {
            false
        }
        fn cancelled(&self) {}
        fn changed(&self) {}
        fn frontmost(&self) -> bool {
            false
        }
        fn relaunch(&self) -> TestLaunch {
            TestLaunch::default()
        }
        fn start_installer(&self, _: InstallerMode, _: &Path, _: &Path, _: &Path) -> io::Result<u32> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Err(io::Error::other("a filesystem-only test never starts an installer"))
        }
    }

    /// Failure: one transient unknown observation of the app during rollback's recheck is reported as "couldn't go
    /// back" (`app_exit_unproven`) instead of being rechecked within the patience the exit wait already has.
    #[test]
    fn the_record_lock_recheck_polls_past_a_transient_unknown_within_its_patience() {
        let dir = recto_testpdf::TempDir::new("record-lock-recheck-");
        let clock = std::cell::Cell::new(Instant::now());
        let patience = Duration::from_millis(100);
        let run = |mut answers: Vec<io::Result<bool>>, repeat: Option<fn() -> io::Result<bool>>| {
            answers.reverse();
            record_lock(
                dir.path(),
                || answers.pop().or_else(|| repeat.map(|again| again())).expect("probe asked past its answers"),
                clock.get() + patience,
                || clock.get(),
                |step| clock.set(clock.get() + step),
            )
            .map(drop)
        };
        assert!(run(vec![Err(io::Error::other("transient unknown")), Ok(false)], None).is_ok());
        let began = clock.get();
        let error = run(vec![], Some(|| Err(io::Error::other("persistent unknown")))).unwrap_err();
        assert!(error.to_string().contains("persistent unknown"), "{error}");
        assert!(
            clock.get().duration_since(began) <= patience,
            "no new limit: the recheck stays within its patience"
        );
        let error = run(vec![Ok(true)], None).unwrap_err();
        assert!(error.to_string().contains("appeared"), "a live app refuses: {error}");
    }

    /// Failure: the rollback wait has two windows, the exit wait's and a fresh one for the record lock's recheck, so
    /// when absence is proven late and the recheck then finds only unknowns it waits up to twice its patience
    /// before it says "couldn't go back".
    #[test]
    fn the_whole_rollback_wait_ends_within_one_patience_when_the_recheck_is_unknown() {
        let dir = recto_testpdf::TempDir::new("rollback-one-patience-");
        let clock = std::cell::Cell::new(Instant::now());
        let began = clock.get();
        let patience = Duration::from_millis(100);
        let mut proven = false;
        let error = recover_within::<super::tests::Fake>(
            None,
            dir.path(),
            || {
                if !proven && clock.get().duration_since(began) >= patience * 9 / 10 {
                    proven = true;
                    Ok(false)
                } else {
                    Err(io::Error::other("unknown"))
                }
            },
            false,
            patience,
            || clock.get(),
            |step| clock.set(clock.get() + step),
        )
        .map(drop)
        .unwrap_err();
        assert!(proven && error.to_string().contains("unknown"), "{error}");
        let waited = clock.get().duration_since(began);
        assert!(waited <= patience, "waited {waited:?} of {patience:?}");
    }

    /// Failure: the recheck holds `run/session.lock` for its whole patience, so a person opening RectoPDF during an
    /// unknown observation (the app's own lock patience is 600 ms) starts with an empty record beside the saved one.
    #[test]
    fn the_record_lock_is_not_held_between_unknown_observations() {
        let dir = recto_testpdf::TempDir::new("record-lock-free-between-");
        let clock = std::cell::Cell::new(Instant::now());
        let free = |what: &str| drop(local::try_update_record_lock(dir.path()).unwrap_or_else(|error| panic!("{what}: {error}")));
        let mut answers = vec![Ok(false), Err(io::Error::other("unknown")), Err(io::Error::other("unknown"))];
        let held = record_lock(
            dir.path(),
            || answers.pop().expect("asked past its answers"),
            clock.get() + Duration::from_secs(1),
            || clock.get(),
            |step| {
                free("the lock must be free while waiting");
                clock.set(clock.get() + step);
            },
        )
        .unwrap();
        assert!(
            local::try_update_record_lock(dir.path()).is_err(),
            "on a definite absence the lock is held when it returns"
        );
        drop(held);
        free("released with the guard");
    }

    fn version(app: &Path, version: &str) -> Vec<u8> {
        std::fs::create_dir_all(app.join("Contents")).unwrap();
        let bytes =
            format!(r#"<?xml version="1.0"?><plist version="1.0"><dict><key>CFBundleShortVersionString</key><string>{version}</string></dict></plist>"#)
                .into_bytes();
        std::fs::write(app.join("Contents/Info.plist"), &bytes).unwrap();
        bytes
    }

    /// Failure: rollback restores record bytes or swaps bundles with a live/unknown writer, or the next launch discards recovery or retries and quits.
    #[test]
    fn unproven_exit_preserves_record_bundles_and_next_launch_recovery() {
        let mut mutated = Vec::new();
        for case in ["live", "unknown", "locked", "reopened", "recheck_unknown"] {
            let dir = recto_testpdf::TempDir::new("rollback-refusal-");
            let mut plan = crate::install::tests::plan();
            plan.app = dir.join("RectoPDF.app");
            plan.beside = crate::plan::beside_app(&plan.app).unwrap();
            plan.root = dir.join("state");
            plan.test = Some(TestLaunch {
                direct: true,
                ..TestLaunch::default()
            });
            let current = version(&plan.app, &plan.to);
            let previous = version(&plan.previous_app(), &plan.from);
            let files = Files::in_state(&plan.root);
            local::ensure_private_dir(&plan.root).unwrap();
            local::ensure_private_dir(&files.work).unwrap();
            std::fs::create_dir_all(files.session()).unwrap();
            std::fs::create_dir_all(files.before()).unwrap();
            std::fs::write(files.session().join("record.a.json"), b"current owner edits").unwrap();
            std::fs::write(files.before().join("record.a.json"), b"earlier record").unwrap();
            std::fs::write(files.before().join(".whole"), b"").unwrap();
            plan.write(&files.plan()).unwrap();
            let run = Run {
                plan: &plan,
                files: files.clone(),
                launched: Launched::default(),
            };
            let _writer = (case == "locked").then(|| local::try_update_record_lock(&plan.root).unwrap());
            let refused = if matches!(case, "locked" | "reopened" | "recheck_unknown") {
                Ok(())
            } else {
                super::retire::<super::tests::Fake>(
                    None,
                    || if case == "unknown" { Err(io::Error::other("unknown")) } else { Ok(true) },
                    false,
                    Duration::ZERO,
                    Instant::now,
                    |_| panic!("zero patience waits"),
                )
                .map(drop)
            }
            .and_then(|()| {
                record_lock(
                    &plan.root,
                    || match case {
                        "reopened" => Ok(true),
                        "recheck_unknown" => Err(io::Error::other("recheck unknown")),
                        _ => Ok(false),
                    },
                    Instant::now(),
                    Instant::now,
                    |_| panic!("zero patience waits"),
                )
            });
            assert_eq!(run.go_back_after("misread", refused), Ended::Stuck);
            if std::fs::read(files.session().join("record.a.json")).ok().as_deref() != Some(b"current owner edits".as_slice())
                || std::fs::read(plan.app.join("Contents/Info.plist")).ok().as_deref() != Some(current.as_slice())
                || std::fs::read(plan.previous_app().join("Contents/Info.plist")).ok().as_deref() != Some(previous.as_slice())
            {
                mutated.push(case);
                continue;
            }
            assert_eq!(std::fs::read(files.session().join("record.a.json")).unwrap(), b"current owner edits");
            assert_eq!(std::fs::read(plan.app.join("Contents/Info.plist")).unwrap(), current);
            assert_eq!(std::fs::read(plan.previous_app().join("Contents/Info.plist")).unwrap(), previous);
            assert_eq!(std::fs::read(files.before().join("record.a.json")).unwrap(), b"earlier record");
            assert!(files.plan().exists());
            assert_eq!(Outcome::read(&files.outcome()).unwrap().kind, OutcomeKind::CouldntGoBack);
            Saved {
                unstarted: 1000,
                probation: Some(Probation {
                    version: plan.to.clone(),
                    early_ends: 1000,
                    ..Probation::default()
                }),
                ..Saved::default()
            }
            .write(&files.saved);
            let host = Arc::new(Quiet::default());
            let updater = Updater::new(
                &plan.root,
                crate::source::resolve(false, true, Some("http://127.0.0.1:9".into()), None),
                host.clone(),
            );
            assert_eq!(
                updater.launched(Facts {
                    version: plan.to.clone(),
                    app: plan.app.clone(),
                    system: "15.0".into()
                }),
                AtLaunch::GoOn
            );
            assert_eq!(host.0.load(Ordering::SeqCst), 0, "unresolved rollback must not retry and quit");
            let notice = updater.notice().unwrap().text;
            assert!(notice.contains("may still be running") && !notice.contains("no earlier version"), "{notice}");
            assert!(files.plan().exists() && files.outcome().exists() && files.before().join(".whole").exists());
            assert_eq!(std::fs::read(plan.previous_app().join("Contents/Info.plist")).unwrap(), previous);
            assert_eq!(std::fs::read(files.session().join("record.a.json")).unwrap(), b"current owner edits");
        }
        assert!(mutated.is_empty(), "rollback mutated record or bundles before exit proof: {mutated:?}");
    }
}
