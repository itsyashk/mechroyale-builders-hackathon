#!/usr/bin/env python3
"""Summarise what a bot experienced, from recorded observation streams (.jsonl or .jsonl.gz).

Used to compare the local sim's house bots against the real server's recorded match
(kit/check/stream.jsonl.gz). Usage: python3 tools/streamstats.py FILE [FILE ...]
"""
import gzip, json, math, sys, statistics as st


def load(path):
    op = gzip.open if path.endswith(".gz") else open
    with op(path, "rt") as f:
        for line in f:
            line = line.strip()
            if line.startswith("{"):
                try:
                    yield json.loads(line)
                except ValueError:
                    pass


def one(path):
    first_seen = first_dmg = death = None
    dmg = {}
    hits_d = {"gun": [], "laser": [], "lightning": [], "mortar": []}
    attackers = set()
    seen_d = []
    last_t = 0.0
    for o in load(path):
        if o.get("type") != "obs":
            continue
        t = o["t"]
        last_t = t
        me = o["self"]
        ms = {m["id"]: m for m in o.get("mechs", []) if m.get("kind") == "mech"}
        if ms and first_seen is None:
            first_seen = t
        for m in ms.values():
            seen_d.append(math.dist(m["pos"][:2], me["pos"][:2]))
        for e in o.get("events", []):
            if e.get("type") != "damaged" or e.get("by") is None or e["by"] >= 100:
                continue
            w = e.get("weapon", "?")
            dmg[w] = dmg.get(w, 0) + e.get("amount", 0)
            attackers.add(e["by"])
            if first_dmg is None:
                first_dmg = t
            a = ms.get(e["by"])
            if a and w in hits_d:
                hits_d[w].append(math.dist(a["pos"][:2], me["pos"][:2]))
        if me.get("hp", 1) <= 0:
            death = t
    return dict(
        first_seen=first_seen,
        first_dmg=first_dmg,
        end_t=last_t,
        dmg_40s=None,
        dmg=dmg,
        attackers=len(attackers),
        hit_dist={k: (round(st.median(v)), len(v)) for k, v in hits_d.items() if v},
        seen_med_d=round(st.median(seen_d)) if seen_d else None,
    )


if __name__ == "__main__":
    rows = [one(p) for p in sys.argv[1:]]
    for p, r in zip(sys.argv[1:], rows):
        print(p.split("/")[-1], r)
    if len(rows) > 1:
        def med(k):
            v = [r[k] for r in rows if r[k] is not None]
            return round(st.median(v), 1) if v else None
        print("median first_seen", med("first_seen"), "first_dmg", med("first_dmg"), "end_t", med("end_t"),
              "attackers", med("attackers"), "seen_d", med("seen_med_d"))
        tot = {}
        for r in rows:
            for k, v in r["dmg"].items():
                tot[k] = tot.get(k, 0) + v
        T = sum(tot.values()) or 1
        print("damage share", {k: round(v / T, 2) for k, v in tot.items()})
        hd = {}
        for r in rows:
            for k, (m, n) in r["hit_dist"].items():
                hd.setdefault(k, []).extend([m] * n)
        print("hit distance median", {k: (round(st.median(v)), len(v)) for k, v in hd.items()})
