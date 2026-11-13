//! **What the last install left, settled at a launch that keeps the record:** the installer's outcome or its plan read, said and tidied away. Moved here
//! unchanged from `updater.rs`, for its size.

use super::{Notice, QUITS_FOR_NOTHING, Settled, Status, Updater, builds, clean_code, helper_alive, installer_reopened, went_back_words, why_not_installed};
use crate::APP_NAME;
use crate::plan::{Outcome, OutcomeKind, Plan, beside_app};
use crate::saved::Saved;
use crate::stage;
use std::path::Path;

impl Updater {
    /// What the last install left behind. The installer tidies up when it finishes: this is for when it didn't get to.
    pub(super) fn settle_last_install(&self, saved: &mut Saved, current: &str, app: &Path, now: u64) -> Settled {
        let mut settled = Settled::default();
        let Some(beside) = beside_app(app) else { return settled };
        if let Some(outcome) = Outcome::read(&self.files.outcome()) {
            let _ = std::fs::remove_file(self.files.outcome());
            let _ = std::fs::remove_file(self.files.plan());
            let _ = std::fs::remove_file(self.files.helper_pid());
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
            settled.plan_gone = true;
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
                    let why = "it didn't come up, and there was no earlier version to go back to";
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
            if !alive {
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
