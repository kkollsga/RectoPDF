//! **A sleep file: what a tab's first frame needs, without opening its PDF** (SPEC decision 25). Owner, 2026-11-07: "When it's on deep sleep, we want its state to be stored on disk, not on memory. But it needs to be stored in a way that's quick to recover without any delay."
//!
//! **Facts only:** the sizes of the pages known, the contents and page labels, the document's length and its file's version, so a tab woken from one is on
//! its paper at its place, at the right sizes, from its first frame, and fills in as its pages are drawn. **No picture of any page is kept** (owner,
//! 2026-11-15: "Remove the image loading feature all together"): a picture in the state folder could be read by any program of the person's. It's about
//! one version of one file, by its stamp, and it's only ever a way to be quick: **it can always be thrown away and rebuilt from the PDF,** unlike a journal,
//! so anything wrong with one is a reason to do just that.
//!
//! **Treated as hostile,** as anything in a folder can be changed. A checksum says it's whole and nothing more. Every number's held to what a real one
//! could be before it sizes anything, a title goes through the same cleaning as one from a PDF, and nothing may follow the one line of facts. A
//! password-protected document never gets one, as this would be its contents in the clear.

use crate::record::{FileRecord, RecordError, open_sealed, seal};
use recto_core::nav::{LabelRange, LabelStyle, MAX_LABEL_PREFIX_BYTES, MAX_LABEL_RANGES, PageLabels};
use recto_core::{MAX_TOC_DEPTH, MAX_TOC_ENTRIES, NONE, Toc, TocLine};
use recto_platform::FileStamp;
use serde::{Deserialize, Serialize};

/// What a sleep file says it is.
pub const SLEEP_KIND: &str = "recto-sleep";

/// **The sleep file's own format,** apart from the record's version in its header, as nothing else in one says when it was written. **A file of an older
/// format is never read:** it's removed, at its first read as any that doesn't read is, and by the pass after each launch
/// ([`sleep_format_of`]), as every format before 4 could hold pictures of pages, which no file holds since (2026-11-15). 1, with no field, was written up to
/// 2026-11-14 (0.1.0 included); 2 held no entry's place down its page; 3 held pictures. One of a newer format is read: what it adds, this build passes
/// over. Costs one wake from the PDF per tab, once, after the update.
pub const SLEEP_FORMAT: u32 = 4;

/// The most a file can be read as. Facts only: a full table of sizes and the most contents an entry list holds come to well under this.
pub const MAX_SLEEP_FILE_BYTES: usize = 1024 * 1024;
/// Page sizes kept, past which the rest are asked for as they come. What's held of a tab for its file is cut to this too, before it waits to be written.
pub const MAX_SIZES: usize = 4096;
/// **What of a document's contents a sleep file may carry, in bytes** ([`contents_bytes`]): a document whose contents are more (a PDF may have up to
/// 100,000 entries of 512 bytes, so about 2,000 long titles pass this) has no sleep file, as a protected one hasn't, and wakes from its PDF with all of them,
/// rather than from a file that drops some for the rest of the session. It holds a file within [`MAX_SLEEP_FILE_BYTES`] however its titles are escaped,
/// with every page size it keeps (`a_file_at_every_cap_is_within_what_a_file_may_be`), and each of the queue's slots to it: with [`MAX_SIZES`] sizes,
/// about 300 KiB a slot, 5 MiB for its 17 at most.
pub const MAX_SLEEP_CONTENTS_BYTES: usize = 256 * 1024;

/// **What a document's contents come to, as a sleep file's cap counts them:** each title's bytes and an entry's numbers, and each page-label range's prefix,
/// twice, as an escape can double it, and its numbers.
#[must_use]
pub fn contents_bytes(toc: &Toc) -> usize {
    let entries: usize = toc
        .entries()
        .iter()
        .map(|entry| toc.title(entry).len() + size_of::<recto_core::TocEntry>())
        .sum();
    let labels: usize = toc.labels.ranges().iter().map(|range| 2 * range.prefix.len() + 32).sum();
    entries + labels
}

/// Whether a document's contents fit in a sleep file of at most `cap` bytes of them ([`MAX_SLEEP_CONTENTS_BYTES`], set small by a test).
#[must_use]
pub fn contents_fit(toc: &Toc, cap: usize) -> bool {
    contents_bytes(toc) <= cap
}

/// How much of a file's start [`sleep_format_of`] needs: its header line and the start of its facts, where the format is said first.
pub const FORMAT_PREFIX_BYTES: usize = 4096;

/// What a sleep file holds.
#[derive(Clone, Debug, PartialEq)]
pub struct SleepFile {
    /// The file this is about. Anything else at that path, and it says nothing.
    pub file: FileStamp,
    /// How long the file is, as opposed to the document with its edits.
    pub file_pages: u32,
    /// How long the document was, and what was on its pages, so a list that's different now says this isn't about it. See `PageEdits::order_mark`.
    pub pages: u32,
    pub list_seq: u64,
    pub renderer_only: bool,
    /// Which version of the file its sizes are of, as the worker said: a rewrite that keeps size, time and number isn't this.
    pub version: Option<u64>,
    pub toc: Toc,
    /// The usual page's size, in points, and the known sizes of pages that aren't it.
    pub usual: Option<(f32, f32)>,
    pub sizes: Vec<(u32, f32, f32)>,
}

#[derive(Serialize, Deserialize)]
struct Meta {
    /// [`SLEEP_FORMAT`]: none in a file written before it was said, which reads as 0. First, so [`sleep_format_of`] finds it in a file's first bytes.
    #[serde(default)]
    sleep_format: u32,
    file: FileRecord,
    file_pages: u32,
    pages: u32,
    list_seq: u64,
    renderer_only: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    version: Option<u64>,
    truncated: bool,
    toc: Vec<(String, u32, u32, u8)>,
    /// Each entry's place down its page, in thousandths from the top, in the entries' order. Empty when no entry has one.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    tops: Vec<u16>,
    /// The file's page labels, as `(start, first, style, prefix)`, so a tab that wakes from here shows `iv` as it did.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    labels: Vec<(u32, u32, u8, String)>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    labels_truncated: bool,
    usual: Option<(f32, f32)>,
    sizes: Vec<(u32, f32, f32)>,
}

/// Just the format a file's first line says, for the pass that removes older ones: `sleep_format` comes first in it.
#[derive(Deserialize)]
struct FormatOnly {
    #[serde(default)]
    sleep_format: u32,
}

fn good_size(width: f32, height: f32) -> bool {
    width.is_finite() && height.is_finite() && width > 0.0 && height > 0.0 && width <= 14_400.0 && height <= 14_400.0
}

/// A file's bytes.
pub fn encode_sleep(sleep: &SleepFile) -> Vec<u8> {
    let toc = sleep
        .toc
        .entries()
        .iter()
        .map(|entry| (sleep.toc.title(entry).to_string(), entry.page, entry.parent, entry.depth))
        .collect();
    let tops: Vec<u16> = sleep.toc.entries().iter().map(|entry| entry.top).collect();
    let tops = if tops.iter().all(|top| *top == 0) { Vec::new() } else { tops };
    let sizes = sleep.sizes.iter().take(MAX_SIZES).copied().collect();
    let labels = sleep
        .toc
        .labels
        .ranges()
        .iter()
        .map(|range| (range.start, range.first, range.style.code(), range.prefix.to_string()))
        .collect();
    let meta = Meta {
        sleep_format: SLEEP_FORMAT,
        file: sleep.file.into(),
        file_pages: sleep.file_pages,
        pages: sleep.pages,
        list_seq: sleep.list_seq,
        renderer_only: sleep.renderer_only,
        version: sleep.version,
        truncated: sleep.toc.truncated,
        toc,
        tops,
        labels,
        labels_truncated: sleep.toc.labels.truncated,
        usual: sleep.usual,
        sizes,
    };
    let mut body = serde_json::to_vec(&meta).unwrap_or_default();
    body.push(b'\n');
    seal(SLEEP_KIND, 0, &body)
}

/// **The format a sleep file says it is, from its first bytes alone** (at most [`FORMAT_PREFIX_BYTES`]): what the pass after a launch removes older files
/// by, without reading or checking the rest. `None` when those bytes aren't a sleep file's start with its format first, as every build since the format was
/// said writes it, which the pass treats as older too: 0.1.0's files say none, and a file that can't say it's of this format has nothing a wake can use. A
/// forged file saying it's newer is kept, and checked whole when it's read. Reads no picture, as it reads nothing past the format.
#[must_use]
pub fn sleep_format_of(prefix: &[u8]) -> Option<u32> {
    let prefix = &prefix[..prefix.len().min(FORMAT_PREFIX_BYTES)];
    let header_end = prefix.iter().take(256).position(|byte| *byte == b'\n')?;
    let header: serde_json::Value = serde_json::from_slice(&prefix[..header_end]).ok()?;
    if header.get("format").and_then(serde_json::Value::as_str) != Some(SLEEP_KIND) {
        return None;
    }
    let body = &prefix[header_end + 1..];
    // The facts' first field, read as far as it goes: `{"sleep_format":4,` is all that's needed. A file with none (format 1, 0.1.0's) starts with another,
    // and says nothing.
    let key = br#"{"sleep_format":"#;
    if !body.starts_with(key) {
        return None;
    }
    let digits: Vec<u8> = body[key.len()..].iter().take(10).take_while(|byte| byte.is_ascii_digit()).copied().collect();
    let format: FormatOnly = serde_json::from_slice(&[key.as_slice(), &digits, b"}"].concat()).ok()?;
    Some(format.sleep_format)
}

/// Reads a file's bytes, believing none of them.
pub fn decode_sleep(bytes: &[u8], newest: u32) -> Result<SleepFile, RecordError> {
    let bad = RecordError::Damaged;
    if bytes.len() > MAX_SLEEP_FILE_BYTES {
        return Err(bad("too large"));
    }
    let (_, _, body) = open_sealed(SLEEP_KIND, bytes, newest)?;
    // The format first, from the start alone, before anything else is parsed: an older file's facts and what follows them are never read.
    if sleep_format_of(bytes).is_none_or(|format| format < SLEEP_FORMAT) {
        return Err(bad("an older format"));
    }
    let end = body.iter().position(|byte| *byte == b'\n').ok_or(bad("no first line"))?;
    let meta: Meta = serde_json::from_slice(&body[..end]).map_err(|_| bad("a first line that doesn't read"))?;
    if meta.sleep_format < SLEEP_FORMAT {
        return Err(bad("an older format"));
    }
    // Facts are all a file holds: whatever follows them (an older format's pictures, or a newer one's) isn't looked at, and makes it no file of this one's.
    if end + 1 != body.len() {
        return Err(bad("something after the facts"));
    }
    if meta.file_pages as usize > crate::edit::MAX_PAGES || meta.pages as usize > crate::edit::MAX_PAGES || meta.pages == 0 || meta.file_pages == 0 {
        return Err(bad("too many pages"));
    }
    if meta.toc.len() > MAX_TOC_ENTRIES || meta.sizes.len() > MAX_SIZES || meta.labels.len() > MAX_LABEL_RANGES {
        return Err(bad("too long a list"));
    }
    let lines = meta.toc.iter().enumerate().map(|(at, (title, page, parent, depth))| {
        let fits = (*page == NONE || *page < meta.file_pages) && (*parent == NONE || (*parent as usize) < at) && *depth <= MAX_TOC_DEPTH;
        fits.then_some(TocLine {
            title: title.as_str(),
            page: *page,
            parent: *parent,
            depth: *depth,
        })
    });
    let lines: Vec<TocLine<'_>> = lines.collect::<Option<_>>().ok_or(bad("contents out of range"))?;
    if meta.tops.len() > meta.toc.len() {
        return Err(bad("more places than entries"));
    }
    let mut toc = Toc::from_lines(&lines, meta.truncated).with_tops(&meta.tops);
    if meta
        .labels
        .iter()
        .any(|(start, _, _, prefix)| *start >= meta.file_pages || prefix.len() > MAX_LABEL_PREFIX_BYTES)
    {
        return Err(bad("page labels out of range"));
    }
    // Held to the same rules as a PDF's own, as whatever wrote this can't be believed.
    toc.labels = PageLabels::from_ranges(
        meta.labels
            .iter()
            .map(|(start, first, style, prefix)| LabelRange {
                start: *start,
                first: *first,
                style: LabelStyle::from_code(*style),
                prefix: prefix.as_str().into(),
            })
            .collect(),
        meta.labels_truncated,
    );
    if meta.usual.is_some_and(|(width, height)| !good_size(width, height)) {
        return Err(bad("a page of an impossible size"));
    }
    if meta
        .sizes
        .iter()
        .any(|(page, width, height)| *page >= meta.pages || !good_size(*width, *height))
    {
        return Err(bad("a page of an impossible size"));
    }
    Ok(SleepFile {
        file: meta.file.into(),
        file_pages: meta.file_pages,
        pages: meta.pages,
        list_seq: meta.list_seq,
        renderer_only: meta.renderer_only,
        version: meta.version,
        toc,
        usual: meta.usual,
        sizes: meta.sizes,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::record::CURRENT_VERSION;

    fn sample() -> SleepFile {
        let toc = Toc::from_lines(
            &[
                TocLine {
                    title: "One",
                    page: 0,
                    parent: NONE,
                    depth: 0,
                },
                TocLine {
                    title: "Inside",
                    page: 2,
                    parent: 0,
                    depth: 1,
                },
            ],
            false,
        );
        SleepFile {
            file: FileStamp {
                size: 9,
                mtime_ns: 5,
                device: 1,
                inode: 2,
            },
            file_pages: 5,
            pages: 4,
            list_seq: 7,
            renderer_only: false,
            version: Some(3),
            toc,
            usual: Some((612.0, 792.0)),
            sizes: vec![(3, 300.0, 400.0)],
        }
    }

    /// Failure: what goes in isn't what comes out, so a tab wakes to the wrong contents, or its pages at the wrong sizes.
    #[test]
    fn a_sleep_file_reads_back_as_it_was() {
        let sleep = sample();
        assert_eq!(decode_sleep(&encode_sleep(&sleep), CURRENT_VERSION).unwrap(), sleep);
    }

    /// A good sleep file with its first line changed, sealed again as a forger would, and read.
    fn forge(good: &[u8], change: &dyn Fn(&mut serde_json::Value)) -> Result<SleepFile, RecordError> {
        let body = &good[good.iter().position(|byte| *byte == b'\n').unwrap() + 1..];
        let end = body.iter().position(|byte| *byte == b'\n').unwrap();
        let mut meta = serde_json::from_slice::<serde_json::Value>(&body[..end]).unwrap();
        change(&mut meta);
        // Written as a build writes it, its format first, where the start of a file says it.
        let format = meta.as_object_mut().unwrap().remove("sleep_format");
        let mut forged = serde_json::to_vec(&meta).unwrap();
        if let Some(format) = format {
            let rest = if forged.len() > 2 { [b",".as_slice(), &forged[1..]].concat() } else { b"}".to_vec() };
            forged = [format!("{{\"sleep_format\":{format}").as_bytes(), &rest].concat();
        }
        forged.push(b'\n');
        forged.extend_from_slice(&body[end + 1..]);
        decode_sleep(&seal(SLEEP_KIND, 0, &forged), CURRENT_VERSION)
    }

    /// Failure (hostile input): a forged sleep file with a good checksum gets numbers past what's real into a layout, a memory allocation or a page's place,
    /// or panics. Every one is refused, and only costs the sleep file. A flipped byte anywhere is caught.
    #[test]
    fn a_forged_file_is_refused() {
        let good = encode_sleep(&sample());
        let body_of = |bytes: &[u8]| bytes[bytes.iter().position(|byte| *byte == b'\n').unwrap() + 1..].to_vec();
        let forged = |change: &dyn Fn(&mut serde_json::Value)| forge(&good, change);
        assert!(forged(&|_| {}).is_ok(), "the forging works");
        type Change = fn(&mut serde_json::Value);
        let changes: [(&str, Change); 8] = [
            ("billions of pages", |meta| meta["pages"] = 4_000_000_000u32.into()),
            ("no pages", |meta| meta["pages"] = 0.into()),
            ("a billion-page file", |meta| meta["file_pages"] = 4_000_000_000u32.into()),
            ("a page of a mile", |meta| meta["usual"] = serde_json::json!([1_000_000.0, 1.0])),
            ("a size past the end", |meta| meta["sizes"][0][0] = 4.into()),
            ("too many sizes", |meta| {
                meta["sizes"] = serde_json::Value::Array(vec![serde_json::json!([0, 1.0, 1.0]); MAX_SIZES + 1])
            }),
            ("contents past the end", |meta| meta["toc"][1][1] = 5.into()),
            ("a parent that comes later", |meta| meta["toc"][0][2] = 1.into()),
        ];
        for (what, change) in changes {
            assert!(forged(&change).is_err(), "{what}");
        }
        // Anything after the facts, and anything flipped.
        let mut long = body_of(&good);
        long.push(0);
        assert!(decode_sleep(&seal(SLEEP_KIND, 0, &long), CURRENT_VERSION).is_err());
        for at in (0..good.len()).step_by(7) {
            let mut flipped = good.clone();
            flipped[at] ^= 0x40;
            assert!(decode_sleep(&flipped, CURRENT_VERSION).is_err(), "byte {at}");
        }
        assert!(
            matches!(
                decode_sleep(&crate::record::seal_as(SLEEP_KIND, CURRENT_VERSION + 1, 0, &body_of(&good)), CURRENT_VERSION),
                Err(RecordError::Newer(_))
            ),
            "a newer app's is left to it"
        );
    }

    /// Failure: a tab woken from its sleep file shows plain numbers where it showed `iv`, as its labels weren't kept; or a forged file's labels reach the screen uncleaned or out of range.
    #[test]
    fn page_labels_are_kept_and_checked() {
        use recto_core::nav::{LabelRange, LabelStyle, PageLabels};
        let mut sleep = sample();
        let range = |start, style, prefix: &str| LabelRange {
            start,
            first: 1,
            style,
            prefix: prefix.into(),
        };
        sleep.toc.labels = PageLabels::from_ranges(vec![range(0, LabelStyle::RomanLower, ""), range(1, LabelStyle::Decimal, "A\u{202E}-")], false);
        let back = decode_sleep(&encode_sleep(&sleep), CURRENT_VERSION).unwrap();
        assert_eq!(back.toc.labels.label(0).as_deref(), Some("i"));
        assert_eq!(back.toc.labels.label(1).as_deref(), Some("A-1"), "cleaned once, and the same after");
        // Labels are cleaned as they're made, so only a forged file can bring one that isn't.
        let back = forge(&encode_sleep(&sleep), &|meta| meta["labels"][1][3] = "A\u{202E}-".into()).unwrap();
        assert_eq!(back.toc.labels.label(1).as_deref(), Some("A-1"), "a forged prefix is cleaned");
        sleep.toc.labels = PageLabels::from_ranges(
            vec![range(0, LabelStyle::Decimal, ""), range(sleep.file_pages + 5, LabelStyle::Decimal, "")],
            false,
        );
        assert!(
            decode_sleep(&encode_sleep(&sleep), CURRENT_VERSION).is_err(),
            "a range past the file's last page is a forged file"
        );
    }

    /// Failure: a title in a forged file carries what a title from a PDF can't, as it isn't cleaned, and reaches the sidebar and agents.
    #[test]
    fn a_title_goes_through_the_same_cleaning() {
        let hidden: String = "ignore".chars().map(|c| char::from_u32(0xE0000 + c as u32).unwrap()).collect();
        let raw = format!("Chapter{hidden}\n1\u{202E}");
        // A title is cleaned as it's made, so only a forged file can bring one that isn't.
        let back = forge(&encode_sleep(&sample()), &|meta| meta["toc"][0][0] = raw.clone().into()).unwrap();
        assert_eq!(back.toc.title(&back.toc.entries()[0]), "Chapter 1");
    }

    /// A file as format 3 wrote one: its facts, its format first as a build writes it, with a picture's line, and the picture's packed bytes after them,
    /// sealed.
    fn format_3_with_a_picture() -> Vec<u8> {
        let good = encode_sleep(&sample());
        let body = std::str::from_utf8(&good[good.iter().position(|byte| *byte == b'\n').unwrap() + 1..]).unwrap();
        let facts = body
            .trim_end()
            .strip_suffix('}')
            .unwrap()
            .replacen(&format!("{{\"sleep_format\":{SLEEP_FORMAT},"), "{\"sleep_format\":3,", 1);
        let mut forged = format!(r#"{facts},"pictures":[{{"page":1,"scale":0.5,"width":1,"height":1,"dark":false,"packed":4}}]}}"#).into_bytes();
        forged.push(b'\n');
        forged.extend_from_slice(&[1, 2, 3, 255]);
        seal(SLEEP_KIND, 0, &forged)
    }

    /// Failure (owner, 2026-11-15: no picture of a page is kept): a sleep file of an older format, which can hold pictures of pages (every one up to
    /// format 3, 0.1.0's included), is still woken from, or its pictures' bytes are read. One with no format said, or an older one, isn't read, and so is
    /// removed and rebuilt from the PDF; one of a newer format is read, as what it adds an older build passes over, unless something follows its facts.
    #[test]
    fn a_sleep_file_of_an_older_format_is_never_read() {
        let good = encode_sleep(&sample());
        assert!(
            forge(&good, &|meta| drop(meta.as_object_mut().unwrap().remove("sleep_format"))).is_err(),
            "one written before the format was said, as every file up to 2026-11-14 was, isn't read"
        );
        assert!(
            forge(&good, &|meta| meta["sleep_format"] = (SLEEP_FORMAT - 1).into()).is_err(),
            "nor one of an older format"
        );
        assert!(
            matches!(
                decode_sleep(&format_3_with_a_picture(), CURRENT_VERSION),
                Err(RecordError::Damaged("an older format"))
            ),
            "a file with pictures is refused at its format, before anything after its facts is looked at"
        );
        assert_eq!(
            forge(&good, &|meta| meta["sleep_format"] = (SLEEP_FORMAT + 1).into()).unwrap(),
            sample(),
            "a newer one is"
        );
    }

    /// Failure (going back: the updater can put 0.1.0 back after a failed update): a file this build writes reads in 0.1.0 as one with pictures or
    /// sizes it would show. 0.1.0's facts have `pictures` with no default, so a first line without that key doesn't read there and the file's removed and
    /// rebuilt from the PDF. This build writes no such key, and nothing after its facts.
    #[test]
    fn a_file_of_this_format_is_one_an_older_build_throws_away() {
        let good = encode_sleep(&sample());
        let body = &good[good.iter().position(|byte| *byte == b'\n').unwrap() + 1..];
        assert_eq!(body.iter().filter(|byte| **byte == b'\n').count(), 1, "one line of facts, and nothing after it");
        let meta = serde_json::from_slice::<serde_json::Value>(&body[..body.len() - 1]).unwrap();
        assert!(meta.get("pictures").is_none(), "{meta}");
    }

    /// Failure (review of the removal, should-fix 1): a document's contents past what a file may be are encoded at each leave and thrown away, as the
    /// store won't write a file that large, and the tab always wakes from its PDF with nothing said; or the queue holds such contents whole in every slot.
    /// The cap is said once: at the cap the contents fit, one byte past it they don't (with the cap set small).
    #[test]
    fn contents_fit_a_sleep_file_up_to_the_cap_and_not_past_it() {
        let toc = sample().toc;
        let bytes = contents_bytes(&toc);
        assert!(bytes > 0);
        assert!(contents_fit(&toc, bytes), "at the cap");
        assert!(!contents_fit(&toc, bytes - 1), "one byte past it");
    }

    /// Failure: the caps on what a sleep file carries don't hold a file within what a file may be read as, so a document within every cap is never woken
    /// from its file (the store refuses to write it). At the contents' cap, with titles that each escape to twice their bytes, every page size kept and the
    /// most page labels, the file is within [`MAX_SLEEP_FILE_BYTES`].
    #[test]
    fn a_file_at_every_cap_is_within_what_a_file_may_be() {
        let title = "\"".repeat(recto_core::MAX_TITLE_BYTES);
        let per_entry = title.len() + size_of::<recto_core::TocEntry>();
        let labels: Vec<recto_core::nav::LabelRange> = (0..recto_core::nav::MAX_LABEL_RANGES as u32)
            .map(|start| recto_core::nav::LabelRange {
                start,
                first: 1,
                style: recto_core::nav::LabelStyle::Decimal,
                prefix: "\"".repeat(recto_core::nav::MAX_LABEL_PREFIX_BYTES).into(),
            })
            .collect();
        let mut sleep = sample();
        sleep.file_pages = 100_000;
        sleep.pages = 100_000;
        sleep.toc.labels = PageLabels::from_ranges(labels, false);
        let room = MAX_SLEEP_CONTENTS_BYTES.saturating_sub(contents_bytes(&sleep.toc));
        let lines: Vec<TocLine<'_>> = (0..room / per_entry)
            .map(|at| TocLine {
                title: &title,
                page: u32::try_from(at).unwrap(),
                parent: NONE,
                depth: 0,
            })
            .collect();
        let labels = sleep.toc.labels.clone();
        sleep.toc = Toc::from_lines(&lines, false);
        sleep.toc.labels = labels;
        assert!(contents_fit(&sleep.toc, MAX_SLEEP_CONTENTS_BYTES));
        sleep.sizes = (0..MAX_SIZES as u32).map(|page| (page, 14_000.5, 14_000.5)).collect();
        let bytes = encode_sleep(&sleep);
        assert!(bytes.len() <= MAX_SLEEP_FILE_BYTES, "{} bytes", bytes.len());
        assert!(decode_sleep(&bytes, CURRENT_VERSION).is_ok());
    }

    /// Failure: the pass after a launch keeps a file of an older format, whose pictures stay on disk till the tab's next wake or 30 days, or removes one of
    /// this format, which costs its tab the quick way back. The format is read from the file's first bytes, whatever follows them.
    #[test]
    fn a_files_format_is_read_from_its_first_bytes() {
        let good = encode_sleep(&sample());
        assert_eq!(sleep_format_of(&good), Some(SLEEP_FORMAT));
        let header = good.iter().position(|byte| *byte == b'\n').unwrap() + 1;
        assert_eq!(sleep_format_of(&good[..header + 20]), Some(SLEEP_FORMAT), "the start is enough");
        assert_eq!(sleep_format_of(&format_3_with_a_picture()), Some(3));
        let body = &good[good.iter().position(|byte| *byte == b'\n').unwrap() + 1..];
        let mut meta = serde_json::from_slice::<serde_json::Value>(&body[..body.len() - 1]).unwrap();
        meta.as_object_mut().unwrap().remove("sleep_format");
        let format_1 = seal(SLEEP_KIND, 0, &serde_json::to_vec(&meta).unwrap());
        assert_eq!(sleep_format_of(&format_1), None, "0.1.0's files say no format, and the pass removes them");
        assert_eq!(sleep_format_of(b"junk"), None);
        assert_eq!(sleep_format_of(&seal("recto-journal", 0, b"{}\n")), None, "another kind of file");
        assert_eq!(sleep_format_of(&[]), None);
    }
}
