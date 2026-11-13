use super::*;

/// Failure: a job for a place the reader has left runs to the end, so a fast scroll on a heavy page queues seconds of work. It's told to stop, once, and not while it's still in view.
#[test]
fn a_job_nobody_wants_any_more_is_cancelled() {
    let mut reader = reader_on(100);
    let job = a_draw_job(&mut reader, DrawKind::Tile);
    reader.note_started(&job, None);
    reader.set_viewport(view(0.0));
    assert_eq!(reader.counts().cancels_asked, 0, "still in view");
    reader.set_viewport(view(40_000.0));
    assert_eq!(reader.counts().cancels_asked, 1, "the reader left");
    reader.set_viewport(view(41_000.0));
    assert_eq!(reader.counts().cancels_asked, 1, "told once");
    reader.finish(job, Outcome::Cancelled);
}

/// Failure: a tile being drawn for the zoom before this one runs to the end, on a heavy page for seconds, since its scale not being the view's used to count as wanted, to spare previews. It stops, and a preview, whose scale never is the view's, doesn't.
#[test]
fn a_job_for_an_old_zoom_is_cancelled_and_a_preview_is_not() {
    let mut reader = reader_on(100);
    let job = a_draw_job(&mut reader, DrawKind::Tile);
    reader.note_started(&job, None);
    reader.set_viewport(Viewport {
        magnification: 1.5,
        ..view(0.0)
    });
    assert_eq!(reader.counts().cancels_asked, 1, "the zoom moved on");
    reader.finish(job, Outcome::Cancelled);

    let mut reader = reader_on(100);
    let job = a_draw_job(&mut reader, DrawKind::Preview);
    reader.note_started(&job, None);
    reader.set_viewport(Viewport {
        magnification: 1.5,
        ..view(0.0)
    });
    assert_eq!(reader.counts().cancels_asked, 0, "its page is still in view");
    reader.finish(job, Outcome::Cancelled);
}

/// Failure: a pinch draws at every scale it passes through, so it stutters and throws work away.
#[test]
fn nothing_is_redrawn_during_a_pinch() {
    let mut reader = reader_on(10);
    settle(&mut reader);
    let scale = reader.report().0["tile_scale"].clone();
    reader.set_viewport(Viewport {
        magnification: 1.7,
        live: true,
        ..view(0.0)
    });
    assert_eq!(reader.report().0["tile_scale"], scale);
    assert!(reader.next_job().is_none());
}

/// Failure: a heavy page is drawn a tile at a time, each pass costing it half a second, as the first version did on the USGS map.
#[test]
fn a_heavy_page_is_drawn_in_regions_with_no_preview() {
    let mut reader = reader_on(1);
    let mut kinds = Vec::new();
    for _ in 0..64 {
        let Some(job) = reader.next_job() else { break };
        let outcome = match &job.kind {
            JobKind::Size => Outcome::Size(PageBox { width: 1224.0, height: 1584.0 }),
            JobKind::Text { .. } => panic!("no text was asked for"),
            JobKind::Links => Outcome::Links(None),
            JobKind::Traits => Outcome::Heavy(true),
            JobKind::Draw { what, .. } => {
                kinds.push(*what);
                Outcome::Drawn {
                    kind: TileKind::Colour,
                    low_res: None,
                }
            }
        };
        reader.finish(job, outcome);
        reader.drain(64);
        reader.applied();
    }
    assert!(kinds.contains(&DrawKind::Region), "{kinds:?}");
    assert!(!kinds.contains(&DrawKind::Preview), "{kinds:?}");
    assert!(reader.is_settled(), "{}", reader.report().0);
}

/// Failure: a page whose size can't be read is asked about again at once, for ever: 100% of a core on a hostile page, nothing else is drawn, and everything waiting for it to settle times out.
#[test]
fn a_page_whose_size_cant_be_read_is_asked_once() {
    let mut reader = reader_on(3);
    let mut asked = 0;
    for _ in 0..200 {
        reader.host.take_ops();
        reader.applied();
        let Some(job) = reader.next_job() else { break };
        let outcome = match &job.kind {
            JobKind::Size if job.position == 0 => {
                asked += 1;
                Outcome::PageFailed {
                    message: "This page couldn't be read.".into(),
                    may_pass: false,
                }
            }
            JobKind::Size => Outcome::Size(letter()),
            JobKind::Text { .. } => panic!("no text was asked for"),
            JobKind::Links => Outcome::Links(None),
            JobKind::Traits => Outcome::Heavy(false),
            JobKind::Draw { .. } => Outcome::Drawn {
                kind: TileKind::Colour,
                low_res: None,
            },
        };
        reader.finish(job, outcome);
        reader.drain(64);
        reader.applied();
    }
    assert_eq!(asked, 1);
    assert!(reader.is_settled(), "{}", reader.report().0);
    assert_eq!(reader.note(), Some("This page couldn't be read."));
}

/// Failure: a page is deleted while a job on it is running, and when the job ends its old picture goes in the place of the page now there, reading as settled. The same for a size or a heavy answer about what used to be there.
#[test]
fn what_a_job_found_before_an_edit_is_thrown_away() {
    let edited = PageMap {
        files: vec![PathBuf::from("/d/a.pdf")],
        slots: Some(vec![(0, 1), (0, 2)]),
        own: None,
        turns: None,
        expected: Vec::new(),
        follows: None,
    };
    // A job of each kind, caught running.
    let mut reader = reader_on(3);
    let size = reader.next_job().expect("a size");
    assert!(matches!(size.kind, JobKind::Size));
    reader.activate(1, 2, 0, edited.clone(), &[], None, Spread::Single);
    reader.finish(size, Outcome::Size(PageBox { width: 100.0, height: 100.0 }));
    assert!(!reader.doc().unwrap().layout.is_known(0), "that size was the old page's");

    let mut reader = reader_on(3);
    let traits = loop {
        let job = reader.next_job().expect("a job");
        match job.kind {
            JobKind::Size => reader.finish(job, Outcome::Size(letter())),
            JobKind::Text { .. } => panic!("no text was asked for"),
            JobKind::Links => reader.finish(job, Outcome::Links(None)),
            JobKind::Traits => break job,
            JobKind::Draw { .. } => panic!("traits come before drawing"),
        }
    };
    let position = traits.position;
    reader.activate(1, 2, 0, edited.clone(), &[], None, Spread::Single);
    reader.finish(traits, Outcome::Heavy(true));
    assert!(!reader.doc().unwrap().heavy.contains_key(&position), "that was the old page's");

    let mut reader = reader_on(3);
    let draw = loop {
        let job = reader.next_job().expect("a job");
        match job.kind {
            JobKind::Size => reader.finish(job, Outcome::Size(letter())),
            JobKind::Text { .. } => panic!("no text was asked for"),
            JobKind::Links => reader.finish(job, Outcome::Links(None)),
            JobKind::Traits => reader.finish(job, Outcome::Heavy(false)),
            JobKind::Draw { .. } => break job,
        }
    };
    reader.note_started(&draw, None);
    reader.activate(1, 2, 0, edited, &[], None, Spread::Single);
    reader.finish(
        draw,
        Outcome::Drawn {
            kind: TileKind::Colour,
            low_res: None,
        },
    );
    assert!(reader.entries.is_empty(), "the old page's picture stays out");
    assert!(reader.cover.is_empty());
}

/// Failure: a surface that held one file's page is handed to another file's worker, so a hostile file reads a page of the one before it. Both ways round: the next file's worker gets surfaces a cold tab's tiles had, and a hostile file that kept a reference, answered "blank" and got its surface taken back then sees the next file's pixels and can repaint them. No surface may ever serve two files. Each is held here, as a hostile worker holds it, so its name can't be given to a new one by chance.
#[test]
fn no_surface_serves_two_files() {
    fn run(reader: &mut Reader, answer: TileKind, held: &mut Vec<PixelSurface>) -> HashSet<u32> {
        let mut seen = HashSet::new();
        for _ in 0..1000 {
            reader.host.take_ops();
            reader.applied();
            let Some(job) = reader.next_job() else { break };
            let outcome = match &job.kind {
                JobKind::Size => Outcome::Size(letter()),
                JobKind::Text { .. } => panic!("no text was asked for"),
                JobKind::Links => Outcome::Links(None),
                JobKind::Traits => Outcome::Heavy(false),
                JobKind::Draw { surface, .. } => {
                    seen.insert(surface.id());
                    held.push(surface.clone());
                    Outcome::Drawn { kind: answer, low_res: None }
                }
            };
            reader.finish(job, outcome);
        }
        seen
    }
    let other = |name: &str| PageMap {
        files: vec![PathBuf::from(name)],
        slots: None,
        own: None,
        turns: None,
        expected: Vec::new(),
        follows: None,
    };
    for answer in [TileKind::Colour, TileKind::Blank { rgb: 0xFF_FF_FF }] {
        let mut held = Vec::new();
        let mut reader = reader_on(3);
        let first = run(&mut reader, answer, &mut held);
        reader.activate(2, 3, 0, other("/d/b.pdf"), &[], None, Spread::Single);
        reader.set_viewport(view(0.0));
        let second = run(&mut reader, TileKind::Colour, &mut held);
        assert!(!first.is_empty() && !second.is_empty());
        assert!(first.is_disjoint(&second), "served both: {:?}", first.intersection(&second).collect::<Vec<_>>());
    }
}

/// Failure (review O5): a protected file's renderer finds it needs a password the keyring has none for, as when the file was moved to another volume or replaced while it was open, and the tab only says "needs a password", with no way to type one: the app has to hear of the file, once, to ask with the sheet. Another failure isn't that, and a password given clears it.
#[test]
fn a_file_the_renderer_has_no_password_for_is_told_to_the_app() {
    let mut reader = reader_on(3);
    let job = reader.next_job().expect("a job");
    reader.finish(
        job,
        Outcome::DocumentFailed {
            message: "This PDF needs a password.".into(),
            needs_key: true,
        },
    );
    assert_eq!(reader.take_keyless(), [PathBuf::from("/d/a.pdf")]);
    assert!(reader.take_keyless().is_empty(), "told once");
    let mut other = reader_on(3);
    let job = other.next_job().expect("a job");
    other.finish(
        job,
        Outcome::DocumentFailed {
            message: "Not a PDF.".into(),
            needs_key: false,
        },
    );
    assert!(other.take_keyless().is_empty(), "only for want of a password");
    let job = reader.next_job();
    if let Some(job) = job {
        reader.finish(
            job,
            Outcome::DocumentFailed {
                message: "This PDF needs a password.".into(),
                needs_key: true,
            },
        );
    }
    reader.key_given(Path::new("/d/a.pdf"));
    assert!(reader.take_keyless().is_empty(), "a password given takes it off the list");
}

/// Failure: a page that couldn't be drawn is asked for on every scroll, or holds up the rest, or shows no reason.
#[test]
fn a_failed_page_says_why_and_isnt_asked_for_again() {
    let mut reader = reader_on(3);
    for _ in 0..16 {
        let Some(job) = reader.next_job() else { break };
        let outcome = match &job.kind {
            JobKind::Size => Outcome::Size(letter()),
            JobKind::Text { .. } => panic!("no text was asked for"),
            JobKind::Links => Outcome::Links(None),
            JobKind::Traits => Outcome::Heavy(false),
            JobKind::Draw { .. } if job.position == 0 => Outcome::PageFailed {
                message: "This page couldn't be drawn.".into(),
                may_pass: false,
            },
            JobKind::Draw { .. } => Outcome::Drawn {
                kind: TileKind::Colour,
                low_res: None,
            },
        };
        reader.finish(job, outcome);
    }
    reader.set_viewport(view(0.0));
    assert_eq!(reader.note(), Some("This page couldn't be drawn."));
    assert!(reader.next_job().is_none_or(|job| job.position != 0));
}
