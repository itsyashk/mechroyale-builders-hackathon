# sim: local Mech Royale match simulator

Plays mechfight/1 matches offline, much faster than real time, so a bot can be tested
without spending real submissions. The engine follows `kit/protocol.md`; the rule set sent in
`hello.config` is the recorded official config (`crates/sim/config.json`, taken verbatim from
`kit/check/stream.jsonl.gz`).

## Build and run

On the Linux dev container (recommended; Windows Smart App Control can block freshly built
`.exe` files):

```sh
MSYS_NO_PATHCONV=1 tools/dev.sh sh -c 'cargo build --release -p sim -p mechbot && \
  /target/release/sim practice --bot /target/release/mechbot --matches 48 --jobs 7'
# kit example bot
MSYS_NO_PATHCONV=1 tools/dev.sh sh -c '/target/release/sim practice --bot "python3 -u kit/python/example_bot.py" --matches 8'
```

Natively (Windows, Git Bash):

```sh
export ZIG="$HOME/tools/zig-x86_64-windows-0.17.1/zig.exe"
cargo build --release -p sim
./target/release/sim practice --bot "python -u kit/python/example_bot.py" --matches 4 --jobs 4
```

Run it from the repo root (the default `--map` is `kit/arena`; `hello.map.dir` is sent as
that folder's absolute path).

## Modes

```
sim practice --bot "<cmd>" [options]
    Your bot in a random slot vs 15 house bots (4 easy, 4 medium, 4 hard, 3 expert) acting
    as one squad. Score = mean(16 - place).

sim ffa --bots "<cmd1>" ["<cmd2>" ...] [--ext-slots N] [options]
    16-slot free-for-all. The first N (default 16) randomly placed slots are filled
    round-robin with the commands; the rest are solo house bots that fight everyone.
    Reports mean(16 - place) per command. `sim ffa` with no --bots runs 16 house bots.
```

Options:

| Option | Meaning |
| --- | --- |
| `--matches N` | matches to play (default 8) |
| `--jobs J` | parallel matches (default CPUs - 1) |
| `--seed S` | base seed (default 1); match k uses a seed derived from (S, k), so runs are reproducible (bot timing aside) |
| `--out FILE` | append one JSON line per external slot per match (place, kills, damage, cause, killer, timing) |
| `--replay-dir DIR` | compact JSONL replay per match: header, a frame every `--replay-every` ticks (`m: [id, x, y, z, hp, yaw, cubes]`, projectile count, zone), death lines, result |
| `--budget-ms MS` | decision budget (default `bot.tick_budget_ms` = 50) |
| `--lenient [MS]` | wait up to MS (default 1000) for each answer so a loaded machine does not turn slow answers into coasting; answers over the budget are still counted as late |
| `--stderr-dir DIR` | save each bot's stderr to `DIR/m<match>_s<slot>.log` |
| `--map DIR`, `--hello-map-dir D` | map folder to load / path to send in `hello.map.dir` |
| `--config FILE` | alternative rule set (same JSON shape as `hello.config`) |
| `--full` | keep simulating after every external bot is dead (default: stop, since its place is final) |
| `--quiet` | no per-match lines |

Output: one line per external slot per match, then a summary with the score, its standard
error, mean place, kills, survival time, damage dealt/taken, cubes, cause-of-death and killer
breakdowns (`easy(hunter)` = killed by an easy bot that was hunting you), and response-time
p50/p99/max with late/missing/bad-line counts.

Debug helpers: `SIM_DEBUG=1` prints per-match gun hits/shots per slot; `sim probe X Y [Z]`
prints the collision picture around a point (loads `kit/arena`).

## Speed

About 1-2 s of wall time per full 480 s match (16 bots, 12 NPCs) on one core, plus whatever
the external bots take. In practice mode a match stops when your bot dies, so typical
practice matches take 0.5-2 s each; 48 matches on 7 jobs take about 10-20 s.

## Lockstep and timing

Like the server: all observations are sent, then each external answer is awaited until
`budget` (or the `--lenient` wait). A missing or late answer coasts (`move`, `boost`, `aim`
kept, one-shots dropped) and after `coast_s` of misses the mech idles. Answers whose `tick`
does not match are dropped. A bot gets `end` (place, kills) the tick it dies, or at match end.
Skipping to the newest observation is the bot's job, as in the real protocol.

## House bots

`src/house.rs`, all knobs in the `TIERS` table:

| | easy | medium | hard | expert |
| --- | --- | --- | --- | --- |
| aim error σ (rad, drifting) | 0.11 | 0.04 | 0.014 | 0.006 |
| reaction / perception delay (s) | 0.9 | 0.35 | 0.22 | 0.12 |
| lead (fraction of velocity used) | 0 | 0.5 | 0.85 | 1.0 |
| aim tracking rate (rad/s) | 1.3 | 2.6 | 3.6 | 6 (server cap 4) |
| strafe weight / dodge jumps | 0.15 / none | 0.45 / rare | 0.7 / some | 0.9 / frequent, boosts |
| laser released at charge | 45% | 75% | 95% | 100% |
| mortar lob at squad-seen targets | no | no | yes | yes |
| farm NPCs, go for supply drops, retreat when low | no | no | yes | yes |

Behaviour: squad bots share vision, their shots and blasts pass through each other, they path
with `Nav` A*, loot weapons (tier preference), shoot crates and (hard/expert) NPCs for cubes,
pick upgrades from a tier preference list, keep inside the zone (the next circle once a shrink
is within 25 s), lead with an iterated intercept (gun) or a ballistic solution (mortar), hold
and release the laser, and fire lightning only with the target in the cone.

Hunting: one hunter from 45 s (the weakest tier first), +1 hunter per fallen squad mate,
+1 per minute after the first minute, all of them from 5:00 or once 4 or fewer remain.
Hunters get a noisy (14 m) fix on their quarry every 2-6 s (by tier) and chase it; others
only engage you within 25-40 m or fight back within 40-55 m. FFA fillers have no squad and
engage anyone they see within 70-180 m.

Calibration: the kit's Python example bot scores about 0.4-0.7 here (0.58 over 168 matches)
against an official baseline of 1.15, so these house bots are a bit harsher than the real ones.
The example bot mostly dies to the first (easy) hunter or by walking into a non-hunter while
stuck on a building corner (it does not path).

## Known fidelity gaps vs protocol.md

- **Zone obs:** `zone.center/radius` are the live, interpolated circle (the spec is ambiguous
  whether they are the phase's start circle). Next-circle centres are random inside the
  current circle, not the server's (unknown) distribution.
- **Movement:** cylinder vs the 1 m span raster, sliding approximated by trying the smallest
  rotation of the move that is free. Roof-edge landing and slide-off are approximate; eaves do
  not slide you out (they only stop a jet). Slopes are checked on terrain only.
- **Lightning cone:** `cone_deg` is used as the allowed deviation from aim yaw (±30°, i.e. a
  60° cone). If the server uses it as the full width, lightning is too generous here.
- **Mortar:** a direct hit deals `damage` only (no extra splash to that target); splash
  distance is measured to the target's cylinder surface; walls block splash by LOS from just
  above the blast point.
- **Vision:** trunks block; crowns attenuate with `exp(-canopyDensity * chord) < canopyBlockT`
  (crowns are ellipsoids). `near_range` ignores trees. Projectiles, cubes and pickups use a
  single-point LOS.
- **Sounds:** steps/boost/jets are emitted from the mover's feet; laser_charge only at charge
  start; no sound for crate hits (only destruction).
- **Zone damage** is reported as one `damaged` event per observation instead of per tick.
- **NPCs** aim without lead, move with simple steering and do not path. Spawn placement of
  NPCs, crates, loot (roofs = flat building tops under 55 m) and drops is an approximation.
- **Supply drops:** capture needs |dz| < 10 m; position is random open ground within
  `spread × next_radius` of the next centre.
- **Debug drawing** in actions is accepted and ignored (no caps check). Bad lines are counted
  but there is no 64 KiB enforcement beyond dropping over-long lines.
- **House bots** walk the ground nav grid only (no jets/roofs), do not use sound, and get
  hunting intel the real squad may not have.
