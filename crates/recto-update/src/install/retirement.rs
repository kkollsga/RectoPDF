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
        self.retire(program, patience)?;
        record_lock(root, || self.running(program))
    }

    fn retire(&self, program: &Path, patience: Duration) -> io::Result<()> {
        let mut child = self.child.borrow_mut();
        retire(
            child.as_mut(),
            || self.running(program),
            self.pending.get(),
            patience,
            Instant::now,
            std::thread::sleep,
        )?;
        *child = None;
        Ok(())
    }
}

pub(super) struct RecordLock(std::fs::File);

impl Drop for RecordLock {
    fn drop(&mut self) {
        let _ = self.0.unlock();
    }
}

pub(super) fn record_lock(root: &Path, mut present: impl FnMut() -> io::Result<bool>) -> io::Result<RecordLock> {
    let held = RecordLock(local::try_update_record_lock(root)?);
    if present()? {
        return Err(io::Error::other("an app appeared before rollback acquired the record"));
    }
    Ok(held)
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
) -> io::Result<()> {
    if let Some(child) = child {
        // An error, including lost ownership, never authorizes a signal.
        if !child.exited()? {
            child.kill()?;
        }
        until(patience, &mut now, &mut pause, || child.exited())?;
    } else if pending && !present()? {
        return Err(io::Error::other("the requested app launch has not yet been observed"));
    }
    // open/Launch Services gives no Child: it may only be observed, never signalled.
    until(patience, &mut now, &mut pause, || present().map(|live| !live))
}

fn until(
    patience: Duration,
    now: &mut impl FnMut() -> Instant,
    pause: &mut impl FnMut(Duration),
    mut done: impl FnMut() -> io::Result<bool>,
) -> io::Result<()> {
    let deadline = now() + patience;
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
        assert!(
            retire::<Fake>(
                None,
                || Ok(observations.next().unwrap()),
                true,
                Duration::ZERO,
                Instant::now,
                |_| panic!("seen launch waits")
            )
            .is_ok()
        );
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
            }
            .and_then(|()| {
                record_lock(&plan.root, || match case {
                    "reopened" => Ok(true),
                    "recheck_unknown" => Err(io::Error::other("recheck unknown")),
                    _ => Ok(false),
                })
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
