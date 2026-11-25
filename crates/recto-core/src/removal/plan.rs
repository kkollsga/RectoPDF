//! Building a `Plan`: validation first, then the fixed step order, then the per-store report.

use super::*;

/// Where each retention store is acted on, restore entry points first (an update copy, a legacy
/// session and the session record slots stop old history coming back), then journals, then what
/// the journals refer to, then backups; corpus aliases go before the corpus text they point at.
const RETENTION_ORDER: [Store; 15] = [
    Store::UpdateCopy,
    Store::LegacySession,
    Store::SessionRecord,
    Store::FindText,
    Store::Journal,
    Store::JournalVariant,
    Store::PageIds,
    Store::KeptPages,
    Store::SleepFile,
    Store::Backup,
    Store::CorpusAliases,
    Store::CorpusText,
    Store::CorpusAgentPages,
    Store::GeneratedContents,
    Store::WriterTemp,
];

/// A store missing from the step order is never acted on, so it fails the build rather than a test
/// someone has to remember to write: every `Purge` and `Rewrite` store is in the order, `FindText`
/// is, and nothing else is.
const _: () = {
    let mut s = 0;
    while s < Store::COUNT {
        let store = Store::ALL[s];
        let mut listed = false;
        let mut i = 0;
        while i < RETENTION_ORDER.len() {
            listed |= RETENTION_ORDER[i] as u8 == store as u8;
            i += 1;
        }
        let acted_on = matches!(store.class(), Class::Purge | Class::Rewrite) || store as u8 == Store::FindText as u8;
        assert!(listed == acted_on, "RETENTION_ORDER and the stores acted on disagree");
        s += 1;
    }
};

/// Decide the transaction for one request, or refuse. Pure and bounded: the artifact count is
/// checked before anything is allocated, and every refusal is decided before any step exists.
pub fn plan(request: &Request, inventory: &Inventory<'_>, limits: &Limits) -> Result<Plan, Refusal> {
    let artifacts = inventory.artifacts();
    if artifacts.len() > limits.max_artifacts as usize {
        return Err(Refusal::TooLarge);
    }
    if request.mode() == TargetMode::Save && request.signed_source() {
        return Err(Refusal::SignedOriginal);
    }
    check_coverage(inventory.complete())?;
    check_artifacts(request, artifacts)?;
    check_unique(artifacts)?;
    Ok(build(request, artifacts, limits))
}

/// An empty store is not "nothing to do": it must be declared searched, or the plan cannot say so.
fn check_coverage(complete: StoreSet) -> Result<(), Refusal> {
    match Store::ALL.into_iter().find(|s| s.enumerated() && !complete.contains(*s)) {
        Some(store) => Err(Refusal::Incomplete(store)),
        None => Ok(()),
    }
}

fn check_artifacts(request: &Request, artifacts: &[Artifact]) -> Result<(), Refusal> {
    for a in artifacts {
        let class = a.store.class();
        if class == Class::Fixed {
            return Err(Refusal::Inconsistent);
        }
        match a.owner {
            Owner::Unknown => return Err(Refusal::Incomplete(a.store)),
            Owner::Shared => return Err(Refusal::Shared(a.store)),
            Owner::SaveAsSource if request.mode() == TargetMode::Save => {
                return Err(Refusal::Inconsistent);
            }
            Owner::Target if class == Class::Unsupported => return Err(Refusal::Unsupported(a.store)),
            // A container this build cannot rewrite is refused; a whole file proven the target's is
            // deleted whatever its format.
            Owner::Target if class == Class::Rewrite && a.format == Format::Foreign => {
                return Err(Refusal::Unsupported(a.store));
            }
            _ => {}
        }
    }
    Ok(())
}

/// Two artifacts with one id would be acted on twice or hide each other. The cap bounds the copy.
fn check_unique(artifacts: &[Artifact]) -> Result<(), Refusal> {
    let mut ids: Vec<u32> = artifacts.iter().map(|a| a.id.0).collect();
    ids.sort_unstable();
    if ids.windows(2).any(|w| w[0] == w[1]) {
        Err(Refusal::Inconsistent)
    } else {
        Ok(())
    }
}

struct Builder {
    steps: Vec<Step>,
    ids: Vec<ArtifactId>,
}

impl Builder {
    fn push(&mut self, effect: Effect) {
        self.steps.push(Step { effect, start: 0, len: 0 });
    }

    /// The store's target artifacts, in input order, in steps of at most `batch`.
    fn push_store(&mut self, store: Store, artifacts: &[Artifact], batch: usize) {
        let effect = if store.class() == Class::Rewrite {
            Effect::Rewrite(store)
        } else {
            Effect::Purge(store)
        };
        let mut targets = artifacts.iter().filter(|a| a.store == store && a.owner == Owner::Target);
        loop {
            let start = self.ids.len();
            self.ids.extend(targets.by_ref().take(batch).map(|a| a.id));
            let len = self.ids.len() - start;
            if len == 0 {
                return;
            }
            // `len <= batch <= u16::MAX`, and the start is below the capped artifact count.
            self.steps.push(Step {
                effect,
                start: start as u32,
                len: len as u16,
            });
        }
    }
}

fn build(request: &Request, artifacts: &[Artifact], limits: &Limits) -> Plan {
    let batch = usize::from(limits.max_batch.get());
    let mut b = Builder {
        steps: Vec::new(),
        ids: Vec::with_capacity(artifacts.len()),
    };
    b.push(Effect::TakeLeases);
    for writer in Writer::ALL {
        b.push(Effect::Quiesce(writer));
    }
    b.push(Effect::WriteCandidate);
    b.push(Effect::VerifyCandidate);
    b.push(Effect::PersistBarrier);
    for store in RETENTION_ORDER {
        if store == Store::FindText {
            b.push(Effect::ClearFindText);
        } else {
            b.push_store(store, artifacts, batch);
        }
    }
    for effect in [
        Effect::VerifyRetired,
        Effect::MarkNoReturn,
        Effect::InstallTarget,
        Effect::ReadBackTarget,
        Effect::CleanSession,
        Effect::VerifyFinal,
        Effect::MarkComplete,
        Effect::PublishInvalidation,
        Effect::ReleaseLeases,
        Effect::RetireBarrier,
    ] {
        b.push(effect);
    }
    let signed_copy = request.signed_source() && request.mode() == TargetMode::SaveAs;
    let lines = lines(request, artifacts);
    let digest = digest::of(request, &b.steps, &b.ids, &lines);
    Plan {
        request: *request,
        steps: b.steps,
        ids: b.ids,
        lines,
        unsigned_copy_notice: signed_copy,
        digest,
    }
}

fn lines(request: &Request, artifacts: &[Artifact]) -> [StoreLine; Store::COUNT] {
    // Per store: target, save-as source, unrelated.
    let mut counts = [[0u32; 3]; Store::COUNT];
    for a in artifacts {
        let slot = match a.owner {
            Owner::Target => 0,
            Owner::SaveAsSource => 1,
            Owner::Unrelated => 2,
            // Refused before a plan is built.
            Owner::Shared | Owner::Unknown => continue,
        };
        counts[a.store as usize][slot] += 1;
    }
    Store::ALL.map(|store| {
        let [acted, kept, untouched] = counts[store as usize];
        StoreLine {
            store,
            disposition: disposition(request, store, acted, kept),
            acted,
            kept,
            untouched,
        }
    })
}

fn disposition(request: &Request, store: Store, acted: u32, kept: u32) -> Disposition {
    match store {
        Store::SourcePdf if request.mode() == TargetMode::Save => Disposition::Replaced,
        Store::SourcePdf => Disposition::KeptAsSource,
        Store::FindText => Disposition::Cleared,
        Store::LiveViews => Disposition::Invalidated,
        Store::HelperJobs => Disposition::Quiesced,
        Store::Logs => Disposition::Excluded(Exclusion::NoDocumentText),
        Store::Clipboard => Disposition::Excluded(Exclusion::NotCleared),
        Store::External => Disposition::Excluded(Exclusion::OutsideApp),
        // A library with target material refused earlier, so none is ever found here.
        Store::Library => Disposition::NoneFound,
        // Only a Save As source is here: it stays on disk, which is not the same as nothing found.
        _ if acted == 0 && kept > 0 => Disposition::KeptAsSource,
        _ if acted == 0 => Disposition::NoneFound,
        _ if store.class() == Class::Rewrite => Disposition::Rewritten,
        _ => Disposition::Purged,
    }
}
