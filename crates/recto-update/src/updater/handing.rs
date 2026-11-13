//! **The install's half of an update, carried out** (step R1.4): what [`flow::next`] says is done here, on the install's own thread, and what came of it is fed
//! back as the next phase. Nothing here decides: it asks the host what the phase reads, in the order it's read, and does what it's told.

use super::flow::{self, Going, Phase, Seen, Step};
use super::{Asked, Facts, HAND_OFF_LIMIT, HandOffRefused, HandedOff, Notice, Status, TEST_START_LIMIT_MS, Updater, WAIT_STEP, lock, test_launch};
use crate::HELPER;
use crate::plan::{Plan, beside_app};
use crate::source::Mode;
use std::sync::Arc;
use std::sync::atomic::Ordering;

impl Updater {
    /// The download's good. What's in flight finishes first, and the user's told if they're waiting on it. Then the hand-off and the quit: from here, what's written is what comes back. What happens at each look is [`flow::next`]'s; this asks the host what it needs and carries it out.
    pub(super) fn wait_and_hand_off(self: &Arc<Self>, asked: Asked, facts: &Facts, to: &Going<'_>) {
        let version = to.version;
        let began_ms = self.wall_ms();
        loop {
            let stopping = self.stopping.load(Ordering::SeqCst);
            let seen = self.seen(stopping, to.development_build);
            let mut step = flow::next(
                to,
                Phase::Waiting {
                    began_ms,
                    busy_at_hand_off: false,
                },
                &seen,
                asked,
                stopping,
                self.wall_ms(),
            );
            if step == Step::HandOff {
                step = match self.host.hand_off(HAND_OFF_LIMIT) {
                    Ok(handed) => {
                        self.quit_after_hand_off(asked, facts, to, handed);
                        return;
                    }
                    Err(HandOffRefused::NotSaved(why)) => {
                        // Nothing's been lost, and the app's taking everything again: the update just doesn't go ahead.
                        self.log(&format!("{version}: the state couldn't be written and read back"));
                        flow::next(to, Phase::NotSaved(&why), &seen, asked, stopping, self.wall_ms())
                    }
                    // Something's in flight that the look didn't see: this look waits, as for what it did.
                    Err(HandOffRefused::Busy(_)) => flow::next(
                        to,
                        Phase::Waiting {
                            began_ms,
                            busy_at_hand_off: true,
                        },
                        &seen,
                        asked,
                        stopping,
                        self.wall_ms(),
                    ),
                };
            }
            match step {
                Step::Wait { waiting_for } => {
                    self.say_waiting(version, waiting_for);
                    std::thread::sleep(WAIT_STEP); // ratchet: poll
                }
                Step::Failed { why, waiting_for } => {
                    self.say_waiting(version, waiting_for);
                    self.failed(asked, version, &why, false);
                    return;
                }
                Step::BackOnOffer { look_again_in_ms, .. } => {
                    self.back_on_offer(version, look_again_in_ms.map(|wait| self.now_ms().saturating_add(wait)));
                    return;
                }
                // Not said before a hand-off, and the app's ended.
                _ => return,
            }
        }
    }

    /// What a by-hand wait is waiting for, said if it's new.
    fn say_waiting(&self, version: &str, waiting_for: Option<String>) {
        let Some(for_what) = waiting_for else { return };
        let mut st = lock(&self.st);
        if matches!(&st.status, Status::Waiting { for_what: known, .. } if *known == for_what) {
            return;
        }
        st.notice = Some(Notice {
            text: format!("Updating to {version} as soon as this is done: {for_what}"),
            fades: false,
        });
        st.status = Status::Waiting {
            version: version.to_owned(),
            for_what,
        };
        drop(st);
        self.host.changed();
    }

    /// What [`flow::next`] reads of the host, asked in the order it's read and no further than it's read: whether the app's ended, and unless a quit's under way
    /// by other means, whether a build may still go in, and if it may, what's in flight and how long since the person touched the app.
    fn seen(&self, stopping: bool, development_build: bool) -> Seen {
        let mut seen = Seen {
            gone: self.host.gone(),
            ..Seen::default()
        };
        if seen.gone || stopping {
            return seen;
        }
        seen.build_allowed = !development_build || self.host.may_take_a_build();
        if seen.build_allowed {
            let look = self.host.look();
            seen.since_input_ms = self.since_input(&look);
            seen.busy = look.busy;
        }
        seen
    }

    /// Past a good hand-off the app's frozen, so every way out from here has to free it. What happens at each step is [`flow::next`]'s.
    fn quit_after_hand_off(self: &Arc<Self>, asked: Asked, facts: &Facts, to: &Going<'_>, handed: HandedOff) {
        let version = to.version;
        let stopping = self.stopping.load(Ordering::SeqCst);
        let mut seen = self.seen(stopping, false);
        let mut phase = Phase::HandedOff;
        loop {
            match flow::next(to, phase, &seen, asked, stopping, self.wall_ms()) {
                Step::BackOnOffer { look_again_in_ms, give_back } => {
                    if give_back {
                        self.host.cancelled();
                    }
                    self.back_on_offer(version, look_again_in_ms.map(|wait| self.now_ms().saturating_add(wait)));
                    return;
                }
                Step::WritePlan => phase = self.write_plan(facts, to, handed),
                Step::StartInstaller => {
                    {
                        let mut st = lock(&self.st);
                        st.status = Status::Installing(version.to_owned());
                        st.notice = None;
                        st.from_menu = false;
                    }
                    let helper = facts.app.join(HELPER);
                    phase = match self.host.start_installer(&helper, &self.files.plan(), &self.files.log) {
                        Ok(_) => Phase::InstallerStarted,
                        Err(_) => Phase::Fault("the installer couldn't be started"),
                    };
                }
                Step::QuitNow => {
                    self.log(&format!("{version}: handed off, quitting"));
                    phase = if self.host.quit_now() {
                        Phase::QuitAsked { told_ms: self.wall_ms() }
                    } else {
                        Phase::Fault("the app couldn't be asked to quit")
                    };
                }
                Step::Wait { .. } => {
                    std::thread::sleep(WAIT_STEP); // ratchet: poll
                    // Only whether the app's ended is looked at again, while it's quitting.
                    seen.gone = self.host.gone();
                }
                Step::GiveUp(why) => {
                    let _ = std::fs::remove_file(self.files.plan());
                    self.host.cancelled();
                    self.failed(asked, version, why, false);
                    return;
                }
                Step::HandOff | Step::Failed { .. } | Step::Gone => return,
            }
        }
    }

    /// The installer's plan, written beside the state: what comes of it is the next phase.
    fn write_plan(&self, facts: &Facts, to: &Going<'_>, handed: HandedOff) -> Phase<'static> {
        let Some(beside) = beside_app(&facts.app) else {
            return Phase::Fault("this copy isn't in a folder");
        };
        let team_required = matches!(self.source.trust, crate::source::Trust::DeveloperId { .. });
        let start_limit_ms = if self.source.mode == Mode::Test {
            TEST_START_LIMIT_MS
        } else {
            crate::plan::START_LIMIT_MS
        };
        let plan = Plan {
            id: crate::plan::new_id(),
            cdhash: to.cdhash.to_owned(),
            team_required,
            any_version: to.any_version,
            development_build: to.development_build,
            team_signed: to.team_signed,
            from: facts.version.clone(),
            to: to.version.to_owned(),
            pid: std::process::id(),
            app: facts.app.clone(),
            beside,
            root: self.files.root.clone(),
            quit_at_ms: self.wall_ms(),
            background: !self.host.frontmost(),
            quit_limit_ms: crate::plan::QUIT_LIMIT_MS,
            start_limit_ms,
            handed_off_tabs: handed.tabs,
            open: Vec::new(),
            keep_record: false,
            test: test_launch(&self.source, self.host.relaunch()),
        };
        match plan.write(&self.files.plan()) {
            Ok(()) => Phase::PlanWritten,
            Err(_) => Phase::Fault("the update's plan couldn't be written"),
        }
    }
}
