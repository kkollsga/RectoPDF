//! **A sleep file: what a tab's first frame needs, without opening its PDF** (SPEC decision 25). Owner, 2026-11-07: "When it's on deep sleep, we want its state to be stored on disk, not on memory. But it needs to be stored in a way that's quick to recover without any delay."
//!
//! A low-resolution picture of the pages at its place, the sizes of the pages known, and its contents. It's about one version of one file, by its stamp, and it's only ever a way to be quick: **it can always be thrown away and rebuilt from the PDF,** unlike a journal, so anything wrong with one is a reason to do just that.
//!
//! **Treated as hostile,** as anything in a folder can be changed. A checksum says it's whole and nothing more. Every number's held to what a real one could be before it sizes anything, and pictures are inflated to exactly the size they say, which is capped. Pixels only ever reach a surface of their own, and a title goes through the same cleaning as one from a PDF. A password-protected document never gets one, as this would be its contents in the clear.

use crate::record::{FileRecord, RecordError, open_sealed, seal};
use recto_core::nav::{LabelRange, LabelStyle, MAX_LABEL_PREFIX_BYTES, MAX_LABEL_RANGES, PageLabels};
use recto_core::{MAX_TOC_DEPTH, MAX_TOC_ENTRIES, NONE, Toc, TocLine};
use recto_platform::FileStamp;
use serde::{Deserialize, Serialize};
use std::io::{Read, Write};

/// What a sleep file says it is.
pub const SLEEP_KIND: &str = "recto-sleep";

/// Pictures in one file: enough for the pages one window shows at an ordinary zoom, side by side too, where a view has twice the pages, each a quarter of the pixels. A view of more than this comes up with those and sharpens. An older build reads a file with more as damaged, and rebuilds it from the PDF.
pub const MAX_SLEEP_PICTURES: usize = 12;
/// The most one picture can be, unpacked. As a preview is.
pub const MAX_PICTURE_BYTES: usize = 4 * 1024 * 1024;
/// The most a picture takes up on disk (owner, 2026-11-07: "reduce SSD wear as much as we can"). It's there for a moment, so a page that doesn't pack to this goes at a lower resolution, and fewer colours, till it does: see [`pack_picture`]. A page of text never comes near it.
pub const MAX_PACKED_PICTURE: usize = 100 * 1024;
/// The most a file can be read as.
pub const MAX_SLEEP_FILE_BYTES: usize = 32 * 1024 * 1024;
/// Page sizes kept, past which the rest are asked for as they come.
const MAX_SIZES: usize = 4096;
/// Bytes per pixel of a picture: 8-bit BGRA, as a surface has them.
const PIXEL: usize = 4;

/// One page's picture, unpacked. Rows are `width * 4` bytes.
#[derive(Clone, Debug, PartialEq)]
pub struct SleepPicture {
    /// Where it is in the document, as the view counts: a position, with edits counted.
    pub page: u32,
    /// Pixels per point of the page.
    pub scale: f32,
    pub width: u32,
    pub height: u32,
    /// Drawn for dark paper, which isn't what white paper wants, and the other way about.
    pub dark: bool,
    pub bgra: Vec<u8>,
}

/// What a sleep file holds.
#[derive(Clone, Debug, PartialEq)]
pub struct SleepFile {
    /// The file this is about. Anything else at that path, and it says nothing.
    pub file: FileStamp,
    /// How long the file is, as opposed to the document with its edits.
    pub file_pages: u32,
    /// How long the document was, and what was on its pages, so a list that's different now says this isn't about it. See `PageEdits::order_mark`.
    pub pages: u32,
    pub list_seq: u64,
    pub renderer_only: bool,
    /// Which version of the file its pictures are of, as the worker said: a rewrite that keeps size, time and number isn't this.
    pub version: Option<u64>,
    pub toc: Toc,
    /// The usual page's size, in points, and the known sizes of pages that aren't it.
    pub usual: Option<(f32, f32)>,
    pub sizes: Vec<(u32, f32, f32)>,
    pub pictures: Vec<SleepPicture>,
}

#[derive(Serialize, Deserialize)]
struct PictureLine {
    page: u32,
    scale: f32,
    width: u32,
    height: u32,
    dark: bool,
    /// Bytes it takes up after the first line, once packed.
    packed: u32,
    /// Each row's held as the difference from the pixel to its left, which is what lets a photograph pack.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    delta: bool,
}

#[derive(Serialize, Deserialize)]
struct Meta {
    file: FileRecord,
    file_pages: u32,
    pages: u32,
    list_seq: u64,
    renderer_only: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    version: Option<u64>,
    truncated: bool,
    toc: Vec<(String, u32, u32, u8)>,
    /// The file's page labels, as `(start, first, style, prefix)`, so a tab that wakes from here shows `iv` as it did. An older file has none, and the tab shows numbers until its PDF is read.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    labels: Vec<(u32, u32, u8, String)>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    labels_truncated: bool,
    usual: Option<(f32, f32)>,
    sizes: Vec<(u32, f32, f32)>,
    pictures: Vec<PictureLine>,
}

fn good_size(width: f32, height: f32) -> bool {
    width.is_finite() && height.is_finite() && width > 0.0 && height > 0.0 && width <= 14_400.0 && height <= 14_400.0
}

/// What a picture's numbers come to, if they're possible: its unpacked bytes.
fn picture_bytes(width: u32, height: u32, scale: f32) -> Option<usize> {
    if width == 0 || height == 0 || !scale.is_finite() || scale <= 0.0 || scale > 64.0 {
        return None;
    }
    let bytes = u64::from(width).checked_mul(u64::from(height))?.checked_mul(PIXEL as u64)?;
    (bytes <= MAX_PICTURE_BYTES as u64).then_some(bytes as usize)
}

fn deflated(bytes: &[u8], level: flate2::Compression) -> Option<Vec<u8>> {
    let mut deflate = flate2::write::DeflateEncoder::new(Vec::with_capacity(bytes.len() / 8), level);
    deflate.write_all(bytes).ok()?;
    deflate.finish().ok()
}

/// A picture smaller by a whole step, each pixel the average of what it covers, with only the top bits of each colour kept.
fn shrunk(picture: &SleepPicture, step: u32, keep_bits: u32) -> (u32, u32, Vec<u8>) {
    let (width, height) = ((picture.width / step).max(1), (picture.height / step).max(1));
    let (step, row) = (step as usize, picture.width as usize * PIXEL);
    let mask = 0xFFu8 << (8 - keep_bits);
    let mut out = Vec::with_capacity(width as usize * height as usize * PIXEL);
    for y in 0..height as usize {
        for x in 0..width as usize {
            let mut sum = [0u32; PIXEL];
            let mut count = 0u32;
            for line in y * step..((y + 1) * step).min(picture.height as usize) {
                let start = line * row + x * step * PIXEL;
                let end = (start + step * PIXEL).min((line + 1) * row);
                for pixel in picture.bgra[start..end].as_chunks::<PIXEL>().0 {
                    for (total, byte) in sum.iter_mut().zip(pixel) {
                        *total += u32::from(*byte);
                    }
                    count += 1;
                }
            }
            for (channel, total) in sum.iter().enumerate() {
                let value = (total / count.max(1)) as u8;
                // Alpha stays as it is, and it's all one value anyway.
                out.push(if channel == PIXEL - 1 { value } else { value & mask });
            }
        }
    }
    (width, height, out)
}

/// Each pixel as its difference from the one to its left, in place.
fn to_deltas(bgra: &mut [u8], width: u32) {
    for row in bgra.chunks_mut(width as usize * PIXEL) {
        for at in (PIXEL..row.len()).rev() {
            row[at] = row[at].wrapping_sub(row[at - PIXEL]);
        }
    }
}

fn from_deltas(bgra: &mut [u8], width: u32) {
    for row in bgra.chunks_mut(width as usize * PIXEL) {
        for at in PIXEL..row.len() {
            row[at] = row[at].wrapping_add(row[at - PIXEL]);
        }
    }
}

/// **Holds a picture to [`MAX_PACKED_PICTURE`].** As it is, if that fits, which is any page that's mostly paper. Otherwise at half the resolution or less, with fewer colours and each row as differences: what lets a photograph fit, at the cost of looking soft for the moment it's there. `None` if its numbers don't add up, or nothing fits.
fn pack_picture(picture: &SleepPicture) -> Option<(PictureLine, Vec<u8>)> {
    if picture_bytes(picture.width, picture.height, picture.scale) != Some(picture.bgra.len()) {
        return None;
    }
    let line = |width, height, scale, packed: &[u8], delta| PictureLine {
        page: picture.page,
        scale,
        width,
        height,
        dark: picture.dark,
        packed: u32::try_from(packed.len()).unwrap_or(u32::MAX),
        delta,
    };
    let whole = deflated(&picture.bgra, flate2::Compression::fast())?;
    if whole.len() <= MAX_PACKED_PICTURE {
        return Some((line(picture.width, picture.height, picture.scale, &whole, false), whole));
    }
    for (step, keep_bits) in [(2u32, 5u32), (3, 5), (4, 4), (6, 4), (8, 4)] {
        let (width, height, mut bgra) = shrunk(picture, step, keep_bits);
        to_deltas(&mut bgra, width);
        let packed = deflated(&bgra, flate2::Compression::new(6))?;
        if packed.len() <= MAX_PACKED_PICTURE {
            return Some((line(width, height, picture.scale / step as f32, &packed, true), packed));
        }
    }
    None
}

/// A file's bytes. Pictures past the caps, or of the wrong size, are left out rather than failing the file.
pub fn encode_sleep(sleep: &SleepFile) -> Vec<u8> {
    let toc = sleep
        .toc
        .entries()
        .iter()
        .map(|entry| (sleep.toc.title(entry).to_string(), entry.page, entry.parent, entry.depth))
        .collect();
    let mut lines = Vec::new();
    let mut packed = Vec::new();
    for picture in sleep.pictures.iter().take(MAX_SLEEP_PICTURES) {
        if let Some((line, bytes)) = pack_picture(picture) {
            lines.push(line);
            packed.extend_from_slice(&bytes);
        }
    }
    let sizes = sleep.sizes.iter().take(MAX_SIZES).copied().collect();
    let labels = sleep
        .toc
        .labels
        .ranges()
        .iter()
        .map(|range| (range.start, range.first, range.style.code(), range.prefix.to_string()))
        .collect();
    let meta = Meta {
        file: sleep.file.into(),
        file_pages: sleep.file_pages,
        pages: sleep.pages,
        list_seq: sleep.list_seq,
        renderer_only: sleep.renderer_only,
        version: sleep.version,
        truncated: sleep.toc.truncated,
        toc,
        labels,
        labels_truncated: sleep.toc.labels.truncated,
        usual: sleep.usual,
        sizes,
        pictures: lines,
    };
    let mut body = serde_json::to_vec(&meta).unwrap_or_default();
    body.push(b'\n');
    body.extend_from_slice(&packed);
    seal(SLEEP_KIND, 0, &body)
}

/// Reads a file's bytes, believing none of them.
pub fn decode_sleep(bytes: &[u8], newest: u32) -> Result<SleepFile, RecordError> {
    let bad = RecordError::Damaged;
    if bytes.len() > MAX_SLEEP_FILE_BYTES {
        return Err(bad("too large"));
    }
    let (_, _, body) = open_sealed(SLEEP_KIND, bytes, newest)?;
    let end = body.iter().position(|byte| *byte == b'\n').ok_or(bad("no first line"))?;
    let meta: Meta = serde_json::from_slice(&body[..end]).map_err(|_| bad("a first line that doesn't read"))?;
    if meta.file_pages as usize > crate::edit::MAX_PAGES || meta.pages as usize > crate::edit::MAX_PAGES || meta.pages == 0 || meta.file_pages == 0 {
        return Err(bad("too many pages"));
    }
    if meta.toc.len() > MAX_TOC_ENTRIES || meta.sizes.len() > MAX_SIZES || meta.pictures.len() > MAX_SLEEP_PICTURES || meta.labels.len() > MAX_LABEL_RANGES {
        return Err(bad("too long a list"));
    }
    let lines = meta.toc.iter().enumerate().map(|(at, (title, page, parent, depth))| {
        let fits = (*page == NONE || *page < meta.file_pages) && (*parent == NONE || (*parent as usize) < at) && *depth <= MAX_TOC_DEPTH;
        fits.then_some(TocLine {
            title: title.as_str(),
            page: *page,
            parent: *parent,
            depth: *depth,
        })
    });
    let lines: Vec<TocLine<'_>> = lines.collect::<Option<_>>().ok_or(bad("contents out of range"))?;
    let mut toc = Toc::from_lines(&lines, meta.truncated);
    if meta
        .labels
        .iter()
        .any(|(start, _, _, prefix)| *start >= meta.file_pages || prefix.len() > MAX_LABEL_PREFIX_BYTES)
    {
        return Err(bad("page labels out of range"));
    }
    // Held to the same rules as a PDF's own, as whatever wrote this can't be believed.
    toc.labels = PageLabels::from_ranges(
        meta.labels
            .iter()
            .map(|(start, first, style, prefix)| LabelRange {
                start: *start,
                first: *first,
                style: LabelStyle::from_code(*style),
                prefix: prefix.as_str().into(),
            })
            .collect(),
        meta.labels_truncated,
    );
    if meta.usual.is_some_and(|(width, height)| !good_size(width, height)) {
        return Err(bad("a page of an impossible size"));
    }
    if meta
        .sizes
        .iter()
        .any(|(page, width, height)| *page >= meta.pages || !good_size(*width, *height))
    {
        return Err(bad("a page of an impossible size"));
    }
    let mut packed = &body[end + 1..];
    let mut pictures = Vec::with_capacity(meta.pictures.len());
    for line in &meta.pictures {
        let unpacked = picture_bytes(line.width, line.height, line.scale).ok_or(bad("a picture of an impossible size"))?;
        if line.page >= meta.pages || line.packed as usize > packed.len() {
            return Err(bad("a picture out of range"));
        }
        let (these, rest) = packed.split_at(line.packed as usize);
        packed = rest;
        let mut bgra = vec![0u8; unpacked];
        // Exactly that many, and there's to be no more.
        let mut inflate = flate2::read::DeflateDecoder::new(these);
        inflate.read_exact(&mut bgra).map_err(|_| bad("a picture that doesn't unpack"))?;
        if inflate.read(&mut [0u8; 1]).map_err(|_| bad("a picture that doesn't unpack"))? != 0 {
            return Err(bad("a picture that's too long"));
        }
        if line.delta {
            from_deltas(&mut bgra, line.width);
        }
        pictures.push(SleepPicture {
            page: line.page,
            scale: line.scale,
            width: line.width,
            height: line.height,
            dark: line.dark,
            bgra,
        });
    }
    if !packed.is_empty() {
        return Err(bad("something after the pictures"));
    }
    Ok(SleepFile {
        file: meta.file.into(),
        file_pages: meta.file_pages,
        pages: meta.pages,
        list_seq: meta.list_seq,
        renderer_only: meta.renderer_only,
        version: meta.version,
        toc,
        usual: meta.usual,
        sizes: meta.sizes,
        pictures,
    })
}

use super::*;

impl<L: DocLoader> Session<L> {
    /// Somebody's using this tab, so it's not to go dormant for a while. Agents' commands say so too.
    pub fn touch(&mut self, id: TabId) {
        let now = self.dormancy.seconds_now();
        self.tabs.touch_at(id, now);
    }

    /// The tabs that could go dormant, as they hold something: not the one on screen, and not one a save's under way on. Next to none however many tabs there are: the warm ones, and those with a list in memory.
    pub(crate) fn dormant_candidates(&self) -> impl Iterator<Item = TabId> + '_ {
        self.warm
            .ids()
            .chain(self.lists.ids())
            .filter(move |id| !self.on_screen(*id) && !self.dormancy.is_held(*id) && self.lists.get(*id).is_none_or(|list| list.saving().is_none()))
    }

    pub fn let_go(&mut self, id: TabId) {
        self.dormancy.let_go(id);
        self.touch(id);
    }

    /// When the next tab comes due to go dormant, in seconds of the session, this long after it was last touched. One deadline for all of them, kept by whoever's waiting anyway. There's no timer per tab.
    pub fn next_dormant_at(&self, after: u32) -> Option<u32> {
        self.dormant_candidates()
            .filter_map(|id| self.tabs.tab(id))
            .map(|tab| tab.touched.saturating_add(after))
            .min()
    }

    /// The tabs due to go dormant now.
    pub fn due_dormant(&self, now: u32, after: u32) -> Vec<TabId> {
        let mut due: Vec<TabId> = self
            .dormant_candidates()
            .filter(|id| self.tabs.tab(*id).is_some_and(|tab| tab.touched.saturating_add(after) <= now))
            .collect();
        due.sort_unstable_by_key(|id| id.0);
        due.dedup();
        due
    }

    /// Every tab that could go dormant, oldest touched first, for when a budget or the system won't wait.
    pub fn dormant_oldest_first(&self) -> Vec<TabId> {
        let mut all: Vec<TabId> = self.dormant_candidates().collect();
        all.sort_unstable_by_key(|id| (self.tabs.tab(*id).map_or(0, |tab| tab.touched), id.0));
        all.dedup();
        all
    }

    /// Whether a tab may go dormant, whatever's asking: not the one on screen, one being worked on, or one being saved.
    pub fn can_go_dormant(&self, id: TabId) -> bool {
        !self.on_screen(id) && !self.dormancy.is_held(id) && self.lists.get(id).is_none_or(|list| list.saving().is_none())
    }

    /// What a dormant tab's journal has to say for its list to go to disk: the whole list, to compact to and compare with. `None` if there's no list, if it's not all in the file, or if nobody keeps journals here.
    pub fn list_to_sleep(&self, id: TabId) -> Option<(String, JournalHeader, record::EditsRecord)> {
        if !self.journals.kept_and_readable() {
            return None;
        }
        let (header, base) = self.checkpoint_if_caught_up(id)?;
        Some((self.journals.name_of(id)?.to_string(), header, base))
    }

    /// **A tab goes dormant, SPEC decision 25: it costs a cold tab's memory from here on.** Its contents go, and its list goes if [`Self::list_to_sleep`] says it can, as all of it's on disk. Said once for each, and read back only if it's touched again. Says whether the list went, or there wasn't one.
    pub fn go_dormant(&mut self, id: TabId) -> bool {
        // A dormant tab holds no list: its generated contents are on disk, and found again when it's shown.
        self.generated.forget(id);
        if !self.can_go_dormant(id) {
            return false;
        }
        self.warm.forget(id);
        self.mail.forget_draws(id);
        self.mail.forget_view_word(id);
        !self.lists.contains(id) || self.put_list_to_sleep(id)
    }

    pub(crate) fn put_list_to_sleep(&mut self, id: TabId) -> bool {
        if self.list_to_sleep(id).is_none() {
            return false;
        }
        self.lists.put_to_sleep(id)
    }

    /// **Brings a tab's list back from its journal, if it's on disk.** Everything about the tab comes here first, so nothing can act on a list it can't see. **If it can't be read just now, it stays asleep, its name kept, to be tried again by whatever's next,** and says false. Nothing goes on that hasn't got a list to go on: a new edit laid on a journal it never read would land on the wrong pages. The tab says so till then. Only a discard or a confirmed save gives up on it.
    pub fn wake_list(&mut self, id: TabId) -> bool {
        let Some(summary) = self.lists.asleep_summary(id) else { return true };
        let restored = self.journals.name_of(id).and_then(|name| self.journals.history()?.read_list(name));
        match restored {
            Some(restored) if restored.edits.summary() == summary || restored.rolled_off => {
                let mut edits = restored.edits;
                edits.set_journal_seq(restored.seq);
                let said_unreadable = self.lists.woken(id, edits);
                if let Some(tab) = self.tabs.tab_mut(id) {
                    if said_unreadable && matches!(tab.file.notice.as_deref(), Some(FileNotice::EditsUnreadable)) {
                        tab.file.notice = None;
                    }
                    if restored.rolled_off {
                        tab.file.notice = Some(Box::new(FileNotice::HistoryRolledOff));
                    }
                }
                // Steps have gone that the journal has, so it has to start again from the list, or it wouldn't read back as this.
                if restored.rolled_off {
                    self.journals.start_fresh(id, restored.seq, None);
                }
                true
            }
            _ => {
                self.lists.wake_failed(id);
                if let Some(tab) = self.tabs.tab_mut(id) {
                    tab.file.notice = Some(Box::new(FileNotice::EditsUnreadable));
                }
                false
            }
        }
    }

    /// A list that couldn't be read is given up on, as what's being done says it's done with: its journal goes.
    pub(crate) fn give_up_on_asleep(&mut self, id: TabId) {
        if self.lists.is_asleep(id) {
            self.lists.forget_asleep(id);
            self.journals.retire(id);
        }
    }

    /// What's asleep, by journal, for reading back: each has to read as what's known of it here.
    pub fn asleep_by_journal(&self) -> HashMap<String, EditsSummary> {
        self.lists
            .asleep()
            .filter_map(|(id, summary)| Some((self.journals.name_of(*id)?.to_string(), *summary)))
            .collect()
    }

    /// What a tab's sleep file would say of it, apart from what the page view knows, if it can have one: its file's been read this session, so its contents are here, and it isn't protected, as this would be those in the clear.
    pub fn sleep_facts(&self, id: TabId) -> Option<SleepFacts> {
        let toc = self.warm.toc(id)?;
        self.sleep_facts_with(id, toc)
    }

    pub(crate) fn sleep_facts_with(&self, id: TabId, toc: &Toc) -> Option<SleepFacts> {
        let tab = self.tabs.tab(id)?;
        let stamp = tab.file.stamp?;
        // What's come from a sleep file and hasn't been checked against the PDF isn't written again as if it had. Nor anything of a file that's not what it was.
        if tab.file.encrypted
            || tab.from_sleep.is_some()
            || self.lists.is_asleep(id)
            || matches!(
                tab.file.notice.as_deref(),
                Some(FileNotice::Changed | FileNotice::Missing | FileNotice::NeedsPassword)
            )
        {
            return None;
        }
        let list = self.lists.get(id);
        let (pages, list_seq) = list.map_or((tab.page_count, 0), |list| (list.len(), list.order_mark()));
        let sleep = sleep::SleepFile {
            file: stamp,
            file_pages: tab.page_count,
            pages,
            list_seq,
            renderer_only: tab.renderer_only,
            version: None,
            toc: toc.clone(),
            usual: None,
            sizes: Vec::new(),
            pictures: Vec::new(),
        };
        // A page from a protected file on any page means pictures of none, and it's the same for each of them.
        Some(SleepFacts {
            id,
            path: self.tabs.path_of(id)?,
            sleep,
            pictures_allowed: !list.is_some_and(PageEdits::has_protected_pages),
        })
    }

    /// The same for a tab that's just been woken some other way, for a picture for its first frame.
    pub fn sleep_to_read_awake(&self, id: TabId) -> Option<(FileStamp, PathBuf)> {
        let tab = self.tabs.tab(id)?;
        if !self.dormancy.sleep_files() || tab.file.encrypted {
            return None;
        }
        Some((tab.file.stamp?, self.tabs.path_of(id)?))
    }

    /// What's on hand to wake a cold tab from its sleep file: which file, as far as anything's known, without looking at it. `None` for a tab that isn't cold.
    pub fn sleep_to_read(&self, id: TabId) -> Option<(FileStamp, PathBuf)> {
        if self.warm.contains(id) {
            return None;
        }
        let tab = self.tabs.tab(id)?;
        if tab.file.encrypted || matches!(tab.file.notice.as_deref(), Some(FileNotice::NeedsPassword)) {
            return None;
        }
        Some((tab.file.stamp?, self.tabs.path_of(id)?))
    }

    /// **Wakes a cold tab from its sleep file, if it has one about its file as last known.** Says whether it did. Every route to a cold tab's through here, or through a read of its PDF if not.
    pub fn wake_from_sleep_file(&mut self, id: TabId) -> bool {
        if !self.dormancy.sleep_files() {
            return false;
        }
        let Some((stamp, path)) = self.sleep_to_read(id) else { return false };
        let Some(sleep) = self.journals.history().and_then(|source| source.read_sleep(stamp, &path)) else {
            return false;
        };
        self.wake_from_sleep(id, &sleep);
        self.dormancy.woke_from_file(id, sleep);
        true
    }

    pub(crate) fn wake_from_sleep(&mut self, id: TabId, sleep: &sleep::SleepFile) {
        let facts = DocFacts {
            page_count: sleep.file_pages,
            byte_len: sleep.file.size,
            encrypted: false,
            version: "".into(),
            title: None,
            toc: sleep.toc.clone(),
            renderer_only: sleep.renderer_only,
        };
        self.give_woken(id, Ok(facts), Some(sleep.file));
        if let Some(tab) = self.tabs.tab_mut(id) {
            tab.from_sleep = Some(sleep.version.unwrap_or(0));
        }
    }

    /// **The worker has the file open, and says how long it is.** A sleep file's word goes only until now. If it's said something else, it's gone, and the file's read: says so, for that to be done. A list's own length is compared with what that read finds, on its way in. A file read in the usual way isn't looked at, as the two readers of a damaged PDF can differ, and neither is wrong.
    pub fn file_length_is(&mut self, id: TabId, pages: u32, version: u64) -> bool {
        let Some(tab) = self.tabs.tab_mut(id) else { return false };
        let Some(said) = tab.from_sleep.take() else { return false };
        // The same length and, if it said, the same version: it was right. A sleep file that didn't say is right about the length only, which is all it's been used for, its pictures having gone if they weren't.
        if tab.page_count == pages && (said == 0 || said == version) {
            return false;
        }
        self.retire_sleep_file(id);
        self.forget_read(id);
        true
    }

    /// What was read of a tab goes, so it's read again: its file's turned out to be something else.
    pub fn forget_read(&mut self, id: TabId) {
        self.generated.forget(id);
        if let Some(tab) = self.tabs.tab_mut(id) {
            tab.from_sleep = None;
        }
        self.warm.forget(id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::record::CURRENT_VERSION;

    fn sample() -> SleepFile {
        let toc = Toc::from_lines(
            &[
                TocLine {
                    title: "One",
                    page: 0,
                    parent: NONE,
                    depth: 0,
                },
                TocLine {
                    title: "Inside",
                    page: 2,
                    parent: 0,
                    depth: 1,
                },
            ],
            false,
        );
        SleepFile {
            file: FileStamp {
                size: 9,
                mtime_ns: 5,
                device: 1,
                inode: 2,
            },
            file_pages: 5,
            pages: 4,
            list_seq: 7,
            renderer_only: false,
            version: Some(3),
            toc,
            usual: Some((612.0, 792.0)),
            sizes: vec![(3, 300.0, 400.0)],
            pictures: vec![
                SleepPicture {
                    page: 1,
                    scale: 0.5,
                    width: 4,
                    height: 2,
                    dark: true,
                    bgra: (0..32).collect(),
                },
                SleepPicture {
                    page: 2,
                    scale: 0.5,
                    width: 1,
                    height: 1,
                    dark: false,
                    bgra: vec![1, 2, 3, 255],
                },
            ],
        }
    }

    /// Failure: what goes in isn't what comes out, so a tab wakes to the wrong contents, or a picture of the wrong size.
    #[test]
    fn a_sleep_file_reads_back_as_it_was() {
        let sleep = sample();
        assert_eq!(decode_sleep(&encode_sleep(&sleep), CURRENT_VERSION).unwrap(), sleep);
    }

    /// Failure: a sleep file keeps fewer pictures than a view side by side shows, twelve pages at an ordinary zoom, so a tab or an update's first frame wakes with bare pages; or a file of that many is read as damaged. And more than that is left out when written, never refused when read.
    #[test]
    fn a_sleep_file_keeps_a_side_by_side_views_pictures() {
        let mut sleep = sample();
        sleep.pages = 40;
        sleep.file_pages = 40;
        sleep.pictures = (0..12)
            .map(|page| SleepPicture {
                page,
                scale: 0.25,
                width: 2,
                height: 2,
                dark: false,
                bgra: vec![200; 16],
            })
            .collect();
        let back = decode_sleep(&encode_sleep(&sleep), CURRENT_VERSION).expect("twelve pictures read back");
        assert_eq!(back.pictures.len(), 12);
        sleep.pictures.push(SleepPicture {
            page: 12,
            scale: 0.25,
            width: 2,
            height: 2,
            dark: false,
            bgra: vec![200; 16],
        });
        assert_eq!(
            decode_sleep(&encode_sleep(&sleep), CURRENT_VERSION).unwrap().pictures.len(),
            12,
            "a thirteenth is left out when written"
        );
    }

    /// Failure: a picture whose numbers don't match its bytes gets written, so the file's refused for all of it. It's left out, and the rest stands.
    #[test]
    fn a_picture_of_the_wrong_size_is_left_out() {
        let mut sleep = sample();
        sleep.pictures[0].bgra.pop();
        let back = decode_sleep(&encode_sleep(&sleep), CURRENT_VERSION).unwrap();
        assert_eq!(back.pictures.len(), 1);
        assert_eq!(back.pictures[0].page, 2);
    }

    /// A good sleep file with its first line changed, sealed again as a forger would, and read.
    fn forge(good: &[u8], change: &dyn Fn(&mut serde_json::Value)) -> Result<SleepFile, RecordError> {
        let body = &good[good.iter().position(|byte| *byte == b'\n').unwrap() + 1..];
        let end = body.iter().position(|byte| *byte == b'\n').unwrap();
        let mut meta = serde_json::from_slice::<serde_json::Value>(&body[..end]).unwrap();
        change(&mut meta);
        let mut forged = serde_json::to_vec(&meta).unwrap();
        forged.push(b'\n');
        forged.extend_from_slice(&body[end + 1..]);
        decode_sleep(&seal(SLEEP_KIND, 0, &forged), CURRENT_VERSION)
    }

    /// Failure (hostile input): a forged sleep file with a good checksum gets numbers past what's real into a surface's size, a memory allocation or a page's place, or panics. Every one is refused, and only costs the sleep file. A flipped byte anywhere is caught.
    #[test]
    fn a_forged_file_is_refused() {
        let good = encode_sleep(&sample());
        let body_of = |bytes: &[u8]| bytes[bytes.iter().position(|byte| *byte == b'\n').unwrap() + 1..].to_vec();
        let forged = |change: &dyn Fn(&mut serde_json::Value)| forge(&good, change);
        assert!(forged(&|_| {}).is_ok(), "the forging works");
        type Change = fn(&mut serde_json::Value);
        let changes: [(&str, Change); 15] = [
            ("billions of pages", |meta| meta["pages"] = 4_000_000_000u32.into()),
            ("no pages", |meta| meta["pages"] = 0.into()),
            ("a billion-page file", |meta| meta["file_pages"] = 4_000_000_000u32.into()),
            ("a picture of 4 GiB", |meta| {
                meta["pictures"][0]["width"] = 65_535.into();
                meta["pictures"][0]["height"] = 16_384.into();
            }),
            ("a picture with no width", |meta| meta["pictures"][0]["width"] = 0.into()),
            ("one that overflows", |meta| {
                meta["pictures"][0]["width"] = u32::MAX.into();
                meta["pictures"][0]["height"] = u32::MAX.into();
            }),
            ("a scale of nothing", |meta| meta["pictures"][0]["scale"] = 0.into()),
            ("a huge scale", |meta| meta["pictures"][0]["scale"] = 100_000.into()),
            ("a picture past the end", |meta| meta["pictures"][0]["page"] = 4.into()),
            ("more bytes than there are", |meta| meta["pictures"][1]["packed"] = 100_000.into()),
            ("a larger picture than its bytes", |meta| meta["pictures"][1]["height"] = 2.into()),
            ("a page of a mile", |meta| meta["usual"] = serde_json::json!([1_000_000.0, 1.0])),
            ("a size past the end", |meta| meta["sizes"][0][0] = 4.into()),
            ("contents past the end", |meta| meta["toc"][1][1] = 5.into()),
            ("a parent that comes later", |meta| meta["toc"][0][2] = 1.into()),
        ];
        for (what, change) in changes {
            assert!(forged(&change).is_err(), "{what}");
        }
        // Junk after the pictures, and anything flipped.
        let mut long = body_of(&good);
        long.push(0);
        assert!(decode_sleep(&seal(SLEEP_KIND, 0, &long), CURRENT_VERSION).is_err());
        for at in (0..good.len()).step_by(7) {
            let mut flipped = good.clone();
            flipped[at] ^= 0x40;
            assert!(decode_sleep(&flipped, CURRENT_VERSION).is_err(), "byte {at}");
        }
        assert!(
            matches!(
                decode_sleep(&crate::record::seal_as(SLEEP_KIND, CURRENT_VERSION + 1, 0, &body_of(&good)), CURRENT_VERSION),
                Err(RecordError::Newer(_))
            ),
            "a newer app's is left to it"
        );
    }

    /// Failure (the owner's rule on SSD wear): a page of a photograph writes half a megabyte each time it's left, as it doesn't pack, so a morning's reading is tens of megabytes. Every picture comes in under its cap, whatever's on it, and still shows the page: right shape, right colours on average. A page of paper isn't touched.
    #[test]
    fn a_picture_is_held_to_its_cap_and_still_shows_the_page() {
        // Noise over a gradient, as a photograph is to deflate: nothing repeats.
        let (width, height) = (612u32, 792u32);
        let mut state = 0x1234_5678u32;
        let mut noisy = || {
            state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            (state >> 24) as u8 & 0x1F
        };
        let mut bgra = Vec::with_capacity((width * height * 4) as usize);
        for y in 0..height {
            for x in 0..width {
                bgra.extend_from_slice(&[(x / 4) as u8 / 2 + noisy(), (y / 4) as u8 / 2 + noisy(), 180 + noisy() / 2, 255]);
            }
        }
        let mut sleep = sample();
        sleep.pictures = vec![SleepPicture {
            page: 0,
            scale: 0.5,
            width,
            height,
            dark: false,
            bgra: bgra.clone(),
        }];
        let encoded = encode_sleep(&sleep);
        let back = decode_sleep(&encoded, CURRENT_VERSION).unwrap();
        let picture = &back.pictures[0];
        assert!(encoded.len() < MAX_PACKED_PICTURE + 2048, "{} bytes", encoded.len());
        assert!(picture.width < width, "it's had to shrink");
        assert_eq!(picture.bgra.len(), picture.width as usize * picture.height as usize * 4);
        // The same page in points, to within a pixel of the smaller one.
        let shown = |pixels: u32, scale: f32| f64::from(pixels) / f64::from(scale);
        assert!((shown(picture.width, picture.scale) - shown(width, 0.5)).abs() <= 2.0 * f64::from(1.0 / picture.scale));
        assert!((shown(picture.height, picture.scale) - shown(height, 0.5)).abs() <= 2.0 * f64::from(1.0 / picture.scale));
        // The colours on average, channel by channel, and the gradient's still there.
        let average = |pixels: &[u8], channel: usize| {
            pixels.as_chunks::<4>().0.iter().map(|pixel| u64::from(pixel[channel])).sum::<u64>() as f64 / (pixels.len() / 4) as f64
        };
        for channel in 0..3 {
            assert!((average(&bgra, channel) - average(&picture.bgra, channel)).abs() < 20.0, "channel {channel}");
        }
        assert!(picture.bgra.as_chunks::<4>().0.iter().all(|pixel| pixel[3] == 255));
        let at = |x: u32, y: u32| picture.bgra[((y * picture.width + x) * 4) as usize];
        assert!(i32::from(at(picture.width - 1, 0)) - i32::from(at(0, 0)) > 20, "left to right");
        // A page of plain paper's kept as it is.
        let mut sleep = sample();
        sleep.pictures = vec![SleepPicture {
            page: 0,
            scale: 0.5,
            width,
            height,
            dark: false,
            bgra: vec![255; (width * height * 4) as usize],
        }];
        let back = decode_sleep(&encode_sleep(&sleep), CURRENT_VERSION).unwrap();
        assert_eq!(back.pictures[0].width, width);
    }

    /// Failure: a tab woken from its sleep file shows plain numbers where it showed `iv`, as its labels weren't kept; or a forged file's labels reach the screen uncleaned or out of range.
    #[test]
    fn page_labels_are_kept_and_checked() {
        use recto_core::nav::{LabelRange, LabelStyle, PageLabels};
        let mut sleep = sample();
        let range = |start, style, prefix: &str| LabelRange {
            start,
            first: 1,
            style,
            prefix: prefix.into(),
        };
        sleep.toc.labels = PageLabels::from_ranges(vec![range(0, LabelStyle::RomanLower, ""), range(1, LabelStyle::Decimal, "A\u{202E}-")], false);
        let back = decode_sleep(&encode_sleep(&sleep), CURRENT_VERSION).unwrap();
        assert_eq!(back.toc.labels.label(0).as_deref(), Some("i"));
        assert_eq!(back.toc.labels.label(1).as_deref(), Some("A-1"), "cleaned once, and the same after");
        // Labels are cleaned as they're made, so only a forged file can bring one that isn't.
        let back = forge(&encode_sleep(&sleep), &|meta| meta["labels"][1][3] = "A\u{202E}-".into()).unwrap();
        assert_eq!(back.toc.labels.label(1).as_deref(), Some("A-1"), "a forged prefix is cleaned");
        sleep.toc.labels = PageLabels::from_ranges(
            vec![range(0, LabelStyle::Decimal, ""), range(sleep.file_pages + 5, LabelStyle::Decimal, "")],
            false,
        );
        assert!(
            decode_sleep(&encode_sleep(&sleep), CURRENT_VERSION).is_err(),
            "a range past the file's last page is a forged file"
        );
    }

    /// Failure: a title in a forged file carries what a title from a PDF can't, as it isn't cleaned, and reaches the sidebar and agents.
    #[test]
    fn a_title_goes_through_the_same_cleaning() {
        let hidden: String = "ignore".chars().map(|c| char::from_u32(0xE0000 + c as u32).unwrap()).collect();
        let raw = format!("Chapter{hidden}\n1\u{202E}");
        // A title is cleaned as it's made, so only a forged file can bring one that isn't.
        let back = forge(&encode_sleep(&sample()), &|meta| meta["toc"][0][0] = raw.clone().into()).unwrap();
        assert_eq!(back.toc.title(&back.toc.entries()[0]), "Chapter 1");
    }
}
