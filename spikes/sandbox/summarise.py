#!/usr/bin/env python3
"""Reads what measure.sh wrote and prints a table: the median over rounds of each number, per layout. `python3 -I summarise.py <raw-output-folder>`."""
import glob
import json
import statistics
import sys

raw = sys.argv[1]
variants = ["plain", "entitled", "dropmach", "selfbox"]
rows = {}


def add(name, variant, value):
    rows.setdefault(name, {}).setdefault(variant, []).append(value)


for variant in variants:
    for path in sorted(glob.glob(f"{raw}/{variant}.*.pages.json")):
        round_ = path.split(".")[-3]
        d = json.load(open(path))["data"]
        add("rfc9110 whole pages p50 ms", variant, d["ms"]["p50"])
        add("rfc9110 whole pages p95 ms", variant, d["ms"]["p95"])
        add("rfc9110 idle worker MiB", variant, d["idle_footprint_bytes"] / 2**20)
        add("rfc9110 peak MiB", variant, d["peak_footprint_bytes"] / 2**20)
        f = json.load(open(f"{raw}/{variant}.{round_}.fresh.json"))["data"]
        add("fresh worker, open, first page p50 ms", variant, f["ms"]["p50"])
        add("fresh worker, open, first page p95 ms", variant, f["ms"]["p95"])
        u = json.load(open(f"{raw}/{variant}.{round_}.usgs.json"))["data"]
        for step in u["steps"]:
            add(f"USGS {step['step']} ms", variant, step["ms"])
            add(f"USGS {step['step']} peak MiB", variant, step["peak_footprint_bytes"] / 2**20)
            add(f"USGS {step['step']} failed tiles", variant, step["failed"] + step["low_res"])
        b = json.load(open(f"{raw}/{variant}.{round_}.bomb.json"))["data"]
        add("form_bomb all steps ms", variant, float(open(f"{raw}/{variant}.{round_}.bomb.ms").read()))
        add("form_bomb peak MiB", variant, b["peak_footprint_bytes"] / 2**20)
        add("form_bomb memory kills", variant, b["memory_kills"])
        add("form_bomb heavy retries", variant, b["heavy_retries"])
        add("form_bomb pages failed", variant, b["pages_failed"])

print("| " + " | ".join(["Number", *variants]) + " |")
print("|" + "---|" + "---:|" * len(variants))
for name, values in rows.items():
    cells = []
    for variant in variants:
        series = values.get(variant, [])
        cells.append(f"{statistics.median(series):.2f}" if series else "")
    print(f"| {name} | " + " | ".join(cells) + " |")
