#!/usr/bin/python3 -I
"""Tests of the soak's driver and verdict, offline: `python3 -I tools/soak/test_soak.py`. Nothing is started.

Each test names the failure it catches. The 24-hour soak of 2026-11-08 drifted from 8 tabs to 70, as the driver closed its own
edit copy (it told it by a name `tabs` never gives) and closed one tab a round at most, and its verdict judged that drift as the
app growing, with no way to tell a per-tab cost from one over time.
"""
import csv
import os
import subprocess
import sys
import tempfile
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
import soak  # noqa: E402


def tab(sel, name="x", active=False, edited=False):
    return {"sel": sel, "untrusted": {"name": name}, "active": active, "edited": edited}


class ToClose(unittest.TestCase):
    def test_keeps_the_edit_copy_by_selector(self):
        """Catches the driver closing its own edit copy: its tab is kept by the selector `open` gave, whatever it's called."""
        tabs = [tab("#1", "edit-copy"), *[tab(f"#{i}") for i in range(2, 10)]]
        closed = soak.to_close(tabs, {"#1"}, 8)
        self.assertNotIn("#1", closed)
        self.assertEqual(closed, ["#2", "#3"])

    def test_closes_down_to_the_limit_not_one_a_round(self):
        """Catches the tab count drifting up: past the limit, enough close in one round to leave room for one open."""
        tabs = [tab(f"#{i}") for i in range(1, 21)]
        self.assertEqual(len(tabs) - len(soak.to_close(tabs, set(), 8)), 7)

    def test_never_the_active_or_an_edited_tab(self):
        """Catches a round closing what's being read or what has unsaved edits, which `close` would refuse anyway."""
        tabs = [tab("#1", active=True), tab("#2", edited=True), *[tab(f"#{i}") for i in range(3, 11)]]
        closed = soak.to_close(tabs, set(), 8)
        self.assertNotIn("#1", closed)
        self.assertNotIn("#2", closed)

    def test_name_is_the_stem(self):
        """Catches comparing a tab's name with a file name: `tabs` gives the stem, as recto-session's `tab_name` makes it."""
        self.assertEqual(soak.stem("/a/b/edit-copy.pdf"), "edit-copy")


HEADER = ("time,elapsed_s,run_id,phase,app_pid,app_mib,children_mib,total_mib,children,app_threads,total_threads,app_fds,total_fds,"
          "app_cpu_s,app_wakeups,state_kib,tabs_open,rounds,actions,errors,timeouts,saves,app_heap_mib,app_compressed_mib,children_names").split(",")


def verdict_of(app_and_tabs, heap=None):
    """Runs verdict.py on a made CSV, a sample a minute for each (app MiB, tabs) given, and the heap's MiB for each if given."""
    with tempfile.TemporaryDirectory() as d:
        path = os.path.join(d, "samples.csv")
        with open(path, "w", newline="") as f:
            w = csv.DictWriter(f, HEADER)
            w.writeheader()
            for i, (app, tabs) in enumerate(app_and_tabs):
                w.writerow({k: "" for k in HEADER} | {
                    "time": f"t{i}", "elapsed_s": i * 60, "run_id": 1, "phase": "rest", "app_pid": 1, "app_mib": f"{app:.3f}",
                    "children_mib": 0, "total_mib": f"{app:.3f}", "children": 0, "app_threads": 11, "total_threads": 11,
                    "app_fds": 20, "total_fds": 20, "tabs_open": tabs, "rounds": i, "actions": i, "errors": 0, "timeouts": 0, "saves": 0,
                    "app_heap_mib": f"{heap[i]:.3f}" if heap else "", "app_compressed_mib": "1.0" if heap else ""})
        out = subprocess.run([sys.executable, "-I", os.path.join(HERE, "verdict.py"), path], capture_output=True, text=True)
        return out.stdout + out.stderr


MINUTES = 24 * 60


class Verdict(unittest.TestCase):
    def test_tabs_at_their_budget_are_flat_and_drift_is_the_drivers(self):
        """Catches judging a tab count that drifts as the app growing: 8 to 70 tabs at 4 KiB each is flat, and the drift is named."""
        rows = [(30 + (8 + 62 * m / MINUTES) * 4 / 1024, int(8 + 62 * m / MINUTES)) for m in range(MINUTES)]
        out = verdict_of(rows)
        self.assertTrue(out.startswith("Soak verdict: FLAT"), out)
        self.assertIn("driver drifted", out)

    def test_growth_over_time_with_tabs_held(self):
        """Catches a leak the per-tab allowance would hide: 1 MiB an hour with 9 tabs open the whole time."""
        rows = [(30 + m / 60, 9) for m in range(MINUTES)]
        out = verdict_of(rows)
        self.assertTrue(out.startswith("Soak verdict: GROWING"), out)
        self.assertNotIn("driver drifted", out)

    def test_a_tab_far_over_budget_where_tabs_vary_apart_from_time(self):
        """Catches what a warm-then-dormant tab keeps: 200 KiB a tab, with tabs going up and down through the day, is growing."""
        rows = []
        for m in range(MINUTES):
            tabs = 6 + (m // 37) % 5
            rows.append((30 + tabs * 200 / 1024, tabs))
        out = verdict_of(rows)
        self.assertTrue(out.startswith("Soak verdict: GROWING"), out)
        self.assertIn("each open tab costs", out)

    def test_a_heap_that_climbs_under_a_flat_footprint(self):
        """Catches a leak the footprint hides, as when the system's compression moves it more than the app does: the heap rising
        0.5 MiB an hour is growing, though the footprint is flat."""
        rows = [(40.0, 9) for _ in range(MINUTES)]
        out = verdict_of(rows, heap=[8 + m / 120 for m in range(MINUTES)])
        self.assertTrue(out.startswith("Soak verdict: GROWING"), out)
        self.assertIn("app heap rises", out)

    def test_no_heap_column_says_so(self):
        """Catches a verdict on a run without the heap reading as though it had judged it: the 2026-11-08 soak had no such column."""
        out = verdict_of([(40.0, 9) for _ in range(MINUTES)])
        self.assertIn("App heap: not sampled", out)


if __name__ == "__main__":
    unittest.main()
