//! Clear leftovers as two calls, **check** and **clear** (`docs/design/removal.md` §10, D5): the survey and plan, the confirmation, the leases, the recheck, the
//! acts and what they came to. Everything here is Rust deciding; the app gives it a place for the pending plans, the facts only it knows and a clock.
//!
//! - **[`check`]** reads only: it surveys the three stores for one document, plans, and, if there is something to do and the asker may confirm, keeps the plan
//!   under a random id ([`random_id`]; an agent never supplies one). It takes no lease and acts on nothing.
//! - **[`clear`]** takes the leases ([`Leases`]) once each, never waited for, and then looks again in full under them, so a folder missing at the check is
//!   listed now and a temp made since changes the digest. Only then is the confirmation taken, against the plan just made: any change is
//!   [`ConfirmRefusal::PlanChanged`], the entry is gone and nothing is done. The walk acts by what the survey pinned ([`act::clear`]), one acknowledgment a step.
//!   A deadline is looked at before every step; past it the walk stops where it is. `VerifyRetired` surveys once more and finds no cleared name.
//!
//! No barrier is needed. Every act is a move into the Trash or an unlink of the app's own copy, and the next check sees exactly what is left, so a crash at any
//! point is cleared by checking again and confirming what remains. Nothing here touches a document, a journal, a backup, the ledger or the live record.

use super::act::{self, Done, Env, Hooks, Unwatched, Why};
use super::leases::{Busy, Leases, Outside};
use super::pin::{Local, PinSet, Pins};
use super::trash::{FolderTrash, MacTrash, Trash};
use super::words::{self, Did, Found, Names};
use crate::removal_store::ledger;
use crate::removal_store::legacy_session::{LegacyCaps, survey_legacy_sessions_with};
use crate::removal_store::update_copy::{UpdateCaps, survey_update_copy_with};
use crate::removal_store::writer_temps::{TempCaps, survey_writer_temps_with};
use recto_core::removal::{
    ArtifactKey, Binding, CONFIRM_LIMITS, ConfirmRefusal, Confirmations, DESIGN_LIMITS, Effect, LEDGER_LIMITS, Outcome, Plan, PlanId, Progress, Refusal,
    Request, Survey, Surveyed, TargetKey, TargetMode, Version, lineage,
};
use recto_core::save::Target;
use recto_platform::FileStamp;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

/// How long a clear may take before it stops where it is. A design limit, not a measurement.
pub const CLEAR_DEADLINE: Duration = Duration::from_secs(10);

/// The outside facts an offer depends on and an act re-asks: the Trash, whether a process is running, and whether a volume is local.
#[derive(Clone)]
pub struct World {
    pub trash: Arc<dyn Trash>,
    pub alive: Arc<dyn Fn(u32) -> bool + Send + Sync>,
    pub local: Local,
    /// The Trash is a folder standing in for the Mac's, for a quiet run or a test, which acts only on a quiet run's files.
    pub stand_in: bool,
}

impl World {
    /// The Mac's own Trash, process table and volumes.
    pub fn real() -> Self {
        Self {
            trash: Arc::new(MacTrash),
            alive: Arc::new(recto_platform::local::process_exists),
            local: Arc::new(recto_platform::folder::is_local),
            stand_in: false,
        }
    }

    /// A quiet run's or a test's: the real process table and volumes, and `folder` for a Trash that takes only a quiet run's files.
    pub fn stand_in(folder: &Path) -> Self {
        Self {
            trash: Arc::new(FolderTrash::in_quiet_run(folder)),
            stand_in: true,
            ..Self::real()
        }
    }

    fn pins(&self) -> Pins {
        Pins::new(Arc::clone(&self.trash), Arc::clone(&self.alive), Arc::clone(&self.local))
    }
}

/// Which document, for whom.
pub struct Asked<'a, A> {
    pub root: &'a Path,
    /// The tab's file, as the tab has it.
    pub document: &'a Path,
    /// What the file was when this was asked: a Save changes it, an edit doesn't.
    pub stamp: FileStamp,
    pub actor: A,
    /// Whether the words may carry the document's and its folder's names (the person's, never an agent's).
    pub names: bool,
}

/// What was looked at: the plan, and what its artifacts were.
pub struct Looked {
    pub plan: Plan,
    surveyed: Surveyed,
    pins: PinSet,
    /// The document's real path, which a confirmation is bound to.
    canonical: PathBuf,
}

/// Why nothing could be planned.
#[derive(Debug, PartialEq, Eq)]
pub enum LookError {
    /// The file is no longer what it was when it was asked about.
    Changed,
    /// The surveys or the plan refused. `went_back`: a copy holds work of an earlier version, which is the reason to say so.
    Refused { why: Refusal, went_back: bool },
}

fn survey(asked_root: &Path, document: &Path, stamp: FileStamp, world: &World) -> Result<(Surveyed, PinSet, PathBuf), LookError> {
    let refuse = |why| LookError::Refused { why, went_back: false };
    let (canonical, version) = ledger::canonical_version(document, stamp).ok_or(LookError::Changed)?;
    let ledger = ledger::read(asked_root);
    let lineage = lineage(&ledger, version, &canonical, &LEDGER_LIMITS).map_err(refuse)?;
    let sink = world.pins();
    let mut survey = Survey::new(&DESIGN_LIMITS);
    let temps = TempCaps {
        pins: Some(sink.clone()),
        ..TempCaps::default()
    };
    survey_writer_temps_with(&lineage, &mut survey, &temps).map_err(refuse)?;
    let legacy = LegacyCaps {
        pins: Some(sink.clone()),
        ..LegacyCaps::default()
    };
    survey_legacy_sessions_with(asked_root, &lineage, TargetMode::Save, &mut survey, &legacy).map_err(refuse)?;
    let update = UpdateCaps {
        pins: Some(sink.clone()),
        ..UpdateCaps::default()
    };
    survey_update_copy_with(asked_root, &lineage, TargetMode::Save, &mut survey, &update).map_err(refuse)?;
    let surveyed = survey.finish().map_err(refuse)?;
    Ok((surveyed, sink.set(), canonical))
}

/// Surveys the three stores for the document and plans what Clear leftovers would do.
pub fn look(root: &Path, document: &Path, stamp: FileStamp, world: &World) -> Result<Looked, LookError> {
    let (surveyed, pins, canonical) = survey(root, document, stamp, world)?;
    let went_back = pins.copies().iter().any(|copy| copy.went_back());
    let plan = recto_core::removal::plan(&Request::leftovers(), &surveyed.inventory(), &DESIGN_LIMITS).map_err(|why| LookError::Refused { why, went_back })?;
    Ok(Looked {
        plan,
        surveyed,
        pins,
        canonical,
    })
}

fn label(path: &Path) -> Option<String> {
    path.file_name().map(|name| recto_core::clean_line(&name.to_string_lossy()))
}

impl Looked {
    /// What the plan would do and what it leaves, as counts.
    pub fn found(&self) -> Found {
        let here = self.canonical.parent();
        let (mut saves_here, mut saves_elsewhere) = (0, 0);
        let mut folders: HashSet<&Path> = HashSet::new();
        for temp in self.pins.temps() {
            if Some(temp.folder()) == here {
                saves_here += 1;
            } else {
                saves_elsewhere += 1;
                folders.insert(temp.folder());
            }
        }
        let count = |len: usize| u32::try_from(len).unwrap_or(u32::MAX);
        let (mut waiting_here, mut waiting_elsewhere) = (0, 0);
        let mut waiting_folders: HashSet<&Path> = HashSet::new();
        for waiting in self.pins.waiting() {
            if Some(waiting.folder()) == here {
                waiting_here += 1;
            } else {
                waiting_elsewhere += 1;
                waiting_folders.insert(waiting.folder());
            }
        }
        Found {
            saves_here,
            saves_elsewhere,
            other_folders: count(folders.len()),
            copies: count(self.pins.copies().len()),
            waiting_here,
            waiting_elsewhere,
            waiting_folders: count(waiting_folders.len()),
            no_trash: count(self.pins.no_trash().len()),
        }
    }

    /// The names the words may use, for the person.
    pub fn names(&self) -> Names {
        Names {
            document: label(&self.canonical),
            folder: self.canonical.parent().and_then(label),
        }
    }

    fn binding<A: Copy + Eq>(&self, asked: &Asked<'_, A>) -> Binding<A> {
        Binding {
            actor: asked.actor,
            // Only a Save changes what the file is; an edit in a tab doesn't, and a plan that is about what is beside the file stays good.
            version: Version { epoch: 0, source: asked.stamp },
            target: TargetKey::new(&self.canonical, Target::Same),
        }
    }
}

/// A random plan id: never seeded outside a test, never an agent's to give.
pub fn random_id() -> u64 {
    let mut bytes = [0u8; 8];
    // No randomness is 0, which is no id: the offer then fails rather than use a guessable one.
    getrandom::fill(&mut bytes).map_or(0, |()| u64::from_le_bytes(bytes))
}

/// What a check came to.
#[derive(Debug)]
pub struct Checked {
    pub found: Found,
    /// The document's and its folder's cleaned names, which the person's words use and an agent's `untrusted` carries.
    pub names: Names,
    /// The plan to confirm, if there is anything to do and the asker may confirm.
    pub id: Option<PlanId>,
    /// Why nothing could be planned, and the words for it.
    pub refused: Option<LookError>,
    pub words: String,
}

/// Why a check couldn't offer.
#[derive(Debug, PartialEq, Eq)]
pub enum CheckError {
    /// The file is no longer what it was when it was asked about.
    Changed,
    /// The plan couldn't be kept.
    Offer(ConfirmRefusal),
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Looks, and offers. `may_confirm` is whether the asker is one who may clear (the one rule for it is the command's, passed here): a plan is kept only for
/// them, so nobody below that level can fill the store. Reads only.
pub fn check<A: Copy + Eq>(
    store: &Mutex<Confirmations<A>>,
    asked: &Asked<'_, A>,
    may_confirm: bool,
    world: &World,
    ids: &mut dyn FnMut() -> u64,
    now: Instant,
) -> Result<Checked, CheckError> {
    let looked = match look(asked.root, asked.document, asked.stamp, world) {
        Ok(looked) => looked,
        Err(LookError::Changed) => return Err(CheckError::Changed),
        Err(LookError::Refused { why, went_back }) => {
            return Ok(Checked {
                found: Found::default(),
                names: Names::default(),
                id: None,
                words: words::refusal(why, went_back),
                refused: Some(LookError::Refused { why, went_back }),
            });
        }
    };
    let found = looked.found();
    let names = looked.names();
    let words = words::check(&if asked.names { names.clone() } else { Names::default() }, &found);
    let mut id = None;
    if !found.is_empty() && may_confirm {
        let binding = looked.binding(asked);
        let mut kept = Err(ConfirmRefusal::Full);
        for _ in 0..8 {
            let Some(candidate) = PlanId::from_raw(ids()) else { continue };
            kept = lock(store).offer(binding, &looked.plan, now, candidate);
            if kept != Err(ConfirmRefusal::Collision) {
                break;
            }
        }
        id = Some(kept.map_err(CheckError::Offer)?);
    }
    Ok(Checked {
        found,
        names,
        id,
        refused: None,
        words,
    })
}

/// Why nothing was cleared.
#[derive(Debug, PartialEq, Eq)]
pub enum ClearError {
    Busy(Busy),
    Look(LookError),
    Confirm(ConfirmRefusal),
}

/// What a clear did.
#[derive(Debug)]
pub struct Cleared {
    pub did: Did,
    pub outcome: Outcome,
    /// Why the walk stopped, if it did.
    pub stopped: Option<String>,
    /// Artifacts acted on, and how many the plan had.
    pub acted: u32,
    pub planned: u32,
    pub words: String,
}

/// Where in a step a [`Steps`] is asked.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Moment {
    /// Before the step's act.
    Before,
    /// The act is done, and not yet acknowledged.
    Acted,
    /// The acknowledgment is made.
    Acknowledged,
    /// The leases are taken, before anything is looked at again.
    Leased,
}

/// What a caller sees of the walk and may stop: the app's QA hold, and a test's crash. `false` stops the walk where it is, as a crash would.
pub trait Steps: Hooks {
    /// `deadline` is when the clear must stop: a hold that waits waits no later.
    fn at(&mut self, _step: u32, _moment: Moment, _deadline: Instant) -> bool {
        true
    }

    /// How long the clear may take from when it was asked. A test's or a QA run's seam: the design's limit unless it says another.
    fn limit(&self) -> Duration {
        CLEAR_DEADLINE
    }
}

/// The walk's hooks with the deadline looked at before every artifact and every entry of a copy, so a long copies step stops at it as a slow step does.
struct Deadlined<'a> {
    steps: &'a mut dyn Steps,
    deadline: Instant,
}

impl Hooks for Deadlined<'_> {
    fn before(&mut self, key: &ArtifactKey) -> bool {
        Instant::now() < self.deadline && self.steps.before(key)
    }

    fn after(&mut self, key: &ArtifactKey, done: &Done) {
        self.steps.after(key, done);
    }

    fn entry(&mut self, name: &std::ffi::OsStr) -> bool {
        Instant::now() < self.deadline && self.steps.entry(name)
    }
}

impl Steps for Unwatched {}

/// A short clause for a stop, never a name.
fn why_words(why: &Why) -> &'static str {
    match why {
        Why::NotPinned => "RectoPDF no longer had it pinned",
        Why::BeingWritten => "a save started being written",
        Why::NoTrash => "there is no Trash for its folder",
        Why::WentBack => "a copy holds work of an earlier version",
        Why::Changed(_) | Why::Unpinned(_) => "something changed in the folder",
        Why::TooDeep => "a copy is nested deeper than RectoPDF clears",
        Why::Trash(_) => "the Trash didn't take it",
        Why::NotMoved(_) => "RectoPDF couldn't confirm what the Trash took",
        Why::LiveRecord => "that is the folder your session is kept in",
        Why::NotDurable => "RectoPDF couldn't be sure it was written to the disk",
        Why::Io(..) => "the disk refused",
        Why::Asked => "it was stopped",
    }
}

/// What a clear says when it stopped for its time.
const RAN_OUT: &str = "it ran out of time";

fn count(len: usize) -> u32 {
    u32::try_from(len).unwrap_or(u32::MAX)
}

/// Confirms plan `id` and clears it. The leases come first and the recheck under them; the confirmation is taken against the plan just made.
pub fn clear<A: Copy + Eq>(
    store: &Mutex<Confirmations<A>>,
    asked: &Asked<'_, A>,
    id: PlanId,
    world: &World,
    outside: Outside,
    now: Instant,
    steps: &mut dyn Steps,
) -> Result<Cleared, ClearError> {
    let deadline = now + steps.limit();
    let mut leases = Leases::take(asked.root, outside).map_err(ClearError::Busy)?;
    steps.at(0, Moment::Leased, deadline);
    let looked = look(asked.root, asked.document, asked.stamp, world).map_err(ClearError::Look)?;
    // An `update/` that has appeared since the leases is leased now, or the clear doesn't go; and before the confirmation is taken, so a refusal here
    // doesn't use the plan up.
    leases.look_again().map_err(ClearError::Busy)?;
    let confirmed = lock(store).take(id, &looked.binding(asked), &looked.plan, now).map_err(ClearError::Confirm)?;
    let mut progress = Progress::start(&looked.plan, confirmed).map_err(|_| ClearError::Confirm(ConfirmRefusal::PlanChanged))?;
    Ok(walk(asked, &looked, &mut progress, world, Some(leases), deadline, steps))
}

fn walk<A: Copy + Eq>(
    asked: &Asked<'_, A>,
    looked: &Looked,
    progress: &mut Progress,
    world: &World,
    mut leases: Option<Leases>,
    deadline: Instant,
    steps: &mut dyn Steps,
) -> Cleared {
    let plan = &looked.plan;
    let live = [asked.root.join("session"), asked.root.join("session-windowless")];
    let env = Env {
        trash: &*world.trash,
        alive: &*world.alive,
        local: &*world.local,
        live: [&live[0], &live[1]],
    };
    let planned = count(
        plan.steps()
            .iter()
            .filter(|s| matches!(s.effect(), Effect::Trash(_) | Effect::Purge(_)))
            .map(|s| plan.artifacts(s).len())
            .sum(),
    );
    let (mut did, mut acted) = (Did::default(), 0u32);
    let mut done_keys: Vec<ArtifactKey> = Vec::new();
    let mut stopped: Option<String> = None;
    while let Some(step) = progress.next(plan) {
        let index = progress.acked();
        if Instant::now() >= deadline {
            stopped = Some(RAN_OUT.to_owned());
            break;
        }
        if !steps.at(index, Moment::Before, deadline) {
            stopped = Some(why_words(&Why::Asked).to_owned());
            break;
        }
        let ack = match step.effect() {
            Effect::TakeLeases => recto_core::removal::Ack::Done,
            Effect::Trash(_) | Effect::Purge(_) => {
                let trash = matches!(step.effect(), Effect::Trash(_));
                let keys: Vec<ArtifactKey> = plan
                    .artifacts(&step)
                    .iter()
                    .filter_map(|id| looked.surveyed.key(*id))
                    .map(|(_, key)| key)
                    .collect();
                let report = act::clear(&looked.pins, &keys, &env, &mut Deadlined { steps: &mut *steps, deadline });
                for (key, result) in &report.done {
                    match result {
                        Done::Trashed(_) => did.trashed += 1,
                        Done::Deleted { unlinked, .. } if *unlinked > 0 => did.copies += 1,
                        _ => {}
                    }
                    acted += 1;
                    done_keys.push(*key);
                }
                did.partly = did.partly.or(report.partly);
                match report.stopped {
                    None => recto_core::removal::Ack::Done,
                    Some((_, why)) => {
                        let late = matches!(why, Why::Asked) && Instant::now() >= deadline;
                        stopped = Some(if late { RAN_OUT } else { why_words(&why) }.to_owned());
                        if trash && why.in_doubt() {
                            recto_core::removal::Ack::DoneNotDurable
                        } else {
                            recto_core::removal::Ack::Failed
                        }
                    }
                }
            }
            Effect::VerifyRetired => match verify(asked, world, &done_keys) {
                Ok(()) => recto_core::removal::Ack::Done,
                Err(why) => {
                    stopped = Some(why.to_owned());
                    recto_core::removal::Ack::Failed
                }
            },
            Effect::ReleaseLeases => {
                leases = None;
                recto_core::removal::Ack::Done
            }
            // No other step is in this scope's plan.
            _ => {
                stopped = Some("RectoPDF has no way to do that step".to_owned());
                recto_core::removal::Ack::Failed
            }
        };
        if !steps.at(index, Moment::Acted, deadline) {
            stopped = Some(why_words(&Why::Asked).to_owned());
            break;
        }
        if progress.acknowledge(plan, index, ack).is_err() || ack != recto_core::removal::Ack::Done {
            stopped.get_or_insert_with(|| "RectoPDF lost its place".to_owned());
            break;
        }
        if !steps.at(index, Moment::Acknowledged, deadline) {
            stopped = Some(why_words(&Why::Asked).to_owned());
            break;
        }
    }
    drop(leases);
    // The outcome and the words are one fact: what `did` says was done. (`Progress` counts any Purge it was told of as an act, which a stop before the first
    // unlink was not.)
    let outcome = match (&stopped, did.is_none()) {
        (None, _) => progress.outcome(plan).unwrap_or(Outcome::Unchanged),
        (Some(_), true) => Outcome::Unchanged,
        (Some(_), false) => Outcome::PartlyCleared,
    };
    let words = match &stopped {
        None => words::done(&did),
        Some(why) => words::stopped(&did, acted, planned, why),
    };
    Cleared {
        did,
        outcome,
        stopped,
        acted,
        planned,
        words,
    }
}

/// Surveys once more: nothing that was cleared is listed again.
fn verify<A: Copy + Eq>(asked: &Asked<'_, A>, world: &World, done: &[ArtifactKey]) -> Result<(), &'static str> {
    let (_, pins, _) = survey(asked.root, asked.document, asked.stamp, world).map_err(|_| "RectoPDF couldn't look again to be sure")?;
    let left = pins.temps().iter().map(|pin| pin.key()).chain(pins.copies().iter().map(|pin| pin.key()));
    if left.into_iter().any(|key| done.contains(key)) {
        return Err("something it cleared was still there");
    }
    Ok(())
}

/// An empty store of pending plans, with the design's limits.
pub fn new_store<A: Copy + Eq>() -> Mutex<Confirmations<A>> {
    Mutex::new(Confirmations::new(CONFIRM_LIMITS))
}

#[cfg(test)]
mod tests;
