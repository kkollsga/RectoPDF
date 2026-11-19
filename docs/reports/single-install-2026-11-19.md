# Duplicate Dock icons, 19 November 2026

The owner reported two RectoPDF icons during preparation of 0.1.3. A bounded CoreServices lookup of `io.github.kkollsga.RectoPDF` returned exactly `/Applications/RectoPDF.app`; Spotlight returned that path too. No RectoPDF window process was running. This was not evidence of two registered installations.

The Dock's `recent-apps` preference contained two RectoPDF records, both naming `file:///Applications/RectoPDF.app/` and the same bundle identifier. Only the redundant record was removed, preserving the other recent entries and all other Dock settings. The preference was backed up before the change. The user's Dock was restarted to reload it, and a fresh preference read confirmed one RectoPDF entry. The installed bundle and its documents were untouched.

Evidence is in `/tmp/rectopdf-release-0.1.3-20261119/dock-cleanup/`: `before.plist` and `verified.json`. The desktop observation API did not provide a usable Dock image, so verification is of the restarted Dock's stored records and CoreServices registrations, not a visual icon count.

The existing automated-reopen safeguards remain necessary: test copies opened through Launch Services use `LSUIElement` so they cannot create lingering regular-app tiles (`crates/recto-cli/tests/common/app.rs`). `--build-kind` exits before creating `NSApplication`; ordinary quiet launches choose the prohibited activation policy before the delegate. Source inspection does not establish which operation created these two recent-app records. Do not claim that removing the record fixes its unknown cause. Release checks must keep their private bundles quiet and verify their cleanup, and the Dock count is checked again after native release testing.

After publishing0.1.3 and retiring the final signed test app, the 12:39:17 UTC bounded CoreServices query again returned exactly`/Applications/RectoPDF.app`. Dock main-app entries were empty. The original installed app was preserved; publication did not install another copy.
