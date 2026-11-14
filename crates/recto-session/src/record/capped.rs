//! Lists read from a record that stop at their cap, moved out of `record.rs` unchanged (R4.3), so the record had room for highlights.

use serde::Deserialize;
use serde::de::{Deserializer, SeqAccess, Visitor};

/// A list that stops being read past its cap, rather than growing to whatever a file says.
pub(crate) fn capped<'de, D: Deserializer<'de>, T: Deserialize<'de>, const CAP: usize>(deserializer: D) -> Result<Vec<T>, D::Error> {
    struct Capped<T, const CAP: usize>(std::marker::PhantomData<T>);
    impl<'de, T: Deserialize<'de>, const CAP: usize> Visitor<'de> for Capped<T, CAP> {
        type Value = Vec<T>;
        fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(formatter, "a list of at most {CAP}")
        }
        fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Vec<T>, A::Error> {
            let mut list = Vec::new();
            while let Some(item) = seq.next_element()? {
                if list.len() >= CAP {
                    return Err(serde::de::Error::custom("too long"));
                }
                list.push(item);
            }
            Ok(list)
        }
    }
    deserializer.deserialize_seq(Capped::<T, CAP>(std::marker::PhantomData))
}
