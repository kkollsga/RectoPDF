//! A read the Mac was too busy for. Its own file: it signals a real render worker, which `search.rs` itself may never start (`a_search_starts_no_program`).

use super::*;

/// Wakes and ends the stopped worker on every path, an assertion's included.
struct Stopped(u32);

impl Stopped {
    fn signal(&self, signal: &str) -> bool {
        std::process::Command::new("/bin/kill")
            .arg(signal)
            .arg(self.0.to_string())
            .status()
            .is_ok_and(|status| status.success())
    }
}

impl Drop for Stopped {
    fn drop(&mut self) {
        self.signal("-CONT");
        self.signal("-KILL");
    }
}

/// Failure (seen on a busy Mac, 2026-11-25): a page the worker had no processor time to read in its deadline comes back as a page that "couldn't be read", so Find says a good page is unreadable and a search lists it with the failed. The Mac being busy is counted by the renderer (`too_busy`), and the page is `Busy`, which a search again reads. Made by stopping the worker, so it has no processor at all; only that one read is short of time (the seam), and the read after runs on a fresh worker under the ordinary deadline.
#[test]
fn a_read_the_mac_was_too_busy_for_isnt_a_page_that_failed() {
    let pdf = recto_testpdf::pages(&[
        recto_testpdf::Page::new(recto_testpdf::text_lines(&["one"])),
        recto_testpdf::Page::new(recto_testpdf::text_lines(&["two"])),
        recto_testpdf::Page::new(recto_testpdf::text_lines(&["three"])),
    ]);
    let folder = recto_testpdf::TempDir::new_in(&std::env::temp_dir().canonicalize().unwrap(), "recto-search-busy-");
    let path = folder.join("doc.pdf");
    std::fs::write(&path, &pdf).unwrap();
    let plan = Plan {
        files: vec![PlanFile {
            path,
            expected: None,
            added: false,
        }],
        pages: (0..3).map(|page| (page, 0, page)).collect(),
        tab: 1,
        keys: None,
    };
    let far = far_future();
    let settings = Settings {
        limit: 20,
        context: 0,
        deadline: far,
        draw_until: far,
        second_pass_until: far,
        live_budget: Duration::from_secs(5),
        indexing: None,
        force_scan: false,
    };
    let mut reader = Reader::new(&plan, &settings);
    let mut slot = Slot::default();
    assert!(matches!(reader.read(0, 0, &mut slot).unwrap(), Got::Page(_)), "a page reads on its own");
    let pid = reader.current.as_mut().unwrap().1.worker_pid().expect("a worker with the file open");
    let stopped = Stopped(pid);
    assert!(stopped.signal("-STOP"));
    let said = recto_render_service::under_page_deadline_for_test(
        &mut reader,
        |reader| &mut reader.current.as_mut().unwrap().1,
        Duration::from_millis(250),
        |reader| match reader.read(0, 1, &mut slot) {
            Ok(Got::Busy) => "busy".to_string(),
            Ok(Got::Failed(why)) => format!("failed: {why}"),
            Ok(Got::Page(_)) => "page".to_string(),
            Ok(_) => "something else".to_string(),
            Err(error) => format!("error: {}", error.message),
        },
    );
    assert_eq!(said, "busy", "{:?}", reader.current.as_mut().unwrap().1.stats());
    let again = reader.read(0, 1, &mut slot).map(|got| matches!(got, Got::Page(_)));
    assert!(matches!(again, Ok(true)), "read again on a fresh worker: {again:?}");
    drop(stopped);
}
