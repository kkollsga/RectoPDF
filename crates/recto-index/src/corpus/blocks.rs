//! **A document's blocks, kept beside its text** (`app/blocks.bin` and `app/blocks.sealed`): which characters of each page are a paragraph, a heading, a list item,
//! a caption, a code block or a footnote (`crate::blocks`), made only when an agent asks for a blocks grain and never on opening a tab (`docs/design/blocks.md`).
//!
//! ```text
//! blocks.bin      one sealed record for each page, end to end, in order, each `blocks::codec::seal_page`: `Purpose::Blocks(page)`
//! blocks.sealed   the header, `Purpose::BlocksHeader`, written last:
//!                 [rules tag (16)] [pages u32] [pages without text u32] [unreadable u32] [stored 8 x u32, by `Shape::index`] [dropped 8 x u32] [bin bytes u64] [retries u32]
//!                 then for each page [offset u32] [length u16]
//! ```
//!
//! **Trusted only when all of these hold:** the header opens under the corpus's key and has [`rules_tag`], its table runs from 0 without a gap or an overlap and ends at
//! `bin bytes`, every length is one a record can have, and `blocks.bin` is exactly that long. A record read opens as its own page's, or [`codec::OpenError`] says why not.
//! Another rules tag is not damage: nothing is used and nothing is thrown away, and the next build replaces it.
//!
//! **All or nothing.** A build removes the header before it touches `blocks.bin` and writes it after the last record, synced. A build stopped at any point leaves a bin with
//! no header, which reads as not made, and a dropped [`BlocksBuild`] removes its bin. While one is alive it holds `build.lock`, so a clear or an eviction leaves the folder alone
//! and takes it after. The files live in the key's folder, so the sweep, the size and age limits, `index clear` and a new version of the file need no code of their own.
//!
//! **Size.** A page's record is 28 bytes of seal and 10 of its own, and 18 for each block; the header is 128 bytes and 6 for each page. A page counts against
//! [`BuildLimits::max_doc_bytes`] with the text.

use super::{BuildLimits, CorpusKey, CorpusPaths, Damaged, FindKey, MAX_PAGES, PageKind, ReadyCorpus, read_seal, write_sealed};
use crate::blocks::codec::{self, MAX_RECORD_BYTES, OpenError, record_len};
use crate::blocks::map::Shape;
use crate::blocks::{MAX_BLOCKS, PageBlocks};
use crate::seal::{self, CorpusSeal, Purpose};
use recto_platform::local;
use sha2::{Digest, Sha256};
use std::fs::{self, File};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

/// What made the candidates the blocks were mapped from: the pinned distillPDF and the scan. `recto-headings`'s `lockfile` test holds that the pin still starts with this revision.
pub const BLOCKS_RULES: &str = "distillpdf fbf5b22 typography";
/// What stands for the mapping rule and the record format in the header's tag.
const FORMAT_NAME: &str = "recto-blocks/2";
/// What the header holds before its table: the tag, three counts, two tallies of eight, the bin's length and the corpus's retries.
const HEADER_FIXED: usize = 16 + 4 * 3 + 4 * 8 * 2 + 8 + 4;
/// A page's entry in the header's table.
const ENTRY: usize = 4 + 2;
/// The most a header takes on disk.
const MAX_HEADER_BYTES: u64 = (HEADER_FIXED + MAX_PAGES as usize * ENTRY + seal::OVERHEAD) as u64;

/// The tag blocks are made and trusted under: the first 16 bytes of the SHA-256 of [`BLOCKS_RULES`], the record format and the mapping's name.
#[must_use]
pub fn rules_tag() -> [u8; 16] {
    let mut hash = Sha256::new();
    for part in [BLOCKS_RULES.as_bytes(), &[codec::FORMAT], FORMAT_NAME.as_bytes()] {
        hash.update((part.len() as u32).to_le_bytes());
        hash.update(part);
    }
    let mut tag = [0; 16];
    tag.copy_from_slice(&hash.finalize()[..16]);
    tag
}

fn bin_path(paths: &CorpusPaths) -> PathBuf {
    paths.app.join("blocks.bin")
}

fn header_path(paths: &CorpusPaths) -> PathBuf {
    paths.app.join("blocks.sealed")
}

/// What a finished set of blocks says of itself.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct BlocksMade {
    pub pages: u32,
    /// Pages whose text couldn't be had, which mapped nothing.
    pub pages_without_text: u32,
    /// Blocks distillPDF gave that couldn't be read as one.
    pub unreadable: u32,
    /// Stored and dropped blocks, by [`Shape::index`].
    pub stored: [u32; Shape::COUNT],
    pub dropped: [u32; Shape::COUNT],
    /// What the two files take: the records and the header.
    pub bytes: u64,
    /// How many times the corpus's failed pages had been read again when these were made. Blocks are made against one reading of the text: a corpus read again since has other text, and these are made again.
    pub retries: u32,
}

/// What a build's mapping counted over all its pages, which the header says.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct BlocksTally {
    pub pages_without_text: u32,
    pub unreadable: u32,
    pub stored: [u32; Shape::COUNT],
    pub dropped: [u32; Shape::COUNT],
}

/// Why a page's blocks weren't kept.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PutError {
    /// With the text, the blocks would be past what one document may take.
    TooLarge,
    /// A write that failed, a record that can't be sealed, or a page that isn't the next.
    Write,
}

/// What starting a build found.
#[derive(Debug)]
pub enum Begin {
    Building(Box<BlocksBuild>),
    /// There are blocks, made under these rules, for this many pages. Nothing to do.
    Made(BlocksMade),
    /// No finished corpus for this key, or one this key may not touch.
    NotIndexed,
    /// Another build holds the folder.
    Busy,
    /// The corpus or the folders on the way don't add up.
    Damaged,
}

/// A build of a corpus's blocks, a page at a time, in order. Holds `build.lock` for as long as it lives; dropped before [`finish`](Self::finish), it leaves nothing.
pub struct BlocksBuild {
    ready: ReadyCorpus,
    root: PathBuf,
    key: CorpusKey,
    limits: BuildLimits,
    bin: File,
    /// Held for its life: it is what a clear and an eviction look at.
    _lock: File,
    /// The next page to put.
    next: u32,
    /// Where each page's record is, and how long.
    table: Vec<(u32, u16)>,
    bin_bytes: u64,
    page_buffer: seal::Plain,
    chunk: Vec<u8>,
    finished: bool,
}

impl std::fmt::Debug for BlocksBuild {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        out.debug_struct("BlocksBuild")
            .field("next", &self.next)
            .field("pages", &self.pages())
            .finish_non_exhaustive()
    }
}

/// How the folders on the way to a corpus stand.
enum Folders {
    Private,
    /// There is no corpus folder.
    Absent,
    /// Something is there that isn't private to this user.
    Unsafe,
}

fn folders(paths: &CorpusPaths, root: &Path) -> Folders {
    if paths.folders_are_private(root) {
        Folders::Private
    } else if matches!(fs::symlink_metadata(&paths.app), Err(error) if error.kind() == io::ErrorKind::NotFound) {
        Folders::Absent
    } else {
        Folders::Unsafe
    }
}

impl BlocksBuild {
    /// Starts a build, or says why there is none to make. See the module's notes for the order: the key, the folders, the lock, the corpus, what's made, and only then is anything removed.
    pub fn begin(find_key: &FindKey, root: &Path, key: CorpusKey, limits: &BuildLimits) -> Begin {
        if !seal::may_key(find_key, key) {
            return Begin::NotIndexed;
        }
        let paths = CorpusPaths::new(root, key);
        match folders(&paths, root) {
            Folders::Private => {}
            Folders::Absent => return Begin::NotIndexed,
            Folders::Unsafe => return Begin::Damaged,
        }
        let Ok(lock) = local::append_private_file(&paths.lock) else {
            return Begin::Damaged;
        };
        if lock.try_lock().is_err() {
            return Begin::Busy;
        }
        let ready = match ReadyCorpus::open(find_key, root, key) {
            Ok(Some(ready)) => ready,
            Ok(None) => return Begin::NotIndexed,
            Err(Damaged) => return Begin::Damaged,
        };
        if let Ok(Some(found)) = parse(&ready.seal, &paths)
            && found.made.pages == ready.pages()
            && found.made.retries == ready.summary.retries
        {
            return Begin::Made(found.made);
        }
        // Whatever is there isn't this build's, or is another's: the header goes first, so nothing is believed while the bin is being replaced.
        remove_files(&paths);
        let Ok(bin) = local::append_private_file(&bin_path(&paths)) else {
            return Begin::Damaged;
        };
        Begin::Building(Box::new(Self {
            table: Vec::with_capacity(ready.pages() as usize),
            ready,
            root: root.to_path_buf(),
            key,
            limits: *limits,
            bin,
            _lock: lock,
            next: 0,
            bin_bytes: 0,
            page_buffer: seal::Plain::default(),
            chunk: Vec::new(),
            finished: false,
        }))
    }

    /// The document's pages, as the corpus has them.
    #[must_use]
    pub fn pages(&self) -> u32 {
        self.ready.pages()
    }

    /// Page `page`'s characters, in the numbering `recto text` gives, into `chars` (which is cleared). False when the page has no text to map against: no text, failed, or can't be read.
    pub fn text(&mut self, page: u32, chars: &mut Vec<char>) -> bool {
        chars.clear();
        // A long page leaves its room in these buffers for the rest of the job. The characters' is given back past 1 Mi of them (4 MiB), and the page's bytes' past 1 MiB, at the next page.
        if chars.capacity() > 1 << 20 {
            *chars = Vec::new();
        }
        let found = match self.ready.page(page, &mut self.page_buffer) {
            Ok(stored) if stored.kind == PageKind::Text => {
                chars.extend(stored.text.chars());
                true
            }
            _ => false,
        };
        zeroize::Zeroize::zeroize(&mut *self.page_buffer);
        if self.page_buffer.capacity() > 1 << 20 {
            self.page_buffer = seal::Plain::default();
        }
        found
    }

    /// Writes the next page's record.
    ///
    /// # Errors
    ///
    /// [`PutError::TooLarge`] when the text and the blocks together would pass [`BuildLimits::max_doc_bytes`] (nothing of this page is written); otherwise [`PutError::Write`].
    pub fn put(&mut self, page: &PageBlocks) -> Result<(), PutError> {
        if page.page != self.next || self.next >= self.pages() {
            return Err(PutError::Write);
        }
        codec::seal_page(&self.ready.seal, page, &mut self.chunk).map_err(|_| PutError::Write)?;
        let bin_bytes = self.bin_bytes + self.chunk.len() as u64;
        // What the header will be, with every page's entry, counts now, so the last page can't be the one that finds it's over.
        let header = (HEADER_FIXED + self.pages() as usize * ENTRY + seal::OVERHEAD) as u64;
        if self.ready.summary.corpus_bytes + bin_bytes + header > self.limits.max_doc_bytes || bin_bytes > u64::from(u32::MAX) {
            return Err(PutError::TooLarge);
        }
        let len = u16::try_from(self.chunk.len()).map_err(|_| PutError::Write)?;
        let offset = u32::try_from(self.bin_bytes).map_err(|_| PutError::TooLarge)?;
        self.bin.write_all(&self.chunk).map_err(|_| PutError::Write)?;
        self.table.push((offset, len));
        self.bin_bytes = bin_bytes;
        self.next += 1;
        Ok(())
    }

    /// Ends the build: only when every page has been put, the bin is synced and then the header is written, last. Then the corpora are kept to their limits, as a build of the text does.
    ///
    /// # Errors
    ///
    /// `InvalidData` when a page is missing; otherwise what the disk said. Nothing is left either way.
    pub fn finish(mut self, tally: &BlocksTally) -> io::Result<BlocksMade> {
        if self.next != self.pages() {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "a page's blocks are missing"));
        }
        self.bin.sync_all()?;
        let mut plain = Vec::with_capacity(HEADER_FIXED + self.table.len() * ENTRY);
        plain.extend_from_slice(&rules_tag());
        for count in [self.pages(), tally.pages_without_text, tally.unreadable] {
            plain.extend_from_slice(&count.to_le_bytes());
        }
        for count in tally.stored.iter().chain(&tally.dropped) {
            plain.extend_from_slice(&count.to_le_bytes());
        }
        plain.extend_from_slice(&self.bin_bytes.to_le_bytes());
        plain.extend_from_slice(&self.ready.summary.retries.to_le_bytes());
        for (offset, len) in &self.table {
            plain.extend_from_slice(&offset.to_le_bytes());
            plain.extend_from_slice(&len.to_le_bytes());
        }
        let paths = &self.ready.paths;
        write_sealed(&self.ready.seal, &header_path(paths), Purpose::BlocksHeader, &plain, true)?;
        self.finished = true;
        let made = BlocksMade {
            pages: self.pages(),
            pages_without_text: tally.pages_without_text,
            unreadable: tally.unreadable,
            stored: tally.stored,
            dropped: tally.dropped,
            bytes: self.bin_bytes + (plain.len() + seal::OVERHEAD) as u64,
            retries: self.ready.summary.retries,
        };
        let _ = super::evict(&self.root, &self.limits, Some(self.key));
        Ok(made)
    }
}

impl Drop for BlocksBuild {
    fn drop(&mut self) {
        if !self.finished {
            remove_files(&self.ready.paths);
        }
    }
}

/// A finished document's blocks, as a reader opens them. Holds the one descriptor of `blocks.bin`, for as long as a reader does.
#[derive(Debug)]
pub struct StoredBlocks {
    seal: CorpusSeal,
    file: File,
    made: BlocksMade,
    table: Vec<(u32, u16)>,
}

impl StoredBlocks {
    /// Opens a corpus's blocks. `Ok(None)` when there are none to use: none made, or made under other rules. `Err` when there are and they don't add up, which is for the caller to throw away ([`remove_blocks`]).
    ///
    /// # Errors
    ///
    /// [`Damaged`].
    pub fn open(find_key: &FindKey, root: &Path, key: CorpusKey) -> Result<Option<Self>, Damaged> {
        Self::open_for(find_key, root, key, None)
    }

    /// [`Self::open`] for blocks to be read now: `Ok(None)` too when they were made for a corpus of another page count, or one read again since (`expect.retries`), which `begin` makes again. Without this a
    /// retried failed page would read `Stale` for ever, and a header for another page count would be believed.
    ///
    /// # Errors
    ///
    /// [`Damaged`].
    pub fn open_matching(find_key: &FindKey, root: &Path, key: CorpusKey, expect: &super::Summary) -> Result<Option<Self>, Damaged> {
        Self::open_for(find_key, root, key, Some(expect))
    }

    fn open_for(find_key: &FindKey, root: &Path, key: CorpusKey, expect: Option<&super::Summary>) -> Result<Option<Self>, Damaged> {
        let paths = CorpusPaths::new(root, key);
        match folders(&paths, root) {
            Folders::Private => {}
            Folders::Absent => return Ok(None),
            Folders::Unsafe => return Err(Damaged),
        }
        if !seal::may_key(find_key, key) {
            return Ok(None);
        }
        if fs::symlink_metadata(header_path(&paths)).is_err() {
            return Ok(None);
        }
        let seal = read_seal(find_key, &paths).ok_or(Damaged)?;
        let found = parse(&seal, &paths)?;
        Ok(found
            .filter(|found| expect.is_none_or(|expect| found.made.pages == expect.pages && found.made.retries == expect.retries))
            .map(|found| Self {
                seal,
                file: found.file,
                made: found.made,
                table: found.table,
            }))
    }
    /// What the header says.
    #[must_use]
    pub fn made(&self) -> &BlocksMade {
        &self.made
    }

    /// Page `page`'s blocks, for a page whose text is `chars_now` characters long now.
    ///
    /// # Errors
    ///
    /// [`OpenError`]: `Damaged` for a record that doesn't open as this page's or a page past the document, `Stale` for one made for a page of another length.
    pub fn page(&self, page: u32, chars_now: u32) -> Result<PageBlocks, OpenError> {
        let &(offset, len) = self.table.get(page as usize).ok_or(OpenError::Damaged)?;
        let mut chunk = vec![0; usize::from(len)];
        recto_platform::read_exact_at(&self.file, &mut chunk, u64::from(offset)).map_err(|_| OpenError::Damaged)?;
        codec::open_page(&self.seal, page, chars_now, &mut chunk)
    }
}

/// What a header says, once it has been checked against the bin beside it.
struct Parsed {
    made: BlocksMade,
    table: Vec<(u32, u16)>,
    file: File,
}

/// Opens a corpus's header under its seal and checks it against the bin. `Ok(None)` when there is no header or it is of other rules.
fn parse(seal: &CorpusSeal, paths: &CorpusPaths) -> Result<Option<Parsed>, Damaged> {
    let Some(mut header) = local::read_private_file(&header_path(paths), MAX_HEADER_BYTES).map_err(|_| Damaged)? else {
        return Ok(None);
    };
    let range = seal.open_in_place(Purpose::BlocksHeader, &mut header).map_err(|_| Damaged)?;
    let plain = &header[range];
    if plain.len() < HEADER_FIXED {
        return Err(Damaged);
    }
    if plain[..16] != rules_tag() {
        return Ok(None);
    }
    let word = |at: usize| u32::from_le_bytes([plain[at], plain[at + 1], plain[at + 2], plain[at + 3]]);
    let mut made = BlocksMade {
        pages: word(16),
        pages_without_text: word(20),
        unreadable: word(24),
        retries: word(HEADER_FIXED - 4),
        ..BlocksMade::default()
    };
    for shape in 0..Shape::COUNT {
        made.stored[shape] = word(28 + shape * 4);
        made.dropped[shape] = word(28 + Shape::COUNT * 4 + shape * 4);
    }
    let bin_bytes = u64::from_le_bytes(plain[HEADER_FIXED - 12..HEADER_FIXED - 4].try_into().map_err(|_| Damaged)?);
    let stored_total: u64 = made.stored.iter().map(|count| u64::from(*count)).sum();
    // Tables and figures have no text to point at, and a page holds only so many blocks.
    let impossible = made.pages > MAX_PAGES
        || made.pages_without_text > made.pages
        || made.stored[Shape::Table.index()] != 0
        || made.stored[Shape::Figure.index()] != 0
        || stored_total > u64::from(made.pages) * MAX_BLOCKS as u64
        || plain.len() != HEADER_FIXED + made.pages as usize * ENTRY;
    if impossible {
        return Err(Damaged);
    }
    let mut table = Vec::with_capacity(made.pages as usize);
    let mut at = 0u64;
    for entry in plain[HEADER_FIXED..].as_chunks::<ENTRY>().0 {
        let offset = u32::from_le_bytes([entry[0], entry[1], entry[2], entry[3]]);
        let len = u16::from_le_bytes([entry[4], entry[5]]);
        if u64::from(offset) != at || usize::from(len) < record_len(0) || usize::from(len) > MAX_RECORD_BYTES {
            return Err(Damaged);
        }
        at += u64::from(len);
        table.push((offset, len));
    }
    let file = local::open_private_file(&bin_path(paths)).map_err(|_| Damaged)?;
    if at != bin_bytes || file.metadata().map_err(|_| Damaged)?.len() != bin_bytes {
        return Err(Damaged);
    }
    made.bytes = bin_bytes + (plain.len() + seal::OVERHEAD) as u64;
    Ok(Some(Parsed { made, table, file }))
}

/// Takes a corpus's blocks away: the header first, then the bin. A corpus without them is answered as one that never had them. **A folder a build holds is left alone:** a reader cleaning up
/// damaged blocks must not unlink the bin a new build is writing, or the finished build would read as damaged.
pub fn remove_blocks(root: &Path, key: CorpusKey) {
    let paths = CorpusPaths::new(root, key);
    if !paths.folders_are_private(root) {
        return;
    }
    let Ok(lock) = local::append_private_file(&paths.lock) else { return };
    if lock.try_lock().is_ok() {
        remove_files(&paths);
    }
}

/// Both files, the header first.
pub(super) fn remove_files(paths: &CorpusPaths) {
    let _ = fs::remove_file(header_path(paths));
    let _ = fs::remove_file(bin_path(paths));
}

/// **For a test:** a finished corpus whose pages' text is `texts` (a page of `None` has no text), made as a build makes one, for the file `key` stands for (noted as that file's when `file` is given). What a document's index would hold of it, without a renderer.
///
/// # Errors
///
/// What the build says.
#[cfg(any(test, feature = "test-seams"))]
pub fn build_text_corpus(find_key: &FindKey, root: &Path, (key, file): (CorpusKey, Option<&Path>), texts: &[Option<String>]) -> io::Result<()> {
    use super::{DocMeta, FedPage, FeedError, PageFeed};
    struct Texts<'a>(&'a [Option<String>]);
    impl PageFeed for Texts<'_> {
        fn page_count(&mut self) -> Result<u32, FeedError> {
            Ok(self.0.len() as u32)
        }
        fn read(&mut self, page: u32) -> Result<FedPage, FeedError> {
            let text = self.0[page as usize].as_deref().unwrap_or("");
            Ok(FedPage {
                chars: text.chars().map(|ch| recto_proto::CharRecord::new(ch as u32, 0)).collect(),
                boxes: Vec::new(),
                hidden_checked: true,
            })
        }
        fn still_the_same_file(&mut self) -> bool {
            true
        }
    }
    let (meta, limits) = (DocMeta::default(), BuildLimits::default());
    match file {
        Some(file) => super::build_for_file(find_key, file, root, key, &mut Texts(texts), &meta, &limits).map(|_| ()),
        None => super::build(find_key, root, key, &mut Texts(texts), &meta, &limits).map(|_| ()),
    }
}

#[cfg(test)]
mod tests;
