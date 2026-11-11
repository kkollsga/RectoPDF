#!/usr/bin/python3 -I
"""Turns a soak's samples.csv into a verdict: flat or growing, with slopes and peaks.

    verdict.py samples.csv [--warmup-h 1]

What decides it, after the warm-up hour:
- the app process's footprint: least-squares slope in MiB an hour, and the median of the last two hours against the first two;
- the whole tree's floor: each hour's 10th percentile, slope over the hours (workers come and go with heavy pages, so the
  floor is what a leak raises, not the peaks);
- threads and descriptors of the whole tree: the last two hours' maximum against the first two hours'.
Growing if the app slope is over 0.5 MiB/h, its last-two-hours median is over 5 MiB above the first two, the tree's floor slope
is over 1 MiB/h, or threads or descriptors keep climbing. Restarts of the app (run_id > 1) are reported as failures on their own.
"""
import csv
import statistics
import sys

path = sys.argv[1]
warm = 1.0
if "--warmup-h" in sys.argv:
    warm = float(sys.argv[sys.argv.index("--warmup-h") + 1])

rows = list(csv.DictReader(open(path)))
alive = [r for r in rows if r["app_mib"]]
gone = [r for r in rows if not r["app_mib"]]
if len(alive) < 10:
    print(f"Too few samples to judge: {len(alive)} with the app alive.")
    sys.exit(1)


def f(r, k):
    return float(r[k])


def slope(xs, ys):
    n = len(xs)
    mx, my = sum(xs) / n, sum(ys) / n
    sxx = sum((x - mx) ** 2 for x in xs)
    if sxx == 0:
        return 0.0
    return sum((x - mx) * (y - my) for x, y in zip(xs, ys)) / sxx


runs = sorted({int(r["run_id"]) for r in alive})
# Judge the longest run of one app process.
by_run = {k: [r for r in alive if int(r["run_id"]) == k] for k in runs}
main_run = max(by_run, key=lambda k: len(by_run[k]))
data = by_run[main_run]
t0 = f(data[0], "elapsed_s")
span_h = (f(data[-1], "elapsed_s") - t0) / 3600
post = [r for r in data if (f(r, "elapsed_s") - t0) / 3600 >= warm] or data
hours = [(f(r, "elapsed_s") - t0) / 3600 for r in post]
app = [f(r, "app_mib") for r in post]
tot = [f(r, "total_mib") for r in post]
thr = [int(r["total_threads"]) for r in post]
fds = [int(r["total_fds"]) for r in post]
app_thr = [int(r["app_threads"]) for r in post]
app_fds = [int(r["app_fds"]) for r in post]

app_slope = slope(hours, app)
win = 2.0
first = [r for r, h in zip(post, hours) if h < hours[0] + win]
last = [r for r, h in zip(post, hours) if h > hours[-1] - win]
first_med = statistics.median(f(r, "app_mib") for r in first)
last_med = statistics.median(f(r, "app_mib") for r in last)

# The tree's floor: each hour's 10th percentile.
buckets = {}
for r, h in zip(post, hours):
    buckets.setdefault(int(h), []).append(f(r, "total_mib"))
floor_h = sorted(buckets)
floors = [sorted(buckets[h])[max(0, int(len(buckets[h]) * 0.1) - 1)] for h in floor_h]
floor_slope = slope([float(h) for h in floor_h], floors) if len(floor_h) >= 2 else 0.0
app_floors = []
for h in floor_h:
    vals = sorted(f(r, "app_mib") for r, hh in zip(post, hours) if int(hh) == h)
    app_floors.append(vals[max(0, int(len(vals) * 0.1) - 1)])

thr_first = max(int(r["total_threads"]) for r in first)
thr_last = max(int(r["total_threads"]) for r in last)
fd_first = max(int(r["total_fds"]) for r in first)
fd_last = max(int(r["total_fds"]) for r in last)
athr_first = max(int(r["app_threads"]) for r in first)
athr_last = max(int(r["app_threads"]) for r in last)
afd_first = max(int(r["app_fds"]) for r in first)
afd_last = max(int(r["app_fds"]) for r in last)

idle = [r for r in post if r["phase"] == "idle"]
idle_app = [f(r, "app_mib") for r in idle]
idle_tot = [f(r, "total_mib") for r in idle]

peak_app = max(data, key=lambda r: f(r, "app_mib"))
peak_tot = max(data, key=lambda r: f(r, "total_mib"))

reasons = []
if app_slope > 0.5:
    reasons.append(f"app footprint rises {app_slope:.2f} MiB/h")
if last_med - first_med > 5:
    reasons.append(f"app median {first_med:.1f} -> {last_med:.1f} MiB")
if floor_slope > 1.0:
    reasons.append(f"tree floor rises {floor_slope:.2f} MiB/h")
if athr_last > athr_first + 2:
    reasons.append(f"app threads {athr_first} -> {athr_last}")
if afd_last > afd_first + 8:
    reasons.append(f"app descriptors {afd_first} -> {afd_last}")
verdict = "GROWING" if reasons else "FLAT"
if span_h < 23:
    verdict += f" (over {span_h:.1f} h, short of 24)"

last_row = rows[-1]
print(f"Soak verdict: {verdict}")
for why in reasons:
    print(f"  because {why}")
print(f"Samples: {len(rows)} ({len(alive)} with the app alive, {len(gone)} without); app runs: {len(runs)}"
      + ("" if len(runs) == 1 else " -- the app was restarted, which is a failure in itself"))
print(f"Judged: run {main_run}, {span_h:.2f} h, {len(post)} samples after a {warm:g} h warm-up")
print(f"App footprint: slope {app_slope:+.3f} MiB/h; median first 2 h {first_med:.2f}, last 2 h {last_med:.2f} MiB; "
      f"min {min(app):.2f}, max {max(app):.2f}")
print(f"Tree footprint: floor slope {floor_slope:+.3f} MiB/h; hourly floors {', '.join(f'{x:.0f}' for x in floors)}")
print(f"App hourly floors: {', '.join(f'{x:.1f}' for x in app_floors)}")
print(f"Peaks: app {f(peak_app, 'app_mib'):.1f} MiB at {peak_app['time']}; tree {f(peak_tot, 'total_mib'):.1f} MiB at {peak_tot['time']} "
      f"({peak_tot['children_names'] or 'no children'})")
if idle:
    print(f"At rest (idle phases): app {min(idle_app):.1f} to {max(idle_app):.1f} MiB, tree {min(idle_tot):.1f} to {max(idle_tot):.1f} MiB")
print(f"Threads, tree: max first 2 h {thr_first}, last 2 h {thr_last}; app {athr_first} -> {athr_last}; overall max {max(thr)}")
print(f"Descriptors, tree: max first 2 h {fd_first}, last 2 h {fd_last}; app {afd_first} -> {afd_last}; overall max {max(fds)}")
print(f"Drive: rounds {last_row['rounds']}, actions {last_row['actions']}, errors {last_row['errors']}, timeouts {last_row['timeouts']}, "
      f"saves {last_row['saves']}")
tabs = [int(r["tabs_open"]) for r in post if r["tabs_open"]]
if tabs:
    print(f"Tabs open: {tabs[0]} after the warm-up, {tabs[-1]} at the end, max {max(tabs)} (the driver keeps about 8 plus its edit copy; more open tabs cost up to 4 KiB each)")
states = [r["state_kib"] for r in rows if r["state_kib"]]
if states:
    print(f"State folder: {int(states[0]) / 1024:.1f} MiB at the start, {int(states[-1]) / 1024:.1f} MiB at the end, "
          f"max {max(int(s) for s in states) / 1024:.1f}")
