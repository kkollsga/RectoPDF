//! **How a snapshot is drawn, by where it goes** ([`drawn_for`]): the page as the person sees it, to their clipboard, or as its file shows it, to a file or
//! the answer. Its own file so the rule, and the test's seam that seeds another answer for it, are in one place (the branch's review, F2).

use recto_proto::contents::TextMode;

/// Where a snapshot goes, which decides how it's drawn ([`drawn_for`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Destination {
    /// A new file, or the answer (text).
    FileOrAnswer,
    /// The person's clipboard, at the window ([`crate::clipboard`]).
    Clipboard,
}

/// **How a snapshot is drawn, by where it goes: the one place this is decided** (coordinator, 2026-11-14, for one meaning of `snapshot` whoever sends it).
/// To the person's clipboard it's the page as they see it: turned by the page's unsaved turn (decided 2026-11-08: what's copied is what's seen), and an
/// SVG's text as outlines only, so hidden text stays out of it as it does from the text item. To a file or the answer it's the page as its file shows it
/// (the answer says `unsaved_rotation_degrees`), and an SVG's text both drawn and searchable, as `export svg` writes it.
///
/// Every drawing of a snapshot takes its turn and its SVG's text from here, a picture and an SVG alike, to the clipboard, a file or the answer (the
/// branch's review, F2): `unsaved_quarter_turns` is the page's unsaved turn, as the tab has it or as the box saw it.
pub(crate) fn drawn_for(to: Destination, unsaved_quarter_turns: u8) -> Drawn {
    #[cfg(test)]
    if let Some(seeded) = seeded(to) {
        return seeded;
    }
    match to {
        Destination::FileOrAnswer => Drawn {
            quarter_turns: 0,
            text: TextMode::Both,
        },
        Destination::Clipboard => Drawn {
            quarter_turns: unsaved_quarter_turns % 4,
            text: TextMode::Paths,
        },
    }
}

/// How a snapshot is drawn ([`drawn_for`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Drawn {
    /// Quarter turns clockwise the picture or SVG comes out turned.
    pub quarter_turns: u8,
    /// How an SVG's text comes out.
    pub text: TextMode,
}

#[cfg(test)]
thread_local! {
    /// Another answer for one destination, that [`drawn_for`] gives on this thread while it's set.
    static SEEDED: std::cell::Cell<Option<(Destination, Drawn)>> = const { std::cell::Cell::new(None) };
}

/// What [`drawn_for`] says for `to`, if a test on this thread seeded another answer for it.
#[cfg(test)]
fn seeded(to: Destination) -> Option<Drawn> {
    SEEDED.with(|seeded| seeded.get().filter(|(seeded, _)| *seeded == to).map(|(_, drawn)| drawn))
}

/// A test's: [`drawn_for`] gives this answer for its destination on this thread, until it's set to none.
#[cfg(test)]
pub(super) fn seed(seeded: Option<(Destination, Drawn)>) {
    SEEDED.with(|cell| cell.set(seeded));
}
