use super::*;

/// Failure: two tabs being woken at once share one place to put what was read, so one is lost and loaded with the session locked. Or a tab closed mid-read keeps its facts, or another tab's are taken for it. Each is kept for its own tab, and only while that tab is cold.
#[test]
fn what_was_read_ahead_stays_with_its_tab() {
    struct Counted(std::rc::Rc<std::cell::Cell<u32>>);
    impl DocLoader for Counted {
        fn load(&self, _: &Path) -> Result<DocFacts, OpenError> {
            self.0.set(self.0.get() + 1);
            Ok(DocFacts {
                page_count: 5,
                byte_len: 1,
                encrypted: false,
                version: "1.7".into(),
                title: None,
                toc: Toc::default(),
                renderer_only: false,
            })
        }
    }
    let loads = std::rc::Rc::new(std::cell::Cell::new(0));
    let mut s = Session::new(Counted(loads.clone()));
    let ids: Vec<TabId> = (0..6).map(|index| s.open(Path::new(&format!("/d/{index}.pdf"))).unwrap()).collect();
    let facts = |pages| {
        Ok(DocFacts {
            page_count: pages,
            byte_len: 1,
            encrypted: false,
            version: "1.7".into(),
            title: None,
            toc: Toc::default(),
            renderer_only: false,
        })
    };
    let base = loads.get();
    // Two wakes done together: both kept, each for its own tab.
    s.sleeper().give_woken(ids[0], facts(11), None);
    s.sleeper().give_woken(ids[1], facts(22), None);
    s.tap(ids[1]).unwrap();
    assert_eq!(s.pages().count(ids[1]), 22);
    assert_eq!(loads.get(), base, "no read with the session held");
    // Closed mid-read: nothing's kept for it.
    s.sleeper().give_woken(ids[2], facts(33), None);
    s.close(ids[2]).unwrap();
    assert!(!s.warm.holds_read_of(ids[2]));
    s.sleeper().give_woken(ids[2], facts(33), None);
    assert!(!s.warm.holds_read_of(ids[2]), "it's gone, so what arrives late isn't kept");
    // Already warm: it's not needed.
    s.sleeper().give_woken(ids[5], facts(55), None);
    assert!(!s.warm.holds_read_of(ids[5]));
    s.tap(ids[0]).unwrap();
    assert_eq!(s.pages().count(ids[0]), 11, "the first's own");
    assert_eq!(loads.get(), base);
}

/// Failure: contents stay in memory for every tab, so many large PDFs cost memory while idle.
#[test]
fn only_a_few_tabs_stay_warm() {
    let mut s = session(10);
    let ids: Vec<_> = (0..20).map(|i| s.open(Path::new(&format!("/d/{i}.pdf"))).unwrap()).collect();
    assert_eq!(s.tier(*ids.last().unwrap()), Tier::Hot);
    let warm = ids.iter().filter(|id| s.tier(**id) == Tier::Warm).count();
    assert!(warm < MAX_WARM);
    assert!(ids[..15].iter().all(|id| s.tier(*id) == Tier::Cold));
}

/// Failure: returning to a recent tab re-reads the file, or a cold tab isn't rebuilt from disk.
#[test]
fn switching_rebuilds_only_cold_tabs() {
    let mut s = session(10);
    let ids: Vec<_> = (0..5).map(|i| s.open(Path::new(&format!("/d/{i}.pdf"))).unwrap()).collect();
    let before = s.loader.loads.get();
    s.tap(ids[3]).unwrap();
    assert_eq!(s.loader.loads.get(), before, "warm tab read the file again");
    s.tap(ids[0]).unwrap();
    assert_eq!(s.loader.loads.get(), before + 1, "cold tab wasn't rebuilt");
}
