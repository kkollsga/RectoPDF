//! A read the Mac was too busy for. Its own file: it signals a real render worker, which `search.rs` itself may never start (`a_search_starts_no_program`).

use super::*;

/// Wakes the stopped worker on every path, an assertion's included. Only -CONT: the pid may be reaped and reused by the time this drops, so nothing is killed by it; the reader (declared first, dropped after) closes the worker, which ends once it runs.
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

/// Failure: the busy arm sat below the read-again arms, or a change moved it, and a page the index marked failed whose worker got no processor came back `Busy`, so a search says "search again" about a page its own index says is damaged. Held only by the order of the arms in `read_live`. A page the index marked failed is read once more by a worker, and a worker that has no time says it failed (`read_deadline`), never that the Mac was busy.
#[test]
fn a_page_the_index_marked_failed_stays_failed_when_the_worker_is_short_of_time() {
    let pdf = recto_testpdf::pages(&[
        recto_testpdf::Page::new(recto_testpdf::text_lines(&["one"])),
        recto_testpdf::Page::new(recto_testpdf::text_lines(&["two"])),
    ]);
    let fixture = tests::fixture("busy-failed", &pdf, vec![None, None]);
    let settings = tests::settings(&fixture);
    let mut reader = Reader::new(&fixture.plan, &settings);
    let mut slot = Slot::default();
    assert!(matches!(reader.read(0, 0, &mut slot).unwrap(), Got::Page(_)), "the first read starts a worker");
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
    drop(stopped);
    assert_eq!(said, "failed: read_deadline");
}

/// Failure: a worker that never says hello (the third busy start in a row is `Unavailable`, and remembered for a moment) made every page asked in that moment "couldn't be read", so Find called a whole file unreadable on a busy Mac, against the rule that a busy page is never "couldn't be read". Now it stops that file for this search: no more pages are asked of it, none is counted as failed, and the file is said once with the reader's own sentence.
#[test]
fn a_renderer_that_wont_start_stops_the_file_and_fails_no_page() {
    let pdf = recto_testpdf::pages(
        &(0..4)
            .map(|n| recto_testpdf::Page::new(recto_testpdf::text_lines(&[&format!("page {n}")])))
            .collect::<Vec<_>>(),
    );
    let folder = recto_testpdf::TempDir::new_in(&std::env::temp_dir().canonicalize().unwrap(), "recto-search-unstarted-");
    let path = folder.join("doc.pdf");
    std::fs::write(&path, &pdf).unwrap();
    let plan = Plan {
        files: vec![PlanFile {
            path,
            expected: None,
            added: false,
        }],
        pages: (0..4).map(|page| (page, 0, page)).collect(),
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
    // A renderer whose worker has no time to say hello: a start timeout of nothing, for every ask, so nothing here waits on a real start.
    let policy = Policy {
        start_timeout: Duration::ZERO,
        ..Policy::default()
    };
    reader.current = Some((
        0,
        DocRenderer::new(&plan.files[0].path, None, ServiceConfig::discover().unwrap(), policy).unwrap(),
    ));
    let said: Vec<&str> = (1..=3)
        .map(|page| match reader.read(0, page, &mut slot) {
            Ok(Got::Busy) => "busy",
            Ok(Got::Nothing) => "nothing",
            Ok(Got::Failed(_)) => "failed",
            _ => "other",
        })
        .collect();
    assert_eq!(
        said,
        ["busy", "busy", "nothing"],
        "two busy starts, then the one that ends the run stops the file without failing a page"
    );
    // Were the file asked again it would be busy again (a fresh run, once the failure is forgotten); it isn't asked, because the file is stopped for this search.
    reader.current.as_mut().unwrap().1.forget_failures();
    assert!(matches!(reader.read(0, 3, &mut slot), Ok(Got::Nothing)), "no later page of the file is asked");
    let coverage = Found::default().coverage(&plan, &reader);
    assert_eq!(coverage["failed_pages"], json!([]), "{coverage}");
    assert!(
        coverage["hits_total"].is_null(),
        "a total of hits over pages that weren't searched is a claim about them: {coverage}"
    );
    let files = coverage["files_not_read"].as_array().expect("the file is listed");
    assert_eq!(files.len(), 1, "said once: {coverage}");
    assert_eq!(files[0]["why"], RenderFailure::Unavailable.message(), "the reader's own sentence");
    assert_eq!(files[0]["code"], "render_unavailable");
}
