use super::*;

fn kind_of(op: &Op) -> &'static str {
    match op {
        Op::Clear => "Clear",
        Op::Document { .. } => "Document",
        Op::Magnify { .. } => "Magnify",
        Op::ScrollTo { .. } => "ScrollTo",
        Op::ScrollAcross { .. } => "ScrollAcross",
        Op::ShowPage { .. } => "ShowPage",
        Op::HidePage { .. } => "HidePage",
        Op::Attach { .. } => "Attach",
        Op::Detach { .. } => "Detach",
        Op::Highlight { .. } => "Highlight",
    }
}

/// [`settle`], keeping every op the host is given, in order.
fn settle_recording(reader: &mut Reader, seen: &mut Vec<Op>) {
    for _ in 0..10_000 {
        seen.extend(reader.host.take_ops());
        reader.applied();
        let Some(job) = reader.next_job() else { break };
        let outcome = match &job.kind {
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
        seen.extend(reader.drain(MAX_ATTACH_PER_TURN));
        reader.applied();
    }
    for _ in 0..100 {
        if !reader.host.has_ops() {
            break;
        }
        seen.extend(reader.drain(MAX_ATTACH_PER_TURN));
        reader.applied();
    }
}

/// Failure: the host applies a batch in the order it's given, so a batch out of order shows wrongly: a `Clear` after a tile's `Attach` leaves the page bare, a `Magnify` after the `ScrollTo` scrolls at the old zoom, a `ShowPage` before the `Document` places a page in a document of the old size, and a `Detach` of a tile that isn't on a layer, or an `Attach` of one that is, leaves a layer the host can't take off. Checked on a first activation, a tab switch and back, a scroll down and up, and a window closed and opened again.
#[test]
fn the_host_gets_each_batch_in_the_order_it_applies_it() {
    const ORDER: [&str; 7] = ["Clear", "Magnify", "Document", "ScrollTo", "ScrollAcross", "ShowPage", "Attach"];
    let in_order = |what: &str, ops: &[Op], needed: &[&str]| {
        let kinds: Vec<&str> = ops.iter().map(kind_of).collect();
        for kind in needed {
            assert!(kinds.contains(kind), "{what}: no {kind} in {kinds:?}");
        }
        let firsts: Vec<usize> = ORDER.iter().filter_map(|kind| kinds.iter().position(|seen| seen == kind)).collect();
        assert!(firsts.windows(2).all(|pair| pair[0] < pair[1]), "{what}: {kinds:?}");
    };
    // The host's layers, as the ops say: every `Detach` is of a tile on one, and every `Attach` of one that isn't. A page's `HidePage` takes its tiles with it.
    let layers_hold = |what: &str, ops: &[Op]| {
        let mut on: HashMap<u64, u32> = HashMap::new();
        for op in ops {
            match op {
                Op::Clear => on.clear(),
                // A page's layer goes with its tiles.
                Op::HidePage { page } => on.retain(|_, on_page| on_page != page),
                Op::Attach { id, page, .. } => assert!(on.insert(*id, *page).is_none(), "{what}: {id} attached twice"),
                Op::Detach { id } => assert!(on.remove(id).is_some(), "{what}: {id} detached while not attached"),
                _ => {}
            }
        }
    };
    let mut reader = Reader::new();
    let mut all = reader.set_viewport(view(0.0));
    reader.activate(1, 12, 0, map(), &[], None, Spread::Single);
    let first = reader.host.take_ops();
    in_order("a first activation", &first, &["Clear", "Magnify", "Document", "ScrollTo", "ScrollAcross"]);
    all.extend(first);
    all.extend(reader.set_viewport(view(0.0)));
    settle_recording(&mut reader, &mut all);
    for step in (1..=10).chain((0..10).rev()) {
        all.extend(reader.set_viewport(view(f64::from(step) * 700.0)));
        settle_recording(&mut reader, &mut all);
    }
    reader.activate(2, 12, 0, map(), &[1], None, Spread::Single);
    let switch = reader.host.take_ops();
    in_order("a switch to a new tab", &switch, &["Clear", "Magnify", "Document", "ScrollTo", "ScrollAcross"]);
    all.extend(switch);
    all.extend(reader.set_viewport(view(0.0)));
    settle_recording(&mut reader, &mut all);
    reader.activate(1, 12, 0, map(), &[2], None, Spread::Single);
    let back = reader.host.take_ops();
    in_order("a switch back to a tab that's drawn", &back, &ORDER);
    all.extend(back);
    all.extend(reader.set_viewport(view(0.0)));
    settle_recording(&mut reader, &mut all);
    layers_hold("before the window closed", &all);
    reader.window_closed();
    reader.window_opened();
    let reopened = reader.host.take_ops();
    in_order(
        "a window opened again",
        &reopened,
        &["Magnify", "Document", "ScrollTo", "ScrollAcross", "ShowPage"],
    );
    let mut after = reopened;
    after.extend(reader.set_viewport(view(0.0)));
    settle_recording(&mut reader, &mut after);
    for step in 1..=6 {
        after.extend(reader.set_viewport(view(f64::from(step) * 700.0)));
        settle_recording(&mut reader, &mut after);
    }
    assert!(after.iter().any(|op| matches!(op, Op::Detach { .. })), "the scroll took tiles off");
    layers_hold("after the window opened again", &after);
}

/// Failure: closing the window leaves ops queued with nobody to take them. Each `Attach` holds a surface's memory until the window's back, and `qa wait-settled` waits for ever. They're gone, and the surfaces with them, the moment it's closed.
#[test]
fn a_closed_window_keeps_no_ops_or_surfaces_for_them() {
    let mut reader = reader_on(10);
    let draw = a_draw_job(&mut reader, DrawKind::Preview);
    let JobKind::Draw { surface, .. } = &draw.kind else { panic!() };
    let held = surface.clone();
    reader.finish(
        draw,
        Outcome::Drawn {
            kind: TileKind::Colour,
            low_res: None,
        },
    );
    // Attached in Rust, and waiting for a host that hasn't come for it.
    assert!(
        reader.host.ops.iter().any(|op| matches!(op, Op::Attach { surface, .. } if *surface != 0)),
        "{:?}",
        reader.host.ops
    );
    let before = held.reference_count();
    reader.window_closed();
    assert!(reader.host.ops.is_empty(), "{:?}", reader.host.ops);
    assert!(
        held.reference_count() < before - 1,
        "the entry's and the waiting op's both gone: {} from {before}",
        held.reference_count()
    );
    assert!(reader.is_settled(), "{}", reader.report().0);
}

/// Failure: a tile comes off in Rust just after the UI thread took its ops, and "applied" for those ops marks it discardable while its layer is still showing it, so the OS can empty a surface on screen. Only what the host was told about counts.
#[test]
fn a_surface_goes_discardable_only_after_the_host_was_told() {
    let mut reader = reader_on(3);
    settle(&mut reader);
    let (&page, ids) = reader.cache.attached.iter().next().unwrap();
    let id = ids[0];
    reader.host.take_ops();
    // The engine's thread, between the host's taking its ops and saying it has applied them.
    reader.detach(id, page);
    reader.applied();
    assert!(!reader.cache.entries[&id].discardable, "the layer still has it");
    reader.host.take_ops();
    reader.applied();
    assert!(reader.cache.entries[&id].discardable, "now the host has been told");
}

/// Failure: a burst of attachments stalls the UI thread, or the leftovers are forgotten and part of a page stays blank until something else happens.
#[test]
fn at_most_eight_tiles_are_attached_in_a_turn_and_the_rest_follow() {
    let mut reader = reader_on(100);
    let big = Viewport {
        width: 4000.0,
        height: 3000.0,
        frame_width: 1168.0,
        ..view(0.0)
    };
    reader.set_viewport(big);
    settle(&mut reader);
    reader.set_viewport(view(40_000.0));
    settle(&mut reader);
    let ops = reader.set_viewport(big);
    reader.applied();
    let attaches = |ops: &[Op]| ops.iter().filter(|op| matches!(op, Op::Attach { .. })).count();
    assert_eq!(attaches(&ops), MAX_ATTACH_PER_TURN, "a full turn, or this tests nothing");
    assert!(reader.host.has_ops(), "and it says there's more");
    let mut turns = 0;
    while reader.host.has_ops() {
        let more = reader.drain(MAX_ATTACH_PER_TURN);
        reader.applied();
        assert!(attaches(&more) <= MAX_ATTACH_PER_TURN);
        turns += 1;
        assert!(turns < 100, "the leftovers never end");
    }
    assert!(turns >= 1);
    assert!(reader.is_settled(), "the leftovers all came: {}", reader.report().0);
}
