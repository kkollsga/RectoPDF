//! **A document's page identities, kept beside its text** (`app/ids.bin` and `app/ids.sealed`), so a search answered from the index can refer to a hit's page
//! without opening the file: the references are the ones `recto ref` makes, from reads taken by `recto_core::page_id::read_page` when the corpus was built.
//!
//! ```text
//! ids.bin      one sealed record for each page of lopdf's page map, in order, each `RECORD_BYTES` long: `Purpose::Identity(page)` over
//!              [identity present (1)] [digest (32)] [object number (4)] [generation (2)] [frame tag (4)] [bytes the read was charged (8)]
//! ids.sealed   the header, `Purpose::Identities`: [rules tag (16)] [map length (4)] [`/ID` pair present (1)] [first (16)] [second (16)]; written last, before `ready.sealed`
//! ```
//!
//! **Trusted only when all of these hold:** the header opens under the corpus's key, its rules tag is [`stored_rules_tag`] (so identities made under other
//! caps never answer), `ids.bin` is exactly the header's map length times a record, and each record read opens as its own page's. A rules tag that is
//! another build's is not damage: nothing is used and nothing is thrown away. An old corpus has neither file and is answered as before.
//!
//! **All or nothing.** The header is not written when the file can't be opened by lopdf, when its map is past [`MAX_PAGES`], or when the reads cost more than
//! [`BuildLimits::identity_bytes`](super::BuildLimits). A build records page `p`'s identity beside its text, so the helper holds nothing for each page.

use super::{BuildLimits, CorpusPaths, Damaged, FindKey, MAX_PAGES, PageFeed, read_seal};
use crate::seal::{self, CorpusSeal, Purpose};
use recto_core::locator::{PageSource, SourceRead, stored_rules_tag};
use recto_core::page_id::{self, Caps, DocumentIds, IndexedReader, PageIdentity, PageMap, PageRead};
use recto_platform::local;
use std::fs::{self, File};
use std::io::{self, Write};
use std::path::Path;

/// What a record holds before it's sealed.
const RECORD_PLAIN: usize = 1 + 32 + 4 + 2 + 4 + 8;
/// A record on disk: what it holds, the nonce and the tag. `docs/engineering/search-index.md` says 79 bytes a page.
pub const RECORD_BYTES: usize = RECORD_PLAIN + seal::OVERHEAD;
const HEADER_PLAIN: usize = 16 + 4 + 1 + 16 + 16;
/// The header on disk.
pub const HEADER_BYTES: usize = HEADER_PLAIN + seal::OVERHEAD;

/// What a feed says of its document's identities, once, before the first page.
#[derive(Clone, Copy, Debug)]
pub struct IdentityInfo {
    /// Pages in lopdf's page map, which is what a reference counts.
    pub pages: u32,
    pub ids: Option<DocumentIds>,
}

/// The file's pages, read by lopdf and nothing else, for the identities a build stores.
pub struct FileIdentities {
    reader: IndexedReader,
    map: PageMap,
}

impl FileIdentities {
    /// Opens exactly this version of the file (`stamp` is what the key's check just saw), through the file wall. `None` if lopdf can't read it.
    pub fn open(path: &Path, stamp: recto_platform::FileStamp) -> Option<Self> {
        let reader = page_id::open_version_for_identity(path, stamp)?;
        let map = reader.page_map().ok()?;
        Some(Self { reader, map })
    }

    pub fn info(&self) -> IdentityInfo {
        IdentityInfo {
            pages: u32::try_from(self.map.len()).unwrap_or(u32::MAX),
            ids: page_id::document_ids(&self.reader),
        }
    }

    /// Page `page` as a reference reads it: under [`Caps::STANDARD`]. A page the map hasn't costs nothing and has no identity.
    pub fn read(&self, page: u32) -> PageRead {
        self.map.get(page as usize).map_or(PageRead { identity: None, bytes: 0 }, |entry| {
            page_id::read_page(&self.reader, entry, &Caps::STANDARD)
        })
    }
}

fn encode(read: &PageRead) -> [u8; RECORD_PLAIN] {
    let mut out = [0; RECORD_PLAIN];
    if let Some(identity) = &read.identity {
        out[0] = 1;
        out[1..33].copy_from_slice(&identity.digest);
        out[33..37].copy_from_slice(&identity.object.0.to_le_bytes());
        out[37..39].copy_from_slice(&identity.object.1.to_le_bytes());
        out[39..43].copy_from_slice(&identity.frame.to_le_bytes());
    }
    out[43..].copy_from_slice(&read.bytes.to_le_bytes());
    out
}

/// A record's plain bytes, if they say something a build could have written: a present flag of 0 or 1, and no identity's bytes where there is none.
fn decode(bytes: &[u8]) -> Option<PageRead> {
    let bytes: &[u8; RECORD_PLAIN] = bytes.try_into().ok()?;
    let charged = u64::from_le_bytes(bytes[43..].try_into().ok()?);
    match bytes[0] {
        0 if bytes[1..43].iter().all(|byte| *byte == 0) => Some(PageRead {
            identity: None,
            bytes: charged,
        }),
        1 => Some(PageRead {
            identity: Some(PageIdentity {
                digest: bytes[1..33].try_into().ok()?,
                object: (
                    u32::from_le_bytes(bytes[33..37].try_into().ok()?),
                    u16::from_le_bytes(bytes[37..39].try_into().ok()?),
                ),
                frame: u32::from_le_bytes(bytes[39..43].try_into().ok()?),
            }),
            bytes: charged,
        }),
        _ => None,
    }
}

/// One record opened where it lies in `ids.bin`.
fn open_record(file: &File, seal: &CorpusSeal, page: u32) -> Option<PageRead> {
    let mut chunk = [0; RECORD_BYTES];
    recto_platform::read_exact_at(file, &mut chunk, u64::from(page) * RECORD_BYTES as u64).ok()?;
    let plain = seal.open_in_place(Purpose::Identity(page), &mut chunk).ok()?;
    decode(&chunk[plain])
}

/// The identities a build writes, a record as each page is built.
pub(super) struct IdentityRun {
    file: File,
    info: IdentityInfo,
    /// Records in the file.
    written: u32,
    /// What the reads were charged, against the limit.
    work: u64,
    limit: u64,
}

impl IdentityRun {
    /// Starts, or goes on with what an earlier build left: the first `resumed` pages' records stay if the file holds exactly those and each opens as its page's, and
    /// are read again if not. `None` when the identities can't be had: the map is too long, or the reads cost more than `limit`. Either way no file is left.
    pub(super) fn begin(
        paths: &CorpusPaths,
        seal: &CorpusSeal,
        info: IdentityInfo,
        resumed: u32,
        limit: u64,
        mut read: impl FnMut(u32) -> PageRead,
    ) -> io::Result<Option<Self>> {
        let _ = fs::remove_file(paths.ids_header());
        if info.pages > MAX_PAGES {
            let _ = fs::remove_file(paths.ids_file());
            return Ok(None);
        }
        let keep = resumed.min(info.pages);
        let kept = Self::kept(paths, seal, keep);
        if kept.is_none() {
            let _ = fs::remove_file(paths.ids_file());
        }
        let file = local::append_private_file(&paths.ids_file())?;
        if !file.metadata()?.is_file() {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "not a plain file"));
        }
        let mut run = Self {
            file,
            info,
            written: if kept.is_some() { keep } else { 0 },
            work: kept.unwrap_or(0),
            limit,
        };
        if run.work > limit {
            run.abandon(paths);
            return Ok(None);
        }
        while run.written < keep {
            let page = run.written;
            if !run.record(paths, seal, page, &mut read)? {
                return Ok(None);
            }
        }
        Ok(Some(run))
    }

    /// What the first `keep` records were charged, if `ids.bin` holds exactly them and each opens as its page's.
    fn kept(paths: &CorpusPaths, seal: &CorpusSeal, keep: u32) -> Option<u64> {
        let file = local::open_private_file(&paths.ids_file()).ok()?;
        if file.metadata().ok()?.len() != u64::from(keep) * RECORD_BYTES as u64 {
            return None;
        }
        (0..keep).try_fold(0u64, |work, page| Some(work.saturating_add(open_record(&file, seal, page)?.bytes)))
    }

    /// Page `page`'s record, once the pages before it are in. `Ok(false)` when the reads went over the limit: nothing is left of the identities.
    pub(super) fn record(&mut self, paths: &CorpusPaths, seal: &CorpusSeal, page: u32, read: &mut impl FnMut(u32) -> PageRead) -> io::Result<bool> {
        if page != self.written || page >= self.info.pages {
            return Ok(true);
        }
        let read = read(page);
        self.work = self.work.saturating_add(read.bytes);
        if self.work > self.limit {
            self.abandon(paths);
            return Ok(false);
        }
        let mut chunk = Vec::with_capacity(RECORD_BYTES);
        seal.seal(Purpose::Identity(page), &mut chunk, |out| out.extend_from_slice(&encode(&read)))?;
        self.file.write_all(&chunk)?;
        self.written += 1;
        Ok(true)
    }

    fn abandon(&mut self, paths: &CorpusPaths) {
        let _ = fs::remove_file(paths.ids_file());
    }

    /// What the records take on the disk so far.
    pub(super) fn disk_bytes(&self) -> u64 {
        u64::from(self.written) * RECORD_BYTES as u64
    }

    /// Whether every page of the map has its record.
    pub(super) fn is_complete(&self) -> bool {
        self.written == self.info.pages
    }

    /// The header, after the records are on the disk. Written last of the corpus's files but `ready.sealed`.
    pub(super) fn seal_header(&mut self, paths: &CorpusPaths, seal: &CorpusSeal) -> io::Result<u64> {
        self.file.sync_all()?;
        let mut plain = Vec::with_capacity(HEADER_PLAIN);
        plain.extend_from_slice(&stored_rules_tag());
        plain.extend_from_slice(&self.info.pages.to_le_bytes());
        match self.info.ids {
            Some(ids) => {
                plain.push(1);
                plain.extend_from_slice(&ids.first);
                plain.extend_from_slice(&ids.second);
            }
            None => plain.extend_from_slice(&[0; 33]),
        }
        super::sealed::write_sealed(seal, &paths.ids_header(), Purpose::Identities, &plain, true)?;
        Ok(HEADER_BYTES as u64)
    }
}

/// A document's stored identities, as a reference reads them. Holds the one descriptor of `ids.bin`, for as long as a search does.
pub struct Identities {
    seal: CorpusSeal,
    file: File,
    pages: u32,
    ids: Option<DocumentIds>,
}

impl Identities {
    /// Opens a finished corpus's identities. `Ok(None)` when there are none to use: an older corpus, or ones made under other rules. `Err` when there are and they
    /// don't add up, which is for the caller to throw away ([`remove_identities`]).
    pub fn open(find_key: &FindKey, root: &Path, key: super::CorpusKey) -> Result<Option<Self>, Damaged> {
        let paths = CorpusPaths::new(root, key);
        if !paths.folders_are_private(root) {
            return Err(Damaged);
        }
        let Some(mut header) = local::read_private_file(&paths.ids_header(), HEADER_BYTES as u64).map_err(|_| Damaged)? else {
            return Ok(None);
        };
        let seal = read_seal(find_key, &paths).ok_or(Damaged)?;
        let plain = seal.open_in_place(Purpose::Identities, &mut header).map_err(|_| Damaged)?;
        let plain = &header[plain];
        if plain.len() != HEADER_PLAIN {
            return Err(Damaged);
        }
        if plain[..16] != stored_rules_tag() {
            return Ok(None);
        }
        let pages = u32::from_le_bytes(plain[16..20].try_into().map_err(|_| Damaged)?);
        let ids = match plain[20] {
            0 => None,
            1 => Some(DocumentIds {
                first: plain[21..37].try_into().map_err(|_| Damaged)?,
                second: plain[37..53].try_into().map_err(|_| Damaged)?,
            }),
            _ => return Err(Damaged),
        };
        let file = local::open_private_file(&paths.ids_file()).map_err(|_| Damaged)?;
        if pages > MAX_PAGES || file.metadata().map_err(|_| Damaged)?.len() != u64::from(pages) * RECORD_BYTES as u64 {
            return Err(Damaged);
        }
        Ok(Some(Self { seal, file, pages, ids }))
    }

    /// The `/ID` pair of the document as the build read it.
    pub fn document_ids(&self) -> Option<DocumentIds> {
        self.ids
    }
}

impl PageSource for Identities {
    fn len(&self) -> usize {
        self.pages as usize
    }

    fn read(&self, index: u32, _: &Caps) -> SourceRead {
        if index >= self.pages {
            return SourceRead::Absent;
        }
        open_record(&self.file, &self.seal, index).map_or(SourceRead::Unreadable, SourceRead::Read)
    }

    fn document_id(&self) -> Option<[u8; 8]> {
        self.ids.map(|ids| ids.first[..8].try_into().unwrap_or([0; 8]))
    }

    fn caps(&self) -> Option<Caps> {
        Some(Caps::STANDARD)
    }
}

/// Takes a corpus's identities away: both files. A corpus without them is answered as an older one is.
pub fn remove_identities(root: &Path, key: super::CorpusKey) {
    let paths = CorpusPaths::new(root, key);
    if paths.folders_are_private(root) {
        remove_files(&paths);
    }
}

pub(super) fn remove_files(paths: &CorpusPaths) {
    let _ = fs::remove_file(paths.ids_header());
    let _ = fs::remove_file(paths.ids_file());
}

// ---------------------------------------------------------------------------------------------------------------------
// What a build does with them: three calls, none of which holds anything for a page
// ---------------------------------------------------------------------------------------------------------------------

/// Starts a build's identities, if its feed can read them, and takes away what an older build left if not.
pub(super) fn start(paths: &CorpusPaths, seal: &CorpusSeal, feed: &mut impl PageFeed, resumed: u32, limits: &BuildLimits) -> io::Result<Option<IdentityRun>> {
    let Some(info) = feed.identities() else {
        remove_files(paths);
        return Ok(None);
    };
    IdentityRun::begin(paths, seal, info, resumed, limits.identity_bytes, |page| feed.identity(page))
}

/// Page `page`'s record. Past the limit the identities are given up: `run` is `None` after, and no file is left.
pub(super) fn step(run: &mut Option<IdentityRun>, paths: &CorpusPaths, seal: &CorpusSeal, feed: &mut impl PageFeed, page: u32) -> io::Result<()> {
    if let Some(active) = run
        && !active.record(paths, seal, page, &mut |page| feed.identity(page))?
    {
        *run = None;
    }
    Ok(())
}

/// What the identities take on the disk so far.
pub(super) fn disk_bytes(run: &Option<IdentityRun>) -> u64 {
    run.as_ref().map_or(0, IdentityRun::disk_bytes)
}

/// The pages the map has past the renderer's count: all of the identity reads, so they come before the last look at the file.
pub(super) fn complete(run: &mut Option<IdentityRun>, paths: &CorpusPaths, seal: &CorpusSeal, feed: &mut impl PageFeed) -> io::Result<()> {
    let Some(pages) = run.as_ref().map(|run| run.info.pages) else { return Ok(()) };
    for page in 0..pages {
        step(run, paths, seal, feed, page)?;
    }
    if run.as_ref().is_some_and(|run| !run.is_complete()) {
        remove_files(paths);
        *run = None;
    }
    Ok(())
}

/// The header, after the last look at the file and before `ready.sealed`. What the identities take on the disk, header included; 0 where there are none.
pub(super) fn seal_header(run: &mut Option<IdentityRun>, paths: &CorpusPaths, seal: &CorpusSeal) -> io::Result<u64> {
    let Some(active) = run else { return Ok(0) };
    Ok(active.disk_bytes() + active.seal_header(paths, seal)?)
}

#[cfg(test)]
mod tests;
