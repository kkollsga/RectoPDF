//! **The record's list of tabs whose place is withheld from agents, read as hostile and failing closed** (the fourth review's item 4).
//!
//! A list that can't be read, longer than [`MAX_TABS`] or not a list of tab ids, says that something was withheld and not what. Failing the whole
//! record as damaged for it cost the person their session; reading it as nothing would give back a place it named. So it reads as **every tab's place
//! withheld**, and the rest of the record is read as usual. Nothing is allocated by what the file claims: past the cap and past a wrong item, what's left
//! is read over without being kept.

use crate::MAX_TABS;
use serde::de::{Deserializer, IgnoredAny, MapAccess, SeqAccess, Visitor};
use serde::{Deserialize, Serialize, Serializer};

/// What the record says of whose place is withheld: these tabs by their ids, or, as read from a list that couldn't be, every tab.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct WithheldPlaces {
    ids: Vec<u32>,
    every: bool,
}

/// How a list that couldn't be read is written back, when a record is written as it was read: a word where a list belongs, which reads again as
/// [`WithheldPlaces::every`]. The session never writes it: it writes each tab by its id.
const EVERY: &str = "every";

impl WithheldPlaces {
    /// These tabs, by their ids.
    #[must_use]
    pub fn of(ids: Vec<u32>) -> Self {
        Self { ids, every: false }
    }

    /// Every tab: what a list that couldn't be read says.
    #[must_use]
    pub fn every() -> Self {
        Self { ids: Vec::new(), every: true }
    }

    /// Whether every tab's place is withheld, the list having been unreadable.
    #[must_use]
    pub fn is_every(&self) -> bool {
        self.every
    }

    /// The tabs named, by their ids: none for [`Self::every`], which names none and withholds all.
    #[must_use]
    pub fn ids(&self) -> &[u32] {
        &self.ids
    }

    /// Nothing withheld: left out of what's written, so a record with nothing withheld is byte for byte as before the list was kept.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        !self.every && self.ids.is_empty()
    }
}

impl Serialize for WithheldPlaces {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        if self.every {
            serializer.serialize_str(EVERY)
        } else {
            self.ids.serialize(serializer)
        }
    }
}

impl<'de> Deserialize<'de> for WithheldPlaces {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_any(ListOrEvery)
    }
}

/// Reads the list, or anything else in its place as every tab.
struct ListOrEvery;

impl<'de> Visitor<'de> for ListOrEvery {
    type Value = WithheldPlaces;

    fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "a list of at most {MAX_TABS} tab ids")
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<WithheldPlaces, A::Error> {
        let mut ids = Vec::new();
        let mut every = false;
        while let Some(item) = seq.next_element::<Item>()? {
            match item {
                Item::Id(id) if !every && ids.len() < MAX_TABS => ids.push(id),
                _ => every = true,
            }
        }
        Ok(if every { WithheldPlaces::every() } else { WithheldPlaces::of(ids) })
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<WithheldPlaces, A::Error> {
        while map.next_entry::<IgnoredAny, IgnoredAny>()?.is_some() {}
        Ok(WithheldPlaces::every())
    }

    fn visit_bool<E>(self, _: bool) -> Result<WithheldPlaces, E> {
        Ok(WithheldPlaces::every())
    }

    fn visit_i64<E>(self, _: i64) -> Result<WithheldPlaces, E> {
        Ok(WithheldPlaces::every())
    }

    fn visit_u64<E>(self, _: u64) -> Result<WithheldPlaces, E> {
        Ok(WithheldPlaces::every())
    }

    fn visit_f64<E>(self, _: f64) -> Result<WithheldPlaces, E> {
        Ok(WithheldPlaces::every())
    }

    fn visit_str<E>(self, _: &str) -> Result<WithheldPlaces, E> {
        Ok(WithheldPlaces::every())
    }

    fn visit_unit<E>(self) -> Result<WithheldPlaces, E> {
        Ok(WithheldPlaces::every())
    }
}

/// One item of the list: a tab's id, or anything else, read over without being kept.
enum Item {
    Id(u32),
    Other,
}

impl<'de> Deserialize<'de> for Item {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_any(ItemVisitor)
    }
}

struct ItemVisitor;

impl<'de> Visitor<'de> for ItemVisitor {
    type Value = Item;

    fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "a tab id")
    }

    fn visit_u64<E>(self, value: u64) -> Result<Item, E> {
        Ok(u32::try_from(value).map_or(Item::Other, Item::Id))
    }

    fn visit_i64<E>(self, value: i64) -> Result<Item, E> {
        Ok(u32::try_from(value).map_or(Item::Other, Item::Id))
    }

    fn visit_f64<E>(self, _: f64) -> Result<Item, E> {
        Ok(Item::Other)
    }

    fn visit_bool<E>(self, _: bool) -> Result<Item, E> {
        Ok(Item::Other)
    }

    fn visit_str<E>(self, _: &str) -> Result<Item, E> {
        Ok(Item::Other)
    }

    fn visit_unit<E>(self) -> Result<Item, E> {
        Ok(Item::Other)
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Item, A::Error> {
        while seq.next_element::<IgnoredAny>()?.is_some() {}
        Ok(Item::Other)
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Item, A::Error> {
        while map.next_entry::<IgnoredAny, IgnoredAny>()?.is_some() {}
        Ok(Item::Other)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Failure: a list that can't be read fails, or is taken as nothing withheld, or keeps what a forged list claims past the cap; or what's written back
    /// of one reads as something else. Each kind of list, read alone.
    #[test]
    fn a_list_that_cant_be_read_withholds_every_tab() {
        let read = |json: &str| serde_json::from_str::<WithheldPlaces>(json).expect(json);
        assert_eq!(read("[3, 1, 3]"), WithheldPlaces::of(vec![3, 1, 3]));
        assert_eq!(read("[]"), WithheldPlaces::default());
        for json in [
            "\"1\"",
            "[-1]",
            "[1, {\"a\": [1]}]",
            "[1, [2]]",
            "[1.5]",
            "[4294967296]",
            "{\"1\": true}",
            "null",
            "true",
            "7",
        ] {
            assert!(read(json).is_every(), "{json}");
        }
        let past_the_cap = format!("[{}]", vec!["1"; MAX_TABS + 1].join(","));
        let past = read(&past_the_cap);
        assert!(past.is_every() && past.ids().is_empty(), "nothing kept of what's past the cap");
        assert_eq!(
            read(&serde_json::to_string(&WithheldPlaces::every()).unwrap()),
            WithheldPlaces::every(),
            "written as read"
        );
    }
}
