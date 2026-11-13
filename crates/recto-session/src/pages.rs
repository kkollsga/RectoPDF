//! **Page labels and places in words:** a look over a tab's record, its list of edits and its contents' labels, changing none of them.

use crate::TabId;
use crate::lists::Lists;
use crate::registry::Tabs;
use crate::warm::Warm;

/// What a tab's pages are called and how many it has now, borrowed from the session for one look: [`crate::Session::pages`].
pub struct Pages<'a> {
    pub(crate) tabs: &'a Tabs,
    pub(crate) lists: &'a Lists,
    pub(crate) warm: &'a Warm,
}

impl Pages<'_> {
    /// **What the file calls a page,** zero-based, counting unsaved edits: `iv` for a page of front matter. Nothing for a file without labels, a page added from another file, or a tab whose file isn't read, which shows its number. A moved page keeps its own label, as its file names it.
    pub fn label(&self, id: TabId, position: u32) -> Option<String> {
        let toc = self.warm.toc(id)?;
        if toc.labels.is_empty() {
            return None;
        }
        let page = match self.lists.get(id) {
            None => position,
            Some(edits) => {
                let slot = edits.slots().get(position as usize)?;
                if slot.source != 0 {
                    return None;
                }
                slot.page
            }
        };
        toc.labels.label(page)
    }

    /// A page's label only where it says more than its number: `iv`, but not `4` for the fourth page.
    pub fn label_differs(&self, id: TabId, position: u32) -> Option<String> {
        self.label(id, position).filter(|label| *label != (u64::from(position) + 1).to_string())
    }

    /// **The words for where a tab is,** as the corner shows them: `12 of 96`, or `iv (4 of 96)` where the file calls the page something else.
    pub fn place_words(&self, id: TabId, page: u32) -> String {
        let plain = format!("{} of {}", page + 1, self.count(id));
        match self.label_differs(id, page) {
            Some(label) => format!("{label} ({plain})"),
            None => plain,
        }
    }

    /// **The page typed in the go-to field,** zero-based: a label the file uses first, as Preview and Acrobat read it, then a plain number. Nothing if it's neither.
    pub fn for_typed(&self, id: TabId, typed: &str) -> Option<u32> {
        let typed = typed.trim();
        let pages = self.count(id);
        if let Some(toc) = self.warm.toc(id)
            && !toc.labels.is_empty()
        {
            let file_pages = self.tabs.tab(id).map_or(0, |tab| tab.page_count);
            if let Some(file_page) = toc.labels.find(typed, file_pages) {
                match self.lists.get(id) {
                    None => return Some(file_page),
                    Some(edits) => {
                        if let Some(position) = edits.slots().iter().position(|slot| slot.source == 0 && slot.page == file_page) {
                            return u32::try_from(position).ok();
                        }
                    }
                }
            }
        }
        typed.parse::<u32>().ok().filter(|page| *page >= 1 && *page <= pages).map(|page| page - 1)
    }

    /// **Where a link's page goes now,** with edits counted: `file_page` of the same file as the page at position `from`, which is what a link on that page means. Nothing if that page isn't in the document any more.
    pub fn position_in_same_file(&self, id: TabId, from: u32, file_page: u32) -> Option<u32> {
        match self.lists.get(id) {
            None => (file_page < self.count(id)).then_some(file_page),
            Some(edits) => {
                let source = edits.slots().get(from as usize)?.source;
                edits
                    .slots()
                    .iter()
                    .position(|slot| slot.source == source && slot.page == file_page)
                    .and_then(|at| u32::try_from(at).ok())
            }
        }
    }

    /// How many pages a tab has now, counting unsaved edits.
    pub fn count(&self, id: TabId) -> u32 {
        self.lists.pages(id).or_else(|| self.tabs.tab(id).map(|t| t.page_count)).unwrap_or(0)
    }
}
