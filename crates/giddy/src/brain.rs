//! Port of mech/brain.go (decision logic), kept in the same order and with the same names
//! (snake_case) so the two can be diffed side by side.
//!
//! Go keeps `*Track` pointers into its maps; here tracks live in BTreeMaps (ascending-id
//! iteration instead of Go's random map order) and the per-tick `seen` list holds copies
//! taken after the selection loop's writes, which is equivalent because nothing mutates a
//! track between that loop and the end of `step`.
#![allow(dead_code)]
#![allow(clippy::too_many_arguments)]

use std::collections::{BTreeMap, HashMap};
use std::f64::consts::PI;
use std::ops::{Deref, DerefMut};

use crate::gomath::{fmax, fmin, fmod, hypot, Rand};
use crate::types::*;
use crate::world::{World, NBRS};

const LAT: f64 = 0.075;

#[derive(Clone, Debug, Default)]
pub struct Track {
    pub m: MechSeen,
    pub seen: f64,
    pub hit_me: f64,
    pub dist: f64,
    pub los: bool,
    pub closing: f64,
    pub engage: bool,
    /// Which Go map the pointer lives in (b.npcs vs b.tracks): with `id`, the identity Go
    /// compares with `e == tg`.
    pub npc: bool,
}

impl Deref for Track {
    type Target = MechSeen;
    fn deref(&self) -> &MechSeen {
        &self.m
    }
}
impl DerefMut for Track {
    fn deref_mut(&mut self) -> &mut MechSeen {
        &mut self.m
    }
}

impl Track {
    fn same(&self, o: &Track) -> bool {
        self.npc == o.npc && self.m.id == o.m.id
    }
}

#[derive(Clone, Debug, Default)]
struct Known {
    id: i64,
    kind: String,
    pos: Vec3,
    hp: f64,
    seen: f64,
}

#[derive(Clone, Copy, Debug, Default)]
struct Circle {
    x: f64,
    y: f64,
    r: f64,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Params {
    pub retreat_hp: f64,
    pub recover_hp: f64,
    pub gun_range: f64,
    pub light_range: f64,
    pub laser_range: f64,
    pub engage_dist: f64,
    pub farm_npc: bool,
    pub explore: bool,
    pub ghost: bool,
    pub home_expo: f64,
    pub loot_radius: f64,
    pub crate_until: f64,
    pub use_drops: bool,
    pub end_ghost: f64,
    pub dodge: bool,
    pub no_mortar: bool,
    pub peek: bool,
    pub laser_first: bool,
    pub crowd_hide: bool,
    pub vulture: f64,
    pub ambush: f64,
    pub laser_seek: f64,
    pub lethal: bool,
    pub alert: f64,
    pub shy: f64,
    pub early_hide: f64,
    pub evade: bool,
    pub odds: f64,
    pub odds_flee: f64,
    pub practice: bool,
    pub stick: f64,
    pub hpw: f64,
    pub low_bonus: f64,
    pub max_target: f64,
    pub lose_rule: bool,
    pub fight_loot: f64,
    pub farm_hp: f64,
    pub all_foes: bool,
    pub cube_goal: f64,
    pub econ: bool,
    pub laser_up: bool,
    pub econ_safe: f64,
    pub opening: bool,
    pub nano_up: bool,
    pub finish: f64,
    pub finish_hp: f64,
    pub finish_rush: bool,
    pub pursue: f64,
    pub fight_boost: f64,
    pub dodge_boost: f64,
    pub threat_boost: f64,
    pub speed_up: bool,
    pub backstab: f64,
    pub lethal_k: f64,
    pub lose_t: f64,
    pub lose_h2: f64,
    pub lose_h3: f64,
    pub lose_r: f64,
    pub fm_exp: f64,
    pub fm_strafe: f64,
    pub fm_er: f64,
}

/// Baked-in tuning overrides ("k=v,k=v"), applied on top of both the base and the
/// practice-mode parameters. Bools take 1/0.
pub const OVERRIDES: &str = "end_ghost=0,all_foes=1,fight_loot=110,fm_exp=60";
/// Perturbs only the bot's RNG seed, for independent draws of the same strategy.
pub const SALT: u64 = 1;

pub fn apply_overrides(p: &mut Params) {
    for kv in OVERRIDES.split(',') {
        let mut it = kv.splitn(2, '=');
        let (k, v) = (it.next().unwrap_or("").trim(), it.next().and_then(|x| x.trim().parse::<f64>().ok()));
        let Some(x) = v else { continue };
        match k {
            "retreat_hp" => p.retreat_hp = x,
            "recover_hp" => p.recover_hp = x,
            "gun_range" => p.gun_range = x,
            "light_range" => p.light_range = x,
            "laser_range" => p.laser_range = x,
            "engage_dist" => p.engage_dist = x,
            "farm_npc" => p.farm_npc = x > 0.5,
            "explore" => p.explore = x > 0.5,
            "ghost" => p.ghost = x > 0.5,
            "home_expo" => p.home_expo = x,
            "loot_radius" => p.loot_radius = x,
            "crate_until" => p.crate_until = x,
            "use_drops" => p.use_drops = x > 0.5,
            "end_ghost" => p.end_ghost = x,
            "dodge" => p.dodge = x > 0.5,
            "no_mortar" => p.no_mortar = x > 0.5,
            "peek" => p.peek = x > 0.5,
            "laser_first" => p.laser_first = x > 0.5,
            "crowd_hide" => p.crowd_hide = x > 0.5,
            "vulture" => p.vulture = x,
            "ambush" => p.ambush = x,
            "laser_seek" => p.laser_seek = x,
            "lethal" => p.lethal = x > 0.5,
            "alert" => p.alert = x,
            "shy" => p.shy = x,
            "early_hide" => p.early_hide = x,
            "evade" => p.evade = x > 0.5,
            "odds" => p.odds = x,
            "odds_flee" => p.odds_flee = x,
            "practice" => p.practice = x > 0.5,
            "stick" => p.stick = x,
            "hpw" => p.hpw = x,
            "low_bonus" => p.low_bonus = x,
            "max_target" => p.max_target = x,
            "lose_rule" => p.lose_rule = x > 0.5,
            "fight_loot" => p.fight_loot = x,
            "farm_hp" => p.farm_hp = x,
            "all_foes" => p.all_foes = x > 0.5,
            "cube_goal" => p.cube_goal = x,
            "econ" => p.econ = x > 0.5,
            "laser_up" => p.laser_up = x > 0.5,
            "econ_safe" => p.econ_safe = x,
            "opening" => p.opening = x > 0.5,
            "nano_up" => p.nano_up = x > 0.5,
            "finish" => p.finish = x,
            "finish_hp" => p.finish_hp = x,
            "finish_rush" => p.finish_rush = x > 0.5,
            "pursue" => p.pursue = x,
            "fight_boost" => p.fight_boost = x,
            "dodge_boost" => p.dodge_boost = x,
            "threat_boost" => p.threat_boost = x,
            "speed_up" => p.speed_up = x > 0.5,
            "backstab" => p.backstab = x,
            "lethal_k" => p.lethal_k = x,
            "lose_t" => p.lose_t = x,
            "lose_h2" => p.lose_h2 = x,
            "lose_h3" => p.lose_h3 = x,
            "lose_r" => p.lose_r = x,
            "fm_exp" => p.fm_exp = x,
            "fm_strafe" => p.fm_strafe = x,
            "fm_er" => p.fm_er = x,
            _ => {}
        }
    }
}

pub fn default_params() -> Params {
    let mut p = Params {
        retreat_hp: 0.38,
        recover_hp: 0.8,
        gun_range: 85.0,
        light_range: 28.0,
        laser_range: 85.0,
        engage_dist: 60.0,
        farm_npc: true,
        explore: false,
        ghost: true,
        home_expo: 120.0,
        loot_radius: 90.0,
        crate_until: 150.0,
        use_drops: false,
        dodge: true,
        no_mortar: true,
        peek: true,
        laser_first: false,
        crowd_hide: true,
        practice: true,
        end_ghost: 140.0,
        lose_t: 1.5,
        lose_h2: 0.55,
        lose_h3: 0.8,
        lose_r: 160.0,
        fm_exp: 30.0,
        fm_strafe: 9.0,
        fm_er: 170.0,
        ..Default::default()
    };
    // per-slot table from bot.py (unset = 140)
    if let Some(v) = std::env::var("GIDDY_ENDGHOST").ok().and_then(|s| s.trim().parse::<f64>().ok()) {
        p.end_ghost = v;
    }

    p.laser_first = true;
    apply_overrides(&mut p);

    p
}

pub struct Brain {
    pub w: World,
    pub cfg: Config,
    pub slot: i64,
    rng: Rand,
    pub p: Params,

    tracks: BTreeMap<i64, Track>,
    npcs: BTreeMap<i64, Track>,
    crates: BTreeMap<i64, Known>,
    cubes: BTreeMap<i64, Known>,
    pickups: BTreeMap<i64, Known>,
    bad: HashMap<i64, f64>,

    now: f64,

    path: Option<Vec<i32>>,
    path_goal: [f64; 2],
    path_at: f64,
    path_i: usize,

    strafe: f64,
    strafe_until: f64,

    retreat: bool,
    cover_at: f64,
    cover_goal: [f64; 2],
    cover_ok: bool,

    last_hurt: f64,
    hurt_bearing: f64,
    hurt_seen_by: bool,

    loot_id: i64,
    loot_since: f64,

    hold: Circle,
    hold_start: f64,

    home_xy: [f64; 2],
    home_at: f64,
    home_set: bool,
    explore: [f64; 2],
    explore_until: f64,

    stuck_since: f64,
    unstuck_until: f64,
    unstuck_dir: [f64; 2],
    last_threat_at: f64,
    last_aim: [f64; 2],
    laser_held: f64,
    pub mode: String,
    pub panics: i64,

    esc_launch: [f64; 2],
    esc_land: [f64; 2],
    esc_at: f64,
    esc_ok: bool,
    esc_flying: bool,
    odds_flee_until: f64,
    base: Params,
    base_set: bool,
    prac_on: bool,
    end_ghost_on: bool,
    open_on: bool,
    npc_hurt: f64,
    npc_bearing: f64,
    not_practice: bool,
    start_alive: i64,
    kills: i64,
    other_dead: i64,
    stick_id: i64,
    stick_at: f64,
    lose_at: f64,
}

#[inline]
fn hyp(x: f64, y: f64) -> f64 {
    hypot(x, y)
}

fn ang_diff(a: f64, b: f64) -> f64 {
    let mut d = fmod(a - b, 2.0 * PI);
    if d > PI {
        d -= 2.0 * PI;
    } else if d < -PI {
        d += 2.0 * PI;
    }
    d
}

#[inline]
fn clamp(v: f64, lo: f64, hi: f64) -> f64 {
    fmax(lo, fmin(hi, v))
}

fn bad_f(v: f64) -> bool {
    v.is_nan() || v.is_infinite()
}

fn scale_of(cfg: &Config, cubes: i64) -> f64 {
    1.0 + cfg.cubes.size_per_cube * cubes as f64
}

fn center_of(cfg: &Config, t: &Track) -> Vec3 {
    let mut h = cfg.mech.height * scale_of(cfg, t.cubes);
    if t.kind == "npc" {
        h = cfg.npc.height;
    }
    [t.pos[0], t.pos[1], t.pos[2] + h * 0.5]
}

fn eye_of(cfg: &Config, t: &Track) -> Vec3 {
    [t.pos[0], t.pos[1], t.pos[2] + cfg.mech.eye_height * scale_of(cfg, t.cubes)]
}

fn has_weapon<'a>(ws: &'a [Option<WeaponState>], kind: &str) -> (i64, Option<&'a WeaponState>) {
    for (i, w) in ws.iter().enumerate() {
        if let Some(w) = w {
            if w.kind == kind {
                return (i as i64, Some(w));
            }
        }
    }
    (-1, None)
}

fn holds(list: &[String], kind: &str) -> bool {
    list.iter().any(|k| k == kind)
}

const UPGRADE_ORDER: &[&str] = &[
    "plating", "armor", "gun_damage", "nanites", "gun_rate", "lightning_efficiency", "lightning_range", "laser_damage",
    "servos", "laser_charge", "gun_mag", "gun_velocity", "gyros", "lightning_chain", "reactor", "capacitor",
    "mortar_reload", "mortar_splash", "mortar_velocity", "sensors", "jets", "laser_pierce",
];

const UPGRADE_ORDER_LASER: &[&str] = &[
    "plating", "armor", "laser_damage", "laser_charge", "nanites", "gun_damage", "gun_rate", "lightning_efficiency", "lightning_range",
    "servos", "gun_mag", "gun_velocity", "gyros", "lightning_chain", "reactor", "capacitor",
    "mortar_reload", "mortar_splash", "mortar_velocity", "sensors", "jets", "laser_pierce",
];

const UPGRADE_ORDER_NANO: &[&str] = &[
    "plating", "armor", "nanites", "laser_damage", "laser_charge", "gun_damage", "gun_rate", "lightning_efficiency", "lightning_range",
    "servos", "gun_mag", "gun_velocity", "gyros", "lightning_chain", "reactor", "capacitor",
    "mortar_reload", "mortar_splash", "mortar_velocity", "sensors", "jets", "laser_pierce",
];

const UPGRADE_ORDER_SPEED: &[&str] = &[
    "plating", "armor", "servos", "jets", "reactor", "laser_damage", "laser_charge", "nanites", "gun_damage", "gun_rate", "lightning_efficiency", "lightning_range",
    "gun_mag", "gun_velocity", "gyros", "lightning_chain", "capacitor",
    "mortar_reload", "mortar_splash", "mortar_velocity", "sensors", "laser_pierce",
];

fn pick_upgrade(of: &Offer, laser_up: bool, nano_up: bool, speed_up: bool) -> i64 {
    let mut order = UPGRADE_ORDER;
    if laser_up {
        order = UPGRADE_ORDER_LASER;
    }
    if nano_up {
        order = UPGRADE_ORDER_NANO;
    }
    if speed_up {
        order = UPGRADE_ORDER_SPEED;
    }
    let (mut best, mut best_rank) = (0usize, 1usize << 30);
    for (i, c) in of.choices.iter().enumerate() {
        let mut rank = order.len();
        for (r, id) in order.iter().enumerate() {
            if *id == c.id {
                rank = r;
                break;
            }
        }
        if rank < best_rank {
            best = i;
            best_rank = rank;
        }
    }
    best as i64
}

fn weapon_rank(k: &str) -> i64 {
    match k {
        "lightning" => 3,
        "laser" => 2,
        "mortar" => 1,
        _ => 0,
    }
}

#[derive(Clone, Debug)]
struct LootChoice {
    kind: &'static str,
    id: i64,
    pos: Vec3,
    value: f64,
}

/// observe's `upd` closure.
fn upd(tab: &mut BTreeMap<i64, Known>, items: &[Item], w: &World, me_pos: Vec3, eye: Vec3, now: f64) {
    for it in items {
        let k = tab.entry(it.id).or_insert_with(|| Known { id: it.id, ..Default::default() });
        k.kind = it.kind.clone();
        k.pos = it.pos;
        k.hp = it.hp;
        k.seen = now;
    }
    tab.retain(|_, k| {
        if k.seen == now {
            return true;
        }
        let d = hyp(k.pos[0] - me_pos[0], k.pos[1] - me_pos[1]);
        !(d < 40.0 && now - k.seen > 0.35 && w.clear(eye, [k.pos[0], k.pos[1], k.pos[2] + 1.5]))
    });
}

impl Brain {
    pub fn new(h: &Hello, w: World) -> Brain {
        let mut seed = h.seed;
        if seed == 0 {
            seed = h.slot + 1;
        }
        if SALT != 0 {
            seed ^= (SALT as i64).wrapping_mul(0x5DEECE66D) | 1;
        }
        Brain {
            w,
            cfg: h.config.clone(),
            slot: h.slot,
            rng: Rand::new(seed),
            p: default_params(),
            tracks: BTreeMap::new(),
            npcs: BTreeMap::new(),
            crates: BTreeMap::new(),
            cubes: BTreeMap::new(),
            pickups: BTreeMap::new(),
            bad: HashMap::new(),
            now: 0.0,
            path: None,
            path_goal: [0.0; 2],
            path_at: 0.0,
            path_i: 0,
            strafe: 1.0,
            strafe_until: 0.0,
            retreat: false,
            cover_at: 0.0,
            cover_goal: [0.0; 2],
            cover_ok: false,
            last_hurt: -100.0,
            hurt_bearing: 0.0,
            hurt_seen_by: false,
            loot_id: -1,
            loot_since: 0.0,
            hold: Circle::default(),
            hold_start: -1.0,
            home_xy: [0.0; 2],
            home_at: 0.0,
            home_set: false,
            explore: [0.0; 2],
            explore_until: 0.0,
            stuck_since: 0.0,
            unstuck_until: 0.0,
            unstuck_dir: [0.0; 2],
            last_threat_at: -100.0,
            last_aim: [0.0; 2],
            laser_held: 0.0,
            mode: String::new(),
            panics: 0,
            esc_launch: [0.0; 2],
            esc_land: [0.0; 2],
            esc_at: 0.0,
            esc_ok: false,
            esc_flying: false,
            odds_flee_until: 0.0,
            base: Params::default(),
            base_set: false,
            prac_on: false,
            end_ghost_on: false,
            open_on: false,
            npc_hurt: 0.0,
            npc_bearing: 0.0,
            not_practice: false,
            start_alive: 0,
            kills: 0,
            other_dead: 0,
            stick_id: 0,
            stick_at: 0.0,
            lose_at: 0.0,
        }
    }

    /// Go's Act: step + panic recovery + NaN scrubbing.
    pub fn act(&mut self, o: &mut Obs) -> Action {
        let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let mut a = Action { tick: o.tick, ..Default::default() };
            self.step(o, &mut a);
            a
        }));
        let mut act = match r {
            Ok(a) => a,
            Err(_) => {
                self.panics += 1;
                return Action { tick: o.tick, aim: Some(self.last_aim), ..Default::default() };
            }
        };
        if let Some(a) = act.aim {
            if bad_f(a[0]) || bad_f(a[1]) {
                act.aim = None;
            } else {
                self.last_aim = a;
            }
        }
        if bad_f(act.move_[0]) || bad_f(act.move_[1]) {
            act.move_ = [0.0; 2];
        }
        act
    }

    fn scale_of(&self, cubes: i64) -> f64 {
        scale_of(&self.cfg, cubes)
    }

    fn center(&self, t: &Track) -> Vec3 {
        center_of(&self.cfg, t)
    }

    fn eye(&self, t: &Track) -> Vec3 {
        eye_of(&self.cfg, t)
    }

    fn observe(&mut self, o: &mut Obs) {
        let now = o.t;
        if o.self_.stats.radius == 0.0 {
            o.self_.stats = self.cfg.mech.clone();
        }
        let me = &o.self_;
        let eye = [me.pos[0], me.pos[1], me.pos[2] + me.stats.eye_height];
        let muzzle = [me.pos[0], me.pos[1], me.pos[2] + me.stats.muzzle_height];
        for m in &o.mechs {
            let npc = m.kind == "npc";
            let tab = if npc { &mut self.npcs } else { &mut self.tracks };
            let t = tab.entry(m.id).or_insert_with(|| Track { hit_me: -100.0, npc, ..Default::default() });
            t.m = m.clone();
            t.seen = now;
            let (dx, dy) = (m.pos[0] - me.pos[0], m.pos[1] - me.pos[1]);
            t.dist = hyp(dx, dy);
            let c = center_of(&self.cfg, t);
            t.los = self.w.clear(muzzle, c);
            if !t.los && m.kind == "mech" {
                let h = self.cfg.mech.height * scale_of(&self.cfg, m.cubes);
                t.los = self.w.clear(muzzle, [m.pos[0], m.pos[1], m.pos[2] + h * 0.85]);
            }
            if t.dist > 1.0 {
                t.closing = -((m.vel[0] - me.vel[0]) * dx + (m.vel[1] - me.vel[1]) * dy) / t.dist;
            }
        }
        self.tracks.retain(|_, t| !(now - t.seen > 20.0));
        self.npcs.retain(|_, t| !(now - t.seen > 120.0));
        upd(&mut self.crates, &o.crates, &self.w, me.pos, eye, now);
        upd(&mut self.cubes, &o.cubes, &self.w, me.pos, eye, now);
        upd(&mut self.pickups, &o.pickups, &self.w, me.pos, eye, now);
        for e in &o.events {
            match e.kind_.as_str() {
                "damaged" => {
                    if e.weapon == "npc" {
                        if let Some(bearing) = e.bearing {
                            self.npc_hurt = now;
                            self.npc_bearing = bearing;
                        }
                    }
                    if let Some(by) = e.by {
                        self.last_hurt = now;
                        if let Some(bearing) = e.bearing {
                            self.hurt_bearing = bearing;
                        }
                        if let Some(t) = self.tracks.get_mut(&by) {
                            t.hit_me = now;
                        }
                        if let Some(n) = self.npcs.get_mut(&by) {
                            n.hit_me = now;
                        }
                    }
                }
                "kill" => {
                    self.kills += 1;
                    self.tracks.remove(&e.target);
                }
                _ => {}
            }
        }

        if !self.base_set {
            self.base = self.p;
            self.base_set = true;
            self.start_alive = o.alive;
        }
        if self.start_alive - o.alive - self.kills > 0 {
            self.other_dead += 1;
        } else {
            self.other_dead = 0;
        }
        if self.other_dead >= 5 {
            self.not_practice = true;
        }
        let on = self.base.practice && !self.not_practice && now > 20.0;
        if on != self.prac_on {
            self.prac_on = on;
            self.p = self.base;
            if on {
                self.p = squad_params(self.base);
            }
        }
        let open = self.base.opening && !self.not_practice && !self.prac_on && self.start_alive - o.alive - self.kills <= 0;
        if open != self.open_on {
            self.open_on = open;
            self.p = self.base;
            if open {
                self.p.laser_first = true;
                self.p.laser_seek = 240.0;
                self.p.loot_radius = 150.0;
            } else if self.prac_on {
                self.p = squad_params(self.base);
            }
        }
        // End game in practice: when the circle gets small, stop fighting the whole squad and hide
        // (rule from SandeepMadhavarapu/BattleRoyale b532564, MIT).
        if self.prac_on && !self.end_ghost_on && self.base.end_ghost > 0.0 && o.zone.next_radius <= self.base.end_ghost {
            self.end_ghost_on = true;
            self.p = self.base;
            self.p.practice = false;
        }

        let z = &o.zone;
        if self.hold_start != z.shrink_start {
            self.hold_start = z.shrink_start;
            self.hold = Circle { x: z.center[0], y: z.center[1], r: z.radius };
        } else if now <= z.shrink_start {
            self.hold = Circle { x: z.center[0], y: z.center[1], r: z.radius };
        }
    }

    fn zone_at(&self, z: &Zone, t: f64) -> Circle {
        if t <= z.shrink_start || z.shrink_end <= z.shrink_start {
            return self.hold;
        }
        let f = clamp((t - z.shrink_start) / (z.shrink_end - z.shrink_start), 0.0, 1.0);
        Circle {
            x: self.hold.x + (z.next_center[0] - self.hold.x) * f,
            y: self.hold.y + (z.next_center[1] - self.hold.y) * f,
            r: self.hold.r + (z.next_radius - self.hold.r) * f,
        }
    }

    fn cur_zone(&self, z: &Zone) -> Circle {
        let live = Circle { x: z.center[0], y: z.center[1], r: z.radius };
        if self.now <= z.shrink_start {
            return live;
        }
        if (live.r - self.hold.r).abs() > 0.01 || (live.x - self.hold.x).abs() > 0.01 {
            return live;
        }
        self.zone_at(z, self.now)
    }

    fn zone_deadline(&self, z: &Zone, x: f64, y: f64, pad: f64) -> f64 {
        if hyp(x - z.next_center[0], y - z.next_center[1]) <= z.next_radius - pad {
            return f64::INFINITY;
        }
        for k in 0..=24 {
            let t = z.shrink_start + (z.shrink_end - z.shrink_start) * k as f64 / 24.0;
            let c = self.zone_at(z, t);
            if hyp(x - c.x, y - c.y) > c.r - pad {
                return t;
            }
        }
        z.shrink_end
    }

    fn zone_penalty(&self, z: &Zone, x: f64, y: f64) -> f64 {
        let c = self.cur_zone(z);
        let mut p = 0.0;
        let d = hyp(x - c.x, y - c.y) - c.r;
        if d > -5.0 {
            p += 150.0 + 5.0 * (d + 5.0);
        }
        let dn = hyp(x - z.next_center[0], y - z.next_center[1]) - z.next_radius;
        if dn > -5.0 {
            let soon = clamp(1.0 - (z.shrink_start - self.now) / 25.0, 0.1, 1.0);
            p += soon * (25.0 + 0.8 * (dn + 5.0));
        }
        p
    }

    fn zone_goal(&self, z: &Zone, x: f64, y: f64) -> (f64, f64) {
        let margin = clamp(0.3 * z.next_radius, 3.0, 30.0);
        if z.next_radius <= 30.0 {
            return (z.next_center[0], z.next_center[1]);
        }
        let (dx, dy) = (x - z.next_center[0], y - z.next_center[1]);
        let d = hyp(dx, dy);
        if d < 1.0 {
            return (x, y);
        }
        let r = fmax(0.0, z.next_radius - margin - 5.0);
        if d <= r {
            return (x, y);
        }
        (z.next_center[0] + dx / d * r, z.next_center[1] + dy / d * r)
    }

    fn step(&mut self, o: &mut Obs, act: &mut Action) {
        self.now = o.t;
        let now = o.t;
        self.observe(o);
        let o: &Obs = o;
        let me = &o.self_;
        let st = &me.stats;
        let (x, y, zf) = (me.pos[0], me.pos[1], me.pos[2]);
        let z = &o.zone;
        let hp_frac = me.hp / fmax(1.0, me.max_hp);

        if let Some(of) = &o.offer {
            if !of.choices.is_empty() {
                let p = pick_upgrade(of, self.p.laser_up, self.p.nano_up, self.p.speed_up);
                act.pick = Some(p);
            }
        }

        let (_, gun) = has_weapon(&me.weapons, "gun");
        let (_, light) = has_weapon(&me.weapons, "lightning");
        let (_, laser) = has_weapon(&me.weapons, "laser");
        let late = (z.next_radius <= 70.0 && !self.end_ghost_on) || o.alive <= 3;

        let mut seen: Vec<Track> = Vec::new();
        let (mut target, mut hidden, mut shy): (Option<usize>, Option<usize>, Option<usize>) = (None, None, None);
        let mut best_score = f64::INFINITY;
        let (mut max_t, mut hpw) = (215.0, 60.0 / 1400.0);
        if self.p.max_target > 0.0 {
            max_t = self.p.max_target;
        }
        if self.p.hpw > 0.0 {
            hpw = self.p.hpw;
        }
        let ids: Vec<i64> = self.tracks.keys().copied().collect();
        for id in ids {
            let mut t = self.tracks[&id].clone();
            if now - t.seen > 2.5 {
                continue;
            }
            t.engage = false;
            if t.seen != now {
                t.los = false;
                let e = self.tracks.get_mut(&id).unwrap();
                e.engage = false;
                e.los = false;
                seen.push(t);
                continue;
            }
            let idx = seen.len();
            let to_me = (y - t.pos[1]).atan2(x - t.pos[0]);
            let aimed = ang_diff(t.aim[0], to_me).abs() < 0.22;
            if self.p.ghost {
                t.engage = t.dist < self.p.engage_dist
                    || now - t.hit_me < 4.0
                    || (aimed && t.charging && t.dist < 120.0)
                    || (t.hp < 350.0 && t.dist < 120.0)
                    || o.alive <= 2;
                let off = ang_diff(t.aim[0], to_me).abs();

                if self.p.vulture > 0.0 && hp_frac > 0.75 && t.hp < self.p.vulture * t.max_hp && off > 0.6 && t.dist < 165.0 {
                    t.engage = true;
                }
                if self.p.alert > 0.0 && t.los && t.dist < self.p.alert && (t.closing > 3.0 || aimed) {
                    t.engage = true;
                }
                if self.p.shy > 0.0
                    && t.los
                    && t.dist < self.p.shy
                    && t.dist >= self.p.engage_dist
                    && (t.closing > 3.0 || aimed)
                    && !t.engage
                    && (shy.is_none() || t.dist < seen[shy.unwrap()].dist)
                {
                    shy = Some(idx);
                }

                if self.p.ambush > 0.0 && hp_frac > 0.85 && off > 1.75 && t.dist < self.p.ambush {
                    t.engage = true;
                }
                if self.p.odds > 0.0
                    && !t.engage
                    && t.los
                    && hp_frac > 0.5
                    && t.dist < self.reach(me)
                    && self.odds(me, &t, now, true) >= self.p.odds
                {
                    t.engage = true;
                }
            } else if self.p.all_foes && t.dist < max_t {
                t.engage = true;
            } else if t.dist < self.p.engage_dist || now - t.hit_me < 6.0 || late {
                t.engage = true;
            } else if t.closing > 3.0 && t.dist < 180.0 {
                t.engage = true;
            } else if t.hp < 0.4 * t.max_hp && t.dist < 160.0 {
                t.engage = true;
            } else if aimed && t.dist < 190.0 {
                t.engage = true;
            }
            {
                let e = self.tracks.get_mut(&id).unwrap();
                e.engage = t.engage;
                e.los = t.los;
            }
            seen.push(t);
            let t = &seen[idx];
            if t.engage && !t.los && t.dist < max_t && (hidden.is_none() || t.dist < seen[hidden.unwrap()].dist) {
                hidden = Some(idx);
            }
            if !t.engage || !t.los || t.dist > max_t {
                continue;
            }
            let mut s = t.dist + hpw * t.hp;
            if self.p.low_bonus > 0.0 && t.hp <= 0.3 * t.max_hp {
                s -= self.p.low_bonus;
            }
            if self.p.stick > 0.0 && t.id == self.stick_id && now - self.stick_at < 2.0 {
                s -= self.p.stick;
            }
            if self.p.backstab > 0.0 && self.shows_back(t, x, y) {
                s -= self.p.backstab;
            }
            if now - t.hit_me < 4.0 {
                s -= 40.0;
            }
            if t.closing > 3.0 {
                s -= 15.0;
            }
            if s < best_score {
                best_score = s;
                target = Some(idx);
            }
        }
        if let Some(ti) = target {
            self.stick_id = seen[ti].id;
            self.stick_at = now;
        }
        let mut foe = target;
        if foe.is_none() && self.p.peek {
            foe = hidden;
        }
        if foe.is_none() && self.p.pursue > 0.0 {
            if let Some(h) = hidden {
                if seen[h].hp < self.p.pursue * seen[h].max_hp {
                    foe = hidden;
                }
            }
        }
        let mut danger = false;
        let mut crowd = 0;
        for t in &seen {
            if t.engage || now - t.hit_me < 4.0 {
                danger = true;
            }
            if t.seen == now && (t.engage || (t.los && t.dist < 150.0)) {
                crowd += 1;
            }
        }
        if danger || now - self.last_hurt < 3.0 {
            self.last_threat_at = now;
        }

        let tgt: Option<&Track> = target.map(|i| &seen[i]);
        let finishing = tgt.map_or(false, |t| t.dist < 120.0 && (t.hp < 200.0 || t.hp < me.hp * 0.5));
        let (_, my_laser) = has_weapon(&me.weapons, "laser");
        let outgunned = tgt.map_or(false, |t| holds(&t.weapons, "laser") && my_laser.is_none() && t.dist > 55.0 && hp_frac < 0.8);
        let mut swamped = self.p.crowd_hide && (crowd >= 3 || (crowd == 2 && hp_frac < 0.65) || outgunned);
        if self.p.odds_flee > 0.0 {
            if let Some(t) = tgt {
                if !finishing && self.odds(me, t, now, false) < self.p.odds_flee {
                    self.odds_flee_until = now + 5.0;
                }
            }
        }
        if now < self.odds_flee_until && !finishing {
            swamped = true;
        }
        if !self.retreat && ((hp_frac < self.p.retreat_hp && now - self.last_threat_at < 2.0) || swamped) {
            self.retreat = true;
            self.cover_at = -1.0;
        }
        if self.retreat && !swamped && (hp_frac > self.p.recover_hp || (now - self.last_threat_at > 7.0 && hp_frac > 0.55)) {
            self.retreat = false;
        }
        if self.p.lose_rule {
            let mut near = 0;
            for t in &seen {
                if t.seen == now && t.dist < self.p.lose_r {
                    near += 1;
                }
            }
            let mut losing = (near >= 2 && hp_frac < self.p.lose_h2) || (near >= 3 && hp_frac < self.p.lose_h3);
            if let Some(t) = tgt {
                let tf = t.hp / fmax(1.0, t.max_hp);
                if hp_frac < 0.35 && tf > hp_frac + 0.1 {
                    losing = true;
                }
                if tf < 0.25 && hp_frac > 0.2 && near <= 2 {
                    losing = false;
                }
            }
            if losing {
                self.lose_at = now;
            }
            self.retreat = now - self.lose_at < self.p.lose_t && self.lose_at > 0.0;
        }

        let mut exposed = false;
        for t in &seen {
            if t.seen == now && t.los && t.dist < 230.0 {
                exposed = true;
            }
        }
        for t in &seen {
            if t.seen == now && t.dist < 45.0 {
                exposed = false;
            }
        }

        let (mut mx, mut my) = (0.0f64, 0.0f64);
        let mut boost = false;
        let mut mode: String = "idle".into();
        let mut farm: Option<Track> = None;
        let mut farm_pos: Option<Vec3> = None;

        let (zgx, zgy) = self.zone_goal(z, x, y);
        let zdist = hyp(zgx - x, zgy - y);
        let t_need = zdist * 1.45 / st.walk_speed + 4.0;
        let deadline = self.zone_deadline(z, x, y, 8.0);
        let zone_must = !me.in_zone || now + t_need + 3.0 >= deadline;
        let zone_should = now + t_need + 14.0 >= deadline;

        let mut esc_jump = false;
        if self.escape(me, &mut mx, &mut my, &mut boost, &mut esc_jump) {
            mode = "escape".into();
        } else if now < self.unstuck_until {
            mx = self.unstuck_dir[0];
            my = self.unstuck_dir[1];
            mode = "unstuck".into();
        } else if self.dodge_shell(o, &mut mx, &mut my) {
            boost = me.energy > 15.0;
            mode = "dodge".into();
        } else if zone_must && zdist > 2.0 {
            (mx, my) = self.travel(me, zgx, zgy);
            boost = (!me.in_zone || now + t_need >= deadline) && me.energy > 5.0;
            mode = "zone".into();
        } else if self.retreat && !finishing && !late {
            if self.cover_move(o, &seen, &mut mx, &mut my) {
                boost = me.energy > 10.0 && (mx != 0.0 || my != 0.0);
                mode = "cover".into();
            } else if let Some(ti) = target {
                (mx, my) = self.fight_move(o, &seen[ti], &seen, 1.0);
                mode = "cornered".into();
            } else {
                (mx, my) = self.travel(me, zgx, zgy);
                if self.npc_hurt > 0.0 && now - self.npc_hurt < 3.0 && mx == 0.0 && my == 0.0 {
                    let mut side = 1.0;
                    if fmod(now, 3.0) > 1.5 {
                        side = -1.0;
                    }
                    mx = -self.npc_bearing.sin() * side;
                    my = self.npc_bearing.cos() * side;
                }
                mode = "flee".into();
            }
        } else if target.is_none() && now - self.last_hurt < 2.5 && !late && self.p.ghost && self.hide_from_bearing(o, &seen, &mut mx, &mut my) {
            boost = me.energy > 30.0 && (mx != 0.0 || my != 0.0);
            mode = "cover".into();
        } else if now < self.p.early_hide && exposed && !finishing && self.cover_move(o, &seen, &mut mx, &mut my) {
            boost = me.energy > 20.0 && (mx != 0.0 || my != 0.0);
            mode = "earlyhide".into();
        } else if foe.is_none() && shy.is_some() && !late && self.cover_move(o, &seen, &mut mx, &mut my) {
            boost = me.energy > 40.0 && (mx != 0.0 || my != 0.0);
            mode = "shy".into();
        } else if self.p.evade
            && foe.map_or(false, |f| {
                !late && !finishing && seen[f].dist > self.p.engage_dist * 1.2 && seen[f].hp > 0.45 * seen[f].max_hp
            })
            && self.cover_move(o, &seen, &mut mx, &mut my)
        {
            boost = me.energy > 30.0 && (mx != 0.0 || my != 0.0);
            mode = "evade".into();
        } else if let Some(fi) = foe {
            let foe_t = &seen[fi];
            let mut ready = gun.map_or(false, |g| g.ammo > 0.0 && g.reload <= 0.25 && foe_t.dist <= g.stats.range - 10.0);
            let hot = laser.map_or(false, |l| l.charge >= l.stats.charge - 0.3 && foe_t.dist <= l.stats.range - 5.0);
            if hot || light.map_or(false, |l| foe_t.dist <= l.stats.range + 6.0 && me.energy >= l.stats.energy * 3.0) {
                ready = true;
            }
            if !self.p.peek && (light.is_some() || laser.is_some()) {
                ready = true;
            }
            let mut want_los = -1.0;
            if ready {
                want_los = 1.0;
            }
            if self.p.peek && !hot {
                for t in &seen {
                    if t.seen == now && t.charging && ang_diff(t.aim[0], (y - t.pos[1]).atan2(x - t.pos[0])).abs() < 0.3 {
                        want_los = -1.6;
                    }
                }
            }
            let mut fin_hp = 0.38;
            if self.p.finish_hp > 0.0 {
                fin_hp = self.p.finish_hp;
            }
            let keep = self.p;
            if self.p.finish > 0.0 && foe_t.hp < fin_hp * foe_t.max_hp && hp_frac > 0.42 && crowd <= 2 {
                self.p.gun_range = self.p.finish;
                self.p.laser_range = self.p.finish;
                self.p.light_range = self.p.finish;
                (mx, my) = self.fight_move(o, foe_t, &seen, want_los);
                self.p = keep;
                if self.p.finish_rush && foe_t.dist > 2.0 * self.p.finish && me.energy > 30.0 {
                    boost = true;
                }
            } else {
                (mx, my) = self.fight_move(o, foe_t, &seen, want_los);
            }
            if light.is_some() && foe_t.dist > 50.0 && me.energy > 55.0 && want_los > 0.0 {
                boost = true;
            }
            mode = "fight".into();
            if self.p.fight_boost > 0.0
                && me.energy > self.p.fight_boost
                && laser.map_or(true, |l| l.charge <= 0.0)
                && (light.is_none() || me.energy > 60.0)
            {
                boost = true;
            }
            if self.p.fight_loot > 0.0 {
                let mut r = self.p.fight_loot;
                if hp_frac <= 0.5 {
                    r *= 0.57;
                }
                let mut bc: Option<Vec3> = None;
                for c in self.cubes.values() {
                    let d = hyp(c.pos[0] - x, c.pos[1] - y);
                    if d < r {
                        r = d;
                        bc = Some(c.pos);
                    }
                }
                if let Some(bc) = bc {
                    (mx, my) = self.travel(me, bc[0], bc[1]);
                    mode = "fight:cube".into();
                }
            }
        } else if zone_should && zdist > 2.0 {
            (mx, my) = self.travel(me, zgx, zgy);
            mode = "zone".into();
        } else {
            let lc = self.choose_loot(o, hp_frac, danger);
            if let Some(lc) = lc {
                mode = format!("loot:{}", lc.kind);
                match lc.kind {
                    "npc" => {
                        let t = self.npcs.get(&lc.id).cloned();
                        match t {
                            Some(t) if now - t.seen < 1.0 && t.los && t.dist < 75.0 => {
                                (mx, my) = self.fight_move(o, &t, &seen, 1.0);
                                farm = Some(t);
                            }
                            _ => {
                                (mx, my) = self.travel(me, lc.pos[0], lc.pos[1]);
                            }
                        }
                    }
                    "crate" => {
                        let d = hyp(lc.pos[0] - x, lc.pos[1] - y);
                        let cp = [lc.pos[0], lc.pos[1], lc.pos[2] + self.cfg.crates.height * 0.5];
                        if d < 50.0 && self.w.clear([x, y, zf + st.muzzle_height], cp) {
                            farm_pos = Some(cp);
                            if d < 12.0 {
                                mx = (x - lc.pos[0]) / fmax(d, 0.1);
                                my = (y - lc.pos[1]) / fmax(d, 0.1);
                            }
                        } else {
                            (mx, my) = self.travel(me, lc.pos[0], lc.pos[1]);
                        }
                    }
                    _ => {
                        (mx, my) = self.travel(me, lc.pos[0], lc.pos[1]);
                        if lc.kind == "drop" && hyp(lc.pos[0] - x, lc.pos[1] - y) < 2.5 {
                            mx = 0.0;
                            my = 0.0;
                        }
                    }
                }
            } else if zdist > 3.0 && (!self.p.ghost || z.next_radius <= 30.0) {
                (mx, my) = self.travel(me, zgx, zgy);
                mode = "zone-walk".into();
            } else if self.p.ghost {
                let (hx, hy, ok) = self.home(o);
                if ok && hyp(hx - x, hy - y) > 3.0 {
                    (mx, my) = self.travel(me, hx, hy);
                    if !danger && now - self.last_hurt > 6.0 && !self.open_on {
                        let q = fmin(1.0, 4.6 / st.walk_speed);
                        mx *= q;
                        my *= q;
                    }
                    mode = "home".into();
                }
            } else if self.p.explore && !late && me.cubes < self.cube_cap() && hp_frac > 0.7 && !danger {
                (mx, my) = self.explore_move(o);
                mode = "explore".into();
            }
        }

        if esc_jump {
            act.jump = true;
        }
        if self.p.dodge && mode != "unstuck" && mode != "escape" {
            let (dx, dy, lz) = self.dodge(o);
            if dx != 0.0 || dy != 0.0 {
                mx = mx * 0.4 + dx * 0.9;
                my = my * 0.4 + dy * 0.9;
                let l = hyp(mx, my);
                if l > 1.0 {
                    mx /= l;
                    my /= l;
                }
                if lz && me.energy > 25.0 {
                    boost = true;
                }
                if self.p.dodge_boost > 0.0 && me.energy > self.p.dodge_boost {
                    boost = true;
                }
            }
        }
        if self.p.threat_boost > 0.0
            && me.energy > self.p.threat_boost
            && (foe.is_some() || now - self.last_hurt < 2.5)
            && (light.is_none() || me.energy > 60.0)
        {
            boost = true;
        }

        if mode == "escape" {
            self.stuck_since = -1.0;
        } else if (mx != 0.0 || my != 0.0) && hyp(me.vel[0], me.vel[1]) < 1.2 && me.grounded {
            if self.stuck_since < 0.0 {
                self.stuck_since = now;
            } else if now - self.stuck_since > 0.7 {
                let a = self.rng.float64() * 2.0 * PI;
                self.unstuck_dir = [a.cos(), a.sin()];
                self.unstuck_until = now + 0.5;
                self.stuck_since = -1.0;
                self.path = None;
                act.jump = true;
            }
        } else {
            self.stuck_since = -1.0;
        }

        let muzzle = [x + me.vel[0] * LAT, y + me.vel[1] * LAT, zf + st.muzzle_height];
        let mut aim = [me.aim[0], 0.0];
        let mut have_aim = false;
        let (mut tp, mut tv): (Vec3, Vec3) = ([0.0; 3], [0.0; 3]);
        let (mut tr, mut th) = (0.0f64, 0.0f64);
        let mut shootable = false;
        let mut is_mech = false;
        let mut defend = false;
        if target.is_none() && self.npc_hurt > 0.0 && now - self.npc_hurt < 3.0 {
            let mut worst: Option<&Track> = None;
            let mut best_off = 0.7;
            for n in self.npcs.values() {
                if now - n.seen > 0.5 || !n.los || n.dist > 150.0 {
                    continue;
                }
                let mut off = ang_diff((n.pos[1] - y).atan2(n.pos[0] - x), self.npc_bearing).abs();
                if now - n.hit_me < 3.0 {
                    off = 0.0;
                }
                if off < best_off {
                    best_off = off;
                    worst = Some(n);
                }
            }
            if let Some(wt) = worst {
                farm = Some(wt.clone());
                farm_pos = None;
                defend = true;
            }
        }
        if self.p.econ && target.is_none() && farm.is_none() && farm_pos.is_none() && gun.is_some() && !self.retreat {
            let mut reach = 110.0;
            for t in self.tracks.values() {
                if now - t.seen < 4.0 {
                    reach = 50.0;
                }
            }
            for c in self.crates.values() {
                if let Some(&until) = self.bad.get(&c.id) {
                    if until > now {
                        continue;
                    }
                }
                let d = hyp(c.pos[0] - x, c.pos[1] - y);
                let cp = [c.pos[0], c.pos[1], c.pos[2] + self.cfg.crates.height * 0.5];
                if d < reach && d > 6.0 && self.w.clear(muzzle, cp) {
                    reach = d;
                    farm_pos = Some(cp);
                }
            }
        }
        if let Some(t) = tgt {
            let sc = self.scale_of(t.cubes);
            tr = self.cfg.mech.radius * sc;
            th = self.cfg.mech.height * sc;
            tp = t.pos;
            tv = t.vel;
            tp[2] += th * 0.5;
            if !self.w.clear(muzzle, tp) {
                tp[2] = t.pos[2] + th * 0.85;
            }
            shootable = true;
            is_mech = true;
        } else if let Some(f) = &farm {
            tr = self.cfg.npc.radius;
            th = self.cfg.npc.height;
            tp = f.pos;
            tv = f.vel;
            tp[2] += th * 0.5;
            shootable = true;
        } else if let Some(fp) = farm_pos {
            tr = self.cfg.crates.radius;
            th = self.cfg.crates.height;
            tp = fp;
            shootable = true;
        }
        if !is_mech && target.is_none() && (self.retreat || hp_frac < 0.5) && !self.p.econ && !defend {
            shootable = false;
        }

        if shootable {
            let dist = hyp(tp[0] - x, tp[1] - y);
            let grounded = tv[2].abs() < 0.5;

            let (mut gy, mut gp) = (0.0, 0.0);
            let mut gun_ok = false;
            if let Some(g) = gun {
                (gy, gp) = self.lead(muzzle, tp, tv, g.stats.speed, grounded);
                gun_ok = dist <= g.stats.range * 0.96;
            }

            let dp = [tp[0] + tv[0] * (LAT + 0.05), tp[1] + tv[1] * (LAT + 0.05), tp[2]];
            let hy = (dp[1] - muzzle[1]).atan2(dp[0] - muzzle[0]);
            let hp = (dp[2] - muzzle[2]).atan2(hyp(dp[0] - muzzle[0], dp[1] - muzzle[1]));
            aim = [gy, gp];
            if gun.is_none() {
                aim = [hy, hp];
            }
            have_aim = true;
            let tol_y = (tr * 0.75).atan2(fmax(dist, 1.0));
            let tol_p = (th * 0.4).atan2(fmax(dist, 1.0));
            let slew = |want_y: f64, want_p: f64| -> bool {
                let ey = fmax(0.0, ang_diff(want_y, me.aim[0]).abs() - st.turn_rate * 0.05);
                let ep = fmax(0.0, (want_p - me.aim[1]).abs() - st.pitch_rate * 0.05);
                ey < tol_y && ep < tol_p
            };
            for (i, w) in me.weapons.iter().enumerate() {
                let Some(w) = w else { continue };
                if i > 1 {
                    continue;
                }
                match w.kind.as_str() {
                    "gun" => {
                        if gun_ok && w.ammo > 0.0 && w.reload <= 0.0 && slew(gy, gp) {
                            act.fire[i] = true;
                        }
                    }
                    "lightning" => {
                        let cone = w.stats.cone_deg * PI / 180.0;
                        if dist < w.stats.range - 1.5 && ang_diff(hy, me.aim[0]).abs() < cone * 0.5 - 0.05 && me.energy >= w.stats.energy {
                            act.fire[i] = true;
                        }
                    }
                    "laser" => 'sw: {
                        if !is_mech || dist > w.stats.range * 0.97 {
                            break 'sw;
                        }
                        let t = tgt.unwrap();
                        let full = w.stats.charge;
                        if self.p.lethal && w.charge >= w.stats.min_charge && w.charge < full - 0.051 {
                            let mut dmg = w.stats.damage
                                * (w.stats.min_frac + (1.0 - w.stats.min_frac) * w.charge / full)
                                * (1.0 + self.cfg.cubes.damage_per_cube * me.cubes.min(self.cfg.cubes.stat_cap) as f64);
                            if holds(&t.weapons, "mortar") {
                                dmg *= 1.0 + w.stats.cookoff;
                            }
                            let mut lk = 0.8;
                            if self.p.lethal_k > 0.0 {
                                lk = self.p.lethal_k;
                            }
                            if self.p.backstab > 0.0 && self.shows_back(t, x, y) {
                                dmg *= self.rear_mult();
                            }
                            if dmg * lk >= t.hp && ang_diff(hy, me.aim[0]).abs() < tol_y * 0.9 && (hp - me.aim[1]).abs() < tol_p {
                                aim = [hy, hp];
                                act.fire[i] = false;
                                break 'sw;
                            }
                        }
                        if w.charge < full - 0.051 {
                            act.fire[i] = true;
                            if w.charge > full * 0.45 {
                                aim = [hy, hp];
                            }
                        } else {
                            aim = [hy, hp];
                            let ey = ang_diff(hy, me.aim[0]).abs();
                            let ep = (hp - me.aim[1]).abs();
                            if ey < tol_y * 0.9 && ep < tol_p {
                                act.fire[i] = false;
                            } else {
                                act.fire[i] = true;
                            }
                        }
                    }
                    "mortar" => {
                        if !is_mech || w.cooldown > 0.25 || dist < 16.0 || dist > 200.0 {
                            continue;
                        }
                        let tube = [muzzle[0], muzzle[1], zf + st.height + 0.5];
                        let foot = [tp[0], tp[1], tp[2] - th * 0.5];
                        let (my2, mp, ok) = self.mortar(tube, foot, tv, w.stats.speed, w.stats.gravity, st);
                        if ok {
                            if gun.map_or(true, |g| g.reload > 0.0 || g.ammo == 0.0 || !gun_ok) {
                                aim = [my2, mp];
                            }
                            let ey = fmax(0.0, ang_diff(my2, me.aim[0]).abs() - st.turn_rate * 0.05);
                            let ep = fmax(0.0, (mp - me.aim[1]).abs() - st.pitch_rate * 0.05);
                            if ey < 0.04 && ep < 0.03 {
                                act.fire[i] = true;
                            }
                        }
                    }
                    _ => {}
                }
            }
        }

        if target.is_none() {
            if let Some(fi) = foe {
                let foe_t = &seen[fi];
                for (i, w) in me.weapons.iter().enumerate() {
                    if let Some(w) = w {
                        if i < 2
                            && w.kind == "laser"
                            && w.cooldown <= 0.05
                            && foe_t.dist < w.stats.range
                            && w.charge < w.stats.charge + w.stats.hold_s - 0.35
                        {
                            act.fire[i] = true;
                        }
                    }
                }
            }
        }

        if !have_aim {
            let mut watch: Option<&Track> = None;
            for t in &seen {
                if t.engage || now - t.hit_me < 6.0 {
                    if watch.map_or(true, |w| t.dist < w.dist) {
                        watch = Some(t);
                    }
                }
            }
            if let Some(w) = watch {
                aim[0] = (w.pos[1] - y).atan2(w.pos[0] - x);
            } else if self.npc_hurt > 0.0 && now - self.npc_hurt < 2.5 {
                aim[0] = self.npc_bearing;
            } else if now - self.last_hurt < 2.5 {
                aim[0] = self.hurt_bearing;
            } else if mx != 0.0 || my != 0.0 {
                aim[0] = my.atan2(mx) + 1.15 * (now * 1.3).sin();
            } else {
                aim[0] = me.aim[0] + 0.35;
            }
            aim[1] = 0.0;
        }
        aim[1] = clamp(aim[1], st.min_pitch, st.max_pitch);
        act.aim = Some(aim);

        if let Some(g) = gun {
            if g.reload <= 0.0 && g.ammo < g.stats.mag {
                if target.is_none() && farm.is_none() && farm_pos.is_none() && (g.ammo <= g.stats.mag * 0.6 || now - self.last_threat_at > 3.0) {
                    act.reload = true;
                }
            }
        }

        let mut slot1 = "";
        if me.weapons.len() > 1 {
            if let Some(w1) = &me.weapons[1] {
                slot1 = w1.kind.as_str();
            }
        }
        for p in &o.pickups {
            if hyp(p.pos[0] - x, p.pos[1] - y) > self.cfg.loot.pickup_radius - 0.3 || (p.pos[2] - zf).abs() > 3.6 {
                continue;
            }
            if !self.want_pickup(me, &p.kind) {
                continue;
            }
            act.take = Some(p.id);
            if !slot1.is_empty() && !me.weapons.is_empty() && me.weapons[0].is_some() {
                if slot1 == "laser" && me.weapons[1].as_ref().unwrap().charge > 0.0 {
                    act.take = None;
                    break;
                }
                act.drop = Some(1);
            }
            break;
        }

        act.move_ = [mx, my];
        act.boost = boost && (mx != 0.0 || my != 0.0);
        self.mode = mode;
    }

    fn escape(&mut self, me: &SelfObs, mx: &mut f64, my: &mut f64, boost: &mut bool, jump: &mut bool) -> bool {
        let (x, y, zf) = (me.pos[0], me.pos[1], me.pos[2]);
        if me.grounded {
            if zf - self.w.ground_at(x, y) > 3.0 && self.w.obst[self.w.vidx(x, y)] {
                self.esc_ok = false;
                self.esc_flying = false;
                return false;
            }
            if !self.w.off_main(x, y) {
                self.esc_ok = false;
                self.esc_flying = false;
                return false;
            }
            self.esc_flying = false;
            if !self.esc_ok || self.now - self.esc_at > 6.0 {
                let (l, d, ok) = self.w.escape_plan(x, y);
                self.esc_launch = l;
                self.esc_land = d;
                self.esc_ok = ok;
                self.esc_at = self.now;
                if !self.esc_ok {
                    return false;
                }
            }
            if hyp(self.esc_launch[0] - x, self.esc_launch[1] - y) > 2.5 {
                let (lx, ly) = (self.esc_launch[0], self.esc_launch[1]);
                (*mx, *my) = self.travel(me, lx, ly);
                return true;
            }
            if me.energy < fmin(70.0, me.max_energy * 0.7) {
                *mx = 0.0;
                *my = 0.0;
                return true;
            }
            self.esc_flying = true;
            *jump = true;
        } else if !self.esc_flying {
            return false;
        }
        let (dx, dy) = (self.esc_land[0] - x, self.esc_land[1] - y);
        let d = hyp(dx, dy);
        if d < 1.5 {
            return true;
        }
        *mx = dx / d;
        *my = dy / d;
        let mut need = -1e9;
        let mut t = 0.0;
        while t <= fmin(d, 30.0) {
            need = fmax(need, self.w.top_at(x + *mx * t, y + *my * t));
            t += 1.0;
        }
        *boost = zf < need + 2.5 && me.energy > 1.0;
        true
    }

    fn want_pickup(&self, me: &SelfObs, kind: &str) -> bool {
        if kind == "gun" {
            return !me.weapons.is_empty() && me.weapons[0].is_none();
        }
        let (i, _) = has_weapon(&me.weapons, kind);
        if i >= 0 {
            return false;
        }
        if kind == "mortar" && self.p.no_mortar {
            return false;
        }
        let rank = |k: &str| -> i64 {
            if self.p.laser_first && k == "laser" {
                return 4;
            }
            weapon_rank(k)
        };
        let mut cur = 0;
        if me.weapons.len() > 1 {
            if let Some(w1) = &me.weapons[1] {
                cur = rank(&w1.kind);
            }
        }
        rank(kind) > cur
    }

    fn lead(&self, muzzle: Vec3, tp: Vec3, tv: Vec3, mut speed: f64, grounded: bool) -> (f64, f64) {
        if speed <= 0.0 {
            speed = 252.0;
        }
        let mut t = 0.0;
        let (mut px, mut py, mut pz) = (0.0, 0.0, 0.0);
        for _k in 0..4 {
            let tt = LAT + t;
            px = tp[0] + tv[0] * tt;
            py = tp[1] + tv[1] * tt;
            pz = tp[2];
            if !grounded {
                pz += tv[2] * tt * 0.5;
            }
            let d = ((px - muzzle[0]) * (px - muzzle[0]) + (py - muzzle[1]) * (py - muzzle[1]) + (pz - muzzle[2]) * (pz - muzzle[2])).sqrt();
            t = fmax(0.0, d - 3.0) / speed;
        }
        ((py - muzzle[1]).atan2(px - muzzle[0]), (pz - muzzle[2]).atan2(hyp(px - muzzle[0], py - muzzle[1])))
    }

    fn mortar(&self, tube: Vec3, foot: Vec3, tv: Vec3, v: f64, g: f64, st: &MechStats) -> (f64, f64, bool) {
        if v <= 0.0 || g <= 0.0 {
            return (0.0, 0.0, false);
        }
        let (mut yaw, mut pitch) = (0.0f64, 0.0f64);
        let mut t = 0.0;
        for _k in 0..3 {
            let (px, py) = (foot[0] + tv[0] * (LAT + t), foot[1] + tv[1] * (LAT + t));
            let dx = hyp(px - tube[0], py - tube[1]);
            let dz = foot[2] - tube[2];
            let disc = v * v * v * v - g * (g * dx * dx + 2.0 * dz * v * v);
            if disc < 0.0 || dx < 1.0 {
                return (0.0, 0.0, false);
            }
            pitch = ((v * v - disc.sqrt()) / (g * dx)).atan();
            yaw = (py - tube[1]).atan2(px - tube[0]);
            t = dx / (v * pitch.cos());
        }
        if pitch < st.min_pitch || pitch > st.max_pitch {
            return (0.0, 0.0, false);
        }

        let (c, s) = (pitch.cos(), pitch.sin());
        let mut tt = 0.1;
        while tt < t * 0.9 {
            let d = v * c * tt;
            let (px, py) = (tube[0] + yaw.cos() * d, tube[1] + yaw.sin() * d);
            let pz = tube[2] + v * s * tt - 0.5 * g * tt * tt;
            if self.w.solid(px, py, pz) {
                return (0.0, 0.0, false);
            }
            tt += 0.1;
        }
        (yaw, pitch, true)
    }

    fn dodge_shell(&self, o: &Obs, mx: &mut f64, my: &mut f64) -> bool {
        let me = &o.self_;
        let ms = self.cfg.weapons.get("mortar").cloned().unwrap_or_default();
        let mut g = ms.gravity;
        if g <= 0.0 {
            g = 22.0;
        }
        let rad = ms.splash_radius + me.stats.radius + 2.0;
        for p in &o.projectiles {
            if p.kind != "mortar" {
                continue;
            }
            let (mut px, mut py, mut pz) = (p.pos[0], p.pos[1], p.pos[2]);
            let mut vz = p.vel[2];
            let mut t = 0.0;
            while t < 4.0 {
                px += p.vel[0] * 0.05;
                py += p.vel[1] * 0.05;
                pz += vz * 0.05;
                vz -= g * 0.05;
                if self.w.solid(px, py, pz) {
                    let mut d = hyp(px - me.pos[0], py - me.pos[1]);
                    if d < rad && t < 2.5 {
                        let (mut dx, mut dy) = (me.pos[0] - px, me.pos[1] - py);
                        if d < 0.5 {
                            dx = -p.vel[1];
                            dy = p.vel[0];
                            d = hyp(dx, dy);
                        }
                        dx /= d;
                        dy /= d;

                        for rot in [0.0f64, 0.7, -0.7, 1.4, -1.4] {
                            let (c, s) = (rot.cos(), rot.sin());
                            let (ex, ey) = (dx * c - dy * s, dx * s + dy * c);
                            if self.w.line_walkable(me.pos[0], me.pos[1], me.pos[0] + ex * 6.0, me.pos[1] + ey * 6.0) {
                                *mx = ex;
                                *my = ey;
                                return true;
                            }
                        }
                    }
                    break;
                }
                t += 0.05;
            }
        }
        false
    }

    pub fn travel(&mut self, me: &SelfObs, gx: f64, gy: f64) -> (f64, f64) {
        let (x, y) = (me.pos[0], me.pos[1]);
        let d = hyp(gx - x, gy - y);
        if d < 0.8 {
            return (0.0, 0.0);
        }
        let unit = |tx: f64, ty: f64| -> (f64, f64) {
            let l = hyp(tx - x, ty - y);
            if l < 1e-6 {
                return (0.0, 0.0);
            }
            ((tx - x) / l, (ty - y) / l)
        };

        if me.pos[2] - self.w.ground_at(x, y) > 3.0 && self.w.obst[self.w.vidx(x, y)] {
            return unit(gx, gy);
        }
        if d < 45.0 && self.w.line_walkable(x, y, gx, gy) {
            return unit(gx, gy);
        }
        if self.path.is_none() || hyp(gx - self.path_goal[0], gy - self.path_goal[1]) > 5.0 || self.now - self.path_at > 2.5 {
            self.path = self.w.path(x, y, gx, gy, 22000).0;
            self.path_goal = [gx, gy];
            self.path_at = self.now;
            self.path_i = 0;
        }
        let plen = self.path.as_ref().map_or(0, |p| p.len());
        if plen == 0 {
            return unit(gx, gy);
        }
        let path = self.path.as_ref().unwrap();

        let (mut bi, mut bd) = (self.path_i, f64::INFINITY);
        let mut i = self.path_i;
        while i < path.len() && i < self.path_i + 10 {
            let (px, py) = self.w.cell_pos(path[i] as i64);
            let dd = hyp(px - x, py - y);
            if dd < bd {
                bi = i;
                bd = dd;
            }
            i += 1;
        }
        self.path_i = bi;
        if bd > 12.0 {
            self.path = None;
            return unit(gx, gy);
        }
        let path = self.path.as_ref().unwrap();
        let mut i = (bi + 14).min(path.len() - 1);
        while i > bi {
            let (px, py) = self.w.cell_pos(path[i] as i64);
            if self.w.line_walkable(x, y, px, py) {
                if i == path.len() - 1 && hyp(px - gx, py - gy) < 4.0 && self.w.line_walkable(x, y, gx, gy) {
                    return unit(gx, gy);
                }
                return unit(px, py);
            }
            i -= 1;
        }
        if bi + 1 < path.len() {
            let (px, py) = self.w.cell_pos(path[bi + 1] as i64);
            return unit(px, py);
        }
        unit(gx, gy)
    }

    fn pref_range(&self, me: &SelfObs, t: &Track) -> f64 {
        let (_, light) = has_weapon(&me.weapons, "lightning");
        let (_, laser) = has_weapon(&me.weapons, "laser");
        let mut r = self.p.gun_range;
        if light.is_some() && me.energy > 12.0 {
            r = self.p.light_range;
        } else if laser.is_some() {
            r = self.p.laser_range;
        }
        if t.kind == "npc" {
            return 58.0;
        }
        if light.is_none() {
            if holds(&t.weapons, "lightning") {
                r = fmax(r, 64.0);
            }
            if holds(&t.weapons, "mortar") {
                r = fmax(r, 80.0);
            }
        }
        r
    }

    fn fight_move(&mut self, o: &Obs, tg: &Track, seen: &[Track], want_los: f64) -> (f64, f64) {
        let me = &o.self_;
        let (x, y) = (me.pos[0], me.pos[1]);
        let rp = self.pref_range(me, tg);
        let (bx, by) = (tg.pos[0] - x, tg.pos[1] - y);
        let bd = hyp(bx, by) + 1e-9;
        let (ux, uy) = (bx / bd, by / bd);
        if self.now >= self.strafe_until {
            if self.rng.float64() < 0.6 {
                self.strafe = -self.strafe;
            }
            self.strafe_until = self.now + 0.3 + self.rng.float64() * 0.7;
        }
        let (sx, sy) = (-uy * self.strafe, ux * self.strafe);
        let tc = self.center(tg);
        let mh = me.stats.muzzle_height;
        let mut cube: Option<&Known> = None;
        for c in self.cubes.values() {
            let d = hyp(c.pos[0] - x, c.pos[1] - y);
            if d < 40.0 && cube.map_or(true, |cb| d < hyp(cb.pos[0] - x, cb.pos[1] - y)) && (c.pos[2] - me.pos[2]).abs() < 4.0 {
                cube = Some(c);
            }
        }
        let mut best = f64::NEG_INFINITY;
        let (mut mx, mut my) = (0.0, 0.0);
        let mut blocked_side = false;
        let mut rings = 1;
        if self.p.peek {
            rings = 2;
        }
        let w = &self.w;
        for k in -1..16 * rings {
            let (mut cx, mut cy, mut dx, mut dy, mut r) = (x, y, 0.0, 0.0, 0.0);
            if k >= 0 {
                let a = (k % 16) as f64 * PI / 8.0;
                r = 6.0 * (1 + k / 16) as f64;
                dx = a.cos();
                dy = a.sin();
                cx = x + dx * r;
                cy = y + dy * r;
                if !w.line_walkable(x, y, cx, cy) || !w.free(cx, cy) {
                    if k < 16 && dx * sx + dy * sy > 0.9 {
                        blocked_side = true;
                    }
                    continue;
                }
            }
            let cz = w.walk_at(cx, cy);
            let d = hyp(tg.pos[0] - cx, tg.pos[1] - cy);
            let mut s = -1.1 * (d - rp).abs() - r * 0.4;
            let los = w.clear([cx, cy, cz + mh], tc);
            if los {
                s += 45.0 * want_los;
            }
            if self.p.peek && k >= 0 && k < 16 && (los == (want_los > 0.0)) {
                let mut q = 0;
                while q < 16 {
                    let (qx, qy) = (cx + (q as f64 * PI / 8.0).cos() * 7.0, cy + (q as f64 * PI / 8.0).sin() * 7.0);
                    if w.free(qx, qy) && w.clear([qx, qy, w.walk_at(qx, qy) + mh], tc) != los {
                        s += 8.0;
                        break;
                    }
                    q += 2;
                }
            }
            let body = [cx, cy, cz + me.stats.height * 0.6];
            for e in seen {
                if e.same(tg) || hyp(e.pos[0] - cx, e.pos[1] - cy) > self.p.fm_er {
                    continue;
                }
                if w.clear(self.eye(e), body) {
                    s -= self.p.fm_exp;
                }
            }
            s -= self.zone_penalty(&o.zone, cx, cy);
            if let Some(cb) = cube {
                s += 1.5 * (hyp(cb.pos[0] - x, cb.pos[1] - y) - hyp(cb.pos[0] - cx, cb.pos[1] - cy));
            }
            let cl = w.clearance[w.cell(cx, cy) as usize] as f64;
            if cl < 4.0 {
                s -= (4.0 - cl) * 2.5;
            }
            if want_los > 0.0 {
                s += self.p.fm_strafe * (dx * sx + dy * sy);
            }
            if k < 0 {
                s -= 8.0;
                if want_los < 0.0 && !los {
                    s += 14.0;
                }
            }
            if s > best {
                best = s;
                mx = dx;
                my = dy;
            }
        }
        if blocked_side {
            self.strafe = -self.strafe;
            self.strafe_until = self.now + 0.5;
        }
        (mx, my)
    }

    fn cover_hidden(&self, me: &SelfObs, threats: &[&Track], cx: f64, cy: f64) -> bool {
        let cz = self.w.walk_at(cx, cy);
        for t in threats {
            let e = self.eye(t);
            if self.w.clear(e, [cx, cy, cz + me.stats.height]) || self.w.clear(e, [cx, cy, cz + me.stats.height * 0.5]) {
                return false;
            }
        }
        true
    }

    fn cover_move(&mut self, o: &Obs, seen: &[Track], mx: &mut f64, my: &mut f64) -> bool {
        let me = &o.self_;
        let (x, y) = (me.pos[0], me.pos[1]);
        let mut threats: Vec<&Track> = Vec::new();
        for t in seen {
            if hyp(t.pos[0] - x, t.pos[1] - y) < 230.0 {
                threats.push(t);
            }
        }
        if threats.is_empty() {
            return false;
        }
        if threats.len() > 4 {
            threats.sort_by(|a, b| {
                let (da, db) = (hyp(a.pos[0] - x, a.pos[1] - y), hyp(b.pos[0] - x, b.pos[1] - y));
                da.partial_cmp(&db).unwrap_or(std::cmp::Ordering::Equal)
            });
            threats.truncate(4);
        }
        if self.cover_ok && self.now - self.cover_at < 0.4 {
            (*mx, *my) = self.travel_cover(me);
            return true;
        }
        self.cover_at = self.now;
        if self.cover_hidden(me, &threats, x, y) && self.zone_penalty(&o.zone, x, y) == 0.0 {
            self.cover_ok = true;
            self.cover_goal = [x, y];
            *mx = 0.0;
            *my = 0.0;
            return true;
        }

        let s = self.w.nearest_free(self.w.cell(x, y));
        if s < 0 {
            return false;
        }
        self.w.gen = self.w.gen.wrapping_add(1);
        let gen = self.w.gen;
        let mut queue: Vec<i32> = vec![s as i32];
        self.w.stamp[s as usize] = gen;
        self.w.g[s as usize] = 0.0;
        let (mut best_c, mut best_s): (i64, f64) = (-1, f64::INFINITY);
        let mut checked = 0;
        let (nnx, nny) = (self.w.nnx, self.w.nny);
        let mut h = 0;
        while h < queue.len() && checked < 350 {
            let c = queue[h] as i64;
            h += 1;
            let (ci, cj) = (c % nnx, c / nnx);
            let steps = self.w.g[c as usize] as f64;
            if steps > 50.0 || (best_c >= 0 && steps * 2.0 > best_s + 30.0) {
                break;
            }
            if (ci + cj) % 2 == 0 && steps > 0.0 {
                let (cx, cy) = self.w.cell_pos(c);
                checked += 1;
                if self.cover_hidden(me, &threats, cx, cy) {
                    let mut sc = steps * 2.0 + self.zone_penalty(&o.zone, cx, cy);
                    let mut near = f64::INFINITY;
                    for t in &threats {
                        near = fmin(near, hyp(t.pos[0] - cx, t.pos[1] - cy));
                    }
                    sc -= fmin(near, 80.0) * 0.25;
                    if sc < best_s {
                        best_c = c;
                        best_s = sc;
                    }
                }
            }
            let w = &mut self.w;
            for (k, o2) in NBRS.iter().enumerate() {
                let (i, j) = (ci + o2[0], cj + o2[1]);
                if i < 0 || j < 0 || i >= nnx || j >= nny {
                    continue;
                }
                let nb = j * nnx + i;
                if w.stamp[nb as usize] == gen || !w.can_step(c, nb, k >= 4) {
                    continue;
                }
                if k >= 4 && (w.blocked[(cj * nnx + i) as usize] || w.blocked[(j * nnx + ci) as usize]) {
                    continue;
                }
                w.stamp[nb as usize] = gen;
                w.g[nb as usize] = w.g[c as usize] + 1.0;
                queue.push(nb as i32);
            }
        }
        if best_c < 0 {
            self.cover_ok = false;
            return false;
        }
        let (gx, gy) = self.w.cell_pos(best_c);
        self.cover_ok = true;
        self.cover_goal = [gx, gy];
        (*mx, *my) = self.travel_cover(me);
        true
    }

    fn travel_cover(&mut self, me: &SelfObs) -> (f64, f64) {
        if hyp(self.cover_goal[0] - me.pos[0], self.cover_goal[1] - me.pos[1]) < 1.5 {
            return (0.0, 0.0);
        }
        let (gx, gy) = (self.cover_goal[0], self.cover_goal[1]);
        self.travel(me, gx, gy)
    }

    fn choose_loot(&mut self, o: &Obs, hp_frac: f64, danger: bool) -> Option<LootChoice> {
        let me = &o.self_;
        let (x, y) = (me.pos[0], me.pos[1]);
        let z = &o.zone;
        let now = self.now;
        let cur = self.cur_zone(z);
        let econ = self.p.econ;
        // econ's wrapper around `consider` (nearOnly) is folded into the closure as a flag.
        let mut near_only = false;
        let mut mech_near = false;
        if econ {
            for t in self.tracks.values() {
                if t.seen == now {
                    mech_near = true;
                }
            }
            for t in self.tracks.values() {
                if self.p.econ_safe > 0.0 && now - t.seen < self.p.econ_safe {
                    near_only = true;
                }
            }
        }
        let mut best: Option<LootChoice> = None;
        {
            let this = &*self;
            let mut best_u = 0.0;
            let mut consider = |kind: &'static str, id: i64, pos: Vec3, value: f64| {
                if near_only && hyp(pos[0] - x, pos[1] - y) > 60.0 {
                    return;
                }
                if let Some(&until) = this.bad.get(&id) {
                    if until > now {
                        return;
                    }
                }
                let d = hyp(pos[0] - x, pos[1] - y);
                if d > 260.0 {
                    return;
                }
                if hyp(pos[0] - cur.x, pos[1] - cur.y) > cur.r - 6.0 {
                    return;
                }

                let dl = this.zone_deadline(z, pos[0], pos[1], 6.0);
                if now + d * 1.4 / me.stats.walk_speed + 5.0 > dl {
                    return;
                }

                if pos[2] - this.w.ground_at(pos[0], pos[1]) > 3.2 {
                    return;
                }
                let nf = this.w.nearest_free(this.w.cell(pos[0], pos[1]));
                if nf < 0 || this.w.comp[nf as usize] != this.w.main_comp {
                    return;
                }
                let mut u = value / (d + 18.0);
                if id == this.loot_id {
                    u *= 1.35;
                }
                if econ {
                    u = value - 0.6 * d;
                    if id == this.loot_id {
                        u += 30.0;
                    }
                }
                if u > best_u {
                    best_u = u;
                    best = Some(LootChoice { kind, id, pos, value });
                }
            };
            if econ {
                let hurt = hp_frac <= 0.45;
                for p in this.pickups.values() {
                    if !this.want_pickup(me, &p.kind) || hurt {
                        continue;
                    }
                    let mut v = 150.0;
                    if me.weapons.len() > 1 && me.weapons[1].is_none() {
                        v = 260.0;
                    }
                    if p.kind == "laser" {
                        v = fmax(v, 270.0) + 20.0;
                    }
                    consider("pickup", p.id, p.pos, v);
                }
                for c in this.cubes.values() {
                    if !hurt || hyp(c.pos[0] - x, c.pos[1] - y) < 35.0 {
                        consider("cube", c.id, c.pos, 110.0);
                    }
                }
                for c in this.crates.values() {
                    if !hurt || hyp(c.pos[0] - x, c.pos[1] - y) < 35.0 {
                        consider("crate", c.id, c.pos, 105.0);
                    }
                }
                if this.p.farm_npc && hp_frac > 0.6 && !mech_near {
                    for n in this.npcs.values() {
                        let mut v = 170.0;
                        if now - n.seen > 12.0 {
                            v = 155.0;
                        }
                        consider("npc", n.id, n.pos, v);
                    }
                }
                for d in &o.drops {
                    if this.p.use_drops && !hurt && hyp(d.pos[0] - x, d.pos[1] - y) / me.stats.walk_speed < d.lands_in + 25.0 {
                        consider("drop", d.id, d.pos, 170.0);
                    }
                }
            } else {
                for p in this.pickups.values() {
                    let mut rad = this.p.loot_radius;
                    let seek = p.kind == "laser" && this.p.laser_seek > rad;
                    if seek {
                        rad = this.p.laser_seek;
                    }
                    if this.want_pickup(me, &p.kind) && hyp(p.pos[0] - x, p.pos[1] - y) < rad {
                        let mut v = 60.0;
                        if me.weapons.len() > 1 && me.weapons[1].is_none() {
                            v = 130.0;
                        }
                        if seek {
                            v = fmax(v, 150.0);
                        }
                        consider("pickup", p.id, p.pos, v);
                    }
                }
                let mut cube_v = 75.0;
                if me.cubes >= this.cube_cap() {
                    cube_v = 25.0;
                }
                for c in this.cubes.values() {
                    if hyp(c.pos[0] - x, c.pos[1] - y) < fmax(60.0, this.p.loot_radius * 0.7) {
                        consider("cube", c.id, c.pos, cube_v);
                    }
                }
                let mut farm_hp = 0.7;
                if this.p.farm_hp > 0.0 {
                    farm_hp = this.p.farm_hp;
                }
                if hp_frac > farm_hp && !danger && now < this.p.crate_until {
                    for c in this.crates.values() {
                        if hyp(c.pos[0] - x, c.pos[1] - y) < this.p.loot_radius {
                            consider("crate", c.id, c.pos, cube_v * 0.6);
                        }
                    }
                    if this.p.farm_npc && me.cubes < this.cube_cap() {
                        for n in this.npcs.values() {
                            if now - n.seen < 12.0 && hyp(n.pos[0] - x, n.pos[1] - y) < this.p.loot_radius {
                                consider("npc", n.id, n.pos, 85.0);
                            }
                        }
                    }
                }
                for d in &o.drops {
                    let dist = hyp(d.pos[0] - x, d.pos[1] - y);
                    if this.p.use_drops && dist / me.stats.walk_speed < d.lands_in + 25.0 && !danger {
                        consider("drop", d.id, d.pos, 260.0);
                    }
                }
            }
        }
        let Some(best) = best else {
            self.loot_id = -1;
            return None;
        };
        if econ {
            let lim = 30.0;
            if best.id != self.loot_id {
                self.loot_id = best.id;
                self.loot_since = now;
            } else if now - self.loot_since > lim
                || (best.kind == "crate" && now - self.loot_since > lim - 6.0 && hyp(best.pos[0] - x, best.pos[1] - y) < 50.0)
            {
                self.bad.insert(best.id, now + 60.0);
                self.loot_id = -1;
                return None;
            }
            return Some(best);
        }
        if best.id != self.loot_id {
            self.loot_id = best.id;
            self.loot_since = now;
        } else if now - self.loot_since > 30.0 {
            self.bad.insert(best.id, now + 60.0);
            self.loot_id = -1;
            return None;
        }
        Some(best)
    }

    fn explore_move(&mut self, o: &Obs) -> (f64, f64) {
        let me = &o.self_;
        let (x, y) = (me.pos[0], me.pos[1]);
        let z = &o.zone;
        if self.now > self.explore_until || hyp(self.explore[0] - x, self.explore[1] - y) < 6.0 {
            for _try in 0..20 {
                let a = self.rng.float64() * 2.0 * PI;
                let r = self.rng.float64().sqrt() * z.next_radius * 0.8;
                let (px, py) = (z.next_center[0] + a.cos() * r, z.next_center[1] + a.sin() * r);
                if !self.w.in_bounds(px, py) || !self.w.free(px, py) {
                    continue;
                }
                let d = hyp(px - x, py - y);
                if d < 50.0 || d > 220.0 {
                    continue;
                }
                self.explore = [px, py];
                self.explore_until = self.now + 30.0;
                break;
            }
        }
        if self.now > self.explore_until {
            return (0.0, 0.0);
        }
        let (ex, ey) = (self.explore[0], self.explore[1]);
        self.travel(me, ex, ey)
    }

    fn home_cost(&self, o: &Obs, px: f64, py: f64) -> f64 {
        let z = &o.zone;
        let mut crowd = 0.0;
        for t in self.tracks.values() {
            if self.now - t.seen < 20.0 {
                crowd += fmax(0.0, 80.0 - hyp(t.pos[0] - px, t.pos[1] - py)) / 80.0;
            }
        }
        for d in &o.drops {
            crowd += fmax(0.0, 60.0 - hyp(d.pos[0] - px, d.pos[1] - py)) / 60.0;
        }
        let edge = hyp(px - z.next_center[0], py - z.next_center[1]) / fmax(30.0, z.next_radius);
        self.p.home_expo * self.w.exposure(px, py) + 0.25 * hyp(px - o.self_.pos[0], py - o.self_.pos[1]) + 40.0 * crowd + 10.0 * edge
    }

    fn home(&mut self, o: &Obs) -> (f64, f64, bool) {
        let z = &o.zone;
        let out = !self.home_set || hyp(self.home_xy[0] - z.next_center[0], self.home_xy[1] - z.next_center[1]) > z.next_radius * 0.8;
        if out || self.now - self.home_at > 12.0 {
            self.home_at = self.now;
            let (mut best, mut best_c) = ([0.0f64; 2], f64::INFINITY);
            for _k in 0..40 {
                let a = self.rng.float64() * 2.0 * PI;
                let r = self.rng.float64().sqrt() * z.next_radius * 0.7;
                let (px, py) = (z.next_center[0] + a.cos() * r, z.next_center[1] + a.sin() * r);
                if !self.w.in_bounds(px, py) || !self.w.reachable(px, py) {
                    continue;
                }
                let c = self.home_cost(o, px, py);
                if c < best_c {
                    best = [px, py];
                    best_c = c;
                }
            }
            if !(best_c == f64::INFINITY) && (out || best_c < self.home_cost(o, self.home_xy[0], self.home_xy[1]) - 15.0) {
                self.home_xy = best;
                self.home_set = true;
            }
        }
        (self.home_xy[0], self.home_xy[1], self.home_set)
    }

    fn hide_from_bearing(&mut self, o: &Obs, seen: &[Track], mx: &mut f64, my: &mut f64) -> bool {
        let me = &o.self_;
        let mut ghost = Track::default();
        ghost.pos = [me.pos[0] + self.hurt_bearing.cos() * 80.0, me.pos[1] + self.hurt_bearing.sin() * 80.0, me.pos[2]];
        let mut all = Vec::with_capacity(seen.len() + 1);
        all.push(ghost);
        all.extend_from_slice(seen);
        self.cover_move(o, &all, mx, my)
    }

    fn dodge(&self, o: &Obs) -> (f64, f64, bool) {
        let me = &o.self_;
        let (cx, cy, cz) = (me.pos[0], me.pos[1], me.pos[2] + me.stats.height * 0.56);
        let reach = me.stats.radius + 1.1;
        let (mut dx, mut dy, mut laser) = (0.0, 0.0, false);
        for p in &o.projectiles {
            if p.kind == "mortar" {
                continue;
            }
            let v2 = p.vel[0] * p.vel[0] + p.vel[1] * p.vel[1] + p.vel[2] * p.vel[2];
            if v2 < 1.0 {
                continue;
            }
            let tc = ((cx - p.pos[0]) * p.vel[0] + (cy - p.pos[1]) * p.vel[1] + (cz - p.pos[2]) * p.vel[2]) / v2;
            if tc <= 0.0 || tc > 0.7 {
                continue;
            }
            let (mxx, myy) = (p.pos[0] + p.vel[0] * tc - cx, p.pos[1] + p.vel[1] * tc - cy);
            if hyp(mxx, myy) < reach {
                let vh = hyp(p.vel[0], p.vel[1]);
                if vh < 1.0 {
                    continue;
                }
                let (mut px, mut py) = (-p.vel[1] / vh, p.vel[0] / vh);
                if px * mxx + py * myy > 0.0 {
                    px = -px;
                    py = -py;
                }
                dx += px;
                dy += py;
            }
        }
        for m in &o.mechs {
            if m.kind != "mech" || !m.charging {
                continue;
            }
            let to_me = (cy - m.pos[1]).atan2(cx - m.pos[0]);
            if ang_diff(m.aim[0], to_me).abs() > 0.15 {
                continue;
            }
            dx += (to_me + PI / 2.0 * self.strafe).cos() * 1.5;
            dy += (to_me + PI / 2.0 * self.strafe).sin() * 1.5;
            laser = true;
        }
        let l = hyp(dx, dy);
        if l > 0.01 {
            return (dx / l, dy / l, laser);
        }
        (0.0, 0.0, false)
    }

    fn reach(&self, me: &SelfObs) -> f64 {
        let mut r = 0.0;
        for w in me.weapons.iter().flatten() {
            match w.kind.as_str() {
                "gun" => r = fmax(r, fmin(w.stats.range - 10.0, 130.0)),
                "laser" => r = fmax(r, w.stats.range - 5.0),
                "lightning" => r = fmax(r, w.stats.range),
                _ => {}
            }
        }
        r
    }

    fn odds(&self, me: &SelfObs, t: &Track, now: f64, opening: bool) -> f64 {
        let (my_w, mut th_w) = (me.weapons.iter().filter(|w| w.is_some()).count() as i64, t.weapons.len() as i64);
        if th_w == 0 {
            th_w = 1;
        }
        let capc = |c: i64| -> f64 { fmin(c as f64, 10.0) };
        let (mut busy_me, mut busy_them) = (0.0, 0.0);
        for u in self.tracks.values() {
            if u.id == t.id || now - u.seen > 2.5 {
                continue;
            }
            if now - u.hit_me < 6.0 {
                busy_me = 1.0;
            }
            let br = (u.pos[1] - t.pos[1]).atan2(u.pos[0] - t.pos[0]);
            if ang_diff(t.aim[0], br).abs() < 0.2 && hyp(u.pos[0] - t.pos[0], u.pos[1] - t.pos[1]) < 160.0 {
                busy_them = 1.0;
            }
        }
        let mut z = 1.5093 * (fmax(me.hp, 1.0) / fmax(t.hp, 1.0)).ln() + 0.7049 * (capc(me.cubes) - capc(t.cubes)) / 10.0
            + 0.9583 * (my_w - th_w) as f64
            - 0.5592 * (busy_me - busy_them);
        if opening {
            z += 0.0562 + 0.7475 * fmin(t.dist, 200.0) / 100.0;
            let to_me = (me.pos[1] - t.pos[1]).atan2(me.pos[0] - t.pos[0]);
            if ang_diff(t.aim[0], to_me).abs() > 1.75 {
                z += 0.8235;
            }
        }
        1.0 / (1.0 + (-z).exp())
    }

    fn cube_cap(&self) -> i64 {
        let mut c = self.cfg.cubes.stat_cap;
        let g = self.p.cube_goal as i64;
        if g > c {
            c = g;
        }
        c
    }

    fn rear_mult(&self) -> f64 {
        let m = self.cfg.mech.rear_mult;
        if m > 1.0 {
            return m;
        }
        1.0
    }

    fn shows_back(&self, t: &Track, x: f64, y: f64) -> bool {
        let mut arc = self.cfg.mech.front_arc_deg;
        if arc <= 0.0 {
            arc = 180.0;
        }
        ang_diff(t.aim[0], (y - t.pos[1]).atan2(x - t.pos[0])).abs() > arc * PI / 360.0 + 0.1
    }

    /// export.go
    pub fn set_now(&mut self, t: f64) {
        self.now = t;
    }
}

pub fn squad_params(mut p: Params) -> Params {
    p.ghost = false;
    p.peek = false;
    p.laser_first = true;
    p.laser_range = 120.0;
    p.loot_radius = 150.0;
    p.crate_until = 1e9;
    p.stick = 120.0;
    p.hpw = 0.2;
    p.low_bonus = 100.0;
    p.max_target = 240.0;
    p.lose_rule = true;
    p.fight_loot = 70.0;
    p.farm_hp = 0.5;

    p.laser_seek = 240.0;
    p.laser_up = true;
    p.lethal = true;
    p.finish = 20.0;
    p.stick = 200.0;
    p.low_bonus = 200.0;
    p.lethal_k = 0.7;
    p.hpw = 0.15;
    p.hpw = 0.18;
    apply_overrides(&mut p);

    p
}
