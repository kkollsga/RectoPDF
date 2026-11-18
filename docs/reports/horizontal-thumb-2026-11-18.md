# Horizontal thumb investigation — 2026-11-18

The owner reports a missing horizontal scrollbar when the page is wider than the window. This remains **open and unconfirmed**, not fixed. Source baseline: `8b5156b9`.

## What was checked

Quiet QA commands exercised the existing UI methods with a temporary copy of `standards_ietf_rfc9110.pdf`: fit and wider zooms, 1440×1000 and 720×600 windows, overlay and legacy system scroller settings, three comparison panes, and close/reopen. The existing named native test `the_thumb_across_shows_with_a_page_wider_than_its_pane` passed. Screenshots and reports are retained locally in `/tmp/rectopdf-horizontal-20261118/` (`before-shots`, `passive-matrix`, `draw-diagnostic`, `no-tint-matrix`, and `rect-matrix`). These are local investigation artifacts, not permanent test fixtures.

The horizontal thumb was present when overflowing and absent at the fit in these cases. Reports were also checked with their implicit `refresh()` temporarily removed; no missing invalidation was found. Diagnostic draw counters showed every comparison pane reaching its paint code with valid geometry. This does not prove the owner's live-window case works.

## Disproved diagnosis

The middle comparison thumb initially looked absent in a scaled screenshot. A scan for long, uninterrupted runs of neutral pixels also missed it. That scan was unsuitable: page text shows through the translucent thumb and breaks those runs. Direct pixel inspection found the middle thumb at `(1700, 1990)` with RGB `(84, 85, 88)` and at `(1700, 1988)` with RGB `(96, 98, 100)`, matching sampled strokes in the other panes. The original capture, a diagnostic with paper tint disabled, and a separate-thumb capture had the same sampled values. The proposed screenshot fix therefore had no demonstrated benefit and was discarded, along with all diagnostic code and the unproven test extension.

No product or test implementation changed. No performance improvement is claimed. No events were posted, no visible app run was used, and the installed app was left alone.

## Next discriminator

Obtain the document and the zoom/window state where the owner sees the problem. Reproduce it through the existing QA methods, inspect the actual rendered thumb rather than only its `shown` flag, and add a regression assertion that fails on that reproduction before implementing a fix. Do not infer a missing thumb from uninterrupted pixel spans over variable page content.
