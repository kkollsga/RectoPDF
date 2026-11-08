//! **Links under the pointer,** for the tab showing: which are where, read once a page is pointed at, and whether a press and release was a click on one.
//!
//! A page's links are read on the page view's own thread from the file's structure (`recto_core::nav`), never on the UI thread, which asks only what's here: a page not read yet says nothing for now and is read next. At most [`MAX_PAGES_HELD`] pages are held, the oldest going first, and none for a tab that isn't showing, so a cold tab holds nothing here.

use recto_core::nav::{LinkTarget, PageLinks};
use recto_index::select::PagePoint;
use std::collections::VecDeque;
use std::sync::Arc;

/// Most pages whose links are held. A reader points at a handful.
pub const MAX_PAGES_HELD: usize = 64;
/// A press that moves this far, in points on the page, is a drag, not a click.
pub const CLICK_SLOP: f32 = 3.0;

/// A link the pointer is over, on a page counted as the view counts it.
#[derive(Clone, Debug, PartialEq)]
pub struct LinkHit {
    pub page: u32,
    /// `[x0, y0, x1, y1]` in points on the page as the file shows it.
    pub rect: [f32; 4],
    pub target: LinkTarget,
}

#[derive(Clone, Copy, Debug)]
struct Press {
    point: PagePoint,
    clicks: u32,
    moved: bool,
}

/// What's known of links on the tab showing.
#[derive(Debug, Default)]
pub struct Linking {
    held: VecDeque<(u32, Arc<PageLinks>)>,
    /// A page the pointer's been over that hasn't been read.
    wanted: Option<u32>,
    /// The page being read now.
    asked: Option<u32>,
    press: Option<Press>,
    clicked: Option<LinkHit>,
}

impl Linking {
    /// Another tab, other pages, or the file's changed: nothing held is about what's showing.
    pub fn clear(&mut self) {
        *self = Self::default();
    }

    /// The link at a point, if the page's links are here. `None` either way if they're not, and then the page is wanted.
    pub fn at(&mut self, point: PagePoint) -> Option<LinkHit> {
        match self.held.iter().find(|(page, _)| *page == point.page) {
            Some((page, links)) => links.at(point.x, point.y).map(|link| LinkHit {
                page: *page,
                rect: link.rect,
                target: link.target.clone(),
            }),
            None => {
                if self.asked != Some(point.page) {
                    self.wanted = Some(point.page);
                }
                None
            }
        }
    }

    /// Whether a page's links are here, read or found to have none.
    pub fn knows(&self, page: u32) -> bool {
        self.held.iter().any(|(held, _)| *held == page)
    }

    /// The page to read next, taken: it's being asked for now.
    pub fn next_wanted(&mut self) -> Option<u32> {
        let page = self.wanted.take()?;
        if self.knows(page) {
            return None;
        }
        self.asked = Some(page);
        Some(page)
    }

    /// A page's links have been read, or couldn't be, which is held as none so it isn't read again and again.
    pub fn take(&mut self, page: u32, links: Option<Arc<PageLinks>>) {
        if self.asked == Some(page) {
            self.asked = None;
        }
        self.held.retain(|(held, _)| *held != page);
        self.held.push_back((page, links.unwrap_or_default()));
        while self.held.len() > MAX_PAGES_HELD {
            self.held.pop_front();
        }
    }

    /// Reading a page was stopped before it said anything. It's asked again when it's pointed at again.
    pub fn cancelled(&mut self, page: u32) {
        if self.asked == Some(page) {
            self.asked = None;
        }
    }

    /// The mouse went down here.
    pub fn press(&mut self, point: Option<PagePoint>, clicks: u32) {
        self.clicked = None;
        self.press = point.map(|point| Press { point, clicks, moved: false });
    }

    /// The mouse moved with the button down.
    pub fn drag(&mut self, point: Option<PagePoint>) {
        if let Some(press) = &mut self.press {
            let far = point.is_none_or(|point| {
                point.page != press.point.page || (point.x - press.point.x).abs() > CLICK_SLOP || (point.y - press.point.y).abs() > CLICK_SLOP
            });
            press.moved |= far;
        }
    }

    /// The mouse went up. A single click that didn't move, on a link whose page is here, is that link's to follow, taken with [`Self::take_clicked`].
    pub fn release(&mut self) {
        let Some(press) = self.press.take() else { return };
        if press.clicks != 1 || press.moved {
            return;
        }
        self.clicked = self.held.iter().find(|(page, _)| *page == press.point.page).and_then(|(page, links)| {
            links.at(press.point.x, press.point.y).map(|link| LinkHit {
                page: *page,
                rect: link.rect,
                target: link.target.clone(),
            })
        });
    }

    pub fn take_clicked(&mut self) -> Option<LinkHit> {
        self.clicked.take()
    }

    /// Pages held, for QA and the budgets.
    pub fn pages_held(&self) -> usize {
        self.held.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use recto_core::nav::Link;

    fn page_with_link() -> Arc<PageLinks> {
        Arc::new(PageLinks {
            links: vec![Link {
                rect: [10.0, 10.0, 100.0, 30.0],
                target: LinkTarget::Page { page: 4, within: None },
            }],
            ..PageLinks::default()
        })
    }

    fn at(page: u32, x: f32, y: f32) -> PagePoint {
        PagePoint { page, x, y }
    }

    /// Failure: the pointer over a page not read yet reads nothing, so the hand never shows; or a page is read again on every move; or more pages are held than the cap.
    #[test]
    fn a_page_is_read_once_when_pointed_at_and_held_within_the_cap() {
        let mut linking = Linking::default();
        assert_eq!(linking.at(at(2, 20.0, 20.0)), None);
        assert_eq!(linking.next_wanted(), Some(2));
        assert_eq!(linking.at(at(2, 20.0, 20.0)), None, "being read");
        assert_eq!(linking.next_wanted(), None, "not asked twice");
        linking.take(2, Some(page_with_link()));
        assert!(linking.at(at(2, 20.0, 20.0)).is_some());
        assert!(linking.at(at(2, 200.0, 20.0)).is_none());
        assert_eq!(linking.next_wanted(), None);
        for page in 10..(10 + MAX_PAGES_HELD as u32 * 2) {
            linking.take(page, None);
        }
        assert_eq!(linking.pages_held(), MAX_PAGES_HELD);
    }

    /// Failure: a drag that starts on a link follows it when it ends, as does a double-click; or a plain click on one doesn't.
    #[test]
    fn only_a_still_single_click_follows() {
        let mut linking = Linking::default();
        linking.take(0, Some(page_with_link()));
        linking.press(Some(at(0, 20.0, 20.0)), 1);
        linking.drag(Some(at(0, 21.0, 21.0)));
        linking.release();
        assert!(matches!(linking.take_clicked(), Some(LinkHit { page: 0, .. })));
        assert_eq!(linking.take_clicked(), None, "once");
        linking.press(Some(at(0, 20.0, 20.0)), 1);
        linking.drag(Some(at(0, 60.0, 20.0)));
        linking.release();
        assert_eq!(linking.take_clicked(), None, "a drag selects");
        linking.press(Some(at(0, 20.0, 20.0)), 2);
        linking.release();
        assert_eq!(linking.take_clicked(), None, "a double-click takes a word");
        linking.press(Some(at(0, 200.0, 200.0)), 1);
        linking.release();
        assert_eq!(linking.take_clicked(), None, "off any link");
    }
}
