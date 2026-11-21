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
    /// When we first saw its current laser charge (s), or -1.
    charge_t0: f32,
    cubes: i32,
    seen_t: f32,
    first_t: f32,
    /// EMA of the angle between its aim and a perfect gun lead on us (rad): skill estimate.
    aerr: f32,
}

/// Per-opponent engagement record (MECHBOT_DUEL instrumentation).
#[derive(Clone, Debug, Default)]
struct Duel {
    t0: f32,
    last: f32,
    dealt: f32,
    taken: f32,
    taken_other: f32,
    my_hp0: f32,
    their_hp0: f32,
    their_hp: f32,
    their_w: String,
    their_cubes: i32,
    my_cubes: i32,
    my_w: String,
    tk_laser: f32,
    tk_gun: f32,
    tk_light: f32,
    tk_mortar: f32,
    tk_rear: f32,
    dl_laser: f32,
    dl_gun: f32,
    ticks: u32,
    tg_ticks: u32,
    st: [u32; 7],
    sum_d: f32,
    n_others: u32,
    cover_ticks: u32,
}

/// Weapon-usage statistics (MECHBOT_WSTAT instrumentation).
#[derive(Clone, Debug, Default)]
struct WStat {
    on: bool,
    laser_t: f32,
    first_w2_t: f32,
    /// Damage dealt to mechs by weapon: gun, laser, lightning, mortar.
    dmg_m: [f32; 4],
    /// Damage dealt to npcs/crates by weapon.
    dmg_o: [f32; 4],
    l_hits_m: u32,
    l_hits_o: u32,
    /// Laser releases: total, at a mech target, partial, held long, no LOS at release.
    l_rel: u32,
    l_rel_m: u32,
    l_partial: u32,
    l_held: u32,
    l_rel_nolos: u32,
    l_charge_sum: f32,
    ch_ticks: u32,
    ch_nolos: u32,
    ch_notgt: u32,
    shorted: u32,
    prev_charge: f32,
    prev_los: bool,
    prev_mech: bool,
    full_ticks: u32,
    full_sum: u32,
    takes: Vec<String>,
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
    sn_pair: Option<([f32; 2], [f32; 2])>,
    sn_t: f32,
    sn_tpos: [f32; 2],
    sn_fire_t: f32,
    sn_prev_charge: f32,
    sn_active: bool,
    sn_stat: [u32; 4],
    duels: HashMap<i64, Duel>,
    duel_on: bool,
    juke_until: f32,
    juke_dir: [f32; 2],
    juke_src: i64,
    akl_prev: [f32; 2],
    ws: WStat,
    /// Death instrumentation: (t, by, weapon, amount, rear, dist to attacker or -1).
    dth_log: VecDeque<(f32, i64, String, f32, bool, f32)>,
    /// (t, mode, retreating, kiting, hp, n visible enemies, n shooting)
    dth_hist: VecDeque<(f32, Mode, bool, bool, f32, usize, usize)>,
    kill_ts: Vec<f32>,
    dth_kiting: bool,
    dth_nshoot: usize,
    dth_myw: String,
    dth_cubes: i32,
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
    /// Laser sniper: charge in cover, peek out to release, duck back during cooldown.
    pub sniper: f32,
    pub sn_min_d: f32,
    pub sn_r: f32,
    pub sn_stay: f32,
    pub sn_loot: f32,
    pub sn_others: f32,
    pub sn_cover: f32,
    pub sn_hyst: f32,
    pub sn_maxp: f32,
    /// Laser juke: 1 = reverse lateral motion during an enemy's charge when no cover-dodge, 2 = always.
    pub lj: f32,
    /// Anti-lightning kite: keep out of arc range (this many m) of lightning bots, never hide from them.
    pub akl: f32,
    /// 1 = also when we carry lightning ourselves.
    pub akl_own: f32,
    /// Stand and fight (no retreat to cover) when the target is this close (m).
    pub corner_d: f32,
    /// Mortar arc throwers whenever ready (concussion pushes them out of range, drains energy).
    pub mlz: f32,
    pub lj_t0: f32,
    pub lj_len: f32,
    /// Gun keeps firing (aimed at its own intercept) while the laser charges.
    pub chg_gun: f32,
    /// Release the laser once charge >= lrel x full and aligned on target.
    pub lrel: f32,
    /// Laser (hitscan) aim lead in seconds of target velocity.
    pub llead: f32,
    /// 1 = laser upgrades first while holding a laser.
    pub lupg: f32,
    /// 1 = may start charging without line of sight while in cover.
    pub lcov: f32,
    /// >0: release a full charge held this long (s) toward the target instead of letting it auto-fire.
    pub lhold: f32,
    /// Mortar slow/clustered targets even with a working gun in LOS.
    pub mclus: f32,
    /// Target-score bonus (m) for mortar carriers while we hold a laser (cook-off).
    pub lcook: f32,
    /// Remember out-of-view enemies for this many s (cover choice, threat estimate).
    pub memt: f32,
    pub nlr: f32,
    pub nlr_t: f32,
    pub nlr_d: f32,
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
            retreat_ratio: 100.0,
            retreat_hp: 0.0,
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
            sniper: 0.0,
            sn_min_d: 45.0,
            sn_r: 17.0,
            sn_stay: 0.0,
            sn_loot: 200.0,
            sn_others: 40.0,
            sn_cover: 0.0,
            sn_hyst: 0.0,
            sn_maxp: 9.0,
            lj: 0.0,
            akl: 52.0,
            akl_own: 1.0,
            corner_d: 0.0,
            mlz: 1.0,
            lj_t0: 0.15,
            lj_len: 0.6,
            chg_gun: 0.0,
            lrel: 1.0,
            llead: 0.1,
            lupg: 0.0,
            lcov: 1.0,
            lhold: 0.0,
            mclus: 0.0,
            lcook: 0.0,
            memt: 0.0,
            nlr: 2.0,
            nlr_t: 90.0,
            nlr_d: 35.0,
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
                    "sniper" => p.sniper = x,
                    "sn_min_d" => p.sn_min_d = x,
                    "sn_r" => p.sn_r = x,
                    "sn_stay" => p.sn_stay = x,
                    "sn_loot" => p.sn_loot = x,
                    "sn_others" => p.sn_others = x,
                    "sn_cover" => p.sn_cover = x,
                    "sn_hyst" => p.sn_hyst = x,
                    "sn_maxp" => p.sn_maxp = x,
                    "lj" => p.lj = x,
                    "akl" => p.akl = x,
                    "akl_own" => p.akl_own = x,
                    "corner_d" => p.corner_d = x,
                    "mlz" => p.mlz = x,
                    "lj_t0" => p.lj_t0 = x,
                    "lj_len" => p.lj_len = x,
                    "chg_gun" => p.chg_gun = x,
                    "lrel" => p.lrel = x,
                    "llead" => p.llead = x,
                    "lupg" => p.lupg = x,
                    "lcov" => p.lcov = x,
                    "lhold" => p.lhold = x,
                    "mclus" => p.mclus = x,
                    "lcook" => p.lcook = x,
                    "memt" => p.memt = x,
                    "nlr" => p.nlr = x,
                    "nlr_t" => p.nlr_t = x,
                    "nlr_d" => p.nlr_d = x,
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
            sn_pair: None,
            sn_t: -100.0,
            sn_tpos: [0.0, 0.0],
            sn_fire_t: -100.0,
            sn_prev_charge: 0.0,
            sn_active: false,
            sn_stat: [0; 4],
            duels: HashMap::new(),
            juke_until: -1.0,
            juke_dir: [0.0, 0.0],
            juke_src: -1,
            akl_prev: [0.0, 0.0],
            ws: WStat { on: std::env::var("MECHBOT_WSTAT").is_ok(), laser_t: -1.0, first_w2_t: -1.0, ..Default::default() },
            dth_log: VecDeque::new(),
            dth_hist: VecDeque::new(),
            kill_ts: Vec::new(),
            dth_kiting: false,
            dth_nshoot: 0,
            dth_myw: String::new(),
            dth_cubes: 0,
            duel_on: std::env::var("MECHBOT_DUEL").is_ok(),
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

    fn duel_close(&self, id: i64, d: &Duel, why: &str) {
        let n = d.tg_ticks.max(1) as f32;
        eprintln!(
            "DUEL id={} why={} t0={:.1} dur={:.1} dealt={:.0} taken={:.0} other={:.0} myhp0={:.0} hp0={:.0} hpend={:.0} tw={} tc={} mc={} mw={} tk_l={:.0} tk_g={:.0} tk_z={:.0} tk_m={:.0} rear={:.0} dl_l={:.0} dl_g={:.0} d={:.0} fire={:.2} nolos={:.2} rng={:.2} rel={:.2} aim={:.2} hold={:.2} oth={:.2} cov={:.2} tgf={:.2}",
            id, why, d.t0, d.last - d.t0, d.dealt, d.taken, d.taken_other, d.my_hp0, d.their_hp0, d.their_hp, d.their_w, d.their_cubes, d.my_cubes, d.my_w,
            d.tk_laser, d.tk_gun, d.tk_light, d.tk_mortar, d.tk_rear, d.dl_laser, d.dl_gun, d.sum_d / n,
            d.st[1] as f32 / n, d.st[2] as f32 / n, d.st[3] as f32 / n, d.st[4] as f32 / n, d.st[5] as f32 / n, d.st[6] as f32 / n,
            d.n_others as f32 / d.ticks.max(1) as f32, d.cover_ticks as f32 / d.ticks.max(1) as f32, d.tg_ticks as f32 / d.ticks.max(1) as f32
        );
    }

    /// Per-opponent engagement bookkeeping (only with MECHBOT_DUEL).
    fn duel_track(&mut self, obs: &Obs, mode: Mode, gst_before: [u32; 7]) {
        let me = &obs.me;
        let wn = |w: &Vec<String>| w.join("+");
        let my_w: String = me.weapons.iter().flatten().map(|w| w.kind.clone()).collect::<Vec<_>>().join("+");
        let mut touched: Vec<(i64, f32, bool, Option<String>, bool)> = vec![];
        for e in &obs.events {
            match e.kind.as_str() {
                "damaged" => {
                    if let Some(b) = e.by.filter(|b| *b < 100) {
                        touched.push((b, e.amount.unwrap_or(0.0), true, e.weapon.clone(), e.rear.unwrap_or(false)));
                    }
                }
                "hit" => {
                    if let Some(tg) = e.target.filter(|b| *b < 100) {
                        if self.tracks.contains_key(&tg) || self.duels.contains_key(&tg) {
                            touched.push((tg, e.amount.unwrap_or(0.0), false, e.weapon.clone(), false));
                        }
                    }
                }
                _ => {}
            }
        }
        for (id, amt, taken, w, rear) in touched.iter().cloned() {
            if !self.duels.contains_key(&id) {
                let tr = self.tracks.get(&id);
                self.duels.insert(
                    id,
                    Duel {
                        t0: obs.t,
                        last: obs.t,
                        my_hp0: me.hp + if taken { amt } else { 0.0 },
                        their_hp0: tr.map(|t| t.hp).unwrap_or(-1.0),
                        their_hp: tr.map(|t| t.hp).unwrap_or(-1.0),
                        their_w: tr.map(|t| wn(&t.weapons)).unwrap_or_default(),
                        their_cubes: tr.map(|t| t.cubes).unwrap_or(-1),
                        my_cubes: me.cubes,
                        my_w: my_w.clone(),
                        ..Default::default()
                    },
                );
            }
            let d = self.duels.get_mut(&id).unwrap();
            d.last = obs.t;
            if taken {
                d.taken += amt;
                match w.as_deref() {
                    Some("laser") => d.tk_laser += amt,
                    Some("gun") => d.tk_gun += amt,
                    Some("lightning") => d.tk_light += amt,
                    Some("mortar") => d.tk_mortar += amt,
                    _ => {}
                }
                if rear {
                    d.tk_rear += amt;
                }
            } else {
                d.dealt += amt;
                match w.as_deref() {
                    Some("laser") => d.dl_laser += amt,
                    Some("gun") => d.dl_gun += amt,
                    _ => {}
                }
            }
        }
        let ids: Vec<i64> = self.duels.keys().copied().collect();
        for id in &ids {
            let other: f32 = touched.iter().filter(|x| x.2 && x.0 != *id).map(|x| x.1).sum();
            let n_oth = touched.iter().filter(|x| x.2 && x.0 != *id).count();
            let is_tg = self.target_id == Some(*id);
            let tr = self.tracks.get(id).cloned();
            let d = self.duels.get_mut(id).unwrap();
            d.taken_other += other;
            d.ticks += 1;
            if n_oth > 0 {
                d.n_others += 1;
            }
            if mode == Mode::Cover {
                d.cover_ticks += 1;
            }
            if let Some(tr) = tr {
                if obs.t - tr.seen_t < 0.01 {
                    d.their_hp = tr.hp;
                    d.their_w = wn(&tr.weapons);
                }
                if is_tg {
                    d.tg_ticks += 1;
                    d.sum_d += hdist(tr.pos, me.pos);
                    for k in 1..7 {
                        if self.gstat[k] > gst_before[k] {
                            d.st[k] += 1;
                        }
                    }
                }
            }
        }
        let killed: Vec<i64> = obs.events.iter().filter(|e| e.kind == "kill").filter_map(|e| e.target).collect();
        for id in ids {
            let d = self.duels[&id].clone();
            if killed.contains(&id) {
                self.duel_close(id, &d, "kill");
                self.duels.remove(&id);
            } else if obs.t - d.last > 8.0 {
                self.duel_close(id, &d, "split");
                self.duels.remove(&id);
            }
        }
    }

    pub fn log_end(&self, raw: &str) {
        eprintln!("end: {raw} gunstat[mech_ticks,fire,nolos,range,ammo/reload,aim,hold]={:?} sniper[peek,hide,nopair,_]={:?}", self.gstat, self.sn_stat);
        if self.ws.on {
            let w = &self.ws;
            eprintln!(
                "wstat: {{\"t\":{:.1},\"laser_t\":{:.1},\"w2_t\":{:.1},\"takes\":{:?},\"dmg_m\":[{:.0},{:.0},{:.0},{:.0}],\"dmg_o\":[{:.0},{:.0},{:.0},{:.0}],\"l_hits_m\":{},\"l_hits_o\":{},\"l_rel\":{},\"l_rel_m\":{},\"l_partial\":{},\"l_held\":{},\"l_rel_nolos\":{},\"l_charge_sum\":{:.2},\"ch_ticks\":{},\"ch_nolos\":{},\"ch_notgt\":{},\"shorted\":{},\"full_sum\":{}}}",
                self.t, w.laser_t, w.first_w2_t, w.takes, w.dmg_m[0], w.dmg_m[1], w.dmg_m[2], w.dmg_m[3], w.dmg_o[0], w.dmg_o[1], w.dmg_o[2], w.dmg_o[3],
                w.l_hits_m, w.l_hits_o, w.l_rel, w.l_rel_m, w.l_partial, w.l_held, w.l_rel_nolos, w.l_charge_sum, w.ch_ticks, w.ch_nolos, w.ch_notgt, w.shorted, w.full_sum
            );
        }
        if self.duel_on {
            self.death_line(raw);
            for (id, d) in &self.duels {
                self.duel_close(*id, d, if self.t - d.last < 8.0 { "died" } else { "split" });
            }
        }
    }

    fn death_line(&self, raw: &str) {
        let t = self.t;
        let w = |dt: f32| self.dth_log.iter().filter(move |d| t - d.0 < dt);
        let mut att5: Vec<i64> = w(5.0).filter(|d| d.1 >= 0 && d.1 < 100).map(|d| d.1).collect();
        att5.sort();
        att5.dedup();
        let mut att10: Vec<i64> = w(10.0).filter(|d| d.1 >= 0 && d.1 < 100).map(|d| d.1).collect();
        att10.sort();
        att10.dedup();
        let sumw = |k: &str| w(10.0).filter(|d| d.2 == k).map(|d| d.3).sum::<f32>();
        let tot10: f32 = w(10.0).map(|d| d.3).sum();
        let rear10: f32 = w(10.0).filter(|d| d.4).map(|d| d.3).sum();
        let npc10: f32 = w(10.0).filter(|d| d.1 >= 100).map(|d| d.3).sum();
        let zone10: f32 = w(10.0).filter(|d| d.1 < 0).map(|d| d.3).sum();
        // top attacker over 10 s
        let mut per: Vec<(i64, f32, f32)> = vec![];
        for d in w(10.0).filter(|d| d.1 >= 0 && d.1 < 100) {
            match per.iter_mut().find(|p| p.0 == d.1) {
                Some(p) => {
                    p.1 += d.3;
                    if d.5 >= 0.0 {
                        p.2 = d.5;
                    }
                }
                None => per.push((d.1, d.3, d.5)),
            }
        }
        per.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap());
        let top = per.first().cloned().unwrap_or((-1, 0.0, -1.0));
        let topw = self.tracks.get(&top.0).map(|tr| tr.weapons.join("+")).unwrap_or_default();
        let toph = self.tracks.get(&top.0).map(|tr| tr.hp).unwrap_or(-1.0);
        let h10: Vec<_> = self.dth_hist.iter().filter(|h| t - h.0 < 10.0).collect();
        let n = h10.len().max(1) as f32;
        let fr = |f: &dyn Fn(&&(f32, Mode, bool, bool, f32, usize, usize)) -> bool| h10.iter().filter(|h| f(h)).count() as f32 / n;
        let hpmax20 = self.dth_hist.iter().filter(|h| t - h.0 < 20.0).map(|h| h.4).fold(0.0f32, f32::max);
        let hp10 = self.dth_hist.iter().find(|h| t - h.0 < 10.0).map(|h| h.4).unwrap_or(-1.0);
        let fdmg = self.dth_log.iter().filter(|d| d.1 >= 0).map(|d| d.0).fold(1e9f32, f32::min);
        let mut bursts = 0.0f32;
        // time from the first hit of the final continuous engagement (no 5 s gap) to death
        let mut eng0 = t;
        for d in self.dth_log.iter().rev() {
            if eng0 - d.0 > 5.0 {
                break;
            }
            eng0 = d.0;
            bursts += d.3;
        }
        let lastk = self.kill_ts.last().copied().unwrap_or(-1.0);
        eprintln!(
            "DEATH raw={} t={:.1} kills={} lastk={:.1} sincek={:.1} att5={} att10={} tot10={:.0} l={:.0} g={:.0} z={:.0} m={:.0} rear={:.0} npc={:.0} zone={:.0} top={} topdmg={:.0} topd={:.0} topw={} toph={:.0} mw={} cubes={} hpmax20={:.0} hp10={:.0} fight={:.2} cover={:.2} other={:.2} retr={:.2} kite={:.2} vis={:.1} shoot={:.1} eng={:.1} engdmg={:.0} firstdmg={:.1}",
            raw.replace(' ', ""), t, self.kill_ts.len(), lastk, if lastk >= 0.0 { t - lastk } else { t }, att5.len(), att10.len(), tot10,
            sumw("laser"), sumw("gun"), sumw("lightning"), sumw("mortar"), rear10, npc10, zone10,
            top.0, top.1, top.2, if topw.is_empty() { "?".into() } else { topw }, toph, self.dth_myw, self.dth_cubes, hpmax20, hp10,
            fr(&|h| h.1 == Mode::Fight), fr(&|h| h.1 == Mode::Cover), fr(&|h| h.1 != Mode::Fight && h.1 != Mode::Cover),
            fr(&|h| h.2), fr(&|h| h.3),
            h10.iter().map(|h| h.5 as f32).sum::<f32>() / n, h10.iter().map(|h| h.6 as f32).sum::<f32>() / n,
            t - eng0, bursts, fdmg
        );
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

    /// Sniper geometry: a (hide, peek) pair near us. `hide` is hidden from the target's eye,
    /// `peek` has a muzzle line to the target's centre, and the two are a few metres apart so
    /// we can step out to release a charged laser and step back during the cooldown.
    #[allow(clippy::too_many_arguments)]
    fn find_peek(&self, pos: V3, tpos: V3, tsc: f32, others: &[V3], circle: ([f32; 2], f32), muzzle_h: f32, my_scale: f32) -> Option<([f32; 2], [f32; 2])> {
        let tc = [tpos[0], tpos[1], tpos[2] + 4.5 * tsc];
        let teye = Self::eye_of(tpos, tsc);
        let p2 = xy(pos);
        let mut peeks: Vec<([f32; 2], f32)> = Vec::new();
        let mut hides: Vec<([f32; 2], f32)> = Vec::new();
        let rmax = self.prm.sn_r;
        let rings = [0.0f32, 2.5, 5.0, 8.0, 12.0, 17.0, 23.0];
        let mut cands: Vec<([f32; 2], f32)> = Vec::new();
        for (ri, &r) in rings.iter().enumerate() {
            if r > rmax {
                break;
            }
            let n = if r == 0.0 { 1 } else { 16 };
            for k in 0..n {
                let a = (k as f32 + 0.5 * (ri % 2) as f32) * 2.0 * PI / n as f32;
                cands.push(([p2[0] + r * a.cos(), p2[1] + r * a.sin()], r));
            }
        }
        // Hysteresis: re-offer the current pair at a discount so we don't flip between spots.
        if self.prm.sn_hyst > 0.0 {
            if let Some((h, p)) = self.sn_pair {
                cands.push((h, dist2(h, p2) - self.prm.sn_hyst));
                cands.push((p, dist2(p, p2) - self.prm.sn_hyst));
            }
        }
        {
            for &(c, r) in &cands {
                if r != 0.0 && (!self.nav.walkable_at(c[0], c[1]) || self.nav.clearance_at(c[0], c[1]) < 2.0) {
                    continue;
                }
                if dist2(c, circle.0) > circle.1 || dist2(c, xy(tpos)) < self.prm.sn_min_d * 0.7 {
                    continue;
                }
                let g = if r == 0.0 { pos[2] } else { self.world.ground(c[0], c[1]) };
                let mid = [c[0], c[1], g + 4.5 * my_scale];
                let top = [c[0], c[1], g + 8.8 * my_scale];
                let muz = [c[0], c[1], g + muzzle_h];
                let n_others = others.iter().filter(|e| self.world.los(**e, mid)).count() as f32;
                let pen = n_others * self.prm.sn_others;
                if self.world.los(muz, tc) {
                    peeks.push((c, r + pen));
                } else if !self.world.los(teye, mid) && !self.world.los(teye, top) {
                    hides.push((c, r + pen));
                }
            }
        }
        let mut best: Option<(f32, [f32; 2], [f32; 2])> = None;
        for h in &hides {
            for p in &peeks {
                let dp = dist2(h.0, p.0);
                if dp > self.prm.sn_maxp {
                    continue;
                }
                let s = h.1 + p.1 * 0.5 + dp * 1.5;
                if best.map(|b| s < b.0).unwrap_or(true) {
                    best = Some((s, h.0, p.0));
                }
            }
        }
        best.map(|b| (b.1, b.2))
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
                "hit" => {
                    self.last_deal_t = obs.t;
                    if self.ws.on {
                        let wi = match e.weapon.as_deref() {
                            Some("gun") => 0,
                            Some("laser") => 1,
                            Some("lightning") => 2,
                            _ => 3,
                        };
                        let mech = e.target.map(|t| t < 100).unwrap_or(false);
                        let a = e.amount.unwrap_or(0.0);
                        if mech {
                            self.ws.dmg_m[wi] += a;
                        } else {
                            self.ws.dmg_o[wi] += a;
                        }
                        if wi == 1 {
                            if mech {
                                self.ws.l_hits_m += 1;
                            } else {
                                self.ws.l_hits_o += 1;
                            }
                        }
                    }
                }
                "effect" if self.ws.on && e.effect.as_deref() == Some("short") => self.ws.shorted += 1,
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
            let charge_t0 = self.tracks.get(&m.id).filter(|t| t.charging && t.charge_t0 >= 0.0 && obs.t - t.seen_t < 0.3).map(|t| t.charge_t0).unwrap_or(obs.t);
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
                    charge_t0: if m.charging { charge_t0 } else { -1.0 },
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
            let lz_held = me.weapons.iter().flatten().any(|w| w.kind == "laser");
            for (i, c) in offer.choices.iter().enumerate() {
                let mut r = 10 + 10 * rank.iter().position(|u| *u == c.id).unwrap_or(rank.len());
                if self.prm.lupg > 0.0 && lz_held {
                    if c.id == "laser_damage" {
                        r = 0;
                    } else if c.id == "laser_charge" && self.prm.lupg > 1.5 {
                        r = 1;
                    } else if c.id == "laser_charge" {
                        r = r.min(15);
                    }
                }
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
            // Laser cook-off: +60% to mortar carriers.
            if self.prm.lcook > 0.0 && m.has("mortar") && me.weapons.iter().flatten().any(|w| w.kind == "laser") {
                s -= self.prm.lcook;
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
        let mut threat_eyes: Vec<V3> = enemies
            .iter()
            .filter(|m| hdist(m.pos, pos) < 260.0)
            .map(|m| Self::eye_of(m.pos, 1.0 + 0.05 * m.cubes.unwrap_or(0) as f32))
            .collect();
        // Remembered threats: enemies out of our view cone (seen in the last `memt` s, dead
        // reckoned) and unseen shooters placed along their damage bearing.
        let mut mem_threats: Vec<(V3, Vec<String>, i32)> = vec![];
        if self.prm.memt > 0.0 {
            for (id, tr) in &self.tracks {
                let dt = obs.t - tr.seen_t;
                if dt < 0.01 || dt > self.prm.memt || enemies.iter().any(|m| m.id == *id) {
                    continue;
                }
                let k = dt.min(1.5);
                let p = [tr.pos[0] + tr.vel[0] * k, tr.pos[1] + tr.vel[1] * k, tr.pos[2]];
                if hdist(p, pos) < 200.0 {
                    mem_threats.push((p, tr.weapons.clone(), tr.cubes));
                }
            }
            let mut done: Vec<i64> = vec![];
            for d in self.dmg_log.iter().rev().filter(|d| obs.t - d.0 < 2.0) {
                let Some(b) = d.2 else { continue };
                if b >= 100 || done.contains(&b) || enemies.iter().any(|m| m.id == b) {
                    continue;
                }
                done.push(b);
                if self.tracks.get(&b).map(|tr| obs.t - tr.seen_t <= self.prm.memt).unwrap_or(false) {
                    continue;
                }
                let p = [pos[0] + d.1.cos() * 50.0, pos[1] + d.1.sin() * 50.0, pos[2]];
                mem_threats.push((p, vec!["gun".to_string()], 0));
            }
            for (p, _, c) in &mem_threats {
                threat_eyes.push(Self::eye_of(*p, 1.0 + 0.05 * *c as f32));
            }
        }

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
        for (mp, mw, mc) in &mem_threats {
            let d = hdist(*mp, pos);
            if d > 230.0 || !self.world.los(Self::eye_of(*mp, 1.0 + 0.05 * *mc as f32), me_c) {
                continue;
            }
            n_shooting += 1;
            let mut e = if d < 60.0 { 110.0 } else if d < 130.0 { 80.0 } else { 50.0 };
            if mw.iter().any(|w| w == "laser") {
                e += 120.0;
            }
            if mw.iter().any(|w| w == "lightning") && d < 50.0 {
                e += 180.0;
            }
            their_dps += e * (1.0 + 0.04 * ((*mc).min(10) as f32));
        }
        let recent_in: f32 = self.dmg_log.iter().filter(|d| obs.t - d.0 < 2.0).map(|d| d.3).sum::<f32>() / 2.0;
        let incoming = their_dps.max(recent_in);
        let ttk = tgt_hp / our_dps.max(1.0);
        let ttd = me.hp / incoming.max(1.0);
        self.last_ttk = ttk;
        self.last_ttd = ttd;
        if self.retreating {
            let safe = hp_frac > 0.8 || enemies.is_empty() && obs.t - self.last_dmg_t > 10.0;
            let cornered = tgt_d < 30.0 && ttk < ttd * 0.8 || self.prm.corner_d > 0.0 && tgt_d < self.prm.corner_d && n_shooting <= 1;
            let winning = ttk < ttd * 0.9 && n_shooting <= 1 || ttk < 2.0 && ttk < ttd;
            if safe || cornered || winning {
                self.retreating = false;
            }
        } else if target_mech.is_some() {
            let losing = ttk > ttd * self.prm.retreat_ratio && ttk > 2.5 && (hp_frac < self.prm.retreat_hp || n_shooting >= 2);
            let low = hp_frac < 0.3 && tgt_hp > me.hp * 0.5;
            let pinned = self.prm.corner_d > 0.0 && tgt_d < self.prm.corner_d && n_shooting <= 1;
            if (losing || low) && !pinned {
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
        // No-laser restraint: while we only have the gun, don't start fights with laser bots
        // (nlr=1) or with anyone (nlr=2) that is not after us; keep looting for a laser instead.
        if self.prm.nlr > 0.0 && !have_laser && obs.t < self.prm.nlr_t {
            if let Some((m, _)) = &target_mech {
                let attacking = attackers.contains(&m.id) || (wrap(m.aim[0] - yaw_to(xy(m.pos), p2)).abs() < 0.2 && hdist(m.pos, pos) < 120.0);
                let hurt = m.hp.unwrap_or(1400.0) < 0.5 * m.max_hp.unwrap_or(1400.0);
                let risky = m.has("laser") || self.prm.nlr > 1.5;
                if risky && !(attacking || tgt_d < self.prm.nlr_d || hurt) {
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
        let gst_before = self.gstat;

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
                let reach = if slot_kind(1).is_empty() || slot_kind(0).is_empty() { 260.0 } else { 150.0 } + if kind == "laser" && !have_laser { self.prm.sn_loot } else { 0.0 };
                if d > reach {
                    continue;
                }
                let s = d - gain * 25.0 - if kind == "laser" && !have_laser { self.prm.sn_loot } else { 0.0 };
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
                        .filter(|c| hdist(c.pos, pos) < self.prm.farm_range + if have_laser { 0.0 } else { self.prm.sn_loot * 0.5 } && dist2(xy(c.pos), gc) < gr)
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

        // Laser sniper: charge behind cover, step out when full to release, back in on cooldown.
        self.sn_active = false;
        let lz = me.weapons.iter().flatten().find(|w| w.kind == "laser").map(|w| (w.charge.unwrap_or(0.0), w.cooldown, w.stat("charge", 0.75), w.stat("range", 280.0)));
        if let Some((ch, _, _, _)) = lz {
            if self.sn_prev_charge > 0.0 && ch <= 0.0 {
                self.sn_fire_t = obs.t;
            }
            self.sn_prev_charge = ch;
        }
        if self.prm.sniper > 0.0 && (mode == Mode::Fight || self.prm.sn_cover > 0.0 && mode == Mode::Cover && hp_frac > 0.3) {
            if let (Some((m, _)), Some((ch, cd, full, lrange))) = (&target_mech, lz) {
                let d = hdist(m.pos, pos);
                let tsc = 1.0 + 0.05 * m.cubes.unwrap_or(0) as f32;
                if d > self.prm.sn_min_d && d < lrange * 0.9 {
                    let teye = Self::eye_of(m.pos, tsc);
                    let others: Vec<V3> = threat_eyes.iter().copied().filter(|e| ((e[0] - teye[0]).powi(2) + (e[1] - teye[1]).powi(2)).sqrt() > 1.0).collect();
                    if obs.t - self.sn_t > 0.4 || dist2(self.sn_tpos, xy(m.pos)) > 6.0 || self.sn_pair.is_none() && obs.t - self.sn_t > 0.2 {
                        self.sn_t = obs.t;
                        self.sn_tpos = xy(m.pos);
                        self.sn_pair = self.find_peek(pos, m.pos, tsc, &others, (gc, gr), muzzle_h, my_scale);
                    }
                    if let Some((hide, peek)) = self.sn_pair {
                        self.sn_active = true;
                        let charging = ch > 0.0;
                        let half = walk * 0.5;
                        let walk_t = dist2(p2, peek) / half.max(1.0);
                        let staying = self.prm.sn_stay > 0.0 && obs.t - self.sn_fire_t < self.prm.sn_stay;
                        let go_peek = (charging && ch >= full - walk_t - 0.05) || staying;
                        let g = if go_peek { peek } else { hide };
                        let _ = cd;
                        if go_peek {
                            self.sn_stat[0] += 1;
                        } else {
                            self.sn_stat[1] += 1;
                        }
                        mv = if dist2(g, p2) > 0.7 { self.goto(p2, g) } else { [0.0, 0.0] };
                        want_boost = !charging && dist2(g, p2) > 4.0;
                    } else {
                        self.sn_stat[2] += 1;
                    }
                }
            }
        }

        // Laser magnet: a laser on the ground nearby is worth breaking off for, even mid-fight.
        if self.prm.sn_loot > 0.0 && !have_laser && !self.retreating && !zone_urgent && matches!(mode, Mode::Fight | Mode::Cover | Mode::Farm | Mode::Hold | Mode::Roam) {
            let near = self
                .pickups
                .iter()
                .filter(|(id, (k, pp, _))| k == "laser" && !self.bad_pickups.contains_key(id) && dist2(xy(*pp), gc) < gr && (pp[2] - pos[2]).abs() < 6.0)
                .map(|(id, (_, pp, _))| (*id, *pp, hdist(*pp, pos)))
                .filter(|x| x.2 < self.prm.sn_loot * if mode == Mode::Fight || mode == Mode::Cover { 0.5 } else { 1.0 })
                .min_by(|a, b| a.2.partial_cmp(&b.2).unwrap());
            if let Some((id, pp, d)) = near {
                mv = self.goto(p2, xy(pp));
                want_boost = d > 8.0;
                if mode == Mode::Farm {
                    aux_target = None;
                }
                if d < self.pickup_radius - 0.6 && (pp[2] - pos[2]).abs() < 3.8 {
                    let drop = if slot_kind(0).is_empty() || slot_kind(1).is_empty() {
                        None
                    } else if slot_kind(0) == "gun" {
                        Some(1)
                    } else if slot_kind(1) == "gun" {
                        Some(0)
                    } else if self.prm.prio(slot_kind(0)) <= self.prm.prio(slot_kind(1)) {
                        Some(0)
                    } else {
                        Some(1)
                    };
                    take = Some((id, drop));
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

        // Anti-lightning kite: an arc thrower inside ~50 m out-damages anything we carry and
        // follows us into cover, so back-pedal (torso keeps shooting) to stay out of its range.
        let mut kiting = false;
        if self.prm.akl > 0.0 && matches!(mode, Mode::Fight | Mode::Cover | Mode::Farm | Mode::Loot | Mode::Roam | Mode::Hold) && (!have_light || self.prm.akl_own > 0.5) {
            let zappers: Vec<[f32; 2]> = enemies
                .iter()
                .filter(|m| m.has("lightning") && hdist(m.pos, pos) < self.prm.akl && (m.pos[2] - pos[2]).abs() < 12.0)
                .map(|m| xy(m.pos))
                .collect();
            if !zappers.is_empty() {
                let others: Vec<[f32; 2]> = enemies.iter().filter(|m| !m.has("lightning") && hdist(m.pos, pos) < 90.0).map(|m| xy(m.pos)).collect();
                let mut best: Option<(f32, [f32; 2])> = None;
                for k in 0..24 {
                    let a = k as f32 * 2.0 * PI / 24.0;
                    let dv = [a.cos(), a.sin()];
                    let p4 = [p2[0] + dv[0] * 4.0, p2[1] + dv[1] * 4.0];
                    let p9 = [p2[0] + dv[0] * 9.0, p2[1] + dv[1] * 9.0];
                    if !self.nav.walkable_at(p4[0], p4[1]) {
                        continue;
                    }
                    let far_ok = self.nav.walkable_at(p9[0], p9[1]) && self.nav.line_walkable(p2, p9);
                    let mut sc = 0.0;
                    for z in &zappers {
                        sc += (dist2(p9, *z) - dist2(p2, *z)).min(9.0);
                    }
                    for o in &others {
                        let dn = dist2(p9, *o);
                        if dn < 40.0 {
                            sc -= (40.0 - dn) * 0.15;
                        }
                    }
                    if !far_ok {
                        sc -= 6.0;
                    }
                    if dist2(p9, gc) > gr {
                        sc -= 4.0 + (dist2(p9, gc) - gr);
                    }
                    sc += 2.0 * (dv[0] * self.akl_prev[0] + dv[1] * self.akl_prev[1]);
                    if best.map(|b| sc > b.0).unwrap_or(true) {
                        best = Some((sc, dv));
                    }
                }
                if let Some((_, dv)) = best {
                    mv = dv;
                    self.akl_prev = dv;
                    want_boost = true;
                    kiting = true;
                    self.dodge_goal = None;
                    if mode == Mode::Cover {
                        mode = Mode::Fight;
                    }
                }
            }
        }

        // Laser juke: house lasers aim at where we were a reaction time ago plus our velocity
        // then; a hard lateral reversal shortly before release makes that prediction miss.
        if self.prm.lj > 0.0 && matches!(mode, Mode::Fight | Mode::Cover) && (self.dodge_goal.is_none() || self.prm.lj > 1.5 || mode == Mode::Cover) {
            let me_c = [pos[0], pos[1], pos[2] + 4.5 * my_scale];
            let mut best: Option<(f32, i64, [f32; 2])> = None;
            for m in enemies.iter().filter(|m| m.charging) {
                let Some(tr) = self.tracks.get(&m.id) else { continue };
                let e = obs.t - tr.charge_t0;
                let b = yaw_to(xy(m.pos), p2);
                if wrap(m.aim[0] - b).abs() > 0.25 || !self.world.los(Self::eye_of(m.pos, 1.0 + 0.05 * m.cubes.unwrap_or(0) as f32), me_c) {
                    continue;
                }
                if best.map(|x| e > x.0).unwrap_or(true) {
                    best = Some((e, m.id, xy(m.pos)));
                }
            }
            if let Some((e, id, ep)) = best {
                let new_src = self.juke_src != id || obs.t > self.juke_until + 0.3;
                if e >= self.prm.lj_t0 && e < self.prm.lj_t0 + self.prm.lj_len && (obs.t >= self.juke_until && new_src) {
                    let b = yaw_to(ep, p2);
                    let perp = [-b.sin(), b.cos()];
                    let lat = me.vel[0] * perp[0] + me.vel[1] * perp[1];
                    let mut dir = if lat > 0.0 { [-perp[0], -perp[1]] } else { perp };
                    if !self.nav.walkable_at(p2[0] + dir[0] * 5.0, p2[1] + dir[1] * 5.0) {
                        // can't reverse: stop dead instead (also breaks a linear lead)
                        dir = [0.0, 0.0];
                    }
                    self.juke_dir = dir;
                    self.juke_src = id;
                    self.juke_until = obs.t + (self.prm.lj_t0 + self.prm.lj_len - e).max(0.1);
                } else if e < self.prm.lj_t0 {
                    // keep moving steadily so the reversal is as large as possible
                    self.strafe_switch_t = self.strafe_switch_t.max(obs.t + 0.15);
                }
            }
            if obs.t < self.juke_until {
                mv = self.juke_dir;
                want_boost = true;
                let b = self.juke_dir;
                if b != [0.0, 0.0] {
                    self.strafe_switch_t = self.juke_until + 0.2;
                }
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
        if have_light && mode == Mode::Fight && tgt_d < 50.0 && self.prm.fboost < 0.5 && !kiting {
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
            let (a, f) = self.weapons_on(obs, tg, muzzle, mode == Mode::Cover, self.sn_active);
            aim = Some(a);
            fire = f;
        }
        if self.ws.on {
            let kinds: Vec<&str> = me.weapons.iter().flatten().map(|w| w.kind.as_str()).collect();
            let ks = kinds.join("+");
            if self.ws.takes.last().map(|x| !x.ends_with(&format!(":{ks}"))).unwrap_or(true) {
                self.ws.takes.push(format!("{:.0}:{ks}", obs.t));
            }
            if kinds.contains(&"laser") && self.ws.laser_t < 0.0 {
                self.ws.laser_t = obs.t;
            }
            if kinds.len() >= 2 && self.ws.first_w2_t < 0.0 {
                self.ws.first_w2_t = obs.t;
            }
            let lw = me.weapons.iter().flatten().find(|w| w.kind == "laser");
            let ch = lw.map(|w| w.charge.unwrap_or(0.0)).unwrap_or(0.0);
            let full = lw.map(|w| w.stat("charge", 0.75)).unwrap_or(0.75);
            if self.ws.prev_charge > 0.0 && ch <= 0.0 {
                self.ws.l_rel += 1;
                self.ws.l_charge_sum += self.ws.prev_charge;
                if self.ws.prev_mech {
                    self.ws.l_rel_m += 1;
                }
                if self.ws.prev_charge < full - 0.05 {
                    self.ws.l_partial += 1;
                }
                self.ws.full_sum += self.ws.full_ticks;
                if self.ws.full_ticks >= 20 {
                    self.ws.l_held += 1;
                }
                if !self.ws.prev_los {
                    self.ws.l_rel_nolos += 1;
                }
            }
            let los_t = tgt.map(|tg| self.world.los(muzzle, [tg.pos[0], tg.pos[1], tg.pos[2] + 4.5 * tg.scale])).unwrap_or(false);
            if ch > 0.0 {
                self.ws.ch_ticks += 1;
                if tgt.is_none() {
                    self.ws.ch_notgt += 1;
                } else if !los_t {
                    self.ws.ch_nolos += 1;
                }
            }
            if ch >= full - 0.01 { self.ws.full_ticks += 1 } else { self.ws.full_ticks = 0 }
            self.ws.prev_charge = ch;
            self.ws.prev_los = los_t;
            self.ws.prev_mech = tgt.map(|t| matches!(t.kind, TargetKind::Mech)).unwrap_or(false);
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
        if self.duel_on {
            self.duel_track(obs, mode, gst_before);
            for e in &obs.events {
                match e.kind.as_str() {
                    "damaged" => {
                        let by = e.by.unwrap_or(-1);
                        let dd = self.tracks.get(&by).filter(|tr| obs.t - tr.seen_t < 1.0).map(|tr| hdist(tr.pos, pos)).unwrap_or(-1.0);
                        self.dth_log.push_back((obs.t, by, e.weapon.clone().unwrap_or_default(), e.amount.unwrap_or(0.0), e.rear.unwrap_or(false), dd));
                    }
                    "kill" => self.kill_ts.push(obs.t),
                    _ => {}
                }
            }
            while self.dth_log.front().map(|d| obs.t - d.0 > 30.0).unwrap_or(false) {
                self.dth_log.pop_front();
            }
            self.dth_hist.push_back((obs.t, mode, self.retreating, kiting, me.hp, enemies.len(), n_shooting));
            while self.dth_hist.front().map(|d| obs.t - d.0 > 20.0).unwrap_or(false) {
                self.dth_hist.pop_front();
            }
            self.dth_myw = me.weapons.iter().flatten().map(|w| w.kind.clone()).collect::<Vec<_>>().join("+");
            self.dth_cubes = me.cubes;
            let _ = self.dth_kiting;
            let _ = self.dth_nshoot;
        }
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
    fn weapons_on(&mut self, obs: &Obs, tg: Target, muzzle: V3, hiding: bool, snipe: bool) -> ([f32; 2], [bool; 2]) {
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
        let direct = aim_at(muzzle, add(center, scale(tg.vel, self.prm.llead)));
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
            // While the charge builds, the gun may keep shooting at its own intercept; swing
            // onto the hitscan line just before the charge is full.
            let gun_live = gun.map(|g| w(g).ammo.unwrap_or(1.0) > 0.0 && w(g).reload.unwrap_or(0.0) <= 0.0).unwrap_or(false) && los && allow;
            let swing = |ga: [f32; 2]| wrap(ga[0] - direct[0]).abs().max((ga[1] - direct[1]).abs()) / 3.0 + 0.1;
            if charge > 0.0 {
                aim = direct;
                if self.prm.chg_gun > 0.0 && gun_live {
                    if let Some(ga) = gun_aim {
                        if charge < full * self.prm.lrel.min(1.0) - swing(ga) {
                            aim = ga;
                        }
                    }
                }
                let ready = charge >= full * self.prm.lrel.min(1.0) - 0.03;
                if ready && los && aligned(direct) {
                    fire[li] = false; // release
                } else {
                    fire[li] = true;
                }
            } else if lw.cooldown <= 0.05 && d < range && (los && allow || (hiding || snipe) && self.prm.lcov > 0.0) && !matches!(tg.kind, TargetKind::Crate) {
                fire[li] = true;
                aim = direct;
                if self.prm.chg_gun > 0.0 && gun_live {
                    if let Some(ga) = gun_aim {
                        aim = ga;
                    }
                }
            }
        }

        // Gun.
        if let (Some(gi), Some(ga)) = (gun, gun_aim) {
            let gw = w(gi);
            let range = gw.stat("range", 230.0);
            let ammo = gw.ammo.unwrap_or(1.0);
            let reloading = gw.reload.unwrap_or(0.0) > 0.0;
            let ok_aim = if laser.map(|li| fire[li]).unwrap_or(false) {
                aligned(ga) || aligned(direct) && (d < 60.0 || self.prm.chg_gun > 1.5)
            } else {
                aligned(ga)
            };
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
            let zapper = self.prm.mlz > 0.0 && dh < 70.0 && self.tracks.get(&tg.id).map(|t| t.weapons.iter().any(|w| w == "lightning")).unwrap_or(false);
            let clus = self.prm.mclus > 0.0
                && matches!(tg.kind, TargetKind::Mech)
                && (hdist(tg.vel, [0.0, 0.0, 0.0]) < 2.5
                    || self.tracks.iter().any(|(id, t)| *id != tg.id && obs.t - t.seen_t < 0.5 && hdist(t.pos, tg.pos) < 12.0));
            if mw.cooldown <= 0.05 && dh > 22.0 && dh < 225.0 && (!los || gun_busy || zapper || clus) && !laser_busy && allow {
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
