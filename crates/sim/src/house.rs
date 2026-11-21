//! In-process "house" bots emulating the official practice opponents: a squad that shares
//! vision, never fights itself, and hunts the external bot(s) weakest-tier first.
//! Without a squad (FFA fillers) the same brain fights anyone it sees.

use crate::cfg::{WK, WS};
use crate::game::*;
use crate::util::*;
use arena::Nav;

pub const SQUAD_TEAM: u8 = 1;
/// The first hunter is sent at this match time (s).
pub const FIRST_HUNT_S_V1: f32 = 45.0;

/// House model (`SIM_HOUSE`): v1 (default; its score levels sit ~0.6-0.8 below the real board for
/// both the kit example bot and ours), v2 (accurate long-range laser bots hunting from t=0, as in
/// kit/check/stream.jsonl.gz, which is not a practice match: house bots die there by 25 s; far too
/// lethal: ~1.3 for us), v3 (v1 with aim error capped past 40 m, retaliation at any range, hard
/// jinking), v4 (calibrated to real practice measurements: hit rate ~90% <40 m, ~61% at 80-120 m,
/// ~36% at 120-160 m; standoff 55-95 m; a hunter from ~5 s, +1 per fallen mate, all late; shared,
/// near-exact intel; lasers early; attacking passive bots draws aggro). See `V4` for its knobs.
pub fn house_v() -> u8 {
    static V: std::sync::OnceLock<u8> = std::sync::OnceLock::new();
    *V.get_or_init(|| match std::env::var("SIM_HOUSE").as_deref() {
        Ok("v1") | Ok("1") => 1,
        Ok("v2") | Ok("2") => 2,
        Ok("v3") | Ok("3") => 3,
        Ok("v4") | Ok("4") => 4,
        _ => 1,
    })
}

pub fn tiers() -> &'static [Tier; 4] {
    match house_v() {
        1 => &TIERS_V1,
        2 => &TIERS_V2,
        4 => &TIERS_V4,
        _ => &TIERS_V3,
    }
}

/// Model 3: model 1's squad, schedule and engagement, but no free lunch at range: aim
/// error stops growing past 40 m (the recorded real bot hit a jinking target at ~150 m) and bots
/// fight back against an attacker at any range.
pub static TIERS_V3: std::sync::LazyLock<[Tier; 4]> = std::sync::LazyLock::new(|| {
    let mut t = TIERS_V1.clone();
    for x in t.iter_mut() {
        x.retaliate_range = 230.0;
        // the recorded bot reversed its strafe every ~0.3-0.5 s
        x.strafe = x.strafe.max(0.75);
        x.strafe_period = (0.25, 0.6);
    }
    t
});

/// Calibration knobs for model 2 (env): SIM_FIRST_HUNT (s), SIM_SPAWN_LASER (0/1), SIM_ENGAGE_MUL.
pub fn env_f(k: &str, d: f32) -> f32 {
    std::env::var(k).ok().and_then(|v| v.parse().ok()).unwrap_or(d)
}

pub fn first_hunt_s() -> f32 {
    static V: std::sync::OnceLock<f32> = std::sync::OnceLock::new();
    *V.get_or_init(|| match house_v() {
        1 | 3 => FIRST_HUNT_S_V1,
        4 => v4().first_hunt,
        _ => env_f("SIM_FIRST_HUNT", 0.0),
    })
}

pub fn engage_mul() -> f32 {
    static V: std::sync::OnceLock<f32> = std::sync::OnceLock::new();
    *V.get_or_init(|| if house_v() == 2 { env_f("SIM_ENGAGE_MUL", 1.0) } else { 1.0 })
}

pub fn spawn_laser() -> bool {
    house_v() == 2 && env_f("SIM_SPAWN_LASER", 1.0) > 0.5
}

/// Model 4 knobs (env `SIM_V4_*`, defaults = the calibrated values).
#[derive(Clone, Debug)]
pub struct V4 {
    /// First hunter is sent at this time (s). `SIM_V4_FIRST`
    pub first_hunt: f32,
    /// Everyone hunts from this time (s). `SIM_V4_ALL`
    pub all_hunt: f32,
    /// Extra hunters per minute after the first minute (0 = only per fallen mate). `SIM_V4_PERMIN`
    pub per_min: f32,
    /// Aim error (1 sigma, metres at the target) = sqrt(c0^2 + (k d^2)^2) x tier mult. `SIM_V4_C0`, `SIM_V4_K`
    pub c0: f32,
    pub k: f32,
    /// Laser aim error multiplier (hitscan, aimed while charging). `SIM_V4_LASERMUL`
    pub laser_mul: f32,
    /// Standoff band (m). `SIM_V4_LO`, `SIM_V4_HI`
    pub lo: f32,
    pub hi: f32,
    /// A squad bot hurt by us hunts us for this long (s); mates within `alert_r` (m) join. `SIM_V4_AGGRO_S`, `SIM_V4_ALERT_R`
    pub aggro_s: f32,
    pub alert_r: f32,
    /// Squad intel noise radius (m). `SIM_V4_INTEL_NOISE`
    pub intel_noise: f32,
    /// House bots of tier >= this spawn with gun + laser (4 = none). `SIM_V4_LASER_TIER`
    pub laser_tier: usize,
    /// Non-hunter engage range multiplier. `SIM_V4_ENGAGE_MUL`
    pub engage_mul: f32,
}

pub fn v4() -> &'static V4 {
    static V: std::sync::OnceLock<V4> = std::sync::OnceLock::new();
    V.get_or_init(|| V4 {
        first_hunt: env_f("SIM_V4_FIRST", 5.0),
        all_hunt: env_f("SIM_V4_ALL", 300.0),
        per_min: env_f("SIM_V4_PERMIN", 0.0),
        c0: env_f("SIM_V4_C0", 1.4),
        k: env_f("SIM_V4_K", 2.9e-4),
        laser_mul: env_f("SIM_V4_LASERMUL", 0.35),
        lo: env_f("SIM_V4_LO", 55.0),
        hi: env_f("SIM_V4_HI", 95.0),
        aggro_s: env_f("SIM_V4_AGGRO_S", 30.0),
        alert_r: env_f("SIM_V4_ALERT_R", 80.0),
        intel_noise: env_f("SIM_V4_INTEL_NOISE", 4.0),
        laser_tier: env_f("SIM_V4_LASER_TIER", 0.0) as usize,
        engage_mul: env_f("SIM_V4_ENGAGE_MUL", 1.0),
    })
}

/// Lasers at spawn for this tier?
pub fn spawn_laser_tier(tier: usize) -> bool {
    spawn_laser() || (house_v() == 4 && tier >= v4().laser_tier)
}

/// Model 4: model 1's squad brain with measured aim (`aim_sigma` is a unitless multiplier of
/// `V4`'s range-dependent metre error), sharp lead, recorded jinking, short intel refresh.
pub static TIERS_V4: std::sync::LazyLock<[Tier; 4]> = std::sync::LazyLock::new(|| {
    let mut t = TIERS_V1.clone();
    let mult = [1.25, 1.1, 0.9, 0.75];
    let react = [0.35, 0.28, 0.2, 0.12];
    for (i, x) in t.iter_mut().enumerate() {
        x.aim_sigma = mult[i];
        x.aim_tau = 0.4;
        x.reaction_s = react[i];
        x.lead = 1.0;
        x.track_rate = x.track_rate.max(3.0);
        x.fire_tol = 1.0;
        x.retaliate_range = 260.0;
        x.strafe = x.strafe.max(0.75);
        x.strafe_period = (0.25, 0.6);
        x.laser_release = 0.95;
        x.intel_s = 1.0;
        x.engage_range *= v4().engage_mul;
    }
    t
});

/// Per-tier behaviour knobs. Tune here.
#[derive(Clone, Debug)]
pub struct Tier {
    pub name: &'static str,
    /// Std-dev of the (slowly drifting) aim error, radians.
    pub aim_sigma: f32,
    /// Correlation time of the aim error, seconds.
    pub aim_tau: f32,
    /// Perception delay: the bot aims at where the target was this long ago (s), and waits
    /// this long after first seeing a target before pulling the trigger.
    pub reaction_s: f32,
    /// 0..1: how much of the target's velocity it uses to lead shots / undo its delay.
    pub lead: f32,
    /// Max rate (rad/s) at which it moves its commanded aim (server turn_rate still caps).
    pub track_rate: f32,
    /// Fires when the torso is within `fire_tol` target radii of its aim solution.
    pub fire_tol: f32,
    /// 0..1 weight of sideways strafing while fighting.
    pub strafe: f32,
    /// Seconds between strafe direction changes (min, max).
    pub strafe_period: (f32, f32),
    /// Dodge jumps per second while fighting.
    pub jump_rate: f32,
    /// Non-hunters engage a target they see within this range (m).
    pub engage_range: f32,
    /// Without a squad (FFA fillers) they engage anything they see within this range.
    pub ffa_engage_range: f32,
    /// Laser released at this fraction of full charge.
    pub laser_release: f32,
    /// Lobs mortar at targets seen only by squad mates.
    pub mortar_lob: bool,
    /// Detours up to this far for weapon pickups (m).
    pub loot_radius: f32,
    /// Hunts NPC drones for cubes when idle.
    pub farm_npc: bool,
    /// Goes for supply drops.
    pub drops: bool,
    /// Weapon preference for the free slot (best first).
    pub weapon_pref: [WK; 3],
    /// Upgrade preference (ids, best first); anything else falls back to choice 0.
    pub upgrade_pref: &'static [&'static str],
    /// Hunters get a fresh (noisy) fix on their quarry every this many seconds.
    pub intel_s: f32,
    /// Non-hunters fight back against an attacker within this range (m).
    pub retaliate_range: f32,
    /// Backs off to regenerate below this HP fraction (0 = never).
    pub retreat_hp: f32,
    /// Boosts while strafing in a fight.
    pub combat_boost: bool,
}

/// Original guess (house bots weak, short-range, hunt from 45 s). `SIM_HOUSE=v1`.
pub const TIERS_V1: [Tier; 4] = [
    Tier {
        name: "easy",
        aim_sigma: 0.11,
        aim_tau: 0.6,
        reaction_s: 0.9,
        lead: 0.0,
        track_rate: 1.3,
        fire_tol: 2.0,
        strafe: 0.15,
        strafe_period: (1.5, 3.5),
        jump_rate: 0.0,
        engage_range: 25.0,
        ffa_engage_range: 70.0,
        laser_release: 0.45,
        mortar_lob: false,
        loot_radius: 40.0,
        farm_npc: false,
        drops: false,
        weapon_pref: [WK::Mortar, WK::Laser, WK::Lightning],
        upgrade_pref: &[],
        intel_s: 6.0,
        retaliate_range: 40.0,
        retreat_hp: 0.0,
        combat_boost: false,
    },
    Tier {
        name: "medium",
        aim_sigma: 0.04,
        aim_tau: 0.6,
        reaction_s: 0.35,
        lead: 0.5,
        track_rate: 2.6,
        fire_tol: 1.3,
        strafe: 0.45,
        strafe_period: (1.0, 2.5),
        jump_rate: 0.05,
        engage_range: 30.0,
        ffa_engage_range: 100.0,
        laser_release: 0.75,
        mortar_lob: false,
        loot_radius: 70.0,
        farm_npc: false,
        drops: false,
        weapon_pref: [WK::Laser, WK::Lightning, WK::Mortar],
        upgrade_pref: &["armor", "plating", "gun_rate", "gun_damage"],
        intel_s: 4.0,
        retaliate_range: 45.0,
        retreat_hp: 0.0,
        combat_boost: false,
    },
    Tier {
        name: "hard",
        aim_sigma: 0.014,
        aim_tau: 0.5,
        reaction_s: 0.22,
        lead: 0.85,
        track_rate: 3.6,
        fire_tol: 1.0,
        strafe: 0.7,
        strafe_period: (0.7, 1.8),
        jump_rate: 0.12,
        engage_range: 35.0,
        ffa_engage_range: 140.0,
        laser_release: 0.95,
        mortar_lob: true,
        loot_radius: 110.0,
        farm_npc: true,
        drops: true,
        weapon_pref: [WK::Laser, WK::Mortar, WK::Lightning],
        upgrade_pref: &["armor", "plating", "gun_rate", "laser_damage", "gun_damage", "gyros", "nanites"],
        intel_s: 3.0,
        retaliate_range: 50.0,
        retreat_hp: 0.3,
        combat_boost: true,
    },
    Tier {
        name: "expert",
        aim_sigma: 0.006,
        aim_tau: 0.5,
        reaction_s: 0.12,
        lead: 1.0,
        track_rate: 6.0,
        fire_tol: 0.85,
        strafe: 0.9,
        strafe_period: (0.45, 1.3),
        jump_rate: 0.2,
        engage_range: 40.0,
        ffa_engage_range: 180.0,
        laser_release: 1.0,
        mortar_lob: true,
        loot_radius: 140.0,
        farm_npc: true,
        drops: true,
        weapon_pref: [WK::Laser, WK::Lightning, WK::Mortar],
        upgrade_pref: &["armor", "gun_rate", "plating", "laser_damage", "laser_charge", "gun_damage", "gyros", "nanites", "servos"],
        intel_s: 2.0,
        retaliate_range: 55.0,
        retreat_hp: 0.35,
        combat_boost: true,
    },
];

/// Calibrated to the recorded (non-practice) real match.
pub const TIERS_V2: [Tier; 4] = [
    Tier {
        name: "easy",
        aim_sigma: 0.02,
        aim_tau: 0.6,
        reaction_s: 0.4,
        lead: 0.85,
        track_rate: 3.0,
        fire_tol: 1.2,
        strafe: 0.7,
        strafe_period: (0.3, 0.7),
        jump_rate: 0.0,
        engage_range: 45.0,
        ffa_engage_range: 70.0,
        laser_release: 0.95,
        mortar_lob: false,
        loot_radius: 90.0,
        farm_npc: false,
        drops: false,
        weapon_pref: [WK::Laser, WK::Mortar, WK::Lightning],
        upgrade_pref: &[],
        intel_s: 3.0,
        retaliate_range: 120.0,
        retreat_hp: 0.0,
        combat_boost: false,
    },
    Tier {
        name: "medium",
        aim_sigma: 0.014,
        aim_tau: 0.6,
        reaction_s: 0.3,
        lead: 0.9,
        track_rate: 3.5,
        fire_tol: 1.1,
        strafe: 0.8,
        strafe_period: (0.3, 0.6),
        jump_rate: 0.05,
        engage_range: 50.0,
        ffa_engage_range: 100.0,
        laser_release: 1.0,
        mortar_lob: false,
        loot_radius: 110.0,
        farm_npc: false,
        drops: false,
        weapon_pref: [WK::Laser, WK::Lightning, WK::Mortar],
        upgrade_pref: &["armor", "plating", "gun_rate", "gun_damage"],
        intel_s: 2.5,
        retaliate_range: 120.0,
        retreat_hp: 0.0,
        combat_boost: false,
    },
    Tier {
        name: "hard",
        aim_sigma: 0.01,
        aim_tau: 0.5,
        reaction_s: 0.2,
        lead: 1.0,
        track_rate: 4.0,
        fire_tol: 1.0,
        strafe: 0.85,
        strafe_period: (0.25, 0.55),
        jump_rate: 0.12,
        engage_range: 55.0,
        ffa_engage_range: 50.0,
        laser_release: 1.0,
        mortar_lob: true,
        loot_radius: 130.0,
        farm_npc: true,
        drops: true,
        weapon_pref: [WK::Laser, WK::Mortar, WK::Lightning],
        upgrade_pref: &["armor", "plating", "gun_rate", "laser_damage", "gun_damage", "gyros", "nanites"],
        intel_s: 2.0,
        retaliate_range: 120.0,
        retreat_hp: 0.3,
        combat_boost: true,
    },
    Tier {
        name: "expert",
        aim_sigma: 0.006,
        aim_tau: 0.5,
        reaction_s: 0.12,
        lead: 1.0,
        track_rate: 6.0,
        fire_tol: 0.9,
        strafe: 0.9,
        strafe_period: (0.25, 0.5),
        jump_rate: 0.2,
        engage_range: 60.0,
        ffa_engage_range: 60.0,
        laser_release: 1.0,
        mortar_lob: true,
        loot_radius: 150.0,
        farm_npc: true,
        drops: true,
        weapon_pref: [WK::Laser, WK::Lightning, WK::Mortar],
        upgrade_pref: &["armor", "gun_rate", "plating", "laser_damage", "laser_charge", "gun_damage", "gyros", "nanites", "servos"],
        intel_s: 1.5,
        retaliate_range: 120.0,
        retreat_hp: 0.35,
        combat_boost: true,
    },
];

/// The practice line-up: 4 easy, 4 medium, 4 hard, 3 expert.
pub const PRACTICE_TIERS: [usize; 15] = [0, 0, 0, 0, 1, 1, 1, 1, 2, 2, 2, 2, 3, 3, 3];

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum TRef {
    Mech(usize),
    Npc(usize),
    Crate(usize),
}

pub struct Brain {
    pub tier: usize,
    pub squad: bool,
    rng: Rng,
    path: Vec<[f32; 2]>,
    path_goal: [f32; 2],
    path_t: f32,
    wander: Option<[f32; 2]>,
    wander_until: f32,
    strafe_sign: f32,
    strafe_until: f32,
    err: [f32; 2],
    cur: Option<TRef>,
    first_seen: f32,
    last_vis: f32,
    stuck_pos: [f32; 2],
    stuck_t: f32,
    detour: Option<([f32; 2], f32)>,
    aim: [f32; 2],
    seen: Vec<f32>,
    mortar_mode: bool,
    intel: Option<(V3, f32)>,
}

pub struct Squad {
    pub hunter: Vec<bool>,
    pub seen: Vec<Option<(V3, V3, f32)>>,
    pub vis: Vec<Vec<bool>>,
    pub intel: Vec<Option<(V3, f32)>>,
    intel_t: f32,
    pub total: usize,
    /// v4: squad member hunts until this time after being hurt by us (or a nearby mate was).
    pub aggro: Vec<f32>,
    rng: Rng,
}

impl Squad {
    pub fn new(n: usize, total: usize, seed: u64) -> Squad {
        Squad { hunter: vec![false; n], seen: vec![None; n], vis: vec![vec![false; n]; n], intel: vec![None; n], intel_t: -100.0, total, aggro: vec![-100.0; n], rng: Rng::new(seed) }
    }

    pub fn update(&mut self, g: &Game, brains: &[Option<Brain>]) {
        let t = g.t;
        let n = g.mechs.len();
        let members: Vec<usize> = (0..n).filter(|&i| g.mechs[i].alive && brains[i].as_ref().map(|b| b.squad).unwrap_or(false)).collect();
        let targets: Vec<usize> = (0..n).filter(|&j| g.mechs[j].alive && g.mechs[j].team != Some(SQUAD_TEAM)).collect();
        for &m in &members {
            for &j in &targets {
                let d = dist(g.mechs[m].pos, g.mechs[j].pos);
                let v = d < 260.0 && g.sees_mech(m, j);
                self.vis[m][j] = v;
                if v {
                    self.seen[j] = Some((g.mechs[j].pos, g.mechs[j].vel, t));
                }
            }
        }
        if t - self.intel_t >= 1.0 {
            self.intel_t = t;
            for &j in &targets {
                let o = self.rng.in_disc(if house_v() == 4 { v4().intel_noise } else { 14.0 });
                let p = g.mechs[j].pos;
                self.intel[j] = Some(([p[0] + o[0], p[1] + o[1], p[2]], t));
            }
        }
        // Hunter schedule: one early, +1 per fallen squad mate, +1 per minute after the
        // first, everyone after 5 minutes or once only 4 remain. Weakest tier first.
        let alive = members.len();
        let mut k = if t >= first_hunt_s() { 1 } else { 0 } + self.total.saturating_sub(alive);
        let v4m = house_v() == 4;
        if v4m {
            let c = v4();
            if t > 60.0 {
                k += ((t - 60.0) / 60.0 * c.per_min) as usize;
            }
            if t >= c.all_hunt || alive <= 4 {
                k = alive;
            }
            // Aggro: a squad bot we hurt (and its mates nearby) hunts us for a while.
            for &m in &members {
                let mm = &g.mechs[m];
                if let Some(Src::Mech(a)) = mm.last_attacker {
                    if t - mm.last_damaged_t < 0.15 && g.mechs[a].team != Some(SQUAD_TEAM) {
                        for &o in &members {
                            if o == m || (c.alert_r > 0.0 && dist(g.mechs[o].pos, mm.pos) < c.alert_r) {
                                self.aggro[o] = t + c.aggro_s;
                            }
                        }
                    }
                }
            }
        } else {
            if t > 60.0 {
                k += ((t - 60.0) / 60.0) as usize;
            }
            if t >= 300.0 || alive <= 4 {
                k = alive;
            }
        }
        let mut order = members.clone();
        order.sort_by_key(|&i| (brains[i].as_ref().map(|b| b.tier).unwrap_or(0), i));
        for h in self.hunter.iter_mut() {
            *h = false;
        }
        for &i in order.iter().take(k) {
            self.hunter[i] = true;
        }
        if v4m {
            for &i in &members {
                if self.aggro[i] > t {
                    self.hunter[i] = true;
                }
            }
        }
    }
}

struct TV {
    r: TRef,
    pos: V3,
    vel: V3,
    rad: f32,
    h: f32,
    vis: bool,
    d: f32,
}

impl Brain {
    pub fn new(tier: usize, squad: bool, seed: u64, n_mechs: usize) -> Brain {
        Brain {
            tier,
            squad,
            rng: Rng::new(seed),
            path: vec![],
            path_goal: [1e9, 1e9],
            path_t: -100.0,
            wander: None,
            wander_until: 0.0,
            strafe_sign: 1.0,
            strafe_until: 0.0,
            err: [0.0, 0.0],
            cur: None,
            first_seen: 0.0,
            last_vis: -100.0,
            stuck_pos: [0.0, 0.0],
            stuck_t: 0.0,
            detour: None,
            aim: [0.0, 0.0],
            seen: vec![-100.0; n_mechs],
            mortar_mode: false,
            intel: None,
        }
    }

    fn go(&mut self, g: &Game, me: usize, nav: &mut Nav, goal: [f32; 2]) -> [f32; 2] {
        let p = g.mechs[me].pos;
        let t = g.t;
        if let Some((d, until)) = self.detour {
            if t < until && dist2d(&p, &d) > 2.0 {
                return unit2([d[0] - p[0], d[1] - p[1]]);
            }
            self.detour = None;
        }
        if dist2d(&goal, &p) < 2.5 {
            return [0.0, 0.0];
        }
        if self.path.is_empty() || dist2d(&goal, &self.path_goal) > 10.0 || t - self.path_t > 4.0 {
            self.path = nav.path([p[0], p[1]], goal, 40_000);
            self.path_goal = goal;
            self.path_t = t;
        }
        while !self.path.is_empty() && dist2d(&self.path[0], &p) < 3.0 {
            self.path.remove(0);
        }
        let nx = self.path.first().copied().unwrap_or(goal);
        unit2([nx[0] - p[0], nx[1] - p[1]])
    }

    fn tview(&self, g: &Game, r: TRef, vis: bool, delay: f32, lead: f32) -> TV {
        match r {
            TRef::Mech(j) => {
                let m = &g.mechs[j];
                let k = ((delay / DT).round() as usize).min(m.hist.len().saturating_sub(1));
                let (pp, pv) = m.hist[m.hist.len() - 1 - k];
                let est = add(pp, scale(pv, delay * lead));
                TV { r, pos: est, vel: pv, rad: m.ms.radius, h: m.ms.height, vis, d: 0.0 }
            }
            TRef::Npc(k) => {
                let n = &g.npcs[k];
                TV { r, pos: n.pos, vel: n.vel, rad: g.cfg.npc("radius"), h: g.cfg.npc("height"), vis, d: 0.0 }
            }
            TRef::Crate(k) => {
                let c = &g.crates[k];
                TV { r, pos: c.pos, vel: [0.0; 3], rad: g.cfg.crate_radius, h: g.cfg.crate_height, vis, d: 0.0 }
            }
        }
    }

    pub fn decide(&mut self, g: &Game, me: usize, squad: &Squad, nav: &mut Nav) -> Cmd {
        let mut tier = tiers()[self.tier].clone();
        tier.engage_range *= engage_mul();
        let t = g.t;
        let m = &g.mechs[me];
        let pos = m.pos;
        let mut cmd = Cmd::default();
        // Upgrades.
        if let Some(o) = &m.offer {
            let mut pick = 0;
            'outer: for want in tier.upgrade_pref {
                for (k, &u) in o.choices.iter().enumerate() {
                    if g.cfg.upgrades[u].id == *want {
                        pick = k;
                        break 'outer;
                    }
                }
            }
            if !tier.upgrade_pref.is_empty() || t >= o.expires - 1.0 {
                cmd.pick = Some(pick as i64);
            }
        }
        // Aim error drift.
        let rho = (-0.1 / tier.aim_tau).exp();
        let q = (1.0 - rho * rho).sqrt() * tier.aim_sigma;
        self.err[0] = self.err[0] * rho + q * self.rng.normal();
        self.err[1] = self.err[1] * rho + q * self.rng.normal() * 0.6;

        // Zone.
        let z = g.zone_at(t);
        let (sc, sr) = if t > z.shrink_start - 25.0 { (z.next_center, z.next_radius) } else { (z.center, z.radius) };
        let margin = (sr * 0.2).min(30.0);
        let dzc = dist2d(&pos, &sc);
        let outside_safe = dzc > (sr - margin).max(0.0);
        let in_safe = |p: &[f32]| dist2d(p, &sc) <= (sr - margin * 0.5).max(3.0);

        // ---- choose a target
        let hunter = self.squad && squad.hunter.get(me).copied().unwrap_or(false);
        let attacked_recently = t - m.last_damaged_t < 5.0;
        if hunter {
            let stale = self.intel.map(|i| t - i.1 >= tier.intel_s).unwrap_or(true);
            if stale {
                // nearest quarry's noisy fix from the squad's intel feed
                self.intel = (0..g.mechs.len())
                    .filter(|&j| g.mechs[j].alive && g.hostile(me, j))
                    .filter_map(|j| squad.intel[j].map(|(p, _)| (p, t)))
                    .min_by(|a, b| dist(a.0, pos).partial_cmp(&dist(b.0, pos)).unwrap());
            }
        } else {
            self.intel = None;
        }
        let mut best: Option<(f32, TRef, bool)> = None; // (score, ref, own-visible)
        let n = g.mechs.len();
        for j in 0..n {
            if j == me || !g.mechs[j].alive || !g.hostile(me, j) {
                continue;
            }
            let o = &g.mechs[j];
            let d = dist(o.pos, pos);
            let vis = if self.squad { squad.vis[me][j] } else { d < 240.0 && g.sees_mech(me, j) };
            if vis {
                self.seen[j] = t;
            }
            let engage_r = if self.squad { tier.engage_range } else { tier.ffa_engage_range };
            let attacker = matches!(m.last_attacker, Some(Src::Mech(a)) if a == j) && attacked_recently && (hunter || !self.squad || d < tier.retaliate_range);
            let known_squad = self.squad && squad.seen[j].map(|s| t - s.2 < 4.0).unwrap_or(false);
            let remembered = t - self.seen[j] < 4.0;
            let engage = vis && d < engage_r || attacker || (hunter && (vis || known_squad || squad.intel[j].is_some())) || (!self.squad && remembered && d < engage_r);
            if !engage {
                continue;
            }
            let score = d + o.hp * 0.03 - if vis { 40.0 } else { 0.0 } - if attacker { 30.0 } else { 0.0 };
            if best.map(|b| score < b.0).unwrap_or(true) {
                best = Some((score, TRef::Mech(j), vis));
            }
        }
        // NPCs: fight back when attacked by one, or farm them.
        if best.is_none() {
            if let Some(Src::Npc(k)) = m.last_attacker {
                if attacked_recently && g.npcs[k].alive {
                    best = Some((0.0, TRef::Npc(k), true));
                }
            }
        }

        let mut tv: Option<TV> = None;
        let mut want_range = 0.0f32;
        let has = |k: WK| m.has(k);
        if let Some((_, r, vis)) = best {
            if self.cur != Some(r) || t - self.last_vis > 1.2 {
                if vis {
                    self.first_seen = t;
                }
            }
            if vis {
                self.last_vis = t;
            }
            self.cur = Some(r);
            let mut v = self.tview(g, r, vis, tier.reaction_s, tier.lead);
            if let TRef::Mech(j) = r {
                if !vis {
                    // Use squad sighting or hunter intel.
                    if let Some((p, vel, _)) = squad.seen.get(j).copied().flatten().filter(|s| t - s.2 < 4.0) {
                        v.pos = p;
                        v.vel = vel;
                    } else if let Some((p, _)) = self.intel {
                        v.pos = p;
                        v.vel = [0.0; 3];
                    }
                }
            }
            v.d = dist(v.pos, pos);
            want_range = if house_v() == 4 {
                0.5 * (v4().lo + v4().hi)
            } else if house_v() != 2 {
                let w = if has(WK::Lightning) {
                    24.0
                } else if has(WK::Laser) {
                    95.0
                } else if has(WK::Mortar) {
                    80.0
                } else {
                    55.0
                };
                if self.tier == 0 {
                    w * 1.3
                } else {
                    w
                }
            } else if has(WK::Laser) {
                // the recorded hunter held 90-150 m with gun + laser
                125.0
            } else if has(WK::Lightning) {
                28.0
            } else if has(WK::Mortar) {
                90.0
            } else {
                85.0
            };
            tv = Some(v);
        } else {
            self.cur = None;
        }

        // ---- movement
        let mut mv: [f32; 2];
        let mut boost = false;
        let mut idle_target: Option<TV> = None;
        if let Some(v) = &tv {
            if v.vis {
                let to = unit2([v.pos[0] - pos[0], v.pos[1] - pos[1]]);
                let perp = [-to[1] * self.strafe_sign, to[0] * self.strafe_sign];
                if t >= self.strafe_until {
                    self.strafe_sign = if self.rng.chance(0.5) { 1.0 } else { -1.0 };
                    self.strafe_until = t + self.rng.range(tier.strafe_period.0, tier.strafe_period.1);
                }
                let mut radial = if house_v() == 4 {
                    // hold the standoff band: close in past `hi`, back off inside `lo`
                    let c = v4();
                    if v.d > c.hi {
                        ((v.d - c.hi) / 20.0 + 0.3).min(1.0)
                    } else if v.d < c.lo {
                        -((c.lo - v.d) / 20.0 + 0.3).min(1.0)
                    } else {
                        0.0
                    }
                } else {
                    ((v.d - want_range) / 25.0).clamp(-1.0, 1.0)
                };
                let low = m.hp < m.max_hp * tier.retreat_hp && t < 300.0;
                if low {
                    radial = -1.0;
                }
                let mut d = [to[0] * radial + perp[0] * tier.strafe, to[1] * radial + perp[1] * tier.strafe];
                let probe = [pos[0] + d[0] * 6.0, pos[1] + d[1] * 6.0];
                if !nav.walkable_at(probe[0], probe[1]) {
                    self.strafe_sign = -self.strafe_sign;
                    d = [to[0] * radial - perp[0] * tier.strafe, to[1] * radial - perp[1] * tier.strafe];
                    let probe = [pos[0] + d[0] * 6.0, pos[1] + d[1] * 6.0];
                    if !nav.walkable_at(probe[0], probe[1]) {
                        d = self.go(g, me, nav, [v.pos[0], v.pos[1]]);
                        if radial < 0.0 {
                            d = [-d[0] * 0.3, -d[1] * 0.3];
                        }
                    }
                }
                mv = d;
                if tier.combat_boost && m.energy > 55.0 && (attacked_recently || v.d > want_range * 1.5) {
                    boost = true;
                }
                if tier.jump_rate > 0.0 && m.energy > 45.0 && m.grounded && self.rng.chance(tier.jump_rate * 0.1) {
                    cmd.jump = true;
                }
            } else {
                let mut goal = [v.pos[0], v.pos[1]];
                // Hunters stay inside the zone.
                if !in_safe(&goal) {
                    let dd = unit2([goal[0] - sc[0], goal[1] - sc[1]]);
                    let rr = (sr - margin).max(0.0);
                    goal = [sc[0] + dd[0] * rr, sc[1] + dd[1] * rr];
                }
                mv = self.go(g, me, nav, goal);
                if v.d > 120.0 && m.energy > 70.0 {
                    boost = true;
                }
            }
        } else {
            // Roam: loot, drops, crates, NPCs, wander. Hunters skip most of it.
            let mut goal: Option<[f32; 2]> = None;
            let need = m.slots[1].is_none() || m.slots[0].is_none();
            if need {
                let mut bp: Option<(f32, usize)> = None;
                for (k, p) in g.pickups.iter().enumerate() {
                    if m.has(p.w.kind) {
                        continue;
                    }
                    let d = dist2d(&p.pos, &pos);
                    let pref = tier.weapon_pref.iter().position(|&w| w == p.w.kind).unwrap_or(3) as f32;
                    let sc2 = d + pref * 25.0;
                    if d < tier.loot_radius * if hunter && house_v() != 2 { 0.4 } else { 1.0 } && (p.pos[2] - g.st.w.ground(p.pos[0], p.pos[1])).abs() < 1.5 && in_safe(&p.pos) && bp.map(|b| sc2 < b.0).unwrap_or(true) {
                        bp = Some((sc2, k));
                    }
                }
                if let Some((_, k)) = bp {
                    let p = &g.pickups[k];
                    if dist2d(&p.pos, &pos) < g.cfg.loot_pickup_radius * 0.8 && (p.pos[2] - pos[2]).abs() < 3.5 {
                        cmd.take = Some(p.id);
                    }
                    goal = Some([p.pos[0], p.pos[1]]);
                }
            }
            if goal.is_none() && tier.drops && !hunter {
                for d in &g.drops {
                    let dd = dist2d(&d.pos, &pos);
                    let eta = dd / m.ms.walk;
                    if dd < 260.0 && d.land_t - t < eta + 5.0 && in_safe(&d.pos) {
                        goal = Some([d.pos[0], d.pos[1]]);
                        break;
                    }
                }
            }
            if goal.is_none() && !hunter {
                // Crates in reach.
                let mut bc: Option<(f32, usize)> = None;
                for (k, c) in g.crates.iter().enumerate() {
                    if !c.alive || (c.pos[2] - g.st.w.ground(c.pos[0], c.pos[1])).abs() > 1.5 || !in_safe(&c.pos) {
                        continue;
                    }
                    let d = dist2d(&c.pos, &pos);
                    if d < 90.0 && bc.map(|b| d < b.0).unwrap_or(true) {
                        bc = Some((d, k));
                    }
                }
                if let Some((d, k)) = bc {
                    let mut v = self.tview(g, TRef::Crate(k), true, 0.0, 0.0);
                    v.d = d;
                    let muz = m.muzzle();
                    let c = add(v.pos, [0.0, 0.0, v.h * 0.5]);
                    v.vis = d < 45.0 && g.st.w.los(muz, c);
                    if v.vis && d < 30.0 {
                        goal = Some([pos[0], pos[1]]);
                    } else {
                        goal = Some([v.pos[0], v.pos[1]]);
                    }
                    idle_target = Some(v);
                }
            }
            if goal.is_none() && tier.farm_npc && !hunter && m.hp > m.max_hp * 0.7 {
                let mut bn: Option<(f32, usize)> = None;
                for (k, npc) in g.npcs.iter().enumerate() {
                    let d = dist2d(&npc.pos, &pos);
                    if npc.alive && d < 140.0 && in_safe(&npc.pos) && bn.map(|b| d < b.0).unwrap_or(true) {
                        bn = Some((d, k));
                    }
                }
                if let Some((d, k)) = bn {
                    let mut v = self.tview(g, TRef::Npc(k), true, 0.0, 0.0);
                    v.d = d;
                    let c = add(v.pos, [0.0, 0.0, v.h * 0.5]);
                    v.vis = d < 80.0 && g.st.w.los(m.muzzle(), c);
                    goal = Some(if v.vis && d < 55.0 { [pos[0], pos[1]] } else { [v.pos[0], v.pos[1]] });
                    idle_target = Some(v);
                }
            }
            let goal = match goal {
                Some(gl) => gl,
                None => {
                    let need_new = self.wander.map(|w| dist2d(&w, &pos) < 6.0 || t > self.wander_until || !in_safe(&w)).unwrap_or(true);
                    if need_new {
                        let r = (sr - margin).max(5.0) * 0.95;
                        let o = self.rng.in_disc(r);
                        self.wander = Some([sc[0] + o[0], sc[1] + o[1]]);
                        self.wander_until = t + self.rng.range(15.0, 35.0);
                    }
                    self.wander.unwrap()
                }
            };
            mv = self.go(g, me, nav, goal);
            if hunter && m.energy > 80.0 {
                boost = true;
            }
        }
        // Zone override.
        if outside_safe {
            let d = self.go(g, me, nav, sc);
            if tv.as_ref().map(|v| v.vis).unwrap_or(false) && m.in_zone && dzc < sr {
                mv = unit2([mv[0] * 0.4 + d[0], mv[1] * 0.4 + d[1]]);
            } else {
                mv = d;
            }
            if !m.in_zone && m.energy > 15.0 {
                boost = true;
            }
        }
        // Stuck detection.
        let ml = (mv[0] * mv[0] + mv[1] * mv[1]).sqrt();
        if ml > 0.3 {
            if t - self.stuck_t > 1.5 {
                if dist2d(&self.stuck_pos, &pos) < 1.5 {
                    cmd.jump = true;
                    boost = m.energy > 30.0;
                    let a = self.rng.range(-3.14, 3.14);
                    self.detour = Some(([pos[0] + a.cos() * 12.0, pos[1] + a.sin() * 12.0], t + 1.5));
                    self.path.clear();
                }
                self.stuck_pos = [pos[0], pos[1]];
                self.stuck_t = t;
            }
        } else {
            self.stuck_pos = [pos[0], pos[1]];
            self.stuck_t = t;
        }
        cmd.mv = mv;
        cmd.boost = boost;

        // ---- aiming and firing
        let fight = tv.as_ref().or(idle_target.as_ref());
        let mut want_aim: Option<[f32; 2]> = None;
        let mut fire = [false, false];
        if let Some(v) = fight {
            let can_shoot = v.vis && (t - self.first_seen >= tier.reaction_s || !matches!(v.r, TRef::Mech(_)));
            let center = add(v.pos, [0.0, 0.0, v.h * 0.5]);
            let lead = if matches!(v.r, TRef::Mech(_)) { tier.lead } else { 1.0 };
            let gun_ws = &m.ws[0];
            let gun_sol = intercept(m.muzzle(), center, v.vel, gun_ws.speed, lead);
            let direct = aim_at(m.muzzle(), center);
            let tol = |dd: f32| (v.rad * tier.fire_tol).atan2(dd.max(1.0)) + 0.004;
            let aligned = |sol: [f32; 2]| ang_between(m.yaw, m.pitch, sol[0], sol[1]) < tol(v.d);
            let mut sol = gun_sol;
            // Laser state.
            let ls = m.slot_of(WK::Laser);
            let laser_charge = ls.and_then(|s| m.slots[s].as_ref()).map(|w| if w.charging { w.charge } else { 0.0 }).unwrap_or(0.0);
            let lws: WS = m.ws[2].clone();
            if laser_charge > lws.charge * 0.5 {
                sol = direct;
            }
            // Mortar.
            let msl = m.slot_of(WK::Mortar);
            let mws: WS = m.ws[1].clone();
            let mortar_ready = msl.and_then(|s| m.slots[s].as_ref()).map(|w| w.cooldown <= 0.15).unwrap_or(false);
            let max_r = mws.speed * mws.speed / mws.gravity.max(1.0) * 0.92;
            let lob_ok = v.vis || (tier.mortar_lob && matches!(v.r, TRef::Mech(_)));
            let mortar_sol = if mortar_ready && laser_charge <= 0.0 && v.d > 28.0 && v.d < max_r && lob_ok && matches!(v.r, TRef::Mech(_) | TRef::Npc(_)) {
                let tube = [pos[0], pos[1], pos[2] + m.ms.height + 0.5];
                ballistic(tube, add(v.pos, [0.0, 0.0, 0.8]), v.vel, mws.speed, mws.gravity, lead)
            } else {
                None
            };
            if let Some(ms) = mortar_sol {
                if !v.vis || self.mortar_mode || self.rng.chance(0.5) {
                    self.mortar_mode = true;
                    sol = ms;
                }
            } else {
                self.mortar_mode = false;
            }
            let es = if house_v() == 3 {
                (40.0 / v.d.max(1.0)).min(1.0)
            } else if house_v() == 4 {
                // aim_sigma is a multiplier here; error in metres at the target, as measured
                let c = v4();
                let dd = v.d.max(5.0);
                let em = (c.c0 * c.c0 + (c.k * dd * dd).powi(2)).sqrt();
                let lm = if laser_charge > 0.0 { c.laser_mul } else { 1.0 };
                em / dd * lm
            } else {
                1.0
            };
            let noisy = [sol[0] + self.err[0] * es, sol[1] + self.err[1] * es];
            want_aim = Some(noisy);
            let al = aligned(noisy);
            for s in 0..2 {
                let Some(w) = &m.slots[s] else { continue };
                match w.kind {
                    WK::Gun => {
                        fire[s] = can_shoot && !self.mortar_mode && v.d <= m.ws[0].range * 0.97 && al && w.reload <= 0.0;
                    }
                    WK::Mortar => {
                        fire[s] = self.mortar_mode && al && (can_shoot || (lob_ok && t - self.first_seen > tier.reaction_s));
                    }
                    WK::Laser => {
                        if w.charging {
                            let release = w.charge >= lws.charge * tier.laser_release - 1e-3 && al;
                            let lost = !v.vis && t - self.last_vis > 0.6;
                            fire[s] = !(release || lost);
                        } else {
                            fire[s] = can_shoot && v.d <= lws.range * 0.97 && w.cooldown <= 0.0 && t >= m.short_until && !self.mortar_mode;
                        }
                    }
                    WK::Lightning => {
                        let lw = &m.ws[3];
                        let yd = angdiff(yaw_to(&pos, &center), m.yaw).abs();
                        fire[s] = can_shoot && v.d <= lw.range * 0.95 && yd < lw.cone_deg.to_radians() * 0.7 && m.energy >= lw.energy + 4.0;
                    }
                }
            }
            if self.mortar_mode && fire.iter().enumerate().any(|(s, &f)| f && m.slots[s].as_ref().map(|w| w.kind == WK::Mortar).unwrap_or(false)) {
                self.mortar_mode = false;
            }
        } else {
            self.mortar_mode = false;
            // Keep a laser charge from being wasted.
            for s in 0..2 {
                if let Some(w) = &m.slots[s] {
                    if w.kind == WK::Laser && w.charging {
                        fire[s] = false;
                    }
                }
            }
            if ml > 0.3 {
                want_aim = Some([mv[1].atan2(mv[0]), 0.0]);
            }
        }
        cmd.fire = fire;
        // Reload when idle.
        if fight.is_none() {
            if let Some(w) = &m.slots[0] {
                if w.kind == WK::Gun && w.ammo < (m.ws[0].mag as i32) / 2 && w.reload <= 0.0 {
                    cmd.reload = true;
                }
            }
        }
        // Rate-limited commanded aim (tracking skill).
        if let Some(a) = want_aim {
            if self.aim == [0.0, 0.0] {
                self.aim = [m.yaw, m.pitch];
            }
            let step = tier.track_rate * 0.1;
            let dy = angdiff(a[0], self.aim[0]).clamp(-step, step);
            let dp = (a[1] - self.aim[1]).clamp(-step, step);
            self.aim = [wrap(self.aim[0] + dy), (self.aim[1] + dp).clamp(m.ms.min_pitch, m.ms.max_pitch)];
            cmd.aim = Some(self.aim);
        }
        cmd
    }
}

fn unit2(v: [f32; 2]) -> [f32; 2] {
    let l = (v[0] * v[0] + v[1] * v[1]).sqrt();
    if l < 1e-6 {
        [0.0, 0.0]
    } else {
        [v[0] / l, v[1] / l]
    }
}

fn aim_at(from: V3, to: V3) -> [f32; 2] {
    let d = sub(to, from);
    [d[1].atan2(d[0]), d[2].atan2((d[0] * d[0] + d[1] * d[1]).sqrt())]
}

/// Iterated intercept for a straight projectile.
pub fn intercept(from: V3, tp: V3, tv: V3, speed: f32, lead: f32) -> [f32; 2] {
    let mut p = tp;
    for _ in 0..4 {
        let tf = (dist(from, p) + 3.0) / speed.max(1.0);
        p = add(tp, scale([tv[0], tv[1], tv[2] * 0.3], tf * lead));
    }
    aim_at(from, p)
}

/// Low-arc ballistic solution with lead. Returns (yaw, pitch).
pub fn ballistic(from: V3, tp: V3, tv: V3, v: f32, g: f32, lead: f32) -> Option<[f32; 2]> {
    let mut p = tp;
    let mut out = None;
    for _ in 0..4 {
        let dx = p[0] - from[0];
        let dy = p[1] - from[1];
        let d = (dx * dx + dy * dy).sqrt().max(1.0);
        let h = p[2] - from[2];
        let disc = v.powi(4) - g * (g * d * d + 2.0 * h * v * v);
        if disc < 0.0 {
            return None;
        }
        let th = ((v * v - disc.sqrt()) / (g * d)).atan();
        let tf = d / (v * th.cos()).max(1.0);
        out = Some([dy.atan2(dx), th]);
        p = add(tp, scale([tv[0], tv[1], 0.0], tf * lead));
    }
    out
}
