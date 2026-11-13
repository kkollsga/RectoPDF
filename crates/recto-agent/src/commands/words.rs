//! Words more than one command takes for a choice, as types (step R2.5): each reads from its word on the wire and prints back to it.

/// `on` or `off`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OnOff {
    On,
    Off,
}

impl OnOff {
    /// Every value, in the order the rows list their words.
    pub const ALL: [Self; 2] = [Self::On, Self::Off];

    /// Its word on the wire.
    #[must_use]
    pub fn word(self) -> &'static str {
        match self {
            Self::On => "on",
            Self::Off => "off",
        }
    }

    /// The value a word on the wire means, if it's one.
    #[must_use]
    pub fn from_word(word: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|value| value.word() == word)
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
