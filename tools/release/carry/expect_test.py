"""expect_test: check-carry's expectations of a pair (expect.py), on made states, for an older release that can't read a version-2 journal (0.1.0)
and one that can (0.1.1 and later). Each case names the failure it catches. No app is run: about a second. Run by tools/release/release-test.sh.

    python3 -I tools/release/carry/expect_test.py
"""

import os
import sys
import unittest

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import expect  # noqa: E402
from expect import Older  # noqa: E402

J = "session/journals/"


def journal(version, *entries):
    return f'RJ {{"version":{version}}}\n'.encode() + b"".join(e.encode() + b"\n" for e in entries)


def sleep(fmt):
    return b'{"sleep":1}\n' + (b'{"sleep_format":%d}' % fmt if fmt else b'{"pictures":[1]}')


def passes(checks):
    return all(good for _, good, _ in checks)


OLD, NEW = Older("0.1.0"), Older("0.1.1")


class Versions(unittest.TestCase):
    def test_each_fact_turns_on_its_first_version(self):
        """Failure: a pair's expectations follow the wrong side of the line, as when they were fixed to 0.1.0's and 0.1.1 failed six of them."""
        self.assertFalse(Older("0.1.0").reads_version_2_journals)
        self.assertFalse(Older("0.1.0").sleeps_without_pictures)
        for v in ("0.1.1", "0.1.2", "0.2.0", "1.0.0"):
            self.assertTrue(Older(v).reads_version_2_journals, v)
            self.assertTrue(Older(v).sleeps_without_pictures, v)

    def test_a_development_build_is_its_release(self):
        """Failure: a development build's fourth part (`0.1.1.5`) is read as a fifth version, or refused."""
        self.assertEqual(expect.version("0.1.1.5"), (0, 1, 1))
        self.assertGreater(expect.version("0.1.10"), expect.version("0.1.9"))


class SleepFiles(unittest.TestCase):
    slept = (1, 2)

    def run_b(self, older, wakes, formats, kept=False):
        a = {f"{J}sleep-{t}.bin": sleep(f) for t, f in formats.items()}
        b = dict(a) if kept else {}
        return expect.b_sleep(older, "0.1.2", wakes, self.slept, {p: expect.sleep_format(d) for p, d in a.items()}, a, b)

    def test_before_011_pictures_are_never_read_and_go(self):
        """Failure: a newer build wakes a tab from 0.1.0's pictures of pages, or leaves them on disk after reading."""
        wakes = {1: {"from_sleep": False}, 2: {"from_sleep": False}, 3: {"from_sleep": False}}
        self.assertTrue(passes(self.run_b(OLD, wakes, {1: 0, 2: 0})))
        self.assertFalse(passes(self.run_b(OLD, {**wakes, 1: {"from_sleep": True}}, {1: 0, 2: 0})))
        self.assertFalse(passes(self.run_b(OLD, wakes, {1: 0, 2: 0}, kept=True)))

    def test_from_011_each_tab_that_slept_wakes_from_its_file(self):
        """Failure: a newer build throws away 0.1.1's sleep files and wakes each tab from its PDF, which the person sees as a slower wake."""
        wakes = {1: {"from_sleep": True}, 2: {"from_sleep": True}, 3: {"from_sleep": True}}
        self.assertTrue(passes(self.run_b(NEW, wakes, {1: 4, 2: 4}, kept=True)))
        self.assertFalse(passes(self.run_b(NEW, {**wakes, 2: {"from_sleep": False}}, {1: 4, 2: 4}, kept=True)))
        # And a pair whose older release still wrote pictures is said, not passed: the version line is wrong.
        self.assertFalse(passes(self.run_b(NEW, wakes, {1: 0, 2: 4}, kept=True)))


class HighlightedTabInTheOlderRelease(unittest.TestCase):
    def test_before_011_the_tab_shows_the_file_and_says_why(self):
        """Failure: 0.1.0 shows part of a version-2 journal's edits, or shows the file without them and says nothing."""
        self.assertTrue(passes(expect.c_highlighted_tab(OLD, "0.1.2", {"edited": False, "file_notice": {"code": "edits_unreadable"}}, None)))
        self.assertFalse(passes(expect.c_highlighted_tab(OLD, "0.1.2", {"edited": False, "file_notice": None}, None)))

    def test_from_011_the_tab_shows_its_edits_and_the_highlight(self):
        """Failure: an older release that reads highlights loses one on going back, or says its edits couldn't be read."""
        one = {"edits": [{"kind": "highlight"}]}
        self.assertTrue(passes(expect.c_highlighted_tab(NEW, "0.1.2", {"edited": True, "file_notice": None}, one)))
        self.assertFalse(passes(expect.c_highlighted_tab(NEW, "0.1.2", {"edited": True, "file_notice": None}, {"edits": []})))
        self.assertFalse(passes(expect.c_highlighted_tab(NEW, "0.1.2", {"edited": False, "file_notice": {"code": "edits_unreadable"}}, one)))


class OlderReleasesEdit(unittest.TestCase):
    v2 = {f"{J}journal-g.log": journal(2, "highlight")}

    def test_before_011_the_version_2_journal_is_left_byte_for_byte(self):
        """Failure: 0.1.0 writes into a journal of a version it can't read, and the highlight is lost."""
        self.assertTrue(passes(expect.e_journal(OLD, "0.1.2", self.v2, dict(self.v2, **{f"{J}journal-g-3.log": journal(1, "turn")}))))
        self.assertFalse(passes(expect.e_journal(OLD, "0.1.2", self.v2, {f"{J}journal-g.log": journal(2, "highlight", "turn")})))

    def test_from_011_the_edit_goes_on_the_version_2_journal(self):
        """Failure: 0.1.1's edit replaces the journal (the highlight gone), lowers its version, or isn't written at all."""
        self.assertTrue(passes(expect.e_journal(NEW, "0.1.2", self.v2, {f"{J}journal-g.log": journal(2, "highlight", "turn")})))
        self.assertFalse(passes(expect.e_journal(NEW, "0.1.2", self.v2, dict(self.v2))))
        self.assertFalse(passes(expect.e_journal(NEW, "0.1.2", self.v2, {f"{J}journal-g.log": journal(2, "turn")})))
        self.assertFalse(passes(expect.e_journal(NEW, "0.1.2", self.v2, {f"{J}journal-g.log": journal(1, "highlight", "turn")})))


class TheNewBuildAgain(unittest.TestCase):
    v2 = {f"{J}journal-g.log": journal(2, "highlight")}
    own = {f"{J}journal-g-3.log": journal(1, "turn")}

    def test_from_011_the_journal_the_older_release_added_to_is_kept(self):
        """Failure: the newer build drops or rewrites a journal the older release added an edit to."""
        after_e = {f"{J}journal-g.log": journal(2, "highlight", "turn")}
        self.assertTrue(passes(expect.f_disk(NEW, "0.1.2", True, None, after_e, {}, dict(after_e))))
        self.assertTrue(passes(expect.f_disk(NEW, "0.1.2", True, None, after_e, {}, {f"{J}journal-g.log": journal(2, "highlight", "turn", "more")})))
        self.assertFalse(passes(expect.f_disk(NEW, "0.1.2", True, None, after_e, {}, {f"{J}journal-g.log": journal(2, "turn")})))

    def test_before_011_merged_lists_start_with_the_version_2_journal_and_keep_the_older_ones(self):
        """Failure: the merge (build/carry-two-journals) loses the highlight's bytes, or the list 0.1.0 made in its own journal."""
        files = {f"{J}journal-g.log": journal(2, "highlight", "turn"), f"{J}journal-g-3.not-merged": self.own[f"{J}journal-g-3.log"]}
        self.assertTrue(passes(expect.f_disk(OLD, "0.1.2", True, None, self.v2, self.own, files)))
        self.assertFalse(passes(expect.f_disk(OLD, "0.1.2", True, None, self.v2, self.own, {f"{J}journal-g.log": files[f"{J}journal-g.log"]})))
        self.assertFalse(passes(expect.f_disk(OLD, "0.1.2", True, None, self.v2, self.own, dict(files, **{f"{J}journal-g.log": journal(2, "turn")}))))

    def test_before_011_a_hidden_highlight_is_set_aside_and_said(self):
        """Failure (faults F, 0.1.1's): the version-2 journal isn't on disk, or the tab's notice says something that isn't so."""
        aside = {f"{J}journal-g.not-merged": self.v2[f"{J}journal-g.log"], **self.own}
        self.assertTrue(passes(expect.f_disk(OLD, "0.1.1", False, "edits_set_aside_after_quit", self.v2, self.own, aside)))
        self.assertFalse(passes(expect.f_disk(OLD, "0.1.1", False, "edits_set_aside_after_quit", self.v2, self.own, dict(self.own))))
        self.assertFalse(passes(expect.f_disk(OLD, "0.1.1", False, "edits_not_merged", self.v2, self.own, aside)))


class FaultLines(unittest.TestCase):
    def test_a_fault_line_that_did_not_happen_fails(self):
        """Failure: a fixed fault's line stays in the pair's file, and the check would say KNOWN FAULT of a fault that comes back unseen."""
        acknowledged = {"D.notice.edits_unreadable": "why", "F.highlight_hidden": "why"}
        self.assertEqual(expect.unmet_faults(acknowledged, set(acknowledged)), [])
        unmet = expect.unmet_faults(acknowledged, {"F.highlight_hidden"})
        self.assertEqual(len(unmet), 1)
        self.assertIn("fault:D.notice.edits_unreadable", unmet[0][0])
        self.assertFalse(unmet[0][1])


if __name__ == "__main__":
    unittest.main(verbosity=1)
