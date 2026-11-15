//! **The Find text, sealed** (owner, 2026-11-15: "Find text is a indexed copy of the pdf text? Lets store it encrypted"). Every part of a corpus that came from the document is kept as independent chunks of AES-256-GCM, so a search still reads one page at a time into one buffer, a build can still be killed at any point, and a chunk that's been changed, cut short, moved to another page or copied from another corpus fails to open, which throws the corpus away as any damage does.
//!
//! **One key per corpus.** Each corpus has a header, `seal.bin`: 8 bytes of magic, a byte saying whether the key it was sealed with lasts past its run, 32 random bytes of salt, and a check chunk over nothing whose associated data holds that byte. Its key is HKDF-SHA256 of the corpus's own root key with that salt, and the corpus's key in the info, so a folder renamed or copied under another file's key can't be opened. A new header is made whenever a corpus is started from nothing, so a rebuilt corpus never shares a key with what it replaces.
//!
//! **One root per corpus, and the helper gets only that** (the security read of 2026-11-15, S1). A corpus's root key is HKDF-SHA256 of the session's Find key ([`recto_platform::find_key`]), with no salt and the corpus's key in the info. The session hands the indexer, for the one file it builds, a key bound to that corpus ([`grant_for`]): its root, its source and its corpus. A bound key derives that corpus's seals and no other's, so a helper, the real one or one put in its place in the app's bundle after the app's check, can read the one corpus of the file it was asked to build, which it could read anyway as it reads the file, and nothing of any other corpus already built. **What a swapped helper can still do** (the second security read): write a validly sealed corpus for that file with any text, title or outline it likes, which Find and agents then take as that file's until the file changes; and it's handed a key for each file built after the swap. That's the trust a hostile PDF's own text already has: what the corpus says is never taken for more than the document's words.
//!
//! **Nonces are random,** 96 bits from the OS for every chunk, and written in front of it. A corpus holds at most 200,000 pages, each sealed at most 1 + 3 retries times plus restarts, so well under 2^21 chunks per key: the chance of two alike is under 2^-55, far inside NIST's 2^-32 for random nonces (SP 800-38D §8.3). A restarted build draws new ones, so a page sealed twice, killed and sealed again with other text, never reuses a nonce. Nothing to count, so nothing to get wrong after a kill.
//!
//! **What a chunk says to someone without the key:** its length (the page's text and what's beside it, plus 28 bytes) and where it is. So the number of pages, each page's length, and which pages have no text, as the table always said. Nothing else.
//!
//! **What's overwritten, and what isn't:** a corpus key's AES schedule when its [`CorpusSeal`] goes (`aes-gcm`'s `zeroize`), and the 32 bytes HKDF writes it into, at once. Not: HKDF's own pseudorandom key and HMAC states, which `hkdf` holds in its own values and drops as they are, on `derive`'s stack; GHASH's key inside the cipher beyond what `aes-gcm`'s `zeroize` covers. Each is a derivative of the Find key, in this process's memory for a moment, and a process that can read this one's memory has the Find key anyway.
//!
//! **A known property, accepted** (the security review of slices 2 and 3, 2026-11-15): a page's chunk is bound to its corpus and its page, not to its place in time, so within one corpus an older chunk of the same page, one a retry has since replaced, still opens if someone who can write here puts it back where the table points. It's that page's own text from before, of that same version of the file; nothing of another page or another file can be made to stand for it.
//!
//! A chunk is `nonce (12) | ciphertext | tag (16)`. Its associated data says what it is: a page and its number, the summary, the title and outline, or the header's check.

use crate::corpus::CorpusKey;
use aes_gcm::aead::{AeadInOut, KeyInit};
use aes_gcm::{Aes256Gcm, Nonce, Tag};
use recto_platform::find_key::FindKey;
use sha2::Sha256;

pub const NONCE_BYTES: usize = 12;
pub const TAG_BYTES: usize = 16;
/// What sealing adds to a chunk.
pub const OVERHEAD: usize = NONCE_BYTES + TAG_BYTES;
const MAGIC: &[u8; 8] = b"RECTOSL1";
const SALT_BYTES: usize = 32;
/// The header: magic, salt and a check chunk of nothing.
pub const HEADER_BYTES: usize = MAGIC.len() + 1 + SALT_BYTES + OVERHEAD;
const SALT_AT: usize = MAGIC.len() + 1;
const INFO: &[u8] = b"recto find text v4\0";
/// The info a corpus's root key is derived with, before its corpus key.
const ROOT_INFO: &[u8] = b"recto find text v4 corpus root\0";

/// What a chunk is, said in its associated data, so one can't stand for another.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Purpose {
    Page(u32),
    Summary,
    Meta,
    /// The header's check, with whether its key lasts.
    Check(bool),
}

impl Purpose {
    fn associated(self) -> [u8; 5] {
        let (kind, number) = match self {
            Self::Page(page) => (1, page),
            Self::Summary => (2, 0),
            Self::Meta => (3, 0),
            Self::Check(lasting) => (4, u32::from(lasting)),
        };
        let mut out = [kind, 0, 0, 0, 0];
        out[1..].copy_from_slice(&number.to_le_bytes());
        out
    }
}

/// **A buffer that holds a page's plain text,** overwritten, all its room, when it goes: a build's page as it's digested, a page opened to be checked or counted. What isn't: a buffer's old room when it grows, and the copies the text's own reading makes (the normaliser's strings), which are ordinary memory, as a page read live always was. The process holding them is the helper or the session, and one that can read their memory has the key anyway.
#[derive(Default)]
pub(crate) struct Plain(pub(crate) Vec<u8>);

impl std::ops::Deref for Plain {
    type Target = Vec<u8>;
    fn deref(&self) -> &Vec<u8> {
        &self.0
    }
}

impl std::ops::DerefMut for Plain {
    fn deref_mut(&mut self) -> &mut Vec<u8> {
        &mut self.0
    }
}

impl Drop for Plain {
    fn drop(&mut self) {
        zeroize::Zeroize::zeroize(&mut self.0);
    }
}

/// It didn't open: the wrong key, a changed byte, a chunk cut short or put where another belongs. All the same to a reader, which throws the corpus away.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Unopened;

/// One corpus's key, ready to seal and open its chunks. Its key schedule is overwritten when it goes (`aes-gcm`'s `zeroize`).
pub struct CorpusSeal {
    cipher: Aes256Gcm,
}

impl std::fmt::Debug for CorpusSeal {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        out.write_str("CorpusSeal")
    }
}

/// A corpus's root key: derived from a session's key, or a bound key's own bytes when it's bound to this corpus. `None` for a key bound to another corpus. The caller overwrites it when done.
fn corpus_root(find_key: &FindKey, corpus: CorpusKey) -> Option<[u8; 32]> {
    match find_key.binding() {
        Some(bound) if bound == corpus.as_bytes() => Some(*find_key.bytes()),
        Some(_) => None,
        None => {
            let mut root = [0u8; 32];
            let hkdf = hkdf::Hkdf::<Sha256>::new(None, find_key.bytes());
            // 32 bytes is far inside HKDF-SHA256's limit of 8,160.
            let _ = hkdf.expand_multi_info(&[ROOT_INFO, corpus.as_bytes()], &mut root);
            Some(root)
        }
    }
}

/// Whether a key may seal or open `corpus`: a session's key, or one bound to that corpus. Callers that would clear or write a corpus ask first, so a key bound to another corpus touches nothing of this one (the second security read's item 3).
#[must_use]
pub fn may_key(find_key: &FindKey, corpus: CorpusKey) -> bool {
    find_key.binding().is_none_or(|bound| bound == corpus.as_bytes())
}

/// **What the indexer is handed for one corpus:** a key bound to it, from the session's key, which seals and opens that corpus and no other ([`recto_platform::find_key::FindKey::bound`]). `None` when the key given is itself bound to another corpus.
#[must_use]
pub fn grant_for(find_key: &FindKey, corpus: CorpusKey) -> Option<FindKey> {
    let mut root = corpus_root(find_key, corpus)?;
    let grant = FindKey::bound(&root, find_key.source(), *corpus.as_bytes());
    zeroize::Zeroize::zeroize(&mut root);
    Some(grant)
}

impl CorpusSeal {
    /// A corpus's seal for a salt, through the corpus's root key ([`corpus_root`]). `None` when the key is bound to another corpus.
    fn derive(find_key: &FindKey, salt: &[u8], corpus: CorpusKey) -> Option<Self> {
        let mut root = corpus_root(find_key, corpus)?;
        let mut okm = [0u8; 32];
        let hkdf = hkdf::Hkdf::<Sha256>::new(Some(salt), &root);
        // 32 bytes is far inside HKDF-SHA256's limit of 8,160.
        let _ = hkdf.expand_multi_info(&[INFO, corpus.as_bytes()], &mut okm);
        let cipher = Aes256Gcm::new(&okm.into());
        zeroize::Zeroize::zeroize(&mut okm);
        zeroize::Zeroize::zeroize(&mut root);
        Some(Self { cipher })
    }

    /// A new corpus's seal, and its header to be written before its first chunk.
    ///
    /// # Errors
    ///
    /// When the OS gives no randomness.
    pub fn create(find_key: &FindKey, corpus: CorpusKey) -> std::io::Result<(Self, Vec<u8>)> {
        let mut salt = [0u8; SALT_BYTES];
        getrandom::fill(&mut salt).map_err(|_| std::io::Error::other("no randomness"))?;
        let seal = Self::derive(find_key, &salt, corpus).ok_or_else(|| std::io::Error::other("a key bound to another corpus"))?;
        let lasting = find_key.source().lasts();
        let mut header = Vec::with_capacity(HEADER_BYTES);
        header.extend_from_slice(MAGIC);
        header.push(u8::from(lasting));
        header.extend_from_slice(&salt);
        let mut check = Vec::with_capacity(OVERHEAD);
        seal.seal(Purpose::Check(lasting), &mut check, |_| {})?;
        header.extend_from_slice(&check);
        Ok((seal, header))
    }

    /// The seal a header says, if this key opens it for this corpus.
    ///
    /// # Errors
    ///
    /// [`Unopened`] for anything else: the wrong key, another corpus's header, or bytes that aren't one.
    pub fn open(find_key: &FindKey, corpus: CorpusKey, header: &[u8]) -> Result<Self, Unopened> {
        let lasting = Self::lasting(header).ok_or(Unopened)?;
        let seal = Self::derive(find_key, &header[SALT_AT..SALT_AT + SALT_BYTES], corpus).ok_or(Unopened)?;
        let mut check = [0u8; OVERHEAD];
        check.copy_from_slice(&header[SALT_AT + SALT_BYTES..]);
        seal.open_in_place(Purpose::Check(lasting), &mut check)?;
        Ok(seal)
    }

    /// Whether a header says its corpus was sealed with a key that lasts past its run. Read without the key, so it's only a hint, for which corpora a run with a key in memory may sweep away: changing it can only get a corpus thrown away, which anyone who can write here could do anyway.
    pub fn lasting(header: &[u8]) -> Option<bool> {
        if header.len() != HEADER_BYTES || !header.starts_with(MAGIC) {
            return None;
        }
        match header[MAGIC.len()] {
            0 => Some(false),
            1 => Some(true),
            _ => None,
        }
    }

    /// Seals what `write` puts in `out`, in place: `out` is cleared, room is left for the nonce, `write` appends the plain bytes, and they're encrypted where they lie. If sealing fails, `out`'s whole room is overwritten before the error comes back, so no plain bytes are left in it. What `write` copied them from is the caller's to wipe ([`Plain`]).
    ///
    /// # Errors
    ///
    /// When the OS gives no randomness, or the chunk is past what GCM can seal (64 GiB, far past [`crate::corpus`]'s caps).
    pub fn seal(&self, purpose: Purpose, out: &mut Vec<u8>, write: impl FnOnce(&mut Vec<u8>)) -> std::io::Result<()> {
        out.clear();
        out.resize(NONCE_BYTES, 0);
        write(out);
        let sealed = self.seal_in_place(purpose, out);
        if sealed.is_err() {
            zeroize::Zeroize::zeroize(out);
        }
        sealed
    }

    fn seal_in_place(&self, purpose: Purpose, out: &mut Vec<u8>) -> std::io::Result<()> {
        let (nonce, plain) = out.split_at_mut(NONCE_BYTES);
        getrandom::fill(nonce).map_err(|_| std::io::Error::other("no randomness"))?;
        let nonce = Nonce::try_from(&*nonce).map_err(|_| std::io::Error::other("nonce"))?;
        let tag = self
            .cipher
            .encrypt_inout_detached(&nonce, &purpose.associated(), plain.into())
            .map_err(|_| std::io::Error::other("too large to seal"))?;
        out.extend_from_slice(&tag);
        Ok(())
    }

    /// Opens a chunk where it lies, and says where its plain bytes are in it. A chunk that doesn't open is left as it was, and nothing of it is plain.
    ///
    /// # Errors
    ///
    /// [`Unopened`].
    pub fn open_in_place(&self, purpose: Purpose, chunk: &mut [u8]) -> Result<std::ops::Range<usize>, Unopened> {
        if chunk.len() < OVERHEAD {
            return Err(Unopened);
        }
        let end = chunk.len() - TAG_BYTES;
        let (head, tag) = chunk.split_at_mut(end);
        let tag = Tag::try_from(&*tag).map_err(|_| Unopened)?;
        let (nonce, sealed) = head.split_at_mut(NONCE_BYTES);
        let nonce = Nonce::try_from(&*nonce).map_err(|_| Unopened)?;
        self.cipher
            .decrypt_inout_detached(&nonce, &purpose.associated(), sealed.into(), &tag)
            .map_err(|_| Unopened)?;
        Ok(NONCE_BYTES..end)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn corpus(byte: u8) -> CorpusKey {
        CorpusKey::from_hex(&format!("{byte:02x}").repeat(32)).unwrap()
    }

    fn sealed(seal: &CorpusSeal, purpose: Purpose, text: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        seal.seal(purpose, &mut out, |out| out.extend_from_slice(text)).unwrap();
        out
    }

    /// Failure (the security read of 2026-11-15, S1): what the indexer is handed for one corpus opens another, so a helper put in the app's place gets every corpus's text from one build; or it opens nothing of its own corpus, made by the session or by itself, so a build's corpus is never read; or it carries the session's key. A grant for X opens X's header both ways round and seals what the session reads, and opens nothing of Y, made by the session or by a grant for Y.
    #[test]
    fn a_grant_opens_its_own_corpus_and_no_other() {
        let session = FindKey::for_tests(1);
        let (x, y) = (corpus(1), corpus(2));
        let grant = grant_for(&session, x).unwrap();
        assert_eq!(grant.binding(), Some(x.as_bytes()));
        assert_ne!(grant.bytes(), session.bytes(), "the grant is the session's key");
        let (session_seal, session_header) = CorpusSeal::create(&session, x).unwrap();
        let (grant_seal, grant_header) = CorpusSeal::create(&grant, x).unwrap();
        assert!(
            CorpusSeal::open(&grant, x, &session_header).is_ok(),
            "the grant doesn't open its corpus as the session made it"
        );
        let reopened = CorpusSeal::open(&session, x, &grant_header).expect("the session doesn't open what the grant made");
        let mut chunk = sealed(&grant_seal, Purpose::Page(0), b"the page");
        let opened = reopened.open_in_place(Purpose::Page(0), &mut chunk).unwrap();
        assert_eq!(&chunk[opened], b"the page");
        drop(session_seal);
        let (_, other_header) = CorpusSeal::create(&session, y).unwrap();
        assert!(CorpusSeal::open(&grant, y, &other_header).is_err(), "a grant for X opened Y");
        assert!(CorpusSeal::create(&grant, y).is_err(), "a grant for X sealed Y");
        assert!(grant_for(&grant, y).is_none(), "a grant for X gave one for Y");
        let (_, y_by_its_grant) = CorpusSeal::create(&grant_for(&session, y).unwrap(), y).unwrap();
        assert!(
            CorpusSeal::open(&grant, y, &y_by_its_grant).is_err(),
            "a grant for X opened what Y's grant made"
        );
        assert!(CorpusSeal::open(&grant, x, &other_header).is_err(), "Y's header opened as X's");
    }

    /// Failure: a page's text is on disk as it is, or comes back other than it went in, or a sealed chunk is longer than its text and the 28 bytes said.
    #[test]
    fn a_chunk_hides_its_text_and_gives_it_back() {
        let key = FindKey::for_tests(1);
        let (seal, header) = CorpusSeal::create(&key, corpus(1)).unwrap();
        assert_eq!(header.len(), HEADER_BYTES);
        let text = b"force majeure clause: the parties agree";
        let mut chunk = sealed(&seal, Purpose::Page(3), text);
        assert_eq!(chunk.len(), text.len() + OVERHEAD);
        assert!(
            !chunk.windows(8).any(|window| text.windows(8).any(|word| word == window)),
            "plain text in a sealed chunk"
        );
        let opened = seal.open_in_place(Purpose::Page(3), &mut chunk).unwrap();
        assert_eq!(&chunk[opened], text);
        let reopened = CorpusSeal::open(&key, corpus(1), &header).unwrap();
        let mut again = sealed(&seal, Purpose::Page(0), b"");
        assert!(
            reopened.open_in_place(Purpose::Page(0), &mut again).is_ok(),
            "the header's seal can't open what the made seal sealed"
        );
    }

    /// Failure: a chunk opens for what it isn't: with a byte changed anywhere, cut short, as another page, as the summary, under another file's key, under another Find key, or with a header that isn't this corpus's.
    #[test]
    fn anything_but_the_chunk_itself_is_refused() {
        let key = FindKey::for_tests(1);
        let (seal, header) = CorpusSeal::create(&key, corpus(1)).unwrap();
        let chunk = sealed(&seal, Purpose::Page(7), b"page seven's text");
        for at in 0..chunk.len() {
            let mut bad = chunk.clone();
            bad[at] ^= 0x40;
            assert_eq!(
                seal.open_in_place(Purpose::Page(7), &mut bad),
                Err(Unopened),
                "a byte changed at {at} still opened"
            );
        }
        for len in 0..chunk.len() {
            assert_eq!(
                seal.open_in_place(Purpose::Page(7), &mut chunk[..len].to_vec()),
                Err(Unopened),
                "cut to {len} still opened"
            );
        }
        assert_eq!(
            seal.open_in_place(Purpose::Page(8), &mut chunk.clone()),
            Err(Unopened),
            "opened as another page"
        );
        assert_eq!(seal.open_in_place(Purpose::Summary, &mut chunk.clone()), Err(Unopened), "opened as the summary");
        assert!(CorpusSeal::open(&key, corpus(2), &header).is_err(), "another file's corpus opened this header");
        assert!(
            CorpusSeal::open(&FindKey::for_tests(2), corpus(1), &header).is_err(),
            "another Find key opened this header"
        );
        let mut short = header.clone();
        short.pop();
        assert!(CorpusSeal::open(&key, corpus(1), &short).is_err());
        let mut magic = header.clone();
        magic[0] ^= 1;
        assert!(CorpusSeal::open(&key, corpus(1), &magic).is_err());
        let mut lasting = header.clone();
        lasting[8] ^= 1;
        assert!(
            CorpusSeal::open(&key, corpus(1), &lasting).is_err(),
            "a header's word on its key was changed and it still opened"
        );
    }

    /// Failure: two corpora, or the same one started again from nothing, get the same key, so a chunk of one opens in the other; or two sealings of the same text match, which says the text is the same and, under GCM, a nonce used twice.
    #[test]
    fn every_corpus_and_every_chunk_is_its_own() {
        let key = FindKey::for_tests(1);
        let (first, _) = CorpusSeal::create(&key, corpus(1)).unwrap();
        let (again, _) = CorpusSeal::create(&key, corpus(1)).unwrap();
        let chunk = sealed(&first, Purpose::Page(0), b"the same text");
        assert_eq!(
            again.open_in_place(Purpose::Page(0), &mut chunk.clone()),
            Err(Unopened),
            "a rebuilt corpus shares its key"
        );
        let mut nonces = std::collections::HashSet::new();
        for _ in 0..10_000 {
            let chunk = sealed(&first, Purpose::Page(0), b"the same text");
            assert!(nonces.insert(chunk[..NONCE_BYTES].to_vec()), "a nonce came twice");
        }
    }

    /// What sealing and opening cost on this Mac, by hand (`--ignored`, release): 37 MB of text in 3.7 KB pages, as the made 10,000-page PDF's pack, each page sealed and opened in one reused buffer.
    #[test]
    #[ignore = "a measurement, by hand"]
    fn what_sealing_costs() {
        let key = FindKey::for_tests(1);
        let (seal, _) = CorpusSeal::create(&key, corpus(1)).unwrap();
        let page = vec![b'a'; 3_700];
        let pages = 10_000u32;
        let mut chunks = Vec::with_capacity(pages as usize);
        let started = std::time::Instant::now();
        for number in 0..pages {
            chunks.push(sealed(&seal, Purpose::Page(number), &page));
        }
        let sealing = started.elapsed();
        let mut buffer = Vec::new();
        let started = std::time::Instant::now();
        for (number, chunk) in chunks.iter().enumerate() {
            buffer.clear();
            buffer.extend_from_slice(chunk);
            seal.open_in_place(Purpose::Page(number as u32), &mut buffer).unwrap();
        }
        let opening = started.elapsed();
        let started = std::time::Instant::now();
        for _ in 0..1_000 {
            let (_, header) = CorpusSeal::create(&key, corpus(1)).unwrap();
            CorpusSeal::open(&key, corpus(1), &header).unwrap();
        }
        let per_open = started.elapsed() / 1_000;
        let megabytes = f64::from(pages) * 3_700.0 / 1e6;
        println!(
            "sealed {megabytes:.1} MB in {sealing:?} ({:.0} MB/s), opened in {opening:?} ({:.0} MB/s); a corpus's seal made and opened in {per_open:?}",
            megabytes / sealing.as_secs_f64(),
            megabytes / opening.as_secs_f64()
        );
    }
}
