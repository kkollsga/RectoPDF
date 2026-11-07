# RectoPDF: platform, architecture and budgets

Research track report, 2026-11-06. Covers R5, R6, R9, R10, R13, R14 of `docs/agent-instructions/rust_app_development.md`, plus the crate graph, the
threads and ownership diagram and the `size_of` list. Measured on the dev machine: Mac mini M4, 16 GB, macOS 27.0.1
(26A434, "Golden Gate"), Xcode 27.2, Swift 6.4.2, SDK 27.2.

Tags used below: **[M]** measured here today, **[D]** stated by a primary source (numbered in §12), **[I]** my inference,
**[DECIDE]** only the owner can answer (collected in §11).

## 0. What this comes to

1. **Recommendation: native AppKit + Rust core over UniFFI, the same split as Seedwell, with WinUI 3 later.** A Rust GPU
   toolkit costs +105 to +184 MiB for its window swapchain alone [M] and gives up the native sidebar, screen reader and IME
   for free. A webview starts at 56.7 MiB across four processes for a blank page [M].
2. **Page surface: a tree of `CALayer`s whose contents are IOSurface tiles,** not a Metal swapchain. Renderer pixels land
   directly in the surface and the compositor reads the same memory: zero copies, even across processes [M].
3. **The biggest finding: `IOSurfaceSetPurgeable(volatile)` removes tile memory from `phys_footprint` at once and gets it
   back instantly.** 256 MiB of detached tiles fell from 271 MiB to 15 MiB of footprint, and all 256 surfaces survived [M].
   That is "let the OS cache for you" (guide §3.2) applied to pixels, and it is what makes the idle-tab budget reachable.
4. **The key budget: one more idle tab costs at most 4 KiB.** Reachable only with no file handle, no layer, no tile, no
   thread per tab. An AppKit sidebar row costs about 220 B; a SwiftUI row about 2.4 KB, most of the budget [M].
5. **Keep SwiftUI out of the main window.** Linking it costs nothing [M], but once a window has used it, about 29 MiB stays
   after closing against 18 MiB for AppKit [M]. Same result as Seedwell's §3.3, now with numbers for this app's window.
6. **Renderer in a sandboxed XPC service from day one.** Measured with a C service: 2.2 MiB floor, 16–21 ms warm spawn, 0.12 ms
   per 512 px tile including IPC, the file reachable only through a passed descriptor, restart after a crash in 15–17 ms [M].
   Rust renderer memory on top is still unknown.
7. **The Mac App Store does not fit the agent requirement.** A sandboxed child could not even see Homebrew's `rg` or the
   user's `claude` [M], and guidelines 2.4.5 and 2.5.2 forbid the workarounds [D]. Ship Developer ID + notarization like Seedwell.
8. **Do not use PDFKit as the viewer.** `PDFView.document =` blocked the main thread 3.1–4.9 s on a 10,000-page file [M].

Not proven yet: anything on a real on-screen compositor. All windows here were off-screen by rule (quiet runs), and this
Mac's display currently reports 60 Hz, so 120 Hz needs the MacBook. See §10.

---

## 1. Measured floors [M]

Method: small `swiftc -O` probes, one binary per framework set so linked frameworks can't distort the floors; accessory
activation policy; windows at (-40000, -40000), never key, never activated; no input of any kind. `phys_footprint` from
`task_info(TASK_VM_INFO)` inside and `proc_pid_rusage` outside. Other builds were running on the machine: footprint is
unaffected, timings are if anything pessimistic. The bare, window and sidebar rows and the SwiftUI sidebar rows ran twice; the
tile, Metal, window-count and close-cycle rows ran once; others say how many.

### 1.1 Framework floors

| Case | `phys_footprint` |
|---|---|
| AppKit, `NSApplication` running, no window | **7.3–7.7 MiB** |
| Same binary shape with SwiftUI linked, not used | 7.4 MiB (linking is free) |
| One titled window, default colour space | **15.5 MiB** |
| One titled window, pinned to sRGB | **14.2 MiB** (saves 1.2–1.4 MiB) |
| Each further window (default / sRGB) | ~0.63 / ~0.59 MiB |
| AppKit `NSSplitViewController` sidebar + `NSOutlineView` source list, 10 rows | **17.5–17.6 MiB** |
| Same, 200 / 5,000 rows | 17.5 / 18.55 MiB, so about **220 B per row** |
| SwiftUI `NavigationSplitView` + `List`, 10 rows | **27.9 MiB** |
| Same, 200 / 5,000 rows | 29.5 / 41 MiB, so about **2.4 KB per row** (plus an AppKit reentrancy warning at 5,000) |
| AppKit sidebar window, 5 open/close cycles, then idle | 7.2 → **18.1 MiB, not returned** |
| SwiftUI sidebar window, 3 open/close cycles, then idle | 7.1 → **29.3 MiB, not returned** |
| PDFKit `PDFView`, 1-page PDF, settled | 60–90 MiB |
| PDFKit `PDFView`, 10,000 pages (31 MB text and 2 GB files), settled | 118–145 MiB |
| PDFKit: holding one more idle `PDFDocument` | 0.6 MiB (1 page), 2.3–3.4 MiB (10k pages) |
| Preview, 1-page PDF (`open -g -j -F`, hidden launch), 3 runs | **98–108 MiB** |
| Preview, 10,000-page PDF, 2 runs | 28.8 MiB (hidden launch: likely nothing drawn yet, so a poor floor) |
| `WKWebView`, blank page: host + GPU + WebContent + Networking | 24.2 + 13.9 + 12.9 + 5.7 = **56.7 MiB** (once) |
| Electron 45 hello world | Not measured: running the downloaded binary was refused by this session's permission rules |

The sRGB row reproduces the ColorSync trap from guide §3.3 on this display.

### 1.2 Startup and idle

| Case | Value |
|---|---|
| `exec` → `applicationDidFinishLaunching` (AppKit) | 54–66 ms (7 runs) |
| `exec` → sRGB sidebar window built, displayed, transaction flushed | **113–133 ms** (7 runs) |
| `PDFDocument(url:)`, 10k pages, 31 MB and 2 GB files | 5.5–6.7 ms (lazy) |
| `PDFView.document = doc`, 10k pages, **on the main thread** | **3,096–4,911 ms** |
| Idle wake-ups, `ri_interrupt_wkups`/s over 10 s: bare / sRGB window / sidebar | 0.2 / 0.3 / 0.5 |
| Same: PDFKit view idle / Preview idle (3 s windows) | 0.7 / 0.33–2.33 |

Reading a 776 MiB file, every page: `mmap` raised footprint from 1.16 to 1.55 MiB (resident rose to 777 MiB, which is clean
and reclaimable); `pread` with a 1 MiB buffer ended at 2.17 MiB [M]. So file size need not appear in any memory budget.

### 1.3 Tile surfaces

| Case | Result |
|---|---|
| 512² BGRA IOSurface allocation | 2,048 bytes per row, 1,048,576 bytes; 1,025 KiB per tile in footprint |
| 256² BGRA / 1024² BGRA | 257 KiB / 4,102 KiB per tile |
| 512² `L008` (one byte per pixel) | 257 KiB, a quarter of BGRA |
| 512² `RGhA` (half float) | 2,050 KiB, double BGRA |
| Creating one surface | 16–40 µs, footprint untouched until written |
| Filling 1 MiB (with first-touch faults) | 65–104 µs |
| `CALayer` per tile, on top of the surface | 2.3–6 KiB |
| Main-thread `CATransaction` + flush, moving 256 layers one step | p50 25 µs, p99 81 µs (client side, off-screen) |
| Main-thread transaction swapping one tile's contents, 256 layers | p50 45 µs, p99 121 µs (client side, off-screen) |
| 256 tiles of 512²: detached → volatile → non-volatile again | 271.4 → **15.4** → 271.4 MiB, 256/256 survived, no memory pressure |
| `CAMetalLayer` 3024×1964, BGRA8, 3 drawables, 12 presents | **+105 MiB** over the window (one drawable is 22.7 MiB) |
| `CAMetalLayer` 5120×2880, same | **+184 MiB** (one drawable is 56.3 MiB) |
| `MTLCreateSystemDefaultDevice` + command queue | +0.2 MiB |

### 1.4 In-app capture of a tile (the QA channel depends on this)

`CALayer.render(in:)` and `NSView.cacheDisplay` both read IOSurface-backed layer contents correctly: BGRA, `RGBA` and `L008`
surfaces all came out with the right values [M]. `CARenderer` to a Metal texture also returned the right pixel once. So
page tiles can be captured in-process with no Screen Recording permission. Seedwell's vibrancy trap remains
(`QAShots.swift`: vibrant sidebar layers come out black under `cacheDisplay`), and its fix can be reused as it is.

Colour tags work in that path: red `BGRA` tagged Display P3, drawn to a P3 context, stayed (255,0,0); untagged or sRGB-tagged
red became (234,51,35), the correct sRGB-in-P3 value [M]. Tag with `kIOSurfaceColorSpace` from `CGColorSpaceCopyPropertyList`
(SDK header).

### 1.5 App Sandbox, child processes [M]

An ad hoc signed app with only `com.apple.security.app-sandbox`:

| Child | Result |
|---|---|
| `/usr/bin/true` | Runs |
| `/opt/homebrew/bin/rg` | Launch fails: "file doesn't exist" |
| `/bin/sh -c rg` | `/opt/homebrew/bin/rg: Operation not permitted` |
| `~/.local/bin/claude` | Launch fails: "file doesn't exist" |
| Embedded C helper, sandbox + `inherit` entitlements | Runs inside the parent's sandbox; `open()` outside it is `EPERM` |
| Embedded C helper, no entitlements | Same: runs, inherits the container, `EPERM` |
| Embedded C helper, sandbox entitlement without `inherit` | Crashes (exit by signal 5) |
| Embedded Homebrew `rg` copy | dyld fails: `libpcre2` from `/opt/homebrew` not readable, so a helper must be self-contained |
| Direct read of a file outside the container | Denied, also from the parent |
| Helper copied into the container and run | **Inconclusive.** Apple says programs in the container can run [D]; my probe got "doesn't exist" and did not separate a copy failure from an exec denial |

Container socket path on this home: `/Volumes/EksternalHome/KristianEX/Library/Containers/<id>/Data/tmp/control.sock` was
104 bytes for the spike's bundle id, over the 103 limit. `io.github.kkollsga.RectoPDF` would be 102. Another reason not to sandbox.

### 1.6 Sandboxed XPC renderer spike [M]

C service, ad hoc signed, sandboxed, no extra entitlements. Three runs.

| Case | Result |
|---|---|
| Spawn + first reply | 109 ms first ever (signature scan), then **16–21 ms** |
| Service footprint | **2.17–2.20 MiB** |
| Empty round trip | p50 14–23 µs, p99 34–54 µs |
| Service creates and fills a 512² tile, returns it, host looks it up | **p50 0.11–0.13 ms**, p99 0.21–0.46 ms |
| Pixels verified in the host | 64/64, same memory, no copy |
| Footprint with 64 live surfaces | service 67.0 MiB, host 4.1 MiB: **charged to the creator** |
| File access | Passed descriptor works; `open()` by path inside the service is `EPERM` |
| Service aborts | Host gets `XPC_ERROR_CONNECTION_INTERRUPTED`; next message relaunches, reply in **15–17 ms** |

Budgets must therefore be measured over the process tree. The host-creates-the-surface direction, which would keep tiles
charged to the host and make them survive a renderer crash, was not tested (spike S4).

---

## 2. R6: architecture alternatives

### 2.1 (a) Native AppKit + Rust core (UniFFI), WinUI 3 later: **recommended**

The same shape as Seedwell (`SPEC.md` §3, `docs/CROSS-PLATFORM.md`), reusing its layout: `crates/`, `app/` built with SwiftPM and
`build.sh`, `app-windows/`, `tools/check.sh`, `tools/sysguard/heavy.sh`, `app/qa/`.

- **Sidebar and tabs:** `NSSplitViewItem(sidebarWithViewController:)` with a source-list `NSOutlineView`. The active tab's
  row is expandable and its children are the TOC. "Tapping the active PDF collapses or expands the TOC" is a single core
  method, `Sidebar::tap(tab_id)`, that returns a model; the host only draws it. Glass comes from the SDK on macOS 26+,
  and on 15 the same code draws the older sidebar [D].
- **Text input, screen reader, IME, RTL:** free in the sidebar and in find. Form fields and notes use a real `NSTextField`
  placed over the page. The page itself needs an `NSAccessibilityElement` tree from Rust whichever option wins.
- **Memory:** 17.6 MiB floor with the window open [M]; window-closed floor 18.1 MiB [M].
- **Windows:** a second host (WinUI 3 / C# over uniffi-bindgen-cs), about 5k lines in Seedwell's case. Cost: two hosts forever.

UniFFI stays at **0.31**: uniffi 0.32.2 is out (2026-09-23), but uniffi-bindgen-cs's newest release, v0.11.1 of 2026-10-09,
is built for 0.31.0 [D]. Pixels never cross UniFFI, and neither do per-frame callbacks: the host pulls a batch of ready
tiles on its display tick.

"SwiftUI where cheap": from the numbers, it is not cheap in anything that stays open. Allow it only in a rare window such as
Settings, knowing the first open leaves about 11 MiB behind for the process's life. Or write those windows in AppKit too; the
price is a little more code. Recommendation: AppKit everywhere until a window proves too costly to write.

Also rejected within (a): **PDFKit as the viewer** (main-thread stall, 60–145 MiB, no custom cache, no Windows). **Pure-Rust AppKit via `objc2`**
(guide §1.2: tables too hard). **`CATiledLayer`**: Core Animation owns the threads and the backing store, so there is no
cancel, no volatility, no cross-process surface, and the contents property can't be set without disabling it [D].

### 2.2 (b) A Rust GPU-UI toolkit: rejected

State on 2026-11-06, from each project's repository [D]:

| Toolkit | Latest | Stated maturity | Screen reader | IME | RTL | Native feel |
|---|---|---|---|---|---|---|
| **GPUI** | crate 0.2.2 (2026-10-30), Zed 1.27 | README: "still pre-1.0... often breaking changes" | AccessKit added 2026-05-27 (#56065); gpui-kit still has open gaps (#3698 label invisible to screen readers, #3437 TextView missing from the macOS tree, both Oct 2026) | Not verified here | Open: #31102 (2025), #7465 (2024) | Own drawing, Zed's look |
| **Slint** | 1.18.1 (2026-09-21) | Stable, commercial backing | AccessKit, active macOS fixes in changelog | Changelog: "treat text fields like native ones" | Elide fix for mixed direction in changelog; depth not verified | Own widgets; GPLv3, or royalty-free with `AboutSlint` or a web badge, or commercial |
| **iced** | 0.14.0 (2025-12-07) | README: "currently experimental software" | Open since 2020 (#552); PR #1849 open since 2023 | Added in 0.14 (#2777) | Not verified here | Own look |
| **egui** | 0.37.0 (2026-10-15) | README: "If you want a GUI that looks native, egui is not for you" | AccessKit | Open bugs (#8813, Nov 2026; #3532) | PRs still open in Nov 2026 (#8818, #8815, #8822, #8828) | Explicitly not |
| **Xilem / Masonry** | 0.4.0 (2025-10-29) | README: "experimental" | AccessKit | Work in progress (#1692 open) | Parley stack; depth not verified | Own look |
| **Makepad** | widgets 1.0.0 (2025-05-13), repo active | Young, about 1.6k downloads in 90 days | Open since 2023 (#196) | macOS fixes Sep 2026 (#1223, #1227) | Not verified here | Own look |

Floem (last release 2024-11) and Dioxus native (0.8 alpha) were not taken further.

Judged against this app:

- **Memory floor.** A full-window `CAMetalLayer` costs +105 MiB on a 14-inch MacBook Pro and +184 MiB at 5K, before any tile [M].
  GPUI's `crates/gpui_apple/src/metal_renderer.rs:214` sets `setMaximumDrawableCount(3)` [D]. Apple permits 2 or 3 [D]; 2 would
  save about a third. Any toolkit drawing a full window this way pays the same. Not built and measured per toolkit (no heavy
  Rust builds in this track): spike S7 if anyone wants to reopen this.
- **UI thread.** UI and page share one frame loop. A stall in UI work stops the scroll too.
- **Native.** No toolkit gets real Liquid Glass, native sidebars or `NSTextField`. "Simplistic, beautiful, modern" would need
  to be drawn and kept current by hand with each macOS.
- **Stability.** Four of six say in writing they are experimental or pre-1.0, and every update could touch the whole UI.
- **What it would buy:** one UI codebase on Mac and Windows. That is real, and it is the strongest argument. If the owner
  makes Windows equal priority at launch, reconsider with S7 before signing off.

### 2.3 (c) Tauri / Electron: the reference, rejected

- **Memory:** 56.7 MiB for a blank `WKWebView` in four processes [M], which is Tauri's floor on macOS (wry uses the system
  webview [D]). Three times the AppKit sidebar window. Electron bundles Chromium and adds more; not measured (§1.1), and
  the owner's stated 50–150 MB (guide §3.2) stands. WebView2 has the same multi-process shape [D].
- **Tiles:** pixels must get from Rust into a canvas or WebGL texture in another process, so at least one copy per tile, with no
  surface shared with the compositor. Memory is also held per-origin in the web process, out of our cache's control.
- **Native feel:** an imitation sidebar, an imitation of glass.
- **Budgets:** one more idle tab would be hard to keep at 4 KiB once a DOM row is involved [I].

### 2.4 Scorecard

| | (a) AppKit + Rust | (b) GPU toolkit | (c) Webview |
|---|---|---|---|
| Window floor | 17.6 MiB [M] | 17+ plus swapchain 105–184 MiB [M, assuming full window, 3 drawables] | 56.7 MiB [M] |
| Copies per tile on Mac | 0 | 0–1, toolkit dependent | ≥ 1 |
| Scroll survives a stalled main thread | Partly: responsive scrolling [D], S1 | No | Compositor thread, tile fill not |
| Sidebar row | 220 B [M] | Toolkit | DOM |
| Screen reader, IME, RTL in chrome | Free | Partial | Free in web content |
| Liquid Glass | Free | No | No |
| Windows work | A second host | Small | Small |
| Risk | Two hosts | Toolkit churn, a11y gaps | Budgets |

### 2.5 The page surface in each option

**Tile basics, any host.** 512×512 device pixels: 1 MiB in BGRA, 256 KiB in `L008` [M]. 256 px tiles quadruple the layer
count for no memory gain beyond better blank-skipping, and 1024 px makes each re-render and each partial miss expensive. Three
savings, in order of value:

- a *blank* tile (one solid colour, reported by the renderer) gets a layer `backgroundColor` and no surface at all;
- a *grey* tile uses `L008` at a quarter of the memory (correct in-process [M]; on-screen is spike S2);
- anything off-screen goes volatile instead of being freed [M].

**macOS: how pixels reach the screen.** The renderer writes straight into an IOSurface, the host sets `layer.contents = surface`
(documented for `IOSurfaceRef` in the SDK's `CALayer.h` [D]), and the compositor maps the same memory. No copy in our process
and none to the GPU on unified memory [I]. `CVPixelBuffer` adds nothing. A `CAMetalLayer` with textures made from the same
surfaces also has zero upload, but pays for 3 window-sized drawables [M]. Keep that as the fallback for one purpose: a
page effect `CALayer` can't do.

**Threads and flow.**

- Each scroll event or display tick, main publishes `Viewport { tab, generation, rect, scale }` with one atomic swap and unparks the
  scheduler. This is a sync FFI call that does no other work.
- The scheduler discards anything outside the new viewport and orders jobs by distance from it.
- Workers render into surfaces from the pool. A `ready` queue collects finished tiles.
- On the next display tick (`NSView.displayLink`, macOS 14+ [D]), main pulls at most 8 ready tiles and attaches them in one transaction, moving nothing else.
- The display link is stopped whenever nothing is animating and no tile is pending. It costs nothing while idle and is not called while the view is hidden [D].

**Scroll at 120 Hz.** Frame budget is 8.33 ms. Main's share is the transaction (p99 ≤ 121 µs client side [M]) plus the atomic
swap; the target is ≤ 2 ms p99. Scrolling itself comes from `NSScrollView`: rubber-banding, momentum and scrollers all native, and
responsive scrolling keeps moving prepared content when main is busy [D]. Two things to prove in spike S1: whether custom layer
trees stay eligible for responsive scrolling (`isCompatibleWithResponsiveScrolling` is per view class [D]), and whether a document
view tall enough for 10,000 pages at high zoom is workable. `NSScrollView` accepted a 5×10⁸ point document in the API [M], but I
could not get the placement test to composite, so precision at that offset is **unverified**. Treat the risk as open, and if it
fails use a rebased coordinate space with a custom scroller.

**Pinch.** `NSScrollView.allowsMagnification` or a transform on the tile container: this moves GPU textures only, in the same frame,
with no renderer call. While the gesture runs, the old tiles scale. When it ends, wait about 100 ms, render for the new scale, and
swap the visible set in one transaction when it is complete, so no seams show.

**Blur to sharp.** Three levels per page:

1. the page's low-res texture, one layer per page at about 1/4 scale, volatile and kept for recent pages, shown at once;
2. tiles at the nearest cached scale, scaled by the GPU;
3. exact-scale tiles.

Never a blank frame after a page has been seen once. Budget one progressive pass per tile, because a page with a heavy image can be
cancelled and restarted at no cost.

**Wide gamut.** 8-bit BGRA with a tag on each surface: sRGB by default, Display P3 where the page's output space asks for it
(confirmed tag handling, in-process [M]). The window itself can stay pinned to sRGB to avoid the ColorSync tables, *if* tiles tagged
P3 still display wide when the window is sRGB: that needs S2 on a P3 screen. Half-float tiles double memory [M], so no HDR in v1.

**Windows.** Two real routes, both unmeasured here:

| | Composition visual per tile | `SwapChainPanel` |
|---|---|---|
| API | `Microsoft.UI.Composition` visuals, `CompositionDrawingSurface` (or sparse `CompositionVirtualDrawingSurface`, `IDCompositionVirtualSurface` with `Trim`) | One flip-model DXGI swap chain via `ISwapChainPanelNative` |
| Equals on Mac | The `CALayer` tree | The `CAMetalLayer` |
| Copies | One upload to the D3D texture; then the CPU copy can be dropped | One draw per frame into the back buffer |
| Off-UI-thread scroll | `InteractionTracker` + `VisualInteractionSource`: runs on an independent thread; touchpad input goes there by default, touch and pen on request [D]. The doc still says "60 FPS": check 120 Hz | `CreateCoreIndependentInputSource` input on a background thread [D] |
| Limits | Doc limits not found | No transparency, no acrylic over it, not stretched when resized, at most about four swap chains recommended [D] |
| Memory | Tiles only [I] | Buffers × window size, like Metal [I] |

Recommendation: composition visuals, same model as the Mac, decided by spike S8 on the PC. Sidebar there is `NavigationView` in left mode, with hierarchical items for
the TOC. `TabView` is horizontal only and Microsoft points to `NavigationView` for many static items [D], so the design is not the
default Windows tab pattern, but it is a supported one. Same layout, Fluent controls (guide §8.4).

### 2.6 Caps

| What | Cap | Why |
|---|---|---|
| Tile size | 512² device px | §2.5 |
| Non-volatile tiles (macOS, BGRA-equivalent) | `max(visible, min(128 MiB, 1.5 × visible))` | Visible always wins; 14-inch: 35 visible, 52 MiB; 5K: 77, 116 MiB; 6K+: visible only beyond 128 MiB |
| Volatile tiles | ≤ 256 MiB and ≤ 1,024 surfaces, LRU | Not in footprint [M]; caps VM churn and Mach-port count |
| Layers | ≤ 2× visible tiles | 2.3–6 KiB each [M] |
| Surface pool | ≤ 64 free | Creating is cheap, first touch is not [M] |
| Render workers | `min(3, perf cores − 1)`, started on demand, gone after 30 s idle | M4 mini has 4 perf cores; guide §3.2 |
| Jobs in flight | ≤ 2 per worker | Bounded in-flight work (guide §3.2) |
| Tiles attached per frame | ≤ 8 | Main stays under budget; verify in S1 |
| Tile render time | Soft budget 16 ms; at 5 s the worker is killed and the page marked failed | Hang isolation |
| Renderer process | 256 MiB footprint, checked with `proc_pid_rusage` per batch; over it, kill and relaunch | Hostile PDFs; recovery measured at 15–17 ms |
| Renderer decoded image | 64 MiB per image, downsample above | Decompression bombs |
| Low-res page textures | 16 most recent pages per hot tab; volatile | §2.5 |

**Cancel on scroll:** every job carries the generation it was queued for. A worker checks it before starting and at each
renderer cooperative point (per content operator batch). A stale tile that finishes anyway stays in the cache if within the prefetch
region, otherwise it returns to the pool. `ready` tiles are checked against the current generation at attach. Named failure: stale tile
on screen after a jump.

### 2.7 Threads and ownership

```
 Host process (RectoPDF)
 ┌──────────────────────────────────────────────────────────────────────────────┐
 │ main (AppKit) ─ owns NSWindow, NSOutlineView, CALayer tree: tile id → CALayer │
 │   input → core method (same one QA and the CLI call)                          │
 │   scroll/zoom → set_viewport(): atomic swap, unpark, never waits              │
 │   display tick → drain_ready(≤ 8) → one CATransaction                         │
 │   view models → pulled only while visible, signature skips redraw             │
 │ ▲ ready queue (lock-free)       │ viewport (atomic), commands (async FFI)     │
 │ │                               ▼                                             │
 │ session thread (Rust, parked when idle) ─ OWNS: tab table, tile cache (LRU,    │
 │   purgeable state), surface pool, sidebar and TOC models, scheduler, sidecars  │
 │   no task, thread, handle, or tile per idle tab                                │
 │                                                                                │
 │ index worker (0–1, QoS background, on demand) ─ owns one doc's text pass       │
 │ io/persist (0–1) ─ session store, debounced 2 s                                │
 │ watcher (0–1, shared) ─ hot tabs only                                          │
 │ agent socket (0–1, only when permission > Off) ─ blocked in kqueue             │
 │ hang watchdog ─ beats only while events are flowing, QA fails at 2 s           │
 └───────────────│──────────────────────────────────────────────────────────────┘
      XPC: job + file descriptor │ ▲ IOSurface (Mach port, zero copy), glyph runs, text
 ┌───────────────▼───────────────│───────────────────────────────────────────────┐
 │ Renderer service (sandboxed, no network, no files except passed descriptors)   │
 │ workers (0–3, on demand) ─ own decoded fonts and images cache, capped           │
 │ exits 30 s after the last job                                                  │
 └────────────────────────────────────────────────────────────────────────────────┘
 OS file cache holds the PDF bytes. pread through the descriptor; no app-level read cache.
```

Rules: the tile cache has one owner (session). The host holds layers and retains a surface only while attached. The renderer holds a surface
only during its job. No lock taken by a sync FFI call is ever held across disk or IPC (guide §2.4). Headless mode (CLI, tests) links `render` in-process
behind the same trait.

### 2.8 Crate graph

```
 recto-platform (below everything; the only crate naming an OS; any crate may use it)

 recto-core ──┬─▶ recto-render ─▶ recto-render-service (bin: XPC / pipe main)
              ├─▶ recto-index ─────┐
              └─▶ recto-sign ──────┤
 recto-proto (job/tile wire types; no deps) ─▶ render-service, session
                                   ▼
                         recto-session ─▶ recto-agent ─┬─▶ recto-ffi ─▶ app/ (Swift), app-windows/ (C#)
                                                      └─▶ recto-cli (`recto`)
```

A crate may depend only on crates to its left. I added two to the requested list: `session` (Seedwell's `engine`: tabs, tile scheduler,
sidebar and view models, so no logic ends up in `agent` or `ffi`) and `proto` (so the service and the session share types without linking each other).

| Crate | Contract | Must not |
|---|---|---|
| `core` | Parser, object model, page tree, sans-I/O over a `ByteSource` trait; `forbid(unsafe_code)` | Know about tiles, threads, OS |
| `render` | `render_tile(page, rect, scale, &mut [u8], cancel)`; writes into a caller's buffer | Allocate surfaces, name an OS |
| `index` | Text extraction to on-disk index, own query | Touch the UI, start threads |
| `sign` | Verify and apply signatures over `core` | Depend on `render` |
| `session` | Everything in §2.7's session thread; all rules | Name a UI toolkit |
| `agent` | Typed, versioned `Command`/`Query`; permission levels; socket and named-pipe transport | Hold state of its own |
| `ffi` | UniFFI surface; every call async or bounded; `unsafe` for surface handles | Hold rules |
| `platform` | `fs`, `dirs`, `proc`, `fswatch`, `surface` (IOSurface / D3D), `isolate` (XPC / AppContainer) | Hold policy |
| `cli` | Thin client of `agent`; can also run a headless session | Link AppKit |

`check.sh --quick` includes `cargo check --target x86_64-pc-windows-gnu` from the first commit, which `platform`'s seams must satisfy.

### 2.9 Structs that need `size_of` assertions

| Struct | Held per | Limit | Note |
|---|---|---|---|
| **`TabRecord`** | Every tab, including cold | **≤ 256 B** + title | The key one |
| `TileKey` | Tile | ≤ 16 B | tab, page, scale tier, x, y |
| `TileEntry` | Cached tile | ≤ 48 B | Key, surface handle, state, generation, LRU links |
| `Viewport` | One, swapped atomically | ≤ 48 B | |
| `RenderJob` | Queued job | ≤ 64 B | Queue itself capped |
| `SidebarRow` (model) | Visible row | ≤ 64 B + borrowed text | |
| `TocEntry` | TOC item, warm tab only | ≤ 32 B | Boxed slice, `u32` title offsets |
| `PageMeta` | Page, warm tab only | ≤ 16 B | Boxed slice: 160 KB at 10k pages; rebuilt from the sidecar |
| `XrefEntry` | Object, warm tab only | ≤ 16 B | Object count capped |
| `SearchHit` | Result, capped | ≤ 32 B | |

```rust
// size: asserted <= 256 by `size_of::<TabRecord>()`. Cold tab: this, its title, nothing else.
struct TabRecord {
    id: TabId,                // u32
    file: FileIdentity,       // [u8; 16] content key, u64 size, i64 mtime_ns
    path: PathId,             // u32 into an interned table; rows share directories
    title: Box<str>,          // 16 B + text
    page: u32, page_count: u32,
    scroll_frac: f32, zoom: f32,
    last_used: u32,           // coarse clock
    flags: TabFlags,          // u16: tier, toc_open, pinned, missing
    // No handle, tiles, TOC or page table here. All of that is rebuilt from disk on wake.
}
```

Tiers, as Seedwell's:

- **Cold:** the record. No handle.
- **Warm:** record + page table + TOC + volatile tiles; ≤ 64 KiB non-volatile + 16 B per page. Drops to Cold after 10 minutes.
- **Hot:** the visible tab: handle, viewport, scheduler entries. One per visible window.

---

## 3. R5: platform tables

### 3.1 macOS

| Feature | API | Min OS | Needs | Limits and traps |
|---|---|---|---|---|
| File association, Open With | `CFBundleDocumentTypes` with `LSHandlerRank`, `LSItemContentTypes` `com.adobe.pdf` | 10.0 | Nothing | Rank `Alternate` to appear in Open With without claiming; `Default` only when the user agrees |
| Become default viewer | `NSWorkspace.setDefaultApplication(at:toOpen:completion:)` | 12.0 | User consent prompt, asynchronous [D] | Offer it once from Settings; off the main thread |
| Open from Finder | `application(_:open:)` | 10.13 | | Can arrive before the window; core call async |
| Sandbox | `com.apple.security.app-sandbox` | 10.7 | **Required on the Mac App Store** [D], not otherwise | §1.5: kills `claude`/`rg` spawning |
| Hardened runtime | Signing flag | 10.14 | **Required for notarization** [D] | No JIT needed, library validation on. Seedwell's entitlements file is close to what's needed, without its app group |
| Files after a restart | Security-scoped bookmark `bookmarkData(options: .withSecurityScope)` | 10.7.3 | Sandbox only, entitlement | Also survives move and rename, useful outside a sandbox [D]. Plain bookmarks recommended [I] |
| Privacy prompts, unsandboxed | TCC "Files and Folders" | 10.15 | User consent for Documents, Downloads, Desktop, iCloud Drive, network volumes, removable volumes [D] | **Trap:** restoring tabs at launch can raise prompts. Touch a cold tab's path only when activated. Child processes' effect on the prompt is not tested (spike S6) |
| Spawn child `claude` | `Process` | | Unsandboxed: nothing | A Finder-started app has a minimal `PATH`: store an absolute path. Not on the main thread |
| Spawn from sandbox | `Process` + embedded self-contained tool signed `app-sandbox` + `inherit` | 10.7 | Children inherit the sandbox; XPC recommended instead [D] | Only self-contained tools, only in the sandbox, and §1.5. Answer: **it can spawn, but it can't reach the user's `claude` or Homebrew `rg`, and they could not read the user's files if it could** |
| Document model | `NSDocument` | Old | | **Rejected.** "Each `NSDocument` has its own `NSWindowController`" [D], which pairs one document with windows; each tab would be a window at about 0.6 MiB [M], and a read-only reader doesn't need its autosave or versions |
| Custom tabs | One window, tab table in core | | | Set `NSWindow.allowsAutomaticWindowTabbing = false` (10.12): `tabbingMode` defaults to the *user's* preference [D], which would add a system tab bar. Recent files via `NSDocumentController.noteNewRecentDocumentURL` still work [I, verify] |
| Sidebar | `NSSplitViewItem(sidebarWithViewController:)` | 10.11 | | HIG: disclosure controls for hierarchy, let people customize [D]. Glass on 26+ on link with SDK 26+ [D] |
| Content under sidebar | `automaticallyAdjustsSafeAreaInsets`, `NSBackgroundExtensionView` | 26.0 | | HIG suggests extending rich content under it [D]. Every extra point is more tiles: use a plain gutter [I, §11 D8] |
| Glass on custom elements | `NSGlassEffectView` | 26.0 | | Sparingly; budget unmeasured on-screen |
| Toolbars, title | `.fullSizeContentView`, `NSToolbar` | 10.10 | | Opts in to layer backing [D]. macOS 27 adds `NSToolbarItemGroupRole` and a `tabs` role on `NSSegmentedControl` [D]; don't require it |
| Text selection on custom view | `NSTextSelectionManager` | 27.0 | | New in 27 [D]. Selection logic belongs in Rust anyway; note it only |
| Change monitoring, one file | `DispatchSource` vnode with `O_EVTONLY` fd | 10.6 | | Event-only descriptor [D], no lock. An atomic replace ends in rename/delete: re-open by path. One fd each, so **hot tabs only** |
| Change monitoring, folders | FSEvents, `kFSEventStreamCreateFlagFileEvents` | 10.7 | | Coalesced. Reuse Seedwell's `trs-fswatch`. One stream |
| `NSFilePresenter` | Foundation | 10.7 | | Only hears other processes that coordinate [D]. Not used for watching. Network volumes miss events: `stat` on focus |
| Reading files | `pread` on a descriptor | | | Never on main. May block on iCloud or network: async open. No `mmap` for the main path: a truncated file means `SIGBUS` [I, well known] |
| Print | `NSPrintOperation` with a view, or `PDFDocument.printOperation(for:scalingMode:autoRotate:)` (10.7) | | | The PDFKit path loads 40+ MiB [M] for the rest of the process's life, so do it from a short-lived helper if the panel allows. S9 |
| Drag in | `registerForDraggedTypes` `.fileURL` | Old | | Window and sidebar |
| Drag out, reorder | `NSFilePromiseProvider` (10.12), `NSPasteboardItem`; `beginDraggingSession(items:gesture:source:)` is new in 27 [D] | | | Fixed in 27: `NSFilePromiseReceiver` asserts its call site [D] |
| Quick Look | `QLPreviewingController` (10.15), `QLPreviewProvider`, `QLThumbnailProvider` (10.15) | | Extension, sandboxed | System handles PDF already. **Skip in v1** |
| Spotlight | `.mdimporter` plugin; system `PDF.mdimporter` is present [M]. `CSImportExtension` (12.0) "don't provide functionality in macOS" [D] | | | System already indexes PDFs. **Skip.** Optional `CSSearchableIndex` donation only if opted in; it would copy text into another store |
| Sidecar data | `~/Library/Application Support/RectoPDF` (state, annotations); `~/Library/Caches/RectoPDF` (index, regenerable) | | | Keyed by content, not path. `isExcludedFromBackupKey` resets on some file operations [D]. Socket path 83 bytes on this home, under 103 |
| Fullscreen | `collectionBehavior.fullScreenPrimary` (10.7) | | | Grows the tile cap. `viewDidChangeBackingProperties`: scale change re-renders. 27 fixed a window lost when exiting via Mission Control [D] |
| Memory traps | ColorSync: pin chrome windows to sRGB (−1.2 MiB [M]); on a wide-gamut screen, check S2. `L008`/volatile: §2.5. Closing a window doesn't return AppKit's memory [M]. Metal drawables: §1.3 | | | |
| 120 Hz | `NSView.displayLink` + `preferredFrameRateRange` | 14.0 | | Callback not invoked when hidden [D]. Also the floor for the app's min OS |
| Intel | | | | Intel apps stop on macOS 28; macOS 27 notes list this [D]. **arm64 only**, one build |

Seedwell's min is macOS 15; `NSView.displayLink` requires 14. §11 D4.

### 3.2 Windows (all unverified here; run on the PC)

| Feature | API | Min OS | Needs | Limits and traps |
|---|---|---|---|---|
| App platform | Windows App SDK 2.6.5 (2026-11-02), WinUI 3 | Windows 10 1809 [D] | | Self-contained as Seedwell |
| Open With | ProgID + `HKCU\Software\Classes\.pdf\OpenWithProgids`, Capabilities + `RegisteredApplications` | | | Per-user. "Default Programs" doc says it doesn't apply to Windows 10 behaviour [D]; check against current |
| Become default | **No programmatic route.** `ms-settings:defaultapps?registeredAppUser=<name>` | Win11 22H2 with 2023-04 update [D] | User does it in Settings | Send `SHCNE_ASSOCCHANGED` after registering [D] |
| Isolation of renderer | AppContainer / LPAC child + Job Object with memory limit | Win8 | Nothing | LPAC "even more isolated" [D]. Pass duplicated handles. Latency not measured |
| Tabs and sidebar | `NavigationView` left mode, `TreeView` for TOC | | | `TabView` horizontal [D]; no native vertical |
| Change monitoring | `ReadDirectoryChangesW` on the parent folder, one per folder | | `FILE_LIST_DIRECTORY` [D] | Buffer overflow loses events; re-`stat`. Seedwell's fswatch has poll only so far |
| No locks | `CreateFile` with `FILE_SHARE_READ \| WRITE \| DELETE` [D] | | | Lets others replace or delete while open. Hot tabs only hold a handle. **This beats Adobe Reader's well-known lock** [I] |
| Print | `Windows.Graphics.Printing` via `IPrintManagerInterop` [D] (with the HWND) | | | The doc is old; verify on WinUI 3. S8 |
| Drag and drop | `AllowDrop`, `DataPackage` | | | Doesn't cross elevation [I] |
| Explorer preview | `IPreviewHandler`, always out of process [D]; `IThumbnailProvider` [D]; `IFilter` for search [D] | Vista | COM registration, signing advisable | Skip in v1 |
| Sidecar | `%LOCALAPPDATA%\RectoPDF` | | | Same keying |
| Timers | About 16 ms sleep rounding (guide §6.3) | | | Don't raise; composition drives frames |
| Fullscreen | `AppWindow.SetPresenter(FullScreen)` | | | |

---

## 4. R10: budgets

Acceptance criteria, in the format of guide §3.1. A missed budget is a bug. The whole process tree counts (host + renderer service,
§1.6). Release builds. Numbers are for an M4-class Mac; Windows numbers are provisional until the PC has measured its floors.

| Metric | Target | How measured | Why this number |
|---|---|---|---|
| **Cold start to first sharp page**, 1 small PDF, any number of tabs | **≤ 300 ms** p50, ≤ 500 ms p95 | Launcher timestamp at `exec` (`CLOCK_UPTIME_RAW`) to the core's `page_settled` event; 20 runs | Floor 113–133 ms [M], leaving ~170 ms for service spawn (16–21 ms [M]) and the first tiles. Seedwell's own bar |
| Open 2 GB, 10k pages: page 1 sharp | **≤ 250 ms** warm, ≤ 600 ms cold; **main never stalls > 16 ms** | QA `open --wait settled`; generated corpus file; main-thread watchdog on | PDFKit opens lazily in 6 ms but then blocks 3–5 s attaching [M]. This is the difference users would feel |
| Re-open the same big PDF | ≤ 50 ms | Same, sidecar present | Index and page table from disk (guide §3.2) |
| Memory, 1 doc, text page, 14-inch window (3024×1964) | **≤ 60 MiB** | `proc_pid_rusage` over the tree, `footprint` to cross-check | 17.6 window [M] + 2.2 service floor [M] + ≤ 15 renderer heap and fonts [I, render track] + grey/blank tiles ≤ 25. Preview: 98–108 [M]; PDFKit: 60–90 [M] |
| Same, colour full-bleed page | ≤ 100 MiB | Same | Window + 52 MiB BGRA tiles (§2.6) + heap |
| Fast scrolling, any doc, 5K+ | ≤ 170 MiB hard | Same, peak, QA scroll simulation | 128 MiB tile cap + floors |
| **Cost of one more idle tab** | **≤ 4 KiB, 0 handles, 0 threads, 0 layers, 0 tiles** | Scale test, own binary: 2 vs 200 tabs via `session`, tree footprint delta ÷ 198 (`tests/scale.rs`); handle count from `proc_pidinfo` | 256 B record + title + 220 B sidebar row [M] + session store share. SwiftUI's row alone would take 2.4 KB [M]; a retained `PDFDocument` costs 0.6–3.4 MiB [M] |
| Warm tab (recently seen) | ≤ 64 KiB + 16 B/page non-volatile; tiles volatile | Same harness, tier forced | Volatile doesn't count [M] |
| 1,000 tabs | ≤ 4 MiB over 2 tabs; sidebar opens < 150 ms | Scale test, QA | Marginal × 1,000 |
| Expand a 10k-entry TOC | ≤ 50 ms | QA | Cost only on tap |
| Idle CPU, window visible, unfocused | ~0%, **≤ 1 wake-up/s** avg | `proc_pid_rusage` over 60 s in QA; `powermetrics` in soak | Sidebar window floor 0.5/s [M], so ≤ 0.5/s of our own. Display link off when idle. Preview: 0.33–2.33 [M] |
| Scroll | **120 fps on ProMotion, 60 on others; ≥ 99% frames on time** | Display-link timestamp gaps in QA `scroll-sim`, calling the same `set_viewport`; Instruments on the MacBook for server hitches | 8.33 ms frame; main's share ≤ 2 ms p99 [M basis: ≤ 121 µs commit]. **Not provable on this Mac's 60 Hz display** |
| Blank during scroll | **0 blank frames** on seen pages; first-visit low-res within 2 frames | QA tile-state report | §2.5 level 1 |
| Sharp after scroll stops | ≤ 100 ms, text page | QA `page_settled` | Render track must confirm |
| Pinch feedback | **Same frame**; sharp ≤ 150 ms after gesture end | QA `zoom-sim`; state report | Transform only, no renderer [I] |
| Background indexing | **1 worker, QoS background, ≤ 16 MiB extra, ≤ 10 MB/s reads**; stops on Low Power, on battery below 20%, at thermal `.serious`, and while scrolling | `session` counters; tree footprint delta; `rusage` bytes read | Guide §3.4. Yields per page batch; resumes from checkpoint |
| Window closed, app still open | **≤ 20 MiB**; surfaces released; service exited after 30 s | Before and after, QA | 18.1 MiB floor [M]. Guide's "memory released" isn't reachable below that |
| Renderer crash | Next tile shown ≤ 100 ms after crash; no lost tabs | QA `crash-renderer` | Relaunch 15–17 ms [M] |
| File replaced by another program | Tab refreshes ≤ 1 s; other program never blocked | Fake writer test | §3 no locks |
| Agent `recto status --json` | ≤ 30 ms with the app running | CLI timing | Socket round trip |
| Soak: 24 h scripted, then 7 days idle per release | Flat ±5%, no crash | Soak rig | Guide §3.6, adjusted; a reader holds less state than a client |

Non-budget: tile volumes and blank share need the corpus (spike S3). Window-on-screen floors may be higher than off-screen ones (§10).

---

## 5. R9: distribution

| Route | Fit | Cost | Notes |
|---|---|---|---|
| **Developer ID + notarization, dmg + zip, `install.sh`** | **Yes: recommended** | Program already paid, pipeline exists | Copy `app/release-zip.sh`, `sign.sh`, `.github/workflows/release.yml`. Hardened runtime required [D]; `notarytool` only since Nov 2023 [D]; no Control-click override since Sequoia [D]. Sign nested code first; XPC service included |
| **Mac App Store** | **No, not with the agent requirement** | | Must be sandboxed (2.4.5(i)) [D]; can't install code or resources in shared locations, so no `recto` link on `PATH` (2.4.5(ii)); own updates not allowed (2.4.5(vii)); no reaching outside its container or running code that adds functionality (2.5.2); and §1.5 for `claude`. A later reduced "Lite" with no CLI is possible if `file access` stays behind one seam |
| **Homebrew cask** | Yes, after notarization | Own tap first | `app "RectoPDF.app"`, `binary "#{appdir}/RectoPDF.app/Contents/MacOS/recto"`. Casks failing Gatekeeper were scheduled for disabling in September 2026 [D]; official casks must pass Gatekeeper checks [D]. `recto` is free as a formula, as a cask, and on crates.io [M] |
| **Windows** | zip + `install.ps1`, per-user | §11 D5 | As Seedwell. Artifact Signing's Public Trust: **organizations in Norway are eligible, individual developers must be in the US or Canada** [D], so the owner needs an organization. Alternative: Microsoft Store |
| **Updates** | `latest.json` like Seedwell | | Reuse `tools/release-merge.py` and the updater: `version, tag, notes_url, minimum_macos, zip, sha256` (RELEASING.md). Sparkle 2.10.1 (2026-10-25) available later. **Check conflicts with "nothing leaves by default"**: §11 D6 |
| **Dev builds** | Same `build.json` signing | | Seedwell's "shared builds folder", which also gets builds to the MacBook for 120 Hz |

**Bundled binaries and licences.**

- `rg` 15.2.0: Unlicense OR MIT [M]. Bundling means shipping the MIT notice in `THIRD-PARTY-NOTICES.md`, signing it with our identity and hardened runtime, and using a self-contained build. Homebrew's links `pcre2` from `/opt/homebrew` [M]: use the project's own release binary or compile without PCRE2. **I recommend not bundling it at all**: `index` has its own search, and agents have their own `rg`.
- `claude`: proprietary, not ours to bundle. Spawn only what the user installed, from a stored path, on an explicit command.
- PDF engine (other track): MPL/BSD fits Seedwell's MIT/Apache; AGPL would change the licence of the whole app.
- Windows App SDK self-contained: follow its redistribution terms. Sparkle: MIT.
- Slint's royalty-free licence needs an `AboutSlint` widget or web badge, one more reason (b) isn't free.

---

## 6. R13: privacy and untrusted PDFs

**What stays local.** Everything. Index, thumbnails, annotations, reading position and recents are on disk only, under the paths in §3.1. No analytics, no crash upload, no network code in `core`, `render`, `index`, `session`.

**What can leave.**

- The update check (if on): one GET of `latest.json` on GitHub, no document data. §11 D6.
- A link: opened only on a click, in the default browser, shown first. No automatic `URI`, `GoTo` remote, `SubmitForm` or `Launch` actions.
- `claude`, if the user asks for it: that program is the network actor. The app says so before the first run.
- Signature revocation (OCSP/CRL): only on request (sign track).

**PDF execution.** JavaScript never runs. XFA is not executed. Attachments never launch. No fonts or images fetched from anywhere.

**Agent interface.** As guide §4.2: local socket mode 0600, named pipe with owner-only DACL, no TCP ever, levels Off / Read-only / Control / Full, default Off. Reading document text requires Read-only at least. Activity log. PDF content given to an agent is untrusted input and must be labelled as document data: a PDF can carry prompt injection.

**Where data lives.** Index is regenerable, so Caches; annotations aren't, so Application Support, with an export to save them into the PDF only on request. Keyed by content hash (first and last 64 KiB + size + trailer `/ID`), so a moved file finds its data and a stranger who gets the file gets nothing. Deleting a tab's history deletes its entries: records stay in their container (guide §2.5).

### Renderer isolation options

| | In-process | Sandboxed XPC service (**recommended**) | One service per doc |
|---|---|---|---|
| Memory | 0 extra | +2.2 MiB + its heap [M, C floor] | Per doc: violates the tab budget |
| Tile latency | 65–104 µs fill [M] | 0.11–0.13 ms including fill [M] | Same |
| First use | 0 | 16–21 ms warm, 109 ms first ever [M] | Same per doc |
| A memory bug in a PDF decoder | Reaches the user's files, keys, session | File passed by descriptor only, `open()` denied [M]; no network entitlement [D]; memory limit enforceable | Same, also isolates docs from each other |
| Crash | Whole app, tabs lost | 15–17 ms relaunch [M], one error page | Same |
| Hang | Thread stuck, can't kill | Kill and relaunch | Same |
| Work | None | IPC, surfaces, Rust ↔ XPC bindings | Same + routing |
| Windows | | AppContainer/LPAC child + Job [D], handle duplication; unmeasured | |
| Same-process fallback | | `render` is a plain crate: CLI, tests and fuzzers use it directly | |

IOSurface zero-copy across the boundary means isolation costs about 50 µs per tile, far under the frame budget [M]. That removes the usual reason to skip it. Fonts and decoded images stay in the service, where a crash or idle exit gives the memory back.

Note: the App Sandbox can apply to the XPC service while the app itself isn't sandboxed [M]. The app keeps Developer ID freedom; the risky parser gets the sandbox.

Windows has no IOSurface: a shared section or D3D shared handle takes its place. Spike S8.

---

## 7. R14: test strategy

### 7.1 The QA channel (guide §4) mapped here

Reuse Seedwell's `QAControl`, `QuietRun` (off-screen hooks, draws-as-active), `HangWatchdog`/`HangDetector`, `QAShots` (vibrancy), `app/qa/*.sh`. First-screen minimum per guide §4.3:

| Verb | Calls | Report or output |
|---|---|---|
| `open <path>` | `Session::open` | Waits for `page_settled` |
| `tab tap <id>` | `Sidebar::tap`: the "active tab toggles TOC" rule | Mapping from the real click to this call: unit test |
| `sidebar report` | | Rows, indent, selection, TOC state, text |
| `view scroll-to`, `scroll-sim <pts/s> <secs>` | `set_viewport` on the display tick, the method the scroll view calls | |
| `zoom-sim <from> <to>` | Same, plus the gesture-end call | |
| `tiles report` | | Per tile: blank, grey, low, sharp, pending, volatile, generation; pool sizes |
| `budget report` | | Tree footprint, handles, threads, wake-ups, frame gaps |
| `shot window\|page\|sidebar` | `cacheDisplay`/`CALayer.render` (works for tiles [M]) + vibrancy fix | 2x PNG, light and dark |
| `hover`, `focus` | Set state, never the pointer | |
| `crash-renderer`, `stall-renderer <ms>`, `slow-reads <ms>` | Fault injection, QA builds only | |

Rules restated: no synthetic input, no AppleScript, no focus, temp state root, wait for state not time. **Assert on the tile report and the model, not pixels.**

### 7.2 Fuzz targets (`cargo fuzz`, per crate; no panics on untrusted input)

- `core`: tokenizer, xref tables and streams, object streams, repair path, page tree (cycles, depth), name and number trees, outlines, encryption dictionaries.
- Filters: Flate, LZW, ASCII85, ASCIIHex, RunLength, DCT, CCITT, JBIG2, JPX (or the wrappers, if an engine owns them). Output-size limits.
- `render`: content-stream interpreter, fonts (CFF, TrueType, Type 1, Type 3, CMaps), colour spaces and ICC, shadings, images, patterns (recursion).
- `sign`: CMS/PKCS#7 parsing, byte-range arithmetic.
- `index`: index file reader. `session`: sidecar and session store readers. `agent`: JSON command parser. Updater: `latest.json`.
- Seeds from the corpus; a mutated-PDF structural fuzzer on top. Every crash goes to the regression corpus. Each target has a time limit; a long run per release candidate.
- `proto`: decoding on the host side treats the renderer as hostile. Tile dimensions, glyph runs and text offsets get validated before use, because a compromised renderer talks to the host.

### 7.3 Render diffs, without pixel-pinned tests

**Each test names its failure:**

- *Seam:* a page rendered as tiles equals the same page rendered whole, within ε along every tile boundary.
- *Scale:* a tile at 2× downsampled matches 1× within a tolerance.
- *Cancel:* no stale generation attached after a jump.
- *Blank:* a tile reported blank is, in fact, one colour.
- *Grey:* a tile reported `L008` has no chroma.
- *Colour tag:* a P3-tagged page round-trips.
- *Determinism:* the same input twice gives identical output on one build.

**Corpus diffs are reports, not tests.** Run our renderer against one or two reference engines over the corpus, compute a perceptual score per page, and commit a dated report with the worst pages as images (guide §3.6). The check fails only on a large regression from the last report, and a human or agent reviews it. Never hash pixels, never tie to a machine or macOS version (guide §6.2).

**Corpus:** legal only, in a separate fetched folder with each source's licence recorded (corpus track). Plus generated files for scale: the classic-xref writer used here made 10k pages/31 MB and 10k pages/2 GB in about a second.

### 7.4 Hang watchdog and timing

- Hang: guide §2.4. Stall over 1 s logged, 2 s fails QA. Beats only while events flow, so idle stays idle (Seedwell's `HangDetector` already distinguishes this; verify the wake-up count).
- `ui_wait_tests` for every sync FFI call, with the renderer stalled and reads slowed. Includes `set_viewport`, `drain_ready`, sidebar model.
- Per-frame metric in QA: main-thread duration per display tick, fails at p99 > 2 ms in `scroll-sim`. Named failure: FFI call blocking on the session.
- Renderer: 5 s per tile, then kill. Test with a page that never finishes.
- Timing tests run twice, the second under `taskpolicy -c utility` (Seedwell's `timing-tests.txt`).

### 7.5 Scale and memory

`tests/scale.rs` in its own binary: tab marginal cost (2 vs 200), per-tier cost, tile accounting matches `budget report`, no growth over 1,000 open/close cycles. `size_of` tests from §2.9. Benchmark harness against Preview and PDFKit (like `bench/`), plus Skim or Sioyek if the prior-art track picks them; all launched hidden and only when not already running.

---

## 8. Confirmed

| Claim | Evidence |
|---|---|
| Sidebar window floor 17.6 MiB AppKit, 27.9 MiB SwiftUI | §1.1, off-screen |
| AppKit row 220 B, SwiftUI row 2.4 KB | §1.1 |
| Closing a window leaves 18 / 29 MiB | §1.1 |
| ColorSync trap reproduces, about 1.2 MiB per first window | §1.1 |
| IOSurface volatile leaves footprint, restores instantly with no pressure | §1.3 |
| IOSurface tile as layer contents: BGRA, RGBA, L008 read correctly in-process | §1.4 |
| Layer-tree tile capture works without permissions | §1.4 |
| P3 tag honoured by CA's in-process path | §1.4 |
| `CAMetalLayer` costs 105–184 MiB | §1.3 |
| PDFKit main-thread stall 3–5 s at 10k pages | §1.2 |
| Sandboxed child can't reach `rg` or `claude`, helper must be self-contained and inherits the sandbox | §1.5 |
| Sandboxed XPC service with passed fd and zero-copy IOSurface: costs in §1.6 | §1.6 |
| `recto` free on Homebrew and crates.io | Checked via `gh api` and crates.io today |
| `pread` and `mmap` over 776 MiB don't grow footprint | §1.2 |

## 9. Not tested

| Item | Why | Goes to |
|---|---|---|
| Any on-screen floor, Liquid Glass memory | Quiet-run rule: no window on a screen | S1, on the MacBook during a dev build |
| 120 Hz | This display reports 60 Hz (`Odyssey G93SC`, 1600×900 @ 60) | S1 |
| Real scrolling, pinch, `NSScrollView` with a tile tree | No input allowed; off-screen | S1 |
| Tile placement at huge document offsets | My `CARenderer` placement test produced no pixels in five attempts; time-boxed out | S1 |
| `L008` and P3 on the real compositor; sRGB window + P3 tiles | Off-screen, `CARenderer` retry inconclusive | S2 |
| Volatile surfaces under real memory pressure, attached-and-volatile | Would disturb the machine | S3 |
| Host-created surfaces filled by the service | Only service → host tested | S4 |
| Rust XPC bindings, Rust renderer floor | Probe was C; no heavy Rust builds | S4 |
| Per-toolkit memory | No heavy Rust builds | S7, only if (b) is reopened |
| Electron floor | Downloaded binary refused by permission rules | Not needed for the decision |
| TCC prompts on restore; effect of a child | A prompt would interrupt the owner | S6 |
| Helper running from the container | Probe inconclusive; not needed as MAS is rejected | Dropped |
| All of Windows | No Windows here | S8 |
| Print paths | Needs real panels and printers | S9 |
| Battery and thermal effects | Mac mini | S1 |
| Preview's hidden-launch number's meaning | It may not have rendered | Benchmark harness in a visible run on the MacBook |

## 10. Spikes

Each time-boxed, throwaway, with a Confirmed / Not tested table (guide §1.2 R11).

| # | Question | Box | Blocks |
|---|---|---|---|
| **S1** | On a real screen at 120 Hz: `NSScrollView` + tile layer tree holds frame budget? Stays eligible for responsive scrolling? Document size limit and placement at 10k pages × high zoom; rebased fallback? On-screen window floor with glass? | 1 day (MacBook, by its agent) | M1 |
| **S2** | `L008` and P3 tags through the actual compositor; sRGB-pinned window showing P3 tiles. Check with in-app readback on a P3 screen | 2 h | Colour budget |
| **S3** | Volatile tiles under memory pressure: restore rate, `purgeableEmpty` handling. Blank and grey share of tiles over the corpus at 256 and 512: settles tile size | 4 h | Tile cap |
| **S4** | The real seam: Rust ↔ XPC (`block2`, raw `xpc_*`), host-created surfaces, descriptor passing, cancel. Rust renderer floor from the chosen engine. Hardened runtime + Developer ID signed, notarized service | 1 day | M1, R13 |
| **S5** | First-screen vertical: sidebar tap → TOC toggle via core; one PDF to tiles; `shot page`; `tiles report` | Part of M1 | M1 |
| **S6** | TCC: session restore for a tab in `~/Downloads`; effect of `noteNewRecentDocumentURL` and of bookmarks; what child `claude` inherits. Fresh user account, by the owner's MacBook agent | 2 h | Restore design |
| **S7** | Only if D1 makes Windows equal priority: GPUI and Slint hello windows, on the 16 GB Mac under the heavy lock, `footprint`, and a real IOSurface tile path | 1 day | D1 |
| **S8** | Windows PC: WinUI 3 floor; composition tile tree vs `SwapChainPanel`, memory and 120 Hz; `InteractionTracker` rate; LPAC child cost; shared-section tiles; `IPrintManagerInterop` on WinUI 3; `ms-settings:defaultapps` | 1–2 days, when Windows starts | Windows only |
| **S9** | Print: full PDF through `NSPrintOperation` paths; residual memory; helper feasibility | 2 h | Print feature |

## 11. [DECIDE] items

**D1. Platform priority. Blocks M1.**
- *Options:* (a) Mac first, Windows parity later (assumed here). (b) Both at launch. (c) Mac only.
- *Recommendation:* (a), matching Seedwell: Windows starts once the Mac's model is stable, with the core built for Windows from the first commit.
- *Cost to change later:* low, if `check.sh`'s Windows cross-check is there from day one. (b) now would reopen (b) in §2.2.

**D2. Architecture. Blocks M1.**
- *Options:* (a) AppKit + Rust; (b) GPU toolkit; (c) webview.
- *Recommendation:* (a), §2.
- *Cost to change later:* high for the host (it is rewritten); the core and agent interface carry over.

**D3. Mac App Store. Blocks little, decides much.**
- *Options:* (a) Developer ID only. (b) Both, with a reduced MAS build. (c) MAS only, no agent features.
- *Recommendation:* (a). (c) contradicts the agent requirement; (b) doubles test paths for little gain.
- *Cost to change later:* medium. Add a sandbox build by keeping bookmarks and file access behind one seam from the start; that seam costs almost nothing.

**D4. Minimum macOS.**
- *Options:* 14 (floor for `NSView.displayLink`), **15** (Seedwell), 26 (Liquid Glass everywhere, one look).
- *Recommendation:* 15, arm64 only. AppKit gives each OS its native look with no extra code; share Seedwell's support window.
- *Cost to change later:* lowering is expensive only if 26-only APIs spread, raising is free.

**D5. Windows signing.**
- *Options:* (a) unsigned, script installer, as Seedwell today. (b) Artifact Signing, via a registered Norwegian organization. (c) Microsoft Store.
- *Recommendation:* (a) until Windows ships; then (b) if there's an organization. Individuals outside the US and Canada aren't eligible [D].
- *Cost to change later:* low. An accounts task for the owner.

**D6. Update check vs "nothing leaves the machine by default".**
- *Options:* (a) Off by default, with a clear "Check for Updates" command and a one-time question on first run. (b) On, once a day, disclosed, no document data. (c) No in-app updates: Homebrew or manual.
- *Recommendation:* (a). It keeps the privacy statement literally true, for one extra question. It is also the one setting that needs a choice before the first release.
- *Cost to change later:* low in code, but a default changed later is a promise changed.

**D7. Should the app itself spawn `claude`, or do agents only call `recto`?**
- *Options:* (a) `recto` only: agents drive the app, the app never starts one. (b) Plus an "Ask Claude" command that runs a user-installed `claude` at a stored path. (c) Bundle tools.
- *Recommendation:* (a) for v1, then (b) when wanted. Both are compatible with a Developer ID build; (c) has licence and signing costs for nothing.
- *Cost to change later:* low: (b) is an addition.

**D8. Under the Liquid Glass sidebar.**
- *Options:* (a) Plain gutter colour under the sidebar. (b) Pages extend under it, as the HIG suggests for rich content.
- *Recommendation:* (a), for now. A page is not decoration, and (b) costs more tiles. Show both in the concept renders (R12): this is a taste call.
- *Cost to change later:* low.

**D9. Closing the last window.**
- *Options:* (a) Quit, restore tabs next launch. (b) Stay running, at about 20 MiB. (c) The user's setting.
- *Recommendation:* (a) with fast restore: it uses 0 MiB versus a floor of 18 [M], and a 300 ms start makes it cheap. The agent CLI can start the app or a headless session when needed.
- *Cost to change later:* low.

**D10. Agent access default.**
- *Options:* Off / Read-only at first run.
- *Recommendation:* Off, with the one-command request flow from guide §4.2. Documents are private by default.
- *Cost to change later:* low.

## 12. Sources

All retrieved 2026-11-06. Dates are the page's own where it shows one.

**Apple (developer.apple.com/documentation, JSON form for page data)**
1. App Review Guidelines 2.4.5, 2.5.1, 2.5.2, 4.2.3: developer.apple.com/app-store/review/guidelines/ (no date shown)
2. App Sandbox ("To distribute a macOS app through the Mac App Store, you must enable"): security/app-sandbox
3. Accessing files from the macOS App Sandbox: security/accessing-files-from-the-macos-app-sandbox
4. Embedding a command-line tool in a sandboxed app: xcode/embedding-a-helper-tool-in-a-sandboxed-app
5. `Process` ("child processes you create with this class inherit the sandbox"; XPC recommended): foundation/process
6. Hardened Runtime; Notarizing macOS software; Customizing the notarization workflow: security/...
7. Updates to runtime protection in macOS Sequoia, 2024-08-06: developer.apple.com/news/?id=saqachfa
8. Controlling app access to files in macOS: support.apple.com/guide/security/secddd1d86a6/web
9. macOS 27 Golden Gate release notes; macOS Tahoe 26 release notes: macos-release-notes/...
10. AppKit updates (June 2026, June 2025): updates/appkit
11. `NSView.displayLink(target:selector:)` (14.0); `CADisplayLink`, `preferredFrameRateRange` (14.0); `CAMetalDisplayLink` (14.0); `CAMetalLayer.maximumDrawableCount` (2 or 3)
12. `CATiledLayer`; `NSScrollView.allowsMagnification`; `NSView.prepareContent(in:)`, `isCompatibleWithResponsiveScrolling`, `preparedContentRect`; AppKit 10.9 notes, "Responsive Scrolling": library/archive/releasenotes/AppKit/RN-AppKitOlderNotes
13. `NSSplitViewItem` sidebar (10.11), `automaticallyAdjustsSafeAreaInsets` (26.0); `NSGlassEffectView`, `NSBackgroundExtensionView` (26.0); Liquid Glass and Adopting Liquid Glass (technologyoverviews); HIG Sidebars and Tab bars
14. `NSWindow.tabbingMode`, `allowsAutomaticWindowTabbing` (10.12); `NSWindow.StyleMask.fullSizeContentView`; `collectionBehavior.fullScreenPrimary`
15. Developing a Document-Based App; `NSDocument`; `NSDocumentController`
16. `NSWorkspace.setDefaultApplication(at:toOpen:completion:)` (12.0); `CFBundleDocumentTypes`, `LSHandlerRank`; `application(_:open:)`
17. `NSURL.bookmarkData`; `NSFilePresenter`; `NSFileCoordinator`; File System Events; `DispatchSourceFileSystemObject`; `isExcludedFromBackupKey`; `man 2 open` (`O_EVTONLY`)
18. `NSPrintOperation`; `PDFDocument.printOperation`; `NSFilePromiseProvider`
19. `QLPreviewingController`, `QLThumbnailProvider`; `CSImportExtension`
20. Creating XPC services; XPC framework
21. SDK 27.2 headers: `QuartzCore/CALayer.h` (contents: "typically a CGImageRef or an IOSurfaceRef"), `IOSurface/IOSurfaceRef.h` (`IOSurfaceSetPurgeable`, 10.12; `kIOSurfaceColorSpace`), `CoreVideo/CVPixelBuffer.h`; `man footprint`

**Microsoft (learn.microsoft.com/en-us)**
22. `SwapChainPanel` (2026-07-28): windows-app-sdk/api/winrt/microsoft.ui.xaml.controls.swapchainpanel
23. `CompositionVirtualDrawingSurface` (2026-07-28); `IDCompositionVirtualSurface` (2018-12-05); DXGI flip model (2018-05-31)
24. InteractionTracker manipulations (2026-03-16): windows/uwp/composition/interaction-tracker-manipulations
25. TabView (2026-07-21); NavigationView (2026-10-02): windows/apps/design/controls
26. Windows App SDK stable channel (2026-11-02); system requirements (2026-09-10)
27. Default Programs (2025-04-15, "does not apply for Windows 10"); File types (2018-05-31); Launch the Settings app (2026-11-02)
28. `ReadDirectoryChangesW` (2018-12-05); `CreateFileW` (2022-08-16)
29. AppContainer isolation (2025-07-08); Implementing an AppContainer (2023-07-20)
30. Preview handlers (2018-05-31); Thumbnail providers (2018-07-02); `IFilter` (2018-05-31); `IPrintManagerInterop` (2018-12-05)
31. Artifact Signing overview (2026-01-02), quickstart (2026-05-21): azure/artifact-signing; pricing page; Smart App Control (2026-09-25)
32. WebView2 process model (2022-04-01)

**Projects (GitHub `gh api`, crates.io, 2026-11-06)**
33. zed-industries/zed: `crates/gpui/README.md`, `Cargo.toml`, `src/_accessibility.rs` (AccessKit, #56065, 2026-05-27), `crates/gpui_apple/src/metal_renderer.rs`, issues #31102, #7465, #41138; longbridge/gpui-kit v0.9.0 (2026-11-02), issues #3698, #3437
34. slint-ui/slint v1.18.1: `CHANGELOG.md`, `LICENSES/LicenseRef-Slint-Royalty-free-2.0.md`, backends doc
35. iced-rs/iced 0.14.0: `README.md`, `CHANGELOG.md`, #552, #1849
36. emilk/egui 0.37.0: `README.md`, #8818, #8815, #8822, #8828, #8813, #3532
37. linebender/xilem v0.4.0: `README.md`, #1692. makepad/makepad: `README.md`, #196, #1223, #1227
38. mozilla/uniffi-rs v0.32.2; NordSecurity/uniffi-bindgen-cs v0.11.1+v0.31.0 (2026-10-09)
39. tauri-apps/wry `README.md`; tauri-v2.12.5 (2026-10-21); electron v45.2.3 (2026-11-04)
40. microsoft/WindowsAppSDK v2.6.5; microsoft/microsoft-ui-xaml
41. BurntSushi/ripgrep 15.2.0 (2026-07-15), `COPYING`; sparkle-project/Sparkle 2.10.1 (2026-10-25)
42. Homebrew 5.0.0 notes (2025-11-12): brew.sh/2025/11/12/homebrew-5.0.0/; Acceptable Casks and Cask Cookbook: docs.brew.sh

**Owner's material**
43. `docs/agent-instructions/rust_app_development.md`; Seedwell `SPEC.md` §2–3, `docs/ENGINEERING.md`, `docs/ENGINE.md`, `docs/CROSS-PLATFORM.md`, `docs/RELEASING.md`, `app/Package.swift`, `app/Seedwell.entitlements`, `app/Sources/SeedwellApp/Support/QuietRun.swift`, `QAShots.swift`, `HangWatchdog.swift` (read-only)

**Side effects of this research on the machine.** Two small empty-ish sandbox containers that macOS created and that I did not try to delete (removing one can raise a system prompt): `~/Library/Containers/io.github.kkollsga.RectoSpike` (104 KB) and `~/Library/Containers/no.rectospike.render` (36 KB). Preview's recents may list two generated PDFs from the hidden launches. Probes were deleted with the scratch files except sources and raw output.
