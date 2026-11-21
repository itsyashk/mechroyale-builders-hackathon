#!/usr/bin/env python3
"""Summarise per-opponent engagements logged by the bot with MECHBOT_DUEL=1.

Usage: python3 tools/duels.py DIAG_DIR   (expects DIAG_DIR/err/m*_s*.log and DIAG_DIR/rep/match_*.jsonl
from `sim practice --stderr-dir DIAG_DIR/err --replay-dir DIAG_DIR/rep`).
"""
import glob, json, os, re, sys
from collections import defaultdict

d = sys.argv[1]
tiers = {}
for f in glob.glob(os.path.join(d, "rep", "match_*.jsonl")):
    k = int(re.search(r"match_(\d+)", f).group(1))
    with open(f) as fh:
        h = json.loads(fh.readline())
    tiers[k] = {s["slot"]: s["who"] for s in h["slots"]}

rows = []
for f in glob.glob(os.path.join(d, "err", "m*_s*.log")):
    m = re.search(r"m(\d+)_s(\d+)", os.path.basename(f))
    k = int(m.group(1))
    for line in open(f, errors="replace"):
        if not line.startswith("DUEL "):
            continue
        r = dict(kv.split("=", 1) for kv in line.split()[1:])
        for x in list(r):
            try:
                r[x] = float(r[x])
            except ValueError:
                pass
        r["tier"] = tiers.get(k, {}).get(int(r["id"]), "?")
        r["match"] = k
        rows.append(r)

print(f"{len(rows)} duels")
solo = lambda r: r["other"] < 0.25 * max(r["taken"], 1)


def summ(name, rs):
    if not rs:
        return
    n = len(rs)
    agg = lambda k: sum(r[k] for r in rs) / n
    wk = defaultdict(int)
    for r in rs:
        wk[r["why"]] += 1
    print(f"{name:28s} n={n:4d} " + " ".join(f"{w}={c}" for w, c in sorted(wk.items())) +
          f" dur={agg('dur'):.1f} dealt={agg('dealt'):.0f} taken={agg('taken'):.0f} other={agg('other'):.0f}"
          f" myhp0={agg('myhp0'):.0f} hp0={agg('hp0'):.0f} hpend={agg('hpend'):.0f} d={agg('d'):.0f}"
          f" fire={agg('fire'):.2f} nolos={agg('nolos'):.2f} rel={agg('rel'):.2f} aim={agg('aim'):.2f} hold={agg('hold'):.2f}"
          f" cov={agg('cov'):.2f} tgf={agg('tgf'):.2f} tk[l/g/z/m/rear]={agg('tk_l'):.0f}/{agg('tk_g'):.0f}/{agg('tk_z'):.0f}/{agg('tk_m'):.0f}/{agg('rear'):.0f}"
          f" dl[l/g]={agg('dl_l'):.0f}/{agg('dl_g'):.0f}")


summ("ALL", rows)
for t in ["easy", "medium", "hard", "expert"]:
    rs = [r for r in rows if r["tier"] == t]
    summ(t, rs)
    summ(t + " solo", [r for r in rs if solo(r)])
    summ(t + " solo died", [r for r in rs if solo(r) and r["why"] == "died"])
    summ(t + " solo kill", [r for r in rs if solo(r) and r["why"] == "kill"])
    summ(t + " split", [r for r in rs if r["why"] == "split"])
print("-- by their weapons (solo)")
ws = defaultdict(list)
for r in rows:
    if solo(r):
        ws[str(r["tw"])].append(r)
for w, rs in sorted(ws.items(), key=lambda x: -len(x[1])):
    summ("tw=" + w, rs)
print("-- by my weapons (solo)")
ws = defaultdict(list)
for r in rows:
    if solo(r):
        ws[str(r["mw"])].append(r)
for w, rs in sorted(ws.items(), key=lambda x: -len(x[1])):
    summ("mw=" + w, rs)
print("-- by mean distance (solo)")
for lo, hi in [(0, 30), (30, 50), (50, 80), (80, 120), (120, 999)]:
    summ(f"d {lo}-{hi}", [r for r in rows if solo(r) and lo <= r["d"] < hi])
