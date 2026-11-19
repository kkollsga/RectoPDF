//! A corruption fixture must change the encrypted nonce even when it starts at any byte value.
pub(super) fn damage(bytes: &mut [u8]) {
    bytes[0] ^= 0x40;
}

/// Failure: the corruption seed is a no-op for one valid random nonce prefix,
/// so a healthy index is reported as a failed fallback.
#[test]
fn every_nonce_prefix_is_changed_by_the_corruption_seed() {
    for prefix in u8::MIN..=u8::MAX {
        let mut bytes = [prefix];
        damage(&mut bytes);
        assert_ne!(bytes, [prefix], "corruption must change nonce prefix {prefix:#04x}");
    }
}
