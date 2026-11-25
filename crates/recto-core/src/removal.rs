//! Pure, bounded retention plan for a safe-removal transaction (`docs/design/removal.md` §3 and §6).
//!
//! Given a typed inventory of what the app retains about one document, `plan` returns the fixed
//! sequence of effects a later carrier performs, one acknowledged step at a time, or refuses. `Progress`
//! classifies a crash only from the acknowledged prefix. Nothing here opens a file, names a path, spawns
//! work, keeps state per tab or decides who may ask: ownership arrives already proven from later
//! per-store adapters, and an unproven artifact refuses the plan instead of being guessed.
//!
//! Every value is small and copyable, no refusal repeats input, and the input is capped before any
//! allocation.

mod confirm;
mod digest;
mod lineage;
mod plan;
mod progress;
mod survey;

pub use confirm::{Binding, CONFIRM_LIMITS, ConfirmLimits, ConfirmRefusal, Confirmations, Confirmed, PlanId, TargetKey, Version};
pub use digest::PlanDigest;
pub use lineage::{LEDGER_LIMITS, LedgerLimits, Lineage, Link, Side, Sides, lineage};
pub use plan::plan;
pub use progress::{Ack, OrderError, Outcome, Progress};
pub use survey::{ArtifactKey, Found, Survey, Surveyed};

use std::num::NonZeroU16;

/// How a store is treated by removal.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Class {
    /// Not inventoried, so an artifact claimed here is a bug. Such a store is still dealt with as a
    /// whole where it matters (the source replaced, find text cleared, live views invalidated, helper
    /// jobs quiesced), or it is excluded from removal and the report says so.
    Fixed,
    /// Whole files proven to be the target's are deleted.
    Purge,
    /// A container shared with other documents: the target's entries are removed, the file stays.
    Rewrite,
    /// Cannot take part yet; target material refuses the plan.
    Unsupported,
}

/// A place the app may retain document material, in `docs/design/removal.md` §3 row order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Store {
    SourcePdf,
    /// The sibling file a writer is producing. The candidate being written for this removal is never
    /// inventoried here: it is the carrier's own, not retained material.
    WriterTemp,
    Backup,
    Journal,
    JournalVariant,
    KeptPages,
    PageIds,
    SleepFile,
    SessionRecord,
    FindText,
    LegacySession,
    CorpusText,
    CorpusAgentPages,
    CorpusAliases,
    GeneratedContents,
    UpdateCopy,
    LiveViews,
    HelperJobs,
    Logs,
    Clipboard,
    External,
    Library,
}

impl Store {
    pub const COUNT: usize = 22;
    pub const ALL: [Store; Store::COUNT] = [
        Store::SourcePdf,
        Store::WriterTemp,
        Store::Backup,
        Store::Journal,
        Store::JournalVariant,
        Store::KeptPages,
        Store::PageIds,
        Store::SleepFile,
        Store::SessionRecord,
        Store::FindText,
        Store::LegacySession,
        Store::CorpusText,
        Store::CorpusAgentPages,
        Store::CorpusAliases,
        Store::GeneratedContents,
        Store::UpdateCopy,
        Store::LiveViews,
        Store::HelperJobs,
        Store::Logs,
        Store::Clipboard,
        Store::External,
        Store::Library,
    ];

    /// An exhaustive match: a new store cannot be added without choosing its class.
    pub const fn class(self) -> Class {
        match self {
            Store::SourcePdf | Store::FindText | Store::LiveViews | Store::HelperJobs => Class::Fixed,
            Store::Logs | Store::Clipboard | Store::External => Class::Fixed,
            Store::WriterTemp
            | Store::Backup
            | Store::Journal
            | Store::JournalVariant
            | Store::KeptPages
            | Store::PageIds
            | Store::SleepFile
            | Store::CorpusText
            | Store::CorpusAgentPages
            | Store::GeneratedContents => Class::Purge,
            Store::SessionRecord | Store::LegacySession | Store::CorpusAliases | Store::UpdateCopy => Class::Rewrite,
            Store::Library => Class::Unsupported,
        }
    }

    /// A store whose contents must be inventoried: every class except `Fixed`.
    pub const fn enumerated(self) -> bool {
        !matches!(self.class(), Class::Fixed)
    }
}

/// A set of stores, as a bit mask.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StoreSet(u32);

impl StoreSet {
    pub const fn empty() -> Self {
        Self(0)
    }

    pub const fn with(self, store: Store) -> Self {
        Self(self.0 | 1 << store as u8)
    }

    pub const fn contains(self, store: Store) -> bool {
        self.0 & (1 << store as u8) != 0
    }
}

/// An opaque number the adapter gave one artifact, unique across the inventory. Never a path.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ArtifactId(pub u32);

/// Whose an artifact has been proven to be, by the adapter that found it.
///
/// For a `Rewrite` store, `Target` means the container holds entries proven to be the target's, and
/// `Shared` means an entry is shared with another history.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Owner {
    Target,
    /// The file a Save As leaves behind; kept on disk and never acted on.
    SaveAsSource,
    Unrelated,
    Shared,
    /// Ownership could not be proven; the plan refuses as incomplete.
    Unknown,
}

/// Whether the adapter can rewrite the artifact's format.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    Current,
    Foreign,
}

/// One retained artifact. Eight bytes, so the capped inventory stays small.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Artifact {
    pub id: ArtifactId,
    pub store: Store,
    pub owner: Owner,
    pub format: Format,
}

/// What the adapters found, and which enumerated stores they covered completely.
#[derive(Clone, Copy, Debug)]
pub struct Inventory<'a> {
    complete: StoreSet,
    artifacts: &'a [Artifact],
}

impl<'a> Inventory<'a> {
    pub(crate) const fn new(complete: StoreSet, artifacts: &'a [Artifact]) -> Self {
        Self { complete, artifacts }
    }

    pub const fn complete(&self) -> StoreSet {
        self.complete
    }

    pub const fn artifacts(&self) -> &'a [Artifact] {
        self.artifacts
    }
}

/// Which kinds of content were asked to be removed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Kinds(u8);

impl Kinds {
    pub const METADATA: u8 = 1;
    pub const ATTACHMENTS: u8 = 2;
    pub const ACTIONS: u8 = 4;

    pub const fn from_bits(bits: u8) -> Result<Kinds, Refusal> {
        if bits == 0 {
            Err(Refusal::NothingSelected)
        } else if bits & !(Self::METADATA | Self::ATTACHMENTS | Self::ACTIONS) != 0 {
            Err(Refusal::UnknownKind)
        } else {
            Ok(Kinds(bits))
        }
    }

    pub const fn contains(self, bit: u8) -> bool {
        self.0 & bit != 0
    }
}

/// How the sanitized file is installed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TargetMode {
    /// Replace the source file in place.
    Save,
    /// Write a new file and leave the source as it is.
    SaveAs,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Request {
    kinds: Kinds,
    mode: TargetMode,
    signed_source: bool,
}

impl Request {
    pub const fn new(kinds: Kinds, mode: TargetMode, signed_source: bool) -> Self {
        Self { kinds, mode, signed_source }
    }

    pub const fn kinds(&self) -> Kinds {
        self.kinds
    }

    pub const fn mode(&self) -> TargetMode {
        self.mode
    }

    pub const fn signed_source(&self) -> bool {
        self.signed_source
    }
}

/// Caps on what outside input may make the plan hold or do at once.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Limits {
    pub max_artifacts: u32,
    pub max_batch: NonZeroU16,
}

/// The design limits of `docs/design/removal.md` §8. Not measurements.
pub const DESIGN_LIMITS: Limits = Limits {
    max_artifacts: 8_192,
    max_batch: match NonZeroU16::new(128) {
        Some(n) => n,
        None => unreachable!(),
    },
};

/// Why no plan was made. Never carries ids or other input.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refusal {
    NothingSelected,
    UnknownKind,
    /// A signed source cannot be replaced in place; Save As is the way.
    SignedOriginal,
    TooLarge,
    /// A store was not covered completely, or an artifact's owner is unknown.
    Incomplete(Store),
    /// An artifact is shared with another history, and no scope decision exists.
    Shared(Store),
    /// The store or the artifact's format cannot take part yet.
    Unsupported(Store),
    /// The inventory contradicts itself.
    Inconsistent,
}

/// A subsystem that can recreate retained material and must be stopped before it is cleared.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Writer {
    Journals,
    SleepFiles,
    IndexJobs,
    HeadingJobs,
    AgentExports,
    Renders,
    /// The snapshot tool's and text copy's offers (`recto-app/src/core/snapshot_tool.rs`). A snapshot of the old version still being made must
    /// not publish after the clear. What happens to an offer already held is undecided (an owner question), and its adapter waits.
    ClipboardOffers,
}

impl Writer {
    const ALL: [Writer; 7] = [
        Writer::Journals,
        Writer::SleepFiles,
        Writer::IndexJobs,
        Writer::HeadingJobs,
        Writer::AgentExports,
        Writer::Renders,
        Writer::ClipboardOffers,
    ];
}

/// The transaction's phases (`docs/design/removal.md` §6). Never stored: an effect names its phase.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Phase {
    Confirmed,
    Quiescing,
    CandidateReady,
    BarrierDurable,
    Purging,
    Purged,
    Replacing,
    Replaced,
    Complete,
}

/// One thing the carrier does, then acknowledges.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Effect {
    TakeLeases,
    Quiesce(Writer),
    WriteCandidate,
    VerifyCandidate,
    PersistBarrier,
    Rewrite(Store),
    ClearFindText,
    Purge(Store),
    VerifyRetired,
    MarkNoReturn,
    InstallTarget,
    ReadBackTarget,
    CleanSession,
    VerifyFinal,
    MarkComplete,
    PublishInvalidation,
    ReleaseLeases,
    RetireBarrier,
}

impl Effect {
    pub const fn phase(self) -> Phase {
        match self {
            Effect::TakeLeases => Phase::Confirmed,
            Effect::Quiesce(_) => Phase::Quiescing,
            Effect::WriteCandidate | Effect::VerifyCandidate => Phase::CandidateReady,
            Effect::PersistBarrier => Phase::BarrierDurable,
            Effect::Rewrite(_) | Effect::ClearFindText | Effect::Purge(_) => Phase::Purging,
            Effect::VerifyRetired | Effect::MarkNoReturn => Phase::Purged,
            Effect::InstallTarget => Phase::Replacing,
            Effect::ReadBackTarget | Effect::CleanSession => Phase::Replaced,
            Effect::VerifyFinal | Effect::MarkComplete | Effect::PublishInvalidation | Effect::ReleaseLeases | Effect::RetireBarrier => Phase::Complete,
        }
    }

    /// A durable effect that finished but was not synced has not happened: it counts as a failure.
    pub const fn needs_durable(self) -> bool {
        matches!(
            self,
            Effect::WriteCandidate
                | Effect::PersistBarrier
                | Effect::Rewrite(_)
                | Effect::ClearFindText
                | Effect::Purge(_)
                | Effect::MarkNoReturn
                | Effect::InstallTarget
                | Effect::CleanSession
                | Effect::MarkComplete
                | Effect::RetireBarrier
        )
    }
}

/// An effect and the artifacts it applies to, as a range in the plan's id list.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Step {
    effect: Effect,
    start: u32,
    len: u16,
}

impl Step {
    pub const fn effect(&self) -> Effect {
        self.effect
    }

    pub const fn start(&self) -> u32 {
        self.start
    }

    pub const fn len(&self) -> u16 {
        self.len
    }

    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }
}

/// What the report must say a store is not removed from, because removal does not reach it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Exclusion {
    NoDocumentText,
    NotCleared,
    OutsideApp,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Disposition {
    Purged,
    Rewritten,
    Cleared,
    Quiesced,
    Invalidated,
    Replaced,
    KeptAsSource,
    NoneFound,
    Excluded(Exclusion),
}

/// One store's row in the report.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StoreLine {
    pub store: Store,
    pub disposition: Disposition,
    /// Target artifacts acted on.
    pub acted: u32,
    /// Save As source artifacts left in place.
    pub kept: u32,
    /// Artifacts of other documents, left alone.
    pub untouched: u32,
}

/// The whole transaction, decided. Immutable; `Progress` walks it.
#[derive(Clone, Debug)]
pub struct Plan {
    request: Request,
    steps: Vec<Step>,
    ids: Vec<ArtifactId>,
    lines: [StoreLine; Store::COUNT],
    unsigned_copy_notice: bool,
    digest: PlanDigest,
}

impl Plan {
    pub fn steps(&self) -> &[Step] {
        &self.steps
    }

    /// The artifacts a step applies to; empty for a step not made by this plan.
    pub fn artifacts(&self, step: &Step) -> &[ArtifactId] {
        let start = step.start as usize;
        self.ids.get(start..start + usize::from(step.len)).unwrap_or(&[])
    }

    pub fn lines(&self) -> &[StoreLine; Store::COUNT] {
        &self.lines
    }

    pub fn line(&self, store: Store) -> StoreLine {
        self.lines[store as usize]
    }

    pub fn request(&self) -> Request {
        self.request
    }

    /// A fingerprint of the whole plan, computed once when it was built.
    pub fn digest(&self) -> PlanDigest {
        self.digest
    }

    /// Save As of a signed source: the copy is unsigned and the report must say so.
    pub fn unsigned_copy_notice(&self) -> bool {
        self.unsigned_copy_notice
    }
}

#[cfg(test)]
mod tests;
