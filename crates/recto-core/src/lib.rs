//! Lazy document structure over the lopdf fork's `IndexedReader`.
//!
//! [`inspect`] opens a PDF, reads its facts and closes it again, so a tab holds no handle afterwards. Everything read
//! here comes from an untrusted file: walks are bounded, cycles are refused, and text is cleaned before anyone sees it.

use lopdf::{BytesSource, Dictionary, FileSource, IndexedReader, Object, ObjectId};
use std::cell::Cell;
use std::collections::{HashMap, HashSet};
use std::io::ErrorKind;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

pub mod agreement;
mod effects;
pub mod nav;
pub mod save;
mod save_error;
mod signatures;
pub mod verdict;
pub use effects::{PageEffects, page_effects, page_effects_bytes};

pub use signatures::{
    MAX_FIELD_VISITS, MAX_SIGNATURES, SignatureEntry, SignatureInventory, SignatureState, signatures, signatures_in_bytes, signatures_within,
};

/// Most contents entries kept for one document. A hostile file can't make the contents grow past this.
pub const MAX_TOC_ENTRIES: usize = 100_000;
/// Deepest nesting kept in the contents.
pub const MAX_TOC_DEPTH: u8 = 32;
/// Longest title kept, in bytes.
pub const MAX_TITLE_BYTES: usize = 512;
/// Most name tree nodes visited for one document's whole contents, across all its lookups. A hostile tree multiplied by a long outline mustn't be able to hold `open` for hours.
const MAX_NAME_TREE_VISITS: usize = 65_536;
/// How long reading one document's contents may take. It runs in the session's own process, so it has to end by itself. What's been read by then is kept, and `Toc::truncated` says so.
const MAX_CONTENTS_TIME: Duration = Duration::from_secs(5);

/// Marks a contents entry with no page or no parent.
pub const NONE: u32 = u32::MAX;

/// Why a PDF couldn't be opened. Wording is built here, never from the OS's text.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OpenError {
    NotFound,
    NotAllowed,
    PasswordNeeded,
    NotPdf,
    /// The structure reader stopped at one of its limits, as on a file with 100,000 revisions or a page tree hundreds deep: built to cost far more
    /// than any real document. The renderer is still asked, so it opens to be read with Save off; this is the answer when the renderer can't read
    /// it either.
    TooComplex,
}

impl OpenError {
    /// A stable code for agents and tests.
    pub fn code(self) -> &'static str {
        match self {
            Self::NotFound => "not_found",
            Self::NotAllowed => "not_allowed",
            Self::PasswordNeeded => "password_needed",
            Self::NotPdf => "not_pdf",
            Self::TooComplex => "too_complex",
        }
    }

    /// A plain sentence for the person using the app.
    pub fn message(self) -> &'static str {
        match self {
            Self::NotFound => "This file couldn't be found.",
            Self::NotAllowed => "RectoPDF isn't allowed to read this file.",
            Self::PasswordNeeded => "This PDF needs a password.",
            Self::NotPdf => "This file couldn't be read as a PDF.",
            Self::TooComplex => "This file goes far past what any real PDF needs, as damaged and hostile files do, and it couldn't be read.",
        }
    }
}

/// One contents entry. Titles live in one shared string, so a long contents costs little.
// size: asserted <= 32 by `toc_entry_stays_small`. Held per entry, warm tabs only.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TocEntry {
    title_start: u32,
    title_len: u32,
    /// Zero-based page, or [`NONE`].
    pub page: u32,
    /// Index of the parent entry, or [`NONE`].
    pub parent: u32,
    pub depth: u8,
}

/// A document's contents, compact and in document order.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Toc {
    titles: Box<str>,
    entries: Box<[TocEntry]>,
    /// Reading stopped at a limit, so there may be more than these.
    pub truncated: bool,
    /// What the file calls its pages, such as `iv`, which travels with its contents: both are how a warm tab finds its way, and neither is read for a cold one. Empty for most files.
    pub labels: nav::PageLabels,
}

/// One line of contents, for putting a [`Toc`] together from what was kept of one.
#[derive(Clone, Copy, Debug)]
pub struct TocLine<'a> {
    pub title: &'a str,
    pub page: u32,
    pub parent: u32,
    pub depth: u8,
}

impl Toc {
    /// Contents from what's been kept of them. Titles go through the same cleaning as those from a PDF, as whatever kept them can't be believed.
    pub fn from_lines(lines: &[TocLine<'_>], truncated: bool) -> Self {
        let mut titles = String::new();
        let mut entries = Vec::with_capacity(lines.len());
        for line in lines {
            let start = u32::try_from(titles.len()).unwrap_or(u32::MAX);
            titles.push_str(&clean_line(line.title));
            let end = u32::try_from(titles.len()).unwrap_or(u32::MAX);
            entries.push(TocEntry {
                title_start: start,
                title_len: end - start,
                page: line.page,
                parent: line.parent,
                depth: line.depth,
            });
        }
        Self {
            titles: titles.into_boxed_str(),
            entries: entries.into_boxed_slice(),
            truncated,
            labels: nav::PageLabels::default(),
        }
    }

    pub fn entries(&self) -> &[TocEntry] {
        &self.entries
    }

    pub fn title(&self, entry: &TocEntry) -> &str {
        let start = entry.title_start as usize;
        self.titles.get(start..start + entry.title_len as usize).unwrap_or("")
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// The last entry at or before a zero-based page, which is the section being read.
    pub fn section_at(&self, page: u32) -> Option<usize> {
        self.entries.iter().rposition(|e| e.page != NONE && e.page <= page)
    }
}

/// What a document is, read without loading it.
#[derive(Clone, Debug)]
pub struct DocFacts {
    pub page_count: u32,
    pub byte_len: u64,
    pub encrypted: bool,
    pub version: Box<str>,
    /// The PDF's own title, cleaned. It comes from the file, so treat it as untrusted.
    pub title: Option<Box<str>>,
    pub toc: Toc,
    /// The structure reader refused this file, and only the renderer could read it. The page count is the renderer's, and there's no title, contents or signature report to give.
    /// Such a tab can be read and drawn, but its pages can't be edited: only the structure reader's writer may change a PDF.
    pub renderer_only: bool,
}

/// Opens a file for the lazy reader. Only a plain file: opening a pipe would wait for a writer for ever, and a device could be anything.
fn open_reader(path: &Path) -> Result<IndexedReader, OpenError> {
    let meta = std::fs::metadata(path).map_err(|e| match e.kind() {
        ErrorKind::NotFound => OpenError::NotFound,
        ErrorKind::PermissionDenied => OpenError::NotAllowed,
        _ => OpenError::NotPdf,
    })?;
    if !meta.is_file() {
        return Err(OpenError::NotPdf);
    }
    let source = FileSource::open(path).map_err(|_| OpenError::NotAllowed)?;
    checked_reader(IndexedReader::open(source))
}

/// [`open_reader`] with a small cache of objects and object streams, for one read that resolves many objects packed in the same streams, as a page of links does: without it each is a fresh inflate of its stream. It lives as long as the reader, which is one read.
pub(crate) fn open_reader_cached(path: &Path) -> Result<IndexedReader, OpenError> {
    let meta = std::fs::metadata(path).map_err(|e| match e.kind() {
        ErrorKind::NotFound => OpenError::NotFound,
        ErrorKind::PermissionDenied => OpenError::NotAllowed,
        _ => OpenError::NotPdf,
    })?;
    if !meta.is_file() {
        return Err(OpenError::NotPdf);
    }
    let source = FileSource::open(path).map_err(|_| OpenError::NotAllowed)?;
    checked_reader(IndexedReader::open_cached(
        source,
        lopdf::IndexedReaderOptions::default(),
        lopdf::IndexedReaderCacheOptions::new(READ_CACHE_BYTES, READ_CACHE_ENTRIES),
    ))
}

/// What [`open_reader_cached`] may keep: enough for a page's annotations and their streams, and gone with the read.
pub(crate) const READ_CACHE_BYTES: u64 = 4 * 1024 * 1024;
pub(crate) const READ_CACHE_ENTRIES: usize = 256;

/// What every way of opening shares: a file that needs a password is refused here, whichever bytes it came from.
pub(crate) fn checked_reader(opened: Result<IndexedReader, lopdf::IndexedReaderError>) -> Result<IndexedReader, OpenError> {
    // A file that needs a password the reader wasn't given, or was given wrongly, says so: it's a PDF, and only the person can open it.
    let reader = opened.map_err(|error| match error {
        lopdf::IndexedReaderError::PasswordRequired | lopdf::IndexedReaderError::InvalidPassword => OpenError::PasswordNeeded,
        other => agreement::refusal_of(&other),
    })?;
    if reader.is_encrypted() && !reader.is_authenticated() {
        return Err(OpenError::PasswordNeeded);
    }
    Ok(reader)
}

/// Opens bytes already in memory, the same way a file is opened.
fn open_bytes(bytes: Arc<[u8]>) -> Result<IndexedReader, OpenError> {
    checked_reader(IndexedReader::open(BytesSource::new(bytes)))
}

pub use recto_platform::{MAX_PASSWORD_BYTES, Password};

/// [`inspect`] with a password the person typed. A wrong one is [`OpenError::PasswordNeeded`], as no password is. The file's closed again when this returns, and the password isn't kept here.
pub fn inspect_with_password(path: &Path, password: &Password) -> Result<DocFacts, OpenError> {
    let meta = std::fs::metadata(path).map_err(|e| match e.kind() {
        ErrorKind::NotFound => OpenError::NotFound,
        ErrorKind::PermissionDenied => OpenError::NotAllowed,
        _ => OpenError::NotPdf,
    })?;
    if !meta.is_file() {
        return Err(OpenError::NotPdf);
    }
    let source = FileSource::open(path).map_err(|_| OpenError::NotAllowed)?;
    // lopdf takes this copy out of the options and drops it once it's derived the file's key, without overwriting it: a copy that remains, listed in `docs/engineering/opening.md`. The options are moved in, so there's nothing left here to clear.
    let options = lopdf::IndexedReaderOptions {
        password: Some(password.bytes().to_vec()),
        ..lopdf::IndexedReaderOptions::default()
    };
    read_facts(&checked_reader(IndexedReader::open_with_options(source, options))?)
}

/// Opens a PDF lazily, reads its facts and closes it. No file handle stays open after this returns.
pub fn inspect(path: &Path) -> Result<DocFacts, OpenError> {
    read_facts(&open_reader(path)?)
}

/// [`inspect`] on bytes in memory. The same walk, so a fuzzer tests the real one, thousands of times a second and without writing a file for each.
pub fn inspect_bytes(bytes: Arc<[u8]>) -> Result<DocFacts, OpenError> {
    read_facts(&open_bytes(bytes)?)
}

fn read_facts(reader: &IndexedReader) -> Result<DocFacts, OpenError> {
    let page_count = u32::try_from(reader.page_count().map_err(|error| agreement::refusal_of(&error))?).map_err(|_| OpenError::NotPdf)?;
    // A damaged outline shouldn't stop a readable PDF from opening, nor damaged labels.
    let mut toc = read_toc(reader).unwrap_or_default();
    toc.labels = nav::read_labels(reader, page_count);
    Ok(DocFacts {
        page_count,
        byte_len: reader.source_len(),
        encrypted: reader.is_encrypted(),
        version: reader.version().into(),
        title: read_title(reader),
        toc,
        renderer_only: false,
    })
}

/// The bit of an encryption dictionary's `/P` that allows copying text and graphics out (PDF 32000-1, table 22, bit 5): what PDFium's `FPDF_GetDocPermissions` reads.
const PERMISSION_COPY: i64 = 1 << 4;

/// **Whether a file asks that it isn't copied**, read straight from its encryption dictionary's `/P`, a document-wide flag: no page is read, so no worker and no characters. A file with no encryption asks nothing. The flag is advice the file gives, which RectoPDF honours with a notice and "anyway" (owner, 2026-11-08). An error means the structure reader couldn't open the file; the caller then asks the renderer.
pub fn copy_restricted(path: &Path) -> Result<bool, OpenError> {
    let reader = open_reader(path)?;
    Ok(restricted_by(&reader))
}

/// [`copy_restricted`] on bytes in memory: the same read, for the fuzzer.
pub fn copy_restricted_bytes(bytes: Arc<[u8]>) -> Result<bool, OpenError> {
    Ok(restricted_by(&open_bytes(bytes)?))
}

fn restricted_by(reader: &IndexedReader) -> bool {
    if !reader.is_encrypted() {
        return false;
    }
    // `/P` is a plain integer, never encrypted. One that can't be read is taken as allowing everything, as PDFium does with a dictionary it can't read: the file still opened.
    let permissions = reader.trailer_entry_owned(b"Encrypt").ok().flatten().and_then(|encrypt| {
        encrypt
            .as_dict()
            .ok()
            .and_then(|dict| dict.get(b"P").ok().and_then(|value| value.as_i64().ok()))
    });
    permissions.is_some_and(|permissions| permissions & PERMISSION_COPY == 0)
}

/// A document's title, as it says, cleaned. `None` if it has none or can't be read.
pub fn document_title(path: &Path) -> Option<Box<str>> {
    read_title(&open_reader(path).ok()?)
}

fn read_title(reader: &IndexedReader) -> Option<Box<str>> {
    let info = reader.trailer_entry_owned(b"Info").ok()??;
    let title = deref(reader, info.as_dict().ok()?.get(b"Title").ok()?)?;
    let text = clean_text(title.as_str().ok()?);
    (!text.is_empty()).then(|| text.into_boxed_str())
}

/// Follows a reference, if this is one.
fn deref(reader: &IndexedReader, object: &Object) -> Option<Object> {
    match object {
        Object::Reference(id) => reader.resolve_object(*id).ok(),
        other => Some(other.clone()),
    }
}

fn dict_of(reader: &IndexedReader, id: ObjectId) -> Option<Dictionary> {
    match reader.resolve_object(id).ok()? {
        Object::Dictionary(dict) => Some(dict),
        _ => None,
    }
}

fn read_toc(reader: &IndexedReader) -> Option<Toc> {
    read_toc_within(reader, MAX_CONTENTS_TIME)
}

fn read_toc_within(reader: &IndexedReader, limit: Duration) -> Option<Toc> {
    let deadline = Instant::now() + limit;
    let walk = NameWalk {
        visits_left: Cell::new(MAX_NAME_TREE_VISITS),
        cut_short: Cell::new(false),
        deadline,
    };
    let root = reader.trailer_entry_owned(b"Root").ok()??;
    let root = root.as_dict().ok()?;
    let outlines = deref(reader, root.get(b"Outlines").ok()?)?;
    let first = outlines.as_dict().ok()?.get(b"First").ok()?.as_reference().ok()?;

    let pages: HashMap<ObjectId, u32> = reader
        .page_map()
        .ok()?
        .iter()
        .enumerate()
        .filter_map(|(index, page)| Some((page.id(), u32::try_from(index).ok()?)))
        .collect();

    let mut titles = String::new();
    let mut entries = Vec::new();
    let mut seen = HashSet::new();
    // An explicit stack, so a deep file can't overflow the thread's. Popping the first child before the next sibling
    // keeps entries in document order.
    let mut stack = vec![(first, NONE, 0u8)];
    let mut truncated = false;
    while let Some((id, parent, depth)) = stack.pop() {
        if entries.len() >= MAX_TOC_ENTRIES || Instant::now() >= deadline {
            truncated = true;
            break;
        }
        // A file can loop its outline back on itself.
        if !seen.insert(id) {
            continue;
        }
        let Some(node) = dict_of(reader, id) else { continue };
        let title = node
            .get(b"Title")
            .ok()
            .and_then(|t| deref(reader, t))
            .and_then(|t| t.as_str().ok().map(clean_text))
            .unwrap_or_default();
        let (Ok(title_start), Ok(title_len)) = (u32::try_from(titles.len()), u32::try_from(title.len())) else {
            break;
        };
        titles.push_str(&title);
        let index = u32::try_from(entries.len()).ok()?;
        entries.push(TocEntry {
            title_start,
            title_len,
            page: target_page(reader, root, &node, &pages, &walk).unwrap_or(NONE),
            parent,
            depth,
        });
        if let Ok(next) = node.get(b"Next").and_then(Object::as_reference) {
            stack.push((next, parent, depth));
        }
        if depth < MAX_TOC_DEPTH
            && let Ok(child) = node.get(b"First").and_then(Object::as_reference)
        {
            stack.push((child, index, depth + 1));
        }
    }
    Some(Toc {
        titles: titles.into_boxed_str(),
        entries: entries.into_boxed_slice(),
        truncated: truncated || walk.visits_left.get() == 0 || walk.cut_short.get(),
        labels: nav::PageLabels::default(),
    })
}

/// What's left for following named destinations, shared across the whole outline.
struct NameWalk {
    visits_left: Cell<usize>,
    /// Children were left out of a walk, because there were more waiting than visits left to reach them.
    cut_short: Cell<bool>,
    deadline: Instant,
}

impl NameWalk {
    fn may_visit(&self) -> bool {
        let left = self.visits_left.get();
        if left == 0 || Instant::now() >= self.deadline {
            return false;
        }
        self.visits_left.set(left - 1);
        true
    }
}

fn target_page(reader: &IndexedReader, root: &Dictionary, node: &Dictionary, pages: &HashMap<ObjectId, u32>, walk: &NameWalk) -> Option<u32> {
    let dest = match node.get(b"Dest") {
        Ok(dest) => deref(reader, dest)?,
        Err(_) => {
            let action = deref(reader, node.get(b"A").ok()?)?;
            let action = action.as_dict().ok()?;
            if action.get(b"S").ok()?.as_name().ok()? != b"GoTo" {
                return None;
            }
            deref(reader, action.get(b"D").ok()?)?
        }
    };
    page_of_dest(reader, root, &dest, pages, walk, 0)
}

fn page_of_dest(reader: &IndexedReader, root: &Dictionary, dest: &Object, pages: &HashMap<ObjectId, u32>, walk: &NameWalk, hops: u8) -> Option<u32> {
    // A named destination can name another, and a file can make that a loop.
    if hops > 4 {
        return None;
    }
    match dest {
        Object::Array(items) => pages.get(&items.first()?.as_reference().ok()?).copied(),
        Object::Name(name) => {
            let dests = deref(reader, root.get(b"Dests").ok()?)?;
            let found = deref(reader, dests.as_dict().ok()?.get(name).ok()?)?;
            page_of_dest(reader, root, &found, pages, walk, hops + 1)
        }
        Object::String(name, _) => {
            let names = deref(reader, root.get(b"Names").ok()?)?;
            let tree = deref(reader, names.as_dict().ok()?.get(b"Dests").ok()?)?;
            let found = find_in_name_tree(reader, &tree, name, walk)?;
            page_of_dest(reader, root, &found, pages, walk, hops + 1)
        }
        Object::Dictionary(dict) => {
            let inner = deref(reader, dict.get(b"D").ok()?)?;
            page_of_dest(reader, root, &inner, pages, walk, hops + 1)
        }
        _ => None,
    }
}

fn find_in_name_tree(reader: &IndexedReader, root: &Object, wanted: &[u8], walk: &NameWalk) -> Option<Object> {
    // Children wait here unresolved, so a wide node costs its array and no reads until they're each reached. Never more wait than can still be visited, and a node is visited once, because a tree that lists itself otherwise adds its whole width on every visit: 394 KB of file held past 1 GiB within the 5 s limit.
    let mut stack = vec![root.clone()];
    let mut seen: HashSet<ObjectId> = HashSet::new();
    while let Some(node) = stack.pop() {
        if let Object::Reference(id) = &node
            && !seen.insert(*id)
        {
            continue;
        }
        if !walk.may_visit() {
            return None;
        }
        let Some(node) = deref(reader, &node) else { continue };
        let Ok(dict) = node.as_dict() else { continue };
        if let Ok(limits) = dict.get(b"Limits").and_then(Object::as_array)
            && let (Some(Ok(low)), Some(Ok(high))) = (limits.first().map(Object::as_str), limits.get(1).map(Object::as_str))
            && (wanted < low || wanted > high)
        {
            continue;
        }
        if let Ok(pairs) = dict.get(b"Names").and_then(Object::as_array) {
            for [name, value] in pairs.as_chunks::<2>().0 {
                if name.as_str().ok() == Some(wanted) {
                    return deref(reader, value);
                }
            }
        }
        if let Ok(kids) = dict.get(b"Kids").and_then(Object::as_array) {
            let room = walk.visits_left.get().saturating_sub(stack.len());
            walk.cut_short.set(walk.cut_short.get() || kids.len() > room);
            stack.extend(kids.iter().take(room).cloned());
        }
    }
    None
}

/// Decodes a PDF text string and strips what could mislead a reader or an agent. See [`clean_line`].
pub fn clean_text(bytes: &[u8]) -> String {
    let decoded = if let Some(rest) = bytes.strip_prefix(&[0xFE, 0xFF]) {
        let units = rest.as_chunks::<2>().0.iter().map(|pair| u16::from_be_bytes(*pair));
        char::decode_utf16(units).map(|c| c.unwrap_or('\u{FFFD}')).collect()
    } else if let Some(rest) = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]) {
        String::from_utf8_lossy(rest).into_owned()
    } else {
        bytes.iter().map(|&b| pdf_doc_char(b)).collect::<String>()
    };
    clean_line(&decoded)
}

/// Makes any short string from outside safe to show or pass to an agent: titles, contents, and file names. Strips control and invisible characters, and returns one line, capped at
/// [`MAX_TITLE_BYTES`] without splitting a character.
pub fn clean_line(text: &str) -> String {
    let mut out = String::with_capacity(text.len().min(MAX_TITLE_BYTES));
    let mut pending_space = false;
    for c in text.chars() {
        if is_invisible_format(c) {
            continue;
        }
        if c.is_whitespace() || c.is_control() {
            pending_space = !out.is_empty();
            continue;
        }
        let extra = c.len_utf8() + usize::from(pending_space);
        if out.len() + extra > MAX_TITLE_BYTES {
            break;
        }
        if pending_space {
            out.push(' ');
            pending_space = false;
        }
        out.push(c);
    }
    out
}

/// Characters that draw nothing, so text can hide in them or be reordered by them: Unicode's format characters and default-ignorables, including tag characters, bidi controls, variation
/// selectors, annotation marks and fillers, plus blanks that aren't spaces and the noncharacters. The one rule for titles, contents, file names and page text.
pub fn is_invisible_format(c: char) -> bool {
    let code = c as u32;
    // Every plane's last two code points, and the block of noncharacters in the BMP.
    if code & 0xFFFE == 0xFFFE || (0xFDD0..=0xFDEF).contains(&code) {
        return true;
    }
    matches!(c,
        '\u{AD}' | '\u{34F}' | '\u{61C}' | '\u{115F}' | '\u{1160}' | '\u{17B4}' | '\u{17B5}' | '\u{180B}'..='\u{180F}'
        | '\u{200B}'..='\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2060}'..='\u{206F}' | '\u{2800}' | '\u{3164}'
        | '\u{FE00}'..='\u{FE0F}' | '\u{FEFF}' | '\u{FFA0}' | '\u{FFF0}'..='\u{FFFB}'
        | '\u{600}'..='\u{605}' | '\u{6DD}' | '\u{70F}' | '\u{8E2}' | '\u{110BD}' | '\u{110CD}' | '\u{13430}'..='\u{1343F}'
        | '\u{1BCA0}'..='\u{1BCA3}' | '\u{1D159}' | '\u{1D173}'..='\u{1D17A}' | '\u{E0000}'..='\u{E0FFF}')
}

fn pdf_doc_char(b: u8) -> char {
    const HIGH: [char; 32] = [
        '•', '†', '‡', '…', '—', '–', 'ƒ', '⁄', '‹', '›', '−', '‰', '„', '“', '”', '‘', '’', '‚', '™', 'ﬁ', 'ﬂ', 'Ł', 'Œ', 'Š', 'Ÿ', 'Ž', 'ı', 'ł', 'œ', 'š',
        'ž', '\u{FFFD}',
    ];
    match b {
        0x80..=0x9F => HIGH[usize::from(b - 0x80)],
        0xA0 => '€',
        _ => char::from(b),
    }
}

/// The PDFs the integration tests write, for a unit test that needs a seam they can't reach.
#[cfg(test)]
#[path = "../tests/support/mod.rs"]
#[allow(dead_code)]
mod test_support;

#[cfg(test)]
mod tests {
    use super::*;

    /// Failure: a field added carelessly makes every contents entry of every warm tab bigger.
    #[test]
    fn toc_entry_stays_small() {
        assert!(std::mem::size_of::<TocEntry>() <= 32);
    }

    /// Failure: reading contents has no bound on time, so a slow file holds `open` for as long as it likes. It must stop, keep what it has, and say so.
    #[test]
    fn contents_stop_at_their_time_limit_and_say_so() {
        let walk = NameWalk {
            visits_left: Cell::new(3),
            cut_short: Cell::new(false),
            deadline: Instant::now() + Duration::from_secs(60),
        };
        assert!(walk.may_visit() && walk.may_visit() && walk.may_visit());
        assert!(!walk.may_visit(), "out of visits");
        let late = NameWalk {
            visits_left: Cell::new(3),
            cut_short: Cell::new(false),
            deadline: Instant::now(),
        };
        assert!(!late.may_visit(), "out of time");
    }

    /// Failure: a wide name tree under a long outline makes `open` resolve every child for every entry, billions of reads, while the session waits. Each entry spends one visit, so it's the time that ends this, and the contents say they aren't whole. Shown at a limit of 200 ms, as the walk is the same at any, in place of waiting out the 5 s one.
    #[test]
    fn a_hostile_name_tree_under_a_long_outline_ends_soon() {
        use crate::test_support as support;
        // 20,000 entries, each naming a destination that isn't in a tree with one node of 200,000 references to itself and to a leaf.
        let refs = "7 0 R ".repeat(200_000);
        let mut extra = vec![
            "<< /Type /Outlines /First 9 0 R >>".to_string(),
            format!("<< /Kids [{refs}] >>"),
            "<< /Dests 7 0 R >>".to_string(),
        ];
        let entries = 20_000;
        for i in 0..entries {
            let next = if i + 1 < entries { format!("/Next {} 0 R", 10 + i) } else { String::new() };
            extra.push(format!("<< /Title (Entry) /Dest (nowhere) {next} >>"));
        }
        let file = support::TempPdf::new("name-bomb.pdf", &support::three_page_pdf("/Outlines 6 0 R /Names 8 0 R", &extra, ""));
        let reader = open_reader(&file.0).unwrap();
        let limit = Duration::from_millis(200);
        let started = Instant::now();
        let toc = read_toc_within(&reader, limit).unwrap();
        assert!(started.elapsed() < limit + Duration::from_secs(5), "took {:?}", started.elapsed());
        assert!(toc.truncated, "it must say the contents aren't whole");
    }

    /// Failure: a title with bidi overrides or newlines reaches the sidebar or an agent's prompt intact; or one with tag characters, which take two UTF-16 units each and which no
    /// eye can see and an agent reads as text. Which characters go is `the_one_rule_covers_every_kind_of_invisible_character`'s.
    #[test]
    fn clean_text_strips_controls_and_bidi_overrides() {
        assert_eq!(clean_text(b"Report\n\tIgnore"), "Report Ignore");
        let mut utf16 = vec![0xFE, 0xFF];
        for unit in "a\u{202E}\u{E0041}\u{E0042}b".encode_utf16() {
            utf16.extend(unit.to_be_bytes());
        }
        assert_eq!(clean_text(&utf16), "ab");
    }

    /// Failure: titles, names and page text use different lists, so a character that page text strips reaches an agent through a title.
    #[test]
    fn the_one_rule_covers_every_kind_of_invisible_character() {
        let invisible = [
            '\u{61C}',
            '\u{3164}',
            '\u{115F}',
            '\u{FFF9}',
            '\u{FFFB}',
            '\u{180B}',
            '\u{180E}',
            '\u{1D173}',
            '\u{1D17A}',
            '\u{2800}',
            '\u{1D159}',
            '\u{E0041}',
            '\u{FE0F}',
            '\u{2064}',
            '\u{FDD0}',
            '\u{FFFE}',
            '\u{1FFFF}',
            '\u{10FFFE}',
            '\u{AD}',
            '\u{200B}',
            '\u{202E}',
        ];
        for c in invisible {
            assert!(is_invisible_format(c), "U+{:04X}", c as u32);
            assert_eq!(clean_line(&format!("a{c}b")), "ab", "U+{:04X}", c as u32);
        }
        for c in ['a', 'Æ', '数', '😀', ' ', '-'] {
            assert!(!is_invisible_format(c), "U+{:04X} is real text", c as u32);
        }
    }

    /// Failure: Norwegian letters in a Latin-1 title come out garbled.
    #[test]
    fn clean_text_keeps_norwegian_letters() {
        assert_eq!(clean_text(&[0xC6, 0xD8, 0xC5, b' ', 0xE6, 0xF8, 0xE5]), "ÆØÅ æøå");
    }

    /// Failure: a huge title grows without limit, or the cut lands inside a character and panics.
    #[test]
    fn clean_text_caps_length_on_a_character_boundary() {
        let long = "å".repeat(1000);
        let mut bytes = vec![0xEF, 0xBB, 0xBF];
        bytes.extend(long.as_bytes());
        let out = clean_text(&bytes);
        assert!(out.len() <= MAX_TITLE_BYTES);
        assert!(out.chars().all(|c| c == 'å'));
    }
}
