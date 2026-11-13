//! **A walk over every tab looks none of them up again** (rule 1, and "a person's action may not wait"). The sidebar is built on every publish and the
//! record on every save of the session's state, and each goes over every tab once; a lookup of a tab by its number inside that walk searches the tabs
//! again, so the walk becomes quadratic in the tabs open. Nobody sees it at ten tabs, and at a thousand every action pays for it: on 2026-11-13 each
//! tab's notice was read by its number in the sidebar, and every action at 1,000 tabs took about 0.5 ms more. Counted, not timed, so it holds on any
//! machine: the records the searches for a tab look at ([`crate::searched`], debug builds only).

use super::*;
use std::hint::black_box;

/// The records the searches for a tab look at while `during` runs, on this thread.
fn searched(during: impl FnOnce()) -> u64 {
    let before = crate::searched::records();
    during();
    crate::searched::records() - before
}

/// `tabs` tabs, the last active, every tenth with an unsaved edit, so the notices' reads of the lists are walked too.
fn opened(tabs: usize) -> Session<FakeLoader> {
    let mut s = session(10);
    for at in 0..tabs {
        let id = s.open(Path::new(&format!("/Users/someone/Documents/f{at:04}.pdf"))).unwrap();
        if at % 10 == 0 {
            s.edit_pages(id, |e| e.delete(0, 1)).unwrap();
        }
    }
    s
}

/// Failure: the sidebar, the record or the list of files to look at after a restore looks a tab up by its number for each tab it walks, so building any
/// of them at 1,000 tabs searches about a million records (what 5cb4acb3 did, through each tab's notice, and in the record and the list through each tab's
/// path). What's left is at most one search over the tabs, for the tab being read: pinned at one search, so any search per tab, even one that finds its tab
/// at once, fails.
#[test]
fn the_walks_over_every_tab_search_the_tabs_at_most_once() {
    for tabs in [100_u64, 1000] {
        let s = opened(tabs as usize);
        let sidebar = searched(|| {
            black_box(s.sidebar());
        });
        let record = searched(|| {
            black_box(s.to_record());
        });
        let looks = searched(|| {
            black_box(s.files_to_look_at());
        });
        println!("{tabs} tabs: the sidebar searched {sidebar} records, the record {record}, the files to look at after a restore {looks}");
        assert!(
            sidebar <= tabs,
            "the sidebar at {tabs} tabs searched {sidebar} records: a tab is looked up again for each tab walked"
        );
        assert!(
            record <= tabs,
            "the record at {tabs} tabs searched {record} records: a tab is looked up again for each tab walked"
        );
        assert!(
            looks <= tabs,
            "the files to look at at {tabs} tabs searched {looks} records: a tab is looked up again for each tab walked"
        );
    }
}
