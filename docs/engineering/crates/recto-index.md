# recto-index

> Part of the engineering notes: [`../README.md`](../README.md).

## Contract

| Crate | Status | Contract | Must not |
|---|---|---|---|
| `recto-index` | **Started** | Line order, text normaliser and hidden-text report: pure functions on the host's side. Selecting: a page's text with its boxes as shown, hit-testing, words, lines, ranges and what's copied. The page-text corpus on disk: its format, its builder over any page source, its reader and eviction | Touch the UI; name a renderer |

## Per-item sizes

| Struct | Held per | Limit |
|---|---|---|
| `recto_index::select::Line` | Line of a page held for selecting, at most 16,384, on at most 8 pages past what's in use | ≤ 32 B |
| `recto_index::select::Selection` | One, on the page view, for what's on screen. No tab holds one | ≤ 64 B |
| `recto_index::order::Line` | Line of a page in hand, at most 16,384, only while it's being read | ≤ 72 B |
| `corpus::PageRecord` | Page in a corpus, on disk, read back one at a time | 28 B on disk |

## Dependencies

| Crate | Why |
|---|---|
| `crc32fast` | A checksum per page of a corpus and per line of a state record, in hardware on this Mac. Already in the tree through flate2, so it adds no crate. `recto-index` and `recto-session` |
| `sha2` | Names a corpus for its file, from its identity and samples of its bytes. Already in the tree through lopdf, so it adds no crate. `recto-index` only |

## Search text and line order

**Search text.** `recto-index::normalize` turns PDFium's characters into text `rg` can search. Its rules and their limits are in that crate's header. In short: at a line-end hyphen it follows the rest of the page if the page shows a form, keeps the hyphen for capitals, one- or two-letter tails and a short closed list of compound words, and otherwise joins. It's a guess, so the other spelling of each decision goes in `also_spelled`. Lines stay lines. On bench100 it joined 12,297 and kept 1,627, with no `simulationbased` or `datadriven` left, and line hits equal to the spike's normaliser on every common word checked but one (`office`, 633 against 634), plus 4 more for `simulation-based`. A hyphen on a page number or a marker doesn't join: a number between hyphens, `-17-`, whatever follows; a real roman numeral or one letter between hyphens, `-iv-`, unless a small letter follows; or a number or one letter alone on its line, `17-`, above a capital. Without that, `wbD34442284` p. 17 read `-17-PBC`. Real tokens still join: `10-` above `year`, `X-` above `ray`, `-civil-` above `ian`, a range of numbers alone on its line, and a number in a sentence, since that's how `21-cm` breaks. Over all 5,872 pages of bench100, compared line by line with the rule switched off, this changes 47 lines, all page numbers in 4 World Bank documents.

**Line order.** A PDF can draw a line's styled words after the rest of the page, and PDFium returns drawing order, as `pdftotext -raw` does. RFC 9110 p. 110 read `A recipient ignore If-Modified-Since`, with `MUST` at the bottom of the page: a dropped `MUST NOT` turns a sentence round, and a search for the real words finds nothing. `recto_index::read_page` is the one place `text` and `search` read a page, so they can't disagree. It uses each character's box. A later run goes into an earlier line if its baseline matches and it fits a word-sized gap, or sits a word's space past either end (`Section` at a line's end, `2.` at its start). Everything else stays in drawing order: no layout sort, since the spike (§10.2) found that right for two-column text on 79 of 84 pages.

The rules that keep it from doing harm: each side of a run with a neighbour must be within 0.8 of a character height, the middle of the line's characters so one tall symbol doesn't widen it. On RFC 9110, 99% of real gaps are under 0.76, with a median of 0.53. A run on the end of a line must not start or end level with the lines drawn around it, since that's what a table's cells do down a column: `wbD34442277` p. 11's gutter is 2.9 pt beside a 3 pt word space, so width alone can't tell them apart, and it read `utilizing at least utilizing at` before this. All of a line moves or none of it, and a run doesn't pass anything drawn since that sits on or against its line, so a fraction or a superscript stays where it is in an equation. Text running backwards, upwards or on a slope is left alone, a line-end hyphen stays at its line's end, and letters touching across the end of a line aren't joined. Only whitespace is ever added or dropped.

**What's left in drawing order:** a whole line drawn late with nothing on its baseline, such as RFC 9110's code samples and page headers. And reference lines whose punctuation is drawn first: `rfc9293` p. 85 reads `, , ,` then `eid1572>.`, because the later run overlaps those gaps rather than fitting them. Mathematics mostly stays as broken up as PDFium makes it, at every raised symbol. Not tested on a real right-to-left PDF: none of the 454 under `distillPDF` has any.

`crates/recto-agent/examples/line_order_corpus.rs` runs it over a folder, against `pdftotext`. On bench100, 5,872 pages: no page gained or lost a non-space character, 2,016 pages changed and 13,840 runs moved. Scored by the share of a page's runs of 8 characters, spaces taken out, that `pdftotext -layout` also has:

| Pages | Changed | Better | Same, within 0.1 of a point | Worse |
|---|---:|---:|---:|---:|
| Standards (RFCs) | 909 | 897 | 7 | 5 |
| Everything else | 1,107 | 69 | 994 | 44 |
| All | 2,016 | 966 | 1,001 | 49 |
| All, with the looser first rule | 2,353 | 1,104 | 1,060 | 189 |

**That score isn't a verdict.** It hid the table merge above as -0.015, so pages were read too: the first lines of the diffs of 31 changed pages on this build, including all 3 changed policy pages, 5 gov and 10 math and econ, and not the 1,985 others. 2 read worse to a person, neither by a column: `wbD34442284` p. 17, `-17-PBC`, where putting the page number's parts together let the normaliser's rule for a hyphen at a line's end join it to the next line (since fixed), and `cs_CV_2607.28611` p. 20, where a chart's axis ticks became one line, `18 10 19 10 20 10`. About 6 are no better and no worse, being display equations and map symbols. The rest read better: `which converts O2 and water (H2`, `Since KAi · Ri < 0`, `<https://standards.ieee.org/ieee/802.11bh/10525/>.`. The tighter rule gave up 156 pages that the looser one had scored better, 52 of them gov: the ones looked at are mostly map scale bars and figure labels, plus a few papers' superscripts (`in S′ we express S′ as`), and none of the RFCs' gains.

**Tried and rejected:** taking short raised runs along too, which joined superscripts to their lines (`Let F′ be`), added 51 better pages and 164 worse, by rearranging display equations.

**Cost**, release, this Mac, whole process, median of 7 runs, `recto local F search zzzqqqxx`, so every page is read and none drawn. Before is the build without any of this.

| | ms a page, before | ms a page, now | `recto`'s peak before | Peak now |
|---|---:|---:|---:|---:|
| `rfc9110`, 194 pages | 1.13 | 1.37 | 3.39 MiB | 4.31 MiB |
| NASA, 383 pages | 3.43 | 3.65 | 5.88 MiB | 6.53 MiB |
| `math_AG_2607.28411`, 83 pages | 1.70 | 2.05 | 3.19 MiB | 4.19 MiB |

`read_page` itself, normalising included, is 91 µs at p50 and 409 µs at p99. The rest of the rise should be asking PDFium for each box and 16 more bytes a character on the wire, which wasn't measured apart. `text` on one page is about the same (34 ms against 34, 50 against 47, 40 against 43), because it already drew the page and read the boxes for its hidden-text check.

**A hostile page of 2,000,000 characters**, by `examples/line_order_hostile.rs` under `/usr/bin/time -l`, whole process: the page itself takes 41.3 MiB.

| Shape | Time in the repair | Peak | Runs moved |
|---|---:|---:|---:|
| A line of a million into a line of a million, each landing in a gap of the first (review's shape) | 11 ms | 49.4 MiB | 0 |
| The most lines it will look at, on one baseline | 18 ms | 51.5 MiB | 0 |
| Many later lines with runs that fit | 26 ms | 66.0 MiB | 122,880 |

A line takes at most 32 runs and gives at most 64, and past 16,384 lines a page stays as it came. Before those caps, review's shape was about 5e11 comparisons by arithmetic, some 10 minutes, and the search's 30 s cap only applies between pages.
