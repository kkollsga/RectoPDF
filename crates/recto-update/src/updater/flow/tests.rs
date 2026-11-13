//! Tier 1: [`next`] over every combination of what it reads, and whole installs driven through it with something happening at every point.

use super::*;

const MIN: u64 = 60_000;
const LIMIT: u64 = 10 * MIN;

fn going(development_build: bool) -> Going<'static> {
    Going {
        version: "0.1.99",
        cdhash: "c",
        idle_ms: if development_build {
            rules::IDLE_BEFORE_BUILD_MS
        } else {
            rules::IDLE_BEFORE_INSTALL_MS
        },
        any_version: false,
        development_build,
        team_signed: !development_build,
    }
}

fn seen(busy: bool, since_input_ms: u64, build_allowed: bool, gone: bool) -> Seen {
    Seen {
        busy: if busy { vec!["A file is being saved.".to_owned()] } else { Vec::new() },
        since_input_ms,
        build_allowed,
        gone,
    }
}

/// Times since the person touched the app, around both rules: a build's 5 minutes and a release's 30.
const SINCE: [u64; 6] = [0, 5 * MIN - 1, 5 * MIN, 30 * MIN - 1, 30 * MIN, 60 * MIN];
const ASKED: [Asked; 2] = [Asked::ByHand, Asked::ByItself];

/// Failure (2026-11-11, c83a227 and 526cae7, and the hand-off's rules since 2026-11-07): before the hand-off, an install goes over a quit that's under way, a
/// development build goes in after agent access dropped below Full, the person coming back is judged by a release's 30 minutes for a build's 5 (so a build
/// waits half an hour, or goes while they're there), it hands off while something's in flight, or a wait by hand never ends. Every combination of what it
/// reads: whether the app's ended, a quit's under way, a build and whether it may still go in, who asked, the time since the person touched it around both
/// rules, something in flight seen by the look or only by the hand-off, and the wait's time before and at its limit. Each rule is said as what wins.
#[test]
fn every_combination_before_the_hand_off_does_what_wins() {
    let mut seen_steps = 0;
    for gone in [false, true] {
        for stopping in [false, true] {
            for development_build in [false, true] {
                for build_allowed in [false, true] {
                    for asked in ASKED {
                        for since in SINCE {
                            for busy in [false, true] {
                                for busy_at_hand_off in [false, true] {
                                    for waited in [0, LIMIT - 1, LIMIT] {
                                        let to = going(development_build);
                                        let look = seen(busy, since, build_allowed || !development_build, gone);
                                        let phase = Phase::Waiting {
                                            began_ms: 1_000,
                                            busy_at_hand_off,
                                        };
                                        let step = next(&to, phase, &look, asked, stopping, 1_000 + waited);
                                        let case = format!(
                                            "gone {gone}, stopping {stopping}, build {development_build} allowed {build_allowed}, {asked:?}, since {since}, busy {busy} at hand-off {busy_at_hand_off}, waited {waited}: {step:?}"
                                        );
                                        seen_steps += 1;
                                        let expected = if gone {
                                            Step::Gone
                                        } else if stopping {
                                            Step::BackOnOffer {
                                                look_again_in_ms: None,
                                                give_back: false,
                                            }
                                        } else if development_build && !build_allowed {
                                            Step::Failed {
                                                why: "agent access isn't Full any more, and a development build is installed only at Full".to_owned(),
                                                waiting_for: None,
                                            }
                                        } else if asked == Asked::ByItself && since < to.idle_ms {
                                            Step::BackOnOffer {
                                                look_again_in_ms: Some(to.idle_ms - since),
                                                give_back: false,
                                            }
                                        } else if !busy && !busy_at_hand_off {
                                            Step::HandOff
                                        } else if asked == Asked::ByItself {
                                            Step::BackOnOffer {
                                                look_again_in_ms: Some(rules::BUSY_RETRY_MS),
                                                give_back: false,
                                            }
                                        } else {
                                            let for_what = if busy { "A file is being saved." } else { "" }.to_owned();
                                            if waited >= LIMIT {
                                                Step::Failed {
                                                    why: format!("it waited too long: {for_what}"),
                                                    waiting_for: Some(for_what),
                                                }
                                            } else {
                                                Step::Wait { waiting_for: Some(for_what) }
                                            }
                                        };
                                        assert_eq!(step, expected, "{case}");
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    assert_eq!(seen_steps, 2 * 2 * 2 * 2 * 2 * SINCE.len() * 2 * 2 * 3);
}

/// Failure (the hand-off's rules since 2026-11-07): past a good hand-off the app's left frozen, or a quit that came in the meantime is turned into a reopening;
/// the person who touched the app in the moment before it froze gets it back only after the installer's started, or never; the plan is written and the installer
/// started in some other order; a step that can't be done leaves the app frozen; or the wait for the quit gives up early, never, or writes after the app has
/// ended. Every phase past the hand-off with every combination of what it reads.
#[test]
fn every_combination_past_the_hand_off_does_what_wins() {
    for stopping in [false, true] {
        for development_build in [false, true] {
            for asked in ASKED {
                for since in SINCE {
                    for gone in [false, true] {
                        let to = going(development_build);
                        let look = seen(false, since, true, gone);
                        let case = format!("stopping {stopping}, build {development_build}, {asked:?}, since {since}, gone {gone}");
                        let step = |phase, now| next(&to, phase, &look, asked, stopping, now);
                        let handed = if stopping {
                            Step::BackOnOffer {
                                look_again_in_ms: None,
                                give_back: false,
                            }
                        } else if asked == Asked::ByItself && since < to.idle_ms {
                            Step::BackOnOffer {
                                look_again_in_ms: Some(to.idle_ms - since),
                                give_back: true,
                            }
                        } else {
                            Step::WritePlan
                        };
                        assert_eq!(step(Phase::HandedOff, 0), handed, "{case}");
                        assert_eq!(step(Phase::PlanWritten, 0), Step::StartInstaller, "{case}");
                        assert_eq!(step(Phase::InstallerStarted, 0), Step::QuitNow, "{case}");
                        assert_eq!(step(Phase::Fault("why"), 0), Step::GiveUp("why"), "{case}");
                        assert_eq!(
                            step(Phase::NotSaved("full"), 0),
                            Step::Failed {
                                why: "the app's state couldn't be saved first: full".to_owned(),
                                waiting_for: None
                            },
                            "{case}"
                        );
                        let patience = duration_ms(QUIT_PATIENCE);
                        for (waited, quitting) in [
                            (0, Step::Wait { waiting_for: None }),
                            (patience - 1, Step::Wait { waiting_for: None }),
                            (patience, Step::GiveUp("the app didn't quit")),
                        ] {
                            let expected = if gone { Step::Gone } else { quitting };
                            assert_eq!(
                                step(Phase::QuitAsked { told_ms: 500 }, 500 + waited),
                                expected,
                                "{case}, quitting for {waited} ms"
                            );
                        }
                    }
                }
            }
        }
    }
}

/// What can happen beside an install, at some look.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Beside {
    Nothing,
    /// The person touches the app.
    Back,
    /// A quit by other means.
    Quit,
    /// Agent access drops below Full.
    Dropped,
    /// Something's in flight from here on.
    Busy,
}

/// How a driven install ended.
#[derive(Debug, PartialEq, Eq)]
enum End {
    /// Quit for the installer, with the app frozen and the plan in place.
    Quit,
    /// Back on offer, the app given back if it was frozen.
    BackOnOffer {
        was_frozen: bool,
        given_back: bool,
    },
    Failed {
        was_frozen: bool,
        given_back: bool,
    },
}

/// **An install carried out as the loops carry it out**, on a host where what happens beside it happens at look `at`, a release or a build asked for `asked`,
/// with the person away long enough at the start. Hand-offs, plans, installers and quits all go; the app quits at once once asked.
fn drive(development_build: bool, asked: Asked, beside: Beside, at: usize) -> End {
    let to = going(development_build);
    let (mut stopping, mut since, mut allowed, mut busy) = (false, to.idle_ms, true, false);
    let mut frozen = false;
    let mut phase = Phase::Waiting {
        began_ms: 0,
        busy_at_hand_off: false,
    };
    let mut now = 0;
    for look in 0..200 {
        if look == at {
            match beside {
                Beside::Nothing => {}
                Beside::Back => since = 0,
                Beside::Quit => stopping = true,
                Beside::Dropped => allowed = false,
                Beside::Busy => busy = true,
            }
        }
        let seen = seen(busy, since, allowed, false);
        match next(&to, phase, &seen, asked, stopping, now) {
            Step::Wait { .. } => {
                now += 100;
                if let Phase::QuitAsked { .. } = phase {
                    return End::Quit;
                }
            }
            Step::HandOff => {
                frozen = true;
                phase = Phase::HandedOff;
            }
            Step::WritePlan => phase = Phase::PlanWritten,
            Step::StartInstaller => phase = Phase::InstallerStarted,
            Step::QuitNow => phase = Phase::QuitAsked { told_ms: now },
            Step::BackOnOffer { give_back, .. } => {
                return End::BackOnOffer {
                    was_frozen: frozen,
                    given_back: give_back,
                };
            }
            Step::Failed { .. } => {
                return End::Failed {
                    was_frozen: frozen,
                    given_back: false,
                };
            }
            Step::GiveUp(_) => {
                return End::Failed {
                    was_frozen: frozen,
                    given_back: true,
                };
            }
            Step::Gone => panic!("the app never ends here"),
        }
    }
    // A wait by hand on something that stays in flight: past its limit.
    End::Failed {
        was_frozen: frozen,
        given_back: false,
    }
}

/// Failure (the same rules, as a whole install): something happening at some point of an install (the person back, a quit by other means, access dropped, a
/// file being saved) leaves the app frozen, hands off after it, or lets an install by itself go while the person's there. Each, at every look of a whole
/// install, for a release and a build, by hand and by itself: a hand-off that doesn't go on is given back, unless a quit by other means is to end the app.
#[test]
fn whatever_happens_at_any_point_a_frozen_app_is_given_back_or_quits() {
    for development_build in [false, true] {
        for asked in ASKED {
            for beside in [Beside::Nothing, Beside::Back, Beside::Quit, Beside::Dropped, Beside::Busy] {
                for at in 0..6 {
                    let end = drive(development_build, asked, beside, at);
                    let case = format!("build {development_build}, {asked:?}, {beside:?} at look {at}: {end:?}");
                    match end {
                        // Look 0 is the last before the hand-off, 1 the one just after it, when the app's frozen: past that, nothing a person does gets in,
                        // and access is read before the hand-off only.
                        End::Quit => assert!(
                            beside == Beside::Nothing
                                || at > 1
                                || (at == 1 && matches!(beside, Beside::Dropped | Beside::Busy))
                                || (beside == Beside::Back && asked == Asked::ByHand)
                                || (beside == Beside::Dropped && !development_build),
                            "it went over what happened before the hand-off was looked at: {case}"
                        ),
                        End::BackOnOffer { was_frozen, given_back } => {
                            assert!(!was_frozen || given_back || beside == Beside::Quit, "left frozen: {case}");
                            assert!(given_back <= was_frozen, "given back what wasn't frozen: {case}");
                        }
                        End::Failed { was_frozen, given_back } => assert!(!was_frozen || given_back, "left frozen: {case}"),
                    }
                    if beside == Beside::Back && asked == Asked::ByItself && at <= 1 {
                        assert_ne!(end, End::Quit, "went by itself while the person was there: {case}");
                    }
                    if beside == Beside::Dropped && development_build && at == 0 {
                        assert!(matches!(end, End::Failed { was_frozen: false, .. }), "a build went in below Full: {case}");
                    }
                }
            }
        }
    }
}
