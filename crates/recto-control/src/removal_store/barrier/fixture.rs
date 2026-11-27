//! A record on a disk, for tests that need one: the plan, the walk and the facts of one removal, and a way to leave a state root as a restart after a crash
//! finds it. Behind `test-seams` so the app's headless tests (which see the gate through the record store, the updater's check and the QA verb) can seed
//! one without a second copy of how it is made. Never in the app.

use super::*;
use recto_core::removal::{
    Actor, ArtifactKey, Binding, CONFIRM_LIMITS, Class, Confirmations, Format, Found, Kinds, LEDGER_LIMITS, Owner, Pin, PinKind, PlanId, Request, Store,
    Survey, TargetKey, TargetMode, Version, encode_header, lineage, plan, read_ledger,
};
use recto_core::save::Target;
use std::num::NonZeroU16;
use std::time::Instant;

fn version(inode: u64) -> recto_core::removal::FileVersion {
    recto_core::removal::FileVersion {
        volume: Some([3; 16]),
        inode,
        size: 10,
        mtime_ns: 1,
    }
}

fn limits(max: u32) -> Limits {
    Limits {
        max_artifacts: max,
        max_batch: NonZeroU16::new(4).unwrap(),
    }
}

fn found(store: Store, name: &str, owner: Owner) -> Found {
    Found {
        store,
        key: ArtifactKey::of(store, name.as_bytes()),
        owner,
        format: Format::Current,
    }
}

pub struct Rig {
    pub surveyed: Surveyed,
    pub limits: Limits,
    pub plan: Plan,
    pub facts: BarrierFacts,
}

/// A removal of a journal, a writer's temporary and a session record, with another document's journal beside them.
pub fn rig() -> Rig {
    rig_named("")
}

/// The same shape of removal over other files: another plan.
pub fn rig_named(suffix: &str) -> Rig {
    rig_of(suffix, limits(8), 0)
}

/// A removal with `journals` more of a document's journals to purge, at the survey's own cap (8,192 artifacts, in steps of 128): the largest record there is,
/// for a measurement.
pub fn rig_at_the_cap(journals: u32) -> Rig {
    rig_of(
        "",
        Limits {
            max_artifacts: 8_192,
            max_batch: NonZeroU16::new(128).unwrap(),
        },
        journals,
    )
}

fn rig_of(suffix: &str, limits: Limits, more_journals: u32) -> Rig {
    let mut survey = Survey::new(&limits);
    for store in Store::ALL.into_iter().filter(|s| s.enumerated()) {
        survey.mark_complete(store);
    }
    let ledger = read_ledger(&encode_header(1_000), &LEDGER_LIMITS);
    survey.history(&lineage(&ledger, version(5), Path::new("/d/a.pdf"), &LEDGER_LIMITS).unwrap());
    for f in [
        found(Store::Journal, &format!("j1{suffix}"), Owner::Target),
        found(Store::WriterTemp, &format!("t1{suffix}"), Owner::Target),
        found(Store::SessionRecord, &format!("s1{suffix}"), Owner::Target),
        found(Store::Journal, "other", Owner::Unrelated),
    ] {
        survey.add(f).unwrap();
    }
    for n in 0..more_journals {
        survey.add(found(Store::Journal, &format!("more-{n}"), Owner::Target)).unwrap();
    }
    let surveyed = survey.finish().unwrap();
    let request = Request::new(Kinds::from_bits(Kinds::METADATA).unwrap(), TargetMode::Save, false);
    let plan = plan(&request, &surveyed.inventory(), &limits).unwrap();
    let pins = surveyed
        .inventory()
        .artifacts()
        .iter()
        .map(|a| {
            (a.owner == Owner::Target && matches!(a.store.class(), Class::Purge | Class::Rewrite | Class::Trash)).then_some(Pin {
                root: 0,
                inode: 100 + u64::from(a.id.0),
                kind: PinKind::File,
            })
        })
        .collect();
    let facts = BarrierFacts {
        target: "/d/a.pdf".into(),
        source: version(5),
        candidate_name: ".a.pdf.new-1".into(),
        candidate: version(6),
        candidate_sha: [9; 32],
        roots: vec![[1; 16]],
        actor: Actor::Person,
        pins,
    };
    Rig { surveyed, limits, plan, facts }
}

impl Rig {
    /// A new walk of the plan, confirmed and acknowledged up to the barrier.
    pub fn progress(&self) -> Progress {
        let mut confirmations = Confirmations::new(CONFIRM_LIMITS);
        let binding = Binding {
            actor: 0u8,
            version: Version {
                epoch: 1,
                source: recto_platform::FileStamp {
                    size: 1,
                    mtime_ns: 2,
                    device: 3,
                    inode: 4,
                },
            },
            target: TargetKey::new(Path::new("/doc.pdf"), Target::Same),
        };
        let now = Instant::now();
        let id = confirmations.offer(binding, &self.plan, now, PlanId::from_raw(9).unwrap()).unwrap();
        let confirmed = confirmations.take(id, &binding, &self.plan, now).unwrap();
        let mut progress = Progress::start(&self.plan, confirmed).unwrap();
        for i in 0..self.base() {
            progress.acknowledge(&self.plan, i, Ack::Done).unwrap();
        }
        progress
    }

    pub fn base(&self) -> u32 {
        self.plan.steps().iter().position(|s| s.effect() == Effect::PersistBarrier).unwrap() as u32
    }

    pub fn last(&self) -> u32 {
        self.plan.steps().len() as u32 - 1
    }

    pub fn begin(&self, root: &Path, progress: &mut Progress, opts: &Opts<'_>) -> Result<Held, BeginError> {
        begin_with(root, &self.plan, &self.surveyed, &self.limits, progress, &self.facts, opts)
    }

    /// A begin that leaves the process-wide token alone, so a test may hold two walks at once (two processes, as far as the record's file can tell: its
    /// `RENAME_EXCL` is what stops the second).
    pub fn begin_ok(&self, root: &Path) -> (Held, Progress) {
        let mut progress = self.progress();
        let opts = Opts {
            alone: false,
            ..Opts::standard()
        };
        let held = self.begin(root, &mut progress, &opts).unwrap();
        (held, progress)
    }

    /// The begin the app makes: it takes the token, so no other `begin`, `reopen` or `abandon` runs while the `Held` lives.
    pub fn begin_held(&self, root: &Path) -> (Held, Progress) {
        let mut progress = self.progress();
        let held = self.begin(root, &mut progress, &Opts::standard()).unwrap();
        (held, progress)
    }

    pub fn len(&self) -> usize {
        let progress = self.progress();
        encode_barrier_within(&self.plan, &self.surveyed, &self.limits, &progress, &self.facts, MAX_BARRIER_BYTES)
            .unwrap()
            .bytes
            .len()
    }
}

/// A state root left as a crash finds it: a removal that acknowledged its barrier and `acked_steps_after` steps after it, and then was killed (the `Held`
/// is dropped, nothing cleaned up). Returns the rig, whose plan is what the record is of.
pub fn pending_at(root: &Path, acked_steps_after: u32) -> Rig {
    let r = rig();
    let (held, mut progress) = r.begin_held(root);
    for i in r.base() + 1..=(r.base() + acked_steps_after).min(r.last() - 1) {
        held.acknowledge(&r.plan, &mut progress, i, Ack::Done).unwrap();
    }
    r
}

/// What a state root's `removal/` can hold, for a test of every answer the gate gives (and of every reader of it agreeing).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// No `removal/` at all.
    NoFolder,
    /// `removal/` holding only the ledger's file, which is not a record.
    LedgerOnly,
    /// A real record, with this many steps acknowledged after the barrier.
    Pending(u32),
    /// Bytes that don't decode, in a private plain file.
    Garbage,
    /// A real record that others can read.
    Mode0644,
    /// A real record's name that is a link to it.
    Link,
    /// A pipe at the name.
    Fifo,
    /// A file of the cap's bytes and one.
    OverCap,
    /// A real record that can't be opened for writing, as on a read-only volume.
    ReadOnly,
    /// A real record of a later version.
    TooNew,
    /// `removal` is a link to a folder.
    FolderIsLink,
    /// `removal` is a file.
    FolderIsFile,
}

impl Kind {
    /// What every reader should say of it, in the words of `gate::state`.
    pub const fn answer(self) -> &'static str {
        match self {
            Self::NoFolder | Self::LedgerOnly => "nothing",
            Self::Pending(_) => "pending",
            Self::Garbage | Self::Mode0644 | Self::Link | Self::Fifo | Self::OverCap | Self::ReadOnly => "damaged",
            Self::TooNew => "too_new",
            Self::FolderIsLink | Self::FolderIsFile => "not_ours",
        }
    }

    /// Every kind, with three later counts of a pending record.
    pub const ALL: [Self; 15] = [
        Self::NoFolder,
        Self::LedgerOnly,
        Self::Pending(0),
        Self::Pending(1),
        Self::Pending(2),
        Self::Pending(3),
        Self::Garbage,
        Self::Mode0644,
        Self::Link,
        Self::Fifo,
        Self::OverCap,
        Self::ReadOnly,
        Self::TooNew,
        Self::FolderIsLink,
        Self::FolderIsFile,
    ];
}

/// Leaves `<root>/removal` as `kind` says. `root` is an empty folder of the test's.
pub fn seed(root: &Path, kind: Kind) {
    use std::os::unix::fs::PermissionsExt as _;
    let dir = root.join("removal");
    let file = dir.join("pending.v1");
    let make_record = || {
        pending_at(root, 0);
    };
    match kind {
        Kind::NoFolder => {}
        Kind::LedgerOnly => {
            std::fs::create_dir(&dir).unwrap();
            std::fs::write(dir.join("ledger.v1"), b"x").unwrap();
        }
        Kind::Pending(later) => {
            pending_at(root, later);
        }
        Kind::Garbage => {
            std::fs::create_dir(&dir).unwrap();
            std::fs::write(&file, b"not a record").unwrap();
            std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o600)).unwrap();
        }
        Kind::Mode0644 => {
            make_record();
            std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o644)).unwrap();
        }
        Kind::Link => {
            make_record();
            let aside = dir.join("aside");
            std::fs::rename(&file, &aside).unwrap();
            std::os::unix::fs::symlink(&aside, &file).unwrap();
        }
        Kind::Fifo => {
            std::fs::create_dir(&dir).unwrap();
            assert!(std::process::Command::new("mkfifo").arg(&file).status().unwrap().success());
        }
        Kind::OverCap => {
            std::fs::create_dir(&dir).unwrap();
            std::fs::write(&file, vec![0u8; MAX_BARRIER_BYTES + 1]).unwrap();
            std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o600)).unwrap();
        }
        Kind::ReadOnly => {
            make_record();
            std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o400)).unwrap();
        }
        Kind::TooNew => {
            make_record();
            let mut bytes = std::fs::read(&file).unwrap();
            bytes[16] = 2;
            std::fs::write(&file, &bytes).unwrap();
        }
        Kind::FolderIsLink => {
            let real = root.join("elsewhere");
            std::fs::create_dir(&real).unwrap();
            std::os::unix::fs::symlink(&real, &dir).unwrap();
        }
        Kind::FolderIsFile => std::fs::write(&dir, b"x").unwrap(),
    }
}

/// What is under `root`, by path: what it is, its inode, its size and its time. A change to any of them is a change to the folder (for "nothing was done").
pub fn tree(root: &Path) -> std::collections::BTreeMap<std::path::PathBuf, String> {
    fn walk(folder: &Path, root: &Path, found: &mut std::collections::BTreeMap<std::path::PathBuf, String>) {
        for entry in std::fs::read_dir(folder).into_iter().flatten().flatten() {
            let path = entry.path();
            let Ok(meta) = std::fs::symlink_metadata(&path) else { continue };
            found.insert(
                path.strip_prefix(root).unwrap().to_owned(),
                format!(
                    "{:?} ino {} len {} mtime {}.{}",
                    meta.file_type(),
                    meta.ino(),
                    meta.len(),
                    meta.mtime(),
                    meta.mtime_nsec()
                ),
            );
            if meta.is_dir() {
                walk(&path, root, found);
            }
        }
    }
    use std::os::unix::fs::MetadataExt as _;
    let mut found = std::collections::BTreeMap::new();
    walk(root, root, &mut found);
    found
}
