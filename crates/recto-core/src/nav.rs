//! **Links and page labels,** read from the file's own structure through the lazy reader.
//!
//! Everything here comes from a hostile file and goes to a person or an agent, so it's bounded and judged here, never by whoever shows it:
//! - A link goes to a page of this document, to a web address the person is asked about first, or nowhere. `file:`, `javascript:`, launch actions, other files and every other scheme are refused, with what they are named so the person can be told. Nothing a link says is ever run or opened from here.
//! - A web address is printable ASCII, with no spaces, at most [`MAX_URI_BYTES`], and `http`, `https` or `mailto`. Anything else is refused rather than cleaned, since a cleaned address is another address.
//! - Page labels (`/PageLabels`) are a number tree of at most [`MAX_LABEL_RANGES`] ranges, each with a prefix of at most [`MAX_LABEL_PREFIX_BYTES`] cleaned like a title, and a label is at most [`MAX_LABEL_BYTES`] long however large its number.
//!
//! Rectangles are in points on the page as it's shown, from its top left, y down: past the crop, turned by the page's own `/Rotate`, the same frame as a selection's. A file's turn the reader adds on top is the page view's to apply.

use crate::{OpenError, clean_text, deref, open_bytes, open_reader_cached};
use lopdf::{Dictionary, IndexedReader, Object, ObjectId};
use std::cell::Cell;
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Most links kept on one page. A page of a thousand links is already an index; past this the rest aren't followed.
pub const MAX_PAGE_LINKS: usize = 1024;
/// Most annotations looked at on one page, links or not.
const MAX_PAGE_ANNOTS: usize = 8192;
/// Longest web address kept. Longer than any real link, and short enough to show in full when asking.
pub const MAX_URI_BYTES: usize = 2048;
/// Most pages read in one go, which shares one opening of the file and one page map.
pub const MAX_PAGES_PER_READ: usize = 16;
/// Most page-label ranges kept. Real books have a handful.
pub const MAX_LABEL_RANGES: usize = 1024;
/// Longest prefix kept, in bytes.
pub const MAX_LABEL_PREFIX_BYTES: usize = 64;
/// Longest label made, in bytes: a letter label of a huge number would otherwise be that many letters.
pub const MAX_LABEL_BYTES: usize = 96;
/// Number-tree nodes and named destinations looked at in one read, and how long it may take. It runs on a page view's own thread, never the UI thread, but a hostile tree mustn't hold that for long.
const MAX_TREE_VISITS: usize = 4096;
const MAX_READ_TIME: Duration = Duration::from_millis(500);
/// Highest number a label counts from. A real `/St` is small; this keeps arithmetic far from overflow.
const MAX_LABEL_START: u32 = 1_000_000_000;

/// Where a page's shown area is, in its own coordinates, and how far the file turns it: what PDFium's bounding box and rotation say, read from the structure.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Frame {
    /// Quarter turns clockwise, 0 to 3.
    pub turns: u8,
    pub left: f32,
    pub bottom: f32,
    pub right: f32,
    pub top: f32,
}

impl Frame {
    /// The page's size as it's shown, or nothing for a frame with no area.
    pub fn shown_size(self) -> Option<(f32, f32)> {
        let (width, height) = (self.right - self.left, self.top - self.bottom);
        if !(width.is_finite() && height.is_finite()) || width <= 0.0 || height <= 0.0 {
            return None;
        }
        Some(if self.turns & 1 == 1 { (height, width) } else { (width, height) })
    }

    /// A rectangle of the page's own coordinates, as shown: `[x0, y0, x1, y1]` from the top left, y down. Nothing if it isn't a real rectangle.
    pub fn to_shown(self, rect: [f32; 4]) -> Option<[f32; 4]> {
        if !rect.iter().all(|edge| edge.is_finite()) {
            return None;
        }
        let (x0, x1) = (rect[0].min(rect[2]) - self.left, rect[0].max(rect[2]) - self.left);
        let (y0, y1) = (rect[1].min(rect[3]) - self.bottom, rect[1].max(rect[3]) - self.bottom);
        let (width, height) = (self.right - self.left, self.top - self.bottom);
        let shown = match self.turns & 3 {
            0 => [x0, height - y1, x1, height - y0],
            1 => [y0, x0, y1, x1],
            2 => [width - x1, y0, width - x0, y1],
            _ => [height - y1, width - x1, height - y0, width - x0],
        };
        (shown.iter().all(|edge| edge.is_finite() && edge.abs() < 1e7) && shown[2] >= shown[0] && shown[3] >= shown[1]).then_some(shown)
    }

    /// How far down the page, from 0 to 1 as shown, a destination's `top` is. Only for a page shown upright or upside down, where the file's top is the shown top; a quarter-turned page's `top` is a shown left, which says nothing about how far down.
    fn within(self, top: f32) -> Option<f32> {
        let height = self.top - self.bottom;
        if !top.is_finite() || !(height.is_finite() && height > 0.0) {
            return None;
        }
        let from_top = match self.turns & 3 {
            0 => self.top - top,
            2 => top - self.bottom,
            _ => return None,
        };
        Some((from_top / height).clamp(0.0, 1.0))
    }
}

/// What a link does, as Rust judges it.
#[derive(Clone, Debug, PartialEq)]
pub enum LinkTarget {
    /// A page of this document, zero-based, and how far down it from 0 to 1 if the link says.
    Page { page: u32, within: Option<f32> },
    /// A web address or an email address, which is only ever opened by the person, after being asked.
    Web(Box<str>),
    /// Back or forward in the reader's history, as a file's named `GoBack` and `GoForward` say.
    History { back: bool },
    /// Refused, and why, in a word.
    Refused(Refused),
    /// It names a place that isn't in this document, or can't be read.
    Nowhere,
}

/// Why a link isn't followed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refused {
    /// A `file:` address, or a link to another file (`GoToR`, `GoToE`, `Launch` of a file).
    File,
    /// A `javascript:` address or a JavaScript action.
    Script,
    /// A launch action, which runs a program.
    Launch,
    /// An address of any other kind, which could start another app.
    Scheme,
    /// An action RectoPDF doesn't do, such as a form's submit, a sound or a movie.
    Action,
    /// An address that isn't printable ASCII, has spaces, or is too long to show in full.
    BadAddress,
}

impl Refused {
    pub fn code(self) -> &'static str {
        match self {
            Self::File => "file",
            Self::Script => "script",
            Self::Launch => "launch",
            Self::Scheme => "scheme",
            Self::Action => "action",
            Self::BadAddress => "bad_address",
        }
    }

    /// What the person is told, in plain words.
    pub fn message(self) -> &'static str {
        match self {
            Self::File => "This link goes to a file on your Mac or another document. RectoPDF doesn't open those from a PDF.",
            Self::Script => "This link would run a script. RectoPDF never runs scripts from a PDF.",
            Self::Launch => "This link would start a program. RectoPDF never starts programs from a PDF.",
            Self::Scheme => "This link would open another app. RectoPDF only opens web and email links, and asks first.",
            Self::Action => "This link does something RectoPDF doesn't do, so nothing happened.",
            Self::BadAddress => "This link's address can't be shown as it is, so it isn't opened.",
        }
    }
}

/// One link on a page.
// size: asserted <= 48 by `link_stays_small`. Held per link of a page the pointer has been over, at most `MAX_PAGE_LINKS` a page.
#[derive(Clone, Debug, PartialEq)]
pub struct Link {
    /// `[x0, y0, x1, y1]` in points as the page is shown, from its top left.
    pub rect: [f32; 4],
    pub target: LinkTarget,
}

/// A page's links.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PageLinks {
    pub frame: Frame,
    pub links: Vec<Link>,
    /// More were on the page than were kept, or reading stopped at a limit.
    pub truncated: bool,
}

impl PageLinks {
    /// The link at a point on the page as shown, the last drawn first, as the topmost is what a click lands on.
    pub fn at(&self, x: f32, y: f32) -> Option<&Link> {
        if !x.is_finite() || !y.is_finite() {
            return None;
        }
        self.links
            .iter()
            .rev()
            .find(|link| x >= link.rect[0] && x <= link.rect[2] && y >= link.rect[1] && y <= link.rect[3])
    }
}

/// What's left for one read: visits and time, shared by everything it looks at.
struct Budget {
    visits_left: Cell<usize>,
    deadline: Instant,
    cut_short: Cell<bool>,
    /// Name-tree nodes already read in this read, as a page of links into one tree reads the same nodes for each: at most [`MAX_NODES_KEPT`], for as long as the read.
    nodes: std::cell::RefCell<HashMap<ObjectId, std::rc::Rc<Dictionary>>>,
}

/// Name-tree nodes kept for one read. A tree a few levels deep has its path in far fewer.
const MAX_NODES_KEPT: usize = 256;

impl Budget {
    fn new() -> Self {
        Self::lasting(MAX_READ_TIME)
    }

    /// A budget with a time of its own: a test of the caps on counts gives the time room, so a busy machine can't make it a test of the clock.
    fn lasting(time: Duration) -> Self {
        Self {
            visits_left: Cell::new(MAX_TREE_VISITS),
            deadline: Instant::now() + time,
            cut_short: Cell::new(false),
            nodes: std::cell::RefCell::new(HashMap::new()),
        }
    }

    fn may_visit(&self) -> bool {
        let left = self.visits_left.get();
        if left == 0 || Instant::now() >= self.deadline {
            self.cut_short.set(true);
            return false;
        }
        self.visits_left.set(left - 1);
        true
    }
}

/// Reads the links on some pages of a file, zero-based, at most [`MAX_PAGES_PER_READ`] of them, in one opening. A page that isn't there, or can't be read, comes back with no links. The file's closed again when this returns.
pub fn page_links(path: &Path, pages: &[u32]) -> Result<Vec<(u32, PageLinks)>, OpenError> {
    Ok(links_of(&open_reader_cached(path)?, pages))
}

/// [`page_links`] through a file already open, with the password it was opened with if it's protected: the file a page view is drawing, whatever is at its path now.
pub fn page_links_in(file: std::fs::File, password: Option<&[u8]>, pages: &[u32]) -> Result<Vec<(u32, PageLinks)>, OpenError> {
    let source = lopdf::FileSource::from_file(file).map_err(|_| OpenError::NotAllowed)?;
    let options = lopdf::IndexedReaderOptions {
        password: password.map(<[u8]>::to_vec),
        ..lopdf::IndexedReaderOptions::default()
    };
    let reader = crate::checked_reader(IndexedReader::open_cached(
        source,
        options,
        lopdf::IndexedReaderCacheOptions::new(crate::READ_CACHE_BYTES, crate::READ_CACHE_ENTRIES),
    ))?;
    Ok(links_of(&reader, pages))
}

/// [`page_links`] on bytes in memory: the same walk, for the fuzzer.
pub fn page_links_bytes(bytes: Arc<[u8]>, pages: &[u32]) -> Result<Vec<(u32, PageLinks)>, OpenError> {
    Ok(links_of(&open_bytes(bytes)?, pages))
}

fn links_of(reader: &IndexedReader, pages: &[u32]) -> Vec<(u32, PageLinks)> {
    links_within(reader, pages, Budget::new())
}

fn links_within(reader: &IndexedReader, pages: &[u32], budget: Budget) -> Vec<(u32, PageLinks)> {
    let (old_dests, names_tree) = (std::cell::OnceCell::new(), std::cell::OnceCell::new());
    let Ok(map) = reader.page_map() else {
        return pages.iter().take(MAX_PAGES_PER_READ).map(|page| (*page, PageLinks::default())).collect();
    };
    let index: HashMap<ObjectId, u32> = map
        .iter()
        .enumerate()
        .filter_map(|(at, entry)| Some((entry.id(), u32::try_from(at).ok()?)))
        .collect();
    let count = u32::try_from(map.len()).unwrap_or(u32::MAX);
    let root = reader
        .trailer_entry_owned(b"Root")
        .ok()
        .flatten()
        .and_then(|root| root.as_dict().ok().cloned())
        .unwrap_or_default();
    let mut out = Vec::new();
    for &page in pages.iter().take(MAX_PAGES_PER_READ) {
        let Some(entry) = map.get(page as usize) else {
            out.push((page, PageLinks::default()));
            continue;
        };
        let frame = frame_of(reader, entry);
        let mut found = PageLinks {
            frame,
            links: Vec::new(),
            truncated: false,
        };
        let Some(dict) = dict_at(reader, entry.id()) else {
            out.push((page, found));
            continue;
        };
        let annots = match dict.get(b"Annots").ok().and_then(|annots| deref(reader, annots)) {
            Some(Object::Array(items)) => items,
            _ => Vec::new(),
        };
        let context = Context {
            old_dests: &old_dests,
            names_tree: &names_tree,
            reader,
            map: &map,
            root: &root,
            index: &index,
            count,
            page,
            budget: &budget,
        };
        for item in annots.iter().take(MAX_PAGE_ANNOTS) {
            if found.links.len() >= MAX_PAGE_LINKS || Instant::now() >= budget.deadline {
                found.truncated = true;
                break;
            }
            let Some(Object::Dictionary(annot)) = deref(reader, item) else { continue };
            if !matches!(annot.get(b"Subtype"), Ok(Object::Name(kind)) if kind == b"Link") {
                continue;
            }
            let Some(rect) = rect_of(reader, &annot).and_then(|rect| frame.to_shown(rect)) else {
                continue;
            };
            // A link of no area can't be pointed at.
            if rect[2] - rect[0] <= 0.0 || rect[3] - rect[1] <= 0.0 {
                continue;
            }
            found.links.push(Link {
                rect,
                target: context.target(&annot),
            });
        }
        found.truncated |= annots.len() > MAX_PAGE_ANNOTS || budget.cut_short.get();
        out.push((page, found));
    }
    out
}

struct Context<'a> {
    old_dests: &'a std::cell::OnceCell<Option<Dictionary>>,
    names_tree: &'a std::cell::OnceCell<Option<Object>>,
    reader: &'a IndexedReader,
    map: &'a lopdf::PageMap,
    root: &'a Dictionary,
    index: &'a HashMap<ObjectId, u32>,
    count: u32,
    /// The page the link is on, which `NextPage` and the like go from.
    page: u32,
    budget: &'a Budget,
}

impl Context<'_> {
    fn target(&self, annot: &Dictionary) -> LinkTarget {
        if let Ok(dest) = annot.get(b"Dest") {
            return self.dest(dest);
        }
        match annot.get(b"A").ok().and_then(|action| deref(self.reader, action)) {
            Some(Object::Dictionary(action)) => self.action(&action),
            _ => LinkTarget::Nowhere,
        }
    }

    fn action(&self, action: &Dictionary) -> LinkTarget {
        let Ok(Object::Name(kind)) = action.get(b"S") else {
            return LinkTarget::Nowhere;
        };
        match kind.as_slice() {
            b"GoTo" => action.get(b"D").map_or(LinkTarget::Nowhere, |dest| self.dest(dest)),
            b"URI" => match action.get(b"URI").ok().and_then(|uri| deref(self.reader, uri)) {
                Some(Object::String(bytes, _)) => judge_address(&bytes),
                _ => LinkTarget::Nowhere,
            },
            b"Named" => match action.get(b"N") {
                Ok(Object::Name(name)) => self.named(name),
                _ => LinkTarget::Nowhere,
            },
            b"Launch" => LinkTarget::Refused(Refused::Launch),
            b"GoToR" | b"GoToE" | b"GoTo3DView" => LinkTarget::Refused(Refused::File),
            b"JavaScript" => LinkTarget::Refused(Refused::Script),
            _ => LinkTarget::Refused(Refused::Action),
        }
    }

    fn named(&self, name: &[u8]) -> LinkTarget {
        let last = self.count.saturating_sub(1);
        let page = match name {
            b"NextPage" => self.page.saturating_add(1).min(last),
            b"PrevPage" => self.page.saturating_sub(1),
            b"FirstPage" => 0,
            b"LastPage" => last,
            b"GoBack" => return LinkTarget::History { back: true },
            b"GoForward" => return LinkTarget::History { back: false },
            _ => return LinkTarget::Refused(Refused::Action),
        };
        if self.count == 0 {
            LinkTarget::Nowhere
        } else {
            LinkTarget::Page { page, within: None }
        }
    }

    /// A destination: an array, a name in the old `/Dests` dictionary, a string in the `/Names` tree, or a dictionary holding one of those under `/D`.
    fn dest(&self, dest: &Object) -> LinkTarget {
        let mut current = deref(self.reader, dest);
        // A named destination can name another, and a file can make that a loop.
        for _ in 0..4 {
            if !self.budget.may_visit() {
                return LinkTarget::Nowhere;
            }
            current = match current {
                Some(Object::Array(items)) => return self.explicit(&items),
                Some(Object::Dictionary(dict)) => dict.get(b"D").ok().and_then(|inner| deref(self.reader, inner)),
                // The old `/Dests` dictionary and the `/Names` tree's root are read once a read, as a page of links names into them over and over: RFC 9110's `/Dests` is one object of every name.
                Some(Object::Name(name)) => self
                    .old_dests
                    .get_or_init(|| {
                        self.root
                            .get(b"Dests")
                            .ok()
                            .and_then(|dests| deref(self.reader, dests))
                            .and_then(|dests| dests.as_dict().ok().cloned())
                    })
                    .as_ref()
                    .and_then(|dests| dests.get(&name).ok())
                    .and_then(|found| deref(self.reader, found)),
                Some(Object::String(name, _)) => {
                    let tree = self.names_tree.get_or_init(|| {
                        self.root.get(b"Names").ok().and_then(|names| deref(self.reader, names)).and_then(|names| {
                            names
                                .as_dict()
                                .ok()
                                .and_then(|names| names.get(b"Dests").ok())
                                .and_then(|tree| deref(self.reader, tree))
                        })
                    });
                    tree.as_ref().and_then(|tree| find_named(self.reader, tree, &name, self.budget))
                }
                _ => return LinkTarget::Nowhere,
            };
        }
        LinkTarget::Nowhere
    }

    /// `[page /XYZ left top zoom]` and the rest. The page is a reference to a page of this document, or, as some files write it, its number.
    fn explicit(&self, items: &[Object]) -> LinkTarget {
        let page = match items.first() {
            Some(Object::Reference(id)) => self.index.get(id).copied(),
            Some(Object::Integer(number)) => u32::try_from(*number).ok().filter(|page| *page < self.count),
            _ => None,
        };
        let Some(page) = page else { return LinkTarget::Nowhere };
        let number = |at: usize| match items.get(at) {
            Some(Object::Integer(value)) => Some(*value as f32),
            Some(Object::Real(value)) => Some(*value),
            _ => None,
        };
        let top = match items.get(1) {
            Some(Object::Name(fit)) => match fit.as_slice() {
                b"XYZ" => number(3),
                b"FitH" | b"FitBH" => number(2),
                b"FitR" => number(5),
                _ => None,
            },
            _ => None,
        };
        // The frame of the page it goes to, which may be turned differently from the page the link is on.
        let within = top.and_then(|top| frame_of(self.reader, self.map.get(page as usize)?).within(top));
        LinkTarget::Page { page, within }
    }
}

/// Whether an address may be offered, and how it's refused if not. Only `http`, `https` and `mailto`, printable ASCII with no spaces, so what's shown is exactly what would open.
pub fn judge_address(bytes: &[u8]) -> LinkTarget {
    if bytes.is_empty() || bytes.len() > MAX_URI_BYTES || !bytes.iter().all(|b| b.is_ascii_graphic()) {
        // A `javascript:` or `file:` address is refused for what it is, whatever else is wrong with it.
        return match scheme_of(bytes) {
            Some(scheme) if scheme == "javascript" || scheme == "vbscript" => LinkTarget::Refused(Refused::Script),
            Some(scheme) if scheme == "file" => LinkTarget::Refused(Refused::File),
            _ => LinkTarget::Refused(Refused::BadAddress),
        };
    }
    // A backslash is a slash to a browser and not to a reader of the address, so `https://evil.example\@bank.com/` would show one host and go to another; and Foundation rewrites these before opening, so what opened wouldn't be what was shown.
    if bytes.iter().any(|b| matches!(b, b'\\' | b'"' | b'<' | b'>' | b'^' | b'`' | b'{' | b'|' | b'}')) {
        return LinkTarget::Refused(Refused::BadAddress);
    }
    let Some(scheme) = scheme_of(bytes) else {
        // A relative address would go against a base the file chooses, so it's another address than the one shown.
        return LinkTarget::Refused(Refused::BadAddress);
    };
    let rest = &bytes[scheme.len() + 1..];
    match scheme.as_str() {
        "http" | "https" if rest.starts_with(b"//") && rest.len() > 2 && rest[2] != b'/' => LinkTarget::Web(String::from_utf8_lossy(bytes).into()),
        "mailto" if rest.len() > 1 => LinkTarget::Web(String::from_utf8_lossy(bytes).into()),
        "http" | "https" | "mailto" => LinkTarget::Refused(Refused::BadAddress),
        "javascript" | "vbscript" => LinkTarget::Refused(Refused::Script),
        "file" => LinkTarget::Refused(Refused::File),
        _ => LinkTarget::Refused(Refused::Scheme),
    }
}

/// An address's scheme, lower-cased, if it starts with one: a letter, then letters, digits, `+`, `-` or `.`, then `:`. Leading spaces and controls are skipped, as some systems skip them.
fn scheme_of(bytes: &[u8]) -> Option<String> {
    let start = bytes.iter().position(|b| !(b.is_ascii_whitespace() || b.is_ascii_control()))?;
    let bytes = &bytes[start..];
    let colon = bytes.iter().take(32).position(|b| *b == b':')?;
    let scheme = &bytes[..colon];
    (scheme.first().is_some_and(u8::is_ascii_alphabetic) && scheme.iter().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'+' | b'-' | b'.')))
        .then(|| String::from_utf8_lossy(scheme).to_ascii_lowercase())
}

fn find_named(reader: &IndexedReader, root: &Object, wanted: &[u8], budget: &Budget) -> Option<Object> {
    let mut stack: Vec<Object> = Vec::new();
    let mut seen: HashSet<ObjectId> = HashSet::new();
    // A root written in place is looked through where it is: copying a tree of every name for each link was most of a page's time.
    match root {
        Object::Dictionary(dict) => {
            if !budget.may_visit() {
                return None;
            }
            if let Some(found) = look_in(reader, dict, wanted, &mut stack, budget) {
                return found;
            }
        }
        other => stack.push(other.clone()),
    }
    while let Some(node) = stack.pop() {
        let Object::Reference(id) = node else { continue };
        if !seen.insert(id) {
            continue;
        }
        if !budget.may_visit() {
            return None;
        }
        let kept = budget.nodes.borrow().get(&id).cloned();
        let dict = match kept {
            Some(dict) => dict,
            None => {
                let Some(Object::Dictionary(dict)) = reader.resolve_object(id).ok() else {
                    continue;
                };
                let dict = std::rc::Rc::new(dict);
                let mut nodes = budget.nodes.borrow_mut();
                if nodes.len() < MAX_NODES_KEPT {
                    nodes.insert(id, std::rc::Rc::clone(&dict));
                }
                dict
            }
        };
        if let Some(found) = look_in(reader, &dict, wanted, &mut stack, budget) {
            return found;
        }
    }
    None
}

/// One node of a name tree: what it names `wanted` as, if it does (`Some(None)` for a name whose value can't be read), and otherwise its kids, waiting.
fn look_in(reader: &IndexedReader, dict: &Dictionary, wanted: &[u8], stack: &mut Vec<Object>, budget: &Budget) -> Option<Option<Object>> {
    if let Ok(limits) = dict.get(b"Limits").and_then(Object::as_array)
        && let (Some(Ok(low)), Some(Ok(high))) = (limits.first().map(Object::as_str), limits.get(1).map(Object::as_str))
        && (wanted < low || wanted > high)
    {
        return None;
    }
    if let Ok(pairs) = dict.get(b"Names").and_then(Object::as_array) {
        for [name, value] in pairs.as_chunks::<2>().0 {
            if name.as_str().ok() == Some(wanted) {
                return Some(deref(reader, value));
            }
        }
    }
    if let Ok(kids) = dict.get(b"Kids").and_then(Object::as_array) {
        let room = budget.visits_left.get().saturating_sub(stack.len());
        // Only references wait: a kid written in place can't be told from its parent, and a real tree's kids are objects.
        stack.extend(kids.iter().take(room).filter(|kid| matches!(kid, Object::Reference(_))).cloned());
    }
    None
}

fn dict_at(reader: &IndexedReader, id: ObjectId) -> Option<Dictionary> {
    match reader.resolve_object(id).ok()? {
        Object::Dictionary(dict) => Some(dict),
        _ => None,
    }
}

fn number_of(reader: &IndexedReader, object: &Object) -> Option<f32> {
    match deref(reader, object)? {
        Object::Integer(value) => Some(value as f32),
        Object::Real(value) => Some(value),
        _ => None,
    }
}

/// Four numbers, as a `/Rect` or a box is.
fn four(reader: &IndexedReader, object: &Object) -> Option<[f32; 4]> {
    let Object::Array(items) = deref(reader, object)? else { return None };
    if items.len() != 4 {
        return None;
    }
    let mut out = [0.0; 4];
    for (slot, item) in out.iter_mut().zip(&items) {
        *slot = number_of(reader, item).filter(|value| value.is_finite())?;
    }
    Some(out)
}

fn rect_of(reader: &IndexedReader, annot: &Dictionary) -> Option<[f32; 4]> {
    four(reader, annot.get(b"Rect").ok()?)
}

/// The page's crop within its media box, and its own turn, as PDFium reads them. A box that can't be read is US Letter, as PDFium's is.
pub(crate) fn frame_of(reader: &IndexedReader, entry: &lopdf::PageMapEntry) -> Frame {
    let inherited = entry.inherited();
    let boxed = |owner: Option<ObjectId>, key: &[u8]| -> Option<[f32; 4]> {
        let dict = dict_at(reader, owner?)?;
        let rect = four(reader, dict.get(key).ok()?)?;
        Some([rect[0].min(rect[2]), rect[1].min(rect[3]), rect[0].max(rect[2]), rect[1].max(rect[3])])
    };
    let media = boxed(inherited.media_box(), b"MediaBox")
        .filter(|m| m[2] > m[0] && m[3] > m[1])
        .unwrap_or([0.0, 0.0, 612.0, 792.0]);
    let shown = match boxed(inherited.crop_box(), b"CropBox") {
        Some(crop) => {
            let cut = [crop[0].max(media[0]), crop[1].max(media[1]), crop[2].min(media[2]), crop[3].min(media[3])];
            if cut[2] > cut[0] && cut[3] > cut[1] { cut } else { media }
        }
        None => media,
    };
    let turns = inherited
        .rotate()
        .and_then(|owner| dict_at(reader, owner))
        .and_then(|dict| deref(reader, dict.get(b"Rotate").ok()?))
        .and_then(|value| value.as_i64().ok())
        .filter(|degrees| degrees % 90 == 0)
        .map_or(0, |degrees| (degrees.rem_euclid(360) / 90) as u8);
    Frame {
        turns,
        left: shown[0],
        bottom: shown[1],
        right: shown[2],
        top: shown[3],
    }
}

/// How a range's numbers are written.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LabelStyle {
    /// No number: the prefix alone.
    None,
    Decimal,
    RomanUpper,
    RomanLower,
    LettersUpper,
    LettersLower,
}

impl LabelStyle {
    /// A number for each, for keeping it in a file of Rust's own.
    pub fn code(self) -> u8 {
        match self {
            Self::None => 0,
            Self::Decimal => 1,
            Self::RomanUpper => 2,
            Self::RomanLower => 3,
            Self::LettersUpper => 4,
            Self::LettersLower => 5,
        }
    }

    /// The other way. A number that isn't one is no number, the prefix alone.
    pub fn from_code(code: u8) -> Self {
        match code {
            1 => Self::Decimal,
            2 => Self::RomanUpper,
            3 => Self::RomanLower,
            4 => Self::LettersUpper,
            5 => Self::LettersLower,
            _ => Self::None,
        }
    }
}

/// One range of page labels, from its first page to the next range's.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LabelRange {
    /// Zero-based page it starts on.
    pub start: u32,
    /// The number its first page has, 1 or more.
    pub first: u32,
    pub style: LabelStyle,
    /// Cleaned, and at most [`MAX_LABEL_PREFIX_BYTES`].
    pub prefix: Box<str>,
}

/// A document's page labels: what it calls its pages, such as `iv` or `A-3`. Empty for a file with none, which is most of them, and then every page is its number.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PageLabels {
    /// Sorted by start, starts all different, the first at page 0 if there are any.
    ranges: Box<[LabelRange]>,
    /// Ranges were left out at a limit.
    pub truncated: bool,
}

impl PageLabels {
    /// From ranges kept elsewhere, such as a sleep file, which can't be believed any more than the PDF: held to the same rules.
    pub fn from_ranges(ranges: Vec<LabelRange>, truncated: bool) -> Self {
        let cleaned = ranges
            .into_iter()
            .take(MAX_LABEL_RANGES)
            .map(|range| LabelRange {
                prefix: clean_prefix(&range.prefix),
                first: range.first.clamp(1, MAX_LABEL_START),
                ..range
            })
            .collect();
        Self {
            ranges: normalise(cleaned),
            truncated,
        }
    }

    pub fn ranges(&self) -> &[LabelRange] {
        &self.ranges
    }

    pub fn is_empty(&self) -> bool {
        self.ranges.is_empty()
    }

    /// A zero-based page's label, or nothing if the file has none.
    pub fn label(&self, page: u32) -> Option<String> {
        let at = self.ranges.partition_point(|range| range.start <= page).checked_sub(1)?;
        let range = &self.ranges[at];
        let number = range.first.saturating_add(page - range.start);
        let mut out = range.prefix.to_string();
        let written = format_number(number, range.style);
        // A label never grows past its cap, whatever the number.
        if out.len() + written.len() > MAX_LABEL_BYTES {
            return Some(format!("{out}{number}").chars().take(MAX_LABEL_BYTES).collect());
        }
        out.push_str(&written);
        Some(out)
    }

    /// A page's label if it says something its number doesn't: `iv` for the fourth page, but nothing for a page labelled `4`.
    pub fn differs(&self, page: u32) -> Option<String> {
        self.label(page).filter(|label| *label != (u64::from(page) + 1).to_string())
    }

    /// The page a typed label names, zero-based, among `pages`: the exact label first, then ignoring case. Nothing if no page has it.
    pub fn find(&self, typed: &str, pages: u32) -> Option<u32> {
        let typed = typed.trim();
        if typed.is_empty() || typed.len() > MAX_LABEL_BYTES {
            return None;
        }
        self.find_by(typed, pages, false).or_else(|| self.find_by(typed, pages, true))
    }

    fn find_by(&self, typed: &str, pages: u32, any_case: bool) -> Option<u32> {
        for (at, range) in self.ranges.iter().enumerate() {
            let end = self.ranges.get(at + 1).map_or(pages, |next| next.start.min(pages));
            if range.start >= end {
                continue;
            }
            let rest = if any_case {
                (typed.len() >= range.prefix.len()
                    && typed.is_char_boundary(range.prefix.len())
                    && typed[..range.prefix.len()].eq_ignore_ascii_case(&range.prefix))
                .then(|| &typed[range.prefix.len()..])
            } else {
                typed.strip_prefix(&*range.prefix)
            };
            let Some(rest) = rest else { continue };
            let number = match range.style {
                LabelStyle::None => rest.is_empty().then_some(range.first),
                style => parse_number(rest, style, any_case),
            };
            let Some(number) = number.filter(|number| *number >= range.first) else {
                continue;
            };
            let page = u64::from(range.start) + u64::from(number - range.first);
            if page < u64::from(end) {
                // Written back the same, so `IV` doesn't find a page labelled `iiii`, nor `007` one labelled `7`.
                let page = page as u32;
                if self
                    .label(page)
                    .is_some_and(|label| if any_case { label.eq_ignore_ascii_case(typed) } else { label == typed })
                {
                    return Some(page);
                }
            }
        }
        None
    }
}

fn clean_prefix(prefix: &str) -> Box<str> {
    let cleaned = crate::clean_line(prefix);
    let mut end = cleaned.len().min(MAX_LABEL_PREFIX_BYTES);
    while !cleaned.is_char_boundary(end) {
        end -= 1;
    }
    cleaned[..end].into()
}

/// Sorted, one range per start, and starting at page 0: pages before a file's first range are their numbers, as Acrobat shows them.
fn normalise(mut ranges: Vec<LabelRange>) -> Box<[LabelRange]> {
    ranges.sort_by_key(|range| range.start);
    ranges.dedup_by_key(|range| range.start);
    if ranges.first().is_some_and(|range| range.start > 0) {
        ranges.insert(
            0,
            LabelRange {
                start: 0,
                first: 1,
                style: LabelStyle::Decimal,
                prefix: "".into(),
            },
        );
        ranges.truncate(MAX_LABEL_RANGES);
    }
    ranges.into_boxed_slice()
}

/// Reads `/PageLabels` from the catalog, bounded. A file with none, or one that can't be read, has none.
pub(crate) fn read_labels(reader: &IndexedReader, pages: u32) -> PageLabels {
    labels_within(reader, pages, &Budget::new())
}

fn labels_within(reader: &IndexedReader, pages: u32, budget: &Budget) -> PageLabels {
    let Some(root) = reader.trailer_entry_owned(b"Root").ok().flatten() else {
        return PageLabels::default();
    };
    let Some(tree) = root.as_dict().ok().and_then(|root| root.get(b"PageLabels").ok()).cloned() else {
        return PageLabels::default();
    };
    let mut ranges = Vec::new();
    let mut truncated = false;
    let mut stack = vec![tree];
    let mut seen: HashSet<ObjectId> = HashSet::new();
    while let Some(node) = stack.pop() {
        if let Object::Reference(id) = &node
            && !seen.insert(*id)
        {
            continue;
        }
        if !budget.may_visit() {
            truncated = true;
            break;
        }
        let Some(Object::Dictionary(dict)) = deref(reader, &node) else { continue };
        if let Ok(nums) = dict.get(b"Nums").and_then(Object::as_array) {
            for [key, value] in nums.as_chunks::<2>().0 {
                if ranges.len() >= MAX_LABEL_RANGES {
                    truncated = true;
                    break;
                }
                let Some(start) = deref(reader, key)
                    .and_then(|key| key.as_i64().ok())
                    .and_then(|key| u32::try_from(key).ok())
                    .filter(|start| *start < pages.max(1))
                else {
                    continue;
                };
                let Some(Object::Dictionary(style)) = deref(reader, value) else { continue };
                ranges.push(range_of(reader, start, &style));
            }
        }
        if let Ok(kids) = dict.get(b"Kids").and_then(Object::as_array) {
            let room = budget.visits_left.get().saturating_sub(stack.len());
            truncated |= kids.len() > room;
            // In reverse, so the first kid is read first and a cap keeps the start of the book.
            stack.extend(kids.iter().take(room).rev().cloned());
        }
    }
    PageLabels {
        ranges: normalise(ranges),
        truncated,
    }
}

fn range_of(reader: &IndexedReader, start: u32, dict: &Dictionary) -> LabelRange {
    let style = match dict.get(b"S") {
        Ok(Object::Name(name)) => match name.as_slice() {
            b"D" => LabelStyle::Decimal,
            b"R" => LabelStyle::RomanUpper,
            b"r" => LabelStyle::RomanLower,
            b"A" => LabelStyle::LettersUpper,
            b"a" => LabelStyle::LettersLower,
            _ => LabelStyle::None,
        },
        _ => LabelStyle::None,
    };
    let prefix = dict
        .get(b"P")
        .ok()
        .and_then(|prefix| deref(reader, prefix))
        .and_then(|prefix| prefix.as_str().ok().map(clean_text))
        .unwrap_or_default();
    let first = dict
        .get(b"St")
        .ok()
        .and_then(|first| deref(reader, first))
        .and_then(|first| first.as_i64().ok())
        .map_or(1, |first| first.clamp(1, i64::from(MAX_LABEL_START)) as u32);
    LabelRange {
        start,
        first,
        style,
        prefix: clean_prefix(&prefix),
    }
}

/// A number as a style writes it. Roman past 3999, which has no standard form, and letters past 16 of them, are written as digits, so a label is short whatever a file asks.
fn format_number(number: u32, style: LabelStyle) -> String {
    match style {
        LabelStyle::None => String::new(),
        LabelStyle::Decimal => number.to_string(),
        LabelStyle::RomanUpper | LabelStyle::RomanLower if (1..=3999).contains(&number) => {
            let roman = roman(number);
            if style == LabelStyle::RomanLower { roman.to_ascii_lowercase() } else { roman }
        }
        LabelStyle::LettersUpper | LabelStyle::LettersLower if number >= 1 && (number - 1) / 26 < 16 => {
            // PDF's letters: A to Z, then AA to ZZ, then AAA, the same letter repeated.
            let letter = b'A' + ((number - 1) % 26) as u8;
            let letter = if style == LabelStyle::LettersLower {
                letter.to_ascii_lowercase()
            } else {
                letter
            };
            String::from(letter as char).repeat(((number - 1) / 26 + 1) as usize)
        }
        _ => number.to_string(),
    }
}

fn roman(mut number: u32) -> String {
    const PARTS: [(u32, &str); 13] = [
        (1000, "M"),
        (900, "CM"),
        (500, "D"),
        (400, "CD"),
        (100, "C"),
        (90, "XC"),
        (50, "L"),
        (40, "XL"),
        (10, "X"),
        (9, "IX"),
        (5, "V"),
        (4, "IV"),
        (1, "I"),
    ];
    let mut out = String::new();
    for (value, part) in PARTS {
        while number >= value {
            out.push_str(part);
            number -= value;
        }
    }
    out
}

/// Reads a typed number in a style. Whatever doesn't write back the same is caught by the caller.
fn parse_number(text: &str, style: LabelStyle, any_case: bool) -> Option<u32> {
    if text.is_empty() || text.len() > 32 {
        return None;
    }
    let digits = || text.bytes().all(|b| b.is_ascii_digit()).then(|| text.parse::<u32>().ok()).flatten();
    match style {
        LabelStyle::None => None,
        LabelStyle::Decimal => digits(),
        LabelStyle::RomanUpper | LabelStyle::RomanLower => {
            if text.bytes().all(|b| b.is_ascii_digit()) {
                // Past 3999 these are written as digits.
                return digits().filter(|number| *number > 3999);
            }
            let upper = text.to_ascii_uppercase();
            let right_case = any_case || (style == LabelStyle::RomanUpper) == (upper == text);
            let mut total = 0u32;
            let mut previous = 0u32;
            for c in upper.bytes().rev() {
                let value = match c {
                    b'I' => 1,
                    b'V' => 5,
                    b'X' => 10,
                    b'L' => 50,
                    b'C' => 100,
                    b'D' => 500,
                    b'M' => 1000,
                    _ => return None,
                };
                if value < previous {
                    total = total.checked_sub(value)?
                } else {
                    total = total.checked_add(value)?
                }
                previous = previous.max(value);
            }
            right_case.then_some(total)
        }
        LabelStyle::LettersUpper | LabelStyle::LettersLower => {
            if text.bytes().all(|b| b.is_ascii_digit()) {
                return digits().filter(|number| *number >= 1 && (number - 1) / 26 >= 16);
            }
            let first = text.as_bytes()[0];
            if !first.is_ascii_alphabetic() || !text.bytes().all(|b| b == first) {
                return None;
            }
            let right_case = any_case || (style == LabelStyle::LettersUpper) == first.is_ascii_uppercase();
            let index = u32::from(first.to_ascii_uppercase() - b'A');
            right_case.then(|| (text.len() as u32 - 1) * 26 + index + 1)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Failure: a page of thousands of links, or a label tree of thousands of ranges, is read whole and held, as nothing counts them. The caps on counts, with the time cap given room, so a busy machine can't make this a test of the clock: `tests/nav.rs` holds the same files to their limits whatever stops them.
    #[test]
    fn counts_are_capped_whatever_the_time() {
        let many = (0..MAX_PAGE_LINKS + 1)
            .map(|at| format!("<< /Subtype /Link /Rect [{at} 0 {} 10] /Dest [3 0 R /Fit] >>", at + 1))
            .collect::<Vec<_>>()
            .join(" ");
        let bytes = recto_testpdf::pages(&[recto_testpdf::Page {
            content: Vec::new(),
            page_extra: format!("/Annots [{many}]"),
            media: (612.0, 792.0),
        }]);
        let reader = open_bytes(Arc::from(bytes)).unwrap();
        let read = links_within(&reader, &[0], Budget::lasting(Duration::from_secs(120)));
        assert_eq!(read[0].1.links.len(), MAX_PAGE_LINKS);
        assert!(read[0].1.truncated);
        let pages = MAX_LABEL_RANGES + 1;
        let nums = (0..MAX_LABEL_RANGES + 1)
            .map(|at| format!("{at} << /S /D /P (p{at}-) >>"))
            .collect::<Vec<_>>()
            .join(" ");
        let bytes = recto_testpdf::pages(&(0..pages).map(|_| recto_testpdf::Page::new(Vec::new())).collect::<Vec<_>>());
        let at = bytes.windows(14).position(|window| window == b"/Type /Catalog").unwrap() + 14;
        let bytes = [&bytes[..at], format!(" /PageLabels << /Nums [{nums}] >>").as_bytes(), &bytes[at..]].concat();
        // The objects after it have moved, and the reader finds them again by their numbers when the table doesn't hold.
        let reader = open_bytes(Arc::from(bytes)).unwrap();
        let labels = labels_within(&reader, pages as u32, &Budget::lasting(Duration::from_secs(120)));
        assert_eq!(labels.ranges().len(), MAX_LABEL_RANGES);
        assert!(labels.truncated);
    }

    /// Failure: a field added to `Link` makes every link of every page the pointer has crossed bigger.
    #[test]
    fn link_stays_small() {
        assert!(std::mem::size_of::<Link>() <= 48, "{}", std::mem::size_of::<Link>());
    }

    /// Failure: a `javascript:` or `file:` address, a custom app scheme or a relative address is offered to open; or an ordinary web address isn't.
    #[test]
    fn only_web_and_email_addresses_are_offered() {
        assert_eq!(judge_address(b"https://example.com/a?b=c"), LinkTarget::Web("https://example.com/a?b=c".into()));
        assert_eq!(judge_address(b"HTTP://example.com"), LinkTarget::Web("HTTP://example.com".into()));
        assert_eq!(
            judge_address(b"mailto:someone@example.com"),
            LinkTarget::Web("mailto:someone@example.com".into())
        );
        for (address, why) in [
            (&b"javascript:alert(1)"[..], Refused::Script),
            (b"JavaScript:void(0)", Refused::Script),
            (b" javascript:alert(1)", Refused::Script),
            (b"file:///etc/passwd", Refused::File),
            (b"FILE:///Applications/Calculator.app", Refused::File),
            (b"x-apple.systempreferences:com.apple", Refused::Scheme),
            (b"vnc://host", Refused::Scheme),
            (b"tel:+4712345678", Refused::Scheme),
            (b"/relative/path", Refused::BadAddress),
            (b"www.example.com", Refused::BadAddress),
            (b"https://exa mple.com", Refused::BadAddress),
            (b"https://example.com/\x00", Refused::BadAddress),
            (b"https:///etc", Refused::BadAddress),
            (b"https:", Refused::BadAddress),
            (b"https://evil.example\\@bank.com/", Refused::BadAddress),
            (b"https://example.com/<script>", Refused::BadAddress),
            (b"https://example.com/a|b", Refused::BadAddress),
            (b"https://example.com/{x}", Refused::BadAddress),
            (b"https://\xc3\xa9xample.com", Refused::BadAddress),
            (b"", Refused::BadAddress),
        ] {
            assert_eq!(judge_address(address), LinkTarget::Refused(why), "{}", String::from_utf8_lossy(address));
        }
        let long = format!("https://example.com/{}", "a".repeat(MAX_URI_BYTES));
        assert_eq!(
            judge_address(long.as_bytes()),
            LinkTarget::Refused(Refused::BadAddress),
            "too long to show in full"
        );
    }

    /// Failure: a link's box lands somewhere else than where it's drawn on a cropped or turned page, so the pointer finds it in the wrong place.
    #[test]
    fn a_links_box_is_where_the_page_shows_it() {
        let upright = Frame {
            turns: 0,
            left: 0.0,
            bottom: 0.0,
            right: 612.0,
            top: 792.0,
        };
        assert_eq!(upright.to_shown([72.0, 700.0, 172.0, 712.0]), Some([72.0, 80.0, 172.0, 92.0]));
        let cropped = Frame {
            turns: 0,
            left: 50.0,
            bottom: 100.0,
            right: 562.0,
            top: 692.0,
        };
        assert_eq!(cropped.to_shown([72.0, 600.0, 172.0, 612.0]), Some([22.0, 80.0, 122.0, 92.0]));
        let turned = Frame { turns: 1, ..upright };
        assert_eq!(turned.shown_size(), Some((792.0, 612.0)));
        assert_eq!(turned.to_shown([72.0, 700.0, 172.0, 712.0]), Some([700.0, 72.0, 712.0, 172.0]));
        assert_eq!(upright.to_shown([f32::NAN, 0.0, 1.0, 1.0]), None);
        assert_eq!(upright.within(792.0), Some(0.0));
        assert_eq!(upright.within(396.0), Some(0.5));
        assert_eq!(turned.within(396.0), None, "a quarter-turned page's top is a shown left");
    }

    fn labels(ranges: &[(u32, u32, LabelStyle, &str)]) -> PageLabels {
        PageLabels::from_ranges(
            ranges
                .iter()
                .map(|(start, first, style, prefix)| LabelRange {
                    start: *start,
                    first: *first,
                    style: *style,
                    prefix: (*prefix).into(),
                })
                .collect(),
            false,
        )
    }

    /// Failure: roman front matter, prefixed appendices or letters read wrong, so the page number in the corner says something the book doesn't.
    #[test]
    fn labels_read_as_the_book_numbers_its_pages() {
        let book = labels(&[
            (0, 1, LabelStyle::RomanLower, ""),
            (4, 1, LabelStyle::Decimal, ""),
            (10, 1, LabelStyle::Decimal, "A-"),
            (12, 1, LabelStyle::LettersUpper, ""),
            (14, 1, LabelStyle::None, "Cover"),
        ]);
        let shown: Vec<String> = (0..16).map(|page| book.label(page).unwrap()).collect();
        assert_eq!(
            shown,
            ["i", "ii", "iii", "iv", "1", "2", "3", "4", "5", "6", "A-1", "A-2", "A", "B", "Cover", "Cover"]
        );
        assert_eq!(book.differs(4), Some("1".into()), "the fifth page, called 1");
        assert_eq!(book.differs(0), Some("i".into()));
        assert_eq!(labels(&[(0, 1, LabelStyle::Decimal, "")]).differs(7), None, "a plain number says nothing new");
        assert_eq!(PageLabels::default().label(3), None);
        assert_eq!(format_number(27, LabelStyle::LettersLower), "aa");
        assert_eq!(format_number(1999, LabelStyle::RomanUpper), "MCMXCIX");
        assert_eq!(format_number(4000, LabelStyle::RomanUpper), "4000", "no standard roman past 3999");
        assert_eq!(
            format_number(u32::MAX, LabelStyle::LettersUpper),
            u32::MAX.to_string(),
            "letters past 16 go as digits"
        );
    }

    /// Failure: typing `iv` or `A-2` in the go-to field doesn't find the page, or finds a page whose label only looks similar.
    #[test]
    fn a_typed_label_finds_its_page() {
        let book = labels(&[
            (0, 1, LabelStyle::RomanLower, ""),
            (4, 1, LabelStyle::Decimal, ""),
            (10, 1, LabelStyle::Decimal, "A-"),
        ]);
        assert_eq!(book.find("iv", 12), Some(3));
        assert_eq!(book.find("IV", 12), Some(3), "case is a fallback");
        assert_eq!(book.find("1", 12), Some(4), "the page labelled 1, which is the fifth");
        assert_eq!(book.find("A-2", 12), Some(11));
        assert_eq!(book.find("a-2", 12), Some(11));
        assert_eq!(book.find("iiii", 12), None, "not how iv is written");
        assert_eq!(book.find("007", 12), None);
        assert_eq!(book.find("v", 12), None, "front matter stops at iv");
        assert_eq!(book.find("7", 12), None, "the decimal range has six pages");
        assert_eq!(book.find("", 12), None);
    }

    /// Failure: a file's ranges out of order, repeated, or starting past page 0 give pages two labels or none.
    #[test]
    fn ranges_are_sorted_and_start_at_the_first_page() {
        let messy = labels(&[
            (5, 1, LabelStyle::Decimal, ""),
            (2, 1, LabelStyle::RomanLower, ""),
            (5, 9, LabelStyle::Decimal, "x"),
        ]);
        let starts: Vec<u32> = messy.ranges().iter().map(|range| range.start).collect();
        assert_eq!(starts, [0, 2, 5]);
        assert_eq!(messy.label(0).as_deref(), Some("1"));
        assert_eq!(messy.label(2).as_deref(), Some("i"));
        let long = labels(&[(0, 1, LabelStyle::Decimal, &"p".repeat(500))]);
        assert!(long.label(0).unwrap().len() <= MAX_LABEL_BYTES);
        let hostile = labels(&[(0, 1, LabelStyle::Decimal, "a\u{202E}b\nc")]);
        assert_eq!(hostile.label(0).as_deref(), Some("ab c1"));
    }
}
