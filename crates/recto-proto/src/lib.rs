//! The wire between the host and a renderer process. No dependencies, no `unsafe`, no I/O: bytes in, values or errors out.
//!
//! **The renderer is hostile.** It parses hostile files, so a compromised one can send anything. Every decoder here is total on
//! arbitrary bytes, every length is checked against a cap before anything is allocated, and [`validate_tile`],
//! [`validate_page_size`] and [`validate_text`] check what a decoded reply claims against what the host asked for. A violation
//! kills the worker.
//!
//! Text carries no offsets on purpose. It travels as fixed records, one per character, so a position is an index whose range is
//! proved by arithmetic on the frame's length: there's no offset for a hostile worker to get wrong.
//!
//! All integers are little-endian. A frame is a 12-byte header and a payload. A version change is a hard stop: the host refuses a
//! worker whose [`VERSION`] differs, which can't happen in one signed app unless something is wrong.

pub mod contents;

/// Bumped on any change to a layout below.
pub const VERSION: u16 = 2;
pub const MAGIC: [u8; 2] = *b"RP";
pub const HEADER_LEN: usize = 12;

/// The largest payload of any frame. Fits [`MAX_PAGE_CHARS`] with boxes, plus the text header.
pub const MAX_FRAME: u32 = 64 * 1024 * 1024;
/// The size of an ordinary tile, in device pixels. See the platform report, §2.5.
pub const TILE_EDGE: u32 = 512;
/// The largest tile edge. Whole-page strips use this; ordinary tiles are [`TILE_EDGE`].
pub const MAX_TILE_EDGE: u32 = 32_767;
/// The largest buffer a tile may claim.
pub const MAX_TILE_BYTES: u64 = 256 * 1024 * 1024;
/// The largest page edge at any scale, in device pixels. PDFium's rendering arguments are `int`s.
pub const MAX_PAGE_EDGE_PX: u32 = 1 << 24;
/// The largest page edge in points. The PDF limit is 14,400 units; `UserUnit` can raise it, and nothing above this is a real page.
pub const MAX_PAGE_POINTS: f32 = 1_000_000.0;
pub const MIN_SCALE: f32 = 1.0 / 64.0;
pub const MAX_SCALE: f32 = 64.0;
/// The most characters one page may return. The busiest page measured has under 20,000.
pub const MAX_PAGE_CHARS: u32 = 2_000_000;
/// The most one region may hold. A 3024 x 1964 viewport is 24 MiB, so this covers it with about a third over for margin, and a larger view takes more passes. Region size
/// drives a heavy page's peak: on the USGS map at 4x, 32 MiB peaks at 465 MiB and 64 MiB at 613, past the 512 MiB limit.
pub const MAX_REGION_BYTES: u64 = 32 * 1024 * 1024;
/// The most tiles one region may hold, which caps its reply.
pub const MAX_REGION_TILES: u32 = 1024;
/// The most pages a document may claim. PDF's own implementation limit on indirect objects, which a page needs one of.
pub const MAX_PAGE_COUNT: u32 = 8_388_607;
/// The longest password accepted on the wire.
pub const MAX_PASSWORD: usize = 1024;
/// Bytes per pixel of [`PixelFormat::Bgrx8`].
pub const BYTES_PER_PIXEL: u32 = 4;

/// Why bytes or a reply were refused. Static text, so refusing costs nothing and can't echo hostile data.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ProtoError(pub &'static str);

impl std::fmt::Display for ProtoError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.0)
    }
}

impl std::error::Error for ProtoError {}

/// What a frame holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u16)]
pub enum Kind {
    /// Worker to host, once, when it's ready.
    Hello = 1,
    /// Host to worker: open the document on the passed descriptor.
    Open = 2,
    Opened = 3,
    Render = 4,
    Tile = 5,
    SizeRequest = 6,
    Size = 7,
    TextRequest = 8,
    Text = 9,
    Failed = 10,
    /// The answer to a region: a tile reply, plus what each tile in it holds.
    Region = 11,
    TraitsRequest = 12,
    Traits = 13,
    /// Asks for a page's contents, or the next piece of them. See [`contents`].
    ContentsRequest = 32,
    Contents = 33,
}

impl Kind {
    fn from_u16(value: u16) -> Result<Self, ProtoError> {
        Ok(match value {
            1 => Self::Hello,
            2 => Self::Open,
            3 => Self::Opened,
            4 => Self::Render,
            5 => Self::Tile,
            6 => Self::SizeRequest,
            7 => Self::Size,
            8 => Self::TextRequest,
            9 => Self::Text,
            10 => Self::Failed,
            11 => Self::Region,
            12 => Self::TraitsRequest,
            13 => Self::Traits,
            32 => Self::ContentsRequest,
            33 => Self::Contents,
            _ => return Err(ProtoError("unknown frame kind")),
        })
    }

    /// The most this kind of frame may carry. Checked before the payload is read, so a claimed length allocates nothing.
    pub fn max_payload(self) -> u32 {
        match self {
            Self::Hello => HELLO_LEN as u32,
            Self::Open => MAX_PASSWORD as u32,
            Self::Opened => OPENED_LEN as u32,
            Self::Render => RENDER_JOB_LEN as u32,
            Self::Tile => TILE_REPLY_LEN as u32,
            Self::SizeRequest => SIZE_REQUEST_LEN as u32,
            Self::Size => PAGE_SIZE_LEN as u32,
            Self::TextRequest => TEXT_JOB_LEN as u32,
            Self::Text => MAX_FRAME,
            Self::Failed => FAILED_LEN as u32,
            Self::Region => (TILE_REPLY_LEN + 4 + MAX_REGION_TILES as usize * REGION_KIND_LEN) as u32,
            Self::TraitsRequest => SIZE_REQUEST_LEN as u32,
            Self::Traits => PAGE_TRAITS_LEN as u32,
            Self::ContentsRequest => contents::CONTENTS_JOB_LEN as u32,
            Self::Contents => contents::MAX_CONTENTS_FRAME,
        }
    }
}

/// A frame's header.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Header {
    pub kind: Kind,
    pub len: u32,
}

impl Header {
    pub fn encode(self) -> [u8; HEADER_LEN] {
        let mut out = [0; HEADER_LEN];
        out[..2].copy_from_slice(&MAGIC);
        out[2..4].copy_from_slice(&VERSION.to_le_bytes());
        out[4..6].copy_from_slice(&(self.kind as u16).to_le_bytes());
        out[8..12].copy_from_slice(&self.len.to_le_bytes());
        out
    }

    pub fn decode(bytes: &[u8; HEADER_LEN]) -> Result<Self, ProtoError> {
        if bytes[..2] != MAGIC {
            return Err(ProtoError("bad frame magic"));
        }
        if u16::from_le_bytes([bytes[2], bytes[3]]) != VERSION {
            return Err(ProtoError("protocol version differs"));
        }
        let kind = Kind::from_u16(u16::from_le_bytes([bytes[4], bytes[5]]))?;
        if bytes[6..8] != [0, 0] {
            return Err(ProtoError("reserved header bytes set"));
        }
        let len = u32::from_le_bytes([bytes[8], bytes[9], bytes[10], bytes[11]]);
        if len > kind.max_payload() {
            return Err(ProtoError("frame longer than its kind allows"));
        }
        Ok(Self { kind, len })
    }
}

/// Reads fixed fields in order, so a short payload is an error rather than a panic.
struct Reader<'a>(&'a [u8]);

impl<'a> Reader<'a> {
    fn take<const N: usize>(&mut self) -> Result<[u8; N], ProtoError> {
        let (head, rest) = self.0.split_first_chunk::<N>().ok_or(ProtoError("payload too short"))?;
        self.0 = rest;
        Ok(*head)
    }
    fn u8(&mut self) -> Result<u8, ProtoError> {
        Ok(self.take::<1>()?[0])
    }
    fn u32(&mut self) -> Result<u32, ProtoError> {
        Ok(u32::from_le_bytes(self.take()?))
    }
    fn u64(&mut self) -> Result<u64, ProtoError> {
        Ok(u64::from_le_bytes(self.take()?))
    }
    fn f32(&mut self) -> Result<f32, ProtoError> {
        Ok(f32::from_le_bytes(self.take()?))
    }
    fn end(self) -> Result<(), ProtoError> {
        if self.0.is_empty() { Ok(()) } else { Err(ProtoError("payload too long")) }
    }
}

fn frame(kind: Kind, payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(HEADER_LEN + payload.len());
    out.extend_from_slice(&Header { kind, len: payload.len() as u32 }.encode());
    out.extend_from_slice(payload);
    out
}

/// The worker's first frame, sent once PDFium is loaded.
pub const HELLO_LEN: usize = 4;

pub fn encode_hello(pid: u32) -> Vec<u8> {
    frame(Kind::Hello, &pid.to_le_bytes())
}

pub fn decode_hello(payload: &[u8]) -> Result<u32, ProtoError> {
    let mut r = Reader(payload);
    let pid = r.u32()?;
    r.end()?;
    Ok(pid)
}

pub fn encode_open(password: &[u8]) -> Result<Vec<u8>, ProtoError> {
    if password.len() > MAX_PASSWORD {
        return Err(ProtoError("password too long"));
    }
    Ok(frame(Kind::Open, password))
}

/// The answer to [`Kind::Open`].
pub const OPENED_LEN: usize = 4;

pub fn encode_opened(page_count: u32) -> Vec<u8> {
    frame(Kind::Opened, &page_count.to_le_bytes())
}

pub fn decode_opened(payload: &[u8]) -> Result<u32, ProtoError> {
    let mut r = Reader(payload);
    let pages = r.u32()?;
    r.end()?;
    if pages > MAX_PAGE_COUNT {
        return Err(ProtoError("more pages than a document can have"));
    }
    Ok(pages)
}

/// The only pixel format for now: blue, green, red, one unused byte. The host treats the tile as opaque.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum PixelFormat {
    Bgrx8 = 1,
}

/// Which tile, for a cache. Not on the wire yet, but it's the key the host's tile cache will hold per tile.
// size: asserted <= 16 by `per_item_sizes_stay_small`. Platform report, §2.9.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct TileKey {
    pub tab: u32,
    pub page: u32,
    pub scale_tier: u16,
    pub x: u16,
    pub y: u16,
}

/// A rectangle of a page in device pixels at the job's scale. The origin is the page's top left.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TileRect {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

/// Draw one tile into the surface passed with the frame.
// size: asserted <= 64 by `per_item_sizes_stay_small`. A queued job, and the queue is capped. Platform report, §2.9.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RenderJob {
    pub generation: u64,
    pub job_id: u32,
    pub page: u32,
    pub scale: f32,
    pub rect: TileRect,
    pub stride: u32,
    /// Zero for one tile. Otherwise `rect` is a region, cut into a grid of tiles this size from the page's origin, each reported separately.
    pub tile_edge: u32,
}

pub const RENDER_JOB_LEN: usize = 8 + 4 + 4 + 4 + 16 + 4 + 4 + 1;

impl RenderJob {
    /// The bytes this job needs in its surface.
    pub fn bytes_needed(&self) -> u64 {
        u64::from(self.stride) * u64::from(self.rect.height)
    }

    /// Refuses a job that can't be drawn. The host checks before sending, and the worker checks again before using it.
    pub fn check(&self) -> Result<(), ProtoError> {
        if !(MIN_SCALE..=MAX_SCALE).contains(&self.scale) {
            return Err(ProtoError("scale out of range"));
        }
        check_dimensions(self.rect.width, self.rect.height, self.stride)?;
        if self.tile_edge != 0 {
            check_region(self.rect, self.tile_edge)?;
            if self.bytes_needed() > MAX_REGION_BYTES {
                return Err(ProtoError("region larger than the cap"));
            }
        }
        Ok(())
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut p = Vec::with_capacity(RENDER_JOB_LEN);
        p.extend_from_slice(&self.generation.to_le_bytes());
        p.extend_from_slice(&self.job_id.to_le_bytes());
        p.extend_from_slice(&self.page.to_le_bytes());
        p.extend_from_slice(&self.scale.to_le_bytes());
        for value in [self.rect.x, self.rect.y, self.rect.width, self.rect.height, self.stride, self.tile_edge] {
            p.extend_from_slice(&value.to_le_bytes());
        }
        p.push(PixelFormat::Bgrx8 as u8);
        frame(Kind::Render, &p)
    }

    pub fn decode(payload: &[u8]) -> Result<Self, ProtoError> {
        let mut r = Reader(payload);
        let job = Self {
            generation: r.u64()?,
            job_id: r.u32()?,
            page: r.u32()?,
            scale: r.f32()?,
            rect: TileRect { x: r.u32()?, y: r.u32()?, width: r.u32()?, height: r.u32()? },
            stride: r.u32()?,
            tile_edge: r.u32()?,
        };
        if r.u8()? != PixelFormat::Bgrx8 as u8 {
            return Err(ProtoError("unknown pixel format"));
        }
        r.end()?;
        job.check()?;
        Ok(job)
    }
}

fn check_dimensions(width: u32, height: u32, stride: u32) -> Result<(), ProtoError> {
    if width == 0 || height == 0 || width > MAX_TILE_EDGE || height > MAX_TILE_EDGE {
        return Err(ProtoError("tile dimensions out of range"));
    }
    if u64::from(stride) < u64::from(width) * u64::from(BYTES_PER_PIXEL) {
        return Err(ProtoError("stride shorter than a row"));
    }
    if u64::from(stride) * u64::from(height) > MAX_TILE_BYTES {
        return Err(ProtoError("tile larger than the cap"));
    }
    Ok(())
}

fn check_region(rect: TileRect, edge: u32) -> Result<(), ProtoError> {
    if edge > MAX_TILE_EDGE {
        return Err(ProtoError("tile edge out of range"));
    }
    if !rect.x.is_multiple_of(edge) || !rect.y.is_multiple_of(edge) {
        return Err(ProtoError("region not on the tile grid"));
    }
    if u64::from(rect.width.div_ceil(edge)) * u64::from(rect.height.div_ceil(edge)) > u64::from(MAX_REGION_TILES) {
        return Err(ProtoError("region has too many tiles"));
    }
    Ok(())
}

/// The tiles of a region, left to right and top to bottom, each as a rectangle of the page. The last in a row or column may be short. Both sides use this one rule.
pub fn region_tiles(rect: TileRect, edge: u32) -> impl Iterator<Item = TileRect> {
    let edge = edge.max(1);
    let rows = rect.height.div_ceil(edge);
    let columns = rect.width.div_ceil(edge);
    (0..rows).flat_map(move |row| {
        (0..columns).map(move |column| {
            let (dx, dy) = (column * edge, row * edge);
            TileRect { x: rect.x + dx, y: rect.y + dy, width: edge.min(rect.width - dx), height: edge.min(rect.height - dy) }
        })
    })
}

/// What a finished tile holds. A blank tile needs no surface and a grey one a quarter of it: platform report, §2.5.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TileKind {
    /// Every pixel is this colour, read as `0x00RRGGBB`.
    Blank { rgb: u32 },
    /// Red, green and blue are equal in every pixel.
    Grey,
    Colour,
}

/// What two pieces hold together. Two different blanks aren't one blank, and a coloured blank beside a grey piece is colour.
pub fn merge_kinds(a: TileKind, b: TileKind) -> TileKind {
    let has_colour = |kind: TileKind| match kind {
        TileKind::Colour => true,
        TileKind::Grey => false,
        TileKind::Blank { rgb } => (rgb >> 16) & 0xFF != (rgb >> 8) & 0xFF || (rgb >> 8) & 0xFF != rgb & 0xFF,
    };
    if a == b {
        a
    } else if has_colour(a) || has_colour(b) {
        TileKind::Colour
    } else {
        TileKind::Grey
    }
}

/// How a job ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TileStatus {
    Done(TileKind),
    /// Its generation went stale. The surface holds nothing useful.
    Cancelled,
}

/// The answer to a [`RenderJob`].
// size: asserted <= 64 by `per_item_sizes_stay_small`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TileReply {
    pub generation: u64,
    pub job_id: u32,
    pub width: u32,
    pub height: u32,
    pub stride: u32,
    pub status: TileStatus,
    /// Time inside the worker. From a hostile process, so for reports only.
    pub render_us: u32,
}

pub const TILE_REPLY_LEN: usize = 8 + 4 + 4 + 4 + 4 + 1 + 4 + 4;

impl TileReply {
    pub fn encode(&self) -> Vec<u8> {
        let mut p = Vec::with_capacity(TILE_REPLY_LEN);
        p.extend_from_slice(&self.generation.to_le_bytes());
        for value in [self.job_id, self.width, self.height, self.stride] {
            p.extend_from_slice(&value.to_le_bytes());
        }
        let (status, rgb) = match self.status {
            TileStatus::Done(TileKind::Blank { rgb }) => (1, rgb),
            TileStatus::Done(TileKind::Grey) => (2, 0),
            TileStatus::Done(TileKind::Colour) => (3, 0),
            TileStatus::Cancelled => (4, 0),
        };
        p.push(status);
        p.extend_from_slice(&rgb.to_le_bytes());
        p.extend_from_slice(&self.render_us.to_le_bytes());
        frame(Kind::Tile, &p)
    }

    pub fn decode(payload: &[u8]) -> Result<Self, ProtoError> {
        let mut r = Reader(payload);
        let (generation, job_id, width, height, stride) = (r.u64()?, r.u32()?, r.u32()?, r.u32()?, r.u32()?);
        let (status, rgb) = (r.u8()?, r.u32()?);
        let status = match (status, rgb) {
            (1, rgb) if rgb <= 0x00FF_FFFF => TileStatus::Done(TileKind::Blank { rgb }),
            (2, 0) => TileStatus::Done(TileKind::Grey),
            (3, 0) => TileStatus::Done(TileKind::Colour),
            (4, 0) => TileStatus::Cancelled,
            _ => return Err(ProtoError("unknown tile status")),
        };
        let render_us = r.u32()?;
        r.end()?;
        Ok(Self { generation, job_id, width, height, stride, status, render_us })
    }
}

/// Checks a tile reply against the job that was sent and the surface that was passed, before anyone reads a pixel.
pub fn validate_tile(job: &RenderJob, reply: &TileReply, surface_len: u64) -> Result<(), ProtoError> {
    if reply.job_id != job.job_id {
        return Err(ProtoError("tile answers a different job"));
    }
    if reply.generation != job.generation {
        return Err(ProtoError("tile has a different generation"));
    }
    if reply.status == TileStatus::Cancelled {
        return Ok(());
    }
    if reply.width != job.rect.width || reply.height != job.rect.height || reply.stride != job.stride {
        return Err(ProtoError("tile dimensions differ from the job"));
    }
    check_dimensions(reply.width, reply.height, reply.stride)?;
    if u64::from(reply.stride) * u64::from(reply.height) > surface_len {
        return Err(ProtoError("tile larger than its surface"));
    }
    Ok(())
}

/// One tile's entry in a region reply: a code, and a blank tile's colour.
const REGION_KIND_LEN: usize = 5;

/// The answer to a region job: what any tile reply says about the whole, and what each tile holds, in [`region_tiles`] order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RegionReply {
    pub tile: TileReply,
    pub kinds: Vec<TileKind>,
}

impl RegionReply {
    pub fn encode(&self) -> Vec<u8> {
        let tile = self.tile.encode();
        let mut p = Vec::with_capacity(TILE_REPLY_LEN + 4 + self.kinds.len() * REGION_KIND_LEN);
        p.extend_from_slice(&tile[HEADER_LEN..]);
        p.extend_from_slice(&(self.kinds.len() as u32).to_le_bytes());
        for kind in &self.kinds {
            let (code, rgb) = match *kind {
                TileKind::Blank { rgb } => (1, rgb),
                TileKind::Grey => (2, 0),
                TileKind::Colour => (3, 0),
            };
            p.push(code);
            p.extend_from_slice(&rgb.to_le_bytes());
        }
        frame(Kind::Region, &p)
    }

    /// Checks lengths before allocating, and every entry as it goes.
    pub fn decode(payload: &[u8]) -> Result<Self, ProtoError> {
        let (head, rest) = payload.split_at_checked(TILE_REPLY_LEN).ok_or(ProtoError("payload too short"))?;
        let tile = TileReply::decode(head)?;
        let mut r = Reader(rest);
        let count = r.u32()?;
        if count > MAX_REGION_TILES {
            return Err(ProtoError("region has too many tiles"));
        }
        if r.0.len() != count as usize * REGION_KIND_LEN {
            return Err(ProtoError("region length differs from its count"));
        }
        let mut kinds = Vec::with_capacity(count as usize);
        for _ in 0..count {
            kinds.push(match (r.u8()?, r.u32()?) {
                (1, rgb) if rgb <= 0x00FF_FFFF => TileKind::Blank { rgb },
                (2, 0) => TileKind::Grey,
                (3, 0) => TileKind::Colour,
                _ => return Err(ProtoError("unknown tile kind")),
            });
        }
        Ok(Self { tile, kinds })
    }
}

/// Checks a region reply against its job and surface, before anyone reads a pixel: everything a tile reply must satisfy, and one kind per tile in the grid.
pub fn validate_region(job: &RenderJob, reply: &RegionReply, surface_len: u64) -> Result<(), ProtoError> {
    validate_tile(job, &reply.tile, surface_len)?;
    if job.tile_edge == 0 {
        return Err(ProtoError("a region reply to a single tile"));
    }
    let expected = if reply.tile.status == TileStatus::Cancelled { 0 } else { region_tiles(job.rect, job.tile_edge).count() };
    if reply.kinds.len() != expected {
        return Err(ProtoError("region has the wrong number of tiles"));
    }
    Ok(())
}

/// Asks for one page's size.
pub const SIZE_REQUEST_LEN: usize = 8;

pub fn encode_size_request(job_id: u32, page: u32) -> Vec<u8> {
    let mut p = [0; SIZE_REQUEST_LEN];
    p[..4].copy_from_slice(&job_id.to_le_bytes());
    p[4..].copy_from_slice(&page.to_le_bytes());
    frame(Kind::SizeRequest, &p)
}

pub fn decode_size_request(payload: &[u8]) -> Result<(u32, u32), ProtoError> {
    let mut r = Reader(payload);
    let request = (r.u32()?, r.u32()?);
    r.end()?;
    Ok(request)
}

pub fn encode_traits_request(job_id: u32, page: u32) -> Vec<u8> {
    let mut p = [0; SIZE_REQUEST_LEN];
    p[..4].copy_from_slice(&job_id.to_le_bytes());
    p[4..].copy_from_slice(&page.to_le_bytes());
    frame(Kind::TraitsRequest, &p)
}

/// What decides how a page is best drawn. From a hostile file, and it only ever chooses between two ways of drawing that are both capped, so it's checked for form alone.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PageTraits {
    pub job_id: u32,
    /// The largest image on the page, in source pixels, as far as the look went.
    pub largest_image_pixels: u64,
}

pub const PAGE_TRAITS_LEN: usize = 12;

impl PageTraits {
    pub fn encode(&self) -> Vec<u8> {
        let mut p = [0; PAGE_TRAITS_LEN];
        p[..4].copy_from_slice(&self.job_id.to_le_bytes());
        p[4..].copy_from_slice(&self.largest_image_pixels.to_le_bytes());
        frame(Kind::Traits, &p)
    }

    pub fn decode(payload: &[u8]) -> Result<Self, ProtoError> {
        let mut r = Reader(payload);
        let traits = Self { job_id: r.u32()?, largest_image_pixels: r.u64()? };
        r.end()?;
        Ok(traits)
    }
}

/// A page's size in points, after its rotation. What the file says, so it's checked by [`validate_page_size`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PageSize {
    pub job_id: u32,
    pub width: f32,
    pub height: f32,
}

pub const PAGE_SIZE_LEN: usize = 12;

impl PageSize {
    pub fn encode(&self) -> Vec<u8> {
        let mut p = [0; PAGE_SIZE_LEN];
        p[..4].copy_from_slice(&self.job_id.to_le_bytes());
        p[4..8].copy_from_slice(&self.width.to_le_bytes());
        p[8..].copy_from_slice(&self.height.to_le_bytes());
        frame(Kind::Size, &p)
    }

    pub fn decode(payload: &[u8]) -> Result<Self, ProtoError> {
        let mut r = Reader(payload);
        let size = Self { job_id: r.u32()?, width: r.f32()?, height: r.f32()? };
        r.end()?;
        Ok(size)
    }
}

pub fn validate_page_size(job_id: u32, size: &PageSize) -> Result<(), ProtoError> {
    if size.job_id != job_id {
        return Err(ProtoError("size answers a different job"));
    }
    for edge in [size.width, size.height] {
        if !edge.is_finite() || edge <= 0.0 || edge > MAX_PAGE_POINTS {
            return Err(ProtoError("page size out of range"));
        }
    }
    Ok(())
}

/// A page's whole size in device pixels at a scale. Both sides use this one rule, so tiles line up.
pub fn device_size(width: f32, height: f32, scale: f32) -> Result<(u32, u32), ProtoError> {
    let edge = |points: f32| -> Result<u32, ProtoError> {
        let px = (f64::from(points) * f64::from(scale)).round();
        if !px.is_finite() || px < 1.0 || px > f64::from(MAX_PAGE_EDGE_PX) {
            return Err(ProtoError("page too large at this scale"));
        }
        Ok(px as u32)
    };
    Ok((edge(width)?, edge(height)?))
}

/// Ask for one page's characters.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TextJob {
    pub job_id: u32,
    pub page: u32,
    /// Send each character's box.
    pub boxes: bool,
    /// Draw the page and flag characters with no ink under them. Costs a render.
    pub hidden_check: bool,
}

pub const TEXT_JOB_LEN: usize = 9;

impl TextJob {
    pub fn encode(&self) -> Vec<u8> {
        let mut p = [0; TEXT_JOB_LEN];
        p[..4].copy_from_slice(&self.job_id.to_le_bytes());
        p[4..8].copy_from_slice(&self.page.to_le_bytes());
        p[8] = u8::from(self.boxes) | u8::from(self.hidden_check) << 1;
        frame(Kind::TextRequest, &p)
    }

    pub fn decode(payload: &[u8]) -> Result<Self, ProtoError> {
        let mut r = Reader(payload);
        let (job_id, page, flags) = (r.u32()?, r.u32()?, r.u8()?);
        r.end()?;
        if flags & !0b11 != 0 {
            return Err(ProtoError("unknown text flags"));
        }
        Ok(Self { job_id, page, boxes: flags & 1 != 0, hidden_check: flags & 2 != 0 })
    }
}

/// One character, packed in four bytes: a scalar value in the low 21 bits and flags above it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CharRecord(u32);

impl CharRecord {
    /// PDFium's marker for a line-end hyphen: U+FFFE or its hyphen flag.
    pub const HYPHEN: u32 = 1 << 21;
    /// PDFium made this character up, such as a line break. It isn't in the file.
    pub const GENERATED: u32 = 1 << 22;
    /// The hidden-text check found no ink under it.
    pub const NO_INK: u32 = 1 << 23;
    const CODE_MASK: u32 = (1 << 21) - 1;
    const KNOWN: u32 = Self::CODE_MASK | Self::HYPHEN | Self::GENERATED | Self::NO_INK;

    /// Packs a character. Anything that isn't a scalar value becomes U+FFFD.
    pub fn new(code: u32, flags: u32) -> Self {
        let code = if char::from_u32(code).is_some() { code } else { 0xFFFD };
        Self(code | (flags & !Self::CODE_MASK & Self::KNOWN))
    }

    /// Unpacks wire bits, refusing what [`Self::new`] can't produce.
    pub fn from_wire(bits: u32) -> Result<Self, ProtoError> {
        if bits & !Self::KNOWN != 0 {
            return Err(ProtoError("unknown character flags"));
        }
        if char::from_u32(bits & Self::CODE_MASK).is_none() {
            return Err(ProtoError("character isn't a scalar value"));
        }
        Ok(Self(bits))
    }

    pub fn bits(self) -> u32 {
        self.0
    }

    pub fn ch(self) -> char {
        char::from_u32(self.0 & Self::CODE_MASK).unwrap_or('\u{FFFD}')
    }

    pub fn has(self, flag: u32) -> bool {
        self.0 & flag != 0
    }
}

/// A character's box in page points: left, bottom, right, top.
pub type CharBox = [f32; 4];

/// The fixed part of a text reply: job, page, flags, count.
const TEXT_HEADER_LEN: usize = 4 + 4 + 1 + 4;

/// A page's characters, one record each, in PDFium's reading order.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TextReply {
    pub job_id: u32,
    pub page: u32,
    /// The hidden-text check ran, so `NO_INK` means something.
    pub hidden_checked: bool,
    pub chars: Vec<CharRecord>,
    /// Empty, or one per character.
    pub boxes: Vec<CharBox>,
}

impl TextReply {
    pub fn encode(&self) -> Result<Vec<u8>, ProtoError> {
        let count = u32::try_from(self.chars.len()).map_err(|_| ProtoError("too many characters"))?;
        let len = text_payload_len(count, !self.boxes.is_empty())?;
        let mut out = Vec::with_capacity(HEADER_LEN + len as usize);
        out.extend_from_slice(&Header { kind: Kind::Text, len }.encode());
        out.extend_from_slice(&self.job_id.to_le_bytes());
        out.extend_from_slice(&self.page.to_le_bytes());
        out.push(u8::from(!self.boxes.is_empty()) | u8::from(self.hidden_checked) << 1);
        out.extend_from_slice(&count.to_le_bytes());
        for record in &self.chars {
            out.extend_from_slice(&record.bits().to_le_bytes());
        }
        for char_box in &self.boxes {
            for edge in char_box {
                out.extend_from_slice(&edge.to_le_bytes());
            }
        }
        Ok(out)
    }

    /// Decodes and checks lengths and values. Nothing is allocated until the frame's length has proved the count.
    pub fn decode(payload: &[u8]) -> Result<Self, ProtoError> {
        let mut r = Reader(payload);
        let (job_id, page, flags, count) = (r.u32()?, r.u32()?, r.u8()?, r.u32()?);
        if flags & !0b11 != 0 {
            return Err(ProtoError("unknown text flags"));
        }
        let has_boxes = flags & 1 != 0;
        if u64::from(text_payload_len(count, has_boxes)?) != payload.len() as u64 {
            return Err(ProtoError("text length differs from its count"));
        }
        let mut chars = Vec::with_capacity(count as usize);
        for _ in 0..count {
            chars.push(CharRecord::from_wire(r.u32()?)?);
        }
        let mut boxes = Vec::with_capacity(if has_boxes { count as usize } else { 0 });
        if has_boxes {
            for _ in 0..count {
                let char_box = [r.f32()?, r.f32()?, r.f32()?, r.f32()?];
                if char_box.iter().any(|edge| !edge.is_finite()) {
                    return Err(ProtoError("character box isn't finite"));
                }
                boxes.push(char_box);
            }
        }
        r.end()?;
        Ok(Self { job_id, page, hidden_checked: flags & 2 != 0, chars, boxes })
    }
}

fn text_payload_len(count: u32, boxes: bool) -> Result<u32, ProtoError> {
    if count > MAX_PAGE_CHARS {
        return Err(ProtoError("too many characters"));
    }
    let per_char: u64 = if boxes { 4 + 16 } else { 4 };
    let len = TEXT_HEADER_LEN as u64 + u64::from(count) * per_char;
    if len > u64::from(MAX_FRAME) {
        return Err(ProtoError("text frame too long"));
    }
    Ok(len as u32)
}

/// Checks a decoded text reply against the job.
pub fn validate_text(job: &TextJob, reply: &TextReply) -> Result<(), ProtoError> {
    if reply.job_id != job.job_id || reply.page != job.page {
        return Err(ProtoError("text answers a different job"));
    }
    if !reply.boxes.is_empty() && !job.boxes {
        return Err(ProtoError("boxes nobody asked for"));
    }
    if job.boxes && reply.boxes.len() != reply.chars.len() {
        return Err(ProtoError("box count differs from character count"));
    }
    if reply.hidden_checked && !job.hidden_check {
        return Err(ProtoError("hidden check nobody asked for"));
    }
    if !reply.hidden_checked && reply.chars.iter().any(|c| c.has(CharRecord::NO_INK)) {
        return Err(ProtoError("ink flag without a check"));
    }
    Ok(())
}

/// Why a job or an open failed, as far as the worker knows. The host maps these to words.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum FailCode {
    /// The page number is past the end, or the file's page can't be loaded.
    BadPage = 1,
    /// A rectangle or scale the page can't honour.
    BadRequest = 2,
    /// The page is larger at this scale than the wire allows.
    TooLarge = 3,
    NeedsPassword = 4,
    /// Not a PDF, or too damaged to open.
    NotPdf = 5,
    /// Encryption PDFium doesn't support.
    Security = 6,
    /// PDFium couldn't read the descriptor.
    Unreadable = 7,
    /// PDFium refused without saying why.
    Internal = 8,
    /// A job arrived before the document was open.
    NoDocument = 9,
    /// The surface the host passed doesn't fit the job.
    BadSurface = 10,
}

impl FailCode {
    fn from_u8(value: u8) -> Result<Self, ProtoError> {
        Ok(match value {
            1 => Self::BadPage,
            2 => Self::BadRequest,
            3 => Self::TooLarge,
            4 => Self::NeedsPassword,
            5 => Self::NotPdf,
            6 => Self::Security,
            7 => Self::Unreadable,
            8 => Self::Internal,
            9 => Self::NoDocument,
            10 => Self::BadSurface,
            _ => return Err(ProtoError("unknown failure code")),
        })
    }
}

/// The answer to any job the worker refused or couldn't finish.
pub const FAILED_LEN: usize = 5;

pub fn encode_failed(job_id: u32, code: FailCode) -> Vec<u8> {
    let mut p = [0; FAILED_LEN];
    p[..4].copy_from_slice(&job_id.to_le_bytes());
    p[4] = code as u8;
    frame(Kind::Failed, &p)
}

pub fn decode_failed(payload: &[u8]) -> Result<(u32, FailCode), ProtoError> {
    let mut r = Reader(payload);
    let (job_id, code) = (r.u32()?, r.u8()?);
    r.end()?;
    Ok((job_id, FailCode::from_u8(code)?))
}

#[cfg(test)]
mod tests;
