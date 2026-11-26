//! What goes beside a page's text in its chunk (spellings and hidden spans): written by a build, read back by a search.

/// What goes beside a page's text: `u16` count of spellings, each a `u16` length and its bytes, then a `u32` count of spans of two `u32`s. Checked when read, because a file on disk can say anything.
#[derive(Debug, Default, PartialEq, Eq)]
pub(super) struct Aux<'a> {
    pub(super) spelled: Vec<&'a str>,
    pub(super) spans: Vec<(u32, u32)>,
}

impl<'a> Aux<'a> {
    pub(super) fn write(spelled: &[String], spans: &[(u32, u32)], out: &mut Vec<u8>) {
        if spelled.is_empty() && spans.is_empty() {
            return;
        }
        let spelled = &spelled[..spelled.len().min(usize::from(u16::MAX))];
        out.extend_from_slice(&(spelled.len() as u16).to_le_bytes());
        for word in spelled {
            let bytes = &word.as_bytes()[..word.floor_char_boundary(usize::from(u16::MAX))];
            out.extend_from_slice(&(bytes.len() as u16).to_le_bytes());
            out.extend_from_slice(bytes);
        }
        out.extend_from_slice(&(spans.len() as u32).to_le_bytes());
        for span in spans {
            out.extend_from_slice(&span.0.to_le_bytes());
            out.extend_from_slice(&span.1.to_le_bytes());
        }
    }

    pub(super) fn read(mut bytes: &'a [u8]) -> Option<Self> {
        let mut aux = Self::default();
        if bytes.is_empty() {
            return Some(aux);
        }
        let take = |bytes: &mut &'a [u8], len: usize| {
            if bytes.len() < len {
                return None;
            }
            let (now, rest) = bytes.split_at(len);
            *bytes = rest;
            Some(now)
        };
        let count = u16::from_le_bytes(take(&mut bytes, 2)?.try_into().ok()?);
        for _ in 0..count {
            let len = u16::from_le_bytes(take(&mut bytes, 2)?.try_into().ok()?);
            aux.spelled.push(std::str::from_utf8(take(&mut bytes, usize::from(len))?).ok()?);
        }
        let count = u32::from_le_bytes(take(&mut bytes, 4)?.try_into().ok()?) as usize;
        if bytes.len() != count.checked_mul(8)? {
            return None;
        }
        aux.spans = bytes
            .as_chunks::<8>()
            .0
            .iter()
            .filter_map(|pair| Some((u32::from_le_bytes(pair[..4].try_into().ok()?), u32::from_le_bytes(pair[4..].try_into().ok()?))))
            .collect();
        Some(aux)
    }
}
