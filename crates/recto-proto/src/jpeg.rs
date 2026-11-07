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
    facts: JpegFacts,
}

const BAD: ProtoError = ProtoError("a JPEG without a JPEG's structure");

impl JpegScan {
    /// With `strip`, [`Self::feed`] writes out the JPEG without what doesn't affect the picture: see [`Self::keeps`].
    pub fn new(strip: bool) -> Self {
        Self { state: State::Soi0, code: 0, high: 0, left: 0, held: Vec::new(), strip, changed: false, seen_frame: false, seen_scan: false, facts: JpegFacts::default() }
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
            // A fill byte before a marker.
            0xFF => {}
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
                if matches!(code, 0xC0..=0xCF) && !matches!(code, 0xC4 | 0xC8 | 0xCC) {
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
                // JFIF can hold a thumbnail, and its other fields are all that's wanted of it.
                let body = if self.strip && held.starts_with(b"JFIF\0") && held.len() > 14 { &held[..14] } else { &held[..] };
                self.changed |= self.strip && body.len() != held.len();
                self.put(out, &((body.len() + 2) as u16).to_be_bytes());
                if self.strip && body.len() != held.len() {
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
                if exif.orientation != 0 {
                    self.facts.orientation = exif.orientation;
                }
                false
            }
            0xE1 if body.starts_with(b"http://ns.adobe.com/x") => {
                self.facts.xmp += 1;
                false
            }
            0xE2 if body.starts_with(b"ICC_PROFILE\0") => {
                // A profile can run across several segments, numbered from 1.
                self.facts.color_profiles += u32::from(body.get(12) == Some(&1));
                true
            }
            0xEE if body.starts_with(b"Adobe") => true,
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
        for (code, body) in [(0xEEu8, &b"Adobe\0\0\0\0\0\0\x01"[..]), (0xE2, b"ICC_PROFILE\0\x01\x01data"), (0xE0, b"JFIF\0\x01\x01\0\0\x01\0\x01\0\0")] {
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

    /// Failure: GPS or a thumbnail goes unnoticed, in either byte order, so nobody's told a photo says where it was taken. Lengths that lie mustn't panic.
    #[test]
    fn exif_says_where_and_what_it_holds() {
        let block = |little: bool, tag: u16, value: u16, next: u32| {
            let (w, l): (fn(u16) -> [u8; 2], fn(u32) -> [u8; 4]) = if little { (u16::to_le_bytes, u32::to_le_bytes) } else { (u16::to_be_bytes, u32::to_be_bytes) };
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
