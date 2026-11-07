//! Reading choices out of fuzz bytes. Running out gives zeros, so every input means something, and a shorter one means something simpler.

/// Bytes being read as choices.
pub struct Take<'a>(pub &'a [u8]);

impl<'a> Take<'a> {
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn u8(&mut self) -> u8 {
        match self.0.split_first() {
            Some((first, rest)) => {
                self.0 = rest;
                *first
            }
            None => 0,
        }
    }

    pub fn u16(&mut self) -> u16 {
        u16::from_le_bytes([self.u8(), self.u8()])
    }

    pub fn u32(&mut self) -> u32 {
        u32::from_le_bytes([self.u8(), self.u8(), self.u8(), self.u8()])
    }

    pub fn f32(&mut self) -> f32 {
        f32::from_bits(self.u32())
    }

    /// One of a table's entries.
    pub fn pick<T: Copy>(&mut self, from: &[T]) -> T {
        from[usize::from(self.u8()) % from.len()]
    }

    pub fn rest(&mut self) -> &'a [u8] {
        std::mem::take(&mut self.0)
    }
}

/// A small generator that needs no crate, so a seeded run repeats exactly on any machine.
#[derive(Clone, Debug)]
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        // Zero would stay zero for ever.
        Self(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1)
    }

    pub fn next_u64(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    /// A number under `limit`, or zero if there's no room.
    pub fn below(&mut self, limit: usize) -> usize {
        if limit == 0 { 0 } else { (self.next_u64() % limit as u64) as usize }
    }

    pub fn pick<T: Copy>(&mut self, from: &[T]) -> T {
        from[self.below(from.len())]
    }
}
