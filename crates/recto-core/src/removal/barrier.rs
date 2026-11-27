//! The transaction record's format (`docs/design/removal.md` §6 and §10, D3a): a pure encoder and a bounded decoder. Nothing here opens a file.
//!
//! One file, `pending.v1`: a header and a body that never change once written, then a fixed block of slots written in place as later steps are
//! acknowledged. All integers are little-endian.
//! - Header, 32 bytes: `RECTOBARRIER\0\0\0\0`, version u32 = 1, `body_len` u32 (offset 0 through the body's SHA-256), `slots_at` u32 (`body_len` rounded up
//!   to 4,096) and `slots` u32.
//! - Body: scope, kinds, mode and signed (a byte each), the plan's limits (`max_artifacts` u32, `max_batch` u16), the plan's digest, `base` u32 (the
//!   `PersistBarrier` step's index), the actor, the complete store bits u32, the history byte (1), the target path, the source's version, the
//!   candidate's name, version and SHA-256, the roots (a count and 16-byte volume UUIDs), the artifacts (a count and 49 bytes each: id, store, owner,
//!   format, key, and a pin of root, inode and kind), then the SHA-256 of everything before it.
//! - Zero padding up to `slots_at`, then `slots` = steps after the barrier but the last. A slot is 32 bytes: `acked` u32, `position` u32, the first 16
//!   bytes of the plan's digest, 4 zero bytes and the CRC32 of those 28. An all-zero slot is unused; a torn slot (its CRC fails) is ignored. The file's
//!   length is exactly `slots_at + slots * 32`.
//!
//! The record holds paths, numbers, keys and saved identities and nothing of a document: no bytes, text, title, password or key to a file.
//!
//! It is the only way into `Progress::resume`: a record is encoded only from a live `Progress` standing at the barrier, and decoding rebuilds the plan
//! from what it holds and needs the digest it recorded. The file is hostile input: `decode_barrier` never panics, checks every count against the bytes
//! that are there before it allocates, and says `Damaged` for anything it cannot take whole.

use super::digest::store_code;
use super::ledger::{Body, put_path, put_version};
use super::progress::index_of;
use super::*;
use sha2::{Digest, Sha256};
use std::path::PathBuf;

/// The most a record may be. One at every design cap is under it (a test shows it).
pub const MAX_BARRIER_BYTES: usize = 2 << 20;
const MAGIC: &[u8; 16] = b"RECTOBARRIER\0\0\0\0";
const VERSION: u32 = 1;
const FIXED: usize = 32;
const PAGE: usize = 4_096;
const SLOT_LEN: usize = 32;
const ARTIFACT_LEN: usize = 49;
const MAX_ROOTS: usize = 8;
const MAX_NAME: usize = 255;
const MAX_ARTIFACTS: u32 = 8_192;
const MAX_BATCH: u16 = 128;
const NO_ROOT: u8 = 0xFF;

/// Where a transaction record says a walk stood: the plan's digest and the acknowledged count. Only decoding a record makes one (its fields are this
/// module's alone; tests of `Progress::resume` that have no record to decode make one with `new`, which a real build does not have).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Checkpoint {
    digest: PlanDigest,
    acked: u32,
}

impl Checkpoint {
    #[cfg(test)]
    pub(super) const fn new(digest: PlanDigest, acked: u32) -> Self {
        Self { digest, acked }
    }

    pub(super) const fn digest(&self) -> PlanDigest {
        self.digest
    }

    pub const fn acked(&self) -> u32 {
        self.acked
    }
}

/// Who asked for the removal.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Actor {
    Person,
    Agent,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PinKind {
    File,
    Folder,
}

/// What an artifact was when the plan was made, for a later look to find it again by: the inode on the volume of root number `root`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Pin {
    pub root: u8,
    pub inode: u64,
    pub kind: PinKind,
}

/// What the carrier adds to the plan to make the record. Paths and identities only.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BarrierFacts {
    pub target: PathBuf,
    pub source: FileVersion,
    pub candidate_name: PathBuf,
    pub candidate: FileVersion,
    pub candidate_sha: [u8; 32],
    /// Volume UUIDs the pins refer to by position.
    pub roots: Vec<[u8; 16]>,
    pub actor: Actor,
    /// One per artifact in the plan's order: `Some` for exactly the target's artifacts that are purged, rewritten or trashed.
    pub pins: Vec<Option<Pin>>,
}

/// Why a record was not made. Never carries input.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BarrierRefusal {
    /// A leftovers walk has no barrier a record could hold.
    Leftovers,
    NotAtBarrier,
    /// The progress, the survey or the plan disagree: the plan rebuilt from the survey has another digest.
    WrongPlan,
    Plan(Refusal),
    BadLimits,
    BadPath,
    BadName,
    BadRoots,
    BadPins,
    TooLarge,
}

/// Where the slots are in the file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Layout {
    base: u32,
    slots_at: u32,
    slots: u32,
    digest: [u8; 16],
}

impl Layout {
    /// The slot that acknowledges the step before `acked` (`base + 1 + position`): its file offset and its 32 bytes. `None` outside the block.
    pub fn slot(&self, acked: u32) -> Option<(u64, [u8; 32])> {
        let position = acked.checked_sub(self.base.checked_add(1)?)?;
        if position >= self.slots {
            return None;
        }
        let mut out = [0u8; SLOT_LEN];
        out[0..4].copy_from_slice(&acked.to_le_bytes());
        out[4..8].copy_from_slice(&position.to_le_bytes());
        out[8..24].copy_from_slice(&self.digest);
        let crc = crc32fast::hash(&out[..28]);
        out[28..].copy_from_slice(&crc.to_le_bytes());
        Some((u64::from(self.slots_at) + u64::from(position) * SLOT_LEN as u64, out))
    }

    /// The `PersistBarrier` step's index.
    pub const fn base(&self) -> u32 {
        self.base
    }

    pub const fn slots(&self) -> u32 {
        self.slots
    }

    /// The file's whole length.
    pub const fn len(&self) -> u64 {
        self.slots_at as u64 + self.slots as u64 * SLOT_LEN as u64
    }

    pub const fn is_empty(&self) -> bool {
        false
    }
}

/// A record ready to write: its bytes (the slots all zero) and where the slots are.
#[derive(Debug)]
pub struct Encoded {
    pub bytes: Vec<u8>,
    pub layout: Layout,
}

/// A record that decoded whole, with the plan it rebuilt.
#[derive(Debug)]
pub struct Recorded {
    plan: Plan,
    limits: Limits,
    facts: BarrierFacts,
    layout: Layout,
    acked: u32,
    /// The SHA-256 the record carries over its header and body: the file is the same one when this is.
    body_sha: [u8; 32],
}

impl Recorded {
    pub fn plan(&self) -> &Plan {
        &self.plan
    }

    pub const fn limits(&self) -> Limits {
        self.limits
    }

    pub fn facts(&self) -> &BarrierFacts {
        &self.facts
    }

    pub const fn layout(&self) -> Layout {
        self.layout
    }

    /// The highest acknowledged count a valid slot holds, or the barrier's index when none does.
    pub const fn acked(&self) -> u32 {
        self.acked
    }

    pub const fn body_sha(&self) -> &[u8; 32] {
        &self.body_sha
    }

    pub const fn checkpoint(&self) -> Checkpoint {
        Checkpoint {
            digest: self.plan.digest,
            acked: self.acked,
        }
    }

    /// What may be said, by the acknowledged prefix alone.
    pub fn outcome(&self) -> Outcome {
        Progress::at(&self.checkpoint()).outcome(&self.plan).unwrap_or(Outcome::Unchanged)
    }

    /// The plan and the walk to go on with. `NotResumable` while the barrier's own slot is not written: nothing was purged, and the record is
    /// abandoned or left.
    pub fn resume(&self) -> Result<(Plan, Progress), OrderError> {
        let progress = Progress::resume(&self.plan, &self.checkpoint())?;
        Ok((self.plan.clone(), progress))
    }
}

#[derive(Debug)]
pub enum Decoded {
    Pending(Box<Recorded>),
    Damaged,
    /// A record of a later version: not ours to read, and not to be touched.
    TooNew,
}

fn round_up(n: usize) -> usize {
    n.div_ceil(PAGE) * PAGE
}

fn put_pin(out: &mut Vec<u8>, pin: Option<Pin>) {
    match pin {
        None => {
            out.push(NO_ROOT);
            out.extend_from_slice(&0u64.to_le_bytes());
            out.push(0);
        }
        Some(p) => {
            out.push(p.root);
            out.extend_from_slice(&p.inode.to_le_bytes());
            out.push(match p.kind {
                PinKind::File => 1,
                PinKind::Folder => 2,
            });
        }
    }
}

fn owner_byte(owner: Owner) -> u8 {
    match owner {
        Owner::Target => 0,
        Owner::SaveAsSource => 1,
        Owner::Unrelated => 2,
        Owner::Shared => 3,
        Owner::Unknown => 4,
    }
}

fn owner_of(byte: u8) -> Option<Owner> {
    [Owner::Target, Owner::SaveAsSource, Owner::Unrelated, Owner::Shared, Owner::Unknown]
        .into_iter()
        .find(|o| owner_byte(*o) == byte)
}

fn format_byte(format: Format) -> u8 {
    match format {
        Format::Current => 0,
        Format::Foreign => 1,
    }
}

fn format_of(byte: u8) -> Option<Format> {
    [Format::Current, Format::Foreign].into_iter().find(|f| format_byte(*f) == byte)
}

fn mode_byte(mode: TargetMode) -> u8 {
    match mode {
        TargetMode::Save => 0,
        TargetMode::SaveAs => 1,
    }
}

fn store_of(byte: u8) -> Option<Store> {
    Store::ALL.into_iter().find(|s| store_code(*s) == byte)
}

fn actor_byte(actor: Actor) -> u8 {
    match actor {
        Actor::Person => 0,
        Actor::Agent => 1,
    }
}

/// Every store bit a `StoreSet` can hold.
const KNOWN_STORES: u32 = (1 << Store::COUNT) - 1;

/// A name inside a folder: one component.
fn name_fits(name: &[u8]) -> bool {
    (1..=MAX_NAME).contains(&name.len()) && name != b"." && name != b".." && !name.contains(&b'/') && !name.contains(&0)
}

fn roots_fit(roots: &[[u8; 16]]) -> bool {
    roots.len() <= MAX_ROOTS && roots.iter().all(|r| *r != [0; 16]) && roots.iter().enumerate().all(|(i, r)| !roots[..i].contains(r))
}

/// Pins sit on exactly the target's artifacts that are acted on by file, and name a root that is there.
fn pins_fit(artifacts: &[Artifact], pins: &[Option<Pin>], roots: usize) -> bool {
    artifacts.len() == pins.len()
        && artifacts.iter().zip(pins).all(|(a, pin)| {
            let pinned = a.owner == Owner::Target && matches!(a.store.class(), Class::Purge | Class::Rewrite | Class::Trash);
            match pin {
                Some(p) => pinned && usize::from(p.root) < roots && p.inode != 0,
                None => !pinned,
            }
        })
}

/// A record of the walk standing at the barrier. `plan` is the plan the person confirmed, `surveyed` and `limits` what it was made from.
pub fn encode_barrier(plan: &Plan, surveyed: &Surveyed, limits: &Limits, progress: &Progress, facts: &BarrierFacts) -> Result<Encoded, BarrierRefusal> {
    encode_barrier_within(plan, surveyed, limits, progress, facts, MAX_BARRIER_BYTES)
}

/// `encode_barrier` with a smaller cap, for tests.
pub fn encode_barrier_within(
    plan: &Plan,
    surveyed: &Surveyed,
    limits: &Limits,
    progress: &Progress,
    facts: &BarrierFacts,
    cap: usize,
) -> Result<Encoded, BarrierRefusal> {
    if plan.request().scope() == Scope::Leftovers {
        return Err(BarrierRefusal::Leftovers);
    }
    if progress.digest() != plan.digest || progress.next(plan).map(|s| s.effect()) != Some(Effect::PersistBarrier) {
        return Err(BarrierRefusal::NotAtBarrier);
    }
    let inventory = surveyed.inventory();
    let rebuilt = super::plan(&plan.request(), &inventory, limits).map_err(BarrierRefusal::Plan)?;
    if rebuilt.digest != plan.digest {
        return Err(BarrierRefusal::WrongPlan);
    }
    if !(1..=MAX_ARTIFACTS).contains(&limits.max_artifacts) || limits.max_batch.get() > MAX_BATCH {
        return Err(BarrierRefusal::BadLimits);
    }
    let base = index_of(plan, Effect::PersistBarrier);
    if plan.steps.last().map(|s| s.effect) != Some(Effect::RetireBarrier) || base as usize + 1 >= plan.steps.len() {
        return Err(BarrierRefusal::NotAtBarrier);
    }
    let artifacts = inventory.artifacts();
    if !name_fits(facts.candidate_name.as_os_str().as_encoded_bytes()) {
        return Err(BarrierRefusal::BadName);
    }
    if !roots_fit(&facts.roots) {
        return Err(BarrierRefusal::BadRoots);
    }
    if !pins_fit(artifacts, &facts.pins, facts.roots.len()) {
        return Err(BarrierRefusal::BadPins);
    }
    let slots = (plan.steps.len() - base as usize - 1) as u32;
    let mut body = Vec::with_capacity(256 + artifacts.len() * ARTIFACT_LEN);
    let request = plan.request();
    body.extend_from_slice(&[0, request.kinds().0, mode_byte(request.mode()), u8::from(request.signed_source())]);
    body.extend_from_slice(&limits.max_artifacts.to_le_bytes());
    body.extend_from_slice(&limits.max_batch.get().to_le_bytes());
    body.extend_from_slice(plan.digest.as_bytes());
    body.extend_from_slice(&base.to_le_bytes());
    body.push(actor_byte(facts.actor));
    body.extend_from_slice(&inventory.complete().0.to_le_bytes());
    body.push(1);
    put_path(&mut body, &facts.target).ok_or(BarrierRefusal::BadPath)?;
    put_version(&mut body, facts.source).ok_or(BarrierRefusal::BadPath)?;
    let name = facts.candidate_name.as_os_str().as_encoded_bytes();
    body.push(name.len() as u8);
    body.extend_from_slice(name);
    put_version(&mut body, facts.candidate).ok_or(BarrierRefusal::BadPath)?;
    body.extend_from_slice(&facts.candidate_sha);
    body.push(facts.roots.len() as u8);
    for root in &facts.roots {
        body.extend_from_slice(root);
    }
    body.extend_from_slice(&(artifacts.len() as u32).to_le_bytes());
    for (at, a) in artifacts.iter().enumerate() {
        body.extend_from_slice(&a.id.0.to_le_bytes());
        body.extend_from_slice(&[store_code(a.store), owner_byte(a.owner), format_byte(a.format)]);
        body.extend_from_slice(inventory.key_at(at).ok_or(BarrierRefusal::WrongPlan)?.as_bytes());
        put_pin(&mut body, facts.pins[at]);
    }
    let body_len = FIXED + body.len() + 32;
    let slots_at = round_up(body_len);
    let total = slots_at + slots as usize * SLOT_LEN;
    if total > cap {
        return Err(BarrierRefusal::TooLarge);
    }
    let mut bytes = Vec::with_capacity(total);
    bytes.extend_from_slice(MAGIC);
    bytes.extend_from_slice(&VERSION.to_le_bytes());
    bytes.extend_from_slice(&(body_len as u32).to_le_bytes());
    bytes.extend_from_slice(&(slots_at as u32).to_le_bytes());
    bytes.extend_from_slice(&slots.to_le_bytes());
    bytes.extend_from_slice(&body);
    let sha: [u8; 32] = Sha256::digest(&bytes).into();
    bytes.extend_from_slice(&sha);
    bytes.resize(total, 0);
    let mut digest16 = [0u8; 16];
    digest16.copy_from_slice(&plan.digest.as_bytes()[..16]);
    let layout = Layout {
        base,
        slots_at: slots_at as u32,
        slots,
        digest: digest16,
    };
    Ok(Encoded { bytes, layout })
}

pub fn decode_barrier(bytes: &[u8]) -> Decoded {
    decode_barrier_within(bytes, MAX_BARRIER_BYTES)
}

/// `decode_barrier` with a smaller cap, for tests.
pub fn decode_barrier_within(bytes: &[u8], cap: usize) -> Decoded {
    if bytes.len() > cap || bytes.len() < FIXED || &bytes[..16] != MAGIC {
        return Decoded::Damaged;
    }
    let word = |at: usize| u32::from_le_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]]);
    match word(16) {
        VERSION => {}
        v if v > VERSION => return Decoded::TooNew,
        _ => return Decoded::Damaged,
    }
    let (body_len, slots_at, slots) = (word(20) as usize, word(24) as usize, word(28));
    if body_len < FIXED + 32 || body_len > bytes.len() || slots_at != round_up(body_len) {
        return Decoded::Damaged;
    }
    // The length is checked against the claimed slot count in 64 bits, before anything is taken from it.
    if slots_at as u64 + u64::from(slots) * SLOT_LEN as u64 != bytes.len() as u64 {
        return Decoded::Damaged;
    }
    let (signed, tail) = bytes[..body_len].split_at(body_len - 32);
    let sha: [u8; 32] = Sha256::digest(signed).into();
    if sha != tail || bytes[body_len..slots_at].iter().any(|b| *b != 0) {
        return Decoded::Damaged;
    }
    match parse(&signed[FIXED..], slots) {
        Some(mut recorded) => match scan_slots(&bytes[slots_at..], &mut recorded) {
            Some(()) => {
                recorded.body_sha = sha;
                Decoded::Pending(Box::new(recorded))
            }
            None => Decoded::Damaged,
        },
        None => Decoded::Damaged,
    }
}

/// The body, whole, and the plan it rebuilds. `slots` is the header's claim, which the plan must agree with.
fn parse(body: &[u8], slots: u32) -> Option<Recorded> {
    let mut c = Body(body);
    let take1 = |c: &mut Body<'_>| c.take(1).map(|b| b[0]);
    if take1(&mut c)? != 0 {
        return None;
    }
    let kinds = Kinds::from_bits(take1(&mut c)?).ok()?;
    let mode = match take1(&mut c)? {
        0 => TargetMode::Save,
        1 => TargetMode::SaveAs,
        _ => return None,
    };
    let signed = match take1(&mut c)? {
        0 => false,
        1 => true,
        _ => return None,
    };
    let max_artifacts = u32::from_le_bytes(c.array()?);
    let max_batch = u16::from_le_bytes(c.array()?);
    if !(1..=MAX_ARTIFACTS).contains(&max_artifacts) || !(1..=MAX_BATCH).contains(&max_batch) {
        return None;
    }
    let limits = Limits {
        max_artifacts,
        max_batch: NonZeroU16::new(max_batch)?,
    };
    let digest = PlanDigest::from_bytes(c.array()?);
    let base = u32::from_le_bytes(c.array()?);
    let actor = match take1(&mut c)? {
        0 => Actor::Person,
        1 => Actor::Agent,
        _ => return None,
    };
    let complete = u32::from_le_bytes(c.array()?);
    if complete & !KNOWN_STORES != 0 || take1(&mut c)? != 1 {
        return None;
    }
    let target = c.path(ledger::MAX_PATH_BYTES)?;
    let source = c.version()?;
    let name_len = usize::from(take1(&mut c)?);
    let name = c.take(name_len)?;
    if !name_fits(name) {
        return None;
    }
    let candidate_name = ledger::path_of(name)?;
    let candidate = c.version()?;
    let candidate_sha: [u8; 32] = c.array()?;
    let roots_n = usize::from(take1(&mut c)?);
    if roots_n > MAX_ROOTS {
        return None;
    }
    let mut roots = Vec::with_capacity(roots_n);
    for _ in 0..roots_n {
        roots.push(c.array::<16>()?);
    }
    if !roots_fit(&roots) {
        return None;
    }
    let count = u32::from_le_bytes(c.array()?);
    // Checked against the cap and against the bytes that are left before the first byte is allocated for it.
    if count > max_artifacts || (count as usize).checked_mul(ARTIFACT_LEN)? != c.0.len() {
        return None;
    }
    let (mut artifacts, mut keys, mut pins) = (
        Vec::with_capacity(count as usize),
        Vec::with_capacity(count as usize),
        Vec::with_capacity(count as usize),
    );
    for position in 0..count {
        let id = u32::from_le_bytes(c.array()?);
        let [store, owner, format] = c.array::<3>()?;
        if id != position + 1 {
            return None;
        }
        artifacts.push(Artifact {
            id: ArtifactId(id),
            store: store_of(store)?,
            owner: owner_of(owner)?,
            format: format_of(format)?,
        });
        keys.push(ArtifactKey::from_bytes(c.array()?));
        let root = take1(&mut c)?;
        let inode = u64::from_le_bytes(c.array()?);
        pins.push(match (take1(&mut c)?, root, inode) {
            (0, NO_ROOT, 0) => None,
            (1, root, inode) => Some(Pin {
                root,
                inode,
                kind: PinKind::File,
            }),
            (2, root, inode) => Some(Pin {
                root,
                inode,
                kind: PinKind::Folder,
            }),
            _ => return None,
        });
    }
    if !c.done() || !pins_fit(&artifacts, &pins, roots.len()) {
        return None;
    }
    let inventory = Inventory::with_history(StoreSet(complete), &artifacts, &keys, true);
    let plan = super::plan(&Request::new(kinds, mode, signed), &inventory, &limits).ok()?;
    let last = plan.steps.len().checked_sub(1)?;
    if plan.digest != digest
        || base != index_of(&plan, Effect::PersistBarrier)
        || plan.steps[last].effect != Effect::RetireBarrier
        || slots as usize != last.checked_sub(base as usize)?
    {
        return None;
    }
    let mut digest16 = [0u8; 16];
    digest16.copy_from_slice(&digest.as_bytes()[..16]);
    let slots_at = round_up(FIXED + body.len() + 32);
    Some(Recorded {
        plan,
        limits,
        facts: BarrierFacts {
            target,
            source,
            candidate_name,
            candidate,
            candidate_sha,
            roots,
            actor,
            pins,
        },
        layout: Layout {
            base,
            slots_at: slots_at as u32,
            slots,
            digest: digest16,
        },
        acked: base,
        body_sha: [0; 32],
    })
}

/// Reads the slot block: the highest valid slot is the acknowledged count, gaps allowed (a power loss can reorder plain syncs). `None` for a slot
/// that is whole but not where or of what it should be.
fn scan_slots(block: &[u8], recorded: &mut Recorded) -> Option<()> {
    let layout = recorded.layout;
    for (position, slot) in block.as_chunks::<SLOT_LEN>().0.iter().enumerate() {
        if slot.iter().all(|b| *b == 0) {
            continue;
        }
        let crc = u32::from_le_bytes([slot[28], slot[29], slot[30], slot[31]]);
        if crc32fast::hash(&slot[..28]) != crc {
            continue;
        }
        let (acked, at) = (
            u32::from_le_bytes([slot[0], slot[1], slot[2], slot[3]]),
            u32::from_le_bytes([slot[4], slot[5], slot[6], slot[7]]),
        );
        let expected = layout.base.checked_add(1)?.checked_add(position as u32)?;
        if acked != expected || at as usize != position || slot[8..24] != layout.digest || slot[24..28] != [0; 4] {
            return None;
        }
        recorded.acked = recorded.acked.max(acked);
    }
    Some(())
}

#[cfg(test)]
mod tests;
