//! **A journal's sidecar: what its edits' pages are, and nothing else** (`pages-<key>.ids` beside `journal-<key>.log`; the coordinator's decision B1,
//! 2026-11-15). The format, as bytes; the record store (`recto-control`'s `record_store/page_ids.rs`) keeps the file.
//!
//! **Its bytes:** a header, then entries, appended as pages are read; never rewritten for a step.
//! - Header: [`MAGIC`] (`recto-pages`, then a version byte, 2); the journal's file name, a length byte (at most [`MAX_NAME`]) and its bytes; the file
//!   version the entries are of, as `u64le` size, `i64le` modification time in nanoseconds and `u64le` file number (the journal's own header says
//!   these; the device isn't, as a volume can come back under another); `u32le` the file's page count; a byte of flags (1: the list followed its file
//!   onto this version and the person hasn't been told yet, slice 2's; other bits are kept 0 and read as nothing); and a check of all of it.
//! - A page: `P`, `u32le` page (zero-based, of that version), the 16 bytes of its digest, `u32le` object number, `u16le` generation, `u32le` frame tag,
//!   and a check: 35 bytes.
//! - The file's `/ID` pair: `D`, the two 16-byte tags, and a check: 37 bytes.
//! - A check is FNV-1a (32 bits, little-endian) over the bytes of its header or entry before it.
//!
//! **Read as hostile** (rule 6): at most [`MAX_BYTES`] are read; a header that doesn't check out, isn't this version or names a name too long is no
//! sidecar; entries are read up to the first that doesn't check out or isn't known (a torn tail, or something written since by a newer app), and at
//! most [`super::MAX_IDENTIFIED`] pages; a page past the file's page count, or one already given, is left out. Nothing is allocated by what the file
//! claims: entries are fixed-size and counted as they're read. Whether a sidecar is this journal's and this version's is the caller's to say from
//! [`Sidecar::header`], and one that isn't is ignored, never trusted.

use super::{Known, MAX_IDENTIFIED};
use recto_core::page_id::DocumentIds;

/// The bytes a sidecar starts with: its name, and the version of this format. Version 2 (2026-11-15, before any release wrote one): a page's frame tag
/// holds the page's own quarter turns in its low two bits (`recto_core::page_id::frame_turns`). A sidecar of version 1, whose tags' low bits are
/// hash, is no sidecar: its pages come back unread, and are read again while the file is the edits' version.
pub const MAGIC: &[u8; 12] = b"recto-pages\x02";
/// Longest journal name a header holds: `journal-` and 16 hex digits, a tab's number, `.log`, with room.
pub const MAX_NAME: usize = 64;
/// A page's entry's length.
pub const PAGE_ENTRY: usize = 35;
/// The `/ID` pair's entry's length.
pub const DOCUMENT_ENTRY: usize = 37;
/// Most bytes a sidecar is read to or written to: its header, a page entry for every page a list may name, and room for a few `/ID` pairs.
pub const MAX_BYTES: usize = 12 + 1 + MAX_NAME + 8 + 8 + 8 + 4 + 1 + 4 + MAX_IDENTIFIED * PAGE_ENTRY + 64 * DOCUMENT_ENTRY;

/// What a sidecar's entries are of: the journal and the version of its file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Header {
    pub journal: String,
    pub size: u64,
    pub mtime_ns: i64,
    pub inode: u64,
    pub file_pages: u32,
    /// The list followed its file onto this version and the person hasn't been told yet ([`super::followed_untold`]).
    pub followed_untold: bool,
}

impl Header {
    /// The header for a journal about this version of its file.
    pub fn new(journal: &str, stamp: recto_platform::FileStamp, file_pages: u32) -> Self {
        Self {
            journal: journal.to_string(),
            size: stamp.size,
            mtime_ns: stamp.mtime_ns,
            inode: stamp.inode,
            file_pages,
            followed_untold: false,
        }
    }

    /// Whether this is the header of a journal of this name about this version of its file, as the journal's own header says it.
    pub fn is_of(&self, journal: &str, header: &crate::record::JournalHeader) -> bool {
        let Some(file) = header.file.map(recto_platform::FileStamp::from) else {
            return false;
        };
        self.journal == journal && self.size == file.size && self.mtime_ns == file.mtime_ns && self.inode == file.inode && self.file_pages == header.file_pages
    }

    /// Its bytes, or `None` for a name too long to hold.
    pub fn encode(&self) -> Option<Vec<u8>> {
        let name = self.journal.as_bytes();
        if name.len() > MAX_NAME {
            return None;
        }
        let mut out = MAGIC.to_vec();
        out.push(u8::try_from(name.len()).ok()?);
        out.extend_from_slice(name);
        out.extend_from_slice(&self.size.to_le_bytes());
        out.extend_from_slice(&self.mtime_ns.to_le_bytes());
        out.extend_from_slice(&self.inode.to_le_bytes());
        out.extend_from_slice(&self.file_pages.to_le_bytes());
        out.push(u8::from(self.followed_untold));
        out.extend_from_slice(&check(&out));
        Some(out)
    }
}

/// What's waiting to go in a list's sidecar ([`super::sidecar_waiting`]).
#[derive(Clone, PartialEq, Eq)]
pub struct Waiting {
    /// The sidecar is written whole: its header, then `bytes`, in place of what's there.
    pub whole: bool,
    /// The entries, encoded.
    pub bytes: Vec<u8>,
    /// How many of the pages waiting these cover, from the first.
    pub unwritten: usize,
    /// Whether the `/ID` pair waiting is among them.
    pub document: bool,
    /// Not whole, and some of what the list knows went to a sidecar before: these are only the rest, to go only onto a sidecar that has the others. A
    /// sidecar that isn't there (the journal starting again under another name) is then written whole instead.
    pub rest: bool,
}

/// `Debug` says how much, never a digest.
impl std::fmt::Debug for Waiting {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Waiting")
            .field("whole", &self.whole)
            .field("bytes", &self.bytes.len())
            .field("unwritten", &self.unwritten)
            .finish_non_exhaustive()
    }
}

/// What became of a write.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Done {
    Written,
    /// What's on disk isn't this list's (another header, or nothing a launch read), so nothing was appended: the next write is whole.
    WantsWhole,
}

/// A sidecar as read.
#[derive(Clone, PartialEq, Eq)]
pub struct Sidecar {
    pub header: Header,
    pub pages: Vec<(u32, Known)>,
    pub document: Option<DocumentIds>,
    /// Bytes of it that read whole, which an append goes on from.
    pub good: usize,
}

/// `Debug` says how many, never a digest.
impl std::fmt::Debug for Sidecar {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Sidecar")
            .field("header", &self.header)
            .field("pages", &self.pages.len())
            .field("good", &self.good)
            .finish_non_exhaustive()
    }
}

fn check(bytes: &[u8]) -> [u8; 4] {
    bytes
        .iter()
        .fold(0x811c_9dc5u32, |value, byte| (value ^ u32::from(*byte)).wrapping_mul(0x0100_0193))
        .to_le_bytes()
}

/// Entries' bytes: each page's, then the `/ID` pair's if given.
pub fn entries(pages: &[(u32, Known)], document: Option<DocumentIds>) -> Vec<u8> {
    let mut out = Vec::with_capacity(pages.len() * PAGE_ENTRY + DOCUMENT_ENTRY);
    for (page, known) in pages {
        let at = out.len();
        out.push(b'P');
        out.extend_from_slice(&page.to_le_bytes());
        out.extend_from_slice(&known.digest);
        out.extend_from_slice(&known.object.0.to_le_bytes());
        out.extend_from_slice(&known.object.1.to_le_bytes());
        out.extend_from_slice(&known.frame.to_le_bytes());
        let sum = check(&out[at..]);
        out.extend_from_slice(&sum);
    }
    if let Some(ids) = document {
        let at = out.len();
        out.push(b'D');
        out.extend_from_slice(&ids.first);
        out.extend_from_slice(&ids.second);
        let sum = check(&out[at..]);
        out.extend_from_slice(&sum);
    }
    out
}

/// A fixed-size piece of `bytes` at `at`, if it's all there.
fn take<const N: usize>(bytes: &[u8], at: usize) -> Option<[u8; N]> {
    bytes.get(at..at.checked_add(N)?)?.try_into().ok()
}

fn header(bytes: &[u8]) -> Option<(Header, usize)> {
    if bytes.get(..MAGIC.len())? != MAGIC {
        return None;
    }
    let name_len = usize::from(*bytes.get(MAGIC.len())?);
    if name_len > MAX_NAME {
        return None;
    }
    let mut at = MAGIC.len() + 1;
    let journal = std::str::from_utf8(bytes.get(at..at + name_len)?).ok()?.to_string();
    at += name_len;
    let size = u64::from_le_bytes(take(bytes, at)?);
    let mtime_ns = i64::from_le_bytes(take(bytes, at + 8)?);
    let inode = u64::from_le_bytes(take(bytes, at + 16)?);
    let file_pages = u32::from_le_bytes(take(bytes, at + 24)?);
    let flags = *bytes.get(at + 28)?;
    at += 29;
    if take::<4>(bytes, at)? != check(&bytes[..at]) {
        return None;
    }
    Some((
        Header {
            journal,
            size,
            mtime_ns,
            inode,
            file_pages,
            followed_untold: flags & 1 != 0,
        },
        at + 4,
    ))
}

/// **Reads a sidecar's bytes:** `None` if they're no sidecar of this format (or more than [`MAX_BYTES`]); otherwise its header and what of its entries
/// read whole, as the module doc says.
pub fn decode(bytes: &[u8]) -> Option<Sidecar> {
    if bytes.len() > MAX_BYTES {
        return None;
    }
    let (header, mut at) = header(bytes)?;
    let mut pages: Vec<(u32, Known)> = Vec::new();
    let mut document = None;
    loop {
        match bytes.get(at) {
            Some(b'P') => {
                let Some(entry) = take::<PAGE_ENTRY>(bytes, at) else { break };
                if entry[31..] != check(&entry[..31]) {
                    break;
                }
                let page = u32::from_le_bytes(entry[1..5].try_into().ok()?);
                let known = Known {
                    digest: entry[5..21].try_into().ok()?,
                    object: (
                        u32::from_le_bytes(entry[21..25].try_into().ok()?),
                        u16::from_le_bytes(entry[25..27].try_into().ok()?),
                    ),
                    frame: u32::from_le_bytes(entry[27..31].try_into().ok()?),
                };
                at += PAGE_ENTRY;
                if page < header.file_pages && pages.len() < MAX_IDENTIFIED {
                    pages.push((page, known));
                }
            }
            Some(b'D') => {
                let Some(entry) = take::<DOCUMENT_ENTRY>(bytes, at) else { break };
                if entry[33..] != check(&entry[..33]) {
                    break;
                }
                document = Some(DocumentIds {
                    first: entry[1..17].try_into().ok()?,
                    second: entry[17..33].try_into().ok()?,
                });
                at += DOCUMENT_ENTRY;
            }
            _ => break,
        }
    }
    // A page given twice is the first given: a stable sort keeps it first.
    pages.sort_by_key(|(page, _)| *page);
    pages.dedup_by_key(|(page, _)| *page);
    Some(Sidecar {
        header,
        pages,
        document,
        good: at,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn known(seed: u8) -> Known {
        Known {
            digest: std::array::from_fn(|at| seed.wrapping_add(at as u8).wrapping_mul(37)),
            object: (u32::from(seed) * 1_000_003 + 7, u16::from(seed) * 3),
            frame: u32::from(seed).wrapping_mul(0x9e37_79b9),
        }
    }

    fn header() -> Header {
        Header {
            journal: "journal-0123456789abcdef.log".into(),
            size: 1234,
            mtime_ns: -5,
            inode: 99,
            file_pages: 20,
            followed_untold: false,
        }
    }

    fn sidecar(pages: &[(u32, Known)], document: Option<DocumentIds>) -> Vec<u8> {
        let mut bytes = header().encode().unwrap();
        bytes.extend(entries(pages, document));
        bytes
    }

    /// Failure caught: what's written doesn't read back as it went in (a byte of a digest, an object number or a frame lost or moved), so after a
    /// restart no edit could follow its page. Pages and the `/ID` pair, in two appends, read back exactly, and the entries are their stated sizes.
    #[test]
    fn what_goes_in_reads_back_exactly() {
        let document = DocumentIds {
            first: [1; 16],
            second: [2; 16],
        };
        let mut bytes = sidecar(&[(0, known(0)), (19, known(0xff))], None);
        assert_eq!(entries(&[(3, known(3))], None).len(), PAGE_ENTRY);
        assert_eq!(entries(&[], Some(document)).len(), DOCUMENT_ENTRY);
        bytes.extend(entries(&[(7, known(7))], Some(document)));
        let read = decode(&bytes).unwrap();
        assert_eq!(read.header, header());
        assert_eq!(read.pages, vec![(0, known(0)), (7, known(7)), (19, known(0xff))], "by page");
        assert_eq!(read.document, Some(document));
        assert_eq!(read.good, bytes.len());
    }

    /// Failure caught (rule 6): a hostile or torn sidecar is believed past what it can show: a torn tail, a flipped byte, a page past the file's end,
    /// a page given twice (the second would replace the first), more pages than a list holds, an unknown entry, a header that doesn't check out or
    /// claims a long name, and a file too large to be one. Each is read to its last good entry or not at all, with no panic.
    #[test]
    fn a_hostile_sidecar_is_read_no_further_than_it_shows() {
        let good = sidecar(&[(1, known(1)), (2, known(2))], None);
        let torn = &good[..good.len() - 3];
        assert_eq!(decode(torn).unwrap().pages.len(), 1, "a torn tail");
        assert_eq!(decode(torn).unwrap().good, good.len() - PAGE_ENTRY);
        let mut flipped = good.clone();
        let last = flipped.len() - 10;
        flipped[last] ^= 1;
        assert_eq!(decode(&flipped).unwrap().pages.len(), 1, "a flipped byte");
        assert!(decode(&sidecar(&[(20, known(1))], None)).unwrap().pages.is_empty(), "past the file's end");
        assert_eq!(
            decode(&sidecar(&[(1, known(1)), (1, known(9))], None)).unwrap().pages,
            vec![(1, known(1))],
            "given twice: the first"
        );
        let mut unknown = good.clone();
        unknown.extend_from_slice(b"Zxxxxxxxxxxxxxxxxxxx");
        assert_eq!(decode(&unknown).unwrap().good, good.len(), "an entry not known");
        let mut bad_header = good.clone();
        bad_header[14] ^= 1;
        assert_eq!(decode(&bad_header), None, "a header that doesn't check out");
        let mut long = good.clone();
        long[12] = 200;
        assert_eq!(decode(&long), None, "a name past its limit");
        let mut version_1 = good.clone();
        version_1[11] = 1;
        let sum = check(&version_1[..good.len() - 2 * PAGE_ENTRY - 4]);
        let at = good.len() - 2 * PAGE_ENTRY - 4;
        version_1[at..at + 4].copy_from_slice(&sum);
        assert_eq!(decode(&version_1), None, "version 1, whose frame tags are of the old definition");
        assert_eq!(decode(b"recto-pages\x03"), None, "another version");
        let mut told = header();
        told.followed_untold = true;
        assert!(decode(&told.encode().unwrap()).unwrap().header.followed_untold, "the flag reads back");
        let mut other_bits = header().encode().unwrap();
        let flags = other_bits.len() - 5;
        other_bits[flags] = 0xfe;
        let sum = check(&other_bits[..other_bits.len() - 4]);
        let end = other_bits.len();
        other_bits[end - 4..].copy_from_slice(&sum);
        assert!(!decode(&other_bits).unwrap().header.followed_untold, "bits not known are nothing");
        assert_eq!(decode(&vec![0u8; MAX_BYTES + 1]), None, "too large");
        for cut in 0..good.len() {
            let _ = decode(&good[..cut]);
        }
    }

    /// Failure caught: more pages than a list may name are taken from a sidecar, which costs memory past the cap. With a file of many pages, entries past
    /// [`MAX_IDENTIFIED`] are left out.
    #[test]
    fn no_more_pages_than_a_list_names() {
        let pages: Vec<(u32, Known)> = (0..=MAX_IDENTIFIED as u32).map(|page| (page, known(1))).collect();
        let mut bytes = Header {
            file_pages: u32::MAX,
            ..header()
        }
        .encode()
        .unwrap();
        bytes.extend(entries(&pages, None));
        assert!(bytes.len() <= MAX_BYTES);
        assert_eq!(decode(&bytes).unwrap().pages.len(), MAX_IDENTIFIED);
    }
}
