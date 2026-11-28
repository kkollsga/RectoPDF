//! Makes a document's blocks on demand, on the generator's one queue ([`crate::generating`]): **what starts it is an agent asking** for a blocks grain (`recto get --grain paragraph`) or
//! `recto wait blocks-ready`. Never opening a tab, never the window.
//!
//! The work is the structure helper's `blocks` job ([`crate::structure::blocks`]), run walled as every job of it is. Each page's candidates are mapped here onto the page's own text,
//! read from the finished corpus, and the mapped blocks go to [`recto_index::corpus::BlocksBuild`], which stages them in `blocks.bin` and writes the header only when the answer ends
//! well: **an answer that wasn't believed to its end, a file that changed (its key looked at by its path too, which a swapped file under the helper's descriptor can't show) and a job
//! that was stopped write nothing.** What was read after a kill, from what the pipe still held, is put and then dropped with the build.

use crate::structure::blocks::{self, BlocksAnswer, BlocksTally, PageStore};
use crate::structure::{Ended, Structure, time_limit};
use recto_agent::blocks::{Blocking, BlocksAsk};
use recto_agent::generated::GenState;
use recto_index::blocks::PageBlocks;
use recto_index::corpus::{self, Begin, BlocksBuild, BlocksMade, BuildLimits, CorpusKey, PutError};
use recto_platform::helper::Priority;
use serde_json::{Value, json};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use crate::generating::{Generator, Job, What};

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

impl Outcome {
    pub(crate) fn code(&self) -> Option<&str> {
        match self {
            Self::Refused(code) | Self::Failed(code) => Some(code),
            _ => None,
        }
    }
}

/// What making them comes down to, so a test can stand in for a process. `stop` is set when they're no longer wanted, and the runner ends as soon as it sees it.
pub trait Runner: Send + Sync {
    fn run(&self, job: &BlocksAsk, stop: &AtomicBool) -> (Outcome, Value);
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
    /// [`crate::helpers::HELPER_PRIORITY`]. Anything else only to measure the work apart from the waiting.
    pub priority: Priority,
    limits: BuildLimits,
    /// A helper to use in place of the installed one: the one laid out in `target/Helpers`, in a test.
    #[cfg(any(test, feature = "test-seams"))]
    exe: Option<PathBuf>,
    /// Debug builds of the helper only: it spins this long before it reads, which is the time to change the file in.
    #[cfg(any(test, feature = "test-seams"))]
    busy_ms: Option<u64>,
}

impl Walled {
    pub fn new(priority: Priority) -> Self {
        Self {
            priority,
            limits: BuildLimits::default(),
            #[cfg(any(test, feature = "test-seams"))]
            exe: None,
            #[cfg(any(test, feature = "test-seams"))]
            busy_ms: None,
        }
    }

    /// For a test: the helper at `exe`, at this process's own level.
    #[cfg(any(test, feature = "test-seams"))]
    pub fn at(exe: PathBuf) -> Self {
        Self {
            priority: Priority::Inherited,
            limits: BuildLimits::default(),
            exe: Some(exe),
            busy_ms: None,
        }
    }

    /// For a test: the helper spins this long before it reads, so a file can be changed under it.
    #[cfg(any(test, feature = "test-seams"))]
    #[must_use]
    pub fn busy_for(mut self, ms: u64) -> Self {
        self.busy_ms = Some(ms);
        self
    }

    /// For a test: other limits, such as a document's size set small.
    #[cfg(any(test, feature = "test-seams"))]
    #[must_use]
    pub fn with_limits(mut self, limits: BuildLimits) -> Self {
        self.limits = limits;
        self
    }

    fn structure(&self) -> Option<Structure> {
        #[cfg(any(test, feature = "test-seams"))]
        if let Some(exe) = &self.exe {
            return Some(Structure::at(exe.clone(), self.priority));
        }
        Structure::installed(self.priority)
    }
}

impl Runner for Walled {
    fn run(&self, job: &BlocksAsk, stop: &AtomicBool) -> (Outcome, Value) {
        let build = match BlocksBuild::begin(&job.grant, &job.corpus_root, job.key, &self.limits) {
            Begin::Building(build) => *build,
            Begin::Made(made) => return (Outcome::AlreadyMade(made), Value::Null),
            Begin::NotIndexed => return (Outcome::Failed("index_not_ready".into()), Value::Null),
            Begin::Busy => return (Outcome::Failed("busy".into()), Value::Null),
            Begin::Damaged => return (Outcome::Failed("index_damaged".into()), Value::Null),
        };
        // Without the helper the build is dropped, which leaves nothing.
        let Some(structure) = self.structure() else {
            return (Outcome::Failed("helper_missing".into()), Value::Null);
        };
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
            note_tally(report, &tally);
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

fn object(report: &mut Value) -> &mut serde_json::Map<String, Value> {
    if !report.is_object() {
        *report = json!({});
    }
    report.as_object_mut().expect("just made an object")
}

fn note_tally(report: &mut Value, tally: &BlocksTally) {
    object(report).insert("map_ms".into(), json!(tally.map_ms));
}

/// What was stored, as the header says it: the session's own numbers.
fn note_made(report: &mut Value, made: &BlocksMade) {
    let numbers = object(report);
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
        self.want_job(Job::Blocks(ask));
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
