//! `sim`: local Mech Royale match simulator (mechfight/1).
//!
//!   sim practice --bot "<cmd>" [--matches N] [--jobs J] [--seed S] ...
//!   sim ffa --bots "<cmd1>" "<cmd2>" ... [--matches N] ...

mod cfg;
mod ext;
mod game;
mod house;
mod obs;
mod phys;
mod trees;
mod util;

use cfg::Cfg;
use ext::ExtBot;
use game::*;
use house::{tiers, Brain, Squad, PRACTICE_TIERS, SQUAD_TEAM};
use std::collections::BTreeMap;
use std::io::Write as _;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use util::*;

#[derive(Clone, PartialEq)]
enum Mode {
    Practice,
    Ffa,
}

#[derive(Clone)]
struct Opts {
    mode: Mode,
    bots: Vec<String>,
    matches: usize,
    jobs: usize,
    seed: u64,
    out: Option<PathBuf>,
    replay_dir: Option<PathBuf>,
    replay_every: u32,
    budget_ms: Option<f32>,
    lenient_ms: Option<f32>,
    stderr_dir: Option<PathBuf>,
    map: String,
    map_dir_override: Option<String>,
    config: Option<String>,
    ext_slots: usize,
    quiet: bool,
    full: bool,
}

const USAGE: &str = r#"sim - local Mech Royale simulator (mechfight/1)

USAGE:
  sim practice --bot "<cmd>" [options]      your bot vs 15 house bots (4 easy, 4 medium, 4 hard, 3 expert squad)
  sim ffa --bots "<cmd1>" ["<cmd2>" ...] [options]
                                            16-slot free-for-all; slots filled round-robin with the commands
                                            (--ext-slots N keeps only N external slots, the rest are solo house bots)

OPTIONS:
  --matches N        matches to play (default 8)
  --jobs J           parallel matches (default: CPUs - 1)
  --seed S           base seed (default 1); match k uses a seed derived from (S, k)
  --out FILE         append one JSON line per external slot per match
  --replay-dir DIR   write a compact JSONL replay per match
  --replay-every T   replay frame every T ticks (default 10 = 0.5 s)
  --budget-ms MS     decision budget (default config bot.tick_budget_ms = 50)
  --lenient [MS]     wait up to MS (default 1000) for each answer; answers over the budget still count as late
  --stderr-dir DIR   capture each bot's stderr to DIR/m<match>_s<slot>.log
  --map DIR          map folder (default kit/arena)
  --hello-map-dir D  value sent as hello.map.dir (default: absolute path of --map)
  --config FILE      rule set JSON (default: the recorded official config)
  --ext-slots N      ffa only: number of external slots (default 16)
  --full             do not stop a match early once every external bot is dead
  --quiet            no per-match lines
"#;

fn parse_args() -> Result<Opts, String> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() || args[0] == "-h" || args[0] == "--help" {
        return Err(String::new());
    }
    let mode = match args[0].as_str() {
        "practice" => Mode::Practice,
        "ffa" => Mode::Ffa,
        m => return Err(format!("unknown mode `{m}`")),
    };
    let jobs = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(2).saturating_sub(1).max(1);
    let mut o = Opts { mode, bots: vec![], matches: 8, jobs, seed: 1, out: None, replay_dir: None, replay_every: 10, budget_ms: None, lenient_ms: None, stderr_dir: None, map: "kit/arena".into(), map_dir_override: None, config: None, ext_slots: 16, quiet: false, full: false };
    let mut i = 1;
    let val = |i: &mut usize| -> Result<String, String> {
        *i += 1;
        args.get(*i).cloned().ok_or_else(|| format!("missing value for {}", args[*i - 1]))
    };
    while i < args.len() {
        let a = args[i].clone();
        match a.as_str() {
            "--bot" => o.bots = vec![val(&mut i)?],
            "--bots" => {
                while i + 1 < args.len() && !args[i + 1].starts_with("--") {
                    i += 1;
                    o.bots.push(args[i].clone());
                }
            }
            "--matches" => o.matches = val(&mut i)?.parse().map_err(|_| "bad --matches")?,
            "--jobs" => o.jobs = val(&mut i)?.parse::<usize>().map_err(|_| "bad --jobs")?.max(1),
            "--seed" => o.seed = val(&mut i)?.parse().map_err(|_| "bad --seed")?,
            "--out" => o.out = Some(val(&mut i)?.into()),
            "--replay-dir" => o.replay_dir = Some(val(&mut i)?.into()),
            "--replay-every" => o.replay_every = val(&mut i)?.parse::<u32>().map_err(|_| "bad --replay-every")?.max(1),
            "--budget-ms" => o.budget_ms = Some(val(&mut i)?.parse().map_err(|_| "bad --budget-ms")?),
            "--lenient" => {
                if i + 1 < args.len() && args[i + 1].parse::<f32>().is_ok() {
                    i += 1;
                    o.lenient_ms = Some(args[i].parse().unwrap());
                } else {
                    o.lenient_ms = Some(1000.0);
                }
            }
            "--stderr-dir" => o.stderr_dir = Some(val(&mut i)?.into()),
            "--map" => o.map = val(&mut i)?,
            "--hello-map-dir" => o.map_dir_override = Some(val(&mut i)?),
            "--config" => o.config = Some(val(&mut i)?),
            "--ext-slots" => o.ext_slots = val(&mut i)?.parse().map_err(|_| "bad --ext-slots")?,
            "--full" => o.full = true,
            "--quiet" => o.quiet = true,
            x => return Err(format!("unknown option `{x}`")),
        }
        i += 1;
    }
    if o.bots.is_empty() && o.mode == Mode::Practice {
        return Err("practice needs --bot \"<cmd>\"".into());
    }
    Ok(o)
}

#[derive(Clone, Debug)]
struct SlotRes {
    slot: usize,
    cmd: Option<usize>,
    tier: Option<usize>,
    name: String,
    place: u32,
    kills: u32,
    survival: f32,
    dealt: f32,
    taken: f32,
    cause: String,
    killer: String,
    cubes: u32,
    times: Vec<f32>,
    late: u32,
    missing: u32,
    bad: u32,
    victims: String,
    taken_by: String,
}

struct MatchRes {
    idx: usize,
    seed: u64,
    wall: f32,
    sim_t: f32,
    slots: Vec<SlotRes>,
}

fn tier_name(t: Option<usize>) -> &'static str {
    t.map(|k| tiers()[k].name).unwrap_or("ext")
}

fn run_match(st: &Arc<Statics>, cfg: &Arc<Cfg>, o: &Opts, idx: usize) -> Result<MatchRes, String> {
    let t0 = Instant::now();
    let seed = mix_seed(o.seed, idx as u64);
    let mut rng = Rng::new(seed ^ 0xABCD);
    let n = cfg.mechs;
    // Slot layout: (external command index, house tier, team).
    let mut layout: Vec<(Option<usize>, Option<usize>, Option<u8>)> = vec![(None, None, None); n];
    match o.mode {
        Mode::Practice => {
            let ext = rng.below(n);
            let mut tiers: Vec<usize> = PRACTICE_TIERS.to_vec();
            while tiers.len() < n - 1 {
                tiers.push(3);
            }
            for k in (1..tiers.len()).rev() {
                let j = rng.below(k + 1);
                tiers.swap(k, j);
            }
            let mut ti = 0;
            for (s, l) in layout.iter_mut().enumerate() {
                if s == ext {
                    *l = (Some(0), None, None);
                } else {
                    *l = (None, Some(tiers[ti]), Some(SQUAD_TEAM));
                    ti += 1;
                }
            }
        }
        Mode::Ffa => {
            let mut order: Vec<usize> = (0..n).collect();
            for k in (1..n).rev() {
                let j = rng.below(k + 1);
                order.swap(k, j);
            }
            let ne = if o.bots.is_empty() { 0 } else { o.ext_slots.min(n) };
            for (k, &s) in order.iter().enumerate() {
                if k < ne {
                    layout[s] = (Some(k % o.bots.len()), None, None);
                } else {
                    layout[s] = (None, Some([3, 2, 1, 0][k % 4]), None);
                }
            }
        }
    }
    let specs: Vec<SlotSpec> = layout.iter().map(|l| SlotSpec { external: l.0.is_some(), team: l.2 }).collect();
    let mut g = Game::new(st.clone(), cfg.clone(), seed, &specs);
    if house::spawn_laser() || house::house_v() == 4 {
        // The recorded real hunter carried gun + laser within 5 s of spawning.
        for (s, l) in layout.iter().enumerate() {
            if l.1.map(house::spawn_laser_tier).unwrap_or(false) {
                let ws = g.mechs[s].ws[cfg::WK::Laser.idx()].clone();
                g.mechs[s].slots[1] = Some(Weapon::new(cfg::WK::Laser, &ws));
            }
        }
    }
    let mut nav = arena::Nav::build(&st.w);
    let mut brains: Vec<Option<Brain>> = layout.iter().enumerate().map(|(s, l)| l.1.map(|t| Brain::new(t, l.2 == Some(SQUAD_TEAM), mix_seed(seed, 100 + s as u64), n))).collect();
    let n_squad = layout.iter().filter(|l| l.2 == Some(SQUAD_TEAM)).count();
    let mut squad = Squad::new(n, n_squad, mix_seed(seed, 7));

    // External processes.
    let mut exts: Vec<Option<ExtBot>> = (0..n).map(|_| None).collect();
    let map_dir = o.map_dir_override.clone().unwrap_or(st.map_dir.clone());
    for s in 0..n {
        if let Some(c) = layout[s].0 {
            let ep = o.stderr_dir.as_ref().map(|d| d.join(format!("m{idx}_s{s}.log")));
            let mut b = ExtBot::spawn(&o.bots[c], ep)?;
            let mut hello = format!(r#"{{"type":"hello","protocol":"mechfight/1","slot":{s},"seed":{},"map":{{"dir":"#, rng.next_u64() % 4_000_000_000);
            jstr(&mut hello, &map_dir);
            hello.push_str(r#","files":["v2/world.json","v2/terrain.i16"]},"config":"#);
            hello.push_str(&cfg.raw_text);
            hello.push('}');
            b.send(&hello);
            exts[s] = Some(b);
        }
    }
    let ready_deadline = Instant::now() + Duration::from_millis(cfg.hello_budget_ms as u64);
    for s in 0..n {
        if let Some(b) = exts[s].as_mut() {
            while let Some((_, line)) = b.recv_until(ready_deadline) {
                if let Ok(v) = serde_json::from_str::<serde_json::Value>(&line) {
                    if v["type"] == "ready" {
                        b.name = v["name"].as_str().unwrap_or("").chars().filter(|c| !c.is_control()).take(32).collect();
                        break;
                    }
                }
                b.bad += 1;
            }
        }
    }

    let budget = o.budget_ms.unwrap_or(cfg.tick_budget_ms);
    let wait_ms = o.lenient_ms.unwrap_or(budget).max(budget);
    let mut last_cmd: Vec<Cmd> = vec![Cmd::default(); n];
    let mut last_ok: Vec<f32> = vec![0.0; n];
    let mut ended: Vec<bool> = vec![false; n];
    let mut replay = o.replay_dir.as_ref().map(|d| {
        let _ = std::fs::create_dir_all(d);
        std::io::BufWriter::new(std::fs::File::create(d.join(format!("match_{idx:04}.jsonl"))).expect("replay file"))
    });
    if let Some(r) = replay.as_mut() {
        let slots: Vec<String> = layout.iter().enumerate().map(|(s, l)| format!(r#"{{"slot":{s},"who":"{}"}}"#, if l.0.is_some() { "ext".to_string() } else { tier_name(l.1).to_string() })).collect();
        let _ = writeln!(r, r#"{{"type":"header","seed":{seed},"slots":[{}]}}"#, slots.join(","));
    }
    let decide = cfg.decide_every.max(1);
    let mut cmds: Vec<Option<Cmd>> = vec![None; n];
    while !g.over {
        if g.tick % decide == 0 {
            g.sync_time();
            let t = g.t;
            let mut sent: Vec<Option<Instant>> = vec![None; n];
            for s in 0..n {
                if !g.mechs[s].alive {
                    continue;
                }
                if let Some(b) = exts[s].as_mut() {
                    b.drain();
                    let line = g.build_obs(s);
                    if b.send(&line) {
                        sent[s] = Some(Instant::now());
                    }
                }
            }
            squad.update(&g, &brains);
            for s in 0..n {
                cmds[s] = None;
                if !g.mechs[s].alive {
                    continue;
                }
                if let Some(b) = brains[s].as_mut() {
                    cmds[s] = Some(b.decide(&g, s, &squad, &mut nav));
                }
            }
            let tick = g.tick as i64;
            for s in 0..n {
                if !g.mechs[s].alive {
                    continue;
                }
                let Some(b) = exts[s].as_mut() else { continue };
                let mut got: Option<Cmd> = None;
                if let Some(t_send) = sent[s] {
                    let deadline = t_send + Duration::from_micros((wait_ms * 1000.0) as u64);
                    while let Some((at, line)) = b.recv_until(deadline) {
                        if line.len() > cfg.max_line_bytes {
                            b.bad += 1;
                            continue;
                        }
                        match serde_json::from_str::<serde_json::Value>(&line) {
                            Ok(v) => {
                                if v.get("tick").and_then(|x| x.as_f64()).map(|x| x as i64) == Some(tick) {
                                    let ms = at.saturating_duration_since(t_send).as_secs_f32() * 1000.0;
                                    b.times_ms.push(ms);
                                    if ms > budget {
                                        b.late += 1;
                                    }
                                    got = Some(Cmd::parse(&v));
                                    break;
                                }
                            }
                            Err(_) => b.bad += 1,
                        }
                    }
                }
                let c = match got {
                    Some(c) => {
                        last_ok[s] = t;
                        last_cmd[s] = c.clone();
                        c
                    }
                    None => {
                        b.missing += 1;
                        if t - last_ok[s] <= cfg.coast_s + 1e-3 {
                            last_cmd[s].coast()
                        } else {
                            Cmd::default()
                        }
                    }
                };
                cmds[s] = Some(c);
            }
            for s in 0..n {
                if let Some(c) = cmds[s].take() {
                    g.apply_cmd(s, &c);
                }
            }
            g.resolve_takes();
        }
        g.step();
        if let Some(r) = replay.as_mut() {
            if g.tick % o.replay_every == 0 {
                let z = g.zone_at(g.t);
                let mut s = format!(r#"{{"tick":{},"t":"#, g.tick);
                jnum(&mut s, g.t, 2);
                s.push_str(r#","zone":["#);
                jnum(&mut s, z.center[0], 1);
                s.push(',');
                jnum(&mut s, z.center[1], 1);
                s.push(',');
                jnum(&mut s, z.radius, 1);
                let _ = std::fmt::Write::write_fmt(&mut s, format_args!(r#"],"proj":{},"m":["#, g.projs.len()));
                let mut first = true;
                for m in g.mechs.iter().filter(|m| m.alive) {
                    if !first {
                        s.push(',');
                    }
                    first = false;
                    let _ = std::fmt::Write::write_fmt(&mut s, format_args!("[{},", m.id));
                    jnum(&mut s, m.pos[0], 1);
                    s.push(',');
                    jnum(&mut s, m.pos[1], 1);
                    s.push(',');
                    jnum(&mut s, m.pos[2], 1);
                    s.push(',');
                    jnum(&mut s, m.hp, 0);
                    s.push(',');
                    jnum(&mut s, m.yaw, 2);
                    let _ = std::fmt::Write::write_fmt(&mut s, format_args!(",{}]", m.cubes));
                }
                s.push_str("]}");
                let _ = writeln!(r, "{s}");
            }
        }
        // Deaths: send `end` right away (the bot gets no more observations).
        for s in 0..n {
            if !ended[s] && !g.mechs[s].alive {
                ended[s] = true;
                if let Some(b) = exts[s].as_mut() {
                    b.send(&format!(r#"{{"type":"end","place":{},"kills":{}}}"#, g.mechs[s].place, g.mechs[s].kills));
                }
                if let Some(r) = replay.as_mut() {
                    let m = &g.mechs[s];
                    let _ = writeln!(r, r#"{{"type":"death","t":{:.2},"slot":{s},"place":{},"cause":"{}","killer":{}}}"#, g.t, m.place, m.cause, m.killer.map(|k| k.to_string()).unwrap_or("null".into()));
                }
            }
        }
        if !o.full && layout.iter().any(|l| l.0.is_some()) && layout.iter().enumerate().all(|(s, l)| l.0.is_none() || !g.mechs[s].alive) {
            break;
        }
    }
    for s in 0..n {
        if !ended[s] {
            if let Some(b) = exts[s].as_mut() {
                b.send(&format!(r#"{{"type":"end","place":{},"kills":{}}}"#, g.mechs[s].place, g.mechs[s].kills));
            }
        }
    }
    let mut slots = vec![];
    for s in 0..n {
        let m = &g.mechs[s];
        let (times, late, missing, bad, name) = match exts[s].as_mut() {
            Some(b) => {
                b.finish();
                (std::mem::take(&mut b.times_ms), b.late, b.missing, b.bad, b.name.clone())
            }
            None => (vec![], 0, 0, 0, tier_name(layout[s].1).to_string()),
        };
        let killer = m.killer.map(|k| format!("{}{}#{k}", tier_name(layout[k].1), if squad.hunter[k] { "(hunter)" } else { "" })).unwrap_or_else(|| if m.alive { "-".into() } else { m.cause.to_string() });
        let mut vl: Vec<(f32, String)> = (0..n)
            .filter(|&j| !g.mechs[j].alive && g.mechs[j].killer == Some(s))
            .map(|j| (g.mechs[j].death_t, format!("[{:.1},\"{}\",\"{}\"]", g.mechs[j].death_t, tier_name(layout[j].1), g.mechs[j].cause)))
            .collect();
        vl.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
        let mut tk: std::collections::BTreeMap<String, f32> = Default::default();
        for (a, w, amt) in &g.mechs[s].taken_log {
            *tk.entry(format!("{}/{}", tier_name(layout[*a].1), w)).or_default() += amt;
        }
        let taken_by = format!("{{{}}}", tk.iter().map(|(k, v)| format!("\"{k}\":{v:.0}")).collect::<Vec<_>>().join(","));
        let victims = format!("[{}]", vl.into_iter().map(|v| v.1).collect::<Vec<_>>().join(","));
        slots.push(SlotRes {
            victims,
            taken_by,
            slot: s,
            cmd: layout[s].0,
            tier: layout[s].1,
            name,
            place: if m.alive && m.place == 0 { 1 } else { m.place },
            kills: m.kills,
            survival: if m.alive { g.t } else { m.death_t },
            dealt: m.dmg_dealt,
            taken: m.dmg_taken,
            cause: if m.alive { "alive".into() } else { m.cause.to_string() },
            killer,
            cubes: m.cubes,
            times,
            late,
            missing,
            bad,
        });
    }
    if std::env::var("SIM_DEBUG").is_ok() {
        let mut line = format!("[dbg m{idx}]");
        for (s, m) in g.mechs.iter().enumerate() {
            line.push_str(&format!(" {}{}:{}/{}{}", tier_name(layout[s].1).chars().take(2).collect::<String>(), s, m.hits, m.shots, if m.alive { "" } else { "x" }));
        }
        eprintln!("{line}");
    }
    if let Some(r) = replay.as_mut() {
        let res: Vec<String> = slots.iter().map(|s| format!(r#"{{"slot":{},"place":{},"kills":{}}}"#, s.slot, s.place, s.kills)).collect();
        let _ = writeln!(r, r#"{{"type":"result","t":{:.2},"slots":[{}]}}"#, g.t, res.join(","));
    }
    Ok(MatchRes { idx, seed, wall: t0.elapsed().as_secs_f32(), sim_t: g.t, slots })
}

fn mean_se(v: &[f32]) -> (f32, f32) {
    if v.is_empty() {
        return (0.0, 0.0);
    }
    let n = v.len() as f32;
    let m = v.iter().sum::<f32>() / n;
    if v.len() < 2 {
        return (m, 0.0);
    }
    let var = v.iter().map(|x| (x - m).powi(2)).sum::<f32>() / (n - 1.0);
    (m, (var / n).sqrt())
}

fn pct(v: &mut [f32], p: f32) -> f32 {
    if v.is_empty() {
        return 0.0;
    }
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let k = ((v.len() as f32 - 1.0) * p).round() as usize;
    v[k.min(v.len() - 1)]
}

fn probe(args: &[String]) {
    let x: f32 = args[0].parse().unwrap();
    let y: f32 = args[1].parse().unwrap();
    let w = arena::World::load("kit/arena").unwrap();
    let z = w.surface_below(x, y, 1000.0, 0.0);
    let z = if args.len() > 2 { args[2].parse().unwrap() } else { z };
    println!("ground {} spans {:?} z {}", w.ground(x, y), w.spans_at(x, y), z);
    let b = phys::Body { r: 2.5, h: 9.0, step: 1.2, slope_tan: 50f32.to_radians().tan() };
    println!("blocked here: {}", phys::blocked(&w, x, y, z, &b));
    for dj in (-6..=6).rev() {
        let mut row = String::new();
        for di in -6..=6 {
            let sp = w.spans_at(x + di as f32, y + dj as f32);
            let bl = sp.iter().any(|&(lo, hi)| lo <= hi && lo < z + 8.95 && hi > z + 1.2);
            let any = sp.iter().any(|&(lo, hi)| lo <= hi);
            row.push(if bl { '#' } else if any { '+' } else { '.' });
        }
        println!("{row}");
    }
    for k in 0..16 {
        let a = k as f32 * std::f32::consts::TAU / 16.0;
        let (sx, sy) = (x - 0.6 + a.cos() * 2.5, y + a.sin() * 2.5);
        println!("ring {k} ({:.2},{:.2}) spans {:?} inb {}", sx - x, sy - y, w.spans_at(sx, sy), w.in_bounds(x - 0.6, y, 2.5));
    }
    for dj in [-3i32, 0, 3] { for di in [-3i32, 0, 3] { println!("{di},{dj}: {:?}", w.spans_at(x + di as f32, y + dj as f32)); } }
    for k in 0..8 {
        let a = k as f32 * std::f32::consts::FRAC_PI_4;
        let (dx, dy) = (a.cos() * 0.6, a.sin() * 0.6);
        let s = phys::surface(&w, x + dx, y + dy, z, 1.2);
        println!("dir {:>3}deg -> {:?}  ground there {} blocked@z {} surf {} blocked@s {}", k * 45, phys::try_move(&w, [x, y, z], dx, dy, &b, true), w.ground(x + dx, y + dy), phys::blocked(&w, x + dx, y + dy, z, &b), s, phys::blocked(&w, x + dx, y + dy, s, &b));
    }
}

fn main() {
    if std::env::args().nth(1).as_deref() == Some("probe") {
        let a: Vec<String> = std::env::args().skip(2).collect();
        probe(&a);
        return;
    }
    let o = match parse_args() {
        Ok(o) => o,
        Err(e) => {
            if !e.is_empty() {
                eprintln!("error: {e}\n");
            }
            eprint!("{USAGE}");
            std::process::exit(if e.is_empty() { 0 } else { 2 });
        }
    };
    let cfg_text = match &o.config {
        Some(p) => std::fs::read_to_string(p).unwrap_or_else(|e| {
            eprintln!("{p}: {e}");
            std::process::exit(2)
        }),
        None => cfg::DEFAULT_CONFIG.to_string(),
    };
    let cfg = Arc::new(Cfg::parse(&cfg_text).unwrap_or_else(|e| {
        eprintln!("{e}");
        std::process::exit(2)
    }));
    let t_load = Instant::now();
    let st = Arc::new(Statics::load(&o.map).unwrap_or_else(|e| {
        eprintln!("map: {e}");
        std::process::exit(2)
    }));
    if let Some(d) = &o.stderr_dir {
        let _ = std::fs::create_dir_all(d);
    }
    eprintln!(
        "map loaded in {:.2}s ({} ground pts, {} roof pts); {} matches on {} jobs; budget {} ms{}",
        t_load.elapsed().as_secs_f32(),
        st.ground_pts.len(),
        st.roof_pts.len(),
        o.matches,
        o.jobs.min(o.matches.max(1)),
        o.budget_ms.unwrap_or(cfg.tick_budget_ms),
        o.lenient_ms.map(|l| format!(" (lenient: wait up to {l} ms)")).unwrap_or_default()
    );
    let t_all = Instant::now();
    let next = AtomicUsize::new(0);
    let results: Mutex<Vec<MatchRes>> = Mutex::new(vec![]);
    let out_file = o.out.as_ref().map(|p| {
        if let Some(d) = p.parent() {
            let _ = std::fs::create_dir_all(d);
        }
        Mutex::new(std::fs::OpenOptions::new().create(true).append(true).open(p).expect("--out file"))
    });
    std::thread::scope(|sc| {
        for _ in 0..o.jobs.min(o.matches.max(1)) {
            sc.spawn(|| loop {
                let k = next.fetch_add(1, Ordering::SeqCst);
                if k >= o.matches {
                    break;
                }
                match run_match(&st, &cfg, &o, k) {
                    Ok(r) => {
                        for s in r.slots.iter().filter(|s| s.cmd.is_some()) {
                            if !o.quiet {
                                let mut tm = s.times.clone();
                                eprintln!(
                                    "match {:>3} slot {:>2} [{}] place {:>2} score {:>2} kills {} surv {:>5.1}s dealt {:>5.0} taken {:>5.0} cubes {} death {} by {} | p50 {:.1}ms max {:.1}ms late {} miss {} | {:.1}s wall",
                                    r.idx,
                                    s.slot,
                                    s.name,
                                    s.place,
                                    16 - s.place as i32,
                                    s.kills,
                                    s.survival,
                                    s.dealt,
                                    s.taken,
                                    s.cubes,
                                    s.cause,
                                    s.killer,
                                    pct(&mut tm, 0.5),
                                    pct(&mut tm, 1.0),
                                    s.late,
                                    s.missing,
                                    r.wall
                                );
                            }
                            if let Some(f) = &out_file {
                                let mut tm = s.times.clone();
                                let line = format!(
                                    r#"{{"match":{},"seed":{},"slot":{},"cmd":{},"name":{},"place":{},"score":{},"kills":{},"survival":{:.2},"dealt":{:.1},"taken":{:.1},"cubes":{},"cause":"{}","killer":"{}","p50_ms":{:.2},"p99_ms":{:.2},"max_ms":{:.2},"late":{},"missing":{},"bad":{},"wall_s":{:.2},"sim_s":{:.2},"victims":{},"taken_by":{}}}"#,
                                    r.idx,
                                    r.seed,
                                    s.slot,
                                    s.cmd.unwrap(),
                                    serde_json::to_string(&s.name).unwrap(),
                                    s.place,
                                    16 - s.place as i32,
                                    s.kills,
                                    s.survival,
                                    s.dealt,
                                    s.taken,
                                    s.cubes,
                                    s.cause,
                                    s.killer,
                                    pct(&mut tm, 0.5),
                                    pct(&mut tm, 0.99),
                                    pct(&mut tm, 1.0),
                                    s.late,
                                    s.missing,
                                    s.bad,
                                    r.wall,
                                    r.sim_t,
                                    s.victims,
                                    s.taken_by
                                );
                                let _ = writeln!(f.lock().unwrap(), "{line}");
                            }
                        }
                        results.lock().unwrap().push(r);
                    }
                    Err(e) => eprintln!("match {k}: {e}"),
                }
            });
        }
    });
    let mut res = results.into_inner().unwrap();
    res.sort_by_key(|r| r.idx);
    let wall = t_all.elapsed().as_secs_f32();
    println!("\n=== {} matches in {:.1}s ({:.2}s/match wall per job, {:.2}s/match throughput) ===", res.len(), wall, res.iter().map(|r| r.wall).sum::<f32>() / res.len().max(1) as f32, wall / res.len().max(1) as f32);
    for (c, cmd) in o.bots.iter().enumerate() {
        let slots: Vec<&SlotRes> = res.iter().flat_map(|r| r.slots.iter()).filter(|s| s.cmd == Some(c)).collect();
        if slots.is_empty() {
            continue;
        }
        let score: Vec<f32> = slots.iter().map(|s| 16.0 - s.place as f32).collect();
        let (ms, se) = mean_se(&score);
        let f = |g: &dyn Fn(&SlotRes) -> f32| slots.iter().map(|s| g(s)).sum::<f32>() / slots.len() as f32;
        println!("bot {c}: `{cmd}` ({} slots)", slots.len());
        println!("  SCORE mean(16-place) = {ms:.3} +- {se:.3} (se)   mean place {:.2}", f(&|s| s.place as f32));
        if std::env::var("SIM_HITSTATS").is_ok() {
            let h = *game::HITSTATS.lock().unwrap();
            let mut line = String::from("  HITSTATS house vs ext (hit%/shots) <40,40-80,80-120,120-160,160+: gun");
            for b in 0..5 {
                line.push_str(&format!(" {:.0}%/{}", 100.0 * h[5 + b] as f32 / h[b].max(1) as f32, h[b]));
            }
            line.push_str(" | laser");
            for b in 0..5 {
                line.push_str(&format!(" {:.0}%/{}", 100.0 * h[15 + b] as f32 / h[10 + b].max(1) as f32, h[10 + b]));
            }
            println!("{line}");
        }
        println!("  kills {:.2}  survival {:.1}s  dealt {:.0}  taken {:.0}  cubes {:.1}", f(&|s| s.kills as f32), f(&|s| s.survival), f(&|s| s.dealt), f(&|s| s.taken), f(&|s| s.cubes as f32));
        let mut causes: BTreeMap<String, usize> = BTreeMap::new();
        let mut killers: BTreeMap<String, usize> = BTreeMap::new();
        for s in &slots {
            *causes.entry(s.cause.clone()).or_default() += 1;
            if s.cause != "alive" {
                let k = s.killer.split('#').next().unwrap_or("").to_string();
                *killers.entry(k).or_default() += 1;
            }
        }
        println!("  death cause: {}", causes.iter().map(|(k, v)| format!("{k} {v}")).collect::<Vec<_>>().join(", "));
        println!("  killed by:   {}", killers.iter().map(|(k, v)| format!("{k} {v}")).collect::<Vec<_>>().join(", "));
        let mut all: Vec<f32> = slots.iter().flat_map(|s| s.times.iter().copied()).collect();
        let late: u32 = slots.iter().map(|s| s.late).sum();
        let miss: u32 = slots.iter().map(|s| s.missing).sum();
        let bad: u32 = slots.iter().map(|s| s.bad).sum();
        let n_ans = all.len();
        println!(
            "  timing: {} answers p50 {:.2}ms p99 {:.2}ms max {:.2}ms; late(>{}ms) {} ({:.2}%), missing {}, bad lines {}",
            n_ans,
            pct(&mut all, 0.5),
            pct(&mut all, 0.99),
            pct(&mut all, 1.0),
            o.budget_ms.unwrap_or(cfg.tick_budget_ms),
            late,
            100.0 * late as f32 / (n_ans.max(1)) as f32,
            miss,
            bad
        );
    }
    if o.mode == Mode::Ffa {
        let full = o.full || o.bots.is_empty();
        for tier in 0..tiers().len() {
            let house: Vec<f32> = res.iter().flat_map(|r| r.slots.iter()).filter(|s| s.cmd.is_none() && s.tier == Some(tier)).map(|s| 16.0 - s.place as f32).collect();
            if !house.is_empty() && full {
                let (m, se) = mean_se(&house);
                println!("house filler {:>6}: mean(16-place) = {m:.3} +- {se:.3} ({} slots)", tiers()[tier].name, house.len());
            }
        }
        if !full {
            println!("(house filler scores need --full: matches stop once every external bot is dead)");
        }
    }
}
