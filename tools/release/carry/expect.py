"""What check_carry.py expects of a pair that turns on what the older release of the pair can read: each fact stated once, with the version it was
first true of, and each expectation a function of the states the runs left. Pure: no app is run here, so made states test it (expect_test.py).

Each function returns the checks it makes, as (what, good, detail), for check_carry.py to say.
"""

import json

# What an older release does, by the first version it was true of. A pair's expectations turn on these, not on the pair's names.
# Highlights, whose journal is version 2. Before: an app leaves a version-2 journal alone byte for byte, shows that tab without its edits and
# says `edits_unreadable`, and an edit made there starts a journal of its own (version 1) beside it.
READS_VERSION_2_JOURNALS = (0, 1, 1)
# Sleep files of facts only (format 4), which a newer build wakes a tab from. Before: they held pictures of pages, which a newer build never
# reads and removes at its first read, so each tab that slept wakes once from its PDF.
SLEEPS_WITHOUT_PICTURES = (0, 1, 1)


def version(text):
    """`0.1.1` as (0, 1, 1); a development build's fourth part is left out, as it has the release's code."""
    return tuple(int(part) for part in text.split(".")[:3])


class Older:
    """The older release of a pair, as the expectations need it."""

    def __init__(self, text):
        self.v = text
        self.reads_version_2_journals = version(text) >= READS_VERSION_2_JOURNALS
        self.sleeps_without_pictures = version(text) >= SLEEPS_WITHOUT_PICTURES


def sleep_format(data):
    """A sleep file's own format, from its meta: 0 for one written before the format was said (0.1.0's)."""
    try:
        header, rest = data.split(b"\n", 1)
        json.loads(header)
        meta, _ = json.JSONDecoder().raw_decode(rest[:1 << 20].decode("utf-8", errors="replace"))
    except (ValueError, IndexError):
        return None
    return meta.get("sleep_format", 0) if isinstance(meta, dict) else None


def journal_version(data):
    try:
        return json.loads(data.split(b" ", 1)[1].split(b"\n", 1)[0]).get("version")
    except (ValueError, IndexError):
        return None


def journals(files):
    return {p: d for p, d in files.items() if p.rsplit("/", 1)[-1].startswith("journal-")}


def b_sleep(older, new_v, wakes, slept, old_sleeps, a_files, b_files):
    """B: what the new build does with the older release's sleep files. `wakes` is each tab's last wake when tapped, `slept` the tabs A put to
    sleep, `old_sleeps` A's sleep files and their formats."""
    old_v = older.v
    if not older.sleeps_without_pictures:
        left = [p for p in old_sleeps if p in b_files and sleep_format(b_files[p]) == old_sleeps[p] and b_files[p] == a_files[p]]
        return [
            (f"B: by design, no tab woke from {old_v}'s sleep pictures (each from its PDF, once)",
             all(w.get("from_sleep") is False for w in wakes.values()), json.dumps(wakes)),
            (f"B: by design, {old_v}'s sleep files ({len(old_sleeps)}, format {sorted(set(old_sleeps.values()))}) are gone once read", not left, ", ".join(left)),
        ]
    return [
        (f"B: {old_v}'s sleep files are of facts only ({len(old_sleeps)}, format {sorted(set(old_sleeps.values()))})",
         bool(old_sleeps) and all(f is not None and f >= 4 for f in old_sleeps.values()), json.dumps(old_sleeps)),
        (f"B: each tab {old_v} put to sleep ({', '.join(f'#{t}' for t in slept)}) woke in {new_v} from its sleep file",
         all(wakes[t].get("from_sleep") is True for t in slept), json.dumps(wakes)),
    ]


def c_highlighted_tab(older, new_v, tab, highlights):
    """C: the older release on the new build's state, on the tab with the highlight the new build made. `highlights` is its answer to
    `highlights` for that tab, or None where it has no such command."""
    notice = (tab.get("file_notice") or {}).get("code")
    if not older.reads_version_2_journals:
        # By design (docs/RELEASING.md, A bad release): a version-2 journal is left alone rather than read in part, and the tab says so.
        return [(f"C: by design, the highlighted tab shows the file without its edits and says why ({notice})",
                 tab.get("edited") is False and notice == "edits_unreadable", json.dumps(tab)[:300])]
    listed = len((highlights or {}).get("edits", []))
    return [(f"C: {older.v} reads {new_v}'s version-2 journal: the highlighted tab shows its edits, the highlight among them ({listed}), and says nothing ({notice})",
             tab.get("edited") is True and notice is None and listed == 1, json.dumps(tab)[:300])]


def e_journal(older, new_v, version_2, e_files):
    """E: the older release's edit on the highlighted tab, and what it did to the new build's version-2 journal (`version_2`, as D left it)."""
    if not older.reads_version_2_journals:
        kept = [p for p in version_2 if e_files.get(p) == version_2[p]]
        return [(f"E: {older.v}'s edit left {new_v}'s version-2 journal on disk byte for byte ({len(version_2)})", len(kept) == len(version_2) == 1,
                 f"journals now {[(p, journal_version(x)) for p, x in journals(e_files).items()]}")]
    grew = [p for p, d in version_2.items() if p in e_files and journal_version(e_files[p]) == 2 and len(e_files[p]) > len(d) and e_files[p].startswith(d)]
    return [(f"E: {older.v}'s edit went on {new_v}'s version-2 journal, which is still version 2 and starts with what was there ({len(version_2)})",
             len(grew) == len(version_2) == 1, f"journals now {[(p, journal_version(x), len(x)) for p, x in journals(e_files).items()]}")]


def set_aside_as(path):
    """The store's name for a list it keeps and won't read (`.not-merged` for `.log`, as Path::with_extension makes it)."""
    return path.rsplit(".", 1)[0] + ".not-merged"


def f_disk(older, new_v, shown, code, version_2, own, f_files):
    """F: the new build again, after the older release's edit, and what's on disk. `shown` is whether the highlight is listed, `code` the tab's
    notice, `version_2` the version-2 journal as E left it, `own` the journals E started that D didn't have (the older release's own list)."""
    old_v = older.v
    on_disk = journals({p: d for p, d in f_files.items()})
    if older.reads_version_2_journals:
        start = [p for p, d in version_2.items() if any(x.startswith(d) for x in on_disk.values())]
        return [(f"F: the version-2 journal {old_v} added to is on disk, as the start of a journal", len(start) == len(version_2) == 1,
                 f"journals now {[(p, journal_version(x), len(x)) for p, x in on_disk.items()]}")]
    if shown:
        # The two lists merged (build/carry-two-journals): the version-2 journal's bytes are the merged journal's start, and the older
        # release's own list is kept aside, byte for byte, under whatever name.
        merged = [p for p, d in version_2.items() if any(x.startswith(d) and len(x) > len(d) for x in on_disk.values())]
        aside = {p: next((q for q, x in f_files.items() if x == d), None) for p, d in own.items()}
        return [
            (f"F: {new_v} merged the lists: a journal on disk starts with its version-2 journal, byte for byte, and goes on", len(merged) == len(version_2) == 1,
             f"journals now {[(p, journal_version(x), len(x)) for p, x in on_disk.items()]}"),
            (f"F: {old_v}'s own list is kept, byte for byte, as {', '.join(map(str, aside.values()))}", len(aside) == 1 and all(aside.values()),
             f"{old_v} started {sorted(own)}; on disk {sorted(f_files)}"),
        ]
    # The highlight hidden (faults F.*, said by check_carry.py from the pair's file): its list is on disk byte for byte, under its own name
    # or set aside, and the tab says it's set aside, which is then so.
    where = {p: p if f_files.get(p) == d else set_aside_as(p) if f_files.get(set_aside_as(p)) == d else None for p, d in version_2.items()}
    return [
        (f"F: {new_v}'s version-2 journal is on disk byte for byte, as {', '.join(map(str, where.values()))}", len(where) == 1 and all(where.values()),
         f"journals now {sorted(p for p in f_files if p.rsplit('/', 1)[-1].startswith('journal-'))}"),
        ("F: the hidden highlight is said on its tab (`edits_set_aside_after_quit`), and its list is set aside in the state folder as the notice says",
         code == "edits_set_aside_after_quit" and all(w == set_aside_as(p) for p, w in where.items()), f"the tab says {code!r}; the journal is at {where}"),
    ]


def unmet_faults(acknowledged, met):
    """A `fault:` line of the pair's file that this run didn't meet: the fault is fixed (or no longer reached), so its line goes."""
    return [(f"the pair's file names `fault:{key}`, which didn't happen in this run: if it's fixed, its line goes", False, why[:200])
            for key, why in sorted(acknowledged.items()) if key not in met]
