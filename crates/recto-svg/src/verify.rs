//! A strict little reader of what this crate writes, for tests here and in the crates that use it: one well-formed document, with nothing in it that can run or reach outside. A failed check is a panic, because it's only for tests and QA. It isn't an XML parser, and it doesn't need to be: anything it doesn't know is a failure.

/// The tags and attributes this crate may write. Anything else in the output is a way out that was missed.
const TAGS: &[&str] = &["svg", "g", "path", "defs", "clipPath", "use", "text", "image", "title", "linearGradient", "radialGradient", "stop"];
const ATTRIBUTES: &[&str] = &[
    "xmlns", "xmlns:xlink", "version", "width", "height", "viewBox", "transform", "d", "fill", "fill-opacity", "fill-rule", "stroke", "stroke-opacity", "stroke-width", "stroke-linecap", "stroke-linejoin", "stroke-dasharray", "stroke-dashoffset",
    "stroke-miterlimit", "opacity", "pointer-events", "vector-effect", "clip-path", "id", "xlink:href", "x", "y", "font-size", "font-family", "font-weight", "font-style", "xml:space", "preserveAspectRatio", "gradientUnits", "x1", "y1", "x2", "y2", "cx", "cy", "r", "offset", "stop-color", "stop-opacity",
];

/// Reads a whole document, and panics on anything that isn't well formed or isn't on the lists above.
pub fn check_document(bytes: &[u8]) {
    let text = std::str::from_utf8(bytes).expect("UTF-8");
    let mut rest = text.strip_prefix("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n").expect("the declaration");
    let mut stack: Vec<&str> = Vec::new();
    let mut seen_root = false;
    while !rest.is_empty() {
        if let Some(after) = rest.strip_prefix('<') {
            if let Some(after) = after.strip_prefix('/') {
                let end = after.find('>').expect("a closed tag");
                assert_eq!(stack.pop(), Some(&after[..end]), "a tag closed out of order");
                rest = &after[end + 1..];
                continue;
            }
            assert!(!after.starts_with('!') && !after.starts_with('?'), "no comment, DOCTYPE, CDATA or instruction may be written: {}", &after[..after.len().min(40)]);
            let name_end = after.find(|c: char| c.is_whitespace() || c == '>' || c == '/').expect("a tag's name");
            let name = &after[..name_end];
            assert!(TAGS.contains(&name), "the tag {name:?}");
            assert!(seen_root || name == "svg", "the root must be svg");
            seen_root = true;
            let mut at = &after[name_end..];
            loop {
                at = at.trim_start();
                if let Some(next) = at.strip_prefix("/>") {
                    rest = next;
                    break;
                }
                if let Some(next) = at.strip_prefix('>') {
                    stack.push(name);
                    rest = next;
                    break;
                }
                let eq = at.find('=').expect("an attribute");
                let key = &at[..eq];
                assert!(ATTRIBUTES.contains(&key), "the attribute {key:?}");
                assert!(!key.starts_with("on"), "an event attribute: {key}");
                let value_start = at[eq + 1..].strip_prefix('"').expect("a quoted value");
                let close = value_start.find('"').expect("a closed value");
                let value = &value_start[..close];
                assert!(!value.contains('<'), "a raw < in an attribute");
                check_entities(value);
                if key == "xlink:href" {
                    assert!(value.starts_with('#') || value.starts_with("data:image/png;base64,") || value.starts_with("data:image/jpeg;base64,"), "an href that leads out: {value}");
                }
                if key != "font-family" && key != "xlink:href" && !["d", "transform", "x", "y", "stroke-dasharray"].contains(&key) {
                    assert!(!value.to_ascii_lowercase().contains("javascript"), "{value}");
                }
                if key != "xlink:href" && key != "font-family" && key != "xmlns" && key != "xmlns:xlink" {
                    assert!(!value.contains("e+") && !value.contains("NaN") && !value.contains("inf"), "a number written wrongly: {key}={value}");
                }
                at = &value_start[close + 1..];
            }
        } else {
            let end = rest.find('<').unwrap_or(rest.len());
            check_entities(&rest[..end]);
            assert!(!rest[..end].contains('>'), "a raw > in text");
            for ch in rest[..end].chars() {
                assert!(crate::keep_in_text(ch), "{:#x} in text", ch as u32);
            }
            if end < rest.len() || !stack.is_empty() {
                assert!(!stack.is_empty() || rest[..end].trim().is_empty(), "text outside the root");
            }
            rest = &rest[end..];
        }
    }
    assert!(stack.is_empty(), "left open: {stack:?}");
    assert!(seen_root);
}

fn check_entities(mut text: &str) {
    while let Some(at) = text.find('&') {
        let after = &text[at + 1..];
        let end = after.find(';').expect("a closed entity");
        let entity = &after[..end];
        assert!(["amp", "lt", "gt", "quot", "apos", "#9", "#10", "#13"].contains(&entity), "the entity {entity}");
        text = &after[end + 1..];
    }
}

/// An image read back from a document.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Embedded {
    /// Raw JPEG bytes.
    Jpeg(Vec<u8>),
    /// Pixels, by rows: 1 grey, 3 RGB, 4 RGBA.
    Png { width: u32, height: u32, channels: usize, pixels: Vec<u8> },
}

/// Every image in a document, decoded byte for byte. For tests that an image really is lossless.
pub fn embedded_images(bytes: &[u8]) -> Vec<Embedded> {
    let text = std::str::from_utf8(bytes).expect("UTF-8");
    let mut images = Vec::new();
    let mut rest = text;
    while let Some(at) = rest.find("data:image/") {
        let after = &rest[at + "data:image/".len()..];
        let (kind, after) = after.split_once(";base64,").expect("base64 in a data URL");
        let (body, after) = after.split_once('"').expect("a closed value");
        let data = from_base64(body);
        images.push(match kind {
            "jpeg" => Embedded::Jpeg(data),
            "png" => read_png_own(&data),
            other => panic!("an image of kind {other}"),
        });
        rest = after;
    }
    images
}

fn from_base64(text: &str) -> Vec<u8> {
    let value = |byte: u8| match byte {
        b'A'..=b'Z' => u32::from(byte - b'A'),
        b'a'..=b'z' => u32::from(byte - b'a') + 26,
        b'0'..=b'9' => u32::from(byte - b'0') + 52,
        b'+' => 62,
        b'/' => 63,
        other => panic!("{other} in base64"),
    };
    let mut out = Vec::with_capacity(text.len() / 4 * 3);
    for group in text.as_bytes().chunks(4) {
        let pad = group.iter().filter(|&&byte| byte == b'=').count();
        let bits = group.iter().filter(|&&byte| byte != b'=').fold(0u32, |bits, &byte| bits << 6 | value(byte)) << (6 * pad);
        out.extend_from_slice(&bits.to_be_bytes()[1..4 - pad]);
    }
    out
}

fn paeth(left: u8, up: u8, corner: u8) -> u8 {
    let estimate = i32::from(left) + i32::from(up) - i32::from(corner);
    let (a, b, c) = ((estimate - i32::from(left)).abs(), (estimate - i32::from(up)).abs(), (estimate - i32::from(corner)).abs());
    if a <= b && a <= c {
        left
    } else if b <= c {
        up
    } else {
        corner
    }
}

/// A PNG the writer here made, which only ever filters by Up, so anything else in one is a fault in the writer.
fn read_png_own(file: &[u8]) -> Embedded {
    read_with(file, true)
}

/// Any plain 8-bit PNG, such as `recto render`'s or a browser's.
pub fn read_png(file: &[u8]) -> Embedded {
    read_with(file, false)
}

fn read_with(file: &[u8], only_up: bool) -> Embedded {
    use std::io::Read;
    assert_eq!(&file[..4], &[0x89, b'P', b'N', b'G'], "the signature");
    let (mut at, mut idat, mut header) = (8, Vec::new(), None);
    while at < file.len() {
        let len = u32::from_be_bytes(file[at..at + 4].try_into().expect("a length")) as usize;
        let kind = &file[at + 4..at + 8];
        let data = &file[at + 8..at + 8 + len];
        let mut crc = flate2::Crc::new();
        crc.update(kind);
        crc.update(data);
        assert_eq!(crc.sum().to_be_bytes(), file[at + 8 + len..at + 12 + len], "a chunk's checksum");
        match kind {
            b"IHDR" => header = Some(data.to_vec()),
            b"IDAT" => idat.extend_from_slice(data),
            _ => {}
        }
        at += 12 + len;
    }
    let header = header.expect("a header");
    let width = u32::from_be_bytes(header[..4].try_into().expect("a width"));
    let height = u32::from_be_bytes(header[4..8].try_into().expect("a height"));
    let channels = match header[9] {
        0 => 1,
        2 => 3,
        6 => 4,
        other => panic!("colour type {other}"),
    };
    let mut filtered = Vec::new();
    flate2::read::ZlibDecoder::new(&idat[..]).read_to_end(&mut filtered).expect("zlib");
    let row_len = width as usize * channels;
    let mut pixels = Vec::with_capacity(row_len * height as usize);
    let mut above = vec![0u8; row_len];
    for row in filtered.chunks(row_len + 1) {
        assert!(!only_up || row[0] == 2, "the Up filter");
        let mut now = vec![0u8; row_len];
        for (at, value) in row[1..].iter().enumerate() {
            let left = if at >= channels { now[at - channels] } else { 0 };
            let corner = if at >= channels { above[at - channels] } else { 0 };
            let up = above[at];
            now[at] = value.wrapping_add(match row[0] {
                0 => 0,
                1 => left,
                2 => up,
                3 => ((u16::from(left) + u16::from(up)) / 2) as u8,
                4 => paeth(left, up, corner),
                other => panic!("filter {other}"),
            });
        }
        pixels.extend_from_slice(&now);
        above = now;
    }
    assert_eq!(pixels.len(), row_len * height as usize, "rows missing");
    Embedded::Png { width, height, channels, pixels }
}
