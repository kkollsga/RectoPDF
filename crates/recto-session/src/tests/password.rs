use super::*;

struct Locked;

impl DocLoader for Locked {
    fn load(&self, _: &Path) -> Result<DocFacts, OpenError> {
        Ok(DocFacts {
            page_count: 3,
            byte_len: 1,
            encrypted: true,
            version: "1.7".into(),
            title: None,
            toc: Toc::default(),
            renderer_only: false,
        })
    }
}

/// Failure (review O1): `encrypted` is taken for "opened with a password", so a file anyone may open is kept from agents, or one the person typed a password for isn't; or the mark's lost across a quit, so what the record kept of it (its length, its place) reaches an agent before the password's typed again; or a tab whose password went missing goes on showing what was read with it instead of asking (review O5).
#[test]
fn a_tab_opened_with_a_password_is_marked_kept_and_asks_again_when_its_key_is_gone() {
    let mut s = Session::new(Locked);
    let anyone = s.open(Path::new("/d/anyone.pdf")).unwrap();
    assert!(!s.withheld_from_agents(anyone), "encrypted alone isn't protected from agents");
    let locked = s
        .open_unlocked(Path::new("/d/locked.pdf"), Locked.load(Path::new("/d/locked.pdf")).unwrap(), None)
        .unwrap();
    assert!(s.withheld_from_agents(locked));
    let record = s.to_record();
    assert_eq!(record.tabs.iter().map(|tab| tab.password).collect::<Vec<_>>(), [false, true]);
    let mut again = Session::new(Locked);
    again.restore(&record, HashMap::new()).unwrap();
    assert!(
        again.withheld_from_agents(TabId(locked.0)) && !again.withheld_from_agents(TabId(anyone.0)),
        "kept across a quit"
    );
    assert!(!s.needs_password(locked));
    s.needs_password_again(locked);
    assert!(s.needs_password(locked) && s.withheld_from_agents(locked));
    assert!(!s.warm_tabs().any(|id| id == locked), "what was read with the password is let go");
}
