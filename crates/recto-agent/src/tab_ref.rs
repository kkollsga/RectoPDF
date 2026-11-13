//! **Which tab a command means** (step R2.5): the one showing, one by its number, or one by a piece of its name, as a type. The window names a tab by its id
//! and never writes `#3` for [`crate::select`] to read back; the words exist only on the wire, where this reads and prints them.
//!
//! A [`TabRef`] that exists prints as words that read back as itself: a name, or a selector that can name no tab, is made only by [`TabRef::parse`]
//! (their fields are private), so no code can make `Name("#3")` or `Name("active")`, which would print as another tab. Printing is canonical rather than
//! as typed: `#007` and `#+3` are tabs 7 and 3, as `str::parse` reads a number and as `main` read them before the type, and print as `#7` and `#3`.
//!
//! What the activity log keeps of one ([`TabRef::log`]) is `active` or a tab's number, never a name: `#+3` is logged as `#3`, the tab it means, where the
//! words before the type logged it as `by name`; a number too large to be a tab (`#4294967296`, or one longer than ten digits) means no tab and is logged
//! as `by name`, as an overlong selector is.

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
    Name(Name),
    /// Written as a tab's number (`#…`) that can't be one, or longer than any name: it means no tab, and no name is searched for it. Kept as written, so it
    /// prints back as it came.
    Nothing(Nothing),
}

/// A piece of a tab's name, as an agent wrote it. Made only by [`TabRef::parse`], so it's never `active`, never starts with `#` and is no longer than a
/// selector is looked at: it prints as words that read back as a name.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct Name(Box<str>);

impl Name {
    /// The words, as written.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// A selector that can name no tab, as written. Made only by [`TabRef::parse`].
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct Nothing(Box<str>);

/// The tab showing, for a command that names none and acts on that one.
pub static ACTIVE: TabRef = TabRef::Active;

impl TabRef {
    /// What a selector on the wire means.
    #[must_use]
    pub fn parse(text: &str) -> Self {
        if text.len() > MAX_SELECTOR_BYTES {
            return Self::Nothing(Nothing(text.into()));
        }
        if text == "active" {
            return Self::Active;
        }
        match text.strip_prefix('#') {
            Some(number) => number.parse().map_or_else(|_| Self::Nothing(Nothing(text.into())), |id| Self::Id(TabId(id))),
            None => Self::Name(Name(text.into())),
        }
    }

    /// Longer than any selector is looked at, which the server refuses before it waits for one.
    #[must_use]
    pub fn over_long(&self) -> bool {
        matches!(self, Self::Nothing(Nothing(text)) if text.len() > MAX_SELECTOR_BYTES)
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
            Self::Name(Name(text)) | Self::Nothing(Nothing(text)) => f.write_str(text),
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
            TabRef::Name(Name("Active".into())),
            "a name, as only `active` is the tab showing"
        );
        assert_eq!(TabRef::parse("report"), TabRef::Name(Name("report".into())));
        assert_eq!(TabRef::parse(""), TabRef::Name(Name("".into())));
        for nothing in ["#", "#x", "#-1", "#4294967296"] {
            assert_eq!(TabRef::parse(nothing), TabRef::Nothing(Nothing(nothing.into())), "{nothing}");
        }
        let long = "a".repeat(MAX_SELECTOR_BYTES + 1);
        assert_eq!(TabRef::parse(&long), TabRef::Nothing(Nothing(long.as_str().into())));
        for tab in [TabRef::Active, TabRef::Id(TabId(7)), TabRef::parse("report"), TabRef::parse("#x")] {
            assert_eq!(TabRef::parse(&tab.to_string()), tab);
        }
    }
}
