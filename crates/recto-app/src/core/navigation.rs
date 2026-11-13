//! **Going to a page, as the person does it** (step R2.3, C5b): a contents row, a link to a page, the go-to field, Back and Forward and the swipe are each the
//! command an agent sends (`goto`, `goto label`, `back`, `forward`), run for the person on the session thread ([`super::person::submit`]), which takes in the
//! page views' places first, as an agent's does. A web link stays the window's own sheet.

use super::*;
use recto_agent::commands::Move;
use recto_agent::commands::linking::{Back, Forward, Goto};

/// **A contents row's page, as `goto` is sent it** (accident A5): a row can name a page the reader doesn't have, as a file whose structure lists more pages
/// than its pages draw, or contents a sleep file kept from before the file was known shorter, so a tap lands on the last page the reader has, as it always
/// did. `goto` itself keeps refusing a page past the end. `shown` is how many pages the reader has of the row's tab, or 0 if it isn't showing.
pub fn contents_page(page: u32, shown: u32) -> u32 {
    let page = page.max(1);
    if shown == 0 { page } else { page.min(shown) }
}

/// What a person's `goto` that didn't go is said as: a typed page that's no page, in the go-to field's words. Any other way it can't go said nothing before,
/// and says nothing now.
pub fn answered(inner: &std::sync::Arc<Inner>, ran: &person::Ran<'_>) {
    if let (Some(_), Command::Move(Move::Goto(Goto { label: Some(typed), .. }))) = (ran.refused(), ran.command) {
        person::say(inner, linking::no_such_page(typed));
    }
}

/// **Going's menu ids** ([`super::menu_rows`]): Back and Forward are `back` and `forward` on the tab being read; Go to Page opens the window's field.
pub fn row<'a>(_: &'a Arc<Inner>, asked: &menu_rows::Asked<'_>) -> Option<menu_rows::Row<'a>> {
    match (asked.action.as_ref()?, asked.id) {
        (menus::Action::Step(back), _) => Some(menu_rows::Row::command(step(*back))),
        (menus::Action::Host(host), "go.page") => Some(menu_rows::Row::host(
            "opens the go-to field for the person to type in; what's typed there is `PageTyped`",
            menu_rows::Agents::ItsOwn("`recto goto <page>`"),
            *host,
        )),
        _ => None,
    }
}

/// Back, or Forward, in the tab being read.
fn step(back: bool) -> Command {
    let tab = "active".to_string();
    if back { Back { tab }.into() } else { Forward { tab }.into() }
}

fn goto(tab: TabId, page: Option<u32>, label: Option<String>, within: Option<f64>) -> Command {
    Goto {
        tab: format!("#{}", tab.0),
        page,
        label,
        within,
    }
    .into()
}

impl Inner {
    /// A contents row tapped: `goto` its page, on its tab.
    pub fn contents_tapped(self: &Arc<Self>, tab: TabId, page: u32) {
        let showing = (0..self.panes.count()).any(|pane| self.panes.tab_of(pane) == Some(tab));
        let shown = if showing {
            self.panes.engine_for_tab(&self.engine, tab).page_count()
        } else {
            0
        };
        person::submit(self, goto(tab, Some(contents_page(page, shown)), None, None));
    }

    /// A link to a page of this document, clicked: `goto` that page and as far down it as the link says, from the page view's hit, with nothing read again.
    /// `position` counts from 0, with edits counted.
    pub fn link_to_page(self: &Arc<Self>, tab: TabId, position: u32, within: Option<f32>) {
        person::submit(self, goto(tab, Some(position + 1), None, within.map(f64::from)));
    }

    /// Back or Forward in the tab showing: ⌘[ and ⌘], a swipe, or a link that says so.
    pub fn step_for_person(self: &Arc<Self>, back: bool) {
        person::submit(self, step(back));
    }

    /// The sheet asking about a web or email address was answered, by its id: with Open, the address, exactly as the sheet showed it; with Cancel, nothing.
    pub fn link_prompt_answered(&self, id: u64, open: bool) -> Option<String> {
        let mut asked = lock(&self.tools.prompts.link);
        // An answer is to the sheet that was shown: a later link's asking is left for its own answer.
        if asked.as_ref().is_none_or(|prompt| prompt.id != id) {
            return None;
        }
        let prompt = asked.take();
        drop(asked);
        self.core.push(CoreCall::Reconcile);
        prompt.filter(|_| open).map(|prompt| prompt.address)
    }

    /// Return in the go-to field: `goto label`, a label the file uses or a page number, on the pane being read's tab. Says whether it was something to look up.
    pub fn page_typed(self: &Arc<Self>, text: &str) -> bool {
        let typed = text.trim();
        if typed.is_empty() || typed.len() > linking::MAX_TYPED_PAGE || self.core.frozen.load(Ordering::SeqCst) {
            return false;
        }
        let reading = self.panes.reading();
        let Some(tab) = self.panes.tab_of(reading) else { return false };
        person::submit(self, goto(tab, None, Some(typed.to_string()), None));
        true
    }

    /// A swipe on the page, as the system reports it: Back or Forward, as in Safari.
    pub fn swiped(self: &Arc<Self>, delta_x: f64) {
        if let Some(back) = linking::swipe_goes_back(delta_x) {
            self.step_for_person(back);
        }
    }
}

#[cfg(test)]
mod tests {
    /// Failure (accident A5): a contents row naming a page past the reader's last is sent to `goto` as it is, which refuses it, so the tap does nothing where
    /// it always landed on the last page; or a row's page is clamped when the reader's count isn't known.
    #[test]
    fn a_contents_row_past_the_end_lands_on_the_last_page() {
        assert_eq!(super::contents_page(99, 12), 12);
        assert_eq!(super::contents_page(5, 12), 5);
        assert_eq!(super::contents_page(12, 12), 12);
        assert_eq!(super::contents_page(99, 0), 99, "not showing: goto says what it says");
        assert_eq!(super::contents_page(0, 12), 1, "pages count from 1");
    }
}
