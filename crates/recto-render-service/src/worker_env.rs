//! **What a worker's allocator is told when it starts,** the one setting in its otherwise empty environment (a stand-in worker's test settings aside).
//!
//! `MallocLargeCache=0` turns off libmalloc's cache of freed large blocks. In libmalloc's newer allocator (xzone) the same variable also turns off deferred reclaim for tiny and small sizes (`xzone_malloc.c` sets `defer_tiny`, `defer_small` and `defer_large` false): read from its source by a reviewer, not measured here, and which allocator a worker uses on this OS isn't known. With it on, every large buffer PDFium lets go of while it draws (a decoded image or mask, an
//! image stretched to the zoom) stays in the worker, dirty and counted in its footprint, and as each zoom asks for blocks of new sizes they are seldom taken again: they
//! pile up. On the USGS map (`gov_usgs_usgs70277338`), what PDFium holds stayed at about 300 MiB from fitted to 3x while the cache grew to 197 MiB beside it.
//! Turned off, in the release app on 2026-11-16, three runs each: the map's worker peaked at 374 MiB against 477 to 480, and the tree after a zoom to about 3x was
//! 336 against 510 to 513; the shaded NASA page (`ntrs20220005969` p175) settled at 96 MiB against 213, with no zoom slower (`docs/engineering/app-memory.md`).
//! `malloc_zone_pressure_relief` released none of the cache, so the setting has to be there from the start, and only the environment can do that.
pub(crate) const WORKER_ENV: &[(&str, &str)] = &[("MallocLargeCache", "0")];
