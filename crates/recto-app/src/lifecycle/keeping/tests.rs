use super::*;

const ASK: Option<Out> = Out::lock(Lock::Ask);
const RESTORE: Option<Out> = Out::lock(Lock::Restore);
const GIVE_BACK: Option<Out> = Out::lock(Lock::GiveBack);
const SAY_NOT_KEPT: Option<Out> = Out::settle(Settle::NotKept);

fn kept(remake: bool) -> Option<Out> {
    Out::settle(Settle::Kept { remake })
}

fn read(keeps: bool) -> Event {
    Event::Read {
        keeps,
        as_read: 0,
        by_update: false,
    }
}

const FAILED: Event = Event::Tried { got: false, busy: false };
const GOT: Event = Event::Tried { got: true, busy: false };
const GOT_BUSY: Event = Event::Tried { got: true, busy: true };

/// A try sent out at `now` and back as `back`: what it comes back to.
fn try_once(keeping: &mut Keeping, back: Event, now: u64) -> Option<Out> {
    assert_eq!(keeping.step(Event::Asking, now), ASK, "a try goes out from {keeping:?}");
    assert_eq!(keeping.due(now), None, "no second try while one is out");
    keeping.step(back, now)
}

fn trying_from(now: u64, by_update: bool) -> Keeping {
    let mut keeping = Keeping::default();
    assert_eq!(
        keeping.step(
            Event::Read {
                keeps: false,
                as_read: 7,
                by_update
            },
            now
        ),
        None,
        "nothing said while it tries"
    );
    keeping
}

/// Every try but the last, failed, from `start`: the time of the last one.
fn all_but_the_last_fail(keeping: &mut Keeping, start: u64, tries: u32) -> u64 {
    let mut now = start;
    for _ in 1..tries {
        now += u64::try_from(keeping.due(now).expect("due").as_millis()).expect("ms");
        assert_eq!(try_once(keeping, FAILED, now), None);
    }
    now + u64::try_from(keeping.due(now).expect("due").as_millis()).expect("ms")
}

/// Failure (owner, 2026-11-13): a launch whose lock comes late tries in a busy loop, or for ever, or more often or less than it says; or gives up before ten
/// seconds, so an old copy still quitting leaves the window unremembered. Ten tries, one a second from the state's taking in, the last at ten seconds, and
/// then it's said and over: no deadline is left.
#[test]
fn it_tries_once_a_second_for_ten_seconds_and_then_says_so() {
    let start = 50_000;
    let mut keeping = trying_from(start, false);
    let mut tried_at = Vec::new();
    let mut now = start;
    let mut said = None;
    while let Some(wait) = keeping.due(now) {
        assert!(tried_at.len() <= TRIES as usize, "more tries than it says: {tried_at:?}");
        now += u64::try_from(wait.as_millis()).unwrap();
        tried_at.push(now - start);
        said = try_once(&mut keeping, FAILED, now);
        assert!(said.is_none() || keeping.due(now).is_none(), "said only at the end");
    }
    assert_eq!(tried_at, (1..=10).map(|second| second * 1_000).collect::<Vec<_>>());
    assert_eq!(said, SAY_NOT_KEPT);
    assert_eq!(keeping, Keeping::NotKept);
    assert_eq!(keeping.step(GOT, now + 1), GIVE_BACK, "a straggler with the lock gives it back");
    assert_eq!(keeping, Keeping::NotKept);
}

/// Failure (third read, 2026-11-13, the blocker): the last try gets the lock while a person's command runs, and the lock is given back by a path that returns
/// before the settling is carried out: "not kept" with nothing said and the updater never told. The last try back with the lock and `busy` gives it back and
/// settles in the same step, for an ordinary launch and one an update opened; one before the last gives it back and goes on trying, saying nothing.
#[test]
fn the_last_try_back_busy_gives_the_lock_back_and_still_settles() {
    for (by_update, tries) in [(false, TRIES), (true, TRIES_AFTER_UPDATE)] {
        let mut keeping = trying_from(0, by_update);
        let last = all_but_the_last_fail(&mut keeping, 0, tries);
        assert_eq!(
            try_once(&mut keeping, GOT_BUSY, last),
            Out::both(Some(Lock::GiveBack), Settle::NotKept),
            "by update {by_update}"
        );
        assert_eq!(keeping, Keeping::NotKept);
        let mut early = trying_from(0, by_update);
        assert_eq!(try_once(&mut early, GOT_BUSY, 1_000), GIVE_BACK, "given back, and nothing said");
        assert!(early.trying() && early.due(1_000).is_some(), "and the next try's due");
    }
}

/// Failure (review, 2026-11-13, S4, and the third read's 4a and 4b): nothing bounds a late keep's restore, so a read on a dead disk holds what the person does and
/// refuses agents for good; or the patience lets the lock go while the read's still inside it, and tells the updater the copy keeps the record. Unclaimed past
/// its patience: said, the lock left with the read until its thread's out (then given back), and a claim after that refused; claimed, the patience is over.
#[test]
fn a_restore_past_its_patience_is_said_and_lets_go_only_once_its_read_is_out() {
    let mut keeping = trying_from(0, false);
    assert_eq!(try_once(&mut keeping, GOT, 1_000), RESTORE);
    assert!(!keeping.writes() && !keeping.claims(), "nothing written or claimed before the read's in hand");
    assert_eq!(keeping.due(1_000 + RESTORING_PATIENCE_MS - 1), Some(Duration::from_millis(1)));
    assert_eq!(keeping.due(1_000 + RESTORING_PATIENCE_MS), Some(Duration::ZERO));
    assert_eq!(keeping.step(Event::ReadPatienceOver, 1_000 + RESTORING_PATIENCE_MS), SAY_NOT_KEPT);
    assert_eq!(keeping, Keeping::LettingGo);
    assert!(!keeping.restoring() && !keeping.refuses_agents_changes() && !keeping.claims() && keeping.not_kept());
    assert_eq!(keeping.due(1_000_000), None);
    assert_eq!(keeping.step(Event::Claimed, 7_000), None, "a claim once it's over is refused");
    assert_eq!(keeping, Keeping::LettingGo);
    assert_eq!(keeping.step(read(true), 7_500), None, "a read while the late one's out keeps nothing");
    assert_eq!(keeping.step(Event::ReadGone, 8_000), GIVE_BACK, "out of its read: the lock goes");
    assert_eq!(keeping, Keeping::NotKept);

    let mut claimed = trying_from(0, false);
    let _ = try_once(&mut claimed, GOT, 1_000);
    assert_eq!(claimed.step(Event::Claimed, 1_200), None);
    assert!(
        claimed.claims() && !claimed.writes(),
        "claimed: the updater hears it keeps it; nothing's written till it's in"
    );
    assert_eq!(claimed.due(1_000_000), None, "no patience once claimed");
    assert_eq!(claimed.step(Event::ReadPatienceOver, 9_000), None);
    assert_eq!(claimed.step(read(true), 9_100), kept(true));
}

/// Failure (review, 2026-11-13, S4): a read whose thread panicked holds the restore for good. Gone: not kept, said, the lock given back.
#[test]
fn a_restore_whose_read_thread_ends_without_it_is_said_and_gives_the_lock_back() {
    for claimed in [false, true] {
        let mut keeping = Keeping::Restoring { since_ms: 0, claimed };
        assert_eq!(
            keeping.step(Event::ReadGone, 10),
            Out::both(Some(Lock::GiveBack), Settle::NotKept),
            "claimed {claimed}"
        );
        assert_eq!(keeping, Keeping::NotKept);
    }
}

/// Failure (the coordinator's decision, 2026-11-13): a launch an update opened gives up after ten seconds like any other, so a lock that comes later leaves the
/// installer hearing only that a second copy started; or it tries for ever, past the installer's two minutes. Sixty tries, one a second, and then said, with
/// the installer's limit at least twice that (held when it's built, beside the constant); a launch no update opened still stops at ten.
#[test]
fn a_launch_an_update_opened_tries_for_a_minute() {
    let mut keeping = trying_from(0, true);
    let mut now = 0;
    let mut said = None;
    let mut tried = 0;
    while let Some(wait) = keeping.due(now) {
        now += u64::try_from(wait.as_millis()).unwrap();
        said = try_once(&mut keeping, FAILED, now);
        tried += 1;
    }
    assert_eq!((tried, now, said), (TRIES_AFTER_UPDATE, KEEP_TRYING_AFTER_UPDATE_MS, SAY_NOT_KEPT));
}

/// Failure: a lock got on a try isn't taken as keeping the record (the saved session isn't restored, or the notice shows anyway), or the window isn't made
/// again from it once it's in, or made again twice; or a launch that kept it from the start is ever told to restore again or that it isn't kept.
#[test]
fn a_lock_got_late_restores_and_is_never_said() {
    let mut keeping = trying_from(0, false);
    assert_eq!(try_once(&mut keeping, FAILED, 1_000), None);
    assert_eq!(try_once(&mut keeping, GOT, 2_000), RESTORE);
    assert_eq!(
        keeping,
        Keeping::Restoring {
            since_ms: 2_000,
            claimed: false
        }
    );
    assert_eq!(
        keeping.due(2_000),
        Some(Duration::from_millis(RESTORING_PATIENCE_MS)),
        "no more tries while it restores: only its patience"
    );
    assert_eq!(
        keeping.step(Event::Looked { now_as: 99 }, 2_001),
        None,
        "nothing the window has stops a restore under way"
    );
    assert_eq!(keeping.step(Event::Stopped, 2_002), None, "nor a quit, whose write waits for it");
    assert_eq!(keeping.step(Event::Claimed, 2_040), None);
    assert_eq!(keeping.step(read(true), 2_050), kept(true), "the window made again from it");
    assert_eq!(keeping, Keeping::Keeps);
    assert_eq!(keeping.step(read(true), 2_060), None, "and only once");
    assert_eq!(
        keeping.step(Event::Looked { now_as: 99 }, 3_000),
        None,
        "what the person does after that is remembered, not said"
    );
    let mut from_the_start = Keeping::default();
    assert_eq!(from_the_start.step(read(true), 0), kept(false), "kept, and nothing made again");
    assert_eq!(from_the_start.due(0), None);
    assert_eq!(from_the_start.step(read(false), 1), None, "a second read changes nothing");
    assert_eq!(from_the_start, Keeping::Keeps);
}

/// Failure (review, 2026-11-13, and the third read's 2): a `session restore` takes the lock outside the machine, by a person or an agent at Control, while it
/// tries or once it's settled not to keep the record, and the state stays "trying" or "not kept" while the store holds the lock: the copy then writes nothing
/// for the run with no notice, or says "won't be remembered" in a window that writes. Taken in with the lock either way, it keeps the record, with nothing
/// restored again over it, and says so (the notice goes).
#[test]
fn a_read_with_the_lock_while_it_tries_or_once_not_kept_keeps_the_record() {
    let mut keeping = trying_from(0, false);
    assert_eq!(keeping.step(read(true), 500), kept(false));
    assert_eq!(keeping, Keeping::Keeps);
    assert_eq!(keeping.due(500), None);
    assert_eq!(keeping.step(Event::Looked { now_as: 8 }, 600), None, "and nothing's said after");
    let mut not_kept = trying_from(0, false);
    let _ = not_kept.step(Event::Looked { now_as: 8 }, 10);
    assert_eq!(not_kept.step(read(false), 20), None, "a read without the lock keeps nothing");
    assert_eq!(not_kept.step(read(true), 30), kept(false));
    assert_eq!(not_kept, Keeping::Keeps);
}

/// Failure (third read, 2026-11-13, the belt): "keeps the record" with the store holding no lock sat silently: nothing written for the run, no notice. A writer
/// that finds it so steps it to "not kept", said; anywhere else it's nothing.
#[test]
fn a_lock_found_gone_while_it_keeps_the_record_is_said() {
    let mut keeping = Keeping::Keeps;
    assert_eq!(keeping.step(Event::LockLost, 0), SAY_NOT_KEPT);
    assert_eq!(keeping, Keeping::NotKept);
    assert_eq!(keeping.step(Event::LockLost, 1), None, "once");
}

/// Failure (review, 2026-11-13): a quit or an update's hand-off begins while it tries, and a try after that takes the lock and restores under the quit's write,
/// which had gathered the empty window and then writes it as the newest record; or the quit shows the notice. It stops, says nothing, and tries no more.
#[test]
fn a_quit_or_a_hand_off_stops_the_trying_quietly() {
    let mut keeping = trying_from(0, false);
    assert_eq!(keeping.step(Event::Stopped, 400), None, "nothing said");
    assert!(matches!(keeping, Keeping::Stopped { .. }));
    assert_eq!(keeping.due(1_000), None, "no more tries");
    assert_eq!(keeping.step(Event::Asking, 1_000), None, "none goes out");
}

/// Failure (third read, 2026-11-13): a hand-off undone after it stopped the trying leaves it stopped for good: never settled, never said, nothing written. Taken
/// up again, the next try a second on, with what was tried counted; a try that was out when it stopped comes back to it.
#[test]
fn a_stop_undone_takes_the_trying_up_again() {
    let mut keeping = trying_from(0, false);
    let _ = try_once(&mut keeping, FAILED, 1_000);
    let _ = keeping.step(Event::Stopped, 1_500);
    assert_eq!(keeping.step(Event::Resumed, 40_000), None);
    assert!(keeping.trying(), "{keeping:?}");
    assert_eq!(keeping.due(40_000), Some(Duration::from_millis(TRY_EVERY_MS)));
    let last = all_but_the_last_fail(&mut keeping, 40_000, TRIES - 1);
    assert_eq!(try_once(&mut keeping, FAILED, last), SAY_NOT_KEPT, "ten tries in all");

    let mut out = trying_from(0, false);
    let _ = out.step(Event::Asking, 1_000);
    let _ = out.step(Event::Stopped, 1_001);
    let _ = out.step(Event::Resumed, 1_002);
    assert_eq!(out.due(1_002), None, "the try that's out is waited for");
    assert_eq!(out.step(GOT, 1_003), RESTORE, "and taken as any");

    let mut back_while_stopped = trying_from(0, false);
    let _ = back_while_stopped.step(Event::Asking, 1_000);
    let _ = back_while_stopped.step(Event::Stopped, 1_001);
    assert_eq!(back_while_stopped.step(GOT, 1_002), GIVE_BACK, "back while stopped: given back, not kept");
    let _ = back_while_stopped.step(Event::Resumed, 1_003);
    assert!(back_while_stopped.due(1_003).is_some(), "and the next try's due once it's taken up");
}

/// Failure (review, 2026-11-13, S1): a try is out on its own thread (the lock file's open on a sleeping disk) when a quit begins or the person changes
/// something, and comes back with the lock: kept, it restores under the quit's write or over what the person has, or keeps the record in a window that says
/// it won't be remembered. Or a second try goes out beside the first. One out at a time; what came while it was out wins, and the lock it brings is given back.
#[test]
fn what_comes_while_a_try_is_out_wins_and_its_lock_is_given_back() {
    for event in [Event::Looked { now_as: 8 }, Event::Stopped] {
        let mut keeping = trying_from(0, false);
        assert_eq!(keeping.step(Event::Asking, 1_000), ASK);
        assert_eq!(keeping.due(5_000), None, "no try's due while one is out");
        assert_eq!(keeping.step(Event::Asking, 5_000), None, "nor goes out");
        let _ = keeping.step(event, 1_001);
        assert!(!keeping.trying(), "{event:?}");
        assert_eq!(keeping.step(GOT, 1_002), GIVE_BACK, "{event:?}");
        assert!(!keeping.trying() && !keeping.restoring(), "{event:?}: and stays so");
    }
}

/// Failure (review, 2026-11-13): an agent's open or edit lands while it's trying, stopping the retry and showing the person a notice for what an agent did, or
/// while the saved session's being read, and the restore then replaces it and its tab number names one of the owner's tabs. Refused exactly while it's trying
/// or restoring, and never once it keeps the record or has settled not to.
#[test]
fn agents_changes_are_refused_while_it_settles_and_only_then() {
    let mut keeping = Keeping::default();
    assert!(!keeping.refuses_agents_changes(), "before the state's read, the socket isn't served at all");
    let _ = keeping.step(
        Event::Read {
            keeps: false,
            as_read: 7,
            by_update: false,
        },
        0,
    );
    assert!(keeping.refuses_agents_changes(), "trying");
    let _ = try_once(&mut keeping, GOT, 1_000);
    assert!(keeping.refuses_agents_changes(), "restoring");
    let _ = keeping.step(read(true), 1_050);
    assert!(!keeping.refuses_agents_changes(), "kept");
    let mut not_kept = trying_from(0, false);
    let _ = not_kept.step(Event::Looked { now_as: 8 }, 10);
    assert!(!not_kept.refuses_agents_changes(), "settled not to keep it");
}

/// Failure (the coordinator's choice, 2026-11-13): the person starts working in the window before the lock comes, and it goes on trying, so a late lock would
/// restore over what they have or write their window over the saved session; or it stops and says nothing until the ten seconds are up. It stops at once and
/// says so, and a look at a window as it was changes nothing.
#[test]
fn the_person_working_in_the_window_stops_it_and_says_so_at_once() {
    let mut keeping = trying_from(0, false);
    assert_eq!(keeping.step(Event::Looked { now_as: 7 }, 300), None, "the window as it was");
    assert!(keeping.trying());
    assert_eq!(keeping.step(Event::Looked { now_as: 8 }, 400), SAY_NOT_KEPT);
    assert_eq!(keeping, Keeping::NotKept);
    assert_eq!(keeping.due(400), None, "no more tries");
    assert_eq!(keeping.step(Event::Asking, 1_000), None, "none goes out");
}

/// Every state the machine can be in, with what it was read with: the states to walk every event from.
fn every_state() -> Vec<Keeping> {
    let mut states = vec![Keeping::NotAsked, Keeping::Keeps, Keeping::NotKept, Keeping::LettingGo];
    for claimed in [false, true] {
        states.push(Keeping::Restoring { since_ms: 0, claimed });
    }
    for (tries, by_update) in [(0, false), (1, false), (TRIES - 1, false), (TRIES - 1, true), (TRIES_AFTER_UPDATE - 1, true)] {
        for asking in [false, true] {
            states.push(Keeping::Trying {
                since_ms: 0,
                tries,
                as_read: 7,
                asking,
                by_update,
            });
            states.push(Keeping::Stopped {
                since_ms: 0,
                tries,
                as_read: 7,
                asking,
                by_update,
            });
        }
    }
    states
}

/// Every event, each way it can come.
fn every_event() -> Vec<Event> {
    let mut events = vec![
        Event::Stopped,
        Event::Resumed,
        Event::Asking,
        Event::Claimed,
        Event::ReadPatienceOver,
        Event::ReadGone,
        Event::LockLost,
    ];
    for got in [true, false] {
        for busy in [true, false] {
            events.push(Event::Tried { got, busy });
        }
    }
    for keeps in [true, false] {
        for by_update in [true, false] {
            events.push(Event::Read { keeps, as_read: 7, by_update });
            events.push(Event::Read { keeps, as_read: 8, by_update });
        }
    }
    events.push(Event::Looked { now_as: 7 });
    events.push(Event::Looked { now_as: 8 });
    events
}

/// The app as the carrier sees it, recorded: the launch held back from the updater (or not), how many times it was told, the notice, the lock, the window
/// made again, the tries sent and the reads begun.
#[derive(Debug, Default)]
struct Recorded {
    facts_held: bool,
    told: u32,
    notice_not_kept: bool,
    gave_back: u32,
    remade: u32,
    asked: u32,
    restored: u32,
}

impl Carry for Recorded {
    fn ask(&mut self) {
        self.asked += 1;
    }
    fn restore(&mut self) {
        self.restored += 1;
    }
    fn give_back(&mut self) {
        self.gave_back += 1;
    }
    fn notice(&mut self, not_kept: bool) {
        self.notice_not_kept = not_kept;
    }
    /// As the app's: what's held is taken, so a second telling tells nothing.
    fn tell_the_updater(&mut self) {
        if std::mem::take(&mut self.facts_held) {
            self.told += 1;
        }
    }
    fn remake(&mut self) {
        self.remade += 1;
    }
}

/// Failure (review, 2026-11-13, and the third read's blocker: a way into "not kept" gave the lock back and returned before the settling was carried out, so the
/// updater, held back for the lock, was never told and nothing was said): every event from every state, stepped and carried out as the app does. Whenever the
/// state becomes keeping or not keeping the record, the updater has been told exactly once and the notice is as the state says; a step that sends a try sends
/// one, a step that reads begins one read, and the lock a try brings back is given back or read with, never left; the window is made again only from a late
/// keep's restore.
#[test]
fn every_way_into_settled_tells_the_updater_once_and_the_notice_is_as_the_state_says() {
    let mut walked = 0;
    for state in every_state() {
        for event in every_event() {
            let mut after = state;
            let out = after.step(event, TRY_EVERY_MS * u64::from(TRIES));
            let mut app = Recorded {
                facts_held: !state.settled(),
                told: 0,
                notice_not_kept: state.not_kept(),
                ..Recorded::default()
            };
            carry(out, &mut app);
            let seen = format!("{state:?} on {event:?} to {after:?}: {out:?}, {app:?}");
            if after.settled() {
                assert!(!app.facts_held, "settled with the updater untold: {seen}");
                assert!(app.told <= 1, "{seen}");
                assert_eq!(app.notice_not_kept, after.not_kept(), "the notice isn't as the state says: {seen}");
            }
            if after.settled() && !state.settled() {
                assert_eq!(app.told, 1, "settled without telling the updater once: {seen}");
            }
            if !after.settled() {
                assert!(app.facts_held == !state.settled() && app.told == 0, "told before it settled: {seen}");
            }
            if after != state && after.not_kept() == state.not_kept() && after.settled() == state.settled() {
                assert!(out.and_then(|out| out.settle).is_none(), "said with nothing settled: {seen}");
            }
            if let Event::Tried { got: true, .. } = event
                && matches!(
                    state,
                    Keeping::Trying { asking: true, .. } | Keeping::Stopped { asking: true, .. } | Keeping::NotKept
                )
            {
                assert_eq!(app.gave_back + app.restored, 1, "a lock brought back is given back or read with: {seen}");
            }
            assert!(app.asked <= 1 && app.restored <= 1 && app.gave_back <= 1, "{seen}");
            assert_eq!(
                app.remade,
                u32::from(matches!((state, after), (Keeping::Restoring { .. }, Keeping::Keeps))),
                "{seen}"
            );
            walked += 1;
        }
    }
    assert_eq!(walked, every_state().len() * every_event().len());
}
