//! A fingerprint of a whole plan, so a confirmation and a `Progress` can be bound to exactly the plan a person was shown.

use super::*;
use sha2::{Digest, Sha256};

/// `SHA-256` over the request (scope included), every step, every artifact id, the 22 report lines and then every artifact's store, owner, format and
/// key in id order, in a fixed little-endian encoding. Equal plans have equal digests; any change to what is done, to which artifacts, to which
/// files they are, or to what is reported changes it. Version 2 (2026-11-26): version 1 hashed ids and counts only, so a file replaced by another
/// of the same kind kept the digest of the plan a person had confirmed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PlanDigest([u8; 32]);

impl PlanDigest {
    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    pub fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }
}

/// Hashes, in order: the domain tag, the request, the steps, the ids, the lines, the artifacts. Codes come from exhaustive matches below, so a new
/// variant fails to compile here instead of being hashed as something else.
pub(super) fn of(request: &Request, steps: &[Step], ids: &[ArtifactId], lines: &[StoreLine; Store::COUNT], inventory: &Inventory<'_>) -> PlanDigest {
    let mut h = Sha256::new();
    h.update(b"recto-removal-plan/2\0");
    h.update([
        request.kinds().0,
        mode_code(request.mode()),
        u8::from(request.signed_source()),
        scope_code(request.scope()),
    ]);
    h.update((steps.len() as u32).to_le_bytes());
    for step in steps {
        h.update(effect_code(step.effect));
        h.update(step.start.to_le_bytes());
        h.update(step.len.to_le_bytes());
    }
    h.update((ids.len() as u32).to_le_bytes());
    for id in ids {
        h.update(id.0.to_le_bytes());
    }
    for line in lines {
        h.update([store_code(line.store)]);
        h.update(disposition_code(line.disposition));
        h.update(line.acted.to_le_bytes());
        h.update(line.kept.to_le_bytes());
        h.update(line.untouched.to_le_bytes());
    }
    h.update((inventory.artifacts().len() as u32).to_le_bytes());
    for (at, a) in inventory.artifacts().iter().enumerate() {
        h.update(a.id.0.to_le_bytes());
        h.update([store_code(a.store), owner_code(a.owner), format_code(a.format)]);
        // A missing key (core's own pure tests only) is marked, so it can't read as a key.
        match inventory.key_at(at) {
            Some(key) => {
                h.update([1]);
                h.update(key.as_bytes());
            }
            None => h.update([0]),
        }
    }
    PlanDigest(h.finalize().into())
}

fn scope_code(scope: Scope) -> u8 {
    match scope {
        Scope::Removal => 0,
        Scope::Leftovers => 1,
    }
}

fn owner_code(owner: Owner) -> u8 {
    match owner {
        Owner::Target => 0,
        Owner::SaveAsSource => 1,
        Owner::Unrelated => 2,
        Owner::Shared => 3,
        Owner::Unknown => 4,
    }
}

fn format_code(format: Format) -> u8 {
    match format {
        Format::Current => 0,
        Format::Foreign => 1,
    }
}

fn mode_code(mode: TargetMode) -> u8 {
    match mode {
        TargetMode::Save => 0,
        TargetMode::SaveAs => 1,
    }
}

/// The store's code, 0..=21 in declaration order, as an explicit table: inserting a variant fails to compile here instead of shifting the
/// codes of the ones after it.
pub(super) fn store_code(store: Store) -> u8 {
    match store {
        Store::SourcePdf => 0,
        Store::WriterTemp => 1,
        Store::Backup => 2,
        Store::Journal => 3,
        Store::JournalVariant => 4,
        Store::KeptPages => 5,
        Store::PageIds => 6,
        Store::SleepFile => 7,
        Store::SessionRecord => 8,
        Store::FindText => 9,
        Store::LegacySession => 10,
        Store::CorpusText => 11,
        Store::CorpusAgentPages => 12,
        Store::CorpusAliases => 13,
        Store::GeneratedContents => 14,
        Store::UpdateCopy => 15,
        Store::LiveViews => 16,
        Store::HelperJobs => 17,
        Store::Logs => 18,
        Store::Clipboard => 19,
        Store::External => 20,
        Store::Library => 21,
    }
}

/// The writer's code, 0..=6 in declaration order, as an explicit table.
pub(super) fn writer_code(writer: Writer) -> u8 {
    match writer {
        Writer::Journals => 0,
        Writer::SleepFiles => 1,
        Writer::IndexJobs => 2,
        Writer::HeadingJobs => 3,
        Writer::AgentExports => 4,
        Writer::Renders => 5,
        Writer::ClipboardOffers => 6,
    }
}

/// `[tag, arg]`: tags 1..=19 in declaration order; the argument is the store or the writer, else 0.
fn effect_code(effect: Effect) -> [u8; 2] {
    match effect {
        Effect::TakeLeases => [1, 0],
        Effect::Quiesce(w) => [2, writer_code(w)],
        Effect::WriteCandidate => [3, 0],
        Effect::VerifyCandidate => [4, 0],
        Effect::PersistBarrier => [5, 0],
        Effect::Rewrite(s) => [6, store_code(s)],
        Effect::ClearFindText => [7, 0],
        Effect::Purge(s) => [8, store_code(s)],
        Effect::VerifyRetired => [9, 0],
        Effect::MarkNoReturn => [10, 0],
        Effect::InstallTarget => [11, 0],
        Effect::ReadBackTarget => [12, 0],
        Effect::CleanSession => [13, 0],
        Effect::VerifyFinal => [14, 0],
        Effect::MarkComplete => [15, 0],
        Effect::PublishInvalidation => [16, 0],
        Effect::ReleaseLeases => [17, 0],
        Effect::RetireBarrier => [18, 0],
        Effect::Trash(s) => [19, store_code(s)],
    }
}

/// `[tag, arg]`: tags 1..=10 in declaration order; `Excluded` carries its reason as 1..=3.
fn disposition_code(disposition: Disposition) -> [u8; 2] {
    match disposition {
        Disposition::Purged => [1, 0],
        Disposition::Rewritten => [2, 0],
        Disposition::Cleared => [3, 0],
        Disposition::Quiesced => [4, 0],
        Disposition::Invalidated => [5, 0],
        Disposition::Replaced => [6, 0],
        Disposition::KeptAsSource => [7, 0],
        Disposition::NoneFound => [8, 0],
        Disposition::Excluded(why) => [9, exclusion_code(why)],
        Disposition::Trashed => [10, 0],
    }
}

fn exclusion_code(why: Exclusion) -> u8 {
    match why {
        Exclusion::NoDocumentText => 1,
        Exclusion::NotCleared => 2,
        Exclusion::OutsideApp => 3,
    }
}
