//! Words more than one command takes for a choice, as types (step R2.5): each reads from its word on the wire and prints back to it.
//!
//! **Each choice's words are written once** (review of R2.5, F5): a type's `WORDS`, in the order of its `ALL`, which its row's `Kind::Choice` takes and its
//! `word` and `from_word` read, so the CLI can't offer a word the type refuses or refuse one it takes. [`CHOICES`] lists them, and a test holds every
//! choice of a typed command's row to one of them.

/// Every type's words for a choice: what a typed command's row may offer, held by the test below.
#[cfg(test)]
const CHOICES: &[&[&str]] = &[
    OnOff::WORDS,
    super::contents::Which::WORDS,
    super::view::Shown::WORDS,
    super::view::PaperChoice::WORDS,
    super::view::LAYOUT_WORDS,
];

/// The word of a value, from a type's values and its words in the same order.
pub(crate) fn word_of<T: PartialEq>(all: &[T], words: &[&'static str], value: &T) -> &'static str {
    all.iter().zip(words).find(|(each, _)| *each == value).map_or("", |(_, word)| word)
}

/// The value a word means, from a type's values and its words in the same order.
pub(crate) fn value_of<T: Copy>(all: &[T], words: &[&str], word: &str) -> Option<T> {
    all.iter().zip(words).find(|(_, each)| **each == word).map(|(value, _)| *value)
}

/// `on` or `off`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OnOff {
    On,
    Off,
}

impl OnOff {
    /// Every value, in the order of [`OnOff::WORDS`].
    pub const ALL: [Self; 2] = [Self::On, Self::Off];
    /// Its words on the wire, which its rows offer.
    pub const WORDS: &'static [&'static str] = &["on", "off"];

    /// Its word on the wire.
    #[must_use]
    pub fn word(self) -> &'static str {
        word_of(&Self::ALL, Self::WORDS, &self)
    }

    /// The value a word on the wire means, if it's one.
    #[must_use]
    pub fn from_word(word: &str) -> Option<Self> {
        value_of(&Self::ALL, Self::WORDS, word)
    }

    #[must_use]
    pub fn from_bool(on: bool) -> Self {
        if on { Self::On } else { Self::Off }
    }

    #[must_use]
    pub fn is_on(self) -> bool {
        self == Self::On
    }
}

const _: () = assert!(OnOff::ALL.len() == OnOff::WORDS.len(), "a value without a word");

#[cfg(test)]
mod tests {
    use super::*;
    use crate::table::{Kind, TABLE};

    /// Failure (review of R2.5, F5): a typed command's row offers words for a choice that its type doesn't read and print from, so the CLI offers a word
    /// the type refuses, or refuses one it takes, once the two are edited apart. Every choice of every typed command's row is one of the types' `WORDS`,
    /// found by walking the table, with no list of arguments to keep.
    #[test]
    fn every_choice_a_typed_commands_row_offers_is_a_types_words() {
        let mut walked = 0;
        for spec in TABLE.iter().filter(|spec| super::super::is_typed(spec.tag)) {
            for arg in spec.args {
                if let Kind::Choice(words) = arg.kind {
                    walked += 1;
                    assert!(
                        CHOICES.contains(&words),
                        "{} --{}: {words:?} aren't any type's WORDS, which its value reads from",
                        spec.name,
                        arg.key
                    );
                }
            }
        }
        assert!(walked >= 9, "the typed rows' choices were walked: {walked}");
    }

    /// Failure: a word reads as a value that prints as another word, or two values share a word; or the words for a paper or a layout, which the record
    /// keeps by the session's own names, drift from them.
    #[test]
    fn each_types_words_read_back_as_its_values() {
        fn round<T: Copy + PartialEq + std::fmt::Debug>(all: &[T], words: &[&'static str]) {
            for (value, word) in all.iter().zip(words) {
                assert_eq!(value_of(all, words, word), Some(*value), "{word}");
                assert_eq!(word_of(all, words, value), *word, "{value:?}");
            }
            let mut unique = words.to_vec();
            unique.sort_unstable();
            unique.dedup();
            assert_eq!(unique.len(), words.len(), "{words:?}");
        }
        use super::super::{contents::Which, view};
        round(&OnOff::ALL, OnOff::WORDS);
        round(&Which::ALL, Which::WORDS);
        round(&view::Shown::ALL, view::Shown::WORDS);
        round(&view::PaperChoice::ALL, view::PaperChoice::WORDS);
        round(&recto_session::record::Spread::ALL, view::LAYOUT_WORDS);
        assert_eq!(view::LAYOUT_WORDS, recto_session::record::Spread::ALL.map(recto_session::record::Spread::name));
        for (choice, word) in view::PaperChoice::ALL.iter().zip(view::PaperChoice::WORDS) {
            assert_eq!(choice.paper().map_or("auto", recto_session::Paper::name), *word);
        }
    }
}
