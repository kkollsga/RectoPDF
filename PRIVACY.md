# Privacy

RectoPDF reads your PDFs on your Mac. It has no account, no analytics, no telemetry, no ads and no crash reporter of its own, and it sends nothing about your documents anywhere.

This describes RectoPDF 0.1.3.

## What it sends

**One update check a day.** RectoPDF asks GitHub, where it's published (github.com/kkollsga/RectoPDF), whether there's a new release. It's an ordinary download of one small file, made by macOS's own `curl`. It says the app's name and version, such as `RectoPDF/0.1.0`, and nothing else: no ID, no cookies, nothing about your Mac or your documents. GitHub sees the request come from your internet address, as any website would, under GitHub's own privacy statement. The first check comes a minute after launch, then one a day. A check that fails is tried again an hour later, then after two hours, and so on, up to a day.

The check runs whether or not **Update automatically** is on. With it on, once you haven't touched the app for 30 minutes, RectoPDF downloads the new version from the same GitHub release and installs it. With it off, the new version is offered in the RectoPDF menu, and nothing is downloaded until you choose it. A download is installed only if its checksum, its Developer ID signature and Gatekeeper all pass.

**Links, when you choose them.** A web or email link in a PDF opens only after you click it and confirm on a sheet that shows the full address. Then your browser or mail app opens it. PDFs can't make RectoPDF go online: JavaScript in a PDF never runs, and pages are drawn in a sandboxed helper with no network access.

That's all. Generated contents (headings found in a PDF that has no usable contents) are made on your Mac.

## What it keeps on this Mac

All of it is in your user folder, readable only by you.

| What | Where | For how long |
|---|---|---|
| Your session: open tabs, where you were in each, the window, the last thing you searched for in Find and what you'd typed there and not run (each up to 4 KiB), Back and Forward, and the 20 files you opened last | `~/Library/Application Support/RectoPDF/agent/session/` | Until it changes. If a session can't be read, up to three unreadable ones are set aside beside it |
| Unsaved edits and undo history, per PDF. Never written into the PDF. Each file names the PDF it's about: its path is in the file's header | `session/journals/` in that folder | While the PDF is open, then 30 days, within 1 GiB in all |
| Pages a save took out of a PDF, kept so the undo history can bring them back. Whole pages, as they were | `session/journals/` | 30 days after the save, within 1 GiB in all with the edit history, looked at when RectoPDF starts. Not removed while a tab's edit history still needs them |
| Which page is which, for the edit history to follow a file that changes: a digest of each page, nothing of its text | `session/journals/` | As long as the edit history it belongs to, and removed with it |
| For a tab that's gone idle, its contents and the sizes of its pages, so it comes back at its place at once. No picture of any page | `session/journals/` | 30 days, within 256 MiB in all |
| One backup of a PDF as it was before you first saved it | `~/Library/Application Support/RectoPDF/agent/backups.noindex/` | 30 days, within 1 GiB in all |
| Page text of PDFs you searched with Find, so the next search is fast. **Stored encrypted** (below) | `~/Library/Caches/RectoPDF/corpus.noindex/` | 90 days after it was last used, within 2 GiB in all. A changed file's old text goes |
| Generated contents you asked for | `~/Library/Caches/RectoPDF/contents.noindex/` | 90 days, within 32 MiB in all |
| Update settings, when it last checked, and what's on offer | `~/Library/Application Support/RectoPDF/agent/updates.json`, `update.log` and `update/` | Until they change |
| A copy of your session from just before an update, to go back to if the new version won't start: everything in `session/` but the histories of PDFs you'd closed, so it holds the edit history and kept pages too | `update/before/` in that folder | A week |
| The previous version of the app, to go back to if a new one won't start | `.RectoPDF-update`, a hidden folder beside the app, usually in `/Applications` | Until the next update |
| Agent access and its activity log (below): each command a program sent, with its time, level and outcome, and the tab, the page numbers and the name of any file it gave (the name, not the folder). Never document text, and never the words of a search | `~/Library/Application Support/RectoPDF/agent/`, in `activity*.jsonl` | The log keeps at most 6 MiB, the oldest going first |

Kept out of Time Machine: the edit history (with the idle tabs' contents, the pages a save took out and the page identities, all in `session/journals/`), the copy of your session kept for an update (`update/before/`), the backups, which files RectoPDF opened and saved (`removal/`), the page text and the generated contents. Out of Spotlight: the backups, the page text and the generated contents. The rest of your session (the open tabs, the files you opened last, where you were, and what you searched for in Find) is not kept out of Time Machine, so a backup can hold it.

**No picture of any page is kept.** A tab you're not reading comes back from its PDF, its pages in place as they're drawn. Pictures that an earlier version kept are removed the first time this version starts.

**Passwords are never kept.** A password-protected PDF asks again after a restart. Its page text isn't kept for searching, and nothing of its pages is kept for when it's idle.

**The page text is stored encrypted** (AES-256-GCM), each PDF's under a key of its own, made from one key that RectoPDF keeps in your Mac's data-protection keychain, in an access group only the signed RectoPDF release can use. On the machine where this was measured (a build runner, not yet a Mac of a person's), a program that isn't that release, whether unsigned, signed ad hoc, or another program signed by RectoPDF's own developer, was refused the key, and refused putting a key of its own or changing or removing the key. So another program you run, a script or an agent included, can't read the page text, and it can't read the key. It can still see that a folder is there for a PDF (named by a hash of the file's identity, not its name or path), how big its files are and when they were written, which shows how many pages there are and how long each page's text is. And, running as you, it can delete or damage the folders: RectoPDF then builds that text again at your next search, and never believes text it can't open. The same holds for the title and outline kept with it. What the folders hold is unchanged: the same PDFs, never one that needed a password, for the same 90 days and 2 GiB.

Two cases are different. **A copy of RectoPDF that isn't the signed release** (one someone built themselves), or a Mac where the keychain can't be used at the time, keeps the key in memory only, so the page text it makes can't be opened after RectoPDF quits and is built again the next time you search. **`recto index --agent`**, which you or a program ask for, also writes a folder of one plain Markdown file per page of that PDF beside the encrypted text, for a program to search with its own tools; any program you run can read it while it exists. It's set aside when RectoPDF quits, and removed the next time RectoPDF runs.

**Page text kept by RectoPDF 0.1.0 was plain.** It's removed the first time the new version starts, and each PDF's text is made again, encrypted, when you next search it.

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

Whatever a program reads from a PDF is then up to that program. Every command a program sends goes in the activity log: what it was, when, at which level, and how it went, with file names but never document text or the words of a search.

## Deleting it

Quit RectoPDF, then move these to the Bin:

- `~/Library/Application Support/RectoPDF`
- `~/Library/Caches/RectoPDF`
- `.RectoPDF-update`, beside the app (in Finder, ⇧⌘. shows hidden folders)

That removes everything above, including unsaved edits and undo history, so save what you want to keep first. To clear only the list of recent files, choose File > Open Recent > Clear Menu.

The key the page text is encrypted with stays in your keychain when you do this; it opens nothing once the folders are gone. To remove RectoPDF itself, move RectoPDF from Applications to the Bin as well.
