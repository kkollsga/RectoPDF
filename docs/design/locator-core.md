# Locator core, first slice

Proposed 2026-11-17. This is the design for a bounded first slice of PLAN 8.1,
not a claim that 8.1 or reference-bearing search is finished. Implementation
waits for the coordinator's CI review. The technical choices below were
settled by the coordinator on 2026-11-17; the design is reported to the owner
before code starts.

## Existing decisions and definitions

- The owner chose the locator first, redaction and Remove Hidden Data alongside,
  and the sidebar afterward (OWNER-RULES, 2026-11-15).
- The locator uses `recto-page/1`; its exact definition is
  `recto-core/src/page_id.rs`, not the older research sketch. Existing image
  prefix and frame-tag limitations remain; a locator adds no stronger page
  identity claim.
- Page positions are one-based outside the core, while the existing page reader
  takes zero-based indices. A reference names a source-file page; an edited
  tab's `PageSlot` conversion belongs to the session edge.
- Coordinates are the existing shown page frame: crop applied, the file's own
  turn applied, points from top left, y down. Unsaved view turns are not another
  stored coordinate system. Existing `nav::Frame::to_shown`, `thousandths` and
  `fraction_down` remain the conversion definitions.
- A token grants no authority, contains no path or quote, and never discovers
  a file. The executor must supply only documents the actor can already read.
- The `.recto`/`.kgl` choice is decided; it has no implementation in this slice.

## Proposed first implementation

One public `locator` module, registered once in `recto-core/src/lib.rs`, with
private fields and validated constructors. Types own no heap allocation. Token
encoding returns one bounded string; decoding works on borrowed input and fixed
fields. No I/O, renderer, thread, timer, document cache or per-tab field is added.

The first slice supports a page and either a character range or a point/box on
that page. Document-only, contents, block and multi-page ranges wait for their
own consumers. Document tags are supplied by the caller; choosing/reading a
first informative page is not smuggled into token parsing. Document-tag
discovery and actual file scanning are later slices, since the research's
first-non-blank heuristic was explicitly unmeasured.

A page reference stores its hinted page number, a prefix of the existing
`PageIdentity.digest`, and `PageIdentity.frame`. It does not repeat the PDF
parser or digest computation. Character ranges are page-relative Unicode
character offsets, start included and end excluded, never byte offsets. The
text check is supplied from the one normalized-text producer; this core does
not introduce its own lowercase/hyphen/whitespace normalizer.

Spot conversion validates finite coordinates against the actual page before
rounding to the token's whole-point representation; it rejects negative
coordinates, invalid frames, inverted/empty PDF or shown boxes, and boxes
extending beyond the supplied page. At a fractional right/bottom edge, nearest
rounding is capped at the floor of that actual bound, so a valid A4 foot at
841.89 points becomes 841 rather than an invalid 842. This is inward endpoint
quantization of less than one point, never clamping invalid input into validity.
A positive box collapsing under quantization is refused. The coordinator settled
this edge policy on 2026-11-18 during the independent read. The token
parser itself can check only the global coordinate/extent limit: page dimensions
are not in the token. A target must be checked against the actual matching frame
before it is used. The thousandths conversion delegates the existing core
rounding. No conversion silently clamps an invalid token into a different place.

## Technical choices settled 2026-11-17

### Check length and grammar

The research's readable dotted form is preferable to an opaque packed form,
but its proposed 20-bit page/text checks are too short to call a re-found
range verified: the report itself estimates about one wrong text re-find in
700 at its search budget. Settled: keep the readable form and 128-byte
ceiling, with 64-bit document tags, a 128-bit page prefix (as the journal keeps),
a 64-bit text check and the existing 32-bit frame tag. Document/text tags are
candidate identifiers, never authority or proof by themselves. Typical tokens grow to
roughly 70–105 characters rather than the research's roughly 40. Exact lengths
will be asserted from the codec, not reported as measurements from this design.

Alternative: retain compact checks only as candidate hints, with no verified
resolution without additional evidence. That shifts complexity to every later
consumer and is not recommended. The stronger checks also permit dictionary
testing of candidate text; no digest is promised to conceal low-entropy words.
The token contains no plaintext, not a secrecy guarantee.

The grammar for this first codec is:

```text
r1.<content-tag>[-<id-tag>].p<page>-<page-check>-<frame>
    [ .c<start>-<end>-<text-check> | .x<x>,<y>[,<width>,<height>] ]
```

Whitespace above is only for presentation and never accepted. Tags/checks
use canonical lowercase Crockford base32 of fixed byte arrays with canonical
unused bits. Numbers are canonical decimal without a sign or leading zeroes,
except the single zero where zero is allowed. No escaping, aliases, mixed case,
padding, repeated parts, unknown parts or trailing bytes are accepted. Pages
are 1–200,000; character offsets 0–2^21 and start < end; coordinates and their
extents are at most 14,400 points. Input is checked for length and ASCII before
its parts are read. Overflow refuses the whole token. Unsupported versions
are distinguished from malformed tokens without accepting their payload.

### Duplicate pages and incomplete reads

The research suggests the nearest of several identical pages. Recommendation:
an original hinted slot can be found directly only against a verified unchanged
original version. After a changed version, a matching hinted slot does not
disambiguate repeated pages. A unique candidate may be moved only after a
complete scan (or strong stable-object/context evidence in a later resolver);
several are ambiguous, and an incomplete scan is incomplete, never gone or a
false unique match. No nearest-page guess. Geometry-specific targets also
require the same frame tag: matching drawing content under a changed crop/turn
does not silently place the same rectangle on a different frame.

No resolver is implemented in this first slice. Its fixed-size references and
geometry/codec are enough independent work, without pretending that character
offsets were checked merely because the page matched. A later resolution slice
will take complete/incomplete scan evidence, document authorization and text
verification, supply work/byte budgets, and map inaccessible documents to the
same not-found answer as nonexistent ones.


### Concrete types and examples

`DocumentTags` holds `[u8; 8]` content plus optional `[u8; 8]` ID. `PageRef`
holds a validated `u32` one-based hint, `[u8; 16]` copied from the existing
page digest, and its existing `u32` frame tag. `CharacterRange` holds bounded
`u32` start/end and `[u8; 8]` candidate text check. `Spot` holds whole-point
`u16` x/y and either no extent (a point) or positive `u16` width/height (a box).
`Target` is Page, Characters or Spot. `Locator` combines document, page and
target with private fields. The compiler size guard measures Locator at 64 bytes, DocumentTags at 17,
PageRef at 24, CharacterRange at 16 and Spot at 10 (2026-11-18, this Mac); each
guard is at that exact number. No reference contains a path,
owned text or an unbounded collection. No type is added to tab state.

The encoder writes a bounded ASCII string; the decoder accepts `&str`, rejects
non-ASCII, and returns fixed fields with no intermediate collection. Base32
encodes bytes in their given order, high bits first, padded only with unused
zero bits in the final digit; frame-tag bytes are big endian. It never accepts
Crockford's aliases for ambiguous letters. Document-tag algorithms, checks of
actual normalized text, and a future short link hint are not defined by simply
choosing byte widths in this codec.

These are illustrative values, not references to a real document (13-digit
content/ID/text tags, 26-digit page check, 7-digit frame tag):

```text
r1.040g2081040g2-081040g208104.p42-0c1g60r30c1g60r30c1g60r30c-28t5cy0
r1.040g2081040g2-081040g208104.p42-0c1g60r30c1g60r30c1g60r30c-28t5cy0.c1203-1219-0g2081040g208
r1.040g2081040g2-081040g208104.p42-0c1g60r30c1g60r30c1g60r30c-28t5cy0.x72,350,200,90
```

The examples have 69, 94 and 84 ASCII bytes respectively, calculated from the
grammar. The largest allowed character-range token is 104 bytes. The 128-byte
input ceiling is still checked before parsing. Adding a new grain later does
not mean the decoder accepts it today; an unknown part is refused whole.

### Link compatibility

No external URL grammar is frozen in this first slice. The full stronger token
does not fit the historical Acrobat 32-character parameter rule. The research
offered a shorter link-only hint; compatibility remains unverified. Plain page
parameters, a short hint, and an exact RectoPDF reference must be distinguished
before 8.2/8.4, rather than making an early token length force a false promise
about other viewers.

## Verification

Tests exercise canonical round trips, maximum values and limit plus one,
truncation/trailing data, overflowing decimals, noncanonical base32, malformed
UTF-8 represented at the byte parsing boundary, frame/extent rejection, all
four crop/rotation conversions, exact thousandths rounding. Reference construction is checked against the existing
real page identity from a small made PDF, not a second hash implementation.
Duplicate/incomplete resolution tests belong to the later resolver slice. Each test names the
failure it catches. No timing sleeps or app launches.

Guards assert fixed type sizes and that no thread/handle/per-tab state exists
by construction; allocation behavior is measured if the chosen test seam can
do so cheaply. The coordinator approved the minimal fuzz-registration boundary extension:
`recto-fuzz/src/targets/locator.rs`, its target/seed/smoke registrations, the
libFuzzer wrapper and manifest entry, and `tools/fuzz.sh`'s target/max-length
rows. The target reads at most 129 bytes, checks canonical round trips and
includes raw invalid UTF-8, non-ASCII, overflow and exact cap-plus-one seeds.
The bounded seeded smoke is part of the normal suite; no long campaign has
yet been claimed.

Named tests first; quick check after implementation settles; repeated new
tests under the existing CI-like harness with load recorded; independent
review and fixes before one committed-tree full check, authorized by the
coordinator. No screenshot or input-to-frame run is needed for a module with
no app/session/host call site.

## Evidence while implementing (2026-11-18)

- Nine new pure core tests and one bounded fuzz smoke pass. Latest focused run:
  10 tests, 0.019 seconds nextest wall, one run at ordinary shared-machine load;
  this is not a CI timing measurement. The independent
  source read is closed with no remaining substantive blocker after the two
  geometry corrections below. All ten tests passed all 20 CI-like rounds; load
  before was 3.15/4.36/4.19 and after 4.78/4.82/4.39. This is local process-limited
  evidence, not a claim about CI wall time. Full checks await the coordinator's
  gate decision.
- Seeded defects were restored after their named tests failed: ignored nonzero
  base32 padding; checking a point only after rounding (accepting raw out-of-page
  input); copying the wrong 16-byte half of the existing page identity.
- The first independent read found two new-code geometry faults. Each assertion
  failed before its fix: inverted PDF boxes normalized by `Frame::to_shown`, and
  valid fractional A4 corners rejected after nearest-point rounding. Both now
  follow the strict/inward rules above, without changing `nav::Frame`.
- The first quick gate passed compilation/clippy and 243 of 244 selected tests;
  the existing `a_saved_file_keeps_its_attributes` test failed on an in-place
  save's quarantine value (expected `0083;5f000000;Safari;`, observed
  `0083;6afcde92;;`). The coordinator owns that separate investigation; no save
  code or existing save test was changed on this branch, and the failure was
  not retried. The complete output is retained outside the worktree.

The token core does not yet resolve references or make an external link. No
finished agent retrieval workflow, runtime memory result or new benchmark is
claimed. It adds no action which starts a process/thread, so click-to-work and
input-to-frame measurements have no changed path in this slice.

## The resolver exists (8.2, part a)

`recto_core::locator::resolve` now answers where a reference's page is in a file already open: found, moved, ambiguous, gone or incomplete, with the
document tags beside the status as information only, and `reference` and `document_tags` make a reference from a page. Two decisions deviate from or
add to the text above. First, a hinted page whose digest prefix equals the reference's is answered `Found` at the hint with no scan, whatever the
file's version, where "Duplicate pages and incomplete reads" asked for a verified unchanged original: the token carries no version stamp, so that rule
would cost a scan of the file on every resolve (rule 1), and identical page content is retrieved identically. Every other case needs a scan of every
page within caps (1 GiB of reads since 2026-11-25, as a stream is charged again on every page that draws it; 200,000 pages, the page digest's own caps): none matching is `Gone` only after a complete scan, one is `Moved`, several are
`Ambiguous` (at most 8 listed), and a cap or a page with no identity makes it `Incomplete` with at most one candidate; never a nearest page. A frame
tag that differs still finds the page, and a spot is then not placed. Second, the document tags are fixed: the id tag is the first 8 bytes of the existing
`recto-id/1` hash of `/ID[0]` (no second hash), and the content tag is the first 8 bytes of SHA-256 over `recto-doc-c/1`, a NUL and the whole
`recto-page/1` digest of the first page with an identity among the first 16. Documents that open on the same page share a content tag, a known limit
that a later library step must answer as ambiguity. Nothing is kept between calls: no state, cache, thread or event.

## Character check, settled 2026-11-25

r1's `.c<start>-<end>-<check>` field had no fixed check algorithm and no `.c` token was ever issued, so its meaning is fixed now without a new version: the check is the first 8 bytes of SHA-256 of `recto-text/1` and a NUL, then the UTF-8 of characters `[start, end)` (Unicode scalar values, the end excluded) of the page's text as `recto text` gives it (`untrusted.text`, from `recto_index::read_page`). The core (`locator/text.rs`: `text_check`, `characters`, `check_characters`, `TextMatch`) hashes the text it is given, never normalises, and walks no further than the range's end. A range is checked at its own offsets only: a page with the same digest but other text at those offsets is `text_changed`, never searched, since a changed reading means the offsets can't be vouched for. `resolve` in the core stays lopdf-only (`Placed::CharactersNotChecked`); the executor reads the text of a found page and does the check, so one function serves `resolve` and `--ref`. Any later change is `recto-text/2` with token `r2`.
