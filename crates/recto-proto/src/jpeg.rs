//! A JPEG's structure, read as bytes arrive, holding at most one metadata segment of 64 KiB. Used to know a file's stored bytes really are a JPEG, to say what they carry besides the picture, and, when asked, to take that out with the compressed picture left byte for byte as it was. Total on arbitrary bytes.

use crate::ProtoError;

/// What a JPEG carries besides the picture.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct JpegFacts {
    pub exif: u32,
    /// EXIF blocks with a GPS section.
    pub gps: u32,
    /// Thumbnails, in EXIF or JFIF: small copies of the picture, which may be from before it was cropped.
    pub thumbnails: u32,
    pub xmp: u32,
    /// Embedded colour profiles.
    pub color_profiles: u32,
    pub comments: u32,
    /// Any other application segment.
    pub other_app: u32,
    /// A colour profile that isn't one sound sRGB profile, which a reader would apply and a page takes no notice of.
    pub foreign_profile: bool,
    /// Bytes after the end of the picture.
    pub trailing_bytes: u64,
    /// What EXIF says about turning it, 0 if nothing. A PDF takes no notice of this and browsers do, so it changes the picture.
    pub orientation: u16,
}

impl JpegFacts {
    pub fn add(&mut self, other: &Self) {
        self.exif += other.exif;
        self.gps += other.gps;
        self.thumbnails += other.thumbnails;
        self.xmp += other.xmp;
        self.color_profiles += other.color_profiles;
        self.comments += other.comments;
        self.other_app += other.other_app;
        self.foreign_profile |= other.foreign_profile;
        self.trailing_bytes += other.trailing_bytes;
    }

    /// Whether a reader of the bytes would turn this, where the page doesn't.
    pub fn is_turned(&self) -> bool {
        self.orientation > 1
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum State {
    Soi0,
    Soi1,
    MarkerStart,
    MarkerCode,
    LengthHigh,
    LengthLow,
    /// Goes straight through.
    Segment,
    /// Held, to be looked at and kept or not.
    Metadata,
    Entropy,
    EntropyMarker,
    Trailing,
}

/// A JPEG being read.
#[derive(Clone, Debug)]
pub struct JpegScan {
    state: State,
    code: u8,
    high: u8,
    left: usize,
    held: Vec<u8>,
    strip: bool,
    changed: bool,
    seen_frame: bool,
    seen_scan: bool,
    /// The next segment of a colour profile expected, and how many it said there'd be.
    profile_next: u8,
    profile_parts: u8,
    facts: JpegFacts,
}

const BAD: ProtoError = ProtoError("a JPEG without a JPEG's structure");

impl JpegScan {
    /// With `strip`, [`Self::feed`] writes out the JPEG without what doesn't affect the picture: see [`Self::keeps`].
    pub fn new(strip: bool) -> Self {
        Self { state: State::Soi0, code: 0, high: 0, left: 0, held: Vec::new(), strip, changed: false, seen_frame: false, seen_scan: false, profile_next: 1, profile_parts: 0, facts: JpegFacts::default() }
    }

    pub fn facts(&self) -> JpegFacts {
        self.facts
    }

    /// Whether what `feed` wrote differs from what it was given.
    pub fn changed(&self) -> bool {
        self.changed
    }

    /// Reads the next bytes. In `strip` mode, what's kept goes on the end of `out`, which is otherwise left alone.
    pub fn feed(&mut self, mut bytes: &[u8], out: &mut Vec<u8>) -> Result<(), ProtoError> {
        while let Some(&byte) = bytes.first() {
            match self.state {
                State::Soi0 | State::Soi1 => {
                    let want = if self.state == State::Soi0 { 0xFF } else { 0xD8 };
                    if byte != want {
                        return Err(ProtoError("a JPEG that doesn't start as one"));
                    }
                    self.put(out, &[byte]);
                    self.state = if self.state == State::Soi0 { State::Soi1 } else { State::MarkerStart };
                    bytes = &bytes[1..];
                }
                State::MarkerStart => {
                    if byte != 0xFF {
                        return Err(if self.seen_frame { BAD } else { ProtoError("a JPEG that doesn't start as one") });
                    }
                    self.state = State::MarkerCode;
                    bytes = &bytes[1..];
                }
                State::MarkerCode => {
                    self.marker(byte, out)?;
                    bytes = &bytes[1..];
                }
                State::LengthHigh => {
                    self.high = byte;
                    self.state = State::LengthLow;
                    bytes = &bytes[1..];
                }
                State::LengthLow => {
                    let length = usize::from(self.high) << 8 | usize::from(byte);
                    self.left = length.checked_sub(2).ok_or(BAD)?;
                    if is_metadata(self.code) {
                        self.held.clear();
                        self.state = State::Metadata;
                    } else {
                        self.put(out, &[0xFF, self.code, self.high, byte]);
                        self.state = State::Segment;
                    }
                    bytes = &bytes[1..];
                    if self.left == 0 {
                        self.end_segment(out);
                    }
                }
                State::Segment => {
                    let take = self.left.min(bytes.len());
                    self.put(out, &bytes[..take]);
                    self.left -= take;
                    bytes = &bytes[take..];
                    if self.left == 0 {
                        self.end_segment(out);
                    }
                }
                State::Metadata => {
                    let take = self.left.min(bytes.len());
                    self.held.extend_from_slice(&bytes[..take]);
                    self.left -= take;
                    bytes = &bytes[take..];
                    if self.left == 0 {
                        self.end_segment(out);
                    }
                }
                State::Entropy => {
                    let run = bytes.iter().position(|value| *value == 0xFF).unwrap_or(bytes.len());
                    self.put(out, &bytes[..run]);
                    bytes = &bytes[run..];
                    if !bytes.is_empty() {
                        self.state = State::EntropyMarker;
                        bytes = &bytes[1..];
                    }
                }
                State::EntropyMarker => {
                    match byte {
                        0x00 | 0xD0..=0xD7 => {
                            self.put(out, &[0xFF, byte]);
                            self.state = State::Entropy;
                        }
                        // A fill byte before a marker.
                        0xFF => self.put(out, &[0xFF]),
                        code => self.marker(code, out)?,
                    }
                    bytes = &bytes[1..];
                }
                State::Trailing => {
                    self.facts.trailing_bytes += bytes.len() as u64;
                    self.changed |= self.strip;
                    if !self.strip {
                        self.put(out, bytes);
                    }
                    return Ok(());
                }
            }
        }
        Ok(())
    }

    /// Checks that it all came.
    pub fn finish(&self) -> Result<(), ProtoError> {
        if self.state == State::Trailing { Ok(()) } else { Err(ProtoError("a JPEG with no end")) }
    }

    fn put(&self, out: &mut Vec<u8>, bytes: &[u8]) {
        if self.strip {
            out.extend_from_slice(bytes);
        }
    }

    fn marker(&mut self, code: u8, out: &mut Vec<u8>) -> Result<(), ProtoError> {
        match code {
            // A fill byte before a marker. What's written has none.
            0xFF => self.changed |= self.strip,
            0xD9 => {
                if !self.seen_scan {
                    return Err(BAD);
                }
                self.put(out, &[0xFF, 0xD9]);
                self.state = State::Trailing;
            }
            // Only ever inside a picture's data, or at its very start.
            0x00 | 0x01 | 0xD0..=0xD8 => return Err(BAD),
            _ => {
                // What a browser's decoder reads and nothing else: a marker it skips could hold anything, and a frame it can't decode isn't a picture to it.
                if !matches!(code, 0xC0..=0xC2 | 0xC4 | 0xCC | 0xDA..=0xDD | 0xE0..=0xEF | 0xFE) {
                    return Err(ProtoError("a JPEG with a marker that isn't known"));
                }
                if matches!(code, 0xC0..=0xC2) {
                    self.seen_frame = true;
                }
                if code == 0xDA {
                    if !self.seen_frame {
                        return Err(BAD);
                    }
                    self.seen_scan = true;
                }
                self.code = code;
                self.state = State::LengthHigh;
            }
        }
        Ok(())
    }

    fn end_segment(&mut self, out: &mut Vec<u8>) {
        if self.state == State::Metadata {
            let held = std::mem::take(&mut self.held);
            let kept = self.look_at(&held);
            if !self.strip || kept {
                self.put(out, &[0xFF, self.code]);
                // JFIF can hold a thumbnail and an Adobe marker anything at all after what it says, and their own fields are all that's wanted of them.
                let is_jfif = held.starts_with(b"JFIF\0");
                let keep = if !self.strip {
                    held.len()
                } else if is_jfif {
                    held.len().min(14)
                } else if self.code == 0xEE {
                    held.len().min(ADOBE_LEN)
                } else {
                    held.len()
                };
                let body = &held[..keep];
                self.changed |= self.strip && keep != held.len();
                self.put(out, &((keep + 2) as u16).to_be_bytes());
                if self.strip && is_jfif && keep != held.len() {
                    self.put(out, &body[..12]);
                    self.put(out, &[0, 0]);
                } else {
                    self.put(out, body);
                }
            } else {
                self.changed = true;
            }
            self.state = State::MarkerStart;
        } else {
            self.state = if self.code == 0xDA { State::Entropy } else { State::MarkerStart };
        }
    }

    /// Counts what's in a segment, and says whether it's kept when stripping. Kept: JFIF, an Adobe marker and a colour profile, because a decoder reads the first two to know the picture's colours and the third says what they mean. A JFIF thumbnail goes.
    fn look_at(&mut self, body: &[u8]) -> bool {
        match self.code {
            0xE0 if body.starts_with(b"JFIF\0") => {
                self.facts.thumbnails += u32::from(body.len() > 14);
                true
            }
            0xE0 if body.starts_with(b"JFXX\0") => {
                self.facts.thumbnails += 1;
                false
            }
            0xE1 if body.starts_with(b"Exif\0\0") => {
                self.facts.exif += 1;
                let exif = read_exif(&body[6..]);
                self.facts.gps += u32::from(exif.gps);
                self.facts.thumbnails += u32::from(exif.thumbnail);
                // More than one can say, and any that says to turn it may be the one a reader goes by.
                self.facts.orientation = self.facts.orientation.max(exif.orientation);
                false
            }
            0xE1 if body.starts_with(b"http://ns.adobe.com/x") => {
                self.facts.xmp += 1;
                false
            }
            0xE2 if body.starts_with(b"ICC_PROFILE\0") => {
                let sound = self.profile_part(body);
                if !sound {
                    self.facts.foreign_profile = true;
                    self.facts.other_app += 1;
                }
                sound
            }
            0xEE if body.starts_with(b"Adobe") && body.len() >= ADOBE_LEN => true,
            0xED if body.starts_with(b"Photoshop 3.0\0") => {
                self.facts.other_app += 1;
                self.facts.thumbnails += u32::from(photoshop_has_thumbnail(&body[14..]));
                false
            }
            0xFE => {
                self.facts.comments += 1;
                false
            }
            _ => {
                self.facts.other_app += 1;
                false
            }
        }
    }
}

impl JpegScan {
    /// Whether this is the next part of a colour profile that holds together: numbered in order from 1 out of one total, starting with a profile's own signature. Anything else is left out when stripping, as a label on who knows what.
    fn profile_part(&mut self, body: &[u8]) -> bool {
        let (Some(&part), Some(&parts)) = (body.get(12), body.get(13)) else { return false };
        let data = &body[14..];
        let first = part == 1;
        if part == 0 || part > parts || part != self.profile_next || (!first && parts != self.profile_parts) || (first && data.get(36..40) != Some(b"acsp")) {
            return false;
        }
        self.profile_next = part.saturating_add(1);
        self.profile_parts = parts;
        if first {
            self.facts.color_profiles += 1;
            // One that takes several segments isn't sRGB's, which is about 3 KiB.
            self.facts.foreign_profile |= parts != 1 || !profile_says_srgb(data);
        }
        true
    }
}

/// The most a profile added to a JPEG may be. Larger than any RGB profile in use, and a page with more goes as PNG.
pub const MAX_ADDED_PROFILE: usize = 1024 * 1024;
/// How much of a profile one segment holds, by the convention for splitting them.
const PROFILE_PART: usize = 65_519;

/// What to correct around a JPEG's picture so that a reader takes it as the page does.
#[derive(Clone, Copy, Debug, Default)]
pub struct HeaderFix<'a> {
    /// Set EXIF to say upright, which is what the page draws.
    pub upright: bool,
    /// Take out its own colour profile.
    pub drop_profile: bool,
    /// Add this one.
    pub add_profile: Option<&'a [u8]>,
}

/// One segment of the header: its marker, and where it starts and ends, length included. `None` at the scan, or where it doesn't hold together.
fn next_segment(jpeg: &[u8], mut at: usize) -> Option<(u8, usize, usize)> {
    if *jpeg.get(at)? != 0xFF {
        return None;
    }
    while *jpeg.get(at + 1)? == 0xFF {
        at += 1;
    }
    let code = jpeg[at + 1];
    if code == 0xDA {
        return None;
    }
    let length = usize::from(*jpeg.get(at + 2)?) << 8 | usize::from(*jpeg.get(at + 3)?);
    if length < 2 {
        return None;
    }
    let end = at.checked_add(2)?.checked_add(length)?;
    (end <= jpeg.len()).then_some((code, at, end))
}

/// Where the first scan's marker is, from which on a JPEG is its compressed picture and what follows.
pub fn scan_start(jpeg: &[u8]) -> Option<usize> {
    if jpeg.get(..2) != Some(&[0xFF, 0xD8]) {
        return None;
    }
    let mut at = 2;
    loop {
        match next_segment(jpeg, at) {
            Some((_, _, end)) => at = end,
            None => {
                let mut marker = at;
                while *jpeg.get(marker + 1)? == 0xFF {
                    marker += 1;
                }
                return (*jpeg.get(marker)? == 0xFF && jpeg[marker + 1] == 0xDA).then_some(marker);
            }
        }
    }
}

/// How many colours its frame says it has.
pub fn frame_components(jpeg: &[u8]) -> Option<u8> {
    let mut at = 2;
    while let Some((code, start, end)) = next_segment(jpeg, at) {
        if matches!(code, 0xC0..=0xC2) {
            return jpeg.get(start + 9).copied();
        }
        at = end;
    }
    None
}

/// The same JPEG with its header corrected and everything from the first scan on, byte for byte, as it was. `None` if that can't be done, or what comes out doesn't read as what was asked: it's then not a JPEG to pass on.
pub fn fix_header(jpeg: &[u8], fix: &HeaderFix<'_>) -> Option<Vec<u8>> {
    let scan = scan_start(jpeg)?;
    let added = fix.add_profile.unwrap_or(&[]);
    if added.len() > MAX_ADDED_PROFILE {
        return None;
    }
    let parts = added.chunks(PROFILE_PART).len();
    let parts_u8 = u8::try_from(parts).ok()?;
    let mut out = Vec::with_capacity(jpeg.len() + added.len() + parts * 18);
    out.extend_from_slice(&jpeg[..2]);
    let mut at = 2;
    let mut profile_written = added.is_empty();
    let write_profile = |out: &mut Vec<u8>| {
        for (index, part) in added.chunks(PROFILE_PART).enumerate() {
            out.extend_from_slice(&[0xFF, 0xE2]);
            out.extend_from_slice(&((part.len() + 2 + 14) as u16).to_be_bytes());
            out.extend_from_slice(b"ICC_PROFILE\0");
            out.extend_from_slice(&[index as u8 + 1, parts_u8]);
            out.extend_from_slice(part);
        }
    };
    while at < scan {
        let (code, start, end) = next_segment(jpeg, at)?;
        // JFIF must stay first.
        if !profile_written && code != 0xE0 {
            write_profile(&mut out);
            profile_written = true;
        }
        let segment = &jpeg[start..end];
        let is_profile = code == 0xE2 && segment.get(4..).is_some_and(|body| body.starts_with(b"ICC_PROFILE\0"));
        if !(is_profile && fix.drop_profile) {
            let from = out.len();
            out.extend_from_slice(segment);
            if fix.upright && code == 0xE1 && segment.get(4..10) == Some(b"Exif\0\0") {
                set_upright(&mut out[from + 10..]);
            }
        }
        at = end;
    }
    if !profile_written {
        write_profile(&mut out);
    }
    out.extend_from_slice(&jpeg[scan..]);
    // Read back by the same parser: sound, and saying what was asked.
    let mut check = JpegScan::new(false);
    check.feed(&out, &mut Vec::new()).ok()?;
    check.finish().ok()?;
    let facts = check.facts();
    let as_asked = (!fix.upright || !facts.is_turned()) && (!fix.drop_profile || fix.add_profile.is_some() || facts.color_profiles == 0) && (fix.add_profile.is_none() || facts.color_profiles == 1);
    as_asked.then_some(out)
}

/// Sets the orientation tags in an EXIF block's first directory to upright. Anything that doesn't hold together is left alone, and the read back finds it.
fn set_upright(tiff: &mut [u8]) {
    let little = match tiff.get(..2) {
        Some(b"II") => true,
        Some(b"MM") => false,
        _ => return,
    };
    let word = |tiff: &[u8], at: usize| tiff.get(at..at.checked_add(2)?).map(|bytes| if little { u16::from_le_bytes([bytes[0], bytes[1]]) } else { u16::from_be_bytes([bytes[0], bytes[1]]) });
    let Some(first) = tiff.get(4..8).map(|bytes| if little { u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) } else { u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) } as usize) else { return };
    let Some(count) = word(tiff, first) else { return };
    for entry in 0..usize::from(count) {
        let at = first + 2 + entry * 12;
        if word(tiff, at) == Some(0x0112)
            && let Some(value) = tiff.get_mut(at + 8..at + 10)
        {
            value.copy_from_slice(&if little { 1u16.to_le_bytes() } else { 1u16.to_be_bytes() });
        }
    }
}

/// How much of an Adobe marker there is to it: its name, version, two sets of flags and the colour transform.
const ADOBE_LEN: usize = 12;

/// Whether an ICC profile's description says sRGB.
pub fn profile_says_srgb(profile: &[u8]) -> bool {
    let word = |at: usize| profile.get(at..at + 4).map(|bytes| u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) as usize);
    let Some(tags) = word(128) else { return false };
    for tag in 0..tags.min(256) {
        let entry = 132 + tag * 12;
        if profile.get(entry..entry + 4) != Some(b"desc") {
            continue;
        }
        let (Some(at), Some(len)) = (word(entry + 4), word(entry + 8)) else { return false };
        let Some(text) = profile.get(at..at.saturating_add(len)) else { return false };
        // A plain string in older profiles, and UTF-16 in newer ones.
        return text.windows(4).any(|bytes| bytes == b"sRGB") || text.windows(8).any(|bytes| bytes == b"\0s\0R\0G\0B");
    }
    false
}

/// Whether Photoshop's resource blocks hold a thumbnail. Lengths that lie just end the look.
fn photoshop_has_thumbnail(mut blocks: &[u8]) -> bool {
    while let Some(rest) = blocks.strip_prefix(b"8BIM") {
        let Some(&[high, low, name_len]) = rest.first_chunk::<3>() else { return false };
        // The name is padded so that it and its length are even.
        let name_end = 2 + (1 + usize::from(name_len)).next_multiple_of(2);
        let Some(size) = rest.get(name_end..name_end + 4) else { return false };
        let size = u32::from_be_bytes([size[0], size[1], size[2], size[3]]) as usize;
        if matches!(u16::from_be_bytes([high, low]), 0x0409 | 0x040C) {
            return true;
        }
        let Some(next) = (name_end + 4).checked_add(size.next_multiple_of(2)) else { return false };
        let Some(after) = rest.get(next..) else { return false };
        blocks = after;
    }
    false
}

fn is_metadata(code: u8) -> bool {
    matches!(code, 0xE0..=0xEF | 0xFE)
}

#[derive(Default)]
struct ExifFacts {
    gps: bool,
    thumbnail: bool,
    orientation: u16,
}

/// What's in an EXIF block's first two directories. Anything that doesn't hold together just isn't there.
fn read_exif(tiff: &[u8]) -> ExifFacts {
    let mut facts = ExifFacts::default();
    let little = match tiff.get(..2) {
        Some(b"II") => true,
        Some(b"MM") => false,
        _ => return facts,
    };
    let word = |at: usize| tiff.get(at..at.checked_add(2)?).map(|bytes| if little { u16::from_le_bytes([bytes[0], bytes[1]]) } else { u16::from_be_bytes([bytes[0], bytes[1]]) });
    let long = |at: usize| tiff.get(at..at.checked_add(4)?).map(|bytes| if little { u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) } else { u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) } as usize);
    if word(2) != Some(42) {
        return facts;
    }
    let Some(first) = long(4) else { return facts };
    let Some(count) = word(first) else { return facts };
    for entry in 0..usize::from(count) {
        let at = first + 2 + entry * 12;
        match word(at) {
            Some(0x8825) => facts.gps = true,
            Some(0x0112) => facts.orientation = word(at + 8).unwrap_or(0),
            Some(_) => {}
            None => return facts,
        }
    }
    // The second directory is the thumbnail's.
    facts.thumbnail = long(first + 2 + usize::from(count) * 12).is_some_and(|second| second != 0 && word(second).is_some());
    facts
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A small JPEG in structure, if not one any decoder would want: start, a frame, a scan with data, end.
    fn plain() -> Vec<u8> {
        let mut out = vec![0xFF, 0xD8];
        out.extend_from_slice(&[0xFF, 0xDB, 0, 4, 1, 2]);
        out.extend_from_slice(&[0xFF, 0xC0, 0, 3, 9]);
        out.extend_from_slice(&[0xFF, 0xDA, 0, 3, 7]);
        out.extend_from_slice(&[1, 2, 0xFF, 0x00, 3, 0xFF, 0xD0, 4]);
        out.extend_from_slice(&[0xFF, 0xD9]);
        out
    }

    fn with(segment: (u8, &[u8]), trailing: &[u8]) -> Vec<u8> {
        let mut out = plain();
        let mut extra = vec![0xFF, segment.0];
        extra.extend_from_slice(&((segment.1.len() + 2) as u16).to_be_bytes());
        extra.extend_from_slice(segment.1);
        out.splice(2..2, extra);
        out.extend_from_slice(trailing);
        out
    }

    fn scan(bytes: &[u8], strip: bool) -> Result<(JpegScan, Vec<u8>), ProtoError> {
        let mut jpeg = JpegScan::new(strip);
        let mut out = Vec::new();
        jpeg.feed(bytes, &mut out)?;
        jpeg.finish()?;
        Ok((jpeg, out))
    }

    /// Failure: something that isn't a JPEG reaches a reader's JPEG decoder as if it were, or a real one is refused and loses its original bytes. Every kind of break is caught, and none panics.
    #[test]
    fn only_a_jpegs_structure_passes() {
        assert!(scan(&plain(), false).is_ok());
        let good = plain();
        for cut in 0..good.len() {
            assert!(scan(&good[..cut], false).is_err(), "cut at {cut}");
        }
        assert_eq!(scan(b"<svg>", false).unwrap_err(), ProtoError("a JPEG that doesn't start as one"));
        assert!(scan(&[0xFF, 0xD8, 0xFF, 0xD9], false).is_err(), "an end with no picture");
        assert!(scan(&[0xFF, 0xD8, 0xFF, 0xDA, 0, 2, 0xFF, 0xD9], false).is_err(), "a scan with no frame");
        assert!(scan(&[0xFF, 0xD8, 0xFF, 0xC0, 0, 1], false).is_err(), "a length too short to be one");
        let mut seed = 11u64;
        for _ in 0..20_000 {
            let mut bad = good.clone();
            seed = seed.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
            let at = (seed >> 33) as usize % bad.len();
            bad[at] = (seed >> 20) as u8;
            let _ = scan(&bad, false);
            let _ = scan(&bad, true);
        }
    }

    /// Failure: what's read depends on where frames happened to cut the bytes, so an image's facts or its stripped bytes change from run to run.
    #[test]
    fn its_the_same_however_the_bytes_are_cut() {
        let bytes = with((0xE1, b"http://ns.adobe.com/xap/1.0/\0<xmp/>"), b"tail");
        let whole = scan(&bytes, true).unwrap();
        for piece in 1..bytes.len() {
            let mut jpeg = JpegScan::new(true);
            let mut out = Vec::new();
            for part in bytes.chunks(piece) {
                jpeg.feed(part, &mut out).unwrap();
            }
            jpeg.finish().unwrap();
            assert_eq!((jpeg.facts(), &out), (whole.0.facts(), &whole.1), "pieces of {piece}");
        }
    }

    /// Failure: stripping changes the picture, or leaves what it's for, or a segment a decoder needs goes with the rest. Everything but metadata stays byte for byte, in order.
    #[test]
    fn stripping_takes_only_whats_not_the_picture() {
        for (code, body) in [(0xE1u8, &b"Exif\0\0II*\0\x08\0\0\0\0\0\0\0\0\0"[..]), (0xE1, b"http://ns.adobe.com/xap/1.0/\0x"), (0xFE, b"a comment"), (0xE5, b"anything"), (0xE0, b"JFXX\0thumb")] {
            let (jpeg, out) = scan(&with((code, body), b"after"), true).unwrap();
            assert_eq!(out, plain(), "{code:x}");
            assert!(jpeg.changed());
        }
        let profile = profile_segment(1, 1, b"Adobe RGB (1998)");
        for (code, body) in [(0xEEu8, &b"Adobe\0\0\0\0\0\0\x01"[..]), (0xE2, &profile), (0xE0, b"JFIF\0\x01\x01\0\0\x01\0\x01\0\0")] {
            let bytes = with((code, body), b"");
            let (jpeg, out) = scan(&bytes, true).unwrap();
            assert_eq!(out, bytes, "{code:x}: kept whole");
            assert!(!jpeg.changed());
        }
        let thumb = with((0xE0, b"JFIF\0\x01\x01\0\0\x01\0\x01\x02\x02RGBRGBRGBRGB"), b"");
        let (jpeg, out) = scan(&thumb, true).unwrap();
        assert_eq!(out, with((0xE0, b"JFIF\0\x01\x01\0\0\x01\0\x01\0\0"), b""), "JFIF stays, without its thumbnail");
        assert_eq!(jpeg.facts().thumbnails, 1);
        // Without stripping, nothing goes anywhere and it's only counted.
        let (jpeg, out) = scan(&with((0xFE, b"note"), b"12345"), false).unwrap();
        assert!(out.is_empty() && !jpeg.changed());
        assert_eq!((jpeg.facts().comments, jpeg.facts().trailing_bytes), (1, 5));
    }

    /// A colour profile's first segment, with a real signature, saying what it's called.
    fn profile_segment(part: u8, parts: u8, name: &[u8]) -> Vec<u8> {
        let mut profile = vec![0u8; 128];
        profile[36..40].copy_from_slice(b"acsp");
        profile.extend_from_slice(&1u32.to_be_bytes());
        profile.extend_from_slice(b"desc");
        profile.extend_from_slice(&144u32.to_be_bytes());
        profile.extend_from_slice(&(name.len() as u32).to_be_bytes());
        profile.extend_from_slice(name);
        let mut body = b"ICC_PROFILE\0".to_vec();
        body.extend_from_slice(&[part, parts]);
        body.extend_from_slice(&profile);
        body
    }

    /// Failure: `--strip-metadata` keeps what's labelled as something a decoder wants, up to 64 KiB a segment, so a thumbnail or GPS goes straight through under the label, or under a marker no decoder reads. What's kept must be what it says it is, cut to its size, and an unknown marker isn't kept in a JPEG at all.
    #[test]
    fn a_label_doesnt_keep_what_isnt_that_thing() {
        let plain_bytes = plain();
        let mut padded = b"Adobe\0\0\0\0\0\0\x01".to_vec();
        padded.extend_from_slice(b"a whole thumbnail or anything else");
        let (jpeg, out) = scan(&with((0xEE, &padded), b""), true).unwrap();
        assert_eq!(out, with((0xEE, b"Adobe\0\0\0\0\0\0\x01"), b""), "cut to what an Adobe marker is");
        assert!(jpeg.changed());
        let (_, out) = scan(&with((0xEE, b"Adobe"), b""), true).unwrap();
        assert_eq!(out, plain_bytes, "too short to be one");
        let (jpeg, out) = scan(&with((0xE2, b"ICC_PROFILE\0\x01\x01a payload, no signature"), b""), true).unwrap();
        assert_eq!(out, plain_bytes, "no profile's signature");
        assert!(jpeg.facts().foreign_profile);
        let (_, out) = scan(&with((0xE2, &profile_segment(2, 1, b"sRGB")), b""), true).unwrap();
        assert_eq!(out, plain_bytes, "numbered past its own total");
        let (_, out) = scan(&with((0xE2, &profile_segment(2, 2, b"sRGB")), b""), true).unwrap();
        assert_eq!(out, plain_bytes, "a second part with no first");
        let good = with((0xE2, &profile_segment(1, 1, b"sRGB")), b"");
        let (jpeg, out) = scan(&good, true).unwrap();
        assert_eq!(out, good, "a real one stays");
        assert!(!jpeg.facts().foreign_profile && jpeg.facts().color_profiles == 1);
        for code in [0xF0u8, 0xF7, 0xFD, 0x02, 0xBF, 0xC8, 0xC3, 0xC9, 0xDE, 0xDF] {
            assert!(scan(&with((code, b"hidden"), b""), false).is_err(), "{code:x}");
            assert!(scan(&with((code, b"hidden"), b""), true).is_err(), "{code:x}");
        }
    }

    /// Failure: a JPEG's own wide-gamut profile goes unnoticed, so a browser colour-manages a picture the page doesn't, or sRGB's own is mistaken for one and costs the JPEG its bytes.
    #[test]
    fn a_profile_thats_not_srgb_is_noticed() {
        let foreign = |bytes: &[u8]| scan(bytes, false).unwrap().0.facts().foreign_profile;
        assert!(foreign(&with((0xE2, &profile_segment(1, 1, b"Adobe RGB (1998)")), b"")));
        assert!(foreign(&with((0xE2, &profile_segment(1, 2, b"sRGB")), b"")), "too long to be it");
        assert!(!foreign(&with((0xE2, &profile_segment(1, 1, b"sRGB IEC61966-2.1")), b"")));
        assert!(!foreign(&with((0xE2, &profile_segment(1, 1, b"\0s\0R\0G\0B")), b"")), "in UTF-16");
        assert!(!foreign(&plain()));
    }

    /// Failure: Photoshop's block holds a copy of the picture, perhaps from before a crop, and it's only counted as "other", so nobody's told. Lengths that lie mustn't panic.
    #[test]
    fn a_photoshop_thumbnail_is_counted() {
        let block = |id: u16, data: &[u8]| {
            let mut out = b"Photoshop 3.0\08BIM".to_vec();
            out.extend_from_slice(&id.to_be_bytes());
            out.extend_from_slice(&[0, 0]);
            out.extend_from_slice(&(data.len() as u32).to_be_bytes());
            out.extend_from_slice(data);
            out
        };
        let facts = scan(&with((0xED, &block(0x040C, b"thumb")), b""), false).unwrap().0.facts();
        assert_eq!((facts.thumbnails, facts.other_app), (1, 1));
        assert_eq!(scan(&with((0xED, &block(0x0404, b"iptc!")), b""), false).unwrap().0.facts().thumbnails, 0);
        assert!(!photoshop_has_thumbnail(b"8BIM\x04"));
        assert!(!photoshop_has_thumbnail(b"8BIM\x04\x04\xFF\0\0"));
        assert!(!photoshop_has_thumbnail(b"8BIM\x04\x04\0\0\xFF\xFF\xFF\xFF"));
    }

    /// Failure: two EXIF blocks, the second saying upright, read as not turned, where a reader going by the first turns the picture.
    #[test]
    fn any_block_saying_to_turn_it_counts() {
        let exif = |value: u8| -> Vec<u8> {
            let mut out = b"Exif\0\0II*\0\x08\0\0\0\x01\0\x12\x01\x03\0\x01\0\0\0".to_vec();
            out.extend_from_slice(&[value, 0, 0, 0, 0, 0, 0, 0]);
            out
        };
        let mut bytes = with((0xE1, &exif(1)), b"");
        let second = exif(6);
        let mut segment = vec![0xFF, 0xE1];
        segment.extend_from_slice(&((second.len() + 2) as u16).to_be_bytes());
        segment.extend_from_slice(&second);
        bytes.splice(2..2, segment);
        assert_eq!(scan(&bytes, false).unwrap().0.facts().orientation, 6, "turned, then upright");
        let reversed = {
            let mut out = with((0xE1, &exif(6)), b"");
            let first = exif(1);
            let mut segment = vec![0xFF, 0xE1];
            segment.extend_from_slice(&((first.len() + 2) as u16).to_be_bytes());
            segment.extend_from_slice(&first);
            out.splice(2..2, segment);
            out
        };
        assert_eq!(scan(&reversed, false).unwrap().0.facts().orientation, 6, "upright, then turned");
    }

    /// Failure: a fill byte before a marker is dropped when stripping and the JPEG still reads as unchanged, so it's counted as original bytes that aren't.
    #[test]
    fn dropping_a_fill_byte_counts_as_a_change() {
        let mut bytes = plain();
        bytes.splice(2..3, [0xFF, 0xFF]);
        let (jpeg, out) = scan(&bytes, true).unwrap();
        assert_eq!(out, plain());
        assert!(jpeg.changed());
    }

    /// An EXIF block saying how to turn the picture.
    fn orientation_block(value: u8) -> Vec<u8> {
        let mut out = b"Exif\0\0II*\0\x08\0\0\0\x01\0\x12\x01\x03\0\x01\0\0\0".to_vec();
        out.extend_from_slice(&[value, 0, 0, 0, 0, 0, 0, 0]);
        out
    }

    /// Failure: correcting what's around a picture touches the picture, which is the whole claim. From the first scan on, every byte stays, and no JPEG comes out of this that the parser wouldn't pass.
    #[test]
    fn a_fix_leaves_the_compressed_picture_as_it_was() {
        let turned = with((0xE1, &orientation_block(6)), b"after");
        let fixed = fix_header(&turned, &HeaderFix { upright: true, ..HeaderFix::default() }).unwrap();
        assert_eq!(fixed[scan_start(&fixed).unwrap()..], turned[scan_start(&turned).unwrap()..], "what follows the header");
        let mut read = JpegScan::new(false);
        read.feed(&fixed, &mut Vec::new()).unwrap();
        assert_eq!(read.facts().orientation, 1, "it says upright");
        assert_eq!(fixed.len(), turned.len(), "by changing the value and nothing else");

        let foreign = with((0xE2, &profile_segment(1, 1, b"Adobe RGB (1998)")), b"");
        let gone = fix_header(&foreign, &HeaderFix { drop_profile: true, ..HeaderFix::default() }).unwrap();
        assert_eq!(gone, plain(), "taken out, and nothing else");

        let profile = profile_segment(1, 1, b"Adobe RGB (1998)")[14..].to_vec();
        let jfif = with((0xE0, b"JFIF\0\x01\x01\0\0\x01\0\x01\0\0"), b"");
        let added = fix_header(&jfif, &HeaderFix { add_profile: Some(&profile), ..HeaderFix::default() }).unwrap();
        assert!(added[2..].starts_with(&[0xFF, 0xE0]), "JFIF stays first");
        assert_eq!(added[scan_start(&added).unwrap()..], jfif[scan_start(&jfif).unwrap()..]);
        let mut read = JpegScan::new(false);
        read.feed(&added, &mut Vec::new()).unwrap();
        assert_eq!(read.facts().color_profiles, 1);
        let replaced = fix_header(&foreign, &HeaderFix { drop_profile: true, add_profile: Some(&profile), ..HeaderFix::default() }).unwrap();
        let mut read = JpegScan::new(false);
        read.feed(&replaced, &mut Vec::new()).unwrap();
        assert_eq!(read.facts().color_profiles, 1, "one, not two");
    }

    /// Failure: a large profile makes a segment too long, or runs past what the parts can count, so a JPEG comes out broken. It's split by the convention and read back whole, and one past the cap isn't added at all.
    #[test]
    fn a_large_profile_is_split_and_past_the_cap_refused() {
        let mut profile = profile_segment(1, 1, b"Adobe RGB (1998)")[14..].to_vec();
        profile.resize(200_000, 7);
        let fixed = fix_header(&plain(), &HeaderFix { add_profile: Some(&profile), ..HeaderFix::default() }).unwrap();
        let mut read = JpegScan::new(true);
        let mut kept = Vec::new();
        read.feed(&fixed, &mut kept).unwrap();
        read.finish().unwrap();
        assert_eq!(kept, fixed, "every part holds together, so stripping keeps them all");
        assert_eq!(read.facts().color_profiles, 1);
        assert!(fixed.len() < plain().len() + profile.len() + 100, "it doesn't grow past what was asked");
        profile.resize(MAX_ADDED_PROFILE + 1, 7);
        assert!(fix_header(&plain(), &HeaderFix { add_profile: Some(&profile), ..HeaderFix::default() }).is_none());
    }

    /// Failure: what isn't a JPEG gets through the rewrite, or it panics on it. Every cut and change of a real structure ends in a sound JPEG or nothing.
    #[test]
    fn a_fix_of_a_broken_jpeg_is_nothing_or_sound() {
        let good = with((0xE1, &orientation_block(6)), b"");
        let fix = HeaderFix { upright: true, drop_profile: true, add_profile: None };
        for cut in 0..good.len() {
            assert!(fix_header(&good[..cut], &fix).is_none(), "cut at {cut}");
        }
        let mut seed = 5u64;
        for _ in 0..20_000 {
            let mut bad = good.clone();
            seed = seed.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
            let at = (seed >> 33) as usize % bad.len();
            bad[at] = (seed >> 20) as u8;
            if let Some(fixed) = fix_header(&bad, &fix) {
                let mut read = JpegScan::new(false);
                read.feed(&fixed, &mut Vec::new()).unwrap();
                read.finish().unwrap();
                assert!(!read.facts().is_turned());
            }
            let _ = frame_components(&bad);
            let _ = scan_start(&bad);
        }
    }

    /// Failure: GPS or a thumbnail goes unnoticed, in either byte order, so nobody's told a photo says where it was taken. Lengths that lie mustn't panic.
    #[test]
    fn exif_says_where_and_what_it_holds() {
        let block = |little: bool, tag: u16, value: u16, next: u32| {
            let w = |value: u16| if little { value.to_le_bytes() } else { value.to_be_bytes() };
            let l = |value: u32| if little { value.to_le_bytes() } else { value.to_be_bytes() };
            let mut out = b"Exif\0\0".to_vec();
            out.extend_from_slice(if little { b"II" } else { b"MM" });
            out.extend_from_slice(&w(42));
            out.extend_from_slice(&l(8));
            out.extend_from_slice(&w(1));
            out.extend_from_slice(&w(tag));
            out.extend_from_slice(&w(3));
            out.extend_from_slice(&l(1));
            out.extend_from_slice(&w(value));
            out.extend_from_slice(&[0, 0]);
            out.extend_from_slice(&l(next));
            out.extend_from_slice(&w(0));
            out
        };
        for little in [true, false] {
            let facts = scan(&with((0xE1, &block(little, 0x8825, 0, 26)), b""), false).unwrap().0.facts();
            assert_eq!((facts.exif, facts.gps, facts.thumbnails, facts.orientation), (1, 1, 1, 0), "{little}");
            let facts = scan(&with((0xE1, &block(little, 0x0112, 6, 0)), b""), false).unwrap().0.facts();
            assert_eq!((facts.gps, facts.thumbnails, facts.orientation), (0, 0, 6), "{little}");
            assert!(facts.is_turned());
        }
        assert_eq!(read_exif(b"II*\0\xFF\xFF\xFF\xFF").orientation, 0);
        assert_eq!(read_exif(b"MM\0*\0\0\0\x08\xFF\xFF").orientation, 0);
        assert!(!read_exif(b"").gps);
    }
}
