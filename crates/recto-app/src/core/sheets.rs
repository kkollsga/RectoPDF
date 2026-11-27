//! **One sheet over the window, in Rust's words** (D6): a feature that asks something puts a [`SheetModel`] in the reader's view, the host draws it as an alert
//! with the buttons in the order given, and the answer comes back as [`UiAction::SheetAnswered`](super::controls::UiAction) with the sheet's kind, id and the
//! button's index. What each button does is the feature's: one line in each of the two tables here.

use super::person::Person;
use super::tools::Tools;
use super::{CoreCall, CoreQueue, HostAction};

/// What a sheet says and offers. The host draws it and decides nothing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SheetModel {
    pub kind: String,
    pub id: u64,
    pub title: String,
    pub message: String,
    pub buttons: Vec<String>,
    pub destructive: Option<u32>,
}

/// The answer of a sheet closed by anything but a button (the window went, or an index the host can't name): never a feature's destructive button.
pub const NO_BUTTON: u32 = u32::MAX;

/// The sheet showing, if any: one feature's at a time, in this order.
pub fn showing(tools: &Tools) -> Option<SheetModel> {
    super::saving::sheet(tools.save_ui()).or_else(|| tools.removal().sheet().showing())
}

/// What answers a sheet of one kind: the person's way in, the tools, the sheet's id and the button.
type Answer = fn(&Person<'_>, &Tools, u64, u32) -> HostAction;

/// One line a feature, by its sheet's kind.
const ANSWERS: &[(&str, Answer)] = &[
    (super::saving::SHEET_KIND, |_, tools, _, button| {
        super::saving::sheet_answered(tools.save_ui(), button)
    }),
    (super::removal::sheet::KIND, |person, tools, id, button| {
        super::removal::sheet::answer_sheet(*person, tools.removal(), id, button)
    }),
];

/// A sheet was answered: the feature that showed it acts, and the view is made again.
pub fn answered(person: Person<'_>, tools: &Tools, core: &CoreQueue, kind: &str, id: u64, button: u32) -> HostAction {
    let Some((_, answer)) = ANSWERS.iter().find(|(known, _)| *known == kind) else {
        return HostAction::None;
    };
    let acted = answer(&person, tools, id, button);
    core.push(CoreCall::Reconcile);
    acted
}
