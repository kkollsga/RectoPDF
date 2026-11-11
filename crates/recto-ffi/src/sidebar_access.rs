//! **Accessibility of the sidebar's own parts:** its scroll thumbs, which are views of the app's own rather than the system's scrollers, and a contents section's fold, which shows only under the pointer. What VoiceOver hears, what a key or an accessibility action folds, and the thumb's colours with Increase Contrast are decided here; the host only says them to the system and calls back the same methods a click does.
//!
//! - **The thumb** is a vertical scroll bar to VoiceOver, named for its list, with its place as a value from 0 to 1 and in words, and increment and decrement move it a page, as a click beside it does.
//! - **A fold** is an action on its row, "Fold Section" or "Unfold Section", and the row says whether it's open. With the sidebar's list holding the keyboard, ← folds the section being read and → opens it, as an outline does on a Mac. Folding the section being read hides its row, so → then opens the section folded last by a key or an action, which is the one hiding it: [`Folds`] keeps that one, 8 bytes.
//! - **Increase Contrast** draws the thumb about twice as strong: `recto_session::SCROLL_THUMB`'s `*_contrast` colours.

use crate::{AppCore, RowKind, SidebarRow, lock};
use std::sync::Mutex;

/// The section folded last by a key or an accessibility action, by its tab and its entry in the contents, for → to open again once the row being read is hidden in it.
#[derive(Debug, Default)]
pub(crate) struct Folds {
    last: Mutex<Option<(u32, u32)>>,
}

/// What a click above or below a thumb, or an increment or decrement, scrolls by: a part of what shows, so a line stays in sight from one page to the next.
pub const PAGE_SHARE: f64 = 0.9;

/// A scroll thumb as VoiceOver has it.
#[derive(Clone, Debug, PartialEq, uniffi::Record)]
pub struct ThumbAccess {
    /// Its name, for the list it scrolls.
    pub label: String,
    /// Where the list is, from 0 at the top to 1 at the end.
    pub value: f64,
    /// The same in words.
    pub value_text: String,
    /// How far an increment or a decrement moves it, as a part of what shows.
    pub page_share: f64,
}

/// The thumb's opacity with Increase Contrast on, light and dark, at rest and under the pointer. Its colours are the usual ones.
#[derive(Clone, Copy, Debug, PartialEq, uniffi::Record)]
pub struct ThumbContrast {
    pub light_alpha: f64,
    pub light_hover_alpha: f64,
    pub dark_alpha: f64,
    pub dark_hover_alpha: f64,
}

/// Which way a key folds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FoldKey {
    Fold,
    Unfold,
}

impl FoldKey {
    fn from_name(key: &str) -> Option<Self> {
        match key {
            "left" => Some(Self::Fold),
            "right" => Some(Self::Unfold),
            _ => None,
        }
    }
}

/// One contents row, as the fold key needs it: its tab and entry, how deep it is, whether it folds, and whether it's open.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct FoldRow {
    tab: u32,
    entry: u32,
    depth: u8,
    expandable: bool,
    expanded: bool,
}

/// The row a key folds or opens, given the row being read and the section folded last: ← folds the row being read if it's an open section, or else the section it's in; → opens it if it's a folded section, or, with no row being read as it's folded away, the section folded last if that's still folded. Nothing past either end, as an outline's arrows do nothing there. A walk back of at most the rows above, once a key.
/// The rows are read where they're kept, by `row`, with `count` of them: a key copies none of an outline of 100,000.
fn fold_target(count: usize, row: impl Fn(usize) -> FoldRow, current: Option<usize>, last: Option<(u32, u32)>, key: FoldKey) -> Option<usize> {
    let Some(at) = current.filter(|at| *at < count) else {
        return match key {
            FoldKey::Unfold => last.and_then(|last| {
                (0..count).find(|at| {
                    let row = row(*at);
                    (row.tab, row.entry) == last && row.expandable && !row.expanded
                })
            }),
            FoldKey::Fold => None,
        };
    };
    let here = row(at);
    match key {
        FoldKey::Unfold => (here.expandable && !here.expanded).then_some(at),
        FoldKey::Fold if here.expandable && here.expanded => Some(at),
        // The section it's in is the nearest row above that's less deep, which is open, as this one shows.
        FoldKey::Fold => (0..at)
            .rev()
            .find(|above| row(*above).depth < here.depth)
            .filter(|parent| row(*parent).expandable),
    }
}

/// A thumb's name, by the list it scrolls.
fn thumb_label(list: &str) -> &'static str {
    match list {
        "contents" => "Contents",
        "tabs" => "Open documents",
        "find" => "Find results",
        "pages" => "Pages",
        _ => "List",
    }
}

/// What `recto qa accessibility` takes, each calling what the system calls on the app's own views: `contrast:on` or `off`, as though Increase Contrast were; `thumb:LIST:increment` or `decrement`, a thumb's action; `fold:N`, contents row N's fold action; `key:left` or `right`, the arrows with the sidebar's list holding the keyboard.
pub(crate) fn is_qa_value(value: &str) -> bool {
    qa_step(value).is_some()
}

/// One of `recto qa accessibility`'s steps, read by Rust, for the host to call what the system would.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum AccessibilityStep {
    /// As though Increase Contrast were on or off, for this app alone.
    Contrast { on: bool },
    /// The arrows with the sidebar's list holding the keyboard: `left` or `right`.
    FoldKey { key: String },
    /// Contents row `row`'s fold action.
    FoldAction { row: u32 },
    /// A thumb's increment or decrement: `list` is `contents`, `tabs`, `find` or `pages`.
    Thumb { list: String, increment: bool },
}

fn qa_step(value: &str) -> Option<AccessibilityStep> {
    let (kind, rest) = value.split_once(':')?;
    Some(match (kind, rest) {
        ("contrast", "on" | "off") => AccessibilityStep::Contrast { on: rest == "on" },
        ("key", "left" | "right") => AccessibilityStep::FoldKey { key: rest.to_string() },
        ("fold", row) => AccessibilityStep::FoldAction { row: row.parse().ok()? },
        ("thumb", rest) => {
            let (list, step) = rest.split_once(':')?;
            if !matches!(list, "contents" | "tabs" | "find" | "pages") || !matches!(step, "increment" | "decrement") {
                return None;
            }
            AccessibilityStep::Thumb {
                list: list.to_string(),
                increment: step == "increment",
            }
        }
        _ => return None,
    })
}

/// What's said when a step did nothing, by what it was.
fn refusal(step: Option<&AccessibilityStep>) -> &'static str {
    match step {
        Some(AccessibilityStep::Contrast { .. }) => "The window is closed.",
        Some(AccessibilityStep::FoldKey { .. }) => "Nothing to fold or open there.",
        Some(AccessibilityStep::FoldAction { .. }) => "That row has no fold, or isn't on screen.",
        Some(AccessibilityStep::Thumb { .. }) => "That thumb has nothing to scroll.",
        None => "That isn't something `qa accessibility` takes.",
    }
}

fn thumb_access_for(list: &str, fraction: f64) -> ThumbAccess {
    let value = if fraction.is_finite() { fraction.clamp(0.0, 1.0) } else { 0.0 };
    ThumbAccess {
        label: thumb_label(list).to_string(),
        value,
        value_text: format!("{}%", (value * 100.0).round() as u32),
        page_share: PAGE_SHARE,
    }
}

#[uniffi::export]
impl AppCore {
    /// A scroll thumb as VoiceOver has it: `list` is `contents`, `tabs`, `find` or `pages`, and `fraction` how far down the list is, 0 to 1.
    pub fn thumb_access(&self, list: String, fraction: f64) -> ThumbAccess {
        thumb_access_for(&list, fraction)
    }

    /// `recto qa accessibility`'s value, read: nothing if it isn't one.
    pub fn qa_accessibility_step(&self, value: String) -> Option<AccessibilityStep> {
        qa_step(&value)
    }

    /// What's said when `recto qa accessibility`'s step did nothing.
    pub fn qa_accessibility_refusal(&self, value: String) -> String {
        refusal(qa_step(&value).as_ref()).to_string()
    }

    /// The thumb's opacity with Increase Contrast on.
    pub fn thumb_contrast(&self) -> ThumbContrast {
        let thumb = recto_session::SCROLL_THUMB;
        ThumbContrast {
            light_alpha: thumb.light_contrast.1,
            light_hover_alpha: thumb.light_contrast_hover.1,
            dark_alpha: thumb.dark_contrast.1,
            dark_hover_alpha: thumb.dark_contrast_hover.1,
        }
    }

    /// The action a contents row offers for its fold, in words, or nothing for a row with no sections under it.
    pub fn fold_action_name(&self, row: SidebarRow) -> Option<String> {
        (row.kind == RowKind::Contents && row.expandable).then(|| if row.expanded { "Fold Section" } else { "Unfold Section" }.to_string())
    }

    /// A contents row's fold action, as VoiceOver calls it: what a click on its fold does, and a fold is kept for → to open again.
    pub fn fold_action(&self, first: u32) {
        let entry = lock(&self.inner.published)
            .contents
            .get(first as usize)
            .map(|entry| (entry.tab, entry.entry, entry.expanded));
        if let Some((tab, entry, true)) = entry {
            *lock(&self.inner.folds.last) = Some((tab, entry));
        }
        self.toggle_contents_section(first);
    }

    /// A key with the sidebar's list holding the keyboard: `left` folds the section being read, `right` opens it. Says whether it did anything, so the key goes on to whatever else takes it if not.
    pub fn contents_fold_key(&self, key: String) -> bool {
        let Some(key) = FoldKey::from_name(&key) else { return false };
        let published = lock(&self.inner.published);
        if !published.sidebar.toc_open {
            return false;
        }
        let (contents, current) = (std::sync::Arc::clone(&published.contents), published.sidebar.contents_current);
        drop(published);
        let row = |at: usize| {
            let entry = &contents[at];
            FoldRow {
                tab: entry.tab,
                entry: entry.entry,
                depth: entry.depth,
                expandable: entry.expandable,
                expanded: entry.expanded,
            }
        };
        let mut last = lock(&self.inner.folds.last);
        let Some(target) = fold_target(contents.len(), row, current.map(|at| at as usize), *last, key) else {
            return false;
        };
        if key == FoldKey::Fold {
            let folded = row(target);
            *last = Some((folded.tab, folded.entry));
        }
        drop(last);
        self.toggle_contents_section(u32::try_from(target).unwrap_or(u32::MAX));
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(depth: u8, expandable: bool, expanded: bool) -> FoldRow {
        FoldRow {
            tab: 1,
            entry: 0,
            depth,
            expandable,
            expanded,
        }
    }

    fn target(rows: &[FoldRow], current: Option<usize>, key: FoldKey) -> Option<usize> {
        fold_target(rows.len(), |at| rows[at], current, None, key)
    }

    /// Failure: ← and → with the sidebar's list holding the keyboard fold the wrong section, or one the reader isn't in, or open what's open, or do something past either end, so a section can't be folded without a pointer, or is folded by surprise.
    #[test]
    fn the_arrows_fold_and_open_the_section_being_read() {
        // Chapter 1, open, with two sections, the second open with one under it; chapter 2, folded.
        let rows = [
            row(0, true, true),
            row(1, false, false),
            row(1, true, true),
            row(2, false, false),
            row(0, true, false),
        ];
        assert_eq!(target(&rows, Some(1), FoldKey::Fold), Some(0), "a section's line folds the chapter it's in");
        assert_eq!(target(&rows, Some(2), FoldKey::Fold), Some(2), "an open section folds itself");
        assert_eq!(target(&rows, Some(3), FoldKey::Fold), Some(2), "the nearest section it's in");
        assert_eq!(target(&rows, Some(0), FoldKey::Fold), Some(0));
        assert_eq!(target(&rows, Some(4), FoldKey::Fold), None, "a folded chapter at the top has nothing to fold");
        assert_eq!(target(&rows, Some(4), FoldKey::Unfold), Some(4));
        assert_eq!(target(&rows, Some(0), FoldKey::Unfold), None, "it's open already");
        assert_eq!(target(&rows, Some(1), FoldKey::Unfold), None, "nothing under it");
        assert_eq!(target(&rows, None, FoldKey::Fold), None, "nothing being read");
        assert_eq!(target(&rows, Some(9), FoldKey::Fold), None, "past the end");
        assert_eq!(FoldKey::from_name("up"), None);
        // The row being read folded away: → opens what was folded last, if it's still folded.
        let rows: Vec<FoldRow> = [(0, true, false), (0, true, false)]
            .iter()
            .enumerate()
            .map(|(at, (depth, expandable, expanded))| FoldRow {
                entry: at as u32 * 2,
                ..row(*depth, *expandable, *expanded)
            })
            .collect();
        assert_eq!(fold_target(rows.len(), |at| rows[at], None, Some((1, 2)), FoldKey::Unfold), Some(1));
        assert_eq!(
            fold_target(rows.len(), |at| rows[at], None, Some((2, 2)), FoldKey::Unfold),
            None,
            "another tab's"
        );
        assert_eq!(fold_target(rows.len(), |at| rows[at], None, Some((1, 2)), FoldKey::Fold), None);
    }

    /// Failure: VoiceOver hears a thumb's place outside 0 to 100%, or as nonsense for a list with nothing to scroll, or an increment moves it by other than a click beside it does.
    #[test]
    fn a_thumb_says_where_its_list_is() {
        let at = thumb_access_for("contents", 0.404);
        assert_eq!((at.label.as_str(), at.value, at.value_text.as_str()), ("Contents", 0.404, "40%"));
        assert_eq!(thumb_access_for("tabs", 2.0).value_text, "100%");
        assert_eq!(thumb_access_for("tabs", f64::NAN).value, 0.0);
        assert_eq!(thumb_access_for("anything", 0.0).label, "List");
        assert!(at.page_share > 0.5 && at.page_share < 1.0);
    }

    /// Failure: with Increase Contrast on the thumb is drawn no stronger than without it, or so strong under the pointer it's the same as at rest, so the setting changes nothing anyone can see.
    #[test]
    fn increase_contrast_draws_the_thumb_stronger() {
        let thumb = recto_session::SCROLL_THUMB;
        for (usual, contrast) in [
            (thumb.light, thumb.light_contrast),
            (thumb.light_hover, thumb.light_contrast_hover),
            (thumb.dark, thumb.dark_contrast),
            (thumb.dark_hover, thumb.dark_contrast_hover),
        ] {
            assert_eq!(usual.0, contrast.0, "the same colour");
            assert!(contrast.1 >= usual.1 * 1.6 && contrast.1 <= 1.0, "{usual:?} {contrast:?}");
        }
        assert!(thumb.light_contrast_hover.1 > thumb.light_contrast.1 && thumb.dark_contrast_hover.1 > thumb.dark_contrast.1);
    }

    /// Failure: `recto qa accessibility` passes a word the host can't do on to it, which reports success for nothing, or refuses one it can.
    #[test]
    fn qa_takes_only_what_the_host_does() {
        for good in [
            "contrast:on",
            "contrast:off",
            "key:left",
            "key:right",
            "fold:3",
            "thumb:contents:increment",
            "thumb:pages:decrement",
        ] {
            assert!(is_qa_value(good), "{good}");
        }
        for wrong in [
            "",
            "contrast",
            "key:up",
            "fold:-1",
            "fold:x",
            "thumb:contents",
            "thumb:menu:increment",
            "thumb:tabs:press",
        ] {
            assert!(!is_qa_value(wrong), "{wrong}");
        }
        assert_eq!(qa_step("fold:3"), Some(AccessibilityStep::FoldAction { row: 3 }));
        assert_eq!(
            qa_step("thumb:pages:decrement"),
            Some(AccessibilityStep::Thumb {
                list: "pages".into(),
                increment: false
            })
        );
        assert_ne!(refusal(qa_step("key:left").as_ref()), refusal(None), "each says what it was");
    }
}
