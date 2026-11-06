use super::*;

fn job() -> RenderJob {
    RenderJob {
        generation: 7,
        job_id: 3,
        page: 2,
        scale: 2.0,
        rect: TileRect { x: 512, y: 0, width: 512, height: 512 },
        stride: 2048,
    }
}

fn tile() -> TileReply {
    TileReply { generation: 7, job_id: 3, width: 512, height: 512, stride: 2048, status: TileStatus::Done(TileKind::Grey), render_us: 9 }
}

fn payload(frame: &[u8]) -> &[u8] {
    &frame[HEADER_LEN..]
}

/// Failure: a wire struct grows, and a queue or tile cache held per item grows with it. Limits are the platform report's §2.9.
#[test]
fn per_item_sizes_stay_small() {
    assert!(size_of::<TileKey>() <= 16, "TileKey is {}", size_of::<TileKey>());
    assert!(size_of::<RenderJob>() <= 64, "RenderJob is {}", size_of::<RenderJob>());
    assert!(size_of::<TileReply>() <= 64, "TileReply is {}", size_of::<TileReply>());
    assert_eq!(size_of::<CharRecord>(), 4, "text is held per character");
}

/// Failure: a field is written in one order and read in another, so tiles land on the wrong page.
#[test]
fn every_message_survives_a_round_trip() {
    assert_eq!(RenderJob::decode(payload(&job().encode())), Ok(job()));
    assert_eq!(TileReply::decode(payload(&tile().encode())), Ok(tile()));
    for status in [TileStatus::Done(TileKind::Blank { rgb: 0xFF_FFFF }), TileStatus::Done(TileKind::Colour), TileStatus::Cancelled] {
        let reply = TileReply { status, ..tile() };
        assert_eq!(TileReply::decode(payload(&reply.encode())), Ok(reply));
    }
    assert_eq!(decode_hello(payload(&encode_hello(42))), Ok(42));
    assert_eq!(decode_opened(payload(&encode_opened(9))), Ok(9));
    assert_eq!(decode_size_request(payload(&encode_size_request(1, 2))), Ok((1, 2)));
    let size = PageSize { job_id: 1, width: 612.0, height: 792.0 };
    assert_eq!(PageSize::decode(payload(&size.encode())), Ok(size));
    let text_job = TextJob { job_id: 4, page: 5, boxes: true, hidden_check: true };
    assert_eq!(TextJob::decode(payload(&text_job.encode())), Ok(text_job));
    let text = TextReply {
        job_id: 4,
        page: 5,
        hidden_checked: true,
        chars: vec![CharRecord::new('a' as u32, 0), CharRecord::new(0xFFFE, CharRecord::HYPHEN), CharRecord::new('😀' as u32, CharRecord::NO_INK)],
        boxes: vec![[1.0, 2.0, 3.0, 4.0]; 3],
    };
    assert_eq!(TextReply::decode(payload(&text.encode().unwrap())), Ok(text));
    assert_eq!(decode_failed(payload(&encode_failed(8, FailCode::TooLarge))), Ok((8, FailCode::TooLarge)));
    assert_eq!(encode_open(b"secret").unwrap().len(), HEADER_LEN + 6);
}

/// Failure: a header from another build or from random bytes is believed, so the host reads garbage as a tile.
#[test]
fn headers_refuse_what_they_cant_vouch_for() {
    let good = Header { kind: Kind::Tile, len: TILE_REPLY_LEN as u32 }.encode();
    assert!(Header::decode(&good).is_ok());
    let mutate = |at: usize, value: u8| {
        let mut bytes = good;
        bytes[at] = value;
        Header::decode(&bytes)
    };
    assert_eq!(mutate(0, b'X'), Err(ProtoError("bad frame magic")));
    assert_eq!(mutate(2, 99), Err(ProtoError("protocol version differs")));
    assert_eq!(mutate(4, 99), Err(ProtoError("unknown frame kind")));
    assert_eq!(mutate(6, 1), Err(ProtoError("reserved header bytes set")));
}

/// Failure: a hostile worker claims a huge frame and the host allocates it before reading a byte.
#[test]
fn a_claimed_length_is_refused_before_anything_is_read() {
    for kind in [Kind::Hello, Kind::Opened, Kind::Tile, Kind::Size, Kind::Failed] {
        let bytes = Header { kind, len: 1024 * 1024 }.encode();
        assert_eq!(Header::decode(&bytes), Err(ProtoError("frame longer than its kind allows")), "{kind:?}");
    }
    let bytes = Header { kind: Kind::Text, len: MAX_FRAME + 1 }.encode();
    assert!(Header::decode(&bytes).is_err());
}

/// Failure: the host trusts a tile whose dimensions differ from the job, and reads past the surface or draws a skewed page.
#[test]
fn bad_tile_metadata_is_rejected() {
    let job = job();
    let len = job.bytes_needed();
    assert_eq!(validate_tile(&job, &tile(), len), Ok(()));
    let bad = [
        (TileReply { job_id: 4, ..tile() }, "different job"),
        (TileReply { generation: 6, ..tile() }, "different generation"),
        (TileReply { width: 513, ..tile() }, "wider"),
        (TileReply { height: u32::MAX, ..tile() }, "taller"),
        (TileReply { stride: 4, ..tile() }, "short stride"),
        (TileReply { width: 0, height: 0, ..tile() }, "empty"),
    ];
    for (reply, why) in bad {
        assert!(validate_tile(&job, &reply, len).is_err(), "{why} was accepted");
    }
    assert!(validate_tile(&job, &tile(), len - 1).is_err(), "a surface too small was accepted");
    let cancelled = TileReply { status: TileStatus::Cancelled, width: 0, height: 0, stride: 0, ..tile() };
    assert_eq!(validate_tile(&job, &cancelled, len), Ok(()), "a cancelled tile has no pixels to check");
}

/// Failure: a claimed stride and height multiply past the cap or overflow, and the check passes on a wrapped number.
#[test]
fn large_dimensions_dont_overflow_the_check() {
    let stride = u32::MAX;
    assert!(check_dimensions(MAX_TILE_EDGE, MAX_TILE_EDGE, stride).is_err());
    assert!(check_dimensions(MAX_TILE_EDGE, MAX_TILE_EDGE, MAX_TILE_EDGE * BYTES_PER_PIXEL).is_err(), "over the byte cap");
    assert!(check_dimensions(512, 512, 2048).is_ok());
    let wide = RenderJob { rect: TileRect { x: 0, y: 0, width: MAX_TILE_EDGE + 1, height: 1 }, ..job() };
    assert!(RenderJob::decode(payload(&wide.encode())).is_err());
    let nan = RenderJob { scale: f32::NAN, ..job() };
    assert!(RenderJob::decode(payload(&nan.encode())).is_err());
}

/// Failure: a page's size from a hostile file reaches arithmetic as NaN, zero or a huge number.
#[test]
fn page_sizes_are_checked_before_use() {
    let ok = PageSize { job_id: 1, width: 612.0, height: 792.0 };
    assert_eq!(validate_page_size(1, &ok), Ok(()));
    assert!(validate_page_size(2, &ok).is_err());
    for bad in [f32::NAN, f32::INFINITY, -1.0, 0.0, 1e30] {
        assert!(validate_page_size(1, &PageSize { width: bad, ..ok }).is_err(), "{bad} was accepted");
        assert!(validate_page_size(1, &PageSize { height: bad, ..ok }).is_err(), "{bad} was accepted");
    }
    assert_eq!(device_size(612.0, 792.0, 2.0), Ok((1224, 1584)));
    assert!(device_size(1e6, 1.0, 64.0).is_err());
    assert!(device_size(0.1, 1.0, 1.0).is_err(), "a page under a pixel has nothing to draw");
}

/// Failure: a text reply's count disagrees with its bytes, so the host reads records that aren't there or skips some.
#[test]
fn text_lengths_are_proved_by_the_frame() {
    let reply = TextReply { job_id: 1, page: 0, chars: vec![CharRecord::new('a' as u32, 0); 4], ..TextReply::default() };
    let encoded = reply.encode().unwrap();
    let body = payload(&encoded);
    assert!(TextReply::decode(body).is_ok());
    assert!(TextReply::decode(&body[..body.len() - 1]).is_err(), "short");
    let mut long = body.to_vec();
    long.extend_from_slice(&[0; 4]);
    assert!(TextReply::decode(&long).is_err(), "long");
    let mut huge = body.to_vec();
    huge[9..13].copy_from_slice(&u32::MAX.to_le_bytes());
    assert!(TextReply::decode(&huge).is_err(), "a claimed count allocated before the check");
}

/// Failure: a hostile worker sends a code that isn't a character, or a box that isn't a number, and it reaches `char` or layout.
#[test]
fn text_values_are_checked() {
    assert!(CharRecord::from_wire(0xD800).is_err(), "surrogate");
    assert!(CharRecord::from_wire(0x11_0000).is_err(), "past the last scalar value");
    assert!(CharRecord::from_wire(1 << 30).is_err(), "unknown flag");
    assert_eq!(CharRecord::new(0xD800, 0).ch(), '\u{FFFD}', "the worker side replaces rather than sends it");
    assert_eq!(CharRecord::new(0xFFFE, CharRecord::HYPHEN).ch(), '\u{FFFE}');
    let boxes = TextReply { job_id: 1, page: 0, chars: vec![CharRecord::new(65, 0)], boxes: vec![[f32::NAN, 0.0, 0.0, 0.0]], ..TextReply::default() };
    assert_eq!(TextReply::decode(payload(&boxes.encode().unwrap())), Err(ProtoError("character box isn't finite")));
}

/// Failure: the host accepts text it didn't ask for or that claims a check nobody ran, so the agent gets a false "no hidden text".
#[test]
fn text_replies_are_checked_against_the_job() {
    let job = TextJob { job_id: 1, page: 2, boxes: false, hidden_check: false };
    let good = TextReply { job_id: 1, page: 2, chars: vec![CharRecord::new(65, 0)], ..TextReply::default() };
    assert_eq!(validate_text(&job, &good), Ok(()));
    assert!(validate_text(&job, &TextReply { job_id: 2, ..good.clone() }).is_err());
    assert!(validate_text(&job, &TextReply { page: 3, ..good.clone() }).is_err());
    assert!(validate_text(&job, &TextReply { boxes: vec![[0.0; 4]], ..good.clone() }).is_err());
    assert!(validate_text(&TextJob { boxes: true, ..job }, &good).is_err(), "boxes were asked for and not sent");
    assert!(validate_text(&job, &TextReply { hidden_checked: true, ..good.clone() }).is_err());
    let flagged = TextReply { chars: vec![CharRecord::new(65, CharRecord::NO_INK)], ..good };
    assert!(validate_text(&job, &flagged).is_err());
}

/// Failure: a decoder panics or hangs on hostile bytes. Not a fuzzer, only a cheap floor until the real targets exist.
#[test]
fn decoders_are_total_on_arbitrary_bytes() {
    let mut state = 0x9E37_79B9_7F4A_7C15_u64;
    let mut next = move || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    };
    let mut buf = [0u8; 96];
    for _ in 0..20_000 {
        for byte in &mut buf {
            *byte = next() as u8;
        }
        let len = next() as usize % buf.len();
        let bytes = &buf[..len];
        let _ = RenderJob::decode(bytes);
        let _ = TileReply::decode(bytes);
        let _ = TextReply::decode(bytes);
        let _ = TextJob::decode(bytes);
        let _ = PageSize::decode(bytes);
        let _ = decode_failed(bytes);
        let _ = decode_hello(bytes);
        let _ = decode_opened(bytes);
        let _ = decode_size_request(bytes);
        let header: [u8; HEADER_LEN] = buf[..HEADER_LEN].try_into().unwrap();
        let _ = Header::decode(&header);
    }
    // Every truncation of a real frame is refused, not misread.
    let cut_all = |body: &[u8], decode: &dyn Fn(&[u8]) -> bool| {
        for cut in 0..body.len() {
            assert!(!decode(&body[..cut]), "accepted {cut} of {} bytes", body.len());
        }
    };
    cut_all(payload(&job().encode()), &|b| RenderJob::decode(b).is_ok());
    cut_all(payload(&tile().encode()), &|b| TileReply::decode(b).is_ok());
    cut_all(payload(&PageSize { job_id: 1, width: 1.0, height: 1.0 }.encode()), &|b| PageSize::decode(b).is_ok());
}
