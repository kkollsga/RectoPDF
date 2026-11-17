//! Hostile locator bytes: no path lookup, unchecked count allocation or partial/noncanonical acceptance.

use recto_core::locator::{self, CharacterRange, DocumentTags, Error, Locator, PageRef, Spot, Target};

/// Reads at most the parser bound plus one, even when a campaign hands it a much larger or invalid-UTF-8 input.
pub fn check(bytes: &[u8]) {
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
    seeds
}
