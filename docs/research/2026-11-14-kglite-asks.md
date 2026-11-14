# What we would ask of KGLite: a requirements note (2026-11-14)

> This note is written to be handed to the KGLite project as it stands. It describes one use case, what it needs from the `.kgl` format and its reader, and why each need is also good for KGLite's other users. Everything said about KGLite was read from its source at commit `373d51e7d` (workspace version 0.21.5), plus a few experiments with the published Python wheel 0.21.4. Paths are under `crates/kglite/src/` unless said. Sizes of work are guesses and are labelled as such. Nothing here was built or changed in KGLite's repository.

## 1. The use case

A desktop application keeps **one index file in a user's folder, beside the user's own documents** (PDFs). It holds a graph of documents, sections and paragraphs with typed edges (contains, next, links to, cites), small properties (content digests, page numbers, rectangles), and a full-text word index.

**How the file is used:**
- **Opened often, and cheaply.** It is opened when the user or an agent searches, often from a short-lived command-line process. The first answer should come after reading a header and the few pages a query touches.
- **Updated one document at a time.** When one document in the folder changes, its part of the index is brought up to date without rewriting the whole file.
- **Possibly someone else's.** The folder may have been copied, synced or sent by someone else, so the file is **untrusted input**. Opening it must never crash the application, run away with memory, or read outside its bounds.
- **Free while idle.** An application may have several such folders open for hours and touch them rarely. While idle, an open file should hold no more than a file handle and a few hundred bytes.
- **Shared across machines, with one writer.** The folder may be shared between two machines by a sync service. One writer at a time on a machine, and a reader that never sees a half-written update.

**Scale.** 10⁵ to 10⁷ nodes. A folder of 1,000 typical PDFs is about 570,000 nodes (1,000 documents, about 30,000 sections, about 42,000 pages and about 500,000 paragraph-level blocks) and about 1.6 million edges. 10,000 PDFs are about ten times that. These are estimates from a corpus of 141 public PDFs.

**What's wanted from KGLite:** to use KGLite's file format for this file, and ideally its reader, so that the same file opens in KGLite's own tools (the MCP server, the visualiser, Cypher) with no export step.

## 2. The asks

Each ask gives what is needed and why, what KGLite does today, a suggested shape, how it would be tested, and what it gives KGLite's other users.

### K1. A small core crate for the file format alone

**Needed.** Read, write and validate `.kgl` files from Rust without the query engine, the Python layer, a thread pool, or most of the dependencies. An application that embeds the format links its reader into a long-running desktop process, where every dependency is code to audit, and a global thread pool holds its threads for the life of the process.

**Today.**
- The engine crate `kglite` carries everything: about 346,000 non-test lines, about 94,000 of them the Cypher engine, with no feature to leave Cypher out.
- `Cargo.lock` reaches about 173 package versions from `kglite` with optional features off (an upper bound: the lockfile unifies the workspace's features).
- Unconditional dependencies include `rayon`, `geo` with `multithreading`, `rstar`, `zstd` (zstd-sys, C), `bzip2`, `stacker` (C and assembly), `petgraph`, `chrono` and `csv`.
- The pool starts on demand: `parallel_digest` in `graph/io/file/mappable.rs` (about lines 645–670) uses rayon for sections of 8 MiB or more, and column loads use `par_iter` (`graph/io/file/columns.rs:30`).
- Python is already apart (`crates/kglite-py`), and network code already moved out (`kglite-datasets`): the direction exists.

**What makes it hard.** The loader builds a `DirGraph` (about 95 public fields) that the query engine, algorithms and storage modes share. The save path materialises the topology as a petgraph (`write_kgl_with_stamp_layout`, `graph/io/file.rs:1017`). Geometry, time zones and the text index are parsed at load. So the format isn't separable from the engine today.

**Suggested shape.**
- A `kglite-format` crate: the container (header, metadata, section table, checksums), typed accessors for each section kind, a writer, and `validate(strict)`.
- No `rayon` by default. Parallel digesting goes behind a feature, or the caller passes in a pool.
- Compression behind a feature, with a pure-Rust codec option.
- `kglite` depends on it.

**Tested by.** Building `kglite-format` alone in CI with a dependency budget (a count that only goes down), and a test that opening and querying a file starts no thread.

**For KGLite's users.** The Java, Node and C wrappers, and anyone who wants to read or produce `.kgl` files (exporters, viewers, other languages), get a small, auditable dependency. It is also the natural home for a written format specification (K7).

### K2. Open by the header and section table only; validate a section when it is first used

**Needed.** The first search on a cold file reads only what it needs. Opening a 10⁷-node file should cost the same as opening a 10⁵-node one.

**Today** (`load_file_inner`, `graph/io/file.rs:1433`). Opening reads far more than the header and table, even in mapped mode:
- it parses the whole JSON metadata (up to `MAX_METADATA_BYTES`, 64 MiB, `file.rs:1289`);
- it decompresses every compressed column section onto the heap;
- it rebuilds declared property and range indexes (unless `defer_index_rebuild`);
- it decodes BM25 and HNSW to the heap;
- it walks the graph to normalise legacy references and compute advisories;
- it validates every 32-byte node record (`storage/frozen/node_record.rs`) and every string byte, since readers then use `from_utf8_unchecked`.

What is already right:
- Per-section CRCs exist.
- Numeric raw sections are already checked lazily (`file.rs` about lines 122–148 explain the policy).
- Touched pages are released afterwards (`MappedFile::release_resident`, `mappable.rs` about 302–337).

The repository's own measurement: 0.24 s warm and 1.0–1.1 s cold to open a 3-million-node, 36-million-edge synthetic graph mapped (`dev-docs/bench/results/2026-10-22-topology-final.md`), and RSS of about 215 MB right after a mapped load at 1M nodes, from validation touching every record.

**Suggested shape.**
- **Binary metadata with caps.** The metadata needed at open (types, the section table, counts) becomes a fixed binary table with its own CRC. The JSON parts become sections read on demand.
- **Every section is validated on its first use,** under its CRC and its structural checks: offsets monotone and in range, UTF-8, the node-record invariants. The result of the check is remembered in a few bits.
- **`validate(Strict)`** checks everything at open, for tools and tests.
- **A string section that hasn't been checked** is read through checked UTF-8 decoding, never `from_utf8_unchecked`.

**Tested by.** A benchmark of open time and bytes read against file size, which must stay flat as the file grows. A corrupted section that is never used doesn't fail the open, and fails the first query that uses it, with an error and no panic.

**For KGLite's users.** The MCP servers start faster and use less memory: four concurrent servers on one graph are a pattern in the owner's own benchmarks (`mapped_migration_2026-10-24.md`). Very large graphs open in constant time.

### K3. Updating part of a file

**Needed.** When one document changes, append its new part and retire its old one; never rewrite a gigabyte file for a 40-page change. One writer and many readers. A reader never sees a half-written update. A crash leaves the last committed state.

**Today.**
- A `.kgl` is only ever rewritten whole: temp file, fsync, rename, directory fsync (`write_kgl_with_stamp_layout`). That is crash-safe, and a crash leaves `<name>.tmp.<pid>.<n>`, reaped by the next writer.
- Saving a mapped (frozen) graph first turns its topology back into a heap petgraph (`dev-docs/plans/mapped-topology-tier.md`).
- The repository measured saves of 1.2 s at 1M nodes and 12M edges, and 4.2 s at 3M and 36M (`dev-docs/bench/results/2026-10-21-topology-p3.md`).
- Incremental durability exists as a **second file**, `.kgl-wal`, of logical mutation frames replayed onto the loaded graph (`graph/wal.rs`), folded in by a full save. It is durable, but it needs a load-and-replay, two files, and memory for the replayed state.
- **Locks** (`graph/io/open.rs`):
  - `<file>.lock` is a `flock` token held by the writer; readers take none.
  - `<file>.lock-owner` records who holds it, for error messages.
  - `flock` is released by the OS when its process dies, which is right. It doesn't reach another machine on a synced folder.
  - Old `.lock` and `.lock-owner` files from months ago remain beside graphs on the owner's machine. That is harmless, but it says the files outlive their use.

**Suggested shape** (one design; others are welcome if they meet the same tests).
- **Two fixed header slots** at the start of the file. Each holds a generation number, the writer's id and the offset, length and CRC of the current *manifest*. A reader takes the valid slot with the highest generation. This is LMDB's two meta pages.
- **Immutable segments appended after them.** Each holds the nodes, edges, properties and index sections of one batch, in today's v9 section format, with ids local to the segment and a mapping to global ids in the manifest.
- **A manifest** lists live segments and a tombstone bitmap of retired nodes and edges.
- **Commit:** append the segment and the new manifest, fsync, write the other header slot, fsync. A crash before the header write leaves the old state and an uncommitted tail, which the next writer truncates under the lock.
- **Compaction** writes the live content to a new file and renames it over the old one, as today's save does. It runs when tombstoned data passes a set share or segments pass a set count.
- **Queries** see a merged view across segments. A type's members and an edge type's adjacency are the concatenation of the segments' sorted runs, so the number of segments must stay small (merge policy), as in Lucene's segments.
- **The lock stays `flock`** on a `.lock` file, or on the file itself. The record of who holds it can stay. Each header names its writer, so a file changed by another machine through a sync service is recognisable, and the application decides what to do (for an index derived from documents: re-derive what differs).

**The hard part.** A merged view over segments for the CSR topology, the id indexes and the property indexes, and keeping the in-memory engine's mutation model coherent with it. This is the largest ask.

**Tested by:**
- crash tests: kill the writer at every step of a commit, then open, and the last committed generation is read;
- a reader running throughout a writer's commits never sees a mix;
- results equal before and after a compaction;
- bytes written for a one-node change are proportional to the change, not the file.

**For KGLite's users.** Graph servers that take daily updates (the owner's legal and petroleum graphs are rebuilt and re-saved whole today) could append instead. It removes the save cliff at millions of nodes, and could replace the WAL sidecar for mapped graphs.

### K4. Nothing on the heap for an open graph beyond small, fixed state

**Needed.** An open, idle file costs a file handle, a mapping and a few hundred bytes, whatever its size. Many files can be open at once in one desktop process.

**Today** (mapped, frozen v9). The topology, id indexes, type indexes and large columns are mapped. The heap still holds:
- the string interner;
- the parsed metadata;
- columns under 256 KB;
- the edge-property map (`topo:edge_props` is decoded to a heap map);
- rebuilt declared indexes;
- the whole BM25 index, about 16 B per (term, node) pair (`algorithms/text_index/mod.rs`);
- HNSW.

The owner's measurements show what that adds up to: the legal graph (158,000 nodes, a 315 MB file) at 46 MB private after a mapped load, and the Sodir graph (558,000 nodes) at 123 MB (`mapped_migration_2026-10-24.md`). For a 5.7M-node index with a text index at about 30 distinct words per paragraph, the BM25 alone would be about 2.4 GB on the heap (estimate).

**Suggested shape.**
- **Every index persisted in a mapped, sorted layout** (property indexes and the interner as sorted arrays plus string sections, edge properties as columns), not rebuilt into heap maps.
- **The text index mapped** (K5).
- **A cap on what an open graph may hold on the heap,** reported by an API, so an embedding application can assert it in its own tests.

**Tested by.** A memory test: open a 10⁶-node file, run one query of each kind, drop the query's results, and measure the process's private footprint against a fixed budget, independent of the file's size.

**For KGLite's users.** The four-servers-per-graph pattern gets cheaper, and graphs larger than RAM become routine.

### K5. Text that can answer phrases, or room for the application's own index

**Needed.** Search by words and by phrase, with positions, ranked. The embedding application may prefer its own index tuned to its text (normalisation rules, hidden-text flags, positions in its own text model).

**Today.** `text_bm25` is lexical BM25 with **no positions** (`Doc { terms: Vec<(TermId, u32)> }`): no phrase queries and no stemming. It is persisted as a compressed `KGLTIDX1` section (`graph/io/file/text_index_persistence.rs`) and decoded to the heap at load. `text_score` is embeddings only (`CYPHER.md`).

**Two asks; either one serves.**
- **K5a, a mapped positional text index:** an FST term dictionary (the `fst` crate's layout maps millions of terms in a few MB and is searched in place), postings with positions, and BM25 computed from mapped statistics.
- **K5b, opaque application sections (the flexible format).** A section class owned by the embedding application:
  - typed by an application id and a section name, versioned by the application, with a CRC;
  - **carried by KGLite through its own saves and compactions, never interpreted, and ignored by KGLite's readers;**
  - so an application keeps its own index beside the graph in one file, and the file still opens in KGLite's tools.

  Today, the self-describing compressed sections (BM25 and HNSW) are already skipped when a reader doesn't recognise their payload (comments at `graph/io/file.rs` about 476–490). But an unclaimed mapped section **refuses the load**: there's a test that says so, `an_unclaimed_section_refuses_the_load` (`graph/io/file/embedding_mapped_tests.rs:433`). K5b needs an owner class that is neither: carried, checked and ignored.

**Tested by.** For K5b, a file with an application section opens in every KGLite tool, survives a KGLite save and compaction byte for byte, and fails its CRC when corrupted without stopping the graph from opening. For K5a, phrase queries against a reference implementation over a corpus.

**For KGLite's users.** K5a gives phrase search to text-heavy graphs (court decisions, documentation vaults). K5b lets any application carry its own data in a `.kgl` (thumbnails, application indexes, provenance) without forking the format.

### K6. A reader that is safe on a hostile file

**Needed.** Opening a file someone else made can't panic, can't allocate gigabytes because the file says so, and can't read out of bounds, even when the attacker recomputes every checksum (CRCs are public).

**Today.**
- Much is right: bounds, alignment, ordering and overlap of the section table are checked, string sections are checked before unchecked use, and node-record invariants are checked before the records are reinterpreted. Twelve of the seventeen crafted files below were refused cleanly.
- **No fuzzing and no property tests.** The repository's own storage review lists "fuzzing/random graph histories" and "hostile large corrupt allocations" as not yet done (`dev-docs/plans/correctness-scan-storage-2026-09-05.md:98`).
- `SECURITY.md` is a reporting policy only.
- **Caps are generous:** `MAX_CODEC_BYTES` is 2 GiB (`graph/io/file.rs:96`) and `MAX_DECOMPRESSED_SECTION_BYTES` is 16 GiB (`file.rs:1290`, `zstd_decompress_limited`). A small zstd section with a valid checksum can therefore make a reader allocate gigabytes; that is read from source and not tried.
- **Two `unsafe` patterns deserve a second look for untrusted files:**
  - mapped bytes reinterpreted as `&[NodeData]` (`storage/frozen/node_record.rs` about 169–176), sound only after every record is validated;
  - soundness that depends on nobody rewriting the file in place after validation: the SAFETY note at `graph/io/file/mappable.rs` about 254–257. A same-length rewrite by another program after validation turns validated bytes into undefined behaviour.

**A concrete report: a panic on a crafted file.**
- *Version:* the Python wheel 0.21.4, mapped storage. The panicking function is unchanged at `373d51e7d` (0.21.5).
- *How the file was made:*
  1. Build a graph of 100,000 nodes of one type, each with a string property, and 100,000 `NEXT` edges, and save it, which gives a v9 file with a frozen topology.
  2. In the `topo:edges` section (the 12-byte edge endpoint records), set some records' endpoint node indices to values at or above the node count.
  3. Recompute that section's CRC32 in the mapped-section table, and the metadata CRC.
- *What happens:* the file loads with `storage="mapped"`, and `MATCH ()-[:NEXT]->() RETURN count(*)` panics with `InternedKey 0 not found in StringInterner`. The panic is at `graph/storage/interner.rs:253`: `StringInterner::resolve` panics by design on an unknown key.
- *What it suggests:* the endpoint records aren't range-checked at open or at first use, and a key lookup on data that came from a file should return an error, not panic.
- *The rest of the run:* of the other 16 crafted variants, 12 were refused cleanly. Two loaded and answered wrongly without an error: peer ids out of range, and edge-type ordinals out of range.
- No crafted file accompanies this note; the steps above reproduce it.

**Suggested shape.**
- **Every reader path over file data returns `Result`.** No `panic!`, `unwrap` or `expect` on file-derived values, enforced by a lint on the format crate.
- **Every count, offset, length and id is range-checked before use,** including edge endpoints, type ordinals and peer ids, at first use under K2.
- **Allocation caps derive from the file's actual size** (for example, decompressed size at most k × compressed, with a small absolute cap), never from sizes the file declares.
- **Decode little-endian fields on read** (as `FrozenElement::decode` already does), rather than reinterpret bytes as Rust structs, wherever the struct has invariants.
- **A structure-aware fuzz target:** generate valid graphs, mutate their sections, recompute checksums, open, query. Run in CI briefly and continuously elsewhere.

**Tested by.** The fuzz target, the crafted cases above as regression tests, and a test that a file truncated or rewritten under a live mapping fails with an error at the next checkpoint rather than undefined behaviour (or the docs say plainly what can't be prevented).

**For KGLite's users.** Anyone opening a graph they didn't build: `kglite-datasets` caches graphs built from downloads, users share `.kgl` files, and MCP servers load files from disk.

### K7. A format version policy, written down

**Needed.** A reader meeting a newer file says so cleanly. A reader meeting an unknown optional section ignores it and says so. Both are needed for a file that moves between machines with different versions installed.

**Today.**
- A newer container magic is refused with a clear error (`newer_portable_format_error`, `graph/io/file.rs:1365`).
- A newer core data version is refused (`CURRENT_CORE_DATA_VERSION`, `file.rs:115` and 2188).
- Unknown compressed index sections are skipped.
- An unknown mapped dtype is refused with "Please upgrade kglite" (`graph/io/file/mappable.rs:826`).
- An unclaimed mapped section refuses the load.
- There's no written specification of the container.

**Suggested shape.**
- **A specification document** for the container: header, table, section kinds and dtypes, checksums, alignment, and segment and manifest rules once K3 exists.
- **A major and minor version.** A newer major is refused by name; a newer minor reads, ignoring unknown optional sections and reporting them.
- **Every section carries a "required" or "optional" flag,** so a reader knows which unknowns are safe to skip.

**Tested by.** Golden files for each released version, read by every later version, and a file with an unknown optional section of each kind.

**For KGLite's users.** Shared files across versions, and other implementations of the format.

## 3. What each ask buys

The application's requirements, against three ways to build its file: (a) KGLite's format and reader as they are; (b) KGLite's format for the graph with the application's own word index beside it in the same file; (c) the application's own file, shaped by KGLite's v9 design, plus a `.kgl` export for KGLite's tools.

| Requirement | (a) today | (b) today | (c) | Asks that make (a) or (b) meet it |
|---|---|---|---|---|
| Cold open reads the header, table and touched pages only | misses | misses | meets | K2, K4 |
| One changed document updated without rewriting the file | misses | misses | meets | K3 |
| Size: no duplicated topology; text index compact | partly (topology stored twice in v9) | partly | meets | K3's compaction writes one copy; K5 |
| Idle cost: a handle and a mapping | misses | misses for the graph part | meets | K4 (and K5a for (a)) |
| Phrase search | misses | meets (the application's index) | meets | K5a for (a) |
| One writer, readers never see a half write, crash-safe | meets for whole-file saves | the same | meets | K3 keeps it for partial updates |
| Two machines on a synced folder | `flock` per machine only, as any design | the same | the same plus writer ids | K3's writer id in the header |
| Read-only volume | meets (readers take no lock) | meets | meets | none |
| Hostile file | misses (a panic, large caps, no fuzzing) | misses for the graph part | meets (own reader, fuzzed) | K6 |
| Light in the application process | misses (~173 crates, a thread pool) | misses | meets | K1 |
| Opens in KGLite's tools without an export | meets | needs K5b | needs the export | K5b |
| Survives version skew between machines | partly | partly | meets (own policy) | K7 |

**The smallest set that lets the application's file simply *be* a KGLite file (no export): K3, K5b and K7.** That assumes the application reads and writes the format with its own code, hardened by itself:
- K7 makes the format a written, stable specification to implement against;
- K5b lets the word index live in the file as an application section that KGLite carries and ignores;
- K3 lets one document's change be appended in a form KGLite's tools also understand.

**To use KGLite's own code in the application process as well,** add K1 (a format crate light enough to link), K2 and K4 (open and idle costs), and K6 (safe on a file someone sent): all seven.

## 4. Order and size

Sizes are guesses by someone outside the project, to be replaced by the maintainer's.

| Ask | Depends on | Guess | Notes |
|---|---|---|---|
| K7 version policy and specification | | 2–3 days | Independent; first, since every other ask changes the format |
| K5b application sections | K7 (the optional flag) | 3–5 days | Independent of the rest; small |
| K6 hostile-input hardening and fuzzing | | 1–2 weeks, then ongoing | Independent; the panic fix is small |
| K2 header-only open, validation on first use | | 1–2 weeks | Much exists (lazy CRCs, per-section checks) |
| K1 format crate | K7 | 1–3 weeks | Untangling `DirGraph` and the save path from the engine is the work |
| K4 nothing on the heap | K2 | 2–4 weeks | Mapped interner, property indexes, edge properties |
| K5a mapped positional text index | K4's layout | 2–3 weeks | |
| **K3 partial update** | K7, and best after K2 and K4 | **3–6 weeks: the hard one** | A merged view over segments for topology and indexes, and coherence with the in-memory engine and the WAL |

**What the application does if only some land.** Its fallback is its own file, shaped by KGLite's v9 design, with a `.kgl` export for KGLite's tools. It builds that whatever happens, and moves onto the KGLite format only when K3, K5b and K7 are in a release. If only K5b and K7 land, the export can become a cheaper "copy the graph sections" step. If K6 lands without K1, KGLite's tools open the application's exports more safely, but nothing changes in the application.

## 5. Questions for the maintainer that would change these asks

1. **Is the v9 mapped format considered stable,** or is a v10 already planned that could absorb K2, K3 and K7 in one step?
2. **Is there work on incremental writes beyond the WAL** (appending to the `.kgl` itself, or a segment design)? If the WAL is the intended answer, could a WAL frame carry pre-built sections instead of logical mutations, so replay doesn't need the heap engine?
3. **How are the `.kgl.lock` and `.lock-owner` files meant to be used** by other processes and other tools? Is a reader meant to take a shared lock? Are stale files meant to be cleaned up?
4. **Would KGLite carry application sections through its own saves** (K5b), or only tolerate them on read? The difference decides whether a user saving a graph in a KGLite tool would drop the application's index.
5. **Is a no-`rayon`, no-C-dependency build of the format** something the project wants for its other wrappers (Java, Node, C), or only for embedders?
6. **Is the 64-byte alignment and little-endian layout of v9 sections** promised, so other implementations can map them directly?
