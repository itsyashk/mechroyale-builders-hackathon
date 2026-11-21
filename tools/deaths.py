#!/usr/bin/env python3
"""Summarise DEATH lines (MECHBOT_DUEL=1) from one or more diag dirs (err/, rep/).

Usage: python3 tools/deaths.py DIAG_DIR [DIAG_DIR...]
"""
import glob, json, os, re, sys
from collections import Counter

rows = []
for d in sys.argv[1:]:
    tiers = {}
    for f in glob.glob(os.path.join(d, "rep", "match_*.jsonl")):
        k = int(re.search(r"match_(\d+)", f).group(1))
        with open(f) as fh:
            h = json.loads(fh.readline())
        tiers[k] = {s["slot"]: s["who"] for s in h["slots"]}
    for f in glob.glob(os.path.join(d, "err", "m*_s*.log")):
        m = re.search(r"m(\d+)_s(\d+)", os.path.basename(f))
        k = int(m.group(1))
        for line in open(f, errors="replace"):
            if not line.startswith("DEATH "):
                continue
            r = {}
            for kv in line.split()[1:]:
                a, b = kv.split("=", 1)
                try:
                    r[a] = float(b)
                except ValueError:
                    r[a] = b
            pl = re.search(r'"place":(\d+)', r["raw"])
            r["place"] = int(pl.group(1)) if pl else -1
            r["tier"] = tiers.get(k, {}).get(int(r["top"]), "none") if r["top"] >= 0 else "none"
            r["src"] = os.path.basename(d.rstrip("/"))
            rows.append(r)

died = [r for r in rows if r["place"] != 1]
print(f"{len(rows)} matches, {len(died)} deaths, mean score {sum(16-r['place'] for r in rows)/max(1,len(rows)):.2f}, "
      f"mean kills {sum(r['kills'] for r in rows)/max(1,len(rows)):.2f}")


def summ(name, rs):
    if not rs:
        return
    n = len(rs)
    a = lambda k: sum(r[k] for r in rs) / n
    print(f"{name:24s} n={n:3d} ({100*n/max(1,len(died)):3.0f}%) sc={sum(16-r['place'] for r in rs)/n:5.2f} t={a('t'):4.0f} k={a('kills'):.1f} sincek={a('sincek'):4.0f} "
          f"att5={a('att5'):.1f} att10={a('att10'):.1f} l/g/z/m={a('l'):.0f}/{a('g'):.0f}/{a('z'):.0f}/{a('m'):.0f} rear={a('rear'):.0f} npc={a('npc'):.0f} "
          f"zone={a('zone'):.0f} topd={a('topd'):.0f} toph={a('toph'):.0f} hpmax20={a('hpmax20'):.0f} hp10={a('hp10'):.0f} "
          f"F={a('fight'):.2f} C={a('cover'):.2f} O={a('other'):.2f} retr={a('retr'):.2f} kite={a('kite'):.2f} vis={a('vis'):.1f} sh={a('shoot'):.1f} eng={a('eng'):.1f}")


summ("ALL deaths", died)
for src in sorted(set(r["src"] for r in died)):
    summ(" src " + src, [r for r in died if r["src"] == src])
print("-- killer tier (top dmg in last 10 s)")
for t in ["easy", "medium", "hard", "expert", "none"]:
    summ(" " + t, [r for r in died if r["tier"] == t])
print("-- attackers in last 5 s")
for n in [0, 1, 2, 3]:
    summ(f" att5={n}{'+' if n == 3 else ''}", [r for r in died if (r["att5"] >= n if n == 3 else r["att5"] == n)])
print("-- top attacker weapons")
for w, _ in Counter(r["topw"] for r in died).most_common(10):
    summ(" " + w, [r for r in died if r["topw"] == w])
print("-- my weapons")
for w, _ in Counter(r["mw"] for r in died).most_common(8):
    summ(" " + w, [r for r in died if r["mw"] == w])
print("-- dominant damage weapon (last 10 s)")
for k in ["l", "g", "z", "m"]:
    summ(" dom=" + k, [r for r in died if max(["l", "g", "z", "m"], key=lambda x: r[x]) == k])
print("-- top attacker distance")
for lo, hi in [(-2, 0), (0, 30), (30, 50), (50, 80), (80, 130), (130, 999)]:
    summ(f" topd {lo}-{hi}", [r for r in died if lo <= r["topd"] < hi])
print("-- death time")
for lo, hi in [(0, 60), (60, 120), (120, 180), (180, 240), (240, 300), (300, 999)]:
    summ(f" t {lo}-{hi}", [r for r in died if lo <= r["t"] < hi])
print("-- kills at death")
for k in range(0, 10):
    summ(f" kills={k}", [r for r in died if r["kills"] == k])
print("-- time since last kill")
for lo, hi in [(0, 10), (10, 30), (30, 999)]:
    summ(f" sincek {lo}-{hi}", [r for r in died if lo <= r["sincek"] < hi and r["kills"] > 0])
print("-- hp 10 s before death / max hp in last 20 s")
for lo, hi in [(0, 0.5), (0.5, 0.8), (0.8, 1.01)]:
    summ(f" hp10/max {lo}-{hi}", [r for r in died if lo <= r["hp10"] / max(r["hpmax20"], 1) < hi])
print("-- mode last 10 s")
summ(" mostly fight", [r for r in died if r["fight"] >= 0.6])
summ(" mostly cover", [r for r in died if r["cover"] >= 0.4])
summ(" mostly other", [r for r in died if r["other"] >= 0.5])
summ(" retreating>0.3", [r for r in died if r["retr"] > 0.3])
summ(" kiting>0.2", [r for r in died if r["kite"] > 0.2])
summ(" rear>=300", [r for r in died if r["rear"] >= 300])
print("-- engagement duration (continuous dmg)")
for lo, hi in [(0, 4), (4, 8), (8, 15), (15, 99)]:
    summ(f" eng {lo}-{hi}", [r for r in died if lo <= r["eng"] < hi])
