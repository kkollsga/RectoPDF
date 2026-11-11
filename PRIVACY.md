# Privacy

RectoPDF reads your PDFs on your Mac. It has no account, no analytics, no telemetry, no ads and no crash reporter of its own, and it sends nothing about your documents anywhere.

This describes RectoPDF 0.1.0.

## What it sends

**One update check a day.** RectoPDF asks GitHub, where it's published (github.com/kkollsga/RectoPDF), whether there's a new release. It's an ordinary download of one small file, made by macOS's own `curl`. It says the app's name and version, such as `RectoPDF/0.1.0`, and nothing else: no ID, no cookies, nothing about your Mac or your documents. GitHub sees the request come from your internet address, as any website would, under GitHub's own privacy statement. The first check comes a minute after launch, then one a day. A check that fails is tried again an hour later, then after two hours, and so on, up to a day.

The check runs whether or not **Update automatically** is on. With it on, once you haven't touched the app for 30 minutes, RectoPDF downloads the new version from the same GitHub release and installs it. With it off, the new version is offered in the RectoPDF menu, and nothing is downloaded until you choose it. A download is installed only if its checksum, its Developer ID signature and Gatekeeper all pass.

**Links, when you choose them.** A web or email link in a PDF opens only after you click it and confirm on a sheet that shows the full address. Then your browser or mail app opens it. PDFs can't make RectoPDF go online: JavaScript in a PDF never runs, and pages are drawn in a sandboxed helper with no network access.

That's all. Generated contents (headings found in a PDF that has no usable contents) are made on your Mac.

## What it keeps on this Mac

All of it is in your user folder, readable only by you.

| What | Where | For how long |
|---|---|---|
| Your session: open tabs, where you were in each, the window, the last thing you searched for, Back and Forward, and the 20 files you opened last | `~/Library/Application Support/RectoPDF/agent/session/` | Until it changes. If a session can't be read, up to three unreadable ones are set aside beside it |
| Unsaved edits and undo history, per PDF. Never written into the PDF | `session/journals/` in that folder | While the PDF is open, then 30 days, within 1 GiB in all |
| A small picture of where you were in a tab that's gone idle, and its contents, so it comes back at once | `session/journals/` | 30 days, within 256 MiB in all |
| One backup of a PDF as it was before you first saved it | `~/Library/Application Support/RectoPDF/agent/backups.noindex/` | 30 days, within 1 GiB in all |
| Page text of PDFs you searched with Find, so the next search is fast | `~/Library/Caches/RectoPDF/corpus.noindex/` | 90 days after it was last used, within 2 GiB in all. A changed file's old text goes |
| Generated contents you asked for | `~/Library/Caches/RectoPDF/contents.noindex/` | 90 days, within 32 MiB in all |
| Update settings, when it last checked, and what's on offer | `~/Library/Application Support/RectoPDF/agent/updates.json`, `update.log` and `update/` | Until they change. A copy of your session from just before an update is kept for a week |
| The previous version of the app, to go back to if a new one won't start | `.RectoPDF-update`, a hidden folder beside the app, usually in `/Applications` | Until the next update |
| Agent access and its activity log (below) | `~/Library/Application Support/RectoPDF/agent/` | The log keeps at most 6 MiB, the oldest going first |

The edit history, the pictures, the backups, the page text and the generated contents are kept out of Time Machine, and the page text and generated contents out of Spotlight.

**Passwords are never kept.** A password-protected PDF asks again after a restart. Its page text isn't kept for searching, and no picture of its pages is kept.

macOS also keeps a few things of its own for any app, such as the folder you last opened a file from.

## Other programs (agent access)

The `recto` command lets programs on your Mac, such as a script or an agent you run, read and work with the PDFs open in RectoPDF. It talks to the app through a private connection on this Mac that only your user account can open. RectoPDF never listens on the network.

Agent access is **Off** until you change it. You raise it by typing `recto permission set <level>` in your own terminal, which then asks you to type back a word that's different every time, so a program that runs `recto` can't raise it for itself. Lowering it needs nothing. Signing is never allowed at any level.

| Level | What a program may do |
|---|---|
| Off | Status, help, and asking for more. It can see how many PDFs are open, and nothing from any of them: no names, no page text and no drawing. |
| Read-only | Read and search documents that are open, and watch what happens. Changes no document. |
| Control | Open and close PDFs, move around, edit pages in memory, save edits by adding them to the end of a file or as a new file, check for updates, and throw away agents' own edits. Never the user's edits. |
| Full | Everything Control can, and write exports and pictures to disk, save over another file or rewrite one, throw away the user's edits too, install updates, change the update settings, and choose a worker's limits. Signing is never included. |

Whatever a program reads from a PDF is then up to that program. Every command a program sends goes in the activity log: what it was, when, at which level, and how it went, with file names but never document text.

## Deleting it

Quit RectoPDF, then move these to the Bin:

- `~/Library/Application Support/RectoPDF`
- `~/Library/Caches/RectoPDF`
- `.RectoPDF-update`, beside the app (in Finder, ⇧⌘. shows hidden folders)

That removes everything above, including unsaved edits and undo history, so save what you want to keep first. To clear only the list of recent files, choose File > Open Recent > Clear Menu.

To remove RectoPDF itself, move RectoPDF from Applications to the Bin as well.
