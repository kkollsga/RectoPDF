//! Makes a document's blocks on demand, on the generator's one queue ([`crate::generating`]): **what starts it is an agent asking** for a blocks grain (`recto get --grain paragraph`) or
//! `recto wait blocks-ready`. Never opening a tab, never the window.
//!
//! The work is the structure helper's `blocks` job ([`crate::structure::blocks`]), run walled as every job of it is. Each page's candidates are mapped here onto the page's own text,
//! read from the finished corpus, and the mapped blocks go to [`recto_index::corpus::BlocksBuild`], which stages them in `blocks.bin` and writes the header only when the answer ends
//! well: **an answer that wasn't believed to its end, a file that changed (its key looked at by its path too, which a swapped file under the helper's descriptor can't show) and a job
//! that was stopped write nothing.** What was read after a kill, from what the pipe still held, is put and then dropped with the build.

use crate::structure::blocks::{self, BlocksAnswer, BlocksTally, PageStore};
use crate::structure::{Ended, Pick, numbers_of, time_limit};
use recto_agent::blocks::{Blocking, BlocksAsk};
use recto_agent::generated::GenState;
use recto_index::blocks::PageBlocks;
use recto_index::corpus::{self, Begin, BlocksBuild, BlocksMade, BuildLimits, CorpusKey, PutError};
use recto_platform::helper::Priority;
use serde_json::{Value, json};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use crate::generating::{By, Ending, Generator, Job, Verdict, What};
use recto_agent::Event;

/// How a blocks job ended.
#[derive(Debug)]
pub enum Outcome {
    Ready(BlocksMade),
    /// Nothing to do: they were made already, under these rules.
    AlreadyMade(BlocksMade),
    /// This version of the file won't have any, for this reason.
    Refused(String),
    Changed,
    /// Not about the file, or stopped at a limit.
    Failed(String),
}

impl Ending for Outcome {
    fn verdict(&self) -> Verdict<'_> {
        match self {
            Self::Ready(_) | Self::AlreadyMade(_) => Verdict::Done,
            Self::Refused(code) => Verdict::Refused(code),
            Self::Changed => Verdict::Changed,
            Self::Failed(code) => Verdict::Failed(code),
        }
    }
}

/// The codes of a limit a blocks job can end at, the version's own doing: past one it isn't run again for that version. The helper's two, the store's size, and a page count the helper and the index can't agree on.
const LIMITS: &[&str] = &["over_memory", "out_of_time", "too_large", "pages_differ"];

pub(crate) fn is_limit(code: &str) -> bool {
    LIMITS.contains(&code)
}

/// The news that blocks are being made for a file.
pub(crate) fn building(ask: &BlocksAsk) -> Event {
    Event::BlocksBuilding {
        path: ask.path.clone(),
        file: ask.key.short(),
    }
}

/// The news of how a job ended: numbers only, the session's own. **None when nothing changed** (the blocks were already made), as nothing is announced then.
pub(crate) fn built(ask: &BlocksAsk, state: &'static str, outcome: &Outcome, report: &Value) -> Option<Event> {
    if matches!(outcome, Outcome::AlreadyMade(_)) {
        return None;
    }
    Some(Event::BlocksBuilt {
        path: ask.path.clone(),
        file: ask.key.short(),
        state,
        code: outcome.code().map(str::to_string),
        elapsed_ms: report["elapsed_ms"].clone(),
        helper_peak_bytes: report["helper_peak_watched_bytes"].clone(),
        walled: report["walled"].clone(),
        pages: report["pages"].clone(),
        stored: report["stored"].clone(),
        dropped: report["dropped"].clone(),
        bytes: report["bytes"].clone(),
        map_ms: report["map_ms"].clone(),
    })
}

/// What making them comes down to, so a test can stand in for a process. `stop` is set when they're no longer wanted, and the runner ends as soon as it sees it.
pub trait Runner: Send + Sync {
    /// `begun` is called when it starts making them, and not when they were already made: only work that was done is announced.
    fn run(&self, job: &BlocksAsk, stop: &AtomicBool, begun: &dyn Fn()) -> (Outcome, Value);
}

/// The store the helper's answer is mapped into: the corpus's own build.
pub(crate) struct Store(pub(crate) BlocksBuild);

impl PageStore for Store {
    fn text(&mut self, page: u32, chars: &mut Vec<char>) -> bool {
        self.0.text(page, chars)
    }

    fn put(&mut self, page: &PageBlocks) -> Result<(), &'static str> {
        self.0.put(page).map_err(|error| match error {
            PutError::TooLarge => "too_large",
            PutError::Write => "write_failed",
        })
    }
}

/// The real thing: the walled structure helper, the answer mapped onto the corpus's text and stored beside it.
#[derive(Clone, Debug)]
pub struct Walled {
    pick: Pick,
    limits: BuildLimits,
    /// Debug builds of the helper only: it spins this long before it reads, which is the time to change the file in.
    #[cfg(any(test, feature = "test-seams"))]
    busy_ms: Option<u64>,
}

impl Walled {
    pub fn new(priority: Priority) -> Self {
        Self::with(Pick::new(priority))
    }

    /// For a test: the helper at `exe`, at this process's own level.
    #[cfg(any(test, feature = "test-seams"))]
    pub fn at(exe: PathBuf) -> Self {
        Self::with(Pick::at(exe))
    }

    fn with(pick: Pick) -> Self {
        Self {
            pick,
            limits: BuildLimits::default(),
            #[cfg(any(test, feature = "test-seams"))]
            busy_ms: None,
        }
    }

    /// For a test: other limits, such as a document's size set small.
    #[cfg(any(test, feature = "test-seams"))]
    #[must_use]
    pub fn with_limits(mut self, limits: BuildLimits) -> Self {
        self.limits = limits;
        self
    }

    /// For a test: the helper spins this long before it reads, so a file can be changed under it.
    #[cfg(any(test, feature = "test-seams"))]
    #[must_use]
    pub fn busy_for(mut self, ms: u64) -> Self {
        self.busy_ms = Some(ms);
        self
    }
}

impl Runner for Walled {
    fn run(&self, job: &BlocksAsk, stop: &AtomicBool, begun: &dyn Fn()) -> (Outcome, Value) {
        let build = match BlocksBuild::begin(&job.grant, &job.corpus_root, job.key, &self.limits) {
            Begin::Building(build) => *build,
            Begin::Made(made) => return (Outcome::AlreadyMade(made), Value::Null),
            Begin::NotIndexed => return (Outcome::Failed("index_not_ready".into()), Value::Null),
            Begin::Busy => return (Outcome::Failed("busy".into()), Value::Null),
            Begin::Damaged => return (Outcome::Failed("index_damaged".into()), Value::Null),
        };
        // Without the helper the build is dropped, which leaves nothing.
        let Some(structure) = self.pick.structure() else {
            return (Outcome::Failed("helper_missing".into()), Value::Null);
        };
        begun();
        let pages = build.pages();
        let deadline = Instant::now() + time_limit(pages);
        let answer = BlocksAnswer::new(Store(build), pages);
        #[allow(unused_mut)]
        let mut line = blocks::job();
        #[cfg(any(test, feature = "test-seams"))]
        if let Some(ms) = self.busy_ms {
            line["test_busy_ms"] = ms.into();
        }
        let (ended, mut report) = structure.run(&job.path, job.key, &line, answer, deadline, stop);
        let outcome = outcome_of(ended, || CorpusKey::of_file(&job.path).is_ok_and(|now| now == job.key), stop, &mut report);
        (outcome, report)
    }
}

/// What the end of a job comes to, and the only place its blocks are made whole: **only a believed answer to its end, of the file that was asked for, not stopped since, gets its header.**
/// `unchanged` looks at the file by its path: defence in depth, as [`Structure::run`] already finds a rename swap by the descriptor's own key (the unlink changes the old file's ctime, which the key holds, measured
/// with `a_file_rewritten_while_blocks_are_made_gets_none`), and this covers a swap after that look. Anything else drops the build, which leaves nothing on disk.
pub(crate) fn outcome_of(ended: Ended<BlocksAnswer<Store>>, unchanged: impl FnOnce() -> bool, stop: &AtomicBool, report: &mut Value) -> Outcome {
    match ended {
        Ended::Answered(answer) => {
            let (store, tally) = answer.into_parts();
            if stop.load(Ordering::SeqCst) || !unchanged() {
                return Outcome::Changed;
            }
            match store.0.finish(&index_tally(&tally)) {
                Ok(made) => {
                    note_made(report, &made);
                    Outcome::Ready(made)
                }
                Err(_) => Outcome::Failed("write_failed".into()),
            }
        }
        Ended::Refused(code) => Outcome::Refused(code),
        Ended::Changed => Outcome::Changed,
        Ended::Failed(code) => Outcome::Failed(code),
    }
}

fn index_tally(tally: &BlocksTally) -> corpus::BlocksTally {
    corpus::BlocksTally {
        pages_without_text: tally.pages_without_text,
        unreadable: tally.unreadable,
        stored: tally.stored,
        dropped: tally.dropped,
    }
}

/// What was stored, as the header says it: the session's own numbers.
fn note_made(report: &mut Value, made: &BlocksMade) {
    let numbers = numbers_of(report);
    numbers.insert("pages".into(), json!(made.pages));
    numbers.insert("stored".into(), json!(made.stored.iter().map(|count| u64::from(*count)).sum::<u64>()));
    numbers.insert(
        "dropped".into(),
        json!(made.dropped.iter().map(|count| u64::from(*count)).sum::<u64>() + u64::from(made.unreadable)),
    );
    numbers.insert("bytes".into(), json!(made.bytes));
}

impl Blocking for Generator {
    fn want(&self, ask: BlocksAsk) {
        self.want_job(By::Agent, Job::Blocks(ask));
    }

    fn state(&self, key: CorpusKey) -> GenState {
        self.state_of(What::Blocks, key)
    }

    fn cancel_files(&self, paths: Option<&[PathBuf]>) {
        self.cancel_paths(What::Blocks, paths);
    }

    fn making(&self, paths: Option<&[PathBuf]>) -> bool {
        self.making_paths(What::Blocks, paths)
    }
}

#[cfg(test)]
mod tests;
