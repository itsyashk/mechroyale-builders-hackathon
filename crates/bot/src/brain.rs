//! Decision making: a small utility-style mode selector (zone / cover / fight / loot / farm /
//! drop / roam) on top of grid navigation, cover search, and predictive aiming.

use crate::aim::*;
use crate::proto::*;
use arena::{Nav, World};
use serde_json::Value;
use std::collections::{HashMap, VecDeque};
use std::f32::consts::PI;

pub struct Rng(u64);
impl Rng {
    pub fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }
    pub fn f(&mut self) -> f32 {
        (self.next() >> 40) as f32 / (1u64 << 24) as f32
    }
    pub fn range(&mut self, a: f32, b: f32) -> f32 {
        a + (b - a) * self.f()
    }
}

#[derive(Clone, Debug)]
struct Track {
    pos: V3,
    vel: V3,
    aim: [f32; 2],
    hp: f32,
    max_hp: f32,
    weapons: Vec<String>,
    charging: bool,
    cubes: i32,
    seen_t: f32,
    first_t: f32,
    /// EMA of the angle between its aim and a perfect gun lead on us (rad): skill estimate.
    aerr: f32,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Mode {
    Zone,
    Cover,
    Fight,
    Loot,
    Farm,
    Drop,
    Roam,
    Hold,
}

#[derive(Clone, Copy)]
enum TargetKind {
    Mech,
    Npc,
    Crate,
}

#[derive(Clone, Copy)]
struct Target {
    id: i64,
    pos: V3,
    vel: V3,
    kind: TargetKind,
    scale: f32,
}

pub struct Brain {
    world: World,
    nav: Nav,
    cfg: Value,
    rng: Rng,
    t: f32,
    tracks: HashMap<i64, Track>,
    pickups: HashMap<i64, (String, V3, f32)>,
    bad_pickups: HashMap<i64, f32>,
    path: Vec<[f32; 2]>,
    path_goal: [f32; 2],
    path_t: f32,
    strafe_sign: f32,
    strafe_switch_t: f32,
    pos_hist: VecDeque<(f32, [f32; 2], bool)>,
    unstick_until: f32,
    unstick_dir: [f32; 2],
    last_dmg_t: f32,
    last_deal_t: f32,
    dmg_log: Vec<(f32, f32, Option<i64>, f32)>, // t, bearing, by, amount
    retreating: bool,
    cover_goal: Option<([f32; 2], f32)>,
    target_id: Option<i64>,
    roam_goal: Option<([f32; 2], f32)>,
    boosting: bool,
    mode: Mode,
    ticks: u64,
    debug: bool,
    debug_every: u64,
    gstat: [u32; 7],
    dodge_goal: Option<[f32; 2]>,
    dodge_check_t: f32,
    dodges: u32,
    last_ttk: f32,
    prm: Params,
    tac_t: f32,
    hold_spot: Option<([f32; 2], f32)>,
    hold_eval_t: f32,
    tac_goal: Option<[f32; 2]>,
    escape_until: f32,
    allow_escape: bool,
    escape_goal: [f32; 2],
    last_ttd: f32,
    // config numbers
    pickup_radius: f32,
    cube_radius: f32,
    gravity: f32,
}

fn cfgf(v: &Value, path: &[&str], d: f32) -> f32 {
    let mut c = v;
    for p in path {
        match c.get(*p) {
            Some(x) => c = x,
            None => return d,
        }
    }
    c.as_f64().map(|x| x as f32).unwrap_or(d)
}

fn statf(me: &Me, k: &str, d: f32) -> f32 {
    me.stats.get(k).and_then(|v| v.as_f64()).map(|v| v as f32).unwrap_or(d)
}

/// Strategy knobs. Defaults are the tuned values; MECHBOT_PARAMS="k=v,k=v" overrides
/// them for local experiments.
#[derive(Clone, Debug)]
pub struct Params {
    pub p_laser: f32,
    pub p_light: f32,
    pub p_gun: f32,
    pub p_mortar: f32,
    pub light_lo: f32,
    pub light_hi: f32,
    pub laser_lo: f32,
    pub laser_hi: f32,
    pub gun_lo: f32,
    pub gun_hi: f32,
    pub vs_light_lo: f32,
    pub retreat_ratio: f32,
    pub retreat_hp: f32,
    pub farm_hp: f32,
    pub farm_range: f32,
    pub engage_all: f32,
    pub drops: f32,
    pub upg: f32,
    pub magnet: f32,
    pub tactic: f32,
    pub heal: f32,
    pub kite: f32,
    pub turn_unseen: f32,
    pub hp_w: f32,
    pub multi_far: f32,
    pub weak_first: f32,
    pub treload: f32,
    pub hold: f32,
    pub hold_t: f32,
    pub hold_r: f32,
    pub leash: f32,
    pub kite_light: f32,
    pub st_lo: f32,
    pub st_hi: f32,
    pub radw: f32,
    pub fboost: f32,
    pub finish: f32,
    pub pick_until: f32,
    pub pick_aerr: f32,
}

impl Default for Params {
    fn default() -> Self {
        Params {
            p_laser: 4.0,
            p_light: 3.0,
            p_gun: 3.5,
            p_mortar: 1.5,
            light_lo: 25.0,
            light_hi: 38.0,
            laser_lo: 80.0,
            laser_hi: 170.0,
            gun_lo: 50.0,
            gun_hi: 100.0,
            vs_light_lo: 56.0,
            retreat_ratio: 0.85,
            retreat_hp: 0.6,
            farm_hp: 0.6,
            farm_range: 70.0,
            engage_all: 1.0,
            drops: 1.0,
            upg: 0.0,
            magnet: 50.0,
            tactic: 0.0,
            heal: 0.0,
            kite: 0.0,
            turn_unseen: 0.0,
            hp_w: 0.04,
            multi_far: 0.0,
            weak_first: 0.0,
            treload: 0.0,
            hold: 0.0,
            hold_t: 40.0,
            hold_r: 70.0,
            leash: 10.0,
            kite_light: 55.0,
            st_lo: 0.45,
            st_hi: 1.3,
            radw: 1.2,
            fboost: 0.0,
            finish: 2.0,
            pick_until: 0.0,
            pick_aerr: 0.09,
        }
    }
}

impl Params {
    pub fn from_env() -> Params {
        let mut p = Params::default();
        if let Ok(v) = std::env::var("MECHBOT_PARAMS") {
            for kv in v.split(',') {
                let mut it = kv.splitn(2, '=');
                let (k, val) = (it.next().unwrap_or("").trim(), it.next().and_then(|x| x.trim().parse::<f32>().ok()));
                let Some(x) = val else { continue };
                match k {
                    "p_laser" => p.p_laser = x,
                    "p_light" => p.p_light = x,
                    "p_gun" => p.p_gun = x,
                    "p_mortar" => p.p_mortar = x,
                    "light_lo" => p.light_lo = x,
                    "light_hi" => p.light_hi = x,
                    "laser_lo" => p.laser_lo = x,
                    "laser_hi" => p.laser_hi = x,
                    "gun_lo" => p.gun_lo = x,
                    "gun_hi" => p.gun_hi = x,
                    "vs_light_lo" => p.vs_light_lo = x,
                    "retreat_ratio" => p.retreat_ratio = x,
                    "retreat_hp" => p.retreat_hp = x,
                    "farm_hp" => p.farm_hp = x,
                    "farm_range" => p.farm_range = x,
                    "engage_all" => p.engage_all = x,
                    "drops" => p.drops = x,
                    "upg" => p.upg = x,
                    "magnet" => p.magnet = x,
                    "tactic" => p.tactic = x,
                    "heal" => p.heal = x,
                    "kite" => p.kite = x,
                    "turn_unseen" => p.turn_unseen = x,
                    "hp_w" => p.hp_w = x,
                    "multi_far" => p.multi_far = x,
                    "weak_first" => p.weak_first = x,
                    "treload" => p.treload = x,
                    "hold" => p.hold = x,
                    "hold_t" => p.hold_t = x,
                    "hold_r" => p.hold_r = x,
                    "leash" => p.leash = x,
                    "kite_light" => p.kite_light = x,
                    "st_lo" => p.st_lo = x,
                    "st_hi" => p.st_hi = x,
                    "radw" => p.radw = x,
                    "fboost" => p.fboost = x,
                    "finish" => p.finish = x,
                    "pick_until" => p.pick_until = x,
                    "pick_aerr" => p.pick_aerr = x,
                    _ => eprintln!("unknown param {k}"),
                }
            }
        }
        p
    }

    fn prio(&self, kind: &str) -> f32 {
        match kind {
            "laser" => self.p_laser,
            "lightning" => self.p_light,
            "gun" => self.p_gun,
            "mortar" => self.p_mortar,
            _ => 0.0,
        }
    }
}

const UPGRADE_RANK_DPS: &[&str] = &[
    "gun_rate",
    "gun_damage",
    "laser_damage",
    "plating",
    "laser_charge",
    "armor",
    "gun_mag",
    "nanites",
    "gyros",
    "lightning_efficiency",
    "lightning_range",
    "gun_velocity",
    "servos",
    "capacitor",
    "reactor",
    "jets",
    "sensors",
    "lightning_chain",
    "mortar_reload",
    "mortar_velocity",
    "mortar_splash",
    "laser_pierce",
];

/// Sustained gun DPS is reload-bound (12 rounds in 2.9 s, then 2.4 s reload), so the drum
/// magazine is worth ~+31% gun DPS: rank it with plating.
const UPGRADE_RANK_MAG: &[&str] = &[
    "plating",
    "gun_mag",
    "armor",
    "laser_damage",
    "gun_damage",
    "nanites",
    "laser_charge",
    "gun_rate",
    "gyros",
    "lightning_efficiency",
    "lightning_range",
    "servos",
    "capacitor",
    "reactor",
    "gun_velocity",
    "jets",
    "sensors",
    "lightning_chain",
    "mortar_reload",
    "mortar_velocity",
    "mortar_splash",
    "laser_pierce",
];

const UPGRADE_RANK: &[&str] = &[
    "plating",
    "armor",
    "laser_damage",
    "nanites",
    "laser_charge",
    "gyros",
    "gun_damage",
    "gun_rate",
    "lightning_efficiency",
    "lightning_range",
    "servos",
    "capacitor",
    "reactor",
    "gun_mag",
    "gun_velocity",
    "jets",
    "sensors",
    "lightning_chain",
    "mortar_reload",
    "mortar_velocity",
    "mortar_splash",
    "laser_pierce",
];

impl Brain {
    pub fn new(hello_line: &str) -> Result<Brain, String> {
        let hello: Value = serde_json::from_str(hello_line).map_err(|e| e.to_string())?;
        let dir = hello["map"]["dir"].as_str().unwrap_or("/arena").to_string();
        let world = World::load(&dir).or_else(|e| {
            eprintln!("map load from {dir} failed: {e}; trying /arena");
            World::load("/arena")
        })?;
        let nav = Nav::build(&world);
        let cfg = hello["config"].clone();
        let seed = hello["seed"].as_u64().unwrap_or(12345) | 1;
        Ok(Brain {
            world,
            nav,
            pickup_radius: cfgf(&cfg, &["loot", "pickup_radius"], 4.0),
            cube_radius: cfgf(&cfg, &["cubes", "pickup_radius"], 3.5),
            gravity: cfgf(&cfg, &["mech", "gravity"], 22.0),
            cfg,
            rng: Rng(seed.wrapping_mul(0x9E3779B97F4A7C15) | 1),
            t: 0.0,
            tracks: HashMap::new(),
            pickups: HashMap::new(),
            bad_pickups: HashMap::new(),
            path: Vec::new(),
            path_goal: [1e9, 1e9],
            path_t: -100.0,
            strafe_sign: 1.0,
            strafe_switch_t: 0.0,
            pos_hist: VecDeque::new(),
            unstick_until: -1.0,
            unstick_dir: [1.0, 0.0],
            last_dmg_t: -100.0,
            last_deal_t: -100.0,
            dmg_log: Vec::new(),
            retreating: false,
            cover_goal: None,
            target_id: None,
            roam_goal: None,
            boosting: false,
            mode: Mode::Roam,
            ticks: 0,
            dodge_goal: None,
            dodge_check_t: -1.0,
            dodges: 0,
            last_ttk: 0.0,
            prm: Params::from_env(),
            tac_t: -1.0,
            hold_spot: None,
            hold_eval_t: -100.0,
            tac_goal: None,
            escape_until: -1.0,
            allow_escape: true,
            escape_goal: [0.0, 0.0],
            last_ttd: 0.0,
            debug: std::env::var("MECHBOT_DEBUG").is_ok(),
            gstat: [0; 7],
            debug_every: std::env::var("MECHBOT_DEBUG").ok().and_then(|v| v.parse().ok()).unwrap_or(10).max(1),
        })
    }

    pub fn recover(&mut self) {
        self.path.clear();
        self.cover_goal = None;
        self.roam_goal = None;
    }

    pub fn log_end(&self, raw: &str) {
        eprintln!("end: {raw} gunstat[mech_ticks,fire,nolos,range,ammo/reload,aim,hold]={:?}", self.gstat);
    }

    // ------------------------------------------------------------------ helpers --------

    fn zone_now(z: &Zone, t: f32) -> ([f32; 2], f32) {
        if t <= z.shrink_start || z.shrink_end <= z.shrink_start {
            return (z.center, z.radius);
        }
        let f = ((t - z.shrink_start) / (z.shrink_end - z.shrink_start)).clamp(0.0, 1.0);
        (
            [z.center[0] + (z.next_center[0] - z.center[0]) * f, z.center[1] + (z.next_center[1] - z.center[1]) * f],
            z.radius + (z.next_radius - z.radius) * f,
        )
    }

    /// The circle every goal should lie in right now, and whether we must hurry.
    fn goal_circle(&self, obs: &Obs, walk: f32) -> ([f32; 2], f32, bool) {
        let z = &obs.zone;
        let pos = xy(obs.me.pos);
        let (cc, cr) = Self::zone_now(z, obs.t);
        let nr = z.next_radius;
        let margin_next = (nr * 0.25).clamp(2.0, 30.0);
        let d_next = (dist2(pos, z.next_center) - (nr - margin_next)).max(0.0);
        let need_t = d_next / (walk * 0.85) + 10.0;
        let to_start = z.shrink_start - obs.t;
        let urgent_cur = !obs.me.in_zone || dist2(pos, cc) > cr - 6.0;
        if need_t > to_start || obs.t > z.shrink_start {
            let r = (nr - margin_next).max(1.5);
            (z.next_center, r, urgent_cur || need_t > to_start + 0.0 && d_next > 0.0 && obs.t > z.shrink_start - 5.0)
        } else {
            let m = (cr * 0.12).clamp(8.0, 40.0);
            (cc, (cr - m).max(2.0), urgent_cur)
        }
    }

    fn clamp_into(p: [f32; 2], c: [f32; 2], r: f32) -> [f32; 2] {
        let d = dist2(p, c);
        if d <= r || d < 1e-3 {
            return p;
        }
        [c[0] + (p[0] - c[0]) / d * r, c[1] + (p[1] - c[1]) / d * r]
    }

    /// Follow an A* path toward `goal`. Returns a unit move vector.
    fn goto(&mut self, pos: [f32; 2], goal: [f32; 2]) -> [f32; 2] {
        let replan = self.path.is_empty() || dist2(goal, self.path_goal) > 5.0 || self.t - self.path_t > 2.0;
        if replan {
            self.path = self.nav.path(pos, goal, 60_000);
            self.path_goal = goal;
            self.path_t = self.t;
            // Unreachable on foot (walled pocket, roof): jet straight over toward the goal.
            let end = self.path.last().copied().unwrap_or(pos);
            if self.allow_escape && dist2(end, goal) > 10.0 && dist2(pos, goal) > 10.0 && dist2(end, pos) < 12.0 {
                self.escape_until = self.t + 2.5;
                self.escape_goal = goal;
            }
        }
        if self.t < self.escape_until {
            return unit2([self.escape_goal[0] - pos[0], self.escape_goal[1] - pos[1]]);
        }
        while let Some(&w) = self.path.first() {
            if dist2(w, pos) < 2.2 && self.path.len() > 1 {
                self.path.remove(0);
            } else {
                break;
            }
        }
        match self.path.first() {
            Some(&w) => {
                if dist2(w, pos) < 0.8 {
                    [0.0, 0.0]
                } else {
                    unit2([w[0] - pos[0], w[1] - pos[1]])
                }
            }
            None => unit2([goal[0] - pos[0], goal[1] - pos[1]]),
        }
    }

    fn eye_of(p: V3, scale: f32) -> V3 {
        [p[0], p[1], p[2] + 8.0 * scale]
    }

    /// Search for a walkable spot near `pos` hidden from every threat eye.
    /// How enclosed a spot is: (rays blocked within 18 m out of 16, open lanes).
    fn nook_score(&self, c: [f32; 2]) -> f32 {
        let g = self.world.ground(c[0], c[1]);
        let eye = [c[0], c[1], g + 6.5];
        let mut blocked = 0;
        for k in 0..16 {
            let a = k as f32 * PI / 8.0;
            let d = [a.cos(), a.sin(), 0.0];
            if self.world.raycast(eye, d, 18.0) < 17.5 {
                blocked += 1;
            }
        }
        // Want walls on most sides but a few lanes to shoot down.
        let b = blocked as f32;
        if blocked >= 15 {
            return 9.0;
        }
        b.min(12.0) - if blocked > 12 { (b - 12.0) * 1.5 } else { 0.0 }
    }

    /// Picks a defensible spot near `pos` inside the circle (re-uses the current one if still good).
    fn pick_hold_spot(&mut self, pos: [f32; 2], circle: ([f32; 2], f32), t: f32) {
        let (gc, gr) = circle;
        let inside = |c: [f32; 2]| dist2(c, gc) < gr * 0.8;
        if let Some((sp, sc)) = self.hold_spot {
            if inside(sp) && t - self.hold_eval_t < 12.0 {
                return;
            }
            if !inside(sp) {
                self.hold_spot = None;
            } else {
                let _ = sc;
            }
        }
        self.hold_eval_t = t;
        let mut best: Option<(f32, [f32; 2], f32)> = self.hold_spot.map(|(sp, _)| {
            let ns = self.nook_score(sp);
            (ns - dist2(sp, pos) / 40.0, sp, ns)
        });
        let r = self.prm.hold_r;
        // Anchor the search on the part of the circle near us.
        let anchor = if inside(pos) { pos } else { [gc[0] + (pos[0] - gc[0]) * 0.5, gc[1] + (pos[1] - gc[1]) * 0.5] };
        for _ in 0..140 {
            let a = self.rng.f() * 2.0 * PI;
            let rr = r * self.rng.f().sqrt();
            let c = [anchor[0] + rr * a.cos(), anchor[1] + rr * a.sin()];
            if !inside(c) || !self.nav.walkable_at(c[0], c[1]) || self.nav.clearance_at(c[0], c[1]) < 3.2 {
                continue;
            }
            let ns = self.nook_score(c);
            let sc = ns - dist2(c, pos) / 40.0;
            if best.map(|b| sc > b.0).unwrap_or(true) {
                best = Some((sc, c, ns));
            }
        }
        self.hold_spot = best.map(|b| (b.1, b.2));
    }

    fn find_cover(&mut self, pos: V3, threats: &[V3], circle: ([f32; 2], f32), away_from: Option<[f32; 2]>, max_r: f32) -> Option<[f32; 2]> {
        let mut best: Option<(f32, [f32; 2])> = None;
        let rings = [5.0f32, 9.0, 14.0, 20.0, 28.0, 38.0];
        let n_dir = 20;
        for (ri, &r) in rings.iter().enumerate() {
            if r > max_r {
                break;
            }
            for k in 0..n_dir {
                let a = (k as f32 + 0.5 * (ri % 2) as f32) * 2.0 * PI / n_dir as f32;
                let c = [pos[0] + r * a.cos(), pos[1] + r * a.sin()];
                if !self.nav.walkable_at(c[0], c[1]) {
                    continue;
                }
                if dist2(c, circle.0) > circle.1 {
                    continue;
                }
                let g = self.world.ground(c[0], c[1]);
                let mut covered = 0;
                for e in threats {
                    let hid_mid = !self.world.los(*e, [c[0], c[1], g + 4.5]);
                    let hid_top = !self.world.los(*e, [c[0], c[1], g + 8.8]);
                    if hid_mid && hid_top {
                        covered += 1;
                    }
                }
                if covered == 0 {
                    continue;
                }
                let mut score = covered as f32 * 100.0 - r * 1.2;
                if covered == threats.len() {
                    score += 200.0;
                }
                if !self.nav.line_walkable(xy(pos), c) {
                    score -= 25.0;
                }
                // Stay away from threats: penalise spots closer to them than we are.
                for e in threats {
                    let dn = dist2(c, xy(*e));
                    if dn < 25.0 {
                        score -= (25.0 - dn) * 4.0;
                    }
                }
                if let Some(af) = away_from {
                    score += (dist2(c, af) - dist2(xy(pos), af)) * 0.6;
                }
                if best.map(|b| score > b.0).unwrap_or(true) {
                    best = Some((score, c));
                }
            }
        }
        best.map(|b| b.1)
    }

    /// Direction to sidestep incoming projectiles, if any threaten us.
    fn projectile_dodge(&self, obs: &Obs, my_scale: f32) -> Option<[f32; 2]> {
        let me = &obs.me;
        let c = [me.pos[0], me.pos[1], me.pos[2] + 4.5 * my_scale];
        let r = 2.5 * my_scale;
        let g = cfgf(&self.cfg, &["weapons", "mortar", "gravity"], 22.0);
        let splash = cfgf(&self.cfg, &["weapons", "mortar", "splash_radius"], 7.0);
        let mut acc = [0.0f32, 0.0];
        let mut any = false;
        for p in &obs.projectiles {
            if p.kind == "mortar" {
                // Integrate the shell to impact.
                let (mut q, mut v) = (p.pos, p.vel);
                let mut t = 0.0;
                while t < 5.0 {
                    v[2] -= g * 0.05;
                    q = add(q, scale(v, 0.05));
                    t += 0.05;
                    if q[2] < self.world.surface_below(q[0], q[1], q[2] + 2.0, 0.0) || self.world.solid(q[0], q[1], q[2]) {
                        break;
                    }
                    if q[2] > c[2] - 4.5 && q[2] < c[2] + 4.5 && hdist(q, c) < r + 0.5 {
                        break;
                    }
                }
                let d = hdist(q, c);
                if d < splash + r + 2.0 && t < 4.0 {
                    let away = unit2([c[0] - q[0], c[1] - q[1]]);
                    let away = if away == [0.0, 0.0] { unit2([-v[1], v[0]]) } else { away };
                    acc[0] += away[0] * 2.0;
                    acc[1] += away[1] * 2.0;
                    any = true;
                }
                continue;
            }
            // Straight bullets: closest approach.
            let rel = sub(c, p.pos);
            let vv = p.vel;
            let v2 = vv[0] * vv[0] + vv[1] * vv[1] + vv[2] * vv[2];
            if v2 < 1.0 {
                continue;
            }
            let tca = (rel[0] * vv[0] + rel[1] * vv[1] + rel[2] * vv[2]) / v2;
            if !(0.04..=1.2).contains(&tca) {
                continue;
            }
            let cp = add(p.pos, scale(vv, tca));
            let miss = sub(cp, c);
            let hmiss = (miss[0] * miss[0] + miss[1] * miss[1]).sqrt();
            if hmiss < r + 1.2 && miss[2].abs() < 5.5 * my_scale {
                // Move perpendicular to the bullet, toward the side it already misses on.
                let mut side = unit2([-vv[1], vv[0]]);
                if side[0] * miss[0] + side[1] * miss[1] > 0.0 {
                    side = [-side[0], -side[1]];
                }
                let w = if p.kind == "npc" { 0.4 } else { 1.0 };
                acc[0] += side[0] * w;
                acc[1] += side[1] * w;
                any = true;
            }
        }
        if any {
            Some(unit2(acc))
        } else {
            None
        }
    }

    #[allow(dead_code)]
    fn visible_from(&self, eye: V3, p: V3, scale: f32) -> bool {
        self.world.los(eye, [p[0], p[1], p[2] + 4.5 * scale]) || self.world.los(eye, [p[0], p[1], p[2] + 8.8 * scale])
    }

    // ------------------------------------------------------------------ main ----------

    pub fn act(&mut self, obs: &Obs) -> Action {
        self.ticks += 1;
        self.t = obs.t;
        let me = &obs.me;
        let pos = me.pos;
        let p2 = xy(pos);
        let my_scale = 1.0 + 0.05 * me.cubes as f32;
        let muzzle_h = statf(me, "muzzle_height", 7.0);
        let eye = [pos[0], pos[1], pos[2] + statf(me, "eye_height", 8.0)];
        let muzzle = [pos[0], pos[1], pos[2] + muzzle_h];
        let walk = statf(me, "walk_speed", 11.7);
        let max_hp = me.max_hp.max(1.0);
        let hp_frac = me.hp / max_hp;
        let mut act = Action::default();

        // ---- memory updates
        for e in &obs.events {
            match e.kind.as_str() {
                "damaged" => {
                    self.last_dmg_t = obs.t;
                    if self.debug {
                        eprintln!("  dmg t={:.1} {:?} by={:?} amt={:.0} rear={:?}", obs.t, e.weapon, e.by, e.amount.unwrap_or(0.0), e.rear);
                    }
                    if let Some(b) = e.bearing {
                        self.dmg_log.push((obs.t, b, e.by, e.amount.unwrap_or(0.0)));
                    }
                }
                "hit" => self.last_deal_t = obs.t,
                "kill" if self.debug => {
                    let ae = e.target.and_then(|id| self.tracks.get(&id)).map(|t| t.aerr).unwrap_or(-1.0);
                    eprintln!("  KILL t={:.1} target={:?} hp={:.0}/{:.0} vis={} aerr={:.3}", obs.t, e.target, me.hp, me.max_hp, obs.mechs.iter().filter(|m| m.is_mech()).count(), ae);
                }
                "take_failed" => {
                    if let Some(id) = e.id.as_ref().and_then(|v| v.as_i64()) {
                        self.bad_pickups.insert(id, obs.t);
                    }
                }
                _ => {}
            }
        }
        self.dmg_log.retain(|d| obs.t - d.0 < 6.0);
        for m in obs.mechs.iter().filter(|m| m.is_mech() && m.id != me.id) {
            let first_t = self.tracks.get(&m.id).map(|t| t.first_t).unwrap_or(obs.t);
            let mut aerr = self.tracks.get(&m.id).map(|t| t.aerr).unwrap_or(0.05);
            {
                let d = hdist(m.pos, pos);
                let their_muz = [m.pos[0], m.pos[1], m.pos[2] + 7.0];
                let my_c = [pos[0], pos[1], pos[2] + 4.5 * my_scale];
                let (q, _) = intercept(their_muz, my_c, me.vel, 252.0, 3.0, 0.0, me.grounded);
                let ideal = aim_at(their_muz, q);
                let direct = aim_at(their_muz, my_c);
                let e = wrap(m.aim[0] - ideal[0]).abs().min(wrap(m.aim[0] - direct[0]).abs());
                if d > 15.0 && d < 200.0 && e < 0.35 {
                    aerr = aerr * 0.93 + e.min(0.2) * 0.07;
                }
            }
            self.tracks.insert(
                m.id,
                Track {
                    pos: m.pos,
                    vel: m.vel,
                    aim: m.aim,
                    hp: m.hp.unwrap_or(1400.0),
                    max_hp: m.max_hp.unwrap_or(1400.0),
                    weapons: m.weapons.clone(),
                    charging: m.charging,
                    cubes: m.cubes.unwrap_or(0),
                    seen_t: obs.t,
                    first_t,
                    aerr,
                },
            );
        }
        self.tracks.retain(|_, t| obs.t - t.seen_t < 20.0);
        for p in &obs.pickups {
            self.pickups.insert(p.id, (p.kind.clone(), p.pos, obs.t));
        }
        {
            let seen: Vec<i64> = obs.pickups.iter().map(|p| p.id).collect();
            let world = &self.world;
            self.pickups.retain(|id, (_, pp, _)| {
                if seen.contains(id) {
                    return true;
                }
                // Forget pickups we should be able to see but don't.
                let d = hdist(*pp, pos);
                !(d < 40.0 && world.los(eye, [pp[0], pp[1], pp[2] + 0.5]))
            });
        }
        self.bad_pickups.retain(|_, t| obs.t - *t < 8.0);

        // ---- upgrades
        if let Some(offer) = &obs.offer {
            let mut best = (usize::MAX, 0usize);
            let rank = if self.prm.upg > 1.5 {
                UPGRADE_RANK_MAG
            } else if self.prm.upg > 0.5 {
                UPGRADE_RANK_DPS
            } else {
                UPGRADE_RANK
            };
            for (i, c) in offer.choices.iter().enumerate() {
                let r = rank.iter().position(|u| *u == c.id).unwrap_or(rank.len());
                if r < best.0 {
                    best = (r, i);
                }
            }
            act.pick = Some(best.1);
        }

        // ---- zone
        let (gc, gr, zone_urgent) = self.goal_circle(obs, walk);

        // ---- enemies
        let enemies: Vec<&Mech> = obs.mechs.iter().filter(|m| m.is_mech() && m.id != me.id).collect();
        let attackers: Vec<i64> = self.dmg_log.iter().filter(|d| obs.t - d.0 < 3.0).filter_map(|d| d.2).collect();
        let dmg_by = |id: i64, log: &Vec<(f32, f32, Option<i64>, f32)>| -> f32 {
            log.iter().filter(|d| obs.t - d.0 < 2.0 && d.2 == Some(id)).map(|d| d.3).sum()
        };
        let mut best_t: Option<(f32, &Mech, bool)> = None;
        for m in &enemies {
            let scale = 1.0 + 0.05 * m.cubes.unwrap_or(0) as f32;
            let d = len3(sub(m.pos, pos));
            let shootable = self.world.los(muzzle, [m.pos[0], m.pos[1], m.pos[2] + 4.5 * scale]);
            let hp = m.hp.unwrap_or(1400.0);
            let mut s = d + hp * self.prm.hp_w;
            if shootable {
                s -= 60.0;
            }
            if attackers.contains(&m.id) {
                s -= 50.0;
            }
            // Whoever is hurting us most right now gets our front armour and our guns.
            s -= dmg_by(m.id, &self.dmg_log) * 0.25;
            if Some(m.id) == self.target_id {
                s -= 30.0;
            }
            if self.prm.finish > 0.0 {
                let mh = m.max_hp.unwrap_or(1400.0).max(1.0);
                s -= self.prm.finish * (1.0 - hp / mh).max(0.0) * 100.0;
            }
            if self.prm.weak_first > 0.0 {
                let ae = self.tracks.get(&m.id).map(|t| t.aerr).unwrap_or(0.05);
                s += self.prm.weak_first * (0.04 - ae.min(0.15)) * 800.0;
            }
            if best_t.map(|b| s < b.0).unwrap_or(true) {
                best_t = Some((s, m, shootable));
            }
        }
        let target_mech = best_t.map(|b| (b.1.clone(), b.2));
        self.target_id = target_mech.as_ref().map(|t| t.0.id);

        self.allow_escape = enemies.is_empty() && obs.t - self.last_dmg_t > 3.0;
        if !self.allow_escape {
            self.escape_until = -1.0;
        }
        let threat_eyes: Vec<V3> = enemies
            .iter()
            .filter(|m| hdist(m.pos, pos) < 260.0)
            .map(|m| Self::eye_of(m.pos, 1.0 + 0.05 * m.cubes.unwrap_or(0) as f32))
            .collect();

        // Weapon inventory.
        let slot_kind = |i: usize| me.weapons.get(i).and_then(|w| w.as_ref()).map(|w| w.kind.as_str()).unwrap_or("");
        let has = |k: &str| slot_kind(0) == k || slot_kind(1) == k;
        let have_laser = has("laser");
        let have_light = has("lightning");

        // ---- fight or flight
        let tgt_hp = target_mech.as_ref().map(|t| t.0.hp.unwrap_or(1400.0)).unwrap_or(0.0);
        // Engagement estimate: our time-to-kill on the target vs time-to-die under fire.
        let cube_mul = 1.0 + 0.04 * (me.cubes.min(10) as f32);
        let tgt_d = target_mech.as_ref().map(|t| hdist(t.0.pos, pos)).unwrap_or(999.0);
        let mut our_dps = 0.0;
        if has("gun") {
            our_dps += if tgt_d < 60.0 { 120.0 } else if tgt_d < 130.0 { 80.0 } else { 45.0 };
        }
        if have_laser {
            our_dps += 120.0;
        }
        if have_light && tgt_d < 45.0 {
            our_dps += 190.0;
        }
        if has("mortar") {
            our_dps += 35.0;
        }
        our_dps *= cube_mul;
        let me_c = [pos[0], pos[1], pos[2] + 4.5 * my_scale];
        let mut their_dps = 0.0;
        let mut n_shooting = 0;
        for m in &enemies {
            let d = hdist(m.pos, pos);
            if d > 230.0 {
                continue;
            }
            let sc = 1.0 + 0.05 * m.cubes.unwrap_or(0) as f32;
            if !self.world.los(Self::eye_of(m.pos, sc), me_c) {
                continue;
            }
            n_shooting += 1;
            let mut e = if d < 60.0 { 110.0 } else if d < 130.0 { 80.0 } else { 50.0 };
            if m.has("laser") {
                e += 120.0;
            }
            if m.has("lightning") && d < 50.0 {
                e += 180.0;
            }
            their_dps += e * (1.0 + 0.04 * (m.cubes.unwrap_or(0).min(10) as f32));
        }
        let recent_in: f32 = self.dmg_log.iter().filter(|d| obs.t - d.0 < 2.0).map(|d| d.3).sum::<f32>() / 2.0;
        let incoming = their_dps.max(recent_in);
        let ttk = tgt_hp / our_dps.max(1.0);
        let ttd = me.hp / incoming.max(1.0);
        self.last_ttk = ttk;
        self.last_ttd = ttd;
        if self.retreating {
            let safe = hp_frac > 0.8 || enemies.is_empty() && obs.t - self.last_dmg_t > 10.0;
            let cornered = tgt_d < 30.0 && ttk < ttd * 0.8;
            let winning = ttk < ttd * 0.9 && n_shooting <= 1 || ttk < 2.0 && ttk < ttd;
            if safe || cornered || winning {
                self.retreating = false;
            }
        } else if target_mech.is_some() {
            let losing = ttk > ttd * self.prm.retreat_ratio && ttk > 2.5 && (hp_frac < self.prm.retreat_hp || n_shooting >= 2);
            let low = hp_frac < 0.3 && tgt_hp > me.hp * 0.5;
            if losing || low {
                self.retreating = true;
                self.cover_goal = None;
            }
        }
        // A laser sniper we can't answer: deny it line of sight and make it come to us.
        let sniper = target_mech
            .as_ref()
            .map(|(m, _)| m.has("laser") && !have_laser && hdist(m.pos, pos) > 85.0)
            .unwrap_or(false);
        // Pick fights: engage when it attacks us, when it is close, or when we are healthy
        // and favoured; otherwise slip away and heal.
        let engage_ok = target_mech
            .as_ref()
            .map(|(m, _)| {
                let attacking = attackers.contains(&m.id)
                    || (wrap(m.aim[0] - yaw_to(xy(m.pos), p2)).abs() < 0.25 && hdist(m.pos, pos) < 120.0);
                attacking || tgt_d < 35.0 || hp_frac > 0.65 && ttk < ttd * 0.8 && n_shooting <= 1
            })
            .unwrap_or(false);
        let _avoid = target_mech.is_some() && !engage_ok;
        // Early on, leave alone bots that are not after us: only pick fights we chose well.
        let mut target_mech = target_mech;
        if obs.t < self.prm.pick_until {
            if let Some((m, _)) = &target_mech {
                let attacking = attackers.contains(&m.id) || (wrap(m.aim[0] - yaw_to(xy(m.pos), p2)).abs() < 0.2 && hdist(m.pos, pos) < 90.0);
                let ae = self.tracks.get(&m.id).map(|t| t.aerr).unwrap_or(0.05);
                let hurt = m.hp.unwrap_or(1400.0) < 0.5 * m.max_hp.unwrap_or(1400.0);
                if !(attacking || tgt_d < 30.0 || ae > self.prm.pick_aerr || hurt) {
                    target_mech = None;
                    self.target_id = None;
                }
            }
        }
        // Heal between fights: nobody is shooting us and we are hurt -> hide and regenerate
        // rather than starting the next fight.
        let under_fire = obs.t - self.last_dmg_t < 2.5;
        let target_attacking = target_mech.as_ref().map(|(m, _)| attackers.contains(&m.id)).unwrap_or(false);
        let healing = self.prm.heal > 0.0 && hp_frac < self.prm.heal && !under_fire && !target_attacking && tgt_d > 40.0;

        // ---- choose mode
        let mut mode;
        let mut goal: Option<[f32; 2]> = None;
        let mut mv = [0.0f32, 0.0];
        let mut want_boost = false;
        let mut take: Option<(i64, Option<usize>)> = None;
        let mut aux_target: Option<Target> = None; // non-mech target (npc/crate)

        if zone_urgent {
            mode = Mode::Zone;
            goal = Some(Self::clamp_into(p2, gc, gr * 0.7));
            want_boost = !me.in_zone;
        } else if target_mech.is_some() && (self.retreating || sniper || healing || self.prm.engage_all < 0.5 && _avoid) {
            mode = Mode::Cover;
        } else if target_mech.is_some() {
            mode = Mode::Fight;
        } else {
            mode = Mode::Roam;
        }

        // Loot / farm decisions when not engaged.
        if matches!(mode, Mode::Roam) {
            // Weapon pickups.
            let worst_slot = {
                let k0 = self.prm.prio(slot_kind(0));
                let k1 = self.prm.prio(slot_kind(1));
                if slot_kind(1).is_empty() {
                    (1usize, 0.0)
                } else if slot_kind(0).is_empty() {
                    (0usize, 0.0)
                } else if k0 <= k1 {
                    (0, k0)
                } else {
                    (1, k1)
                }
            };
            let mut best_p: Option<(f32, i64, V3)> = None;
            for (id, (kind, pp, _)) in &self.pickups {
                if self.bad_pickups.contains_key(id) || has(kind) {
                    continue;
                }
                let pr = self.prm.prio(kind);
                // Keep the gun unless we would hold two specials (laser + lightning).
                let gain = pr - worst_slot.1;
                if gain <= 0.0 {
                    continue;
                }
                if worst_slot.1 > 0.0 && slot_kind(worst_slot.0) == "gun" && !(kind == "laser" || kind == "lightning") {
                    continue;
                }
                if dist2(xy(*pp), gc) > gr + 10.0 {
                    continue;
                }
                let d = hdist(*pp, pos);
                let reach = if slot_kind(1).is_empty() || slot_kind(0).is_empty() { 260.0 } else { 150.0 };
                if d > reach {
                    continue;
                }
                let s = d - gain * 25.0;
                if best_p.map(|b| s < b.0).unwrap_or(true) {
                    best_p = Some((s, *id, *pp));
                }
            }
            if let Some((_, id, pp)) = best_p {
                mode = Mode::Loot;
                goal = Some(xy(pp));
                let dz = (pp[2] - pos[2]).abs();
                if hdist(pp, pos) < self.pickup_radius - 0.6 && dz < 3.8 {
                    let drop = if slot_kind(worst_slot.0).is_empty() { None } else { Some(worst_slot.0) };
                    take = Some((id, drop));
                }
            }
            // Cubes.
            if mode == Mode::Roam {
                if let Some(c) = obs
                    .cubes
                    .iter()
                    .filter(|c| hdist(c.pos, pos) < 70.0 && dist2(xy(c.pos), gc) < gr + 5.0)
                    .min_by(|a, b| hdist(a.pos, pos).partial_cmp(&hdist(b.pos, pos)).unwrap())
                {
                    mode = Mode::Loot;
                    goal = Some(xy(c.pos));
                }
            }
            // Supply drops.
            if mode == Mode::Roam && hp_frac > 0.3 && self.prm.drops > 0.5 {
                for d in &obs.drops {
                    let dd = hdist(d.pos, pos);
                    let eta = dd / (walk * 1.2);
                    if dd < 220.0 && d.lands_in < eta + 3.0 && dist2(xy(d.pos), gc) < gr + 5.0 {
                        let contested = self.tracks.values().any(|t| obs.t - t.seen_t < 4.0 && hdist(t.pos, d.pos) < 40.0);
                        if !contested {
                            mode = Mode::Drop;
                            goal = Some(xy(d.pos));
                            break;
                        }
                    }
                }
            }
            // Crates and NPC drones for cubes.
            if mode == Mode::Roam {
                let gun_ok = slot_kind(0) == "gun" || slot_kind(1) == "gun" || have_laser || have_light;
                if gun_ok {
                    if let Some(c) = obs
                        .crates
                        .iter()
                        .filter(|c| hdist(c.pos, pos) < self.prm.farm_range && dist2(xy(c.pos), gc) < gr)
                        .min_by(|a, b| hdist(a.pos, pos).partial_cmp(&hdist(b.pos, pos)).unwrap())
                    {
                        mode = Mode::Farm;
                        aux_target = Some(Target { id: c.id, pos: c.pos, vel: [0.0; 3], kind: TargetKind::Crate, scale: 1.0 });
                        goal = Some(xy(c.pos));
                    } else if hp_frac > self.prm.farm_hp {
                        if let Some(n) = obs
                            .mechs
                            .iter()
                            .filter(|m| m.kind == "npc" && hdist(m.pos, pos) < 80.0 && dist2(xy(m.pos), gc) < gr)
                            .min_by(|a, b| hdist(a.pos, pos).partial_cmp(&hdist(b.pos, pos)).unwrap())
                        {
                            mode = Mode::Farm;
                            aux_target = Some(Target { id: n.id, pos: n.pos, vel: n.vel, kind: TargetKind::Npc, scale: 1.0 });
                            goal = Some(xy(n.pos));
                        }
                    }
                }
            }
        }
        // NPC shooting at us while we do something else: kill it if close.
        if aux_target.is_none() && target_mech.is_none() {
            let npc_attack = self.dmg_log.iter().any(|d| obs.t - d.0 < 2.0 && d.2.map(|b| b >= 100).unwrap_or(false));
            if npc_attack {
                if let Some(n) = obs
                    .mechs
                    .iter()
                    .filter(|m| m.kind == "npc" && hdist(m.pos, pos) < 90.0)
                    .min_by(|a, b| hdist(a.pos, pos).partial_cmp(&hdist(b.pos, pos)).unwrap())
                {
                    aux_target = Some(Target { id: n.id, pos: n.pos, vel: n.vel, kind: TargetKind::Npc, scale: 1.0 });
                }
            }
        }

        // Hold: once hunters are out, stop wandering; wait in a nook and let them come.
        let holding = self.prm.hold > 0.0 && obs.t > self.prm.hold_t;
        if holding {
            self.pick_hold_spot(p2, (gc, gr), obs.t);
        }
        let hold_spot = if holding { self.hold_spot.map(|h| h.0) } else { None };
        if let Some(sp) = hold_spot {
            if matches!(mode, Mode::Roam | Mode::Farm | Mode::Loot | Mode::Drop) {
                let near_goal = goal.map(|g| dist2(g, p2) < 18.0 && dist2(g, sp) < 30.0).unwrap_or(false);
                let need_weapon = slot_kind(0).is_empty() || slot_kind(1).is_empty();
                let keep = mode == Mode::Loot && (near_goal || need_weapon);
                if !keep {
                    if mode == Mode::Farm {
                        aux_target = None;
                    }
                    mode = Mode::Hold;
                    goal = Some(sp);
                }
            }
        }

        // ---- movement per mode
        let tgt: Option<Target> = target_mech
            .as_ref()
            .map(|(m, _)| Target { id: m.id, pos: m.pos, vel: m.vel, kind: TargetKind::Mech, scale: 1.0 + 0.05 * m.cubes.unwrap_or(0) as f32 })
            .or(aux_target);

        if obs.t > self.strafe_switch_t {
            self.strafe_sign = if self.rng.f() < 0.5 { -1.0 } else { 1.0 };
            self.strafe_switch_t = obs.t + self.rng.range(self.prm.st_lo, self.prm.st_hi);
        }

        match mode {
            Mode::Hold => {
                if let Some(g) = goal {
                    mv = if dist2(g, p2) > 2.0 { self.goto(p2, g) } else { [0.0, 0.0] };
                }
            }
            Mode::Zone | Mode::Loot | Mode::Drop => {
                if let Some(g) = goal {
                    mv = self.goto(p2, g);
                    if mode == Mode::Drop && dist2(g, p2) < 3.0 {
                        mv = [0.0, 0.0];
                    }
                }
            }
            Mode::Cover => {
                let away = target_mech.as_ref().map(|t| xy(t.0.pos));
                let hit_now = obs.events.iter().any(|e| e.kind == "damaged" && e.by.map(|b| b < 100).unwrap_or(false));
                let need_new = match self.cover_goal {
                    None => true,
                    Some((c, t0)) => obs.t - t0 > 1.0 || hit_now && dist2(c, p2) < 2.0,
                };
                if need_new {
                    let c = self.find_cover(pos, &threat_eyes, (gc, gr), if self.retreating { away } else { None }, 40.0);
                    self.cover_goal = c.map(|c| (c, obs.t));
                }
                match self.cover_goal {
                    Some((c, _)) => {
                        mv = if dist2(c, p2) > 1.5 { self.goto(p2, c) } else { [0.0, 0.0] };
                        want_boost = dist2(c, p2) > 6.0;
                    }
                    None => {
                        // No cover: fight it out, but back off if retreating.
                        mode = Mode::Fight;
                    }
                }
            }
            _ => {}
        }

        if mode == Mode::Fight {
            if let Some((m, shootable)) = &target_mech {
                let d = hdist(m.pos, pos);
                let (lo, hi) = if have_light {
                    (self.prm.light_lo, self.prm.light_hi)
                } else if have_laser {
                    if m.has("lightning") {
                        (self.prm.laser_lo + 10.0, self.prm.laser_hi + 10.0)
                    } else {
                        (self.prm.laser_lo, self.prm.laser_hi)
                    }
                } else if m.has("lightning") {
                    (self.prm.vs_light_lo, self.prm.vs_light_lo + 40.0)
                } else {
                    (self.prm.gun_lo, self.prm.gun_hi)
                };
                let (lo, hi) = if self.prm.kite_light > 0.0 && m.has("lightning") && !(have_light && tgt_hp < 400.0) {
                    (lo.max(self.prm.kite_light), hi.max(self.prm.kite_light + 40.0))
                } else {
                    (lo, hi)
                };
                // Other known enemies near us (visible now or seen in the last 3 s).
                let others: Vec<[f32; 2]> = self
                    .tracks
                    .iter()
                    .filter(|(id, t)| **id != m.id && obs.t - t.seen_t < 3.0 && hdist(t.pos, pos) < 130.0)
                    .map(|(_, t)| xy(t.pos))
                    .collect();
                let (lo, hi) = if self.prm.multi_far > 0.0 && !others.is_empty() {
                    (lo.max(self.prm.multi_far), hi.max(self.prm.multi_far + 40.0))
                } else {
                    (lo, hi)
                };
                let lsh = self.prm.leash;
                let hold_here = hold_spot.filter(|sp| dist2(*sp, p2) < lsh + 15.0);
                if !shootable && hold_here.is_some() && d > 40.0 {
                    // Ambush: it has to come round the corner to us.
                    let sp = hold_here.unwrap();
                    mv = if dist2(sp, p2) > 2.0 { self.goto(p2, sp) } else { [0.0, 0.0] };
                } else if !shootable {
                    // Go find a line on it (or wait at a corner when it is a sniper).
                    mv = self.goto(p2, xy(m.pos));
                    want_boost = d > 60.0;
                } else {
                    let b = yaw_to(p2, xy(m.pos));
                    let perp = [-b.sin() * self.strafe_sign, b.cos() * self.strafe_sign];
                    let radial = if d > hi {
                        1.0
                    } else if d < lo {
                        -1.0
                    } else {
                        0.0
                    };
                    let to = [b.cos(), b.sin()];
                    let rw = self.prm.radw;
                    let mut cand = unit2([perp[0] + to[0] * radial * rw, perp[1] + to[1] * radial * rw]);
                    if self.prm.kite > 0.0 && !others.is_empty() {
                        let n = others.len() as f32;
                        let c = [others.iter().map(|o| o[0]).sum::<f32>() / n, others.iter().map(|o| o[1]).sum::<f32>() / n];
                        let away = unit2([p2[0] - c[0], p2[1] - c[1]]);
                        cand = unit2([cand[0] + away[0] * self.prm.kite, cand[1] + away[1] * self.prm.kite]);
                    }
                    if let Some(sp) = hold_here {
                        let nxt = [p2[0] + cand[0] * 4.0, p2[1] + cand[1] * 4.0];
                        if dist2(nxt, sp) > lsh && dist2(p2, sp) > lsh * 0.5 {
                            let back = unit2([sp[0] - p2[0], sp[1] - p2[1]]);
                            cand = unit2([cand[0] + back[0] * 1.5, cand[1] + back[1] * 1.5]);
                        }
                    }
                    let ok = |c: [f32; 2], nav: &Nav| nav.walkable_at(p2[0] + c[0] * 5.0, p2[1] + c[1] * 5.0);
                    if !ok(cand, &self.nav) {
                        self.strafe_sign = -self.strafe_sign;
                        let perp = [-perp[0], -perp[1]];
                        cand = unit2([perp[0] + to[0] * radial * rw, perp[1] + to[1] * radial * rw]);
                        if !ok(cand, &self.nav) {
                            cand = if radial != 0.0 { [to[0] * radial, to[1] * radial] } else { [0.0, 0.0] };
                            if !ok(cand, &self.nav) {
                                cand = self.goto(p2, xy(m.pos));
                            }
                        }
                    }
                    mv = cand;
                    want_boost = true;
                }
            }
        }

        // Tactical position: with several enemies around, fight from a spot that sees the
        // target but is hidden from the others.
        if mode == Mode::Fight && self.prm.tactic > 0.5 && threat_eyes.len() >= 2 {
            if let Some((m, _)) = &target_mech {
                let tsc = 1.0 + 0.05 * m.cubes.unwrap_or(0) as f32;
                let tc = [m.pos[0], m.pos[1], m.pos[2] + 4.5 * tsc];
                let teye = Self::eye_of(m.pos, tsc);
                if obs.t > self.tac_t {
                    self.tac_t = obs.t + 0.5;
                    let others: Vec<V3> = threat_eyes
                        .iter()
                        .copied()
                        .filter(|e| ((e[0] - teye[0]).powi(2) + (e[1] - teye[1]).powi(2)).sqrt() > 1.0)
                        .collect();
                    let here_exposed = others.iter().filter(|e| self.world.los(**e, me_c)).count();
                    let mut best: Option<(f32, [f32; 2])> = None;
                    for &r in &[0.0f32, 4.0, 8.0, 13.0, 19.0] {
                        let n = if r == 0.0 { 1 } else { 16 };
                        for k in 0..n {
                            let a = k as f32 * 2.0 * PI / n as f32;
                            let c = [p2[0] + r * a.cos(), p2[1] + r * a.sin()];
                            if r > 0.0 && !self.nav.walkable_at(c[0], c[1]) {
                                continue;
                            }
                            if dist2(c, gc) > gr {
                                continue;
                            }
                            let g = self.world.ground(c[0], c[1]);
                            let muz = [c[0], c[1], g + muzzle_h];
                            if !self.world.los(muz, tc) {
                                continue;
                            }
                            let cc = [c[0], c[1], g + 4.5 * my_scale];
                            let exposed = others.iter().filter(|e| self.world.los(**e, cc)).count();
                            let dt = dist2(c, xy(m.pos));
                            let score = -(exposed as f32) * 100.0 - r * 1.5 - if dt < 20.0 { (20.0 - dt) * 3.0 } else { 0.0 };
                            if best.map(|b| score > b.0).unwrap_or(true) {
                                best = Some((score, c));
                            }
                        }
                    }
                    self.tac_goal = best.filter(|b| dist2(b.1, p2) > 2.0 && b.0 > -(here_exposed as f32) * 100.0 + 20.0).map(|b| b.1);
                }
                if let Some(g) = self.tac_goal {
                    if dist2(g, p2) > 1.5 {
                        mv = self.goto(p2, g);
                    } else {
                        self.tac_goal = None;
                    }
                }
            }
        }

        if mode == Mode::Farm {
            if let Some(a) = aux_target {
                let d = hdist(a.pos, pos);
                let los = self.world.los(muzzle, [a.pos[0], a.pos[1], a.pos[2] + 1.0]);
                if !los || d > 45.0 {
                    mv = self.goto(p2, xy(a.pos));
                } else if matches!(a.kind, TargetKind::Npc) {
                    let b = yaw_to(p2, xy(a.pos));
                    mv = [-b.sin() * self.strafe_sign, b.cos() * self.strafe_sign];
                    if !self.nav.walkable_at(p2[0] + mv[0] * 4.0, p2[1] + mv[1] * 4.0) {
                        mv = [0.0, 0.0];
                    }
                } else if d > 12.0 {
                    // Close in on the crate so its drops are near.
                    mv = self.goto(p2, xy(a.pos));
                    mv = [mv[0] * 0.5, mv[1] * 0.5];
                }
            }
        }

        if mode == Mode::Roam {
            let need = match self.roam_goal {
                None => true,
                Some((g, t0)) => dist2(g, p2) < 6.0 || obs.t - t0 > 30.0 || dist2(g, gc) > gr,
            };
            if need {
                let mut g = gc;
                for _ in 0..20 {
                    let a = self.rng.range(0.0, 2.0 * PI);
                    let r = gr * self.rng.range(0.1, 0.7).sqrt();
                    let c = [gc[0] + r * a.cos(), gc[1] + r * a.sin()];
                    if self.nav.walkable_at(c[0], c[1]) {
                        g = c;
                        break;
                    }
                }
                self.roam_goal = Some((g, obs.t));
            }
            let g = self.roam_goal.unwrap().0;
            mv = self.goto(p2, g);
        }

        // Cube magnet: kills drop cubes (+4% HP and damage each); grab nearby ones even mid-fight.
        if self.prm.magnet > 0.0 && matches!(mode, Mode::Fight | Mode::Farm | Mode::Cover) && !self.retreating {
            if let Some(c) = obs
                .cubes
                .iter()
                .filter(|c| hdist(c.pos, pos) < self.prm.magnet && dist2(xy(c.pos), gc) < gr + 5.0 && (c.pos[2] - pos[2]).abs() < 4.0)
                .min_by(|a, b| hdist(a.pos, pos).partial_cmp(&hdist(b.pos, pos)).unwrap())
            {
                let to = self.goto(p2, xy(c.pos));
                let w = if mode == Mode::Cover { 0.6 } else { 1.4 };
                mv = unit2([mv[0] + to[0] * w, mv[1] + to[1] * w]);
            }
        }

        // Laser dodge: an enemy charging at us with a clear line -> duck into nearby cover.
        if matches!(mode, Mode::Fight | Mode::Farm | Mode::Roam | Mode::Loot | Mode::Hold) {
            let me_c = [pos[0], pos[1], pos[2] + 4.5 * my_scale];
            let charger = enemies
                .iter()
                .filter(|m| m.charging)
                .find(|m| {
                    let b = yaw_to(xy(m.pos), p2);
                    wrap(m.aim[0] - b).abs() < 0.15 && self.world.los(Self::eye_of(m.pos, 1.0), me_c)
                })
                .map(|m| Self::eye_of(m.pos, 1.0 + 0.05 * m.cubes.unwrap_or(0) as f32));
            if let Some(ce) = charger {
                if obs.t > self.dodge_check_t {
                    self.dodge_goal = self.find_cover(pos, &[ce], (gc, gr + 10.0), None, 14.0);
                    self.dodge_check_t = obs.t + 0.3;
                }
                if let Some(c) = self.dodge_goal {
                    if dist2(c, p2) > 1.0 {
                        mv = self.goto(p2, c);
                        want_boost = true;
                        self.dodges += 1;
                    }
                }
            } else {
                self.dodge_goal = None;
            }
        }

        // Projectile dodge: steer away from bullets/shells predicted to hit us.
        if let Some(dv) = self.projectile_dodge(obs, my_scale) {
            let blend = unit2([mv[0] * 0.4 + dv[0], mv[1] * 0.4 + dv[1]]);
            if self.nav.walkable_at(p2[0] + blend[0] * 3.0, p2[1] + blend[1] * 3.0) {
                mv = blend;
                want_boost = true;
            }
        }

        // Keep inside the goal circle while fighting / farming.
        if matches!(mode, Mode::Fight | Mode::Farm) && dist2(p2, gc) > gr {
            let inward = unit2([gc[0] - p2[0], gc[1] - p2[1]]);
            mv = unit2([mv[0] + inward[0] * 1.5, mv[1] + inward[1] * 1.5]);
        }

        // ---- stuck detection
        let moving_wanted = mv[0].abs() + mv[1].abs() > 0.3;
        self.pos_hist.push_back((obs.t, p2, moving_wanted));
        while self.pos_hist.front().map(|f| obs.t - f.0 > 1.6).unwrap_or(false) {
            self.pos_hist.pop_front();
        }
        if obs.t < self.escape_until {
            act.jump = me.grounded;
            want_boost = true;
            self.boosting = true;
        }
        if obs.t < self.unstick_until {
            mv = self.unstick_dir;
            act.jump = me.grounded;
            want_boost = true;
        } else if self.pos_hist.len() >= 14 && self.pos_hist.iter().all(|h| h.2) {
            let f = self.pos_hist.front().unwrap().1;
            if dist2(f, p2) < 1.5 {
                let a = self.rng.range(0.0, 2.0 * PI);
                // Prefer continuing toward the waypoint but sideways.
                self.unstick_dir = unit2([mv[0] + a.cos() * 0.9, mv[1] + a.sin() * 0.9]);
                self.unstick_until = obs.t + 0.8;
                self.path.clear();
                self.pos_hist.clear();
                act.jump = me.grounded;
            }
        }

        // ---- boost with energy hysteresis (keep a reserve for lightning)
        let reserve = if have_light { 45.0 } else { 12.0 };
        if have_light && mode == Mode::Fight && tgt_d < 50.0 && self.prm.fboost < 0.5 {
            want_boost = false;
        }
        if self.boosting {
            if me.energy < reserve {
                self.boosting = false;
            }
        } else if me.energy > reserve + 25.0 {
            self.boosting = true;
        }
        act.boost = want_boost && self.boosting && (mv[0].abs() + mv[1].abs() > 0.2);
        if obs.t < self.escape_until {
            act.boost = me.energy > 1.0;
        }
        if act.jump {
            act.boost = me.energy > 5.0;
        }
        act.mv = mv;

        // ---- aim & fire
        let mut aim: Option<[f32; 2]> = None;
        let mut fire = [false, false];
        if let Some(tg) = tgt {
            let (a, f) = self.weapons_on(obs, tg, muzzle, mode == Mode::Cover);
            aim = Some(a);
            fire = f;
        }
        // Being hurt by someone we cannot see (usually from behind, x1.5): turn to face it.
        if self.prm.turn_unseen > 0.0 && tgt_hp > 300.0 {
            let mut per: Vec<(i64, f32, f32)> = vec![];
            for d in self.dmg_log.iter().filter(|d| obs.t - d.0 < 1.2) {
                if let Some(b) = d.2 {
                    if b >= 100 || enemies.iter().any(|m| m.id == b) {
                        continue;
                    }
                    match per.iter_mut().find(|p| p.0 == b) {
                        Some(p) => {
                            p.1 += d.3;
                            p.2 = d.1;
                        }
                        None => per.push((b, d.3, d.1)),
                    }
                }
            }
            if let Some(w) = per.iter().max_by(|a, b| a.1.partial_cmp(&b.1).unwrap()) {
                if w.1 >= self.prm.turn_unseen {
                    aim = Some([w.2, 0.0]);
                }
            }
        }
        if aim.is_none() {
            // Look toward recent damage from unseen attackers, then sounds, else sweep.
            let recent = self.dmg_log.iter().rev().find(|d| obs.t - d.0 < 2.5).map(|d| d.1);
            let heard = obs
                .sounds
                .iter()
                .filter(|s| matches!(s.kind.as_str(), "gun" | "laser" | "laser_charge" | "lightning" | "step" | "boost" | "jet" | "mortar"))
                .filter(|s| s.dist != "far")
                .map(|s| s.bearing)
                .next();
            let yaw = if let Some(b) = recent.or(heard) {
                b
            } else {
                let base = if mv[0].abs() + mv[1].abs() > 0.1 { mv[1].atan2(mv[0]) } else { me.aim[0] };
                if self.tracks.values().any(|t| obs.t - t.seen_t < 8.0) {
                    base + (obs.t * 1.1).sin() * 1.1
                } else {
                    // Nobody around: spin the torso to find loot and threats all around.
                    me.aim[0] + 1.2
                }
            };
            aim = Some([yaw, 0.0]);
            // Top up the magazine when quiet.
            for (i, w) in me.weapons.iter().enumerate() {
                if let Some(w) = w {
                    if w.kind == "gun" && w.reload.unwrap_or(0.0) <= 0.0 && w.ammo.unwrap_or(12.0) < w.stat("mag", 12.0) * 0.5 {
                        let _ = i;
                        act.reload = true;
                    }
                }
            }
        }
        // Tactical reload: the target has no line on us, so top up the magazine now.
        if self.prm.treload > 0.0 {
            if let Some(tg) = &tgt {
                let c = [tg.pos[0], tg.pos[1], tg.pos[2] + 4.5 * tg.scale];
                if !self.world.los(muzzle, c) {
                    for w in me.weapons.iter().flatten() {
                        if w.kind == "gun" && w.reload.unwrap_or(0.0) <= 0.0 && w.ammo.unwrap_or(12.0) < w.stat("mag", 12.0) * self.prm.treload {
                            act.reload = true;
                        }
                    }
                }
            }
        }
        act.aim = aim;
        act.fire = fire;
        for w in me.weapons.iter().flatten() {
            if w.kind == "gun" && w.ammo.unwrap_or(1.0) <= 0.0 && w.reload.unwrap_or(0.0) <= 0.0 {
                act.reload = true;
            }
        }
        // A charging laser cannot be swapped.
        if let Some((id, drop)) = take {
            let charging = me.weapons.iter().flatten().any(|w| w.kind == "laser" && w.charge.unwrap_or(0.0) > 0.0);
            if !charging {
                act.take = Some(id);
                act.drop = drop;
            }
        }
        self.mode = mode;
        let _ = my_scale;
        if self.debug && self.ticks % self.debug_every == 0 {
            eprintln!(
                "t={:.1} {:?} hp={:.0}/{:.0} e={:.0} pos=({:.0},{:.0},{:.0}) tgt={:?} enemies={} w=[{},{}] cubes={} zone=({:.0},{:.0} r{:.0}) mv=({:.2},{:.2}) fire={:?} retreat={} ttk={:.1} ttd={:.1}",
                obs.t, mode, me.hp, max_hp, me.energy, pos[0], pos[1], pos[2], self.target_id, enemies.len(),
                slot_kind(0), slot_kind(1), me.cubes, gc[0], gc[1], gr, act.mv[0], act.mv[1], act.fire, self.retreating, self.last_ttk, self.last_ttd
            );
        }
        if self.ticks % 5 == 0 {
            act.label = Some(format!("{:?} hp{:.0} t{}", mode, me.hp, self.target_id.unwrap_or(-1)));
        }
        act
    }

    /// Aim and trigger decisions for both slots against a target.
    fn weapons_on(&mut self, obs: &Obs, tg: Target, muzzle: V3, hiding: bool) -> ([f32; 2], [bool; 2]) {
        let me = &obs.me;
        let pos = me.pos;
        let (center_h, radius) = match tg.kind {
            TargetKind::Mech => (4.5 * tg.scale, 2.5 * tg.scale),
            TargetKind::Npc => (1.75, 1.8),
            TargetKind::Crate => (1.2, 1.4),
        };
        let center = [tg.pos[0], tg.pos[1], tg.pos[2] + center_h];
        let d = len3(sub(center, muzzle));
        let grounded = tg.vel[2].abs() < 0.5;
        let los = self.world.los(muzzle, center);
        let mut fire = [false, false];
        let cur_aim = me.aim;

        let gun = me.weapons.iter().position(|w| w.as_ref().map(|w| w.kind == "gun").unwrap_or(false));
        let laser = me.weapons.iter().position(|w| w.as_ref().map(|w| w.kind == "laser").unwrap_or(false));
        let light = me.weapons.iter().position(|w| w.as_ref().map(|w| w.kind == "lightning").unwrap_or(false));
        let mortar = me.weapons.iter().position(|w| w.as_ref().map(|w| w.kind == "mortar").unwrap_or(false));
        let w = |i: usize| me.weapons[i].as_ref().unwrap();

        // Default aim: gun intercept (or straight at the target).
        let gun_aim = gun.map(|g| {
            let sp = w(g).stat("speed", 252.0);
            let (q, _) = intercept(muzzle, center, tg.vel, sp, 3.0, 0.05, grounded);
            aim_at(muzzle, q)
        });
        let direct = aim_at(muzzle, add(center, scale(tg.vel, 0.1)));
        let mut aim = gun_aim.unwrap_or(direct);

        let tol_yaw = (radius * 0.75 / d.max(1.0)).atan() + 0.006;
        let tol_pitch = (center_h * 0.8 / d.max(1.0)).atan() + 0.006;
        let aligned = |a: [f32; 2]| wrap(cur_aim[0] - a[0]).abs() < tol_yaw && (cur_aim[1] - a[1]).abs() < tol_pitch;

        // In cover we hold fire (lets regen start) unless the enemy is on top of us.
        let allow = !hiding || d < 40.0 || obs.t - self.last_dmg_t < 1.0 && los;

        // Laser: charge while we can see it, release when full and on target.
        if let Some(li) = laser {
            let lw = w(li);
            let full = lw.stat("charge", 0.75);
            let range = lw.stat("range", 280.0);
            let charge = lw.charge.unwrap_or(0.0);
            if charge > 0.0 {
                aim = direct;
                let ready = charge >= full - 0.03;
                if ready && los && aligned(direct) {
                    fire[li] = false; // release
                } else if charge > full + 1.6 {
                    fire[li] = false; // about to auto-fire anyway
                } else {
                    fire[li] = true;
                }
            } else if lw.cooldown <= 0.05 && d < range && (los && allow || hiding) && !matches!(tg.kind, TargetKind::Crate) {
                fire[li] = true;
                aim = direct;
            }
        }

        // Gun.
        if let (Some(gi), Some(ga)) = (gun, gun_aim) {
            let gw = w(gi);
            let range = gw.stat("range", 230.0);
            let ammo = gw.ammo.unwrap_or(1.0);
            let reloading = gw.reload.unwrap_or(0.0) > 0.0;
            let ok_aim = if laser.map(|li| fire[li]).unwrap_or(false) { aligned(ga) || aligned(direct) && d < 60.0 } else { aligned(ga) };
            if los && d < range && ammo > 0.0 && !reloading && ok_aim && allow {
                fire[gi] = true;
            }
            if matches!(tg.kind, TargetKind::Mech) {
                let k = if fire[gi] {
                    1
                } else if !los {
                    2
                } else if d >= range {
                    3
                } else if ammo <= 0.0 || reloading {
                    4
                } else if !ok_aim {
                    5
                } else {
                    6
                };
                self.gstat[0] += 1;
                self.gstat[k] += 1;
            }
        }

        // Lightning: auto-targets in its cone.
        if let Some(li) = light {
            let lw = w(li);
            let range = lw.stat("range", 45.0);
            let cone = lw.stat("cone_deg", 30.0).to_radians() * 0.5;
            let cost = lw.stat("energy", 3.0);
            let yaw_err = wrap(cur_aim[0] - yaw_to(xy(pos), xy(tg.pos))).abs();
            if hdist(tg.pos, pos) < range * 0.93 && yaw_err < cone * 0.8 && los && me.energy >= cost + 1.0 && lw.cooldown <= 0.05 && allow {
                fire[li] = true;
            }
            if gun_aim.is_none() && laser.is_none() {
                aim = direct;
            }
        }

        // Mortar: lob when no direct line or when the gun is busy.
        if let Some(mi) = mortar {
            let mw = w(mi);
            let dh = hdist(tg.pos, pos);
            let gun_busy = gun.map(|g| w(g).reload.unwrap_or(0.0) > 0.0 || w(g).ammo.unwrap_or(1.0) <= 0.0).unwrap_or(true);
            let laser_busy = laser.map(|l| w(l).charge.unwrap_or(0.0) > 0.0).unwrap_or(false);
            if mw.cooldown <= 0.05 && dh > 22.0 && dh < 225.0 && (!los || gun_busy) && !laser_busy && allow {
                let sp = mw.stat("speed", 72.0);
                let g = mw.stat("gravity", self.gravity);
                let tube = [pos[0], pos[1], pos[2] + statf(me, "height", 9.0) + 0.5];
                let mut tp = tg.pos;
                let mut sol = None;
                for _ in 0..3 {
                    let dd = hdist(tp, tube) - 3.0;
                    let h = tp[2] + 1.0 - tube[2];
                    let maxp = statf(me, "max_pitch", 1.3) - 0.02;
                    sol = ballistic(dd, h, sp, g, !los).filter(|s| s.0 <= maxp).or_else(|| ballistic(dd, h, sp, g, false));
                    if let Some((_, tf)) = sol {
                        tp = add(tg.pos, scale([tg.vel[0], tg.vel[1], 0.0], tf + 0.05));
                    }
                }
                if let Some((pitch, _)) = sol {
                    let ma = [yaw_to(xy(tube), xy(tp)), pitch];
                    aim = ma;
                    if wrap(cur_aim[0] - ma[0]).abs() < 0.03 && (cur_aim[1] - ma[1]).abs() < 0.03 {
                        fire[mi] = true;
                    }
                }
            }
        }

        (aim, fire)
    }
}
