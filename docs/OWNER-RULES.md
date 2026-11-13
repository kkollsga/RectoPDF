# What the owner has decided

The owner's product rules and one-off decisions, in their words, with the date. Moved here from `AGENTS.md` on 2026-11-13 so that file stays short: it is loaded into every session and read by every worker. Read the section that touches your task; behaviour as built is in `SPEC.md`. A newer rule that replaces an older one says so.

## Product rules from the owner (2026-11-13)
- A tab that leaves the screen at a place it has no picture of wakes to a bare page. Asked whether the place should be drawn first, at the cost of a render nobody asked for and a few more bytes written: "I think it makes sense, and extra draw is not costly right?" So it is built, and the answer to the question is owed as numbers: it draws only when the place has no picture, while the tab's worker is still there (no file is opened again for it), one tab at a time, and its CPU, memory and bytes written are measured before and after.
- Sleep files and a cool-down. With a tab pushed out of the three warm ones getting its sleep file written at once, and a tab left again inside the 60 seconds between writes waking to an empty pane, the owner asked: "Should we have a cool down period before a tab becomes inactive?" The coordinator's recommendation: the push out of the warm ones stays immediate (a cool-down there means more than three tabs warm, each with its open document and up to 4 MiB of pictures); the cool-down is on the write: a pushed-out tab's sleep file waits about 20 seconds in the queue there is (at most 8 tabs and 16 MiB), is dropped if the tab is shown again, and is written at a quit or on going to the background; a crash inside the wait costs that tab only its quick way back, never its place or its edits. Owner: "I agree with your recommendation". The design is reported with numbers (writes saved in a scripted run through tabs, memory held while waiting) before it is built, and whether 1 MiB of sleep files an hour is enough for ordinary switching is measured with it.
- Asked what happens when an open file is modified by something else: "I assume we don't have to lock the file, we can simply load the new version on the fly". So no lock, ever; a new version is read when it's noticed, as `SPEC.md` already says. Then, told that unsaved edits against a new version with other pages hadn't been verified: "For unsaved edits against changed page count, we need to run a page matcher of some kind. That's the way we can ensure edits on the correct page". So when a file with unsaved edits changes on disk, each edit is matched to its page in the new version by what the page is, not by its number; an edit whose page can't be found is not applied to another page, and that is said.
- Asked for the coordinator's recommendations on three open decisions, and given them: "I agree, go ahead". So: (1) of the operation types that would take `Session` and `Reader` to the 60-method budget, two are done inside phase R (`Session`'s journal desk and sleeper) and the rest wait until after the release; phase R ends Monday 16 November, with `Session` at about 103 and `Reader` at about 82. (2) The page matcher is built after phase R and before the release, as proposed: each edited page's identity recorded when the edit is made, matched by object id and then by content, an edit whose page isn't found parked and said, never moved to another page. (3) A window that doesn't get the record's lock at its start tries again quietly for about ten seconds and starts remembering once it has it; another RectoPDF really keeping the state is said as it is today. Then a second pass of the real app with screenshots for the owner, and 0.1.1 on the owner's word. The same day: "I wont have time to do manual tests before we finish the r project", so what a person would check by hand is shown by agents in the real app, in quiet runs, with screenshots the coordinator has looked at.
- Shown dark paper on a page whose photo covers 53% of it, dimmed whole to light grey by the rule that a page that's mostly a picture isn't turned over, and asked whether such a page should flip its paper: "The image should flip if the background is mostly white". So what decides is the background, not how much of the page a picture covers: a page whose background is mostly white is turned over on dark paper however large its picture, with a photo keeping its own colours; and a picture whose own background is mostly white is turned over with it, as the rule of 2026-11-07 says ("similar with images with white bg"). The coordinator's reading, not the owner's words: a large vector drawing on white paper is turned over with its page as today, since it has no picture to keep.
- Asked why every decision was being written into `AGENTS.md`: "why are you writing everything into agents.md? Isnt agents.md loaded into all sessions?" So product rules and one-off decisions live in this file, and `AGENTS.md` keeps only how agents work.

## Product rules from the owner (2026-11-06)
- "We need to maximize vertical reading space." No top bar; nothing above or over the page.
- "the left sidebar menu is a bit too complicated (visually noisy)". Names and contents at rest; details on hover.
- "I prefer the agent requesting page info when it needs rather than us providing it."
- "We build everything in one go, but we test as we build."
- "Remember performance and low memory use (even when many large pdfs are loaded is crucial."

## Product rules from the owner (2026-11-07)
- "The left sidebar toc should have a max height within scroll so it doesn't push everything of screen".
- "The agent logo in the upper right is wrong, it should be a ai 4 point star".
- "The toc should auto scroll to where you are in the document (smoothly so you don't notice it in a elegant way)".
- "It can whoosh with smooth acceleration at both ends. It can have a \"dead-zone\" so it doesn't react to small scrolls within the window, so basically it repositions, stays still (while user is reading slowly scrolling) then repositions after the user is scrolling a bit."
- "We have two paper color modes: white and dark, let's add a yellowed white option that's better for the eyes".
- "Im seeing an issue: in dark mode scanned pages appear bright white: We need to detect scanned images (with predominently white bg, and flip and match grey tone paper color".
- "similar with images with white bg" (dark paper: an image with a white background flips with the page, as a scan does).
- "Lets style the scrollbar better. Lets use the mac native option, as seen in pages".
- "I think option 4 is the best one, dont modify the bad toc, but offer a generated toc using distillpdf".
- "To save horizontal space the scrollbar can stop below the agent icon. Add an option in the macos top bar to turn off agent bar (hides the icon), enable + - and command - scroll for zooming".
- "And select text should work natively".
- "For rectopdf we need a update feature similar to what we added to seedwell. Can you have a look and implement a similar handsoff system? For us we can add a check for updates button in the top bar, that turns into a update to… button if new version has been released. App settings should have a auto update toggle, and we check for updates once per day. It's crucial that app state is perfectly recovered before and after update".
- "Actually it should recover state so perfectly that it can update while the user is using it. But for auto update let's update in a window the user hasn't touched the app for 30 minutes".
- "I was thinking we need to reduce SSD wear as much as we can, while of course keeping a good memory budget." Edits come in bursts: "usually when people are editing they will edit a lot of things, so we need to make sure we keep the budget".
- "We want a good balance between SSD-aware performance and memory use for warm PDFs that's actively being edited. It's fine to keep in memory for a while, and then as it goes dormant we store to disk."
- "When it's on deep sleep, we want its state to be stored on disk, not on memory. But it needs to be stored in a way that's quick to recover without any delay."
- "and edit should also keep undo, redo history of edits until save has been applied."
- "We don't want to add the undo redo history to the file itself, but we can have a rolling save redo history on the system, so it kind of in the temp files per PDF."
- "1000 steps and 64mb is good." On Save: "after a save you might still have regrets and want to make changes. But you definitely don't want history to leak into the pdf file".
- "II agree with this signature approach": a change that needs a rewrite of a signed PDF (delete a page, redact, compress) turns Save off, and Save As makes a new copy with every signature fully removed, field, value and drawn box, saying "The new copy won't be signed. The original keeps its signature." Additions are appended and keep the signature valid. The signed original is never touched.
- "All in one release." Asked which: everything ships together, not the reader first. Replaced on 2026-11-08: "Lets release after phase 3. the rest we tackle after". The first release is the complete reader (phases 2 and 3) with the release work (phase 7). Saving and editing, tools and the agent sidebar (phases 4 to 6) come after it. Then, the same day: "No we release everything we have started in the first release. We don't park anything (finish on going features)." So a feature already under way, such as Save stage 2, is finished and ships; what hasn't started waits. And: "But don't start new post phase 3 features before release". No new feature from phases 4 to 6 starts until the first release is out.
- "it's clear to me that the test program is slowing us down considerably. I need your help for us to speed up safely".
- "Two tabs of the same pdf? This shouldn't be possible. A pdf can only be opened once".
- "I agree, but license mit is fine" (to the coordinator's recommendations, listed in `docs/PLAN.md` under Decisions made).

## Product rules from the owner, first use of the restore build (2026-11-07)
- "We dont need detailed zoom options in the left sidebar menu. Zoom is pretty natural to people. Instead lets show the zoom level in upper left and clicking the zoom value enables you to type in zoom level or reset."
- Replaces where the zoom level goes: "The zoom level should be in the upper right of the app (not in the sidebar)".
- "Focus mode enable disable should be in the right click menu. In the right click menu you can select side by side layout, with and without header page."
- Replaces the focus part of that: "Actually instead of focus view, simply have a sidebar icon to the right of the traffic light buttons that toggles sidebar on and of. No need mor a separate mode". So there is no focus mode; a sidebar button beside the window controls shows and hides the sidebar.
- Contents: "The hover delay before showing the full toc name makes in not intuitive, and the expanded name should have the color of the title." "there should be a collapse toggle button for sections with sub chapters on the left side of the headers." Page numbers on the right that don't make sense, and section indents, are bugs.
- "When selecting across pages the page number is copied. Both footer and header should be ignored when coppying body text."
- "The page number should always be visible in the lower right." Replaced on 2026-11-08 by "Zoom level should auto hid 3s after last zoom change, and similar behaviour for page number": the page number shows when the place changes, and fades 3 s after.
- "The font in the future competitiveness is not super readible. I think its a bit to skinny. Can you check?"
- Dark mode: images with a white background should be dark, as a chart with a white background was left light.
- "The full app settings should be accessible from file -> settings"
- "The sidebar menu is more for tools, like edit mode etc. When not in edit mode rotate page will rotate all pages together (it still enables save to commit the adjustment). Usefull when there is a scan issue or similar, for a quick fix."
- "The margin left of the ai icon is too large. It should have the same margin to the right and above. Above should have slightly smaller margin then now (and right gets the same one)."
- "and improve the styling of the scrollbar. The sidebar thumb should always be visible, and lets hide the rail."
- "the thumb should be thinner": slimmer than the system's default thumb, in the sidebar and the page area.
- "add some margin above the first page, and below the last page". Replaced on 2026-11-08 by "The margin above first page, and below last page should be 50% of page height".
- "the scroll thumb always visible, the rail always hidden": for the page area too, whatever the system's scroll bar setting.
- "Ctrl clicking more then one pdf should enable comparison view, right click menu can enable separate scroll (default) and synced scroll. Zoom is always synced."

## Product rules from the owner: copying and printing protected PDFs (2026-11-08)
- Exports of a copy-restricted PDF ask "anyway" like Copy. Asked whether SVG export, PNG render and print-to-PDF should get the same step: "Yes, same step". Agents need `--anyway`, and it's logged.
- Print ignores a PDF's no-print flag. Asked what Print should do with a file that says it may not be printed: "Ignore the flag". No notice and no refusal.

## Product rules from the owner, using the full reader build (2026-11-08, "RectoPDF - User observations")
- "Scroll bar track is quite wide. Can you make it thinner with a small margin on the right side so its centered on the thumb?"
- "Text selection should be the default tool (not camera tool), enable camera tool via hotkey or sidebar menu."
- "Im not seeing check for updates button in the macos top bar menu."
- "When sidebar is hidden, I want the traffic light buttons and sidebar toggle to autohide. To show them again you hover above them."
- "Zoom level should auto hid 3s after last zoom change, and similar behaviour for page number."
- "The margin above first page, and below last page should be 50% of page height"
- "When you are testing the app it flashes open in lower left corner. Is it possible to test the app in the background? Ie headless?" A quiet run must never show a window on the screen, not even for a moment (rule: quiet runs never take the screen).

## Release gate: memory soak (2026-11-11)
- "24 hour memory soak? We don't have time for that". Asked what replaces it, the owner first chose a 2-hour fast soak, then: "Actually let's skip the soak". So 0.1.0 releases on the existing memory tests (per-feature budgets, 1,000 tabs, the leak tests), with no soak. The 24-hour soak runs after the release, and growth it finds goes into 0.1.1.

## Release signing (2026-11-11)
- "I dont want any manual steps." The `release` Environment has no required reviewer; its keys reach only workflows from `main` or a `v*.*.*` tag. The owner changed that setting themselves (the coordinator's own change was blocked as weakening security). Publishing a release stays the owner's decision, given in chat, not a click.

## Release 0.1.0 (2026-11-11)
- "I preauthorize the 0.1.0 release once everything is done". This covers pushing the `v0.1.0` tag and publishing its draft, once: the open release work is merged, `tools/check.sh --full` passed on the commit, CI is green on it, and the release workflow's draft built, signed, notarized and passed its checks. If any of those fails, nothing is published; it's fixed, and the tag is made again. It covers 0.1.0 only.

## Release 0.1.1 (2026-11-13)
- Asked whether the fix for a quiet run giving an agent Full on the user's own files (in 0.1.0 as released) should go out as 0.1.1 as soon as it has merged: "no lets wait until we are done with R before we release". So nothing is released during phase R, security fixes included: they merge to `main` and wait. 0.1.1 is prepared when phase R is done, and publishing it is still the owner's word then.

## Product rules from the owner: privacy (2026-11-11)
- Asked to approve `PRIVACY.md`, the quiet line in the empty window about the daily check, and the Settings hint and link: "Approve as written".
- Asked whether the daily update check should get a real off, as it runs with *Update automatically* off too: "Keep as is". One anonymous request a day, always; said in Settings and `PRIVACY.md`.
- Asked whether an agent at Control may save edits into a PDF by appending (the coordinator's default until then): "Keep at Control".

## Product rules from the owner, before the first release (2026-11-08)
- The contents helper (Generate contents, through distillPDF) reads untrusted PDFs outside the renderer's sandbox. Asked what 0.1.0 should do: "Turn its image decoding off". Headings come from text.
- The ✦ agent icon does nothing until the agent sidebar ships. Asked what 0.1.0 should do with it: "Keep it as it is".
- The owner's dev copy (`target/dev/RectoPDF.app`): "You can swap out rectopdf even if its running (as long as its not actively beeing used, remember the cool down rules)" (2026-11-11). Then: "for dev work, lets set the limit to 5 mins" (2026-11-11). So the dev copy is swapped after 5 minutes with no use; the product's own automatic updates keep their 30 minutes. With agent access off, the coordinator goes by the Mac's own idle time (HID idle ≥ 5 min), which can only be longer than the app's.
- Signing (owner, 2026-11-11: "Do we need to sign dev copies? Is it possible to only sign when we push to git? This has been an issue that slows down dev work on seedwell"): development builds are never notarized and never get a secure timestamp. Then: "The reason why it slows down dev is because I need to approve the use of the keychain, that in some cases takes hours". So a development build never touches the keychain: it's signed ad hoc with a designated requirement naming only the bundle identifier, so macOS keeps its folder permissions across dev builds. Developer ID signing and notarization happen only in CI, when a release tag is pushed.
- "Im seeing two rectopdf installs in macos. Can you check? We should only ever have one. We should enable dev updates (similar to the seedwell feature)." (2026-11-11). The one install is `/Applications/RectoPDF.app`, and development builds go in through the updater (SPEC decision 28, `tools/dev-build.sh`, `recto update install <zip>`).
- Installing a dev build needs agent access Full. Asked how dev builds should reach their copy, which runs at Off, the owner chose "Raise my copy to Full" (2026-11-11). So the owner's copy runs at Full, and agents install dev builds after 5 idle minutes. Then: "you have permission to set to full and leave it there. I will update at a later stage, when we are done with development". The coordinator sets it to Full and leaves it; the owner lowers it themselves after development.

## Product rules from the owner (2026-11-12)
- "when sidebar is hidden the traffic light buttons and sidebar toggle doesn't appear at all, they should appear when hovering over the the left corner."
- "In some pdfs the pages aren't centered (with equal black gutters on both sides), pages area should always have equal black gutters on both sides to appear centered, with and without sidebar, and ui elements like zoom and page number shouldn't impact this."
- Phase R (the restructuring, `docs/PLAN.md`): "Yes you have the goahead". The goal, the same day: "after this project is done the codebase is easily maintainable and coding projects are efficient and fast. Leading to rapid and robust development and making it easy and cheap to expand features."
- Agent access on the owner's copy, asked whether the coordinator should set it back to Full after finding it Off: "Raise it permanently". The app itself refuses an agent's raise ("Raising agent access needs the user, at a real terminal"), and the coordinator doesn't go around that: the owner runs `recto permission set full` once in their own terminal, and it stays at Full. Found Off again, the coordinator asks the owner to run it and doesn't ask whether.
- With a build ready and waiting for its five idle minutes: "I dont see the update now button in settings." Settings shows a waiting update with a button that installs it now. The same message, for that one install: "please go ahead, overrule the 5min limit this time" (once; the five-minute rule stands).
- With `main` red on CI: "The red ci issue is something we need to fix properly. What's the solution to ensure we are able to debug and fix issues after one failure? Run targeted tests locally, reproduce errors etc". So one failure must be enough: a failing test leaves its evidence, says how to run it again here as CI ran it, and CI measures its rate the first time (PLAN R0.7).
- Asked whether CI's machine should be copied here by a macOS virtual machine (faithful: a clean user and Launch Services, 40 to 60 GB of disk, 7 GB of memory) or by limits on the process (nothing installed, but no clean user): "Lets do option 2". No virtual machine and nothing installed; CI's conditions are approximated with process limits.
