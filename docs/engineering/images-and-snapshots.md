# Images and snapshots

> Part of the engineering notes: [`README.md`](README.md).

SPEC section 6, Extraction, and the owner's 2026-11-08 decision on copy-restricted files. `recto_agent::extract` is the one place that says what comes out and where it goes: `recto images`, `recto image` and `recto snapshot` write files or answer, and the app's snapshot tool asks `image_clip` and `snapshot_clip` for the clipboard, with its types and their order.

**Images go through the contents walk, asking only for images.** A `ContentsJob` now says which images it wants (`images`): everything, as `export svg` has it; `IMAGES_LIST`, where each image is as an `ImagePlace` record (its own pixels, whether it's a JPEG stream, its matrix) with no bytes and nothing else of the page; or one number, that image whole and as itself, with no clip, group or blend, a masked one as a single RGBA picture (`MASK_NOT_SEPARABLE`). Both modes count images at the same point of the same walk, so a number from a list is the image it names, and layers still decide: an image in a layer that's off isn't listed and can't be clicked. The host's `StreamCheck::for_images` refuses whatever the job didn't ask for, and a second picture for one number, which kills the worker as any violation does. A list draws nothing, so on the 346 MiB USGS map (66 images) it took 0.82 s, release, at load 10 to 12, with the host at 3.8 MiB peak and the worker at 351 MiB, nearly all PDFium opening that page.

**What an image comes out as.** `.jpg` takes a JPEG's own bytes, or its picture byte for byte with what's around it corrected (`jpeg_header_fixed`, as `export svg` does), and anything else is refused before a byte is written (`not_a_jpeg`), as it would be re-encoded. `.png` takes any image losslessly at its own size, a JPEG decoded as the page decodes it, a mask as alpha. An image PDFium can draw but not hand over is drawn alone at 2x and says `lossless: false`. On the clipboard, a JPEG goes as `public.jpeg` with a `public.png` beside it for apps that can't take a JPEG; anything else as `public.png` alone.

**A snapshot** is a box in points on the page as it's shown. PNG and JPG go through the export's drawing (`draw_page_box`, strips over the box, the page itself held to the caps, so a box can't get past them), sharp at the scale asked or refused. JPG is a small baseline encoder of ours (`render/jpeg.rs`: 4:4:4, quality 90, Annex K tables, the AAN float transform), streaming eight rows at a time, and every answer says it's re-encoded. SVG is `export svg`'s writer cut to the box, now generic over where it writes (`Output`: a temp file, or memory up to a cap). Text is `PageText::text_in_box`: characters whose middle is in the box, a line at a time in reading order, hidden text and chrome left out by a copy's rules, and a box of only chrome gets the chrome. On the clipboard all four go at once, PNG, text, SVG and JPG, each up to 32 MiB and 64 MiB in all: past that one is left off and the notice says which. The PNG is the snapshot, so its failure is the snapshot's.

**A file that asks not to be copied** is refused, exit 8 and `copy_restricted`, unless `--anyway`, for every format and for images, and for every export too: `render`, `export svg` with or without `--rect`, and `print --to-pdf` (owner, 2026-11-08: "Yes, same step"). Each use that gives something goes in `activity-overrides.jsonl`, as `copy --anyway` does, checked end to end through the real `recto`. **The flag is read straight from the file** (`recto_core::copy_restricted`): the encryption dictionary's `/P`, bit 5, which is document-wide and plain, so no page, worker or character is read, and a refusal costs nothing. Only a file the structure reader can't open asks the renderer, and one neither can say is taken as not asking, so an image copy can't fail because a page's characters can't be read (review, 2026-11-08). In the app, every export path offers the notice: the snapshot tool's "Copy anyway", and a print the panel saves or previews as a PDF, which gets "Print anyway". Each button names the notice that offered it, so a stale one takes nothing, and turning the tool or pressing for a new box takes the notice away.

**The clipboard, after review** (2026-11-08):
- **One job at a time.** A release while a snapshot is being taken stops it, through its worker's cancel word (`ClipStop`), and waits in its place, the newest only (`JobSlot`). Ten clicks on the USGS map were ten workers of 350 to 750 MiB at once; now they're one at a time, on the one thread.
- **Every item is made into a buffer capped at the room left** under 32 MiB each and 64 MiB in all, so nothing past the cap is made and then dropped.
- **PNG and JPG come from one drawing**, each row going to both encoders; the JPG is the extra, dropped if it passes its cap.
- **The SVG is promised, not made:** it's declared on the pasteboard and made by `extract::snapshot_svg` only when a paste asks for that type, which on the map is the larger worker's 742 MiB. Another process reading the PNG and the JPEG left it unmade (checked with a second process on the quiet run's named pasteboard); a read in this process resolves the item's promises, which `qa copied` allows for. The system asks on the UI thread and waits: the one wait there, only for a paste of the SVG from another app. On the way out a promise isn't kept.
- **The clipboard's SVG has text as outlines only** (`TextMode::Paths`), so hidden text stays out of it as from the text item; a file's SVG keeps real text, as `export svg` does.
- **One offer.** A text copy and a snapshot reach the host as one `ClipboardOffer` from `take_clipboard`, its types in order and which are promised, and each type's bytes come one at a time from `clipboard_data`, dropped from Rust as they go. So at most one item is across the bridge at once, rather than all of them in one record: the copies are Rust's buffer, UniFFI's, Swift's `Data` and the pasteboard's. Fewer would need a C entry point handing Swift memory it frees itself (phase R).
- **A JPEG on the clipboard ends where its picture does:** bytes a file put after the end are left off. A file written keeps every byte.
- **`image --out` is held to `OutputCaps::max_bytes`,** as `render` is (`LimitedOut`).

**The clipboard before and after the review's fixes,** measured 2026-11-08 on the USGS map's page 1, release builds, this Mac under the build queue's load (load 6 to 11, other agents building), by `examples/clip_memory.rs`: `snapshot_clip` as the app's copy calls it, with this process's peak `phys_footprint`, the largest worker's, and the tree's peak (this process and every worker at once, sampled every 5 ms). Before is `b4e103d`, the branch as reviewed. Two runs each for the smaller box, the same to the MiB.

| Box | | Took | Host peak | Largest worker | Tree peak | Made |
|---|---|---:|---:|---:|---:|---:|
| 600 x 400 pt | before: PNG, text, SVG and JPG, each its own drawing | 3.1 s | 17.6 MiB | 720 MiB | 733 MiB | 7.0 MB |
| 600 x 400 pt | after: the copy, SVG promised | 0.74 s | 8.8 MiB | 489 MiB | 494 MiB | 1.1 MB |
| 600 x 400 pt | after: a paste of the SVG, when one asks | 1.85 s | 14.9 MiB | 706 MiB | 721 MiB | 5.9 MB |
| 1200 x 900 pt | before | 7.3 s | 63.9 MiB | 762 MiB | 809 MiB | 38.2 MB |
| 1200 x 900 pt | after: the copy | 1.42 s | 27.3 MiB | 524 MiB | 527 MiB | 6.0 MB |

So a box copy no longer drives the larger worker unless the SVG is pasted, and ten clicks are one worker at a time. The copy's own worker is the page's opening and its characters for the text item, which a copy of text costs too. **Not measured:** the app itself, and a real paste into another app, whose request for the SVG was checked to arrive by type only (a second process reading the PNG and the JPEG from the quiet run's pasteboard didn't make it).

**Where they write.** Every export now keeps Save As's rules (`saving::path_problem`, found in a review of Save): the right ending, no hidden folder on the way, as given or as it leads, nothing through a link to a file of another kind, and nothing in `~/Library`. `render` and `export svg` checked only the ending.

**In the window.** The tool is `recto_reader::snapshot` (two points and a flag) and `recto-ffi::snapshot_tool`. While it's on, `selection_press`, `selection_drag` and `selection_release` go to it instead of to text selection, so the host's mouse code is the same, and so are QA's `mouse-down`, `mouse-drag` and `mouse-up`. A drag draws a box (`ReaderOp::SnapshotBox`, a dashed `CAShapeLayer` on the page's layer, in its coordinates, so it zooms and scrolls with it); a press that moves no more than 3 points is a click, which takes the image on top under it. A release starts one `recto-snapshot` thread with a worker of its own and says "Copying…"; a release meanwhile stops it and waits in its place, and only the newest is taken. A box dragged across a page break stays on the page it was pressed on, held to its edge (`Engine::point_on_given_page`). What it took goes to the clipboard through `copy_ready`, `take_clipboard` and `clipboard_data`, as a text copy does. An update waits for it, and so does `qa wait-settled`. At rest the tool has no thread, worker, timer or buffer. "Copy anyway" on the notice takes the same box or image again. Its pointer is a crosshair. **It isn't in the state record:** it starts off on every launch, as a tool does in Preview, which is the one exception it adds to decision 23.

**Measured** 2026-11-08, release `recto`, this Mac, other agents' builds running (load 8 to 12), whole process by `/usr/bin/time -l`, worker by its own peak:

| | Took | `recto` peak | Worker peak |
|---|---:|---:|---:|
| USGS map, `images 1` | 0.82 s | 3.8 MiB | 351 MiB |
| USGS map, `image 1 --number 1` to PNG, 5104 x 6004, 17.3 MB | 3.9 s | 7.2 MiB | 429 MiB |
| USGS map, the same to `.jpg`, refused | 0.69 s | 4.5 MiB | |
| USGS map, a 600 x 400 pt box as PNG at 2x | 0.43 s | 7.3 MiB | 352 MiB |
| USGS map, the whole page as a box, PNG at 2x, 4464 x 4032 | 1.05 s | 37.1 MiB | 417 MiB |
| USGS map, a 600 x 400 pt box as SVG | 2.7 s | 7.5 MiB | 742 MiB, in the larger worker |
| A made 600 dpi scan, 5100 x 6600 grey, `image` to PNG | 0.10 s | 6.4 MiB | 39 MiB |
| The scan, whole page as JPG at 4x, 2448 x 3168 | 0.10 s | 33.7 MiB | 84 MiB |

The 30 MiB in the largest rows is the strip of shared memory the host copies rows out of, as `render` has. **The app,** a debug build in a quiet run, at load 8 to 9: on the USGS map, the app at 20.9 MiB before, a click on its image took 6 s and left it at 43.2 MiB, the PNG's 23 MiB being what NSPasteboard keeps in the process; a box of 1200 x 900 pt took 14 s, nearly all its 27.8 MB SVG, and left the app at 52.2 MiB. Workers back to 0 and threads back down after each. A release app and a real clipboard weren't measured.

**Not tested.** A real paste into another app: a quiet run has a clipboard of its own, and what's on it is checked by type, size and SHA-256 (`qa copied`). A JPEG in CMYK or with a profile as the clipboard's JPEG: it goes as PNG, as `.jpg` refuses it. Right-to-left and vertical text in a box.
