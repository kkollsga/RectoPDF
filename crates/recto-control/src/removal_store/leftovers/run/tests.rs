//! The check and the clear on real folders, with the Trash a folder and every process dead (`tests::world`): the order of the calls, what a confirmation is
//! bound to, and what a stop at any step leaves.

use super::super::act::Hooks;
use super::super::leases::Outside;
use super::super::tests::{DEAD, World as Fixture, temp_name, world};
use super::super::trash::FolderTrash;
use super::*;
use std::ffi::OsStr;
use std::os::unix::fs::MetadataExt;
use std::time::Duration;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Who {
    Person,
    Agent,
}

struct Fx {
    fixture: Fixture,
    store: Mutex<Confirmations<Who>>,
    world: World,
}

const READY: Outside = Outside {
    keeps_record: true,
    beside_newer: false,
    sleep_pass_done: true,
};

fn fx() -> Fx {
    let fixture = world();
    let world = World {
        trash: Arc::new(FolderTrash::at(&fixture.trash)),
        alive: Arc::new(|_| false),
        local: Arc::new(|_| true),
        stand_in: false,
    };
    Fx {
        fixture,
        store: new_store(),
        world,
    }
}

impl Fx {
    fn document(&self) -> PathBuf {
        self.fixture.docs.join("mine.pdf")
    }

    fn asked<'a>(&'a self, document: &'a Path, actor: Who) -> Asked<'a, Who> {
        Asked {
            root: self.fixture.root(),
            document,
            stamp: recto_platform::file_stamp(document).unwrap(),
            actor,
            names: actor == Who::Person,
        }
    }

    fn check(&self, actor: Who) -> Checked {
        let document = self.document();
        let mut next = random_id;
        check(&self.store, &self.asked(&document, actor), true, &self.world, &mut next, Instant::now()).expect("a check")
    }

    fn clear(&self, id: PlanId, actor: Who, now: Instant, steps: &mut dyn Steps) -> Result<Cleared, ClearError> {
        let document = self.document();
        clear(&self.store, &self.asked(&document, actor), id, &self.world, READY, now, steps)
    }

    /// What the person has that no clear may touch: the document's bytes, the live record's names and numbers, and a file outside everything.
    fn kept(&self) -> (Vec<u8>, Vec<(String, u64)>, Vec<u8>) {
        let live: Vec<(String, u64)> = std::fs::read_dir(self.fixture.root().join("session"))
            .unwrap()
            .map(|e| e.unwrap())
            .map(|e| (e.file_name().to_string_lossy().into_owned(), e.metadata().unwrap().ino()))
            .collect();
        let mut live = live;
        live.sort();
        (
            std::fs::read(self.document()).unwrap(),
            live,
            std::fs::read(self.fixture.outside.join("sentinel")).unwrap(),
        )
    }

    /// A new run of the app: nothing is pending.
    fn restart(&self) {
        *lock(&self.store) = Confirmations::new(CONFIRM_LIMITS);
    }

    fn trashed(&self) -> usize {
        std::fs::read_dir(&*self.fixture.trash).unwrap().count()
    }
}

/// What is in `dir` as (relative name, inode), for "nothing else changed". A lock file is left by a lease that was tried, as the installer's own start leaves
/// it: empty, and not anything of the person's.
fn names(dir: &Path) -> Vec<(String, u64)> {
    let mut all = Vec::new();
    fn walk(dir: &Path, base: &Path, all: &mut Vec<(String, u64)>) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            let meta = std::fs::symlink_metadata(&path).unwrap();
            if path.extension().is_some_and(|ext| ext == "lock") {
                continue;
            }
            all.push((path.strip_prefix(base).unwrap().display().to_string(), meta.ino()));
            if meta.is_dir() {
                walk(&path, base, all);
            }
        }
    }
    walk(dir, dir, &mut all);
    all.sort();
    all
}

fn id_of(checked: &Checked) -> PlanId {
    checked.id.expect("a plan to confirm")
}

/// Stops the walk, as a crash would, at one moment of one step.
struct Crash(u32, Moment);

impl Hooks for Crash {}

impl Steps for Crash {
    fn at(&mut self, step: u32, moment: Moment, _deadline: Instant) -> bool {
        !(step == self.0 && moment == self.1)
    }
}

/// Failure: a temp made after the check (one more than was shown) is moved to the Trash under a confirmation that was about what the check showed. **Two
/// guards hold this, so one seeded alone passes:** the digest compare in `Confirmations::take` and the one in `Progress::start`. It fails with both removed
/// (seen, review round 1), which is what it holds.
#[test]
fn a_temp_added_after_the_check_stops_the_clear() {
    let fx = fx();
    let checked = fx.check(Who::Person);
    assert_eq!((checked.found.saves_here, checked.found.copies), (2, 3), "{}", checked.words);
    std::fs::write(fx.fixture.docs.join(temp_name(DEAD, 3, ".tmp")), b"made since").unwrap();
    let before = (names(fx.fixture.root()), names(&fx.fixture.docs));
    assert_eq!(
        fx.clear(id_of(&checked), Who::Person, Instant::now(), &mut Unwatched).unwrap_err(),
        ClearError::Confirm(ConfirmRefusal::PlanChanged)
    );
    assert_eq!((names(fx.fixture.root()), names(&fx.fixture.docs)), before, "nothing was done");
    assert_eq!(fx.trashed(), 0);
    assert!(lock(&fx.store).is_empty(), "the entry is gone: look again");
}

/// Failure: one temp replaced by another (equal counts) is cleared under the first one's confirmation.
#[test]
fn a_different_temp_with_the_same_count_stops_the_clear() {
    let fx = fx();
    let checked = fx.check(Who::Person);
    let gone = fx.fixture.docs.join(temp_name(DEAD, 1, ".tmp"));
    std::fs::remove_file(&gone).unwrap();
    std::fs::write(fx.fixture.docs.join(temp_name(DEAD, 9, ".tmp")), b"another").unwrap();
    assert_eq!(
        fx.clear(id_of(&checked), Who::Person, Instant::now(), &mut Unwatched).unwrap_err(),
        ClearError::Confirm(ConfirmRefusal::PlanChanged)
    );
    assert_eq!(fx.trashed(), 0);
}

/// Failure: the clear goes through end to end but moves the wrong things, or leaves the document, the live record or a file outside touched.
#[test]
fn a_whole_clear_moves_the_saves_and_deletes_the_copies_and_nothing_else() {
    let fx = fx();
    let kept = fx.kept();
    let checked = fx.check(Who::Person);
    assert!(
        checked.words.contains("Move to Trash") || checked.words.contains("unfinished save"),
        "{}",
        checked.words
    );
    let cleared = fx.clear(id_of(&checked), Who::Person, Instant::now(), &mut Unwatched).expect("cleared");
    assert_eq!(cleared.outcome, Outcome::Complete, "{}", cleared.words);
    assert_eq!((cleared.did.trashed, cleared.did.copies), (2, 3));
    assert_eq!(fx.trashed(), 2);
    assert_eq!(fx.kept(), kept);
    assert!(!fx.fixture.root().join("update").join("before").exists());
    assert!(!fx.fixture.root().join("session.damaged").exists());
    // And looking again finds nothing to clear, with no id to give.
    let again = fx.check(Who::Person);
    assert!(again.found.is_empty() && again.id.is_none(), "{}", again.words);
}

/// Failure: the order of calls lets an id do what it shouldn't: a second check leaves the first's id good, a clear twice clears twice, a clear with no check
/// is taken, an id from another document, another asker or another run of the app is accepted, an old one doesn't expire, or a save between is ignored.
#[test]
fn ids_are_good_once_for_their_actor_their_document_and_their_time() {
    let fx = fx();
    let first = id_of(&fx.check(Who::Person));
    let second = id_of(&fx.check(Who::Person));
    let unknown = |error: ClearError| error == ClearError::Confirm(ConfirmRefusal::Unknown);
    assert!(
        unknown(fx.clear(first, Who::Person, Instant::now(), &mut Unwatched).unwrap_err()),
        "a second check replaces the first"
    );
    // The person's id sent by an agent is unknown, and doesn't use it up.
    assert!(unknown(fx.clear(second, Who::Agent, Instant::now(), &mut Unwatched).unwrap_err()));
    // Another document's tab: refused as another target, and the plan is dropped.
    let other = fx.fixture.docs.join("other.pdf");
    std::fs::write(&other, b"%PDF other").unwrap();
    let asked = fx.asked(&other, Who::Person);
    let wrong = clear(&fx.store, &asked, second, &fx.world, READY, Instant::now(), &mut Unwatched).unwrap_err();
    assert_eq!(wrong, ClearError::Confirm(ConfirmRefusal::OtherTarget));
    assert!(
        unknown(fx.clear(second, Who::Person, Instant::now(), &mut Unwatched).unwrap_err()),
        "refused, so gone"
    );
    // With no check at all, and after a restart (a new store, and ids made again).
    assert!(unknown(
        fx.clear(PlanId::from_raw(1).unwrap(), Who::Person, Instant::now(), &mut Unwatched).unwrap_err()
    ));
    let third = id_of(&fx.check(Who::Person));
    fx.restart();
    assert!(unknown(fx.clear(third, Who::Person, Instant::now(), &mut Unwatched).unwrap_err()));
    let third = id_of(&fx.check(Who::Person));
    // Cleared once; the same id again is unknown.
    fx.clear(third, Who::Person, Instant::now(), &mut Unwatched).expect("cleared");
    assert!(unknown(fx.clear(third, Who::Person, Instant::now(), &mut Unwatched).unwrap_err()));
}

/// Failure: a save between the check and the clear (a new file stamp) doesn't stop it. (That an edit in a tab, which changes nothing on disk, doesn't is held
/// in the app, where a tab can be edited: `an_edit_between_check_and_clear_still_clears`.)
#[test]
fn a_save_between_check_and_clear_is_stale() {
    let fx = fx();
    let id = id_of(&fx.check(Who::Person));
    let document = fx.document();
    let bound = fx.asked(&document, Who::Person);
    std::fs::write(&document, b"%PDF a save wrote this, longer").unwrap();
    let saved = Asked {
        stamp: recto_platform::file_stamp(&document).unwrap(),
        ..bound
    };
    let error = clear(&fx.store, &saved, id, &fx.world, READY, Instant::now(), &mut Unwatched).unwrap_err();
    assert_eq!(error, ClearError::Confirm(ConfirmRefusal::Stale));
    assert_eq!(fx.trashed(), 0);
}

/// Failure: a plan older than ten minutes is cleared.
#[test]
fn an_old_id_has_expired() {
    let fx = fx();
    let id = id_of(&fx.check(Who::Person));
    let later = Instant::now() + Duration::from_secs(601);
    assert_eq!(
        fx.clear(id, Who::Person, later, &mut Unwatched).unwrap_err(),
        ClearError::Confirm(ConfirmRefusal::Expired)
    );
    assert_eq!(fx.trashed(), 0);
}

/// Failure: an agent below the level that may clear is given an id (so it fills the store), or the words differ in what they say will happen.
#[test]
fn only_one_who_may_confirm_is_given_an_id_and_an_agent_gets_no_names() {
    let fx = fx();
    let document = fx.document();
    let mut next = random_id;
    let read_only = check(&fx.store, &fx.asked(&document, Who::Agent), false, &fx.world, &mut next, Instant::now()).unwrap();
    assert!(read_only.id.is_none() && lock(&fx.store).is_empty(), "no entry for one who may not confirm");
    assert!(!read_only.words.contains('“'), "{}", read_only.words);
    let person = fx.check(Who::Person);
    assert!(person.words.contains("“mine.pdf”"), "{}", person.words);
    assert_eq!(read_only.found, person.found);
}

/// Failure: an id made again (a collision with a plan pending) replaces another's plan, or the check fails where another id would do; and an id that is 0 is used.
#[test]
fn a_taken_id_is_not_used_again_and_zero_is_no_id() {
    let fx = fx();
    let first = id_of(&fx.check(Who::Person));
    let other = fx.fixture.docs.join("other.pdf");
    std::fs::write(&other, b"%PDF other").unwrap();
    let mut ids = [0, first.get(), first.get(), 77].into_iter();
    let mut next = move || ids.next().unwrap();
    let again = check(&fx.store, &fx.asked(&other, Who::Agent), true, &fx.world, &mut next, Instant::now()).unwrap();
    assert_eq!(again.id.map(PlanId::get), Some(77), "0 and the taken id were passed over");
    assert!(random_id() != random_id(), "two random ids are not the same");
}

/// Failure: a clear that is past its time still acts, or says it cleared. Past the deadline it stops before the first step with nothing done.
#[test]
fn a_clear_past_its_deadline_stops_before_it_acts() {
    let fx = fx();
    let id = id_of(&fx.check(Who::Person));
    let past = Instant::now()
        .checked_sub(CLEAR_DEADLINE + Duration::from_secs(1))
        .expect("a clock that has run");
    let cleared = fx.clear(id, Who::Person, past, &mut Unwatched).expect("it started");
    assert_eq!((cleared.outcome, cleared.did.is_none()), (Outcome::Unchanged, true), "{}", cleared.words);
    assert!(cleared.words.contains("Nothing was cleared"), "{}", cleared.words);
    assert_eq!(fx.trashed(), 0);
}

/// Failure: a clear beside a newer app's record goes on (its live folder is a copy's name), or leaves something changed on the way to refusing.
#[test]
fn beside_a_newer_record_nothing_is_cleared() {
    let fx = fx();
    let id = id_of(&fx.check(Who::Person));
    let before = (names(fx.fixture.root()), names(&fx.fixture.docs));
    let document = fx.document();
    let newer = Outside { beside_newer: true, ..READY };
    let error = clear(
        &fx.store,
        &fx.asked(&document, Who::Person),
        id,
        &fx.world,
        newer,
        Instant::now(),
        &mut Unwatched,
    )
    .unwrap_err();
    assert_eq!(error, ClearError::Busy(Busy::BesideNewer));
    assert_eq!((names(fx.fixture.root()), names(&fx.fixture.docs)), before);
    // The plan is still good once that is over: a lease that wasn't had used nothing up.
    assert!(fx.clear(id, Who::Person, Instant::now(), &mut Unwatched).is_ok());
}

/// Failure: the installer's or a windowless session's lock held during the clear doesn't stop it, or what stops it uses the confirmation up.
#[test]
fn a_held_lock_stops_the_clear_before_anything_is_done() {
    let fx = fx();
    let id = id_of(&fx.check(Who::Person));
    let before = (names(fx.fixture.root()), names(&fx.fixture.docs));
    let held = {
        let path = fx.fixture.root().join("run");
        std::fs::create_dir_all(&path).unwrap();
        let file = recto_platform::local::open_private_for_writing(&path.join("session-windowless.lock"), true).unwrap();
        file.try_lock().unwrap();
        file
    };
    assert_eq!(
        fx.clear(id, Who::Person, Instant::now(), &mut Unwatched).unwrap_err(),
        ClearError::Busy(Busy::Windowless)
    );
    assert_eq!(names(&fx.fixture.docs), before.1);
    drop(held);
    assert!(
        fx.clear(id, Who::Person, Instant::now(), &mut Unwatched).is_ok(),
        "the plan was not used up by a lease that wasn't had"
    );
}

/// Failure: a walk that stops at any step, before its act, after it and before its acknowledgment, or after that, leaves what the next check can't read: it
/// lists something that is gone, misses something that is left, a clear after it doesn't finish, or something that was never planned is changed.
#[test]
fn a_stop_at_any_step_leaves_what_the_next_check_lists_and_a_clear_finishes() {
    // (steps of the plan, and the artifacts each takes): leases, two saves one a step, the update's copy, the two session copies, verify, release.
    let takes = [0u32, 1, 1, 1, 2, 0, 0];
    let total: u32 = takes.iter().sum();
    for step in 0..takes.len() as u32 {
        for moment in [Moment::Before, Moment::Acted, Moment::Acknowledged] {
            let fx = fx();
            let kept = fx.kept();
            let first = fx.check(Who::Person);
            let id = id_of(&first);
            let cleared = fx.clear(id, Who::Person, Instant::now(), &mut Crash(step, moment)).expect("started");
            let acted: u32 = takes[..step as usize].iter().sum::<u32>() + if moment == Moment::Before { 0 } else { takes[step as usize] };
            assert_eq!(cleared.acted, acted, "step {step} {moment:?}: {}", cleared.words);
            // A new run of the app: nothing pending, and the check lists what is left.
            fx.restart();
            let fresh = &fx;
            let left = fresh.check(Who::Person);
            let remaining = left.found.saves_here + left.found.copies;
            assert_eq!(remaining, total - acted, "step {step} {moment:?}: {}", left.words);
            if let Some(id) = left.id {
                let done = fresh.clear(id, Who::Person, Instant::now(), &mut Unwatched).expect("finishes");
                assert_eq!(done.outcome, Outcome::Complete, "step {step} {moment:?}: {}", done.words);
            }
            assert!(fresh.check(Who::Person).found.is_empty(), "step {step} {moment:?}");
            assert_eq!(
                fresh.kept(),
                kept,
                "step {step} {moment:?}: the document, the live record and what is outside are as they were"
            );
        }
    }
}

/// Stops a walk once it has been asked about this many entries of a copy, as a failing disk or a crash would.
struct Mid(usize);

impl Hooks for Mid {
    fn entry(&mut self, _: &OsStr) -> bool {
        if self.0 == 0 {
            return false;
        }
        self.0 -= 1;
        true
    }
}

impl Steps for Mid {}

/// Failure (review round 1, 1): a copy stopped part way, with some entries unlinked, is told as "nothing was cleared" because only whole acts were counted, so the
/// person is told their session copy is as it was when half of it is gone. With only copies to clear, stop after each number of entries: the words never say
/// nothing was cleared (a stop before the first unlink is the one case that does), say a copy was partly deleted, and a check and a clear after it finish.
#[test]
fn a_stop_in_the_middle_of_a_copy_never_says_nothing_was_cleared_and_a_clear_after_it_finishes() {
    let mut stopped_part_way = 0;
    for entries in 1..=14 {
        let fx = fx();
        // Only the copies are left to clear, so the first act is a copy's.
        for entry in std::fs::read_dir(&*fx.fixture.docs).unwrap() {
            let path = entry.unwrap().path();
            if path.file_name().unwrap().as_encoded_bytes().starts_with(b".append-") {
                if path.is_dir() {
                    std::fs::remove_dir_all(&path)
                } else {
                    std::fs::remove_file(&path)
                }
                .unwrap();
            }
        }
        let id = id_of(&fx.check(Who::Person));
        let cleared = fx.clear(id, Who::Person, Instant::now(), &mut Mid(entries)).expect("started");
        if cleared.stopped.is_none() {
            continue;
        }
        stopped_part_way += 1;
        let said = cleared.words.to_lowercase();
        assert!(!said.contains("nothing was"), "{entries} entries: {}", cleared.words);
        assert_eq!(cleared.outcome, Outcome::PartlyCleared, "{entries} entries: {}", cleared.words);
        fx.restart();
        let left = fx.check(Who::Person);
        let id = id_of(&left);
        let done = fx.clear(id, Who::Person, Instant::now(), &mut Unwatched).expect("finishes");
        assert_eq!(done.outcome, Outcome::Complete, "{entries} entries: {}", done.words);
        assert!(fx.check(Who::Person).found.is_empty());
    }
    assert!(stopped_part_way >= 8, "the walk was stopped part way {stopped_part_way} times");
}

/// Spends the clear's time inside the first entry, then lets it go on: what a slow disk does to a long copy.
struct Slow(bool);

impl Hooks for Slow {
    fn entry(&mut self, _: &OsStr) -> bool {
        if !std::mem::replace(&mut self.0, true) {
            let until = Instant::now() + Duration::from_millis(30);
            while Instant::now() < until {
                std::hint::spin_loop();
            }
        }
        true
    }
}

impl Steps for Slow {
    fn limit(&self) -> Duration {
        Duration::from_millis(20)
    }
}

/// Failure (review round 1, 4): the deadline is looked at only between steps, so one step that deletes many copies, each with many entries, runs on past
/// it. With a clear whose first entry takes the whole time, the walk stops at the next entry, part way through that copy, and says it ran out of time.
#[test]
fn a_long_copies_step_stops_at_the_deadline_part_way_and_says_so() {
    let fx = fx();
    // One copy to clear and nothing else, so the stop can only be inside it.
    for name in ["session.damaged", "session.damaged.1"] {
        std::fs::remove_dir_all(fx.fixture.root().join(name)).unwrap();
    }
    for entry in std::fs::read_dir(&*fx.fixture.docs).unwrap() {
        let path = entry.unwrap().path();
        if path.file_name().unwrap().as_encoded_bytes().starts_with(b".append-") {
            if path.is_dir() {
                std::fs::remove_dir_all(&path)
            } else {
                std::fs::remove_file(&path)
            }
            .unwrap();
        }
    }
    let id = id_of(&fx.check(Who::Person));
    let cleared = fx.clear(id, Who::Person, Instant::now(), &mut Slow(false)).expect("started");
    assert_eq!(cleared.stopped.as_deref(), Some("it ran out of time"), "{}", cleared.words);
    assert_eq!(cleared.outcome, Outcome::PartlyCleared, "{}", cleared.words);
    assert!(
        cleared.words.contains("partly deleted"),
        "stopped inside the copy, not after it: {}",
        cleared.words
    );
    assert!(
        fx.fixture.root().join("update").join("before").exists(),
        "what is left is left for the next check"
    );
}

/// An installer that starts between the leases and the confirmation: its folder appears and its lock is held.
struct LateInstaller {
    root: PathBuf,
    lock: Option<std::fs::File>,
}

impl Hooks for LateInstaller {}

impl Steps for LateInstaller {
    fn at(&mut self, _step: u32, moment: Moment, _deadline: Instant) -> bool {
        if moment == Moment::Leased && self.lock.is_none() {
            std::fs::create_dir_all(self.root.join("update")).unwrap();
            let file = recto_platform::local::open_private_for_writing(&self.root.join("update").join("installer.lock"), true).unwrap();
            file.try_lock().unwrap();
            self.lock = Some(file);
        }
        true
    }
}

/// Failure (review round 1, 5): a late `update/` whose lock is held is looked for after the confirmation was taken, so the refusal uses the plan up and the person
/// has to look again for nothing. The refusal comes before the confirmation: the same plan is good once the installer is gone.
#[test]
fn a_late_installer_refuses_before_the_plan_is_used_up() {
    let fx = fx();
    // No update folder yet: the leases take no installer lock, and one that starts later brings its folder.
    std::fs::remove_dir_all(fx.fixture.root().join("update")).unwrap();
    let id = id_of(&fx.check(Who::Person));
    let mut late = LateInstaller {
        root: fx.fixture.root().to_path_buf(),
        lock: None,
    };
    assert_eq!(
        fx.clear(id, Who::Person, Instant::now(), &mut late).unwrap_err(),
        ClearError::Busy(Busy::Installer)
    );
    assert_eq!(fx.trashed(), 0, "nothing was done");
    drop(late);
    assert!(
        fx.clear(id, Who::Person, Instant::now(), &mut Unwatched).is_ok(),
        "the plan was not used up by a lease that wasn't had"
    );
}

/// Failure (review round 2, a): a copy is told "partly deleted" because the gate let an entry go, though the entry then turned out to be another file and
/// nothing was unlinked. The words must say nothing was deleted from that copy.
#[test]
fn a_stop_right_after_the_gate_with_nothing_unlinked_says_nothing_was_cleared() {
    let fx = fx();
    for name in ["session.damaged", "session.damaged.1"] {
        std::fs::remove_dir_all(fx.fixture.root().join(name)).unwrap();
    }
    for entry in std::fs::read_dir(&*fx.fixture.docs).unwrap() {
        let path = entry.unwrap().path();
        if path.file_name().unwrap().as_encoded_bytes().starts_with(b".append-") {
            if path.is_dir() {
                std::fs::remove_dir_all(&path)
            } else {
                std::fs::remove_file(&path)
            }
            .unwrap();
        }
    }
    let id = id_of(&fx.check(Who::Person));
    let whole = fx.fixture.root().join("update").join("before").join(".whole");
    struct Swap(PathBuf, bool);
    impl Hooks for Swap {
        fn entry(&mut self, name: &OsStr) -> bool {
            if name == OsStr::new(".whole") && !self.1 {
                self.1 = true;
                std::fs::remove_file(&self.0).unwrap();
                std::fs::write(&self.0, b"another file").unwrap();
            }
            true
        }
    }
    impl Steps for Swap {}
    let cleared = fx.clear(id, Who::Person, Instant::now(), &mut Swap(whole, false)).expect("started");
    assert!(cleared.stopped.is_some(), "{}", cleared.words);
    assert_eq!(cleared.outcome, Outcome::Unchanged, "the outcome and the words are one fact: {}", cleared.words);
    assert!(cleared.words.contains("Nothing was cleared"), "{}", cleared.words);
    assert!(!cleared.words.contains("partly"), "{}", cleared.words);
}
