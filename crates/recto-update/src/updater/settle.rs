//! **What the last install left, settled at a launch that keeps the record:** the installer's outcome or its plan read, said and tidied away. Moved here
//! unchanged from `updater.rs`, for its size.

use super::{Notice, QUITS_FOR_NOTHING, Settled, Status, Updater, builds, clean_code, helper_alive, installer_reopened, went_back_words, why_not_installed};
use crate::APP_NAME;
use crate::plan::{Files, Outcome, OutcomeKind, Plan, beside_app};
use crate::saved::Saved;
use crate::stage;
use std::path::Path;

impl Updater {
    /// What the last install left behind. The installer tidies up when it finishes: this is for when it didn't get to.
    pub(super) fn settle_last_install(&self, saved: &mut Saved, current: &str, app: &Path, now: u64) -> Settled {
        let mut settled = Settled::default();
        let Some(beside) = beside_app(app) else { return settled };
        let (outcome, earlier_refusal) = launch_outcome(&self.files, current, app);
        if let Some(outcome) = outcome {
            let exit_unproven = outcome.kind == OutcomeKind::CouldntGoBack && outcome.why == "app_exit_unproven";
            if exit_unproven {
                // This unresolved refusal must not immediately launch another rollback and quit again.
                saved.unstarted = 0;
                saved.probation = None;
            } else {
                let _ = std::fs::remove_file(self.files.outcome());
                let _ = std::fs::remove_file(self.files.plan());
                let _ = std::fs::remove_file(self.files.helper_pid());
            }
            // Gone back with it put back, the copy's for nothing. Stuck on a version that didn't come up, it's the state before it, and stays. Gone back with the record as it was, it's what to fall back on if the previous version can't read that, and it goes after its week as ever.
            let kept_ending = outcome.kind == OutcomeKind::WentBack && outcome.why == "kept_ending";
            if kept_ending {
                settled.fall_back = self.files.before().join(".whole").exists().then(|| self.files.before());
            } else if outcome.kind != OutcomeKind::CouldntGoBack {
                let _ = std::fs::remove_dir_all(self.files.before());
            }
            settled.reopened = match outcome.kind {
                OutcomeKind::WentBack => true,
                OutcomeKind::NotInstalled => installer_reopened(&outcome.why),
                OutcomeKind::CouldntGoBack => false,
            };
            settled.changed = true;
            settled.plan_gone = !exit_unproven;
            match outcome.kind {
                OutcomeKind::WentBack => {
                    saved.note_failed_version(&outcome.to);
                    saved.install_failures = 0;
                    settled.status = Some(Status::WentBack {
                        version: outcome.to.clone(),
                        back_on: current.to_owned(),
                    });
                    settled.notice = Some(Notice {
                        text: went_back_words(&outcome.why, &outcome.to, current),
                        fades: true,
                    });
                    self.log(&format!("{} didn't work ({}), and {current} came back", outcome.to, clean_code(&outcome.why)));
                }
                OutcomeKind::CouldntGoBack => {
                    saved.note_failed_version(&outcome.to);
                    let why = if exit_unproven {
                        "the new app may still be running, so the earlier version was not put back"
                    } else {
                        "it didn't come up, and there was no earlier version to go back to"
                    };
                    settled.status = Some(Status::Failed {
                        version: outcome.to.clone(),
                        why: why.to_owned(),
                    });
                    settled.notice = Some(Notice {
                        text: format!("{APP_NAME} {} is what's installed, and {why}.", outcome.to),
                        fades: true,
                    });
                    self.log(&format!("{} didn't come up, and couldn't go back: {}", outcome.to, clean_code(&outcome.why)));
                }
                OutcomeKind::NotInstalled => {
                    saved.install_failures = saved.install_failures.saturating_add(1);
                    saved.quits_for_nothing = saved.quits_for_nothing.saturating_add(1);
                    saved.last_install_ms = Some(now);
                    let (why, lasting) = why_not_installed(&outcome.why);
                    // Whatever it was, the app went away and came back for nothing, and the user may have seen that. Say so, and don't do it for ever.
                    if lasting || saved.quits_for_nothing >= QUITS_FOR_NOTHING {
                        saved.note_failed_version(&outcome.to);
                    }
                    settled.status = Some(Status::Failed {
                        version: outcome.to.clone(),
                        why: why.to_owned(),
                    });
                    settled.notice = Some(Notice {
                        text: format!("Couldn't update to {}: {why}. {APP_NAME} is unchanged.", outcome.to),
                        fades: true,
                    });
                    self.log(&format!("{} wasn't installed: {}", outcome.to, clean_code(&outcome.why)));
                }
            }
            return settled;
        }
        let Some(plan) = Plan::read(&self.files.plan()) else {
            settled.plan_gone = true;
            return settled;
        };
        settled.changed = true;
        let alive = helper_alive(&self.files);
        if plan.to == current {
            settled.status = Some(Status::Updated(plan.to.clone()));
            settled.notice = Some(Notice {
                text: builds::arrived_words(&plan),
                fades: true,
            });
            saved.offered = None;
            saved.install_failures = 0;
            saved.quits_for_nothing = 0;
            saved.failures = 0;
            if !alive && !earlier_refusal {
                self.tidy_after_swap(&plan, &beside);
            }
            settled.arrived = Some(plan);
        } else if alive {
            // Reopened by someone in the moment it was gone, with the installer still about. It stands down when it sees this, and there's nothing here to wait for: this copy goes on as normal, and the next launch hears how it ended.
            self.log("the app was reopened while an installer was at work");
        } else {
            // The installer was stopped before it swapped, so this is the old app, and there's no harm done.
            saved.install_failures = saved.install_failures.saturating_add(1);
            saved.quits_for_nothing = saved.quits_for_nothing.saturating_add(1);
            saved.last_install_ms = Some(now);
            if saved.quits_for_nothing >= QUITS_FOR_NOTHING {
                saved.note_failed_version(&plan.to);
            }
            settled.status = Some(Status::Failed {
                version: plan.to.clone(),
                why: "the installer stopped before it finished".to_owned(),
            });
            settled.notice = Some(Notice {
                text: format!(
                    "Couldn't update to {}: the installer stopped before it finished. {APP_NAME} is unchanged.",
                    plan.to
                ),
                fades: true,
            });
            self.log(&format!("the install of {} stopped before it finished, and {current} is unchanged", plan.to));
            let _ = std::fs::remove_file(self.files.plan());
            let _ = std::fs::remove_file(self.files.helper_pid());
            let _ = std::fs::remove_dir_all(self.files.before());
            stage::clear_staged(app);
        }
        settled
    }
}

/// A refusal describes one version pair, never a later install's startup handshake.
fn launch_outcome(files: &Files, current: &str, app: &Path) -> (Option<Outcome>, bool) {
    let Some(outcome) = Outcome::read(&files.outcome()) else {
        return (None, false);
    };
    if outcome.kind != OutcomeKind::CouldntGoBack || outcome.why != "app_exit_unproven" {
        return (Some(outcome), false);
    }
    let superseded = Plan::read(&files.plan()).is_some_and(|plan| {
        plan.to == current
            && plan.from != plan.to
            && plan.root == files.root
            && plan.app.as_path() == app
            && (plan.from != outcome.from || plan.to != outcome.to)
    });
    if current == outcome.to && !superseded {
        return (Some(outcome), false);
    }
    // One fixed, bounded evidence slot. Failure to archive cannot mask a valid later arrival.
    let archived = serde_json::to_vec(&outcome).map_err(std::io::Error::other).and_then(|bytes| {
        if bytes.len() > 64 * 1024 {
            return Err(std::io::Error::other("refused outcome exceeded its evidence cap"));
        }
        recto_platform::local::write_private_file_durably(&files.work.join("outcome.refused.json"), &bytes)
    });
    match archived {
        Ok(()) => {
            if Outcome::read(&files.outcome()).as_ref() == Some(&outcome) {
                let _ = std::fs::remove_file(files.outcome());
            }
        }
        Err(error) => crate::plan::log(&files.log, &format!("the earlier rollback refusal is retained in outcome.json: {error}")),
    }
    (None, true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plan::TestLaunch;
    use crate::updater::{HandOffRefused, HandedOff, Host, Look};
    use std::sync::Arc;
    use std::time::Duration;

    struct Quiet;
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
    }

    /// Failure: an earlier refused rollback masks a valid later plan, so its healthy first frame has no arrival to acknowledge.
    #[test]
    fn an_old_refusal_does_not_mask_a_new_matching_arrival() {
        for archive_fails in [false, true] {
            let dir = recto_testpdf::TempDir::new("new-arrival-after-refusal-");
            let app = dir.join("RectoPDF.app");
            let root = dir.join("state");
            let files = crate::plan::Files::in_state(&root);
            recto_platform::local::ensure_private_dir(&root).unwrap();
            recto_platform::local::ensure_private_dir(&files.work).unwrap();
            std::fs::create_dir_all(files.before()).unwrap();
            std::fs::write(files.before().join(".whole"), b"earlier record").unwrap();
            std::fs::create_dir(&app).unwrap();
            std::fs::write(app.join("untouched"), b"new bundle").unwrap();
            let plan = Plan {
                id: "later".into(),
                cdhash: String::new(),
                team_required: false,
                any_version: false,
                development_build: false,
                team_signed: false,
                from: "0.2.0".into(),
                to: "0.3.0".into(),
                pid: 0,
                app: app.clone(),
                beside: beside_app(&app).unwrap(),
                root: root.clone(),
                quit_at_ms: 0,
                background: true,
                quit_limit_ms: 1,
                start_limit_ms: 1,
                handed_off_tabs: None,
                open: Vec::new(),
                keep_record: false,
                test: None,
            };
            plan.write(&files.plan()).unwrap();
            let old = Outcome {
                kind: OutcomeKind::CouldntGoBack,
                from: "0.1.0".into(),
                to: "0.2.0".into(),
                why: "app_exit_unproven".into(),
            };
            old.write(&files.outcome()).unwrap();
            let archive = files.work.join("outcome.refused.json");
            if archive_fails {
                std::fs::create_dir(&archive).unwrap();
            }
            let updater = Updater::new(&root, crate::source::resolve(false, false, None, None), Arc::new(Quiet));
            let settled = updater.settle_last_install(&mut Saved::default(), &plan.to, &app, 0);
            assert_eq!(
                settled.arrived.as_ref().map(|arrived| (&arrived.from, &arrived.to)),
                Some((&plan.from, &plan.to)),
                "old refusal must not suppress launch-ok eligibility"
            );
            assert_eq!(Plan::read(&files.plan()), Some(plan));
            assert_eq!(std::fs::read(files.before().join(".whole")).unwrap(), b"earlier record");
            assert_eq!(std::fs::read(app.join("untouched")).unwrap(), b"new bundle");
            let evidence = if archive_fails { files.outcome() } else { archive };
            assert_eq!(Outcome::read(&evidence), Some(old));
        }
    }
}
