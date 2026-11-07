use super::*;

fn job() -> RenderJob {
    RenderJob {
        generation: 7,
        job_id: 3,
        page: 2,
        scale: 2.0,
        rect: TileRect { x: 512, y: 0, width: 512, height: 512 },
        stride: 2048,
        tile_edge: 0,
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
    let traits = PageTraits { job_id: 3, largest_image_pixels: 1 << 40 };
    assert_eq!(PageTraits::decode(payload(&traits.encode())), Ok(traits));
    assert_eq!(decode_size_request(payload(&encode_traits_request(1, 2))), Ok((1, 2)));
    assert_eq!(decode_size_request(payload(&encode_size_request(1, 2))), Ok((1, 2)));
    let size = PageSize { job_id: 1, width: 612.0, height: 792.0 };
    assert_eq!(PageSize::decode(payload(&size.encode())), Ok(size));
    let text_job = TextJob { job_id: 4, page: 5, boxes: true, hidden_check: true, generation: 9 };
    assert_eq!(TextJob::decode(payload(&text_job.encode())), Ok(text_job));
    let text = TextReply {
        job_id: 4,
        page: 5,
        hidden_checked: true,
        chars: vec![CharRecord::new('a' as u32, 0), CharRecord::new(0xFFFE, CharRecord::HYPHEN), CharRecord::new('😀' as u32, CharRecord::NO_INK)],
        boxes: vec![[1.0, 2.0, 3.0, 4.0]; 3],
        frame: PageFrame { turns: 3, left: 1.0, bottom: 2.0, right: 300.0, top: 400.0 },
        copy_restricted: true,
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
    let frame = TextReply { job_id: 1, page: 0, frame: PageFrame { right: f32::INFINITY, ..PageFrame::default() }, ..TextReply::default() };
    assert_eq!(TextReply::decode(payload(&frame.encode().unwrap())), Err(ProtoError("page frame isn't finite")));
}

/// Failure: the host accepts text it didn't ask for or that claims a check nobody ran, so the agent gets a false "no hidden text".
#[test]
fn text_replies_are_checked_against_the_job() {
    let job = TextJob { job_id: 1, page: 2, boxes: false, hidden_check: false, generation: NEVER_STALE };
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

fn region_job() -> RenderJob {
    RenderJob { rect: TileRect { x: 512, y: 1024, width: 1100, height: 600 }, stride: 4400, tile_edge: 512, ..job() }
}

fn region_reply() -> RegionReply {
    let job = region_job();
    let tile = TileReply { width: job.rect.width, height: job.rect.height, stride: job.stride, status: TileStatus::Done(TileKind::Colour), ..tile() };
    let kinds = vec![TileKind::Grey; 6];
    RegionReply { tile, kinds }
}

/// Failure: the two sides cut a region differently, so tile pixels are handed out from the wrong place, and a seam shows.
#[test]
fn a_region_is_cut_into_grid_tiles_covering_it_exactly() {
    let tiles: Vec<TileRect> = region_tiles(TileRect { x: 512, y: 1024, width: 1100, height: 600 }, 512).collect();
    assert_eq!(tiles.len(), 6);
    assert_eq!(tiles[0], TileRect { x: 512, y: 1024, width: 512, height: 512 });
    assert_eq!(tiles[2], TileRect { x: 1536, y: 1024, width: 76, height: 512 }, "short at the right");
    assert_eq!(tiles[3], TileRect { x: 512, y: 1536, width: 512, height: 88 }, "short at the bottom");
    let area: u64 = tiles.iter().map(|t| u64::from(t.width) * u64::from(t.height)).sum();
    assert_eq!(area, 1100 * 600);
}

/// Failure: a region reply reaches the host with lengths or kinds it can't vouch for, and tiles are classified from nonsense.
#[test]
fn bad_region_replies_are_rejected() {
    let job = region_job();
    let good = region_reply();
    assert_eq!(RegionReply::decode(payload(&good.encode())), Ok(good.clone()));
    assert_eq!(validate_region(&job, &good, job.bytes_needed()), Ok(()));
    let short = RegionReply { kinds: vec![TileKind::Grey; 5], ..good.clone() };
    assert!(validate_region(&job, &short, job.bytes_needed()).is_err(), "too few tiles");
    let wide = RegionReply { tile: TileReply { width: 1101, ..good.tile }, ..good.clone() };
    assert!(validate_region(&job, &wide, job.bytes_needed()).is_err(), "dimensions differ from the job");
    assert!(validate_region(&job, &good, job.bytes_needed() - 1).is_err(), "larger than its surface");
    assert!(validate_region(&self::job(), &good, u64::MAX).is_err(), "not a region job");
    let encoded = good.encode();
    let mut body = payload(&encoded).to_vec();
    body.pop();
    assert!(RegionReply::decode(&body).is_err(), "short");
    let mut body = payload(&encoded).to_vec();
    body[TILE_REPLY_LEN..TILE_REPLY_LEN + 4].copy_from_slice(&u32::MAX.to_le_bytes());
    assert!(RegionReply::decode(&body).is_err(), "a claimed count");
    let mut body = payload(&encoded).to_vec();
    body[TILE_REPLY_LEN + 4] = 9;
    assert!(RegionReply::decode(&body).is_err(), "an unknown kind");
    let mut body = payload(&encoded).to_vec();
    body[TILE_REPLY_LEN + 4] = 1;
    body[TILE_REPLY_LEN + 8] = 0xFF;
    assert!(RegionReply::decode(&body).is_err(), "a blank colour that isn't RGB");
    assert!(Header::decode(&Header { kind: Kind::Region, len: MAX_FRAME }.encode()).is_err(), "a claimed length past what any region needs");
}

/// Failure: a region job off the grid or past the cap reaches PDFium, so a hostile or mistaken request costs far more memory than a viewport.
#[test]
fn bad_region_jobs_are_refused() {
    assert_eq!(region_job().check(), Ok(()));
    assert!(RenderJob { rect: TileRect { x: 100, ..region_job().rect }, ..region_job() }.check().is_err(), "off the grid");
    let huge = RenderJob { rect: TileRect { x: 0, y: 0, width: 4096, height: 4096 }, stride: 16384, ..region_job() };
    assert!(huge.check().is_err(), "past the byte cap");
    let many = RenderJob { rect: TileRect { x: 0, y: 0, width: 1024, height: 1024 }, stride: 4096, tile_edge: 1, ..region_job() };
    assert!(many.check().is_err(), "past the tile cap, which caps the reply");
    assert_eq!(RenderJob::decode(payload(&region_job().encode())), Ok(region_job()));
}

/// Failure: a whole region's kind is reported as colour-free when one tile has colour, or two different blanks read as one.
#[test]
fn kinds_merge_truthfully() {
    let (white, black, red) = (TileKind::Blank { rgb: 0xFF_FFFF }, TileKind::Blank { rgb: 0 }, TileKind::Blank { rgb: 0xFF_0000 });
    assert_eq!(merge_kinds(white, white), white);
    assert_eq!(merge_kinds(white, black), TileKind::Grey);
    assert_eq!(merge_kinds(white, red), TileKind::Colour);
    assert_eq!(merge_kinds(red, TileKind::Grey), TileKind::Colour);
    assert_eq!(merge_kinds(TileKind::Grey, white), TileKind::Grey);
    assert_eq!(merge_kinds(TileKind::Colour, white), TileKind::Colour);
}

/// Failure: a worker claims billions of pages and the host believes it, so anything sized by the page count allocates without limit.
#[test]
fn a_page_count_past_the_cap_is_refused() {
    assert_eq!(decode_opened(payload(&encode_opened(MAX_PAGE_COUNT))), Ok(MAX_PAGE_COUNT));
    assert!(decode_opened(payload(&encode_opened(MAX_PAGE_COUNT + 1))).is_err());
    assert!(decode_opened(payload(&encode_opened(u32::MAX))).is_err());
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
        let _ = RegionReply::decode(bytes);
        let _ = PageTraits::decode(bytes);
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

/// Failure: a worker's claim of how many image boxes there are is believed, so a frame can claim more than its bytes, or past the cap. Or the rectangles don't survive the trip.
#[test]
fn image_boxes_round_trip_and_refuse_a_bad_count() {
    let images = PageImages { job_id: 9, boxes: vec![[0, 1, 65535, 300]] };
    let frame = images.encode();
    assert_eq!(PageImages::decode(&frame[HEADER_LEN..]).unwrap(), images);
    let mut lying = frame[HEADER_LEN..].to_vec();
    lying[4] = 200;
    assert!(PageImages::decode(&lying).is_err(), "past the cap");
    lying[4] = 2;
    assert!(PageImages::decode(&lying).is_err(), "more than the bytes");
    let many = PageImages { job_id: 1, boxes: vec![[0; 4]; MAX_IMAGE_BOXES + 5] };
    assert_eq!(PageImages::decode(&many.encode()[HEADER_LEN..]).unwrap().boxes.len(), MAX_IMAGE_BOXES, "what's sent is capped too");
}

/// Failure: a job far off any page passes its check, and walking its region's tiles adds past the end of a `u32`: a panic with overflow checks on, and rectangles that wrap round to the top of the page without them. Found by the `proto` fuzz target, whose input this is, cut down.
#[test]
fn a_job_off_every_page_is_refused() {
    let far = RenderJob { rect: TileRect { x: 0xFFFF_FE00, y: 0, width: 1024, height: 512 }, stride: 4096, tile_edge: 512, ..job() };
    assert!(far.check().is_err());
    assert!(RenderJob::decode(payload(&far.encode())).is_err());
    let low = RenderJob { rect: TileRect { x: 0, y: 0xFFFF_FE00, width: 512, height: 1024 }, stride: 2048, tile_edge: 512, ..job() };
    assert!(low.check().is_err());
    let single = RenderJob { rect: TileRect { x: MAX_PAGE_EDGE_PX, y: 0, width: 1, height: 1 }, stride: 4, tile_edge: 0, ..job() };
    assert!(single.check().is_err());
    // The last place a tile can really be, on the largest page there is.
    let last = RenderJob { rect: TileRect { x: MAX_PAGE_EDGE_PX - 512, y: MAX_PAGE_EDGE_PX - 512, width: 512, height: 512 }, stride: 2048, tile_edge: 512, ..job() };
    assert!(last.check().is_ok());
    assert_eq!(region_tiles(last.rect, last.tile_edge).count(), 1);
}
