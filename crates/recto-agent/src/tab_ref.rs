//! **Which tab a command means** (step R2.5): the one showing, one by its number, or one by a piece of its name, as a type. The window names a tab by its id
//! and never writes `#3` for [`crate::select`] to read back; the words exist only on the wire, where this reads and prints them.

use crate::MAX_SELECTOR_BYTES;
use recto_session::TabId;
use std::fmt;

/// Which tab a command means. On the wire, `active`, `#3`, or part of a tab's name; selectors never guess, so a name that matches more than one is refused
/// with the candidates.
#[derive(Clone, Default, PartialEq, Eq, Hash)]
pub enum TabRef {
    /// The tab showing.
    #[default]
    Active,
    /// A tab by its number.
    Id(TabId),
    /// A tab by a piece of its name, any case.
    Name(Box<str>),
    /// Written as a tab's number (`#…`) that can't be one, or longer than any name: it means no tab, and no name is searched for it. Kept as written, so it
    /// prints back as it came.
    Nothing(Box<str>),
}

/// The tab showing, for a command that names none and acts on that one.
pub static ACTIVE: TabRef = TabRef::Active;

impl TabRef {
    /// What a selector on the wire means.
    #[must_use]
    pub fn parse(text: &str) -> Self {
        if text.len() > MAX_SELECTOR_BYTES {
            return Self::Nothing(text.into());
        }
        if text == "active" {
            return Self::Active;
        }
        match text.strip_prefix('#') {
            Some(number) => number.parse().map_or_else(|_| Self::Nothing(text.into()), |id| Self::Id(TabId(id))),
            None => Self::Name(text.into()),
        }
    }

    /// Longer than any selector is looked at, which the server refuses before it waits for one.
    #[must_use]
    pub fn over_long(&self) -> bool {
        matches!(self, Self::Nothing(text) if text.len() > MAX_SELECTOR_BYTES)
    }

    /// What goes in the activity log: `active` or the tab's number, never a name as typed, which could be anything.
    #[must_use]
    pub fn log(&self) -> String {
        match self {
            Self::Active => "active".to_string(),
            Self::Id(id) => format!("#{}", id.0),
            Self::Name(_) | Self::Nothing(_) => "by name".to_string(),
        }
    }
}

impl From<TabId> for TabRef {
    fn from(id: TabId) -> Self {
        Self::Id(id)
    }
}

/// A selector as an agent writes it, read as the wire reads it: for a line typed in a test or a tool, never for a tab the code has in hand, which is
/// [`TabRef::Id`].
impl From<&str> for TabRef {
    fn from(text: &str) -> Self {
        Self::parse(text)
    }
}

/// Its words on the wire.
impl fmt::Display for TabRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Active => f.write_str("active"),
            Self::Id(id) => write!(f, "#{}", id.0),
            Self::Name(text) | Self::Nothing(text) => f.write_str(text),
        }
    }
}

/// As its words on the wire, so a command reads in a log or a test as the line it is.
impl fmt::Debug for TabRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(&self.to_string(), f)
    }
}

impl serde::Serialize for TabRef {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

/// Any string is a selector; anything else isn't, as before.
impl<'de> serde::Deserialize<'de> for TabRef {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = std::borrow::Cow::<'de, str>::deserialize(deserializer)?;
        Ok(Self::parse(&text))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Failure: a selector means another tab once typed, or a tab's number that can't be one, or an overlong name, is searched for as a name; or a tab the
    /// window names prints as words that read back as another.
    #[test]
    fn a_selector_means_what_select_took_it_to_mean() {
        assert_eq!(TabRef::parse("active"), TabRef::Active);
        assert_eq!(TabRef::parse("#3"), TabRef::Id(TabId(3)));
        assert_eq!(TabRef::parse("#+3"), TabRef::Id(TabId(3)), "as `str::parse` reads a number");
        assert_eq!(
            TabRef::parse("Active"),
            TabRef::Name("Active".into()),
            "a name, as only `active` is the tab showing"
        );
        assert_eq!(TabRef::parse("report"), TabRef::Name("report".into()));
        assert_eq!(TabRef::parse(""), TabRef::Name("".into()));
        for nothing in ["#", "#x", "#-1", "#4294967296"] {
            assert_eq!(TabRef::parse(nothing), TabRef::Nothing(nothing.into()), "{nothing}");
        }
        let long = "a".repeat(MAX_SELECTOR_BYTES + 1);
        assert_eq!(TabRef::parse(&long), TabRef::Nothing(long.as_str().into()));
        for tab in [
            TabRef::Active,
            TabRef::Id(TabId(7)),
            TabRef::Name("report".into()),
            TabRef::Nothing("#x".into()),
        ] {
            assert_eq!(TabRef::parse(&tab.to_string()), tab);
        }
    }
}
