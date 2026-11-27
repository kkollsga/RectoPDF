//! **Clear leftovers in the app** (`docs/design/removal.md` §10, D5): `recto removal check` and `recto removal clear`, which the window's sheet will send as
//! the same commands (D6). What is decided (the survey, the plan, the leases, the confirmation, the acts and every word) is `recto-control`'s
//! `removal_store::leftovers`; this holds only what belongs to the app: the pending plans, the one clear at a time, a QA hold, and the facts only the app knows.
//!
//! Both run on the caller's thread (the socket's, or the window's own thread for the person: `WAITS`), never on the UI thread and never under the session's
//! lock, which is taken only to read which tab and file.

pub mod sheet;

use super::saver::Saver;
use super::sleeper::OnDisk;
use super::tools::Tools;
use super::{AppHost, Serving};
use crate::lifecycle::keeping::Keeping;
use recto_agent::commands::removal::{RemovalCheck, RemovalClear, plan_word, read_plan, removal_needs_app};
use recto_agent::commands::{Cmd, Removal as Family, row_access};
use recto_agent::{Actor, AgentError, Code, Command, TabRef, exit};
use recto_control::removal_store::leftovers::leases::{Busy, Outside};
use recto_control::removal_store::leftovers::run::{self, Asked, CheckError, ClearError, LookError, Moment, Steps, World};
use recto_control::removal_store::leftovers::{act::Hooks, words};
use recto_core::removal::{CONFIRM_LIMITS, ConfirmRefusal, Confirmations, Refusal};
use serde_json::{Value, json};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Condvar, Mutex, PoisonError};
use std::time::{Duration, Instant};

/// How long a QA hold waits for its release before the clear goes on, so a test that forgot can't hold an app for ever.
const HOLD_LIMIT: Duration = Duration::from_secs(60);

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// What a QA run holds a clear at: before the act of one step, until it is let go.
#[derive(Default)]
struct Hold {
    state: Mutex<HoldState>,
    changed: Condvar,
}

#[derive(Default)]
struct HoldState {
    armed: Option<u32>,
    reached: bool,
}

/// The app's part of Clear leftovers: the plans pending, whether a clear is running, a QA hold and, for a test, the world it acts in. At rest it holds an empty
/// store and a few flags: no thread, buffer or handle.
pub struct Removal {
    store: Mutex<Confirmations<Actor>>,
    clearing: AtomicBool,
    hold: Hold,
    world: Mutex<Option<World>>,
    /// How long a clear may take, in milliseconds, when a test or a QA run shortens it; 0 is the design's.
    limit_ms: AtomicU64,
    /// The sheet over the window, and its ids.
    sheet: sheet::Leftovers,
}

impl Default for Removal {
    fn default() -> Self {
        Self {
            store: Mutex::new(Confirmations::new(CONFIRM_LIMITS)),
            clearing: AtomicBool::new(false),
            hold: Hold::default(),
            world: Mutex::new(None),
            limit_ms: AtomicU64::new(0),
            sheet: sheet::Leftovers::default(),
        }
    }
}

/// Marks a clear as running until it is dropped.
struct Clearing<'a>(&'a AtomicBool);

impl Drop for Clearing<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::SeqCst);
    }
}

/// A clear's walk as the app sees it: held at the step a QA run asked for.
struct Held<'a>(&'a Hold, Duration);

impl Hooks for Held<'_> {}

impl Steps for Held<'_> {
    fn limit(&self) -> Duration {
        self.1
    }

    fn at(&mut self, step: u32, moment: Moment, deadline: Instant) -> bool {
        if moment != Moment::Before {
            return true;
        }
        let mut state = lock(&self.0.state);
        if state.armed != Some(step) {
            return true;
        }
        state.reached = true;
        self.0.changed.notify_all();
        // Held until released, and never past the clear's own time.
        let until = deadline.min(Instant::now() + HOLD_LIMIT);
        while state.armed == Some(step) && Instant::now() < until {
            state = self
                .0
                .changed
                .wait_timeout(state, until.saturating_duration_since(Instant::now()))
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
        true
    }
}

impl Removal {
    /// QA: hold the next clear before the act of this step.
    pub fn hold_at(&self, step: u32) {
        *lock(&self.hold.state) = HoldState {
            armed: Some(step),
            reached: false,
        };
    }

    /// QA: let a held clear go on, and hold none.
    pub fn release(&self) {
        lock(&self.hold.state).armed = None;
        self.hold.changed.notify_all();
    }

    /// Whether a clear is held at the step a QA run asked for.
    pub fn held(&self) -> bool {
        let state = lock(&self.hold.state);
        state.armed.is_some() && state.reached
    }

    /// Whether a clear is running now: what `settled` and a hand-off see.
    pub fn clearing(&self) -> bool {
        self.clearing.load(Ordering::SeqCst)
    }

    /// QA and tests: act in this world (a Trash, a process table, a volume fact) and not the Mac's.
    pub fn set_world(&self, world: World) {
        *lock(&self.world) = Some(world);
    }

    /// Tests only (the compiler holds it): a clear may take this long, not the design's 10 seconds.
    #[cfg(any(test, feature = "testkit"))]
    pub fn set_limit(&self, limit: Duration) {
        self.limit_ms
            .store(u64::try_from(limit.as_millis()).unwrap_or(u64::MAX).max(1), Ordering::SeqCst);
    }

    fn limit(&self) -> Duration {
        match self.limit_ms.load(Ordering::SeqCst) {
            0 => run::CLEAR_DEADLINE,
            ms => Duration::from_millis(ms),
        }
    }

    /// The sheet over the window, which the person's answers fill.
    pub fn sheet(&self) -> &sheet::Leftovers {
        &self.sheet
    }

    /// How many plans wait for a yes.
    pub fn pending(&self) -> usize {
        lock(&self.store).len()
    }

    /// **The Mac's own Trash, unless the state folder is a test's or a quiet run's** (`overridden`, which the run's own start decides and checks is in the
    /// system's temp folders): there a folder beside the state, which takes only a quiet run's files. Never decided by where the folder is, which a link can
    /// change: the person's real app always has the Mac's Trash.
    fn world(&self, root: &std::path::Path, overridden: bool) -> World {
        if let Some(world) = lock(&self.world).clone() {
            return world;
        }
        if overridden {
            World::stand_in(&root.join("trash-stand-in"))
        } else {
            World::real()
        }
    }
}

/// **In a quiet run or a test, nothing outside its folders is moved or deleted:** the state folder and the document are a quiet run's files, or the clear is
/// refused with nothing done. With the Mac's own Trash, whatever the person has.
pub(crate) fn within_the_run(world: &World, paths: &[&std::path::Path]) -> Result<(), AgentError> {
    if !world.stand_in {
        return Ok(());
    }
    match paths.iter().find(|path| !recto_control::quiet::is_a_quiet_runs_file(path)) {
        Some(outside) => Err(recto_agent::quiet_files::outside_the_run(outside)),
        None => Ok(()),
    }
}

impl Removal {
    /// The QA verbs of Clear leftovers (`qa removal-hold`, `qa removal-state`): `None` for any other verb. A hold stops the next clear before the act of a step
    /// until released; the state is what is pending, whether a clear runs and whether it is held.
    pub(crate) fn qa_verb(
        &self,
        verb: &str,
        value: &str,
        outside: impl FnOnce() -> Outside,
        record: impl FnOnce() -> Value,
    ) -> Option<Result<Option<Value>, AgentError>> {
        Some(match verb {
            "removal_hold" if value == "release" => {
                self.release();
                Ok(None)
            }
            "removal_hold" => match value.parse::<u32>() {
                Ok(step) => {
                    self.hold_at(step);
                    Ok(None)
                }
                Err(_) => Err(AgentError::new(Code::USAGE, "It's a step number, or `release`.", exit::USAGE)),
            },
            "removal_state" => {
                let outside = outside();
                let mut state = json!({
                    "pending": self.pending(), "clearing": self.clearing(), "held": self.held(), "sheet": self.sheet.kind(),
                    "ready": outside.keeps_record && !outside.beside_newer && outside.sleep_pass_done,
                });
                // What an unfinished removal's record says (`record`) and what the store withholds because of it (`withheld`): no path, no name.
                if let (Some(state), Some(record)) = (state.as_object_mut(), record().as_object()) {
                    state.extend(record.clone());
                }
                Ok(Some(state))
            }
            _ => return None,
        })
    }
}

fn refuse(code: Code, words: impl Into<String>, exit: i32) -> AgentError {
    AgentError::new(code, words, exit)
}

/// What a tab is, as a clear needs it: the real file, which tab's, and nothing of what's in it. Taken under the session's lock, briefly, and never held.
fn file_of(host: &AppHost, actor: Actor, tab: &TabRef) -> Result<PathBuf, AgentError> {
    host.session.with_session(|session| {
        let id = recto_agent::select_tab(session, tab)?;
        // A document the person opened with its password gives an agent nothing, its folder's listing included (the rule is the command's: `PROTECTED_OK`).
        if !actor.sees_protected() && session.tabs().withheld_from_agents(id) {
            return Err(recto_agent::protected_refusal());
        }
        session
            .tabs()
            .path_of(id)
            .ok_or_else(|| AgentError::new(Code::NO_SUCH_TAB, "That tab isn't open.", exit::NOT_FOUND))
    })
}

fn clear_error(error: ClearError) -> AgentError {
    match error {
        ClearError::Busy(Busy::BesideNewer) => refuse(Code::REMOVAL_CANT_CLEAR, words::busy(Busy::BesideNewer), exit::FAILED),
        ClearError::Busy(busy) => refuse(Code::REMOVAL_BUSY, words::busy(busy), exit::BUSY),
        ClearError::Look(LookError::Changed) => refuse(Code::REMOVAL_PLAN_CHANGED, words::confirm_refusal(ConfirmRefusal::Stale), exit::FAILED),
        ClearError::Look(LookError::Refused { why, went_back }) => refuse(Code::REMOVAL_CANT_CLEAR, words::refusal(why, went_back), exit::FAILED),
        ClearError::Confirm(why) => {
            let code = match why {
                ConfirmRefusal::Expired => Code::REMOVAL_PLAN_EXPIRED,
                ConfirmRefusal::Unknown => Code::REMOVAL_UNKNOWN_PLAN,
                ConfirmRefusal::OtherTarget | ConfirmRefusal::Stale | ConfirmRefusal::PlanChanged => Code::REMOVAL_PLAN_CHANGED,
                ConfirmRefusal::Full | ConfirmRefusal::Collision | ConfirmRefusal::Mismatched => Code::REMOVAL_BUSY,
            };
            refuse(code, words::confirm_refusal(why), exit::FAILED)
        }
    }
}

/// The parts of the app a clear reads, borrowed for one command: nothing of the whole core.
pub(crate) struct Parts<'a> {
    pub tools: &'a Tools,
    pub serving: &'a Serving,
    pub saver: &'a Saver,
    pub on_disk: &'a OnDisk,
    pub life: &'a super::Lifecycle,
}

/// Whether this app keeps its record for a clear's lease: the lock is held **and** the lifecycle says it keeps it. A late try's or a restore's read can still run
/// `restore_record` from `update/before` while the lock is held and the state isn't `Keeps` yet, so the lock alone is not the lease.
pub(crate) fn keeps_the_record(keeping: Keeping, holds_the_lock: bool) -> bool {
    holds_the_lock && keeping == Keeping::Keeps
}

/// What the app knows that no folder says, for a clear's lease and for QA's `ready`.
pub(crate) fn outside(saver: &Saver, on_disk: &OnDisk, life: &super::Lifecycle) -> Outside {
    Outside {
        keeps_record: keeps_the_record(life.keeping(), saver.store.holds_the_record()),
        beside_newer: saver.store.beside_newer(),
        sleep_pass_done: on_disk.sleeper().older_pass_done(),
    }
}

/// What `qa removal-state` says of an unfinished removal's record: what is there (`removal_store::gate::state`, which reads the disk once), and what the record
/// store has withheld because of it.
pub(crate) fn record_state(saver: &Saver) -> Value {
    json!({
        "record": recto_control::removal_store::gate::state(saver.store.state_root()),
        "withheld": saver.store.withheld().map(recto_control::removal_store::gate::Withheld::code),
    })
}

/// `removal check` and `removal clear`, for whoever asked: the level's been checked ([`AppHost::run_app`]). Everything else is the controller's.
pub(crate) fn command(host: &AppHost, parts: Parts<'_>, actor: Actor, command: &Command) -> Result<Value, AgentError> {
    let Command::Removal(family) = command else {
        return Err(removal_needs_app());
    };
    let Parts {
        tools,
        serving,
        saver,
        on_disk,
        life,
    } = parts;
    let removal = tools.removal();
    let root = serving.paths.as_ref().map(|paths| paths.root.clone()).ok_or_else(removal_needs_app)?;
    let (tab, confirm) = match family {
        Family::RemovalCheck(RemovalCheck { tab }) => (tab, None),
        Family::RemovalClear(RemovalClear { tab, confirm }) => (tab, Some(read_plan(confirm)?)),
    };
    let document = file_of(host, actor, tab)?;
    let stamp =
        recto_platform::file_stamp(&document).map_err(|_| refuse(Code::FILE_MISSING, "That tab's file isn't there to look beside.", exit::NOT_FOUND))?;
    let asked = Asked {
        root: &root,
        document: &document,
        stamp,
        actor,
        names: !actor.is_agent(),
    };
    let world = removal.world(&root, serving.paths.as_ref().is_some_and(|paths| paths.overridden));
    within_the_run(&world, &[&root, &document])?;
    // One at a time: a check or a clear while another clear runs is told so, and nothing is done.
    let busy = || refuse(Code::REMOVAL_BUSY, words::CLEARING, exit::BUSY);
    match confirm {
        None => {
            if removal.clearing() {
                return Err(busy());
            }
            // Only one who may clear is given a plan to confirm: the rule is the clear command's own row, so an agent below it can't fill the store.
            let may_confirm = actor.may(row_access(<RemovalClear as Cmd>::TAG));
            let checked = run::check(&removal.store, &asked, may_confirm, &world, &mut run::random_id, Instant::now()).map_err(|error| match error {
                CheckError::Changed => refuse(Code::REMOVAL_PLAN_CHANGED, words::confirm_refusal(ConfirmRefusal::Stale), exit::FAILED),
                CheckError::Offer(why) => refuse(Code::REMOVAL_BUSY, words::confirm_refusal(why), exit::BUSY),
            })?;
            let found = checked.found;
            let mut data = json!({
                "words": checked.words,
                "found": {
                    "saves_here": found.saves_here, "saves_elsewhere": found.saves_elsewhere, "other_folders": found.other_folders,
                    "copies": found.copies, "waiting_here": found.waiting_here, "waiting_elsewhere": found.waiting_elsewhere, "waiting_folders": found.waiting_folders, "no_trash": found.no_trash,
                },
                "offered": checked.id.is_some(),
                "refused": checked.refused.as_ref().is_some_and(|why| matches!(why, LookError::Refused { why: Refusal::Incomplete(_) | Refusal::TooLarge | Refusal::Inconsistent, .. })),
            });
            if !found.is_empty() {
                data["button"] = json!(words::BUTTON);
            }
            if let Some(id) = checked.id {
                data["plan"] = json!(plan_word(id));
            }
            if checked.names.document.is_some() {
                // The tab's own name is what `tabs` gives at every level; the folder it is in is where the person's files are, which is Control's.
                let folder = actor.sees_users_words_and_paths().then_some(checked.names.folder);
                data["untrusted"] = json!({ "document": checked.names.document, "folder": folder });
            }
            Ok(data)
        }
        Some(id) => {
            if removal.clearing.compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst).is_err() {
                return Err(busy());
            }
            let _clearing = Clearing(&removal.clearing);
            let outside = outside(saver, on_disk, life);
            let cleared = run::clear(
                &removal.store,
                &asked,
                id,
                &world,
                outside,
                Instant::now(),
                &mut Held(&removal.hold, removal.limit()),
            )
            .map_err(clear_error)?;
            if cleared.stopped.is_some() {
                return Err(refuse(Code::REMOVAL_STOPPED, cleared.words, exit::FAILED));
            }
            Ok(json!({
                "words": cleared.words,
                "outcome": "complete",
                "cleared": { "trashed": cleared.did.trashed, "copies": cleared.did.copies },
                "acted": cleared.acted,
                "planned": cleared.planned,
            }))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Failure (review round 1, 2): the real app has the stand-in Trash because its state folder, or a link in it, resolves outside the real home, so the
    /// person's unfinished saves go to a hidden folder and are lost to them; or a quiet run is given the Mac's Trash. Only a state folder the run itself named
    /// (overridden) gets the stand-in, wherever it is; the person's own always has the Mac's.
    #[test]
    fn only_a_named_state_folder_gets_the_stand_in_and_the_persons_own_never_does() {
        let removal = Removal::default();
        let temp = std::env::temp_dir().canonicalize().unwrap();
        assert!(!removal.world(&temp, false).stand_in, "not overridden: the Mac's Trash, wherever the folder is");
        assert!(removal.world(&temp, true).stand_in);
    }

    /// Failure (review round 1, 2): a quiet run opened on a person's real document moves its unfinished saves, or deletes beside it. In stand-in mode a path outside
    /// the system's temp folders is refused before anything is looked at; the Mac's own Trash has no such wall.
    #[test]
    fn in_a_quiet_run_nothing_outside_the_temp_folders_is_acted_on() {
        let temp = std::env::temp_dir().canonicalize().unwrap();
        let outside = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target").canonicalize().unwrap();
        let stand_in = World::stand_in(&temp.join("trash-stand-in"));
        assert!(within_the_run(&stand_in, &[&temp]).is_ok());
        assert_eq!(within_the_run(&stand_in, &[&temp, &outside]).unwrap_err().code.as_str(), "outside_quiet_run");
        assert!(
            within_the_run(&World::real(), &[&outside]).is_ok(),
            "the person's own app acts on the person's files"
        );
    }

    /// Failure (review round 1, 6): a clear goes while the lock is held but a late try's restore is still being taken in, when `restore_record` can still run
    /// from `update/before` under it. Only the lock held and the state `Keeps` is the lease; every state that can still read or try, and every one that doesn't
    /// keep, refuses.
    #[test]
    fn the_record_is_kept_only_when_the_lock_is_held_and_the_state_says_keeps() {
        let not_keeping = [
            Keeping::NotAsked,
            Keeping::Trying {
                since_ms: 0,
                tries: 1,
                as_read: 0,
                asking: false,
                by_update: false,
            },
            Keeping::Restoring { since_ms: 0, claimed: true },
            Keeping::LettingGo,
            Keeping::NotKept,
        ];
        for keeping in not_keeping {
            assert!(!keeps_the_record(keeping, true), "{keeping:?} with the lock held");
        }
        assert!(!keeps_the_record(Keeping::Keeps, false), "kept by the state with no lock");
        assert!(keeps_the_record(Keeping::Keeps, true));
    }
}
