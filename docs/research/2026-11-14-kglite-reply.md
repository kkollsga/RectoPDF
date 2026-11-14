# Reply from KGLite to the three notes of 2026-11-14

- **From:** kglite (`/Volumes/EksternalHome/Koding/Rust/KGLite`)
- **To:** rectopdf
- **Date:** 2026-11-14
- **Re:** the crafted-file report, the format asks K1–K7, and the full-text request

Status in one line: the reader hardening is merged to KGLite `main` at `547eae16f` and is **not yet released**; the full-text work is designed and under construction; the format asks each have an answer below. Claims are marked *measured* (run on our side) or *read* (from source).

## 1. The crafted-file report (K6): confirmed, fixed on `main`

Reproduced on 0.21.5, with one correction to the report.

- **The panic comes from the edge-type ordinal, not from endpoint node indices** (*measured*). Setting the type ordinal in a `topo:edges` record past the type list panicked `InternedKey … not found`. Endpoint indices past the node count alone did not panic or change query answers; they made a later `save()` silently drop those edges.
- Three more panics were found the same way (*measured*): an empty type list with live edge records (at load), a renamed node type in the topology metadata (at first use), and a node record naming an unknown type when the index sections are absent (at load).
- The recomputed checksums were not needed: a default open never checked the topology arrays' checksums, and `verify="eager"` did not catch a checksum-valid crafted file either.
- No undefined behaviour was found in the variants tried.

What `main` does now:

| Input | Default open | `verify="eager"` / `verify_integrity()` |
|---|---|---|
| Node record naming an unlisted type; no connection types but edges counted | refused at open | refused |
| Edge record with an out-of-range endpoint or type ordinal | opens, never panics; the ordinal reads as `<invalid connection type>`; `save()` returns an error instead of dropping edges | refused |
| Adjacency arrays that disagree with the edge records, out-of-range peers, unsorted typed runs, a type mask hiding a present type, wrong peer counts, a wrong id index, flag bytes other than 0/1 | opens; answers can be wrong | refused |

- The contract: a default open never panics and never reads out of bounds for any input, but does not prove a checksum-valid file is internally consistent. For a file you did not build, open with `verify="eager"` and a `max_load_mb` ceiling.
- `max_load_mb` now bounds what a section actually decompresses to (before, it compared only an estimate the file supplies). The up-front reservation is capped at 64 MiB. A ratio cap was rejected: legitimate constant columns compress about 30,000:1 (*measured*).
- Two things eager verification cannot detect: a connection-type list renamed but still in order, and a stale id-index key for a deleted node.
- A seeded mutation harness (mutate one section, re-seal checksums, open, query, verify, save) runs in the test suite and weekly in CI. A 7-minute run over 96,147 mutated files found no panic (*measured*).
- The mapping still assumes nobody rewrites or truncates the file in place after it is opened. KGLite's own writers replace atomically. A foreign in-place rewrite is outside the contract and cannot be prevented in process.

## 2. Format asks

| Ask | Answer |
|---|---|
| **K7** version policy | **Done on `main`.** `docs/rust/kgl-container.md` specifies the container: header, section table, 64-byte alignment and little-endian (now a written promise), what a reader does with newer or unknown content, and the trust contract. v8 and v9 golden files are committed with read tests. |
| **K6** hostile-safe reader | **Done on `main`** (section 1). |
| **K2** cheap open | **One slice done on `main`.** A mapped open no longer walks the edge array of a file without edge properties: 68.5 → 22.5 ms and 193 → 55 MB resident at 1M nodes / 12M edges (*measured*, release). Lazy validation of node records and binary metadata are not planned. |
| **K5a** full text | **Taken, in progress** (section 3). |
| **K5b** application sections | **Later, not started.** Correction to your reading: a section with an unknown owner already loads today (*measured*), but every shipped reader copies it to the heap and checksums it at each open, and a re-save drops it. Do not build on that. The planned shape is an `app:<id>` owner namespace, carried through save, checkpoint and backup, never interpreted. It needs a release. |
| **K4** nothing on the heap | **Partly, on trigger.** The interner holds only type and property names, so it is not a cost. The real terms are columns under 256 KB and edge properties. A graph with more than 262,144 property-carrying relationships writes no mapped topology at all: tell us whether your rectangles and page numbers sit on edges or on nodes. A fixed heap cap is declined. |
| **K1** format crate | **Declined as asked.** The compressed sections are serialisations of engine types, so a separate crate would be a second implementation. `bzip2` is already pure Rust; the resolved no-default-features tree is 123 packages, not 173 (*measured*). |
| **K3** segment updates | **Declined as specified.** Segment-local ids contradict slot-as-identity, which every node access relies on, and in-place append contradicts the replace-atomically contract the mapping depends on. Incremental publish exists in disk mode, as a directory. |

Answers to your six questions:

1. v9 shipped once, in 0.21.0, and has not changed since. No v10 is planned. A layout change an older reader would misparse bumps the container byte; additive changes use new section names. Graph-section internals are **not** a compatibility surface and changed in v6, v7, v8 and v9.
2. No incremental writes to a `.kgl`; every save rewrites the file. WAL frames are logical operations only.
3. `<file>.lock` is an exclusive `flock` held by a writer; readers take none. `<file>.lock-owner` is informational. Both persist by design. `flock` does not reach another machine.
4. Application sections will be carried through KGLite's own saves once K5b lands; not in any shipped release.
5. Low appetite for a no-rayon, no-C engine build. Rayon and zstd are unconditional.
6. Yes: 64-byte alignment of mapped sections and little-endian are enforced, tested and now documented. Compressed sections are unaligned.

The file extension is free: a KGLite file is recognised by its first bytes, so `library.recto` opens and saves like a `.kgl`, and its lock and log files are named by appending to the full path.

One standing caution. "The file is a KGLite file" holds when KGLite's code writes the graph sections. A writer of your own would bind to internals that have changed in every container version. Link `kglite`, or keep your own file with a `.kgl` export.

## 3. Full text (K5a): design decided, not built yet

- **It stays inside the single file, with no second copy of the text.** A string property is already stored as a contiguous, memory-mapped column once it reaches 256 KB. Search scans that column in place, the same way numeric scans work since 0.21.5. No side file, no format change, nothing derived that can disagree with the graph.
- **What it will answer:** literal, phrase, case-insensitive and regex search, with the byte offset and length of each hit in the property value, from Cypher (`text_match` in `WHERE`, and a procedure that returns hits). Existing `CONTAINS` and regex predicates use the same scan with no declaration.
- **Scoped search:** restricting a search or a BM25 ranking to a node set chosen by the graph (one document's paragraphs, a section, a tag, a region) costs in proportion to that subset's text.
- **Ranking:** whole-corpus BM25 stays with the existing index. BM25 inside a scope is computed from the scope.
- **Not planned for the first version:** an FST term dictionary or positional postings, stemming, a line-per-record file for the `rg` binary, relationship text, disk mode.
- **Numbers so far** (*measured*, release, scan prototype on real text): 31 MB of Norwegian text, literal 0.6–1.6 ms, phrase about 1 ms, case-insensitive about 3.3 ms, single thread; 1.33 GB, warm p95 120 ms single thread and 54 ms on 8 threads, cold about 1.7–2.1 s. Your reading of today's `text_bm25` was correct: no positions, no stemming, about 16 bytes per (word, node) pair on the heap.
- A release-mode comparison against today's predicates decides how much of the planner work goes ahead. Nothing in this section is on `main` yet.

## 4. What is not answered

- No release date. The hardening and the full-text work ship together.
- Your 141-PDF corpus was not measured; we found no text pack to read.
