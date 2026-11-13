//! **The install's half of an update, as one pure step** (step R1.4): from a download that's good to the app gone, what happens next is [`next`] of what's known
//! now, and nothing else. [`super::Updater`]'s two loops (`wait_and_hand_off`, `quit_after_hand_off`) only gather what it needs, carry out what it says, and
//! feed back what came of that as the next [`Phase`]: a hand-off is written or refused, a plan is written or isn't, the installer starts or doesn't, the app's
//! asked to quit. So every order of those, and of what can happen beside them (the person coming back, a quit by other means, agent access dropping, something
//! in flight, the clock), is a tier-1 test here, with no app, thread or sleep.
//!
//! **What's on which clock.** `now_ms` is the updater's wall clock ([`super::Updater::wall_ms`]): how long a wait has gone on ([`WAIT_LIMIT`]) and how long the
//! app has had to quit ([`QUIT_PATIENCE`]), as before. The time since the person touched the app is the app's idle clock, as the host says it, moved by a test's
//! clock ([`Seen::since_input_ms`]). When it's to look again is said as a time from now ([`Step::BackOnOffer`]), which the loop puts on the updater's own `now_ms`,
//! as the menu's next wake goes by.

use super::{Asked, duration_ms};
use crate::rules;
use std::time::Duration;

/// How long an install that's been asked for will wait for something in flight, before saying it gave up.
pub(super) const WAIT_LIMIT: Duration = Duration::from_secs(10 * 60);
/// How long the app gives itself to quit, before it says that didn't happen and carries on.
pub(super) const QUIT_PATIENCE: Duration = Duration::from_secs(15);

/// What an install is going to, once it's been checked: a release downloaded, or a development build handed over.
pub(super) struct Going<'a> {
    pub version: &'a str,
    pub cdhash: &'a str,
    /// How long the app has to have gone unused, for an install that goes by itself: a release's 30 minutes, or a development build's 5.
    pub idle_ms: u64,
    /// Any other version may replace this one, older included ([`crate::plan::Plan::any_version`]).
    pub any_version: bool,
    /// A development build handed over ([`crate::plan::Plan::development_build`]), and whether a team signed it.
    pub development_build: bool,
    pub team_signed: bool,
}

/// What's been looked at for this step, as the host says it. Asked for only where the phase reads it: before the hand-off and just after it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct Seen {
    /// What's in flight, in words for the person, first the one to say.
    pub busy: Vec<String>,
    /// Since the person last touched the app, by the app's idle clock and a test's.
    pub since_input_ms: u64,
    /// Whether a development build may still go in: agent access is still Full ([`super::Host::may_take_a_build`]). Always so for a release.
    pub build_allowed: bool,
    /// The app this updater was part of has ended, as only a test's headless app does with its process still there ([`super::Host::gone`]). Looked at by the
    /// phases that look again: never so in a real app, whose updater ends with its process.
    pub gone: bool,
}

/// Where the install is, and what came of the last thing it did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Phase<'a> {
    /// Before the hand-off, waiting since `began_ms` for nothing to be in flight. `busy_at_hand_off`: the hand-off was just tried on this look and said
    /// something's in flight that the look didn't, so this look waits too.
    Waiting { began_ms: u64, busy_at_hand_off: bool },
    /// The hand-off couldn't write and read back the state: nothing's lost, and the app's taking everything again.
    NotSaved(&'a str),
    /// A good hand-off: the app's frozen, so every way out from here frees it, but a quit that's under way by other means.
    HandedOff,
    /// The installer's plan is written beside the state.
    PlanWritten,
    /// The installer's running on the plan.
    InstallerStarted,
    /// The app was asked to quit at `told_ms`.
    QuitAsked { told_ms: u64 },
    /// What was to be done next couldn't be: the plan not written, the installer not started, the app not able to be asked to quit.
    Fault(&'static str),
}

/// What's to be done now.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Step {
    /// Look again after a step of [`super::WAIT_STEP`]. `waiting_for`: what a by-hand wait says it's waiting for, if that's new.
    Wait { waiting_for: Option<String> },
    /// The release goes back on offer. `look_again_in_ms`: when an install by itself is to be tried again, from now. `give_back`: the app's frozen by a good
    /// hand-off, and takes what the person does again.
    BackOnOffer { look_again_in_ms: Option<u64>, give_back: bool },
    /// Nothing's in flight: write and read back the state, freezing the app.
    HandOff,
    /// The update doesn't go ahead, and says why. `waiting_for`: said first, as a by-hand wait says what it's waiting for on its last look too.
    Failed { why: String, waiting_for: Option<String> },
    /// Write the installer's plan.
    WritePlan,
    /// Say it's installing, and start the installer on the plan.
    StartInstaller,
    /// Ask the app to quit.
    QuitNow,
    /// Past a good hand-off and it can't go on: the plan goes, so the installer stands down, the app's given back, and the update failed, for this.
    GiveUp(&'static str),
    /// The app's ended while this looked again ([`Seen::gone`]): nothing's to be given back or written, and the install's thread ends.
    Gone,
}

/// **What happens next,** from where the install is, what's been looked at, who asked, whether a quit's under way by other means, and the wall clock now.
pub(super) fn next(going: &Going<'_>, phase: Phase<'_>, look: &Seen, asked: Asked, stopping: bool, now_ms: u64) -> Step {
    let by_itself = asked == Asked::ByItself;
    let come_back = by_itself && look.since_input_ms < going.idle_ms;
    let wait_left = going.idle_ms.saturating_sub(look.since_input_ms);
    match phase {
        Phase::Waiting { .. } | Phase::QuitAsked { .. } if look.gone => Step::Gone,
        Phase::Waiting { began_ms, busy_at_hand_off } => {
            // The person's quitting by other means, and this doesn't turn that into a reopening.
            if stopping {
                return Step::BackOnOffer {
                    look_again_in_ms: None,
                    give_back: false,
                };
            }
            // A build was handed over at Full: if access has dropped since, it doesn't go in, as no one at the level that asked for it is there to.
            if going.development_build && !look.build_allowed {
                return Step::Failed {
                    why: "agent access isn't Full any more, and a development build is installed only at Full".to_owned(),
                    waiting_for: None,
                };
            }
            // They've come back. It's all ready for the next quiet stretch.
            if come_back {
                return Step::BackOnOffer {
                    look_again_in_ms: Some(wait_left),
                    give_back: false,
                };
            }
            if look.busy.is_empty() && !busy_at_hand_off {
                return Step::HandOff;
            }
            if by_itself {
                return Step::BackOnOffer {
                    look_again_in_ms: Some(rules::BUSY_RETRY_MS),
                    give_back: false,
                };
            }
            let for_what = look.busy.first().cloned().unwrap_or_default();
            if now_ms.saturating_sub(began_ms) >= duration_ms(WAIT_LIMIT) {
                return Step::Failed {
                    why: format!("it waited too long: {for_what}"),
                    waiting_for: Some(for_what),
                };
            }
            Step::Wait { waiting_for: Some(for_what) }
        }
        Phase::NotSaved(why) => Step::Failed {
            why: format!("the app's state couldn't be saved first: {why}"),
            waiting_for: None,
        },
        // A quit that came in the meantime goes on as it was going, frozen and with nothing started. If it doesn't go through, it's that which takes everything
        // again.
        Phase::HandedOff if stopping => Step::BackOnOffer {
            look_again_in_ms: None,
            give_back: false,
        },
        // The person may have touched it in the moment before it froze.
        Phase::HandedOff if come_back => Step::BackOnOffer {
            look_again_in_ms: Some(wait_left),
            give_back: true,
        },
        Phase::HandedOff => Step::WritePlan,
        Phase::PlanWritten => Step::StartInstaller,
        Phase::InstallerStarted => Step::QuitNow,
        // Past here this process should be gone. If it isn't, the plan goes, so the installer stands down, and the app's given back. By this updater's clock: a
        // test's process that hands off goes on, and its clock doesn't, so this never gives up under it; its app ending ends this (above).
        Phase::QuitAsked { told_ms } if now_ms.saturating_sub(told_ms) >= duration_ms(QUIT_PATIENCE) => Step::GiveUp("the app didn't quit"),
        Phase::QuitAsked { .. } => Step::Wait { waiting_for: None },
        Phase::Fault(why) => Step::GiveUp(why),
    }
}

#[cfg(test)]
mod tests;
