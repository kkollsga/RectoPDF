//! Everything a compromised worker can send to the host. Nothing here trusts the worker, so every decoder must be total, must allocate only what bytes have already paid for, and every validator must refuse what doesn't match the job.

use crate::guard::{Budget, peak_of, within};
use crate::input::Take;
use recto_platform::isolate::Channel;
use recto_proto::*;
use recto_render_service::transport::{FrameReader, ReadError, send_all};
use std::time::Duration;

const BUDGET: Budget = Budget {
    time: Duration::from_secs(10),
    memory: 256 * 1024 * 1024,
};

/// A job the host could really have sent, chosen by the bytes.
fn job_from(take: &mut Take<'_>) -> RenderJob {
    let width = 1 + u32::from(take.u16()) % 2048;
    let height = 1 + u32::from(take.u16()) % 2048;
    let edge = take.pick(&[0, 0, 256, 512]);
    RenderJob {
        generation: u64::from(take.u8()),
        job_id: u32::from(take.u8()),
        page: 0,
        scale: 1.0,
        rect: TileRect { x: 0, y: 0, width, height },
        stride: width * BYTES_PER_PIXEL + u32::from(take.u8()),
        tile_edge: edge,
    }
}

/// Catches: a decoder that panics, reads out of bounds, or allocates what a claimed length says rather than what arrived; a reply that decodes differently once re-encoded; a validator that lets through a reply that doesn't match its job.
pub fn check(data: &[u8]) {
    within("decoding what a worker sent", BUDGET, || {
        let mut take = Take(data);
        match take.u8() % 4 {
            0 => frames_through_a_real_channel(take.rest()),
            1 => {
                let job = job_from(&mut take);
                replies_against_a_job(&job, take.rest());
            }
            2 => every_decoder(take.rest()),
            _ => numbers(&mut take),
        }
    });
}

/// Every decoder on the same bytes. What a decoder took has to mean the same once sent again.
fn every_decoder(payload: &[u8]) {
    if let Ok(hello) = decode_hello(payload) {
        assert_eq!(decode_hello(&encode_hello(hello)[HEADER_LEN..]), Ok(hello));
    }
    if let Ok((pages, _)) = decode_opened(payload) {
        assert!(pages <= MAX_PAGE_COUNT);
        assert_eq!(decode_opened(&encode_opened(pages, 1)[HEADER_LEN..]), Ok((pages, 1)));
    }
    if let Ok(job) = RenderJob::decode(payload) {
        assert!(job.check().is_ok());
        assert_eq!(RenderJob::decode(&job.encode()[HEADER_LEN..]), Ok(job));
        assert!(job.bytes_needed() <= MAX_TILE_BYTES);
        if job.tile_edge != 0 {
            // What `validate_region` and the worker walk. A job that passes its own check can't make them overflow, or walk far.
            let mut tiles = 0;
            for tile in region_tiles(job.rect, job.tile_edge) {
                assert!(
                    tile.width >= 1 && tile.height >= 1 && tile.width <= job.tile_edge && tile.height <= job.tile_edge,
                    "{tile:?}"
                );
                tiles += 1;
            }
            assert!(tiles <= MAX_REGION_TILES, "{tiles} tiles");
        }
    }
    // A page's contents, as an export, a list of images and one image each check it: none of them may panic or run long on hostile records.
    if let Ok((_, records)) = contents::decode_contents(payload) {
        for images in [contents::IMAGES_ALL, contents::IMAGES_LIST, 1] {
            let mut check = contents::StreamCheck::for_images(images);
            let mut reader = contents::Records::new(records);
            while let Ok(Some(record)) = reader.next_record() {
                if check.accept(&record).is_err() {
                    break;
                }
            }
        }
    }
    if let Ok(job) = contents::ContentsJob::decode(payload) {
        assert!(job.check().is_ok());
        assert_eq!(contents::ContentsJob::decode(&job.encode()[HEADER_LEN..]), Ok(job));
    }
    if let Ok(reply) = TileReply::decode(payload) {
        assert_eq!(TileReply::decode(&reply.encode()[HEADER_LEN..]), Ok(reply));
    }
    let (region, peak) = peak_of(|| RegionReply::decode(payload));
    assert!(peak <= payload.len() * 2 + 4096, "a region reply of {} bytes took {peak}", payload.len());
    if let Ok(reply) = region {
        assert!(reply.kinds.len() <= MAX_REGION_TILES as usize);
        assert_eq!(RegionReply::decode(&reply.encode()[HEADER_LEN..]), Ok(reply));
    }
    if let Ok((job_id, page)) = decode_size_request(payload) {
        assert_eq!(decode_size_request(&encode_size_request(job_id, page)[HEADER_LEN..]), Ok((job_id, page)));
    }
    if let Ok(traits) = PageTraits::decode(payload) {
        assert_eq!(PageTraits::decode(&traits.encode()[HEADER_LEN..]), Ok(traits));
    }
    if let Ok(size) = PageSize::decode(payload) {
        let again = PageSize::decode(&size.encode()[HEADER_LEN..]).expect("what was decoded decodes");
        assert_eq!((again.width.to_bits(), again.height.to_bits()), (size.width.to_bits(), size.height.to_bits()));
        if validate_page_size(size.job_id, &size).is_ok() {
            assert!(size.width.is_finite() && size.height.is_finite() && size.width > 0.0 && size.height > 0.0);
            let _ = device_size(size.width, size.height, 1.0);
        }
    }
    if let Ok(job) = TextJob::decode(payload) {
        assert_eq!(TextJob::decode(&job.encode()[HEADER_LEN..]), Ok(job));
    }
    // The bound that matters most: a count of two million in a short frame must cost nothing.
    let (text, peak) = peak_of(|| TextReply::decode(payload));
    assert!(peak <= payload.len() * 2 + 4096, "a text reply of {} bytes took {peak}", payload.len());
    if let Ok(reply) = text {
        assert!(reply.chars.len() <= MAX_PAGE_CHARS as usize);
        assert!(reply.boxes.is_empty() || reply.boxes.len() == reply.chars.len());
        assert!(reply.boxes.iter().flatten().all(|edge| edge.is_finite()), "the decoder lets no NaN through");
        let wire = reply.encode().expect("what was decoded fits");
        assert_eq!(TextReply::decode(&wire[HEADER_LEN..]).as_ref(), Ok(&reply));
        let job = TextJob {
            job_id: reply.job_id,
            page: reply.page,
            boxes: !reply.boxes.is_empty(),
            hidden_check: reply.hidden_checked,
            generation: 0,
        };
        let _ = validate_text(&job, &reply);
        // A reply that says it holds what wasn't asked for is refused.
        assert!(reply.boxes.is_empty() || validate_text(&TextJob { boxes: false, ..job }, &reply).is_err());
        assert!(!reply.hidden_checked || validate_text(&TextJob { hidden_check: false, ..job }, &reply).is_err());
        assert!(!reply.chars.iter().any(|c| c.has(CharRecord::NO_INK)) || validate_text(&TextJob { hidden_check: false, ..job }, &reply).is_err());
    }
    if let Ok((job_id, code)) = decode_failed(payload) {
        assert_eq!(decode_failed(&encode_failed(job_id, code)[HEADER_LEN..]), Ok((job_id, code)));
    }
}

/// A reply checked against the job it claims to answer.
fn replies_against_a_job(job: &RenderJob, payload: &[u8]) {
    let surface = job.bytes_needed();
    if let Ok(reply) = TileReply::decode(payload) {
        if validate_tile(job, &reply, surface).is_ok() {
            assert_eq!((reply.job_id, reply.generation), (job.job_id, job.generation), "a tile for another job");
            if reply.status != TileStatus::Cancelled {
                assert_eq!((reply.width, reply.height, reply.stride), (job.rect.width, job.rect.height, job.stride));
                assert!(u64::from(reply.stride) * u64::from(reply.height) <= surface, "a tile past its surface");
            }
        }
        assert!(
            validate_tile(job, &reply, 0).is_err() || reply.status == TileStatus::Cancelled,
            "no surface can hold a tile"
        );
    }
    if let Ok(reply) = RegionReply::decode(payload)
        && validate_region(job, &reply, surface).is_ok()
    {
        assert!(job.tile_edge != 0);
        if reply.tile.status != TileStatus::Cancelled {
            assert_eq!(reply.kinds.len(), region_tiles(job.rect, job.tile_edge).count());
        }
    }
    // A reply that's right in every way, with one field chosen by the bytes.
    let mut take = Take(payload);
    let mut reply = TileReply {
        generation: job.generation,
        job_id: job.job_id,
        width: job.rect.width,
        height: job.rect.height,
        stride: job.stride,
        status: TileStatus::Done(TileKind::Colour),
        render_us: 0,
    };
    assert!(validate_tile(job, &reply, surface).is_ok(), "a true reply must pass");
    let hostile = take.pick(&[0u32, 1, 4096, 32_767, 32_768, 65_536, 0x7FFF_FFFF, u32::MAX]);
    match take.u8() % 3 {
        0 => reply.width = hostile,
        1 => reply.height = hostile,
        _ => reply.stride = hostile,
    }
    let changed = reply.width != job.rect.width || reply.height != job.rect.height || reply.stride != job.stride;
    assert!(
        !changed || validate_tile(job, &reply, surface).is_err(),
        "a reply differing from its job passed: {reply:?} for {job:?}"
    );
}

/// Sizes and scales from the worker, and a page's size at any scale.
fn numbers(take: &mut Take<'_>) {
    let (width, height, scale) = (take.f32(), take.f32(), take.f32());
    if let Ok((w, h)) = device_size(width, height, scale) {
        assert!((1..=MAX_PAGE_EDGE_PX).contains(&w) && (1..=MAX_PAGE_EDGE_PX).contains(&h));
    }
    let bits = take.u32();
    if let Ok(record) = CharRecord::from_wire(bits) {
        assert_eq!(record.bits(), bits);
        let _ = record.ch();
    }
    let packed = CharRecord::new(bits, take.u32());
    assert!(CharRecord::from_wire(packed.bits()).is_ok(), "what `new` packs, `from_wire` must take");
    let kind = |take: &mut Take<'_>| match take.u8() % 3 {
        0 => TileKind::Grey,
        1 => TileKind::Colour,
        _ => TileKind::Blank { rgb: take.u32() & 0xFF_FFFF },
    };
    let (a, b) = (kind(take), kind(take));
    assert_eq!(merge_kinds(a, b), merge_kinds(b, a), "order can't change what two pieces hold");
}

/// Bytes down a real channel, cut into pieces, as a worker could send them. Catches: a claimed length that costs memory before bytes arrive, or a frame past its kind's cap.
fn frames_through_a_real_channel(bytes: &[u8]) {
    let Ok((sender, receiver)) = Channel::pair() else { return };
    let (_, peak) = peak_of(|| {
        std::thread::scope(|scope| {
            scope.spawn(move || {
                let _ = send_all(&sender, bytes, &[]);
                drop(sender);
            });
            // Moved in, so it closes when this stops reading, and a writer blocked on a full socket gets its error instead of waiting for ever.
            let receiver = receiver;
            let mut reader = FrameReader::default();
            loop {
                match reader.read_blocking(&receiver) {
                    Ok(frame) => {
                        assert!(frame.payload.len() as u32 <= frame.kind.max_payload());
                        assert!(frame.handles.is_empty(), "nothing here sends a handle");
                        every_decoder(&frame.payload);
                    }
                    Err(ReadError::Closed | ReadError::Bad(_) | ReadError::Io(_)) => return,
                }
            }
        });
    });
    // What arrived, twice over for the reader and the decoders, and one read's step: never what a header claimed.
    assert!(peak <= bytes.len() * 2 + 1024 * 1024, "{} bytes through a channel took {peak}", bytes.len());
}
