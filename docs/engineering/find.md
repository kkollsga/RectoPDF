# Find

> Part of the engineering notes: [`README.md`](README.md).

**One query, in the background, by `search`'s reading.** `recto_agent::find` runs a phrase over a plan of the document being read, page by page, from its corpus if there's one and through one worker if not, asking for a corpus for next time, so Find finds what `search` finds (`find_lists_what_search_finds_in_order_and_steps_from_the_reader`). It keeps at most 1,000 hits, counts the rest, and stops at 60 s. `recto_control::finding::Finder` holds the one query's hits: a new query stops the last between two pages, and its one thread exists only while a query runs, so a session at rest has none. Hits are places as characters of the page's text, which a selection counts too, so going to one is a selection (`Reader::reveal_range`), brought a third of the way down the view once its page's text says where it is. The app's sidebar reads the Finder's hits under a short lock and never the session's; every step goes through the session thread to `SessionHost::find_*`, the same calls as `recto find`.

**Measured** 2026-11-08, release `recto` (`recto serve`, `recto find --json`'s `first_hit_ms` and `took_ms`), this Mac with other agents building (load 6 to 8), so under load. The session's footprint by `recto budget`.

| | Pages | Page by page, first hit | Page by page, whole | From the index, first hit / whole | Footprint, open → after three queries |
|---|---:|---:|---:|---:|---:|
| NASA, `lunar` (one hit, on p. 194) | 383 | 948 ms | 1,619 ms | 1.4 / 2.3 ms | |
| NASA, `the` (6,113 hits) | 383 | 10 ms | 1,196 ms | 0.4 / 8.8 ms | |
| NASA, `e` (58,236 hits, 1,000 kept) | 383 | 9.8 ms | 1,318 ms | 0.4 / 6.3 ms | 5.36 → 7.28 MiB, 7.36 after the index's |
| USGS map, `the` | 1 (330 MiB) | 276 ms | 276 ms | 0.4 / 0.4 ms | 2.91 → 3.78 MiB, 3.91 after the index's |

Threads: 1 at rest; 2 while the index was building after the first query, the indexer's; back to 1 once it was done, so no thread stays for Find. **Two workers at once on a first query:** the query reads its pages through a worker of its own while the corpus it asked for is built by the indexer's, in that helper's process, so they can't share one; the indexer's is at utility under half a reader's limit, and Find's goes when the query does. Later queries read the corpus and start none. Hidden text: a page with hits is checked as `search` checks one, from its corpus or by drawing it, for at most 10 s a query; past that its hits are listed as not hidden, unchecked. Not measured: a worker's peak during a query (it's `search`'s worker, under a reader's limits), the made 10,000-page PDF, and the app's own footprint with Find open (the app's QA runs show no stall over 1 s while finding).
