use super::*;

struct Any;

impl DocLoader for Any {
    fn load(&self, _: &Path) -> Result<DocFacts, OpenError> {
        Ok(DocFacts {
            page_count: 3,
            byte_len: 1,
            encrypted: false,
            version: "1.7".into(),
            title: None,
            toc: Toc::default(),
            renderer_only: false,
        })
    }
}

/// Failure: File > Open Recent misses a file just opened, lists one twice, grows without a cap, or forgets them all after a quit; or Clear Menu leaves any.
#[test]
fn recent_files_are_newest_first_capped_and_kept() {
    let mut s = Session::new(Any);
    for at in 0..(MAX_RECENT + 5) {
        s.open(&PathBuf::from(format!("/d/file{at}.pdf"))).unwrap();
    }
    s.open(Path::new("/d/file10.pdf")).unwrap();
    let recent = s.recent_files();
    assert_eq!(recent.len(), MAX_RECENT);
    assert_eq!(recent[0].path, PathBuf::from("/d/file10.pdf"), "opening again brings it to the front");
    assert_eq!(recent.iter().filter(|file| file.path == Path::new("/d/file10.pdf")).count(), 1);
    assert_eq!((recent[0].name.as_str(), recent[0].open.is_some()), ("file10", true));
    let record = s.to_record();
    let mut again = Session::new(Any);
    again.restore(&record, HashMap::new()).unwrap();
    assert_eq!(
        again.recent_files().iter().map(|file| file.path.clone()).collect::<Vec<_>>(),
        recent.iter().map(|file| file.path.clone()).collect::<Vec<_>>()
    );
    let mut forged = record.clone();
    forged.recent = vec![PathField::Text("relative.pdf".into()), PathField::Text("/d/ok.pdf".into())];
    again.restore(&forged, HashMap::new()).unwrap();
    assert_eq!(again.recent_files().len(), 1, "only a whole path is kept from a record");
    assert!(again.recent_mut().clear());
    assert!(again.recent_files().is_empty() && again.to_record().recent.is_empty());
}
