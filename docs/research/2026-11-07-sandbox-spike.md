# Spike: PDFium in a sandbox on a Developer ID build

2026-11-07, macOS 27 (Darwin 27.0.0), arm64, 16 GB, ad-hoc signed, release builds. Code in `spikes/sandbox/`, outside the workspace. Production code is unchanged.

## Answer

**Yes, PDFium runs sandboxed with every limit and number intact, but not in an XPC service, and the App Sandbox alone doesn't hold the "start processes" wall.**

**Recommendation: a launched helper, as today, with three walls.**

1. The worker stays a child process, started by the same `ChildProcess::spawn` over the same socket pair and shared memory. It lives in its own small `.app` inside the main app, signed with `com.apple.security.app-sandbox`.
2. `RLIMIT_NPROC`, `RLIMIT_FSIZE` and `RLIMIT_CORE` at zero, as now.
3. New: once PDFium has loaded and before any hostile byte arrives, the worker lets go of every Mach send right it holds except the kernel's. This closes a hole the sandbox leaves open (below).

The worker also tests the wall before it says hello, and exits if the sandbox isn't on, so a build signed wrong can't draw a hostile file.

| Cost against today | Before | After |
|---|---:|---:|
| Worker start to hello, p50 | 4.76 ms | 8.64 ms |
| Respawn, reopen, draw first page, `rfc9110`, p50 | 14.57 ms | 17.22 ms |
| Idle worker footprint | 2.22 MiB | 2.34 MiB |
| Round trip, whole pages at 2x, `rfc9110`, p50 / p95 | 1.90 / 2.44 ms | 1.85 / 2.44 ms |
| USGS map first view / scroll / zoom | 554 / 108 / 408 ms | 558 / 110 / 390 ms |
| USGS map peak, first view / zoom | 394 / 454 MiB | 394 / 451 MiB |
| `form_bomb`, all steps, kills, heavy retries, peak | 2.48 s, 2, 1, 1026 MiB | 2.47 s, 2, 1, 1026 MiB |
| Pages drawn both ways, PNG bytes compared | | 12 of 12 identical |

About 3 to 4 ms per worker start and 0.12 MiB per worker. Nothing else moved past the noise. The 103 existing tests in `crates/recto-render-service/tests` pass with no change against the sandboxed worker, which refuses to start unsandboxed, so they did run inside it.

**Why not XPC, which the platform report recommended:** two measured blockers. An app gets one service process however many connections it opens, so one worker per document can't exist without private API. And launchd delays the next start by 10 s after any service that ran under 10 s, whether it was killed or left by itself, so the memory sequence takes 17 s instead of 2.3 s.

## Method

`spikes/sandbox/bundle.sh` lays out each arrangement, ad-hoc signed:

| Layout | What it is |
|---|---|
| `plain` | Today: the worker as built |
| `entitled` | The same worker binary, unchanged, signed with the App Sandbox entitlement |
| `dropmach` | `entitled`, plus the check and the Mach drop. **The recommendation** |
| `selfbox` | No entitlement. Calls `sandbox_init` with a deny-by-default profile after loading PDFium |
| `xpc` | The worker as an XPC service. XPC launches it and hands over the socket, which then carries the same frames |

The variants are built from the real `worker.rs` by `build.rs`, so they can't drift from it. `plain`, `entitled`, `dropmach` and `selfbox` run the real `recto`, whose release build finds a worker beside itself, so these numbers come from the production host. `xpc` can't, because the host's `ChildProcess` can't start it, so `xpc-host` measures it with the same frames and the same 5 ms memory poll, and measures `plain` the same way as a control.

Taking turns over 5 rounds (`measure.sh`, under `heavy.sh`) so a busy machine hits them all alike; another agent was building at the same time, load about 5. Figures are medians over rounds. Every run used temp state, and `recto` was given its level the way the CLI's own tests do.

Reused the bundle ID `no.rectospike.render`. **No new container was made.** See "Left on this machine".

## 1. What breaks in the sandbox

Nothing that PDFium needs, once the worker has a bundle identity. Evidence from the kernel's sandbox log during all the runs.

| Thing | Result |
|---|---|
| **A bare signed binary** | **Dies at launch, SIGTRAP, in `_libsecinit_appsandbox`.** `secinitd`: "Unable to get bundle identifier because Info.plist from code signature information has no value". It needs an `.app` with an `Info.plist` signed into it. This is the only thing that broke outright |
| **AppKit** | No problem. `otool -L` lists it, but `nm -mu` shows **no symbol imported from AppKit or Foundation**: 135 from libSystem, 23 from CoreGraphics, 4 from CoreFoundation. It's a load command with nothing behind it. No window server connection appears in the worker's log |
| **`dlopen`** | Works for a library inside the worker's own bundle. Denied from the outer app's `Contents/Frameworks` (`file-read-data` denied, worker exits with no hello). So the library lives in the helper |
| **Fonts** | Works. PDFium reads `/System/Library/Fonts` and `/Library/Fonts` itself, as files, and the sandbox allows both. `CoreText` isn't involved: PDFium imports `CGFontCreateWithDataProvider` and no `CT*` symbol |
| **Mach lookups** | The worker's log names only `cfprefsd` and `lsd.mapdb`, during loading. Both allowed. Nothing depends on them after loading: the Mach drop draws the same pixels |
| **Passed file, socket, shared memory** | All work with no entitlement: `SCM_RIGHTS`, `mmap` of the shared memory, `pread` on a file on an external volume the sandbox can't open by path |
| **Self-measurement** | `proc_pid_rusage` on itself works, so the worker's own watchdog still fires. `kill` on its parent is refused, which is right |
| **Denied and harmless** | `system-info vfs.disk-space`, from loading. Rendering is byte-identical with it denied |

**Fonts can fail silently.** The first `selfbox` profile allowed the font folders but not a stat of their parents. It drew page 1 of the font test with a substitute and reported success. That's why `the_sandboxed_worker_draws_system_fonts_the_same_as_a_plain_one` compares pixels. `make_font_pdf.py` makes the file: 10 base-14 fonts, 21 Mac font names, 4 CJK fonts, none embedded.

**Pixels.** 12 pages each through `entitled`, `dropmach` and `selfbox`, against `plain`: 36 of 36 PNGs byte-identical (`compare_pixels.sh`). The three font pages, `rfc9110` p10 and p110, the USGS map, two NASA pages, and four arXiv pages, including `cs_CV_2607.28624`, whose font list has Courier and Helvetica not embedded. I looked at the font pages and `cs_CV` p3: they read correctly, and CJK and Mac fonts are present.

**User-installed fonts: not tested.** PDFium's string is a literal `~/Library/Fonts`, and it holds no `HOME` string, so it probably never finds that folder, sandboxed or not. This Mac has no such folder and I didn't make one. If that's wrong, a sandboxed worker would lose them.

## 2. Which mechanism

| | Launched helper + entitlement (**recommended**) | `sandbox_init` at worker start | XPC service |
|---|---|---|---|
| One worker per document | Yes | Yes | **No.** Two connections, one pid (measured). `xpcservice.plist(5)`: "a new connection to an existing service" |
| Restart after kill or exit | 8.6 ms | 9.5 ms | **10,026 ms.** launchd: "Service only ran for 0 seconds. Pushing respawn out by 10 seconds." Clean exit too |
| Memory sequence on `form_bomb` | 2.32 s | 2.30 s | **17.2 s** |
| `phys_footprint` from outside | Yes | Yes | Yes |
| `SIGKILL` | Yes, as a child, safe against a recycled pid | Same | By pid from outside, not a child, so a number can go stale. `xpc_connection_get_pid`'s own header warns of this |
| Exit status, peak after death | Yes | Yes | **No.** Not the parent. Peak reads `None` once gone, so the host must sample it |
| Heavy slot | Unchanged | Unchanged | Meaningless with one process |
| Inherited channel, shared memory | Unchanged | Unchanged | Socket survives, passed in a message. Needs a held message to stay boosted: p50 8.7 ms without, 1.91 ms with |
| Works from `recto` with no app, and from `cargo test` | Yes | Yes | **No.** Connection invalid outside a bundle. Through a link into a bundle, yes |
| Walls | All, with the Mach drop | All | LaunchServices hole, as the entitlement alone. Mach drop not tried |
| Idle footprint | 2.34 MiB | **5.00 MiB**, the profile's compiled form | 2.63 MiB |
| Container | One, 36 KB | None | One |
| Status of the API | **Supported**, no private or deprecated call | **Deprecated, and outside its contract.** See below | Supported, but the instance call is private |

**XPC's way round one process per application** is `xpc_connection_set_instance`, which is exported but in no SDK header, so it's private. I didn't try it. A set of N copies of the service would be public, but each needs its own bundle ID, container and copy of the library, since one can't read another's. Not built, because it would have made new containers.

**The 10 s delay and the 15–17 ms relaunch in platform report §1.6 are both right.** launchd counts from the last start: a worker killed after a 12 s run has its successor say hello in 21.4 ms, and after a 3 s run in 7,025 ms. A renderer's retries and retirements all come quickly, so it gets the delay.

**`sandbox_init` is the runner-up**, and the only route with a tighter wall: the kernel refuses `fork`, `exec`, every Mach lookup, `stat` and `IOSurfaceRoot`. But:
- `sandbox.h`: "This header is deprecated and may be removed", and "No longer supported" on both functions since 10.8.
- Its header says flags "Must be SANDBOX_NAMED. All other values are reserved". A profile as text, the only way to write a custom one, passes flags 0, so it's outside the documented contract.
- **The documented form is already dead here.** `sandbox_init` with the named profile `pure-computation`, `no-network`, `no-internet`, `no-write` or `no-write-except-temporary` kills the process, termination namespace `SANDBOX` (`sandbox-exec -n` too: rc 137), though the `kSBXProfile*` constants still hold those names.
- It can't be combined with the entitlement: called inside the App Sandbox, it returns -1, as the header says.
- Chromium's `sandbox/mac/seatbelt.cc` calls `sandbox_init` with the comment "OS X deprecated these functions, but did not provide a suitable replacement", plus the private `sandbox_compile_string` and `sandbox_apply` (read 2026-11-07), so Apple breaking this would break Chrome. That's a comfort, not a promise.

Keep it as the fallback if the entitlement route ever stops working: `profile.sb` is 6 lines, and the variant works today.

**Others looked at, not used.** `sandbox-exec`: deprecated, the same mechanism. `BERenderingProcess` in BrowserEngineKit is in the macOS SDK (14.3), but it's for browser engines and an extension, and I didn't look further.

## 3. The walls

`spikes/sandbox/tests/walls.rs`, 8 tests, each naming the failure it catches. `sandbox-probe` stands in for a compromised renderer and tries the real calls. Every attempt also runs with no sandbox, and a broken attempt fails that test. The canaries are the test's own: one in its scratch folder, and one in a folder it makes in the home folder and removes.

| Attempt | No sandbox | App Sandbox alone | + limits | **+ Mach drop (recommended)** | `sandbox_init` profile |
|---|---|---|---|---|---|
| Read canary, scratch | Yes | **EPERM** | **EPERM** | **EPERM** | **EPERM** |
| Read canary, home | Yes | **EPERM** | **EPERM** | **EPERM** | **EPERM** |
| List home | Yes | **EPERM** | **EPERM** | **EPERM** | **EPERM** |
| Write outside the container | Yes | **EPERM** | **EPERM** | **EPERM** | **EPERM** |
| Write inside the container | Yes | Yes | **EFBIG** | **EFBIG** | **EPERM** |
| TCP connect, loopback listener | Yes | **EPERM** | **EPERM** | **EPERM** | **EPERM** |
| UDP bind, DNS socket | Yes | **EPERM** | **EPERM** | **EPERM** | **EPERM** |
| `posix_spawn`, `fork` | Yes | Yes | **EAGAIN** | **EAGAIN** | **EPERM** |
| Signal the parent | Yes | **EPERM** | **EPERM** | **EPERM** | **EPERM** |
| Look up LaunchServices, window server, pasteboard | Yes | Yes | Yes | **Fails** | **Denied** |
| **Launch an app through LaunchServices** | Yes | **Yes, and it runs unsandboxed** | **Yes, and it runs unsandboxed** | **Fails** | **Fails** |
| `exec` a system binary | Yes | Yes | Yes | Yes, see below | **EPERM** |
| `IOSurfaceRoot` user client | Yes | Yes | Yes | Yes | **Denied** |
| `stat` a file in home | Yes | Yes | Yes | Yes | **EPERM** |

**The hole.** Inside the App Sandbox with the limits on, `LSOpenCFURLRef` launched a background test app, which read the home canary: it runs outside the sandbox. Asked for an app it couldn't read, the call returned -54, so the check seems to be whether the worker can read the app, and it can list `/Applications` and read `/System/Applications/Calculator.app`. So a compromised worker can probably start any installed app, and through a URL, a browser, which would send what it's drawing to the network. **I launched only my own background test app: a real app or `https://` would take the screen.** XPC has the same hole: the same attempt from inside the XPC service also launched and read the canary. No entitlement takes this away: entitlements only add.

**The Mach drop closes it.** `src/mach_drop.rs`, about 40 lines, public calls in `<mach/*.h>` with no deprecation marks: `mach_port_names`, `mach_port_kernel_object`, `mach_port_mod_refs`, `task_set_special_port`, `mach_ports_register`, `task_set_exception_ports`. A Mach service can only be reached with a send right, and a process can't invent one. In the probe it drops 10 rights and keeps 6 kernel objects: this task, threads, host. After it:
- `bootstrap_look_up` returns 32, LaunchServices -10661, and no app runs.
- `exec` of `/usr/bin/open` on the test app fails with -10827, since the new image has no way to launchd either. A plain `exec` still works and stays inside the sandbox: `/bin/cat` on the home canary gets EPERM.
- Dropping only `task_set_special_port(bootstrap)` and the `bootstrap_port` right wasn't enough: LaunchServices still worked, because libxpc holds its own copy. All of them must go, which is why it walks the whole table.
- Every other place the kernel hands out a right was asked: task, thread and host special ports, registered ports, exception ports. One user port comes back, slot 9 (`TASK_ACCESS_PORT`), which refuses to be cleared (rc 8) and doesn't answer a bootstrap lookup. The test fails if another appears.

It isn't a documented security boundary. It's the second wall behind the sandbox, and it's about Mach only. A kernel bug is out of its reach.

**What a compromised worker can still do with the recommendation:** `stat` a path it can guess (existence, size, dates), read world-readable system files and list `/`, create empty files in its container, open `IOSurfaceRoot`, and draw a wrong picture, as before. Only `sandbox_init` closes `stat` and IOKit.

**Crash reports.** Dropping the exception port means a worker makes none. The host still reads the exit status. A report would hold hostile bytes anyway.

## Numbers

**The production host and `recto budget`**, 5 rounds, median. `recto local F budget render --count 200 --samples --retire-mib 4000 --limit-mib 4096`, `--count 40 --fresh`, `budget viewport --viewport 3024x1964`. `form_bomb` is timed around `budget render --count 1`.

| | `plain` | `entitled` | `dropmach` | `selfbox` |
|---|---:|---:|---:|---:|
| `rfc9110` whole pages p50 ms | 1.90 | 1.87 | 1.85 | 1.91 |
| `rfc9110` whole pages p95 ms | 2.44 | 2.48 | 2.44 | 2.56 |
| Idle worker MiB | 2.22 | 2.34 | 2.34 | 4.98 |
| `rfc9110` peak MiB | 29.95 | 29.80 | 29.78 | 31.45 |
| Fresh worker, open, first page, p50 ms | 14.57 | 17.70 | 17.22 | 18.19 |
| Same, p95 ms | 17.52 | 19.09 | 17.78 | 20.16 |
| USGS first view ms | 554 | 562 | 558 | 553 |
| USGS scroll ms | 108 | 109 | 110 | 105 |
| USGS zoom ms | 408 | 394 | 390 | 403 |
| USGS peak first view / zoom MiB | 394 / 454 | 394 / 451 | 394 / 451 | 394 / 452 |
| USGS tiles failed or low-res | 0 | 0 | 0 | 0 |
| `form_bomb`, all steps, ms | 2479 | 2442 | 2469 | 2489 |
| `form_bomb` memory kills / heavy retries / pages failed | 2 / 1 / 1 | 2 / 1 / 1 | 2 / 1 / 1 | 2 / 1 / 1 |
| `form_bomb` peak MiB | 1026 | 1025 | 1026 | 1027 |

**The harness**, for comparing with XPC. 3 rounds for all but XPC, once for XPC. `rfc9110`, 194 pages at 2x.

| | `plain` | `entitled` | `dropmach` | `selfbox` | `xpc` |
|---|---:|---:|---:|---:|---:|
| Start to hello, p50 ms, 40 starts | 4.76 | 8.59 | 8.64 | 9.45 | **10,026** (first: 103), 3 starts |
| Start after a clean exit, ms | about 6 | about 10 | about 10 | about 11 | **10,025** |
| Gone after `SIGKILL`, p50 ms | 0.63 | 0.62 | 0.63 | 0.64 | 0.62 |
| Round trip p50 / p95 ms | 1.94 / 2.44 | 1.92 / 2.39 | 1.90 / 2.35 | 1.94 / 2.40 | 8.70 / 10.77, or 1.91 / 2.30 holding a message |
| Peak MiB | 30.0 | 29.8 | 29.7 | 31.3 | 29.7 |
| `form_bomb`, 512 MiB then 1 GiB, ms | 2325 | 2318 | 2322 | 2301 | **17,167** |

XPC's own notice of a dead service came in under 0.1 ms. The slow XPC round trip was scheduling: `setpriority(PRIO_DARWIN_PROCESS)` and a thread's QoS changed nothing, holding one XPC message open did.

**First start after the bundle moves or changes: about 80 ms, once.** `secinitd` rebuilds the profile it caches in the container when the bundle's path changes: 79.7 ms, then 11.2 and 10.7 ms for the first start of later runs. So that's once after an install or an update. The layouts share one ID here, which is what made it visible.

**The 2.76 MiB of `selfbox`** is libsandbox compiling the profile's text in the worker, for every worker. There's no public way to precompile it.

**Hardened runtime, ad-hoc:** the sandboxed helper ran with `-o runtime`, library validation switched off for this test only because ad-hoc has no team: hello p50 11.75 ms and round trip p50 3.14 ms, with other work running. No JIT entitlement was needed.

## 4. What it needs at signing time

Developer ID, the existing one. No provisioning profile should be needed, since the entitlement is plain App Sandbox. **Not tested.**

```
RectoPDF.app/Contents/
  MacOS/RectoPDF                       Not sandboxed. Hardened runtime.
  MacOS/recto                          Not sandboxed. Hardened runtime.
  Helpers/RectoRender.app/Contents/
    Info.plist                         CFBundleIdentifier io.github.kkollsga.RectoPDF.Render, LSBackgroundOnly
    MacOS/recto-render-service         Hardened runtime, the one entitlement below
    Frameworks/libpdfium.dylib         Re-signed: it arrives ad-hoc
```

The only entitlement anywhere for this: `com.apple.security.app-sandbox`, on the helper. **Not** `com.apple.security.inherit`, which is for a child of a sandboxed parent: this parent isn't. **Not** `com.apple.security.cs.disable-library-validation`, as the same team signs the library. Sign from the inside out, and don't use `--deep`.

| | Ad-hoc, verified here | Needs a real Developer ID, **not tested** |
|---|---|---|
| Sandbox, walls, limits, sequence, kill | Yes | Should be the same |
| Rendering, numbers | Yes | Should be the same |
| Nested layout, `codesign --verify --deep --strict` | Yes | |
| Library validation with hardened runtime | **Rejected**, as ad-hoc has no team ("Library Validation failed: Rejecting") | That the same Team ID passes |
| Notarization of the nested helper | | All of it |
| Gatekeeper and quarantine on a downloaded copy, a helper started with `posix_spawn` rather than LaunchServices | | All of it |
| App Translocation | | All of it |
| Whether the container accepts a new signature after a Team ID or requirement change | Ad-hoc to ad-hoc with a new hash raised no prompt | With a team |

## What wasn't tested

- Everything in the right column above. The plan's release blocker stays open until it's tested on the signed build.
- Developer ID, and any Mac but this one, or any macOS but 27. The drop leans on the kernel's description of ports, so `no_right_to_launchd_can_be_fetched_back_after_the_drop` should run on every OS we support, and it says what to look at if it fails.
- A launch of a real installed app or by `https://`, or reading the pasteboard: these would take the screen or touch the owner's data. A URL with a made-up scheme registered to the test app told me nothing: it returned -10814 even with no sandbox, so `ls_open_url` in the probe proves nothing yet.
- A bare tool with an `Info.plist` embedded in the binary rather than a nested `.app`, or a tool beside the main app's binary: either could end up under the main app's identity and make a new container, so I didn't try them. A simpler layout may be possible.
- User-installed fonts (section 1). Encrypted PDFs in the sandbox.
- Many workers starting at once: whether `secinitd` makes them queue.
- A real exploit. The probe calls the APIs; it's not a memory-corruption attack, and it runs its attempts before any PDF is open, in a small process.
- The Mach drop inside XPC, or under real memory pressure.

## To build it

Small, and behind the launch seam. No other change to `DocRenderer`, the protocol or the limits.

1. **Worker:** after `Pdfium::load`, if asked by an argument the host passes when bundled: test the wall, drop the rights, or exit. Into `recto-platform`, beside `harden_this_process`.
2. **Host:** `ServiceConfig` finds the helper in the bundle, following a link from `recto` on `PATH` to get there, so the app and `recto` use the same worker.
3. **Packaging:** the layout above, in the app build `.worktrees/app` is working on.
4. **Tests:** move `walls.rs` into the workspace, once a signed build exists to run it on.

## Left on this machine

- **No new container.** `~/Library/Containers` still has `io.github.kkollsga.RectoSpike` and `no.rectospike.render`, and the unrelated `io.github.kkollsga.Seedwell.FPSpike.Ext` and one Apple entry. `no.rectospike.render`'s metadata now points at this spike's last bundle path, which took it from 36 KB to 48 KB in all. Nothing is left in its data folder's `tmp`.
- **Crash reports** in `~/Library/Logs/DiagnosticReports`, from deliberate crashes: `recto-render-service` (2), `plainprobe` (6), `sandbox-exec` (2), `sandbox-probe` (1), `xpc-service` (3), `k` (1), `python3.14` (1). Not deleted, as another agent's reports sit beside them. The `hostile-*` reports there were from the test suite running through its crash cases, which is what those tests do.
- **LaunchServices database:** entries for `no.rectospike.escape`, `no.rectospike.render` and `no.rectospike.xpchost` from launches, at scratch paths. The test unregisters its own now, and the earlier ones should clear once the paths are gone.
- Scratch files and layouts were in the session's scratch folder, which goes with the session.
