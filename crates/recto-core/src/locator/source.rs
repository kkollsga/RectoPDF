//! Where a reference's page reads come from: the file read live ([`LiveSource`]), or reads an indexer stored when it built the text corpus
//! (`recto_index::corpus::Identities`). [`Referrer`](super::Referrer) goes through [`PageSource`] for both, so its memo, its caps, its tag rule and its
//! `Unreferenceable` mapping are one piece of code and a reference is the same token by either road. [`resolve`](super::resolve) stays live only: it is
//! what checks a reference against the file as it is now.

use super::TAG_PAGES;
use crate::page_id::{self, Caps, DIGEST_NAME, IndexedReader, PageMap, PageRead};
use sha2::{Digest, Sha256};

/// What asking a source for page `index` came to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SourceRead {
    /// The document has no such page.
    Absent,
    Read(PageRead),
    /// A stored read that doesn't hold up (a record that doesn't open as this page's): the source can't answer, and says nothing about the page.
    Unreadable,
}

/// A document's pages, read as [`page_id::read_page`] reads them.
pub trait PageSource {
    /// Pages the document has (the length of its lopdf page map).
    fn len(&self) -> usize;

    /// Page `index` (zero-based) read under `caps`. A source that holds reads made under fixed caps answers only for those: see [`PageSource::caps`].
    fn read(&self, index: u32, caps: &Caps) -> SourceRead;

    /// The first eight bytes of the `/ID` tag, where the document has two `/ID` strings.
    fn document_id(&self) -> Option<[u8; 8]>;

    /// The digest caps this source's reads were made under, `None` where it reads under whatever it is asked.
    fn caps(&self) -> Option<Caps> {
        None
    }

    fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// The file as it is, through the lazy reader and its page map.
pub struct LiveSource<'a> {
    reader: &'a IndexedReader,
    map: &'a PageMap,
}

impl<'a> LiveSource<'a> {
    pub fn new(reader: &'a IndexedReader, map: &'a PageMap) -> Self {
        Self { reader, map }
    }
}

impl PageSource for LiveSource<'_> {
    fn len(&self) -> usize {
        self.map.len()
    }

    fn read(&self, index: u32, caps: &Caps) -> SourceRead {
        match self.map.get(index as usize) {
            Some(entry) => SourceRead::Read(page_id::read_page(self.reader, entry, caps)),
            None => SourceRead::Absent,
        }
    }

    fn document_id(&self) -> Option<[u8; 8]> {
        page_id::document_ids(self.reader).map(|ids| first_eight(&ids.first))
    }
}

pub(super) fn first_eight(bytes: &[u8]) -> [u8; 8] {
    let mut out = [0; 8];
    out.copy_from_slice(&bytes[..8]);
    out
}

/// **What a stored read is only good for:** a hash of everything that decides what [`page_id::read_page`] answers under [`Caps::STANDARD`] and what a
/// document tag is made of: the digest's name, every field of the standard caps, how many pages a tag looks at and the tag's own name. A stored read
/// carries it, and is trusted only where it equals this build's: a caps change (the names cap went from 256 to 8,192 on 2026-11-25) would otherwise leave
/// stored "no identity" under the same digest name. Every field is named below, so a new cap field doesn't compile until it is here.
pub fn stored_rules_tag() -> [u8; 16] {
    let Caps {
        content_streams,
        content_bytes,
        xobjects,
        forms_drawing,
        xobject_bytes,
        page_bytes,
        page_read,
        decode_work,
    } = Caps::STANDARD;
    let mut hasher = Sha256::new();
    hasher.update(b"recto-stored-identities/1\0");
    hasher.update(DIGEST_NAME.as_bytes());
    hasher.update([0]);
    for number in [
        content_streams as u64,
        content_bytes,
        xobjects as u64,
        forms_drawing as u64,
        xobject_bytes,
        page_bytes,
        page_read,
        decode_work,
        TAG_PAGES as u64,
    ] {
        hasher.update(number.to_le_bytes());
    }
    hasher.update(super::resolve::CONTENT_TAG_NAME);
    let mut out = [0; 16];
    out.copy_from_slice(&hasher.finalize()[..16]);
    out
}
