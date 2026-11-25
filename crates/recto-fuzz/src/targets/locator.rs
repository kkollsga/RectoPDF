//! Hostile locator bytes: no path lookup, unchecked count allocation or partial/noncanonical acceptance.

use recto_core::locator::{self, CharacterRange, DocumentTags, Error, Locator, Looked, PageRef, Placed, ScanCaps, Spot, Status, Target};
use recto_core::page_id::{IndexedReader, PageMap};
use std::sync::OnceLock;

/// A three-page file, [A, A, B], opened once: what a decoded token is resolved against.
fn fixture() -> &'static (IndexedReader, PageMap) {
    static FIXTURE: OnceLock<(IndexedReader, PageMap)> = OnceLock::new();
    FIXTURE.get_or_init(|| {
        let pages: Vec<_> = ["A", "A", "B"]
            .iter()
            .map(|name| recto_testpdf::Page::new(format!("BT /F1 12 Tf 72 700 Td ({name}) Tj ET")))
            .collect();
        let reader = recto_core::page_id::open_bytes_for_identity(std::sync::Arc::from(recto_testpdf::pages(&pages))).expect("the fixture opens");
        let map = reader.page_map().expect("the fixture's pages");
        (reader, map)
    })
}

/// A resolve against the fixture under small caps: what any decoded token may be asked, and what an answer may never say. One bit of the token
/// (the lowest of its page check) holds the page cap at two, under the three pages of the fixture, so that `Incomplete` is reached too.
fn resolves(read: &Locator) {
    let (reader, map) = fixture();
    let caps = ScanCaps {
        bytes: 1 << 20,
        pages: if read.page().page_check()[0] & 1 == 1 { 2 } else { 8 },
        ..ScanCaps::STANDARD
    };
    let now = locator::resolve(read, reader, map, &caps);
    assert!(now.pages_read <= caps.pages, "a cap on pages read holds");
    let hint = read.page().number() - 1;
    match now.status {
        Status::Found { page } => assert!(now.looked == Looked::Hint && page == hint, "found only where the hint holds the page"),
        Status::Moved { .. } | Status::Ambiguous { .. } | Status::Gone => assert_eq!(now.looked, Looked::All, "only every page read says more or less"),
        Status::Incomplete { .. } => assert_eq!(now.looked, Looked::Part),
    }
    match &now.status {
        Status::Found { page } | Status::Moved { page } => assert!((*page as usize) < map.len()),
        Status::Ambiguous { pages, .. } => assert!((2..=locator::MAX_CANDIDATES).contains(&pages.len())),
        Status::Incomplete { pages } => assert!(pages.len() <= 1),
        Status::Gone => {}
    }
    if let Some(Placed::Spot([left, top, right, bottom])) = now.target {
        // The fixture's pages are US Letter (612 x 792 points) by construction, so its bounds are stated here rather than
        // read through `nav::frame_of`, which is private to recto-core.
        assert!(0.0 <= left && left <= right && right <= 612.0, "a spot lies inside its page");
        assert!(0.0 <= top && top <= bottom && bottom <= 792.0, "a spot lies inside its page");
    }
}

/// A character range over text the input itself gives, as the executor does over a page's text: no panic whatever the text and the offsets; a range
/// made over the text is the same over it again, and any range ending past the text's characters is `Changed`.
fn characters_over(range: CharacterRange, bytes: &[u8]) {
    let text = String::from_utf8_lossy(&bytes[..bytes.len().min(4096)]);
    let count = text.chars().count() as u64;
    let (start, end) = range.offsets();
    let held = locator::check_characters(range, &text);
    if u64::from(end) > count {
        assert_eq!(held, locator::TextMatch::Changed, "a range past the text is never the same");
    }
    // Offsets taken from the range, and from the input's own first bytes, as a hostile `--chars` would be.
    let wild = [
        (start, end),
        (end, start),
        (0, count as u32),
        (count as u32, count as u32 + 1),
        (u32::from(bytes.first().copied().unwrap_or(0)), end),
    ];
    for (from, to) in wild {
        if let Ok(made) = locator::characters(&text, from, to) {
            assert_eq!(made.offsets(), (from, to));
            assert_eq!(
                locator::check_characters(made, &text),
                locator::TextMatch::Same,
                "a range made over a text holds over it"
            );
            assert!(u64::from(to) <= count);
        }
    }
}

/// A range made over any text, with offsets from the input's own bytes: the text is what follows the first NUL, so it is whatever bytes (multi-byte characters
/// among them) and not only what a canonical token holds, which is ASCII. A range made over the text is the same over it; one past its end is refused.
fn characters_free(bytes: &[u8]) {
    let Some(nul) = bytes.iter().position(|byte| *byte == 0) else { return };
    let (head, tail) = (&bytes[..nul], &bytes[nul + 1..]);
    let text = String::from_utf8_lossy(&tail[..tail.len().min(4096)]);
    let count = text.chars().count() as u32;
    let offset = |at: usize| head.get(at).copied().map_or(0, u32::from) * 31 % (count + 2);
    let (start, end) = (offset(0), offset(1));
    match locator::characters(&text, start, end) {
        Ok(made) => {
            assert_eq!(made.offsets(), (start, end));
            assert_eq!(locator::check_characters(made, &text), locator::TextMatch::Same);
        }
        Err(_) => assert!(start >= end || end > count || end - start > 1 << 21),
    }
}

/// Reads at most the parser bound plus one, even when a campaign hands it a much larger or invalid-UTF-8 input.
pub fn check(bytes: &[u8]) {
    characters_free(bytes);
    let bounded = &bytes[..bytes.len().min(locator::MAX_TOKEN_BYTES + 1)];
    let Ok(token) = std::str::from_utf8(bounded) else { return };
    if token.len() > locator::MAX_TOKEN_BYTES {
        assert_eq!(locator::decode(token), Err(Error::TooLong));
        return;
    }
    if let Ok(read) = locator::decode(token) {
        let canonical = locator::encode(&read);
        assert!(canonical.len() <= locator::MAX_TOKEN_BYTES);
        assert_eq!(canonical, token, "a token has exactly one spelling, with no ignored suffix or padding");
        assert_eq!(locator::decode(&canonical), Ok(read));
        resolves(&read);
        if let Target::Characters(range) = read.target() {
            characters_over(range, bytes);
        }
    }
}

/// Small seeds cover valid targets, overflow, UTF-8 refusal and the exact input cap plus one.
pub fn seeds() -> Vec<Vec<u8>> {
    let identity = recto_core::page_id::PageIdentity {
        digest: [3; 32],
        object: (7, 0),
        frame: 0x1234_5678,
    };
    let page = PageRef::new(42, &identity).unwrap();
    let document = DocumentTags::new([1; 8], Some([2; 8]));
    let mut seeds: Vec<_> = [
        Target::Page,
        Target::Characters(CharacterRange::new(0, 32, [4; 8]).unwrap()),
        Target::Spot(Spot::point(72, 350).unwrap()),
        Target::Spot(Spot::area(72, 350, 200, 90).unwrap()),
    ]
    .iter()
    .map(|target| locator::encode(&Locator::new(document, page, *target)).into_bytes())
    .collect();
    let plain = std::str::from_utf8(&seeds[0]).unwrap();
    seeds.push(plain.replace(".p42-", ".p42949672960-").into_bytes());
    seeds.push(vec![b'x'; locator::MAX_TOKEN_BYTES + 1]);
    seeds.push(vec![0xff]);
    seeds.push("r1.é".as_bytes().to_vec());
    seeds.push(b"r1..p1--.x-1,0".to_vec());
    seeds.extend(fixture_seeds());
    seeds
}

/// References made from the fixture's own pages, so a campaign starts with tokens that find, move and spot: each page as a whole, then a spot, a point,
/// a character range and one past the end of the file.
fn fixture_seeds() -> Vec<Vec<u8>> {
    let (reader, map) = fixture();
    let mut seeds = Vec::new();
    for page in 0..map.len() as u32 {
        let Ok(reference) = locator::reference(reader, map, page, &ScanCaps::STANDARD) else {
            continue;
        };
        for target in [
            Target::Page,
            Target::Spot(Spot::area(72, 100, 200, 90).unwrap()),
            Target::Spot(Spot::point(700, 900).unwrap()),
            Target::Characters(CharacterRange::new(0, 5, [9; 8]).unwrap()),
        ] {
            seeds.push(locator::encode(&Locator::new(reference.document(), reference.page(), target)).into_bytes());
        }
    }
    seeds
}
