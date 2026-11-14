//! **The file's highlights a worker leaves out of what it draws** (review of R4.3, S2): a tab's removals of its file's own highlights, not yet saved.
//! The host sends a document's whole set ahead of a job whenever it's another than the worker holds; the worker hides each one that is what the set
//! says (a `/Highlight` at that place in the page's `/Annots`, with that `/Rect` within [`RECT_SLACK`]) in the document as it holds it in memory, and
//! never writes anything. One that isn't is drawn as the file has it, and the answer counts it.
//!
//! The host's frame is as hostile to a worker as a worker's is to the host: both ends check it, held to [`MAX_HIDDEN_ANNOTS`].

use super::{Kind, ProtoError, Reader, frame};

/// Most highlights one set hides: what a document may hold taken out and unsaved.
pub const MAX_HIDDEN_ANNOTS: usize = 10_000;
/// How far an annotation's `/Rect` may be from the set's and still be the one it names, in points.
pub const RECT_SLACK: f32 = 0.5;
/// Largest edge a set's rectangle may have, in points.
const MAX_EDGE: f32 = 200_000.0;
const HEAD_LEN: usize = 12;
const PAGE_LEN: usize = 8;
const ENTRY_LEN: usize = 20;
/// The longest a set's frame can be.
pub const MAX_HIDDEN_FRAME: u32 = (HEAD_LEN + MAX_HIDDEN_ANNOTS * (PAGE_LEN + ENTRY_LEN)) as u32;
/// The answer's length.
pub const HIDDEN_TAKEN_LEN: u32 = 12;

/// One page's highlights to hide: the page, and each one's place in its `/Annots` and `/Rect`, `[left, bottom, right, top]`.
pub type HiddenPage = (u32, Vec<(u32, [f32; 4])>);

/// **A document's set,** whole: `generation` names it, and goes up when it changes.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct HiddenAnnots {
    pub generation: u64,
    pub pages: Vec<HiddenPage>,
}

impl HiddenAnnots {
    /// How many highlights it hides.
    pub fn len(&self) -> usize {
        self.pages.iter().map(|(_, entries)| entries.len()).sum()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Within the cap, each page named once with at least one entry, each rectangle finite and within bounds.
    pub fn check(&self) -> Result<(), ProtoError> {
        if self.len() > MAX_HIDDEN_ANNOTS {
            return Err(ProtoError("too many hidden highlights"));
        }
        let mut pages: Vec<u32> = self.pages.iter().map(|(page, _)| *page).collect();
        pages.sort_unstable();
        if pages.windows(2).any(|pair| pair[0] == pair[1]) || self.pages.iter().any(|(_, entries)| entries.is_empty()) {
            return Err(ProtoError("a hidden highlights' page named twice, or with none"));
        }
        let sound = |rect: &[f32; 4]| rect.iter().all(|edge| edge.is_finite() && edge.abs() <= MAX_EDGE);
        if !self.pages.iter().flat_map(|(_, entries)| entries).all(|(_, rect)| sound(rect)) {
            return Err(ProtoError("a hidden highlight's rectangle isn't one"));
        }
        Ok(())
    }

    pub fn encode(&self) -> Result<Vec<u8>, ProtoError> {
        self.check()?;
        let mut p = Vec::with_capacity(HEAD_LEN + self.pages.len() * PAGE_LEN + self.len() * ENTRY_LEN);
        p.extend_from_slice(&self.generation.to_le_bytes());
        p.extend_from_slice(&u32::try_from(self.pages.len()).map_err(|_| ProtoError("too many pages"))?.to_le_bytes());
        for (page, entries) in &self.pages {
            p.extend_from_slice(&page.to_le_bytes());
            p.extend_from_slice(&u32::try_from(entries.len()).map_err(|_| ProtoError("too many entries"))?.to_le_bytes());
            for (index, rect) in entries {
                p.extend_from_slice(&index.to_le_bytes());
                rect.iter().for_each(|edge| p.extend_from_slice(&edge.to_le_bytes()));
            }
        }
        Ok(frame(Kind::HiddenAnnots, &p))
    }

    pub fn decode(payload: &[u8]) -> Result<Self, ProtoError> {
        let mut r = Reader(payload);
        let generation = r.u64()?;
        let count = r.u32()? as usize;
        if count > MAX_HIDDEN_ANNOTS {
            return Err(ProtoError("too many hidden highlights"));
        }
        // Held to what the payload can hold, so a count it can't back takes nothing.
        let mut pages = Vec::with_capacity(count.min(payload.len() / PAGE_LEN));
        let mut total = 0usize;
        for _ in 0..count {
            let page = r.u32()?;
            let entries = r.u32()? as usize;
            total = total.saturating_add(entries);
            if total > MAX_HIDDEN_ANNOTS {
                return Err(ProtoError("too many hidden highlights"));
            }
            let mut read = Vec::with_capacity(entries.min(r.0.len() / ENTRY_LEN));
            for _ in 0..entries {
                read.push((r.u32()?, [r.f32()?, r.f32()?, r.f32()?, r.f32()?]));
            }
            pages.push((page, read));
        }
        r.end()?;
        let set = Self { generation, pages };
        set.check()?;
        Ok(set)
    }
}

/// **The worker's answer:** the set it now holds, and how many of its highlights weren't what it said (another annotation there, another
/// `/Rect`, a place past the list, a page the document hasn't), which are drawn as the file has them.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct HiddenTaken {
    pub generation: u64,
    pub unmatched: u32,
}

impl HiddenTaken {
    pub fn encode(&self) -> Vec<u8> {
        let mut p = [0; HIDDEN_TAKEN_LEN as usize];
        p[..8].copy_from_slice(&self.generation.to_le_bytes());
        p[8..].copy_from_slice(&self.unmatched.to_le_bytes());
        frame(Kind::HiddenTaken, &p)
    }

    pub fn decode(payload: &[u8]) -> Result<Self, ProtoError> {
        let mut r = Reader(payload);
        let taken = Self {
            generation: r.u64()?,
            unmatched: r.u32()?,
        };
        r.end()?;
        Ok(taken)
    }
}

/// Whether an annotation's `/Rect`, as PDFium gives it, is the set's.
pub fn same_rect(there: [f32; 4], said: [f32; 4]) -> bool {
    let normal = |[a, b, c, d]: [f32; 4]| [a.min(c), b.min(d), a.max(c), b.max(d)];
    normal(there).iter().zip(normal(said)).all(|(there, said)| (there - said).abs() <= RECT_SLACK)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set(entries: usize) -> HiddenAnnots {
        HiddenAnnots {
            generation: 7,
            pages: vec![(3, vec![(2, [1.0, 2.0, 3.0, 4.0]); entries])],
        }
    }

    /// Failure: a set a hostile end sends past the cap, with a NaN, a page twice, a page with none, or cut short, is taken; or one within them
    /// doesn't come back as it was sent. Held at the cap and the cap plus one.
    #[test]
    fn a_hidden_set_is_checked_at_both_ends() {
        let at_cap = set(MAX_HIDDEN_ANNOTS);
        let bytes = at_cap.encode().unwrap();
        assert!(bytes.len() - super::super::HEADER_LEN <= MAX_HIDDEN_FRAME as usize);
        assert_eq!(HiddenAnnots::decode(&bytes[super::super::HEADER_LEN..]).unwrap(), at_cap);
        assert!(set(MAX_HIDDEN_ANNOTS + 1).encode().is_err());
        let mut forged = set(1).encode().unwrap()[super::super::HEADER_LEN..].to_vec();
        forged[12 + 4..12 + 8].copy_from_slice(&(MAX_HIDDEN_ANNOTS as u32 + 1).to_le_bytes());
        assert!(HiddenAnnots::decode(&forged).is_err(), "a count past the cap");
        let mut nan = set(1);
        nan.pages[0].1[0].1[0] = f32::NAN;
        assert!(nan.encode().is_err());
        let twice = HiddenAnnots {
            generation: 1,
            pages: vec![(1, vec![(0, [0.0; 4])]), (1, vec![(1, [0.0; 4])])],
        };
        assert!(twice.check().is_err());
        assert!(
            HiddenAnnots {
                generation: 1,
                pages: vec![(1, vec![])]
            }
            .check()
            .is_err()
        );
        let whole = set(2).encode().unwrap();
        assert!(HiddenAnnots::decode(&whole[super::super::HEADER_LEN..whole.len() - 1]).is_err(), "cut short");
        let taken = HiddenTaken { generation: 9, unmatched: 1 };
        assert_eq!(HiddenTaken::decode(&taken.encode()[super::super::HEADER_LEN..]), Ok(taken));
        assert!(same_rect([10.0, 20.0, 30.0, 40.0], [10.4, 19.6, 30.0, 40.0]));
        assert!(!same_rect([10.0, 20.0, 30.0, 40.0], [11.0, 20.0, 30.0, 40.0]));
    }
}
