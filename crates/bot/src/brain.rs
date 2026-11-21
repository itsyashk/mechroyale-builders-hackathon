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
    dodge_goal: Option<[f32; 2]>,
    dodge_check_t: f32,
    dodges: u32,
    last_ttk: f32,
    escape_until: f32,
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

fn weapon_prio(kind: &str) -> f32 {
    match kind {
        "laser" => 4.0,
        "lightning" => 3.0,
        "gun" => 2.0,
        "mortar" => 1.5,
        _ => 0.0,
    }
}

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
            escape_until: -1.0,
            escape_goal: [0.0, 0.0],
            last_ttd: 0.0,
            debug: std::env::var("MECHBOT_DEBUG").is_ok(),
            debug_every: std::env::var("MECHBOT_DEBUG").ok().and_then(|v| v.parse().ok()).unwrap_or(10).max(1),
        })
    }

    pub fn recover(&mut self) {
        self.path.clear();
        self.cover_goal = None;
        self.roam_goal = None;
    }

    pub fn log_end(&self, raw: &str) {
        eprintln!("end: {raw}");
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
            if dist2(end, goal) > 10.0 && dist2(pos, goal) > 10.0 && dist2(end, pos) < 12.0 {
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
            for (i, c) in offer.choices.iter().enumerate() {
                let r = UPGRADE_RANK.iter().position(|u| *u == c.id).unwrap_or(UPGRADE_RANK.len());
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
        let mut best_t: Option<(f32, &Mech, bool)> = None;
        for m in &enemies {
            let scale = 1.0 + 0.05 * m.cubes.unwrap_or(0) as f32;
            let d = len3(sub(m.pos, pos));
            let shootable = self.world.los(muzzle, [m.pos[0], m.pos[1], m.pos[2] + 4.5 * scale]);
            let hp = m.hp.unwrap_or(1400.0);
            let mut s = d + hp * 0.04;
            if shootable {
                s -= 60.0;
            }
            if attackers.contains(&m.id) {
                s -= 50.0;
            }
            if Some(m.id) == self.target_id {
                s -= 30.0;
            }
            if best_t.map(|b| s < b.0).unwrap_or(true) {
                best_t = Some((s, m, shootable));
            }
        }
        let target_mech = best_t.map(|b| (b.1.clone(), b.2));
        self.target_id = target_mech.as_ref().map(|t| t.0.id);

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
            let losing = ttk > ttd * 0.85 && ttk > 2.5 && (hp_frac < 0.6 || n_shooting >= 2);
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
        } else if target_mech.is_some() && (self.retreating || sniper) {
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
                let k0 = weapon_prio(slot_kind(0));
                let k1 = weapon_prio(slot_kind(1));
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
                let pr = weapon_prio(kind);
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
            if mode == Mode::Roam && hp_frac > 0.3 {
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
                        .filter(|c| hdist(c.pos, pos) < 70.0 && dist2(xy(c.pos), gc) < gr)
                        .min_by(|a, b| hdist(a.pos, pos).partial_cmp(&hdist(b.pos, pos)).unwrap())
                    {
                        mode = Mode::Farm;
                        aux_target = Some(Target { id: c.id, pos: c.pos, vel: [0.0; 3], kind: TargetKind::Crate, scale: 1.0 });
                        goal = Some(xy(c.pos));
                    } else if hp_frac > 0.6 {
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

        // ---- movement per mode
        let tgt: Option<Target> = target_mech
            .as_ref()
            .map(|(m, _)| Target { id: m.id, pos: m.pos, vel: m.vel, kind: TargetKind::Mech, scale: 1.0 + 0.05 * m.cubes.unwrap_or(0) as f32 })
            .or(aux_target);

        if obs.t > self.strafe_switch_t {
            self.strafe_sign = if self.rng.f() < 0.5 { -1.0 } else { 1.0 };
            self.strafe_switch_t = obs.t + self.rng.range(0.45, 1.3);
        }

        match mode {
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
                    (10.0, 32.0)
                } else if have_laser {
                    if m.has("lightning") {
                        (55.0, 150.0)
                    } else {
                        (45.0, 140.0)
                    }
                } else if m.has("lightning") {
                    (56.0, 95.0)
                } else {
                    (18.0, 55.0)
                };
                if !shootable {
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
                    let mut cand = unit2([perp[0] + to[0] * radial * 1.2, perp[1] + to[1] * radial * 1.2]);
                    let ok = |c: [f32; 2], nav: &Nav| nav.walkable_at(p2[0] + c[0] * 5.0, p2[1] + c[1] * 5.0);
                    if !ok(cand, &self.nav) {
                        self.strafe_sign = -self.strafe_sign;
                        let perp = [-perp[0], -perp[1]];
                        cand = unit2([perp[0] + to[0] * radial * 1.2, perp[1] + to[1] * radial * 1.2]);
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

        // Laser dodge: an enemy charging at us with a clear line -> duck into nearby cover.
        if matches!(mode, Mode::Fight | Mode::Farm | Mode::Roam | Mode::Loot) {
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
        if have_light && mode == Mode::Fight && tgt_d < 50.0 {
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
