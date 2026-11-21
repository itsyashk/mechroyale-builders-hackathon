//! The authoritative match engine: 20 Hz ticks, movement, weapons, damage, loot, NPCs,
//! zone, supply drops, upgrades, deaths and placement.

use crate::cfg::*;
use crate::phys::{self, Body};
use crate::trees::TreeGrid;
use crate::util::*;
use arena::{Nav, World};
use serde_json::Value;
use std::collections::VecDeque;
use std::f32::consts::PI;
use std::fmt::Write;
use std::sync::Arc;

pub const DT: f32 = 0.05;
pub const HIST: usize = 32;

/// Data shared by every match (loaded once).
pub struct Statics {
    pub w: World,
    pub trees: TreeGrid,
    pub ground_pts: Vec<[f32; 3]>,
    pub roof_pts: Vec<[f32; 3]>,
    pub map_dir: String,
}

impl Statics {
    pub fn load(dir: &str) -> Result<Statics, String> {
        let w = World::load(dir)?;
        let txt = std::fs::read_to_string(std::path::Path::new(dir).join("v2/world.json")).map_err(|e| e.to_string())?;
        let v: Value = serde_json::from_str(&txt).map_err(|e| e.to_string())?;
        let density = v["rules"]["canopyDensity"].as_f64().unwrap_or(0.35) as f32;
        let block_t = v["rules"]["canopyBlockT"].as_f64().unwrap_or(0.25) as f32;
        let trees = TreeGrid::build(&w, density, block_t);
        let nav = Nav::build(&w);
        let mut ground_pts = Vec::new();
        for c in 0..nav.base_cost.len() {
            if nav.base_cost[c] >= 0.0 && nav.clearance[c] >= 4.0 {
                let p = nav.center(c);
                // Skip cells under a low bridge or overhang.
                let g = w.ground(p[0], p[1]);
                ground_pts.push([p[0], p[1], g]);
            }
        }
        let mut roof_pts = Vec::new();
        let mut j = 2;
        while j + 2 < w.ny {
            let mut i = 2;
            while i + 2 < w.nx {
                let k = j * w.nx + i;
                let (lo0, hi0, lo1, hi1) = (w.lo0[k], w.hi0[k], w.lo1[k], w.hi1[k]);
                let g = w.terrain[k];
                if lo0 <= hi0 && hi0 - g > 3.0 && hi0 - g < 55.0 && (lo1 > hi1 || lo1 > hi0 + 12.0) {
                    let mut flat = true;
                    for (di, dj) in [(-2i32, 0i32), (2, 0), (0, -2), (0, 2), (-2, -2), (2, 2), (-2, 2), (2, -2)] {
                        let kk = (j as i32 + dj) as usize * w.nx + (i as i32 + di) as usize;
                        if !(w.lo0[kk] <= w.hi0[kk] && (w.hi0[kk] - hi0).abs() < 0.3 && (w.lo1[kk] > w.hi1[kk] || w.lo1[kk] > hi0 + 12.0)) {
                            flat = false;
                            break;
                        }
                    }
                    let x = w.min_x + i as f32 * w.step;
                    let y = w.min_y + j as f32 * w.step;
                    if flat && w.in_bounds(x, y, 20.0) {
                        roof_pts.push([x, y, hi0]);
                    }
                }
                i += 3;
            }
            j += 3;
        }
        let abs = std::fs::canonicalize(dir).map(|p| p.to_string_lossy().to_string()).unwrap_or(dir.to_string());
        let abs = abs.strip_prefix(r"\\?\").unwrap_or(&abs).to_string();
        Ok(Statics { w, trees, ground_pts, roof_pts, map_dir: abs })
    }
}

#[derive(Clone, Debug)]
pub struct Weapon {
    pub kind: WK,
    pub cooldown: f32,
    pub ammo: i32,
    pub reload: f32,
    pub charge: f32,
    pub charging: bool,
    pub release_pending: bool,
    pub hold: f32,
}

impl Weapon {
    pub fn new(kind: WK, ws: &WS) -> Weapon {
        Weapon { kind, cooldown: 0.0, ammo: ws.mag as i32, reload: 0.0, charge: 0.0, charging: false, release_pending: false, hold: 0.0 }
    }
}

#[derive(Clone, Default, Debug)]
pub struct Cmd {
    pub mv: [f32; 2],
    pub boost: bool,
    pub jump: bool,
    pub aim: Option<[f32; 2]>,
    pub fire: [bool; 2],
    pub reload: bool,
    pub pick: Option<i64>,
    pub take: Option<i64>,
    pub drop: Option<i64>,
}

impl Cmd {
    pub fn parse(v: &Value) -> Cmd {
        let v2 = |k: &str| -> Option<[f32; 2]> {
            let a = v.get(k)?.as_array()?;
            let x = a.first()?.as_f64()? as f32;
            let y = a.get(1)?.as_f64()? as f32;
            if x.is_finite() && y.is_finite() {
                Some([x, y])
            } else {
                None
            }
        };
        let b = |k: &str| v.get(k).and_then(|x| x.as_bool()).unwrap_or(false);
        let i = |k: &str| v.get(k).and_then(|x| x.as_f64()).filter(|x| x.is_finite()).map(|x| x as i64);
        let mut fire = [false, false];
        if let Some(a) = v.get("fire").and_then(|f| f.as_array()) {
            for (k, x) in a.iter().take(2).enumerate() {
                fire[k] = x.as_bool().unwrap_or(false);
            }
        }
        Cmd { mv: v2("move").unwrap_or([0.0, 0.0]), boost: b("boost"), jump: b("jump"), aim: v2("aim"), fire, reload: b("reload"), pick: i("pick"), take: i("take"), drop: i("drop") }
    }
    /// Continuous parts only (used while a bot misses deadlines).
    pub fn coast(&self) -> Cmd {
        Cmd { mv: self.mv, boost: self.boost, aim: self.aim, ..Default::default() }
    }
}

pub struct Offer {
    pub id: u32,
    pub choices: Vec<usize>,
    pub expires: f32,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Src {
    Mech(usize),
    Npc(usize),
    Zone,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tgt {
    Mech(usize),
    Npc(usize),
    Crate(usize),
}

pub struct Mech {
    pub id: usize,
    pub alive: bool,
    pub team: Option<u8>,
    pub external: bool,
    pub pos: V3,
    pub vel: V3,
    pub yaw: f32,
    pub pitch: f32,
    pub cmd_yaw: f32,
    pub cmd_pitch: f32,
    pub mv: [f32; 2],
    pub boost: bool,
    pub fire: [bool; 2],
    pub jump_req: bool,
    pub hp: f32,
    pub max_hp: f32,
    pub energy: f32,
    pub grounded: bool,
    pub cubes: u32,
    pub upgrades: Vec<String>,
    pub up_count: Vec<u32>,
    pub mech_kv: KV,
    pub ms: MS,
    pub wkv: [KV; 4],
    pub ws: [WS; 4],
    pub slots: [Option<Weapon>; 2],
    pub last_combat: f32,
    pub last_spend: f32,
    pub offer: Option<Offer>,
    pub pending_offers: u32,
    pub next_offer_id: u32,
    pub thresh_idx: usize,
    pub short_until: f32,
    pub step_acc: f32,
    pub jet_acc: f32,
    pub boosting: bool,
    pub jetting: bool,
    pub in_zone: bool,
    pub events: Vec<String>,
    pub zone_acc: f32,
    pub kills: u32,
    pub dmg_dealt: f32,
    pub dmg_taken: f32,
    pub death_t: f32,
    pub place: u32,
    pub cause: &'static str,
    pub killer: Option<usize>,
    pub last_hit_by: Option<Src>,
    pub last_hit_weapon: &'static str,
    pub last_damaged_t: f32,
    pub last_attacker: Option<Src>,
    pub seen_until: Vec<f32>,
    pub last_obs_t: f32,
    pub hist: VecDeque<(V3, V3)>,
    /// Gun shots fired / gun hits on mechs (diagnostics).
    pub shots: u32,
    pub hits: u32,
}

impl Mech {
    pub fn has(&self, k: WK) -> bool {
        self.slots.iter().any(|s| s.as_ref().map(|w| w.kind == k).unwrap_or(false))
    }
    pub fn slot_of(&self, k: WK) -> Option<usize> {
        (0..2).find(|&s| self.slots[s].as_ref().map(|w| w.kind == k).unwrap_or(false))
    }
    pub fn charging(&self) -> bool {
        self.slots.iter().any(|s| s.as_ref().map(|w| w.charging).unwrap_or(false))
    }
    pub fn center(&self) -> V3 {
        [self.pos[0], self.pos[1], self.pos[2] + self.ms.height * 0.5]
    }
    pub fn eye(&self) -> V3 {
        [self.pos[0], self.pos[1], self.pos[2] + self.ms.eye]
    }
    pub fn muzzle(&self) -> V3 {
        [self.pos[0], self.pos[1], self.pos[2] + self.ms.muzzle]
    }
}

pub struct Npc {
    pub id: i64,
    pub alive: bool,
    pub pos: V3,
    pub vel: V3,
    pub yaw: f32,
    pub pitch: f32,
    pub hp: f32,
    pub home: V3,
    pub target: Option<usize>,
    pub cooldown: f32,
    pub wander: [f32; 2],
    pub returning: bool,
    pub think: f32,
}

pub struct Crate {
    pub id: i64,
    pub pos: V3,
    pub hp: f32,
    pub alive: bool,
}

pub struct Cube {
    pub id: i64,
    pub pos: V3,
    pub alive: bool,
}

pub struct Pickup {
    pub id: i64,
    pub pos: V3,
    pub w: Weapon,
    pub alive: bool,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PK {
    Gun,
    Mortar,
    Npc,
}

pub struct Proj {
    pub id: i64,
    pub kind: PK,
    pub owner: Src,
    pub team: Option<u8>,
    pub pos: V3,
    pub vel: V3,
    pub left: f32,
    pub dmg: f32,
    pub radius: f32,
    pub mws: Option<WS>,
    pub scale: f32,
    pub alive: bool,
}

pub struct Sound {
    pub kind: &'static str,
    pub pos: V3,
    pub t: f32,
    pub src: Option<usize>,
}

pub struct Beam {
    pub by: usize,
    pub kind: &'static str,
    pub path: Vec<V3>,
    pub t: f32,
}

pub struct SupplyDrop {
    pub id: i64,
    pub pos: V3,
    pub land_t: f32,
    pub holder: Option<usize>,
    pub progress: f32,
    pub taken: bool,
}

pub struct ZoneNow {
    pub center: [f32; 2],
    pub radius: f32,
    pub next_center: [f32; 2],
    pub next_radius: f32,
    pub shrink_start: f32,
    pub shrink_end: f32,
    pub dps: f32,
}

#[derive(Clone)]
pub struct SlotSpec {
    pub external: bool,
    pub team: Option<u8>,
}

pub struct Game {
    pub st: Arc<Statics>,
    pub cfg: Arc<Cfg>,
    pub rng: Rng,
    pub tick: u32,
    pub t: f32,
    pub mechs: Vec<Mech>,
    pub npcs: Vec<Npc>,
    pub crates: Vec<Crate>,
    pub cubes: Vec<Cube>,
    pub pickups: Vec<Pickup>,
    pub projs: Vec<Proj>,
    pub sounds: Vec<Sound>,
    pub beams: Vec<Beam>,
    pub drops: Vec<SupplyDrop>,
    pub circles: Vec<([f32; 2], f32)>,
    pub next_cube: i64,
    pub next_pickup: i64,
    pub next_proj: i64,
    pub next_drop: i64,
    pub drops_called: usize,
    pub over: bool,
    take_reqs: Vec<(usize, i64, Option<i64>)>,
}

fn body_of(ms: &MS) -> Body {
    Body { r: ms.radius, h: ms.height, step: ms.step_up, slope_tan: ms.max_slope_deg.to_radians().tan() }
}

impl Game {
    pub fn new(st: Arc<Statics>, cfg: Arc<Cfg>, seed: u64, slots: &[SlotSpec]) -> Game {
        let mut rng = Rng::new(seed);
        let n_actor = slots.len() + 64;
        // Zone circles.
        let mut circles = vec![(cfg.zone_center, cfg.zone_radius)];
        let b = st.w.bounds;
        let mid = [(b[0] + b[1]) * 0.5, (b[2] + b[3]) * 0.5];
        for ph in &cfg.phases {
            let (c, r) = *circles.last().unwrap();
            let slack = (r - ph.radius).max(0.0);
            let mut best = c;
            for k in 0..64 {
                let o = rng.in_disc(slack * 0.75);
                let mut cand = [c[0] + o[0], c[1] + o[1]];
                if k > 40 {
                    // drift toward the map middle if we keep failing
                    let f = 0.3;
                    cand = [cand[0] + (mid[0] - cand[0]) * f, cand[1] + (mid[1] - cand[1]) * f];
                }
                let m = ph.radius.min(120.0).max(25.0);
                if cand[0] > b[0] + m && cand[0] < b[1] - m && cand[1] > b[2] + m && cand[1] < b[3] - m && dist2d(&cand, &c) + ph.radius <= r + 0.01 {
                    best = cand;
                    break;
                }
            }
            circles.push((best, ph.radius));
        }
        let base_mech = cfg.mech_kv.clone();
        let ms0 = MS::from_kv(&base_mech, 1.0);
        let wkv = cfg.weapons_kv.clone();
        let ws: [WS; 4] = [WS::from_kv(&wkv[0]), WS::from_kv(&wkv[1]), WS::from_kv(&wkv[2]), WS::from_kv(&wkv[3])];
        // Spawns.
        let spawn_r = cfg.zone_radius * cfg.spawn_zone_frac;
        let mut spawns: Vec<[f32; 3]> = Vec::new();
        let mut sep = cfg.spawn_min_sep;
        let mut tries = 0;
        while spawns.len() < slots.len() {
            tries += 1;
            if tries % 4000 == 0 {
                sep *= 0.92;
                spawns.clear();
            }
            let p = st.ground_pts[rng.below(st.ground_pts.len())];
            if dist2d(&p, &cfg.zone_center) > spawn_r || !st.w.in_bounds(p[0], p[1], cfg.arena_margin) {
                continue;
            }
            if spawns.iter().any(|s| dist2d(s, &p) < sep) {
                continue;
            }
            spawns.push(p);
        }
        let mut mechs = Vec::new();
        for (i, sl) in slots.iter().enumerate() {
            let p = spawns[i];
            let yaw = rng.range(-PI, PI);
            mechs.push(Mech {
                id: i,
                alive: true,
                team: sl.team,
                external: sl.external,
                pos: [p[0], p[1], st.w.surface_below(p[0], p[1], p[2] + 0.5, 0.0)],
                vel: [0.0; 3],
                yaw,
                pitch: 0.0,
                cmd_yaw: yaw,
                cmd_pitch: 0.0,
                mv: [0.0; 2],
                boost: false,
                fire: [false; 2],
                jump_req: false,
                hp: ms0.hp,
                max_hp: ms0.hp,
                energy: ms0.energy,
                grounded: true,
                cubes: 0,
                upgrades: vec![],
                up_count: vec![0; cfg.upgrades.len()],
                mech_kv: base_mech.clone(),
                ms: ms0.clone(),
                wkv: wkv.clone(),
                ws: ws.clone(),
                slots: [Some(Weapon::new(WK::Gun, &ws[0])), None],
                last_combat: -100.0,
                last_spend: -100.0,
                offer: None,
                pending_offers: 0,
                next_offer_id: 1,
                thresh_idx: 0,
                short_until: -1.0,
                step_acc: 0.0,
                jet_acc: 0.0,
                boosting: false,
                jetting: false,
                in_zone: true,
                events: vec![],
                zone_acc: 0.0,
                kills: 0,
                dmg_dealt: 0.0,
                dmg_taken: 0.0,
                death_t: -1.0,
                place: 0,
                cause: "alive",
                killer: None,
                last_hit_by: None,
                last_hit_weapon: "",
                last_damaged_t: -100.0,
                last_attacker: None,
                seen_until: vec![-100.0; n_actor],
                last_obs_t: -1.0,
                hist: VecDeque::with_capacity(HIST + 1),
                shots: 0,
                hits: 0,
            });
        }
        let mut g = Game {
            st: st.clone(),
            cfg: cfg.clone(),
            rng,
            tick: 0,
            t: 0.0,
            mechs,
            npcs: vec![],
            crates: vec![],
            cubes: vec![],
            pickups: vec![],
            projs: vec![],
            sounds: vec![],
            beams: vec![],
            drops: vec![],
            circles,
            next_cube: 5000,
            next_pickup: 8000,
            next_proj: 1,
            next_drop: 20000,
            drops_called: 0,
            over: false,
            take_reqs: vec![],
        };
        // NPCs.
        let npc_n = cfg.npc("count") as usize;
        for k in 0..npc_n {
            let p = g.random_ground_in(cfg.zone_center, cfg.zone_radius * 0.9, 1000);
            g.npcs.push(Npc {
                id: 100 + k as i64,
                alive: true,
                pos: p,
                vel: [0.0; 3],
                yaw: 0.0,
                pitch: 0.0,
                hp: cfg.npc("hp"),
                home: p,
                target: None,
                cooldown: 0.0,
                wander: [p[0], p[1]],
                returning: false,
                think: 0.0,
            });
        }
        // Crates.
        for k in 0..cfg.crate_count {
            let roof = !g.st.roof_pts.is_empty() && g.rng.f32() < cfg.crate_roof_fraction;
            let p = if roof { g.st.roof_pts[g.rng.below(g.st.roof_pts.len())] } else { g.random_ground_in(cfg.zone_center, cfg.zone_radius, 1000) };
            g.crates.push(Crate { id: 1000 + k as i64, pos: p, hp: cfg.crate_hp, alive: true });
        }
        // Floor loot.
        for _ in 0..cfg.floor_count {
            let roof = !g.st.roof_pts.is_empty() && g.rng.f32() < cfg.floor_roof_fraction;
            let p = if roof { g.st.roof_pts[g.rng.below(g.st.roof_pts.len())] } else { g.random_ground_in(cfg.zone_center, cfg.zone_radius, 1000) };
            let k = g.random_weapon_kind();
            g.spawn_pickup(p, Weapon::new(k, &ws[k.idx()]));
        }
        for i in 0..g.mechs.len() {
            let s = (g.mechs[i].pos, [0.0; 3]);
            g.mechs[i].hist.push_back(s);
        }
        g
    }

    pub fn random_weapon_kind(&mut self) -> WK {
        let wsum: f32 = self.cfg.loot_weights.iter().sum();
        let mut r = self.rng.f32() * wsum.max(1e-6);
        for k in WK::ALL {
            r -= self.cfg.loot_weights[k.idx()];
            if r <= 0.0 {
                return k;
            }
        }
        WK::Lightning
    }

    pub fn random_ground_in(&mut self, c: [f32; 2], r: f32, tries: usize) -> V3 {
        let n = self.st.ground_pts.len();
        let mut best = self.st.ground_pts[self.rng.below(n)];
        let mut bd = f32::MAX;
        for _ in 0..tries {
            let p = self.st.ground_pts[self.rng.below(n)];
            let d = dist2d(&p, &c);
            if d <= r && self.st.w.in_bounds(p[0], p[1], 15.0) {
                return p;
            }
            if d < bd {
                bd = d;
                best = p;
            }
        }
        best
    }

    fn spawn_pickup(&mut self, p: V3, w: Weapon) -> i64 {
        let id = self.next_pickup;
        self.next_pickup += 1;
        self.pickups.push(Pickup { id, pos: p, w, alive: true });
        id
    }

    fn spawn_cube(&mut self, p: V3) {
        let id = self.next_cube;
        self.next_cube += 1;
        self.cubes.push(Cube { id, pos: p, alive: true });
    }

    fn scatter(&mut self, p: V3, r: f32) -> V3 {
        let o = self.rng.in_disc(r);
        let (x, y) = (p[0] + o[0], p[1] + o[1]);
        let z = self.st.w.surface_below(x, y, p[2] + 0.5, 0.0);
        if (z - p[2]).abs() > 2.0 {
            p
        } else {
            [x, y, z]
        }
    }

    pub fn sound(&mut self, kind: &'static str, pos: V3, src: Option<usize>) {
        self.sounds.push(Sound { kind, pos, t: self.t + DT, src });
    }

    /// Sets `t` to the current tick (call before building observations).
    pub fn sync_time(&mut self) {
        self.t = self.tick as f32 * DT;
    }

    pub fn alive_count(&self) -> usize {
        self.mechs.iter().filter(|m| m.alive).count()
    }

    pub fn hostile(&self, a: usize, b: usize) -> bool {
        a != b && (self.mechs[a].team.is_none() || self.mechs[a].team != self.mechs[b].team)
    }

    pub fn cube_mult(&self, i: usize) -> f32 {
        1.0 + self.cfg.damage_per_cube * (self.mechs[i].cubes as f32).min(self.cfg.stat_cap)
    }

    pub fn zone_at(&self, t: f32) -> ZoneNow {
        let mut start = 0.0f32;
        for (i, ph) in self.cfg.phases.iter().enumerate() {
            let hold_end = start + ph.hold_s;
            let end = hold_end + ph.shrink_s;
            if t < end {
                let (c0, r0) = self.circles[i];
                let (c1, r1) = self.circles[i + 1];
                let f = if t < hold_end { 0.0 } else { ((t - hold_end) / ph.shrink_s.max(1e-6)).clamp(0.0, 1.0) };
                return ZoneNow {
                    center: [c0[0] + (c1[0] - c0[0]) * f, c0[1] + (c1[1] - c0[1]) * f],
                    radius: r0 + (r1 - r0) * f,
                    next_center: c1,
                    next_radius: r1,
                    shrink_start: hold_end,
                    shrink_end: end,
                    dps: ph.dps,
                };
            }
            start = end;
        }
        let (c, r) = *self.circles.last().unwrap();
        ZoneNow { center: c, radius: r, next_center: c, next_radius: r, shrink_start: start, shrink_end: start, dps: self.cfg.phases.last().map(|p| p.dps).unwrap_or(0.0) }
    }

    // ------------------------------------------------------------------ vision

    /// Can mech `v` see point `p` (or `top`)? `trees`: apply tree occlusion beyond near range.
    pub fn sees(&self, v: usize, p: V3, top: Option<V3>) -> bool {
        let m = &self.mechs[v];
        let eye = m.eye();
        let d = dist(eye, p);
        let near = self.cfg.vis_near + m.ms.near_bonus;
        let w = &self.st.w;
        if d <= near {
            return w.los(eye, p) || top.map(|t| w.los(eye, t)).unwrap_or(false);
        }
        if d > self.cfg.vis_range + m.ms.vision_bonus {
            return false;
        }
        let half = self.cfg.vis_fov_deg.to_radians() * 0.5;
        if angdiff(yaw_to(&eye, &p), m.yaw).abs() > half {
            return false;
        }
        let tr = self.cfg.canopy_blocks;
        if w.los(eye, p) && !(tr && self.st.trees.blocks(eye, p)) {
            return true;
        }
        if let Some(t) = top {
            if w.los(eye, t) && !(tr && self.st.trees.blocks(eye, t)) {
                return true;
            }
        }
        false
    }

    pub fn sees_mech(&self, v: usize, j: usize) -> bool {
        let m = &self.mechs[j];
        self.sees(v, m.center(), Some([m.pos[0], m.pos[1], m.pos[2] + m.ms.height]))
    }

    pub fn sees_npc(&self, v: usize, k: usize) -> bool {
        let n = &self.npcs[k];
        let h = self.cfg.npc("height");
        self.sees(v, [n.pos[0], n.pos[1], n.pos[2] + h * 0.5], Some([n.pos[0], n.pos[1], n.pos[2] + h]))
    }

    // ------------------------------------------------------------------ commands

    pub fn apply_cmd(&mut self, i: usize, c: &Cmd) {
        let t = self.t;
        let m = &mut self.mechs[i];
        if !m.alive {
            return;
        }
        let mut mv = c.mv;
        if !mv[0].is_finite() || !mv[1].is_finite() {
            mv = [0.0, 0.0];
        }
        let l = (mv[0] * mv[0] + mv[1] * mv[1]).sqrt();
        if l > 1.0 {
            mv = [mv[0] / l, mv[1] / l];
        }
        m.mv = mv;
        m.boost = c.boost;
        m.fire = c.fire;
        if c.jump {
            m.jump_req = true;
        }
        if let Some(a) = c.aim {
            m.cmd_yaw = wrap(a[0]);
            m.cmd_pitch = a[1].clamp(m.ms.min_pitch, m.ms.max_pitch);
        }
        if c.reload {
            let mag = m.ws[0].mag as i32;
            let rl = m.ws[0].reload;
            let mut snd = false;
            for s in m.slots.iter_mut().flatten() {
                if s.kind == WK::Gun && s.reload <= 0.0 && s.ammo < mag {
                    s.reload = rl;
                    snd = true;
                }
            }
            if snd {
                let p = m.pos;
                self.sound("reload", p, Some(i));
            }
        }
        if let Some(k) = c.pick {
            let ok = self.mechs[i].offer.as_ref().map(|o| k >= 0 && (k as usize) < o.choices.len()).unwrap_or(false);
            if ok {
                let u = self.mechs[i].offer.as_ref().unwrap().choices[k as usize];
                self.mechs[i].offer = None;
                self.apply_upgrade(i, u);
            }
        }
        let _ = t;
        if let Some(id) = c.take {
            self.take_reqs.push((i, id, c.drop));
        } else if let Some(s) = c.drop {
            self.drop_slot(i, s);
        }
    }

    fn drop_slot(&mut self, i: usize, s: i64) {
        if !(0..=1).contains(&s) {
            return;
        }
        let s = s as usize;
        let busy = self.mechs[i].slots[s].as_ref().map(|w| w.charging).unwrap_or(true);
        if busy {
            return;
        }
        let mut w = self.mechs[i].slots[s].take().unwrap();
        w.charge = 0.0;
        w.charging = false;
        let p = self.mechs[i].pos;
        let kind = w.kind;
        let id = self.spawn_pickup(p, w);
        self.ev(i, format!(r#"{{"type":"dropped","id":{id},"kind":"{}","slot":{s}}}"#, kind.name()));
    }

    /// Resolves this decision's `take` requests (closest mech wins a contested pickup).
    pub fn resolve_takes(&mut self) {
        let mut reqs = std::mem::take(&mut self.take_reqs);
        let dist_of = |g: &Game, r: &(usize, i64, Option<i64>)| -> f32 {
            g.pickups.iter().find(|p| p.alive && p.id == r.1).map(|p| dist2d(&p.pos, &g.mechs[r.0].pos)).unwrap_or(f32::MAX)
        };
        reqs.sort_by(|a, b| dist_of(self, a).partial_cmp(&dist_of(self, b)).unwrap_or(std::cmp::Ordering::Equal));
        for (i, id, drop) in reqs {
            let pi = match self.pickups.iter().position(|p| p.alive && p.id == id) {
                Some(p) => p,
                None => {
                    self.ev(i, format!(r#"{{"type":"take_failed","id":{id},"reason":"gone"}}"#));
                    continue;
                }
            };
            let m = &self.mechs[i];
            let pp = self.pickups[pi].pos;
            if dist2d(&pp, &m.pos) > self.cfg.loot_pickup_radius || (pp[2] - m.pos[2]).abs() > 4.0 {
                self.ev(i, format!(r#"{{"type":"take_failed","id":{id},"reason":"range"}}"#));
                continue;
            }
            let kind = self.pickups[pi].w.kind;
            let drop = drop.filter(|d| (0..=1).contains(d)).map(|d| d as usize);
            let other_has = (0..2).any(|s| Some(s) != drop && m.slots[s].as_ref().map(|w| w.kind == kind).unwrap_or(false));
            if other_has {
                self.ev(i, format!(r#"{{"type":"take_failed","id":{id},"reason":"have"}}"#));
                continue;
            }
            let slot = match drop {
                Some(s) => s,
                None => match (0..2).find(|&s| m.slots[s].is_none()) {
                    Some(s) => s,
                    None => {
                        self.ev(i, format!(r#"{{"type":"take_failed","id":{id},"reason":"full"}}"#));
                        continue;
                    }
                },
            };
            if m.slots[slot].as_ref().map(|w| w.charging).unwrap_or(false) {
                self.ev(i, format!(r#"{{"type":"take_failed","id":{id},"reason":"busy"}}"#));
                continue;
            }
            if self.mechs[i].slots[slot].is_some() {
                self.drop_slot(i, slot as i64);
            }
            let mut w = self.pickups[pi].w.clone();
            self.pickups[pi].alive = false;
            let cd = self.mechs[i].ws[kind.idx()].cooldown;
            w.cooldown = w.cooldown.max(self.cfg.equip_s.max(cd));
            w.charging = false;
            w.charge = 0.0;
            w.hold = 0.0;
            w.release_pending = false;
            self.mechs[i].slots[slot] = Some(w);
            self.ev(i, format!(r#"{{"type":"took","id":{id},"kind":"{}","slot":{slot}}}"#, kind.name()));
        }
        self.pickups.retain(|p| p.alive);
    }

    pub fn ev(&mut self, i: usize, s: String) {
        if self.mechs[i].external {
            self.mechs[i].events.push(s);
        }
    }

    // ------------------------------------------------------------------ upgrades & cubes

    pub fn recompute(&mut self, i: usize) {
        let cap = self.cfg.stat_cap;
        let hpc = self.cfg.hp_per_cube;
        let spc = self.cfg.size_per_cube;
        let m = &mut self.mechs[i];
        let size = 1.0 + spc * m.cubes as f32;
        m.ms = MS::from_kv(&m.mech_kv, size);
        for k in 0..4 {
            m.ws[k] = WS::from_kv(&m.wkv[k]);
        }
        let new_max = m.ms.hp * (1.0 + hpc * (m.cubes as f32).min(cap));
        if new_max > m.max_hp {
            m.hp += new_max - m.max_hp;
        }
        m.max_hp = new_max;
        m.hp = m.hp.min(m.max_hp);
        m.energy = m.energy.min(m.ms.energy);
    }

    pub fn apply_upgrade(&mut self, i: usize, u: usize) {
        let def = self.cfg.upgrades[u].clone();
        {
            let m = &mut self.mechs[i];
            for e in &def.effects {
                let parts: Vec<&str> = e.path.split('.').collect();
                let kv: &mut KV = match parts.as_slice() {
                    ["mech", _] => &mut m.mech_kv,
                    ["weapons", k, _] => match WK::parse(k) {
                        Some(w) => &mut m.wkv[w.idx()],
                        None => continue,
                    },
                    _ => continue,
                };
                let key = *parts.last().unwrap();
                let cur = kv_get(kv, key);
                let nv = if let Some(mu) = e.mul { cur * mu } else { cur + e.add.unwrap_or(0.0) };
                kv_set(kv, key, nv);
            }
            m.up_count[u] += 1;
            m.upgrades.push(def.id.clone());
        }
        self.recompute(i);
        let id = def.id.clone();
        let mut s = String::from(r#"{"type":"upgrade","id":"#);
        jstr(&mut s, &id);
        s.push('}');
        self.ev(i, s);
    }

    fn draw_offer(&mut self, i: usize) {
        let mut elig: Vec<usize> = Vec::new();
        {
            let m = &self.mechs[i];
            for (u, d) in self.cfg.upgrades.iter().enumerate() {
                if m.up_count[u] >= d.max {
                    continue;
                }
                if let Some(r) = d.requires {
                    if !m.has(r) {
                        continue;
                    }
                }
                elig.push(u);
            }
        }
        let mut choices = Vec::new();
        while choices.len() < 3 && !elig.is_empty() {
            let k = self.rng.below(elig.len());
            choices.push(elig.swap_remove(k));
        }
        if choices.is_empty() {
            return;
        }
        let m = &mut self.mechs[i];
        let id = m.next_offer_id;
        m.next_offer_id += 1;
        m.offer = Some(Offer { id, choices, expires: self.t + self.cfg.offer_timeout_s });
    }

    pub fn give_cubes(&mut self, i: usize, n: u32, event: bool) {
        for _ in 0..n {
            self.mechs[i].cubes += 1;
            let c = self.mechs[i].cubes;
            let th = &self.cfg.thresholds;
            let m = &mut self.mechs[i];
            while m.thresh_idx < th.len() && c >= th[m.thresh_idx] {
                m.thresh_idx += 1;
                m.pending_offers += 1;
            }
        }
        self.recompute(i);
        if event {
            let c = self.mechs[i].cubes;
            self.ev(i, format!(r#"{{"type":"pickup","cubes":{c}}}"#));
        }
    }

    // ------------------------------------------------------------------ damage

    /// Applies damage. `from` is where it came from (for bearing / rear armour).
    pub fn hurt(&mut self, tgt: Tgt, amount: f32, weapon: &'static str, src: Src, from: V3, effect: Option<&'static str>) -> f32 {
        let t = self.t;
        match tgt {
            Tgt::Mech(i) => {
                if !self.mechs[i].alive || amount <= 0.0 {
                    return 0.0;
                }
                let m = &self.mechs[i];
                let c = m.center();
                let bearing = yaw_to(&c, &from);
                let mut amt = amount;
                let mut rear = false;
                if src != Src::Zone {
                    if dist2d(&c, &from) > 0.2 && Src::Mech(i) != src {
                        rear = angdiff(bearing, m.yaw).abs() > m.ms.front_arc_deg.to_radians() * 0.5;
                    }
                    if rear {
                        amt *= m.ms.rear_mult;
                    }
                    amt *= (1.0 - m.ms.armor).max(0.0);
                }
                let m = &mut self.mechs[i];
                m.hp -= amt;
                m.dmg_taken += amt;
                m.last_combat = t;
                m.last_hit_by = Some(src);
                m.last_hit_weapon = weapon;
                if src != Src::Zone && src != Src::Mech(i) {
                    m.last_damaged_t = t;
                    m.last_attacker = Some(src);
                }
                if src == Src::Zone {
                    m.zone_acc += amt;
                    return amt;
                }
                if m.external {
                    let mut s = String::from(r#"{"type":"damaged","amount":"#);
                    jnum(&mut s, amt, 2);
                    let _ = write!(s, r#","weapon":"{weapon}","bearing":"#);
                    jnum(&mut s, bearing, 3);
                    s.push_str(r#","by":"#);
                    match src {
                        Src::Mech(a) => {
                            let _ = write!(s, "{a}");
                        }
                        Src::Npc(k) => {
                            let _ = write!(s, "{}", self.npcs[k].id);
                        }
                        Src::Zone => s.push_str("null"),
                    }
                    if rear {
                        s.push_str(r#","rear":true"#);
                    }
                    s.push('}');
                    m.events.push(s);
                }
                if let Src::Mech(a) = src {
                    if a != i {
                        self.mechs[a].dmg_dealt += amt;
                        self.mechs[a].last_combat = t;
                        self.hit_event(a, i as i64, amt, weapon, effect, rear);
                    }
                }
                amt
            }
            Tgt::Npc(k) => {
                if !self.npcs[k].alive {
                    return 0.0;
                }
                self.npcs[k].hp -= amount;
                if let Src::Mech(a) = src {
                    self.npcs[k].target = Some(a);
                    self.npcs[k].returning = false;
                    self.mechs[a].last_combat = t;
                    let id = self.npcs[k].id;
                    self.hit_event(a, id, amount, weapon, effect, false);
                }
                amount
            }
            Tgt::Crate(k) => {
                if !self.crates[k].alive {
                    return 0.0;
                }
                self.crates[k].hp -= amount;
                if let Src::Mech(a) = src {
                    let id = self.crates[k].id;
                    self.hit_event(a, id, amount, weapon, effect, false);
                }
                if self.crates[k].hp <= 0.0 {
                    self.crates[k].alive = false;
                    let p = self.crates[k].pos;
                    for _ in 0..self.cfg.crate_cubes {
                        self.spawn_cube(p);
                    }
                    if self.rng.f32() < self.cfg.crate_weapon_chance {
                        let kind = self.random_weapon_kind();
                        let ws = WS::from_kv(&self.cfg.weapons_kv[kind.idx()]);
                        let q = self.scatter(p, 1.5);
                        self.spawn_pickup(q, Weapon::new(kind, &ws));
                    }
                    self.sound("crate", p, None);
                }
                amount
            }
        }
    }

    fn hit_event(&mut self, a: usize, target: i64, amt: f32, weapon: &str, effect: Option<&str>, rear: bool) {
        if !self.mechs[a].external {
            return;
        }
        let mut s = format!(r#"{{"type":"hit","target":{target},"amount":"#);
        jnum(&mut s, amt, 2);
        let _ = write!(s, r#","weapon":"{weapon}""#);
        if let Some(e) = effect {
            let _ = write!(s, r#","effect":"{e}""#);
        }
        if rear {
            s.push_str(r#","rear":true"#);
        }
        s.push('}');
        self.mechs[a].events.push(s);
    }

    fn effect_event(&mut self, i: usize, effect: &str, by: Option<i64>) {
        let b = by.map(|b| b.to_string()).unwrap_or("null".into());
        self.ev(i, format!(r#"{{"type":"effect","effect":"{effect}","by":{b}}}"#));
    }

    /// All actors hit along a ray, sorted by distance. `src` is excluded (and its squad).
    pub fn ray_actors(&self, p: V3, d: V3, len: f32, src: Src, extra_r: f32, crates: bool) -> Vec<(f32, Tgt)> {
        let mut out = Vec::new();
        let (src_mech, src_team) = match src {
            Src::Mech(i) => (Some(i), self.mechs[i].team),
            _ => (None, None),
        };
        for (j, m) in self.mechs.iter().enumerate() {
            if !m.alive || Some(j) == src_mech {
                continue;
            }
            if src_team.is_some() && m.team == src_team {
                continue;
            }
            if let Some(t) = phys::seg_cyl(p, d, len, m.pos, m.ms.radius + extra_r, m.ms.height) {
                out.push((t, Tgt::Mech(j)));
            }
        }
        if matches!(src, Src::Npc(_)) {
            out.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
            return out;
        }
        let nr = self.cfg.npc("radius") + extra_r;
        let nh = self.cfg.npc("height");
        for (k, n) in self.npcs.iter().enumerate() {
            if n.alive {
                if let Some(t) = phys::seg_cyl(p, d, len, n.pos, nr, nh) {
                    out.push((t, Tgt::Npc(k)));
                }
            }
        }
        if crates {
            let cr = self.cfg.crate_radius + extra_r;
            for (k, c) in self.crates.iter().enumerate() {
                if c.alive {
                    if let Some(t) = phys::seg_cyl(p, d, len, c.pos, cr, self.cfg.crate_height) {
                        out.push((t, Tgt::Crate(k)));
                    }
                }
            }
        }
        out.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
        out
    }

    fn shot_origin(&self, i: usize, base: V3, dir: V3) -> V3 {
        let off = self.mechs[i].ms.radius + 0.5;
        let d = self.st.w.raycast(base, dir, off);
        let k = if d < off { (d - 0.3).max(0.0) } else { off };
        add(base, scale(dir, k))
    }

    fn tgt_center(&self, t: Tgt) -> V3 {
        match t {
            Tgt::Mech(j) => self.mechs[j].center(),
            Tgt::Npc(k) => {
                let n = &self.npcs[k];
                [n.pos[0], n.pos[1], n.pos[2] + self.cfg.npc("height") * 0.5]
            }
            Tgt::Crate(k) => {
                let c = &self.crates[k];
                [c.pos[0], c.pos[1], c.pos[2] + self.cfg.crate_height * 0.5]
            }
        }
    }

    // ------------------------------------------------------------------ weapons

    fn fire_gun(&mut self, i: usize) {
        let ws = self.mechs[i].ws[0].clone();
        let m = &self.mechs[i];
        let base = m.muzzle();
        let sp = ws.spread_deg.to_radians();
        let (yaw, pitch) = (m.yaw, m.pitch);
        let aim_dir = dir_of(yaw, pitch);
        let origin = self.shot_origin(i, base, aim_dir);
        let dir = dir_of(yaw + self.rng.normal() * sp, pitch + self.rng.normal() * sp);
        let id = self.next_proj;
        self.next_proj += 1;
        let team = self.mechs[i].team;
        let dmg = ws.damage * self.cube_mult(i);
        self.projs.push(Proj { id, kind: PK::Gun, owner: Src::Mech(i), team, pos: origin, vel: scale(dir, ws.speed), left: ws.range, dmg, radius: self.cfg.gun_radius, mws: None, scale: 1.0, alive: true });
        self.mechs[i].shots += 1;
        self.sound("gun", base, Some(i));
    }

    fn fire_mortar(&mut self, i: usize) {
        let ws = self.mechs[i].ws[1].clone();
        let m = &self.mechs[i];
        let base = [m.pos[0], m.pos[1], m.pos[2] + m.ms.height + 0.5];
        let dir = dir_of(m.yaw, m.pitch);
        let origin = self.shot_origin(i, base, dir);
        let id = self.next_proj;
        self.next_proj += 1;
        let team = m.team;
        let sc = self.cube_mult(i);
        self.projs.push(Proj { id, kind: PK::Mortar, owner: Src::Mech(i), team, pos: origin, vel: scale(dir, ws.speed), left: ws.ttl, dmg: ws.damage * sc, radius: 0.3, mws: Some(ws), scale: sc, alive: true });
        self.sound("mortar", base, Some(i));
    }

    fn fire_laser(&mut self, i: usize, charge: f32) {
        let ws = self.mechs[i].ws[2].clone();
        let m = &self.mechs[i];
        let base = m.muzzle();
        let dir = dir_of(m.yaw, m.pitch);
        let origin = self.shot_origin(i, base, dir);
        let frac = ws.min_frac + (1.0 - ws.min_frac) * (charge / ws.charge.max(1e-6)).min(1.0);
        let dmg = ws.damage * frac * self.cube_mult(i);
        let dw = self.st.w.raycast(origin, dir, ws.range);
        let hits = self.ray_actors(origin, dir, dw, Src::Mech(i), 0.0, true);
        let n = 1 + ws.pierce.max(0.0) as usize;
        let mut end = add(origin, scale(dir, dw));
        for (k, (t, tg)) in hits.iter().enumerate() {
            if k >= n {
                break;
            }
            let mut d = dmg;
            let mut eff = None;
            if let Tgt::Mech(j) = tg {
                if self.mechs[*j].has(WK::Mortar) {
                    d *= 1.0 + ws.cookoff;
                    eff = Some("cookoff");
                }
            }
            self.hurt(*tg, d, "laser", Src::Mech(i), origin, eff);
            if let (Some(_), Tgt::Mech(j)) = (eff, tg) {
                self.effect_event(*j, "cookoff", Some(i as i64));
            }
            if k + 1 == n {
                end = add(origin, scale(dir, *t));
            }
        }
        self.beams.push(Beam { by: i, kind: "laser", path: vec![origin, end], t: self.t + DT });
        self.sound("laser", base, Some(i));
    }

    fn fire_lightning(&mut self, i: usize) {
        let ws = self.mechs[i].ws[3].clone();
        let m = &self.mechs[i];
        let base = m.muzzle();
        let yaw = m.yaw;
        let dir = dir_of(m.yaw, m.pitch);
        let origin = self.shot_origin(i, base, dir);
        let cone = ws.cone_deg.to_radians();
        let mut cands: Vec<(Tgt, V3)> = Vec::new();
        for (j, o) in self.mechs.iter().enumerate() {
            if o.alive && self.hostile(i, j) {
                cands.push((Tgt::Mech(j), o.center()));
            }
        }
        for (k, n) in self.npcs.iter().enumerate() {
            if n.alive {
                cands.push((Tgt::Npc(k), self.tgt_center(Tgt::Npc(k))));
            }
        }
        for (k, c) in self.crates.iter().enumerate() {
            if c.alive {
                cands.push((Tgt::Crate(k), self.tgt_center(Tgt::Crate(k))));
            }
        }
        let w = &self.st.w;
        let mut first: Option<(f32, usize)> = None;
        for (ci, (_, c)) in cands.iter().enumerate() {
            let d = dist(base, *c);
            if d > ws.range || angdiff(yaw_to(&base, c), yaw).abs() > cone {
                continue;
            }
            if first.map(|f| d < f.0).unwrap_or(true) && w.los(base, *c) {
                first = Some((d, ci));
            }
        }
        let mut path = vec![origin];
        let mut hit: Vec<usize> = vec![];
        let dmg0 = ws.damage * self.cube_mult(i);
        if let Some((_, ci)) = first {
            hit.push(ci);
            let mut cur = ci;
            for _ in 0..(ws.chain.max(0.0) as usize) {
                let cp = cands[cur].1;
                let mut best: Option<(f32, usize)> = None;
                for (cj, (_, c)) in cands.iter().enumerate() {
                    if hit.contains(&cj) {
                        continue;
                    }
                    let d = dist(cp, *c);
                    if d <= ws.chain_range && best.map(|b| d < b.0).unwrap_or(true) && w.los(cp, *c) {
                        best = Some((d, cj));
                    }
                }
                match best {
                    Some((_, cj)) => {
                        hit.push(cj);
                        cur = cj;
                    }
                    None => break,
                }
            }
        }
        if hit.is_empty() {
            let d = self.st.w.raycast(origin, dir, ws.range);
            path.push(add(origin, scale(dir, d)));
        }
        let mut from = origin;
        for (k, &ci) in hit.iter().enumerate() {
            let (tg, c) = cands[ci];
            path.push(c);
            let d = dmg0 * ws.chain_falloff.powi(k as i32);
            let mut eff = None;
            if let Tgt::Mech(j) = tg {
                if self.mechs[j].has(WK::Laser) {
                    eff = Some("short");
                }
            }
            self.hurt(tg, d, "lightning", Src::Mech(i), from, eff);
            if let (Some(_), Tgt::Mech(j)) = (eff, tg) {
                let t = self.t;
                let m = &mut self.mechs[j];
                m.short_until = t + ws.short_s;
                for s in m.slots.iter_mut().flatten() {
                    if s.kind == WK::Laser {
                        s.charging = false;
                        s.charge = 0.0;
                        s.hold = 0.0;
                        s.release_pending = false;
                    }
                }
                self.effect_event(j, "short", Some(i as i64));
            }
            from = c;
        }
        self.beams.push(Beam { by: i, kind: "lightning", path, t: self.t + DT });
        self.sound("lightning", base, Some(i));
    }

    fn explode(&mut self, p: V3, prev: V3, owner: Src, team: Option<u8>, direct: Option<Tgt>, ws: &WS, sc: f32) {
        let oi = match owner {
            Src::Mech(i) => Some(i),
            _ => None,
        };
        if let Some(t) = direct {
            self.hurt(t, ws.damage * sc, "mortar", owner, p, Some("concuss").filter(|_| matches!(t, Tgt::Mech(_))));
        }
        let back = norm(sub(prev, p));
        let pa = add(p, scale(back, 0.4));
        let r = ws.splash_radius.max(0.1);
        let edge = ws.splash_edge;
        let w = &self.st.w;
        let mut mech_hits: Vec<(usize, f32)> = vec![];
        for (j, m) in self.mechs.iter().enumerate() {
            if !m.alive {
                continue;
            }
            if Some(j) != oi && team.is_some() && m.team == team {
                continue;
            }
            let hd = (dist2d(&m.pos, &p) - m.ms.radius).max(0.0);
            let cz = p[2].clamp(m.pos[2], m.pos[2] + m.ms.height);
            let d = (hd * hd + (cz - p[2]).powi(2)).sqrt();
            if d > r {
                continue;
            }
            let ok = direct == Some(Tgt::Mech(j)) || w.los(pa, m.center()) || w.los(pa, [m.pos[0], m.pos[1], cz]);
            if ok {
                mech_hits.push((j, 1.0 - (1.0 - edge) * (d / r)));
            }
        }
        let nr = self.cfg.npc("radius");
        let nh = self.cfg.npc("height");
        let mut npc_hits = vec![];
        for (k, n) in self.npcs.iter().enumerate() {
            if !n.alive || direct == Some(Tgt::Npc(k)) {
                continue;
            }
            let hd = (dist2d(&n.pos, &p) - nr).max(0.0);
            let cz = p[2].clamp(n.pos[2], n.pos[2] + nh);
            let d = (hd * hd + (cz - p[2]).powi(2)).sqrt();
            if d <= r && w.los(pa, [n.pos[0], n.pos[1], cz]) {
                npc_hits.push((k, 1.0 - (1.0 - edge) * (d / r)));
            }
        }
        let mut crate_hits = vec![];
        for (k, c) in self.crates.iter().enumerate() {
            if !c.alive || direct == Some(Tgt::Crate(k)) {
                continue;
            }
            let hd = (dist2d(&c.pos, &p) - self.cfg.crate_radius).max(0.0);
            let cz = p[2].clamp(c.pos[2], c.pos[2] + self.cfg.crate_height);
            let d = (hd * hd + (cz - p[2]).powi(2)).sqrt();
            if d <= r && w.los(pa, [c.pos[0], c.pos[1], cz]) {
                crate_hits.push((k, 1.0 - (1.0 - edge) * (d / r)));
            }
        }
        for (j, f) in mech_hits {
            let is_direct = direct == Some(Tgt::Mech(j));
            if !is_direct {
                let mut dmg = ws.splash * sc * f;
                if Some(j) == oi {
                    dmg *= self.mechs[j].ms.self_damage_mult;
                }
                self.hurt(Tgt::Mech(j), dmg, "mortar", owner, p, Some("concuss"));
            }
            let m = &mut self.mechs[j];
            let mut hdir = [m.pos[0] - p[0], m.pos[1] - p[1]];
            let l = (hdir[0] * hdir[0] + hdir[1] * hdir[1]).sqrt();
            if l < 1e-3 {
                hdir = [1.0, 0.0];
            } else {
                hdir = [hdir[0] / l, hdir[1] / l];
            }
            m.vel[0] += hdir[0] * ws.knock * f;
            m.vel[1] += hdir[1] * ws.knock * f;
            m.vel[2] = m.vel[2].max(0.0) + ws.knock_up * f;
            m.grounded = false;
            m.energy = (m.energy - ws.drain * f).max(0.0);
            let by = oi.map(|o| o as i64);
            self.effect_event(j, "concuss", by);
        }
        for (k, f) in npc_hits {
            self.hurt(Tgt::Npc(k), ws.splash * sc * f, "mortar", owner, p, None);
        }
        for (k, f) in crate_hits {
            self.hurt(Tgt::Crate(k), ws.splash * sc * f, "mortar", owner, p, None);
        }
        self.sound("explosion", p, oi);
    }

    fn mech_weapons(&mut self, i: usize) {
        let t = self.t;
        for s in 0..2 {
            let mut w = match self.mechs[i].slots[s].take() {
                Some(w) => w,
                None => continue,
            };
            let fire = self.mechs[i].fire[s];
            let ws = self.mechs[i].ws[w.kind.idx()].clone();
            w.cooldown -= DT;
            match w.kind {
                WK::Gun => {
                    if w.reload > 0.0 {
                        w.reload -= DT;
                        if w.reload <= 0.0 {
                            w.reload = 0.0;
                            w.ammo = ws.mag as i32;
                        }
                    }
                    let mut fired = false;
                    let mut guard = 0;
                    while fire && w.reload <= 0.0 && w.ammo > 0 && w.cooldown <= 0.0 && guard < 3 {
                        guard += 1;
                        self.fire_gun(i);
                        fired = true;
                        w.ammo -= 1;
                        w.cooldown += ws.cooldown.max(0.01);
                        if w.ammo <= 0 {
                            w.reload = ws.reload;
                            let p = self.mechs[i].pos;
                            self.sound("reload", p, Some(i));
                        }
                    }
                    if !fired && w.cooldown < 0.0 {
                        w.cooldown = 0.0;
                    }
                }
                WK::Mortar => {
                    if fire && w.cooldown <= 0.0 {
                        self.fire_mortar(i);
                        w.cooldown += ws.cooldown;
                    }
                    if w.cooldown < 0.0 {
                        w.cooldown = 0.0;
                    }
                }
                WK::Laser => {
                    let shorted = t < self.mechs[i].short_until;
                    if w.charging {
                        if shorted {
                            w.charging = false;
                            w.charge = 0.0;
                        } else {
                            let full = ws.charge.max(1e-3);
                            w.charge = (w.charge + DT).min(full);
                            if w.charge >= full - 1e-4 {
                                w.hold += DT;
                            }
                            let mut go = false;
                            if !fire || w.release_pending {
                                if w.charge >= ws.min_charge - 1e-4 {
                                    go = true;
                                } else {
                                    w.release_pending = true;
                                }
                            } else if w.hold >= ws.hold_s {
                                go = true;
                            }
                            if go {
                                let c = w.charge;
                                // put the weapon back while firing so `has(...)` checks see it
                                w.charging = false;
                                w.charge = 0.0;
                                w.hold = 0.0;
                                w.release_pending = false;
                                w.cooldown = ws.cooldown;
                                self.mechs[i].slots[s] = Some(w);
                                self.fire_laser(i, c);
                                continue;
                            }
                        }
                    } else if fire && w.cooldown <= 0.0 && !shorted {
                        w.charging = true;
                        w.charge = 0.0;
                        w.hold = 0.0;
                        w.release_pending = false;
                        let p = self.mechs[i].muzzle();
                        self.sound("laser_charge", p, Some(i));
                    }
                    if w.cooldown < 0.0 {
                        w.cooldown = 0.0;
                    }
                }
                WK::Lightning => {
                    if fire && w.cooldown <= 0.0 && self.mechs[i].energy >= ws.energy {
                        self.mechs[i].energy -= ws.energy;
                        self.mechs[i].last_spend = t;
                        self.mechs[i].slots[s] = Some(w.clone());
                        self.fire_lightning(i);
                        if let Some(ww) = self.mechs[i].slots[s].take() {
                            w.charge = ww.charge;
                        }
                        w.cooldown += ws.cooldown;
                    }
                    if w.cooldown < 0.0 {
                        w.cooldown = 0.0;
                    }
                }
            }
            self.mechs[i].slots[s] = Some(w);
        }
    }

    // ------------------------------------------------------------------ movement

    fn mech_move(&mut self, i: usize) {
        let t = self.t;
        let st = self.st.clone();
        let w = &st.w;
        let m = &mut self.mechs[i];
        let ms = m.ms.clone();
        let charging = m.charging();
        // Aim slew.
        let tm = if charging { m.ws[2].charge_turn_mult.max(0.0) } else { 1.0 };
        let tr = ms.turn_rate * tm * DT;
        let pr = ms.pitch_rate * tm * DT;
        m.yaw = wrap(m.yaw + angdiff(m.cmd_yaw, m.yaw).clamp(-tr, tr));
        m.pitch = (m.pitch + (m.cmd_pitch - m.pitch).clamp(-pr, pr)).clamp(ms.min_pitch, ms.max_pitch);
        // Jump.
        if m.jump_req {
            if m.grounded {
                m.vel[2] = ms.jump;
                m.grounded = false;
            }
            m.jump_req = false;
        }
        let mvl = (m.mv[0] * m.mv[0] + m.mv[1] * m.mv[1]).sqrt();
        let want_boost = m.boost && m.energy > 0.0;
        m.boosting = m.grounded && want_boost && mvl > 0.05;
        let mut speed = if m.boosting || (!m.grounded && want_boost) { ms.boost } else { ms.walk } * mvl;
        if charging {
            speed *= ms.charge_move_mult;
        }
        let (tx, ty) = if mvl > 1e-6 { (m.mv[0] / mvl * speed, m.mv[1] / mvl * speed) } else { (0.0, 0.0) };
        let acc = if m.grounded { ms.ground_accel } else { ms.air_accel } * DT;
        let (dvx, dvy) = (tx - m.vel[0], ty - m.vel[1]);
        let dl = (dvx * dvx + dvy * dvy).sqrt();
        let k = if dl > acc { acc / dl } else { 1.0 };
        m.vel[0] += dvx * k;
        m.vel[1] += dvy * k;
        let mut spent = false;
        m.jetting = false;
        if !m.grounded {
            m.vel[2] -= ms.gravity * DT;
            if want_boost {
                m.jetting = true;
                if m.vel[2] < ms.max_rise {
                    m.vel[2] = (m.vel[2] + ms.jet_accel * DT).min(ms.max_rise);
                }
                m.energy -= ms.jet_drain * DT;
                spent = true;
            }
        } else if m.boosting {
            m.energy -= ms.boost_drain * DT;
            spent = true;
        }
        if spent {
            m.energy = m.energy.max(0.0);
            m.last_spend = t;
        }
        let body = body_of(&ms);
        let toward = if mvl > 1e-6 { [m.mv[0] / mvl, m.mv[1] / mvl] } else { [0.0, 0.0] };
        phys::integrate(w, &mut m.pos, &mut m.vel, &mut m.grounded, DT, &body, toward);
        // Footstep / jet sounds.
        let hs = (m.vel[0] * m.vel[0] + m.vel[1] * m.vel[1]).sqrt();
        let mut snd: Option<&'static str> = None;
        if m.grounded && hs > self.cfg.step_speed {
            m.step_acc += DT;
            if m.step_acc >= self.cfg.step_every {
                m.step_acc -= self.cfg.step_every;
                snd = Some(if m.boosting { "boost" } else { "step" });
            }
        }
        if m.jetting {
            m.jet_acc += DT;
            if m.jet_acc >= 0.5 {
                m.jet_acc -= 0.5;
                snd = Some("jet");
            }
        }
        let p = m.pos;
        if let Some(s) = snd {
            self.sound(s, p, Some(i));
        }
    }

    fn push_apart(&mut self) {
        let n = self.mechs.len();
        let st = self.st.clone();
        for a in 0..n {
            if !self.mechs[a].alive {
                continue;
            }
            for b in (a + 1)..n {
                if !self.mechs[b].alive {
                    continue;
                }
                let (pa, pb) = (self.mechs[a].pos, self.mechs[b].pos);
                let (ra, rb) = (self.mechs[a].ms.radius, self.mechs[b].ms.radius);
                let (ha, hb) = (self.mechs[a].ms.height, self.mechs[b].ms.height);
                if pa[2] >= pb[2] + hb || pb[2] >= pa[2] + ha {
                    continue;
                }
                let d = dist2d(&pa, &pb);
                let ov = ra + rb - d;
                if ov <= 0.0 {
                    continue;
                }
                let dir = if d > 1e-3 { [(pb[0] - pa[0]) / d, (pb[1] - pa[1]) / d] } else { [1.0, 0.0] };
                let push = (ov * 0.5 + 0.01).min(1.0);
                for (k, sgn) in [(a, -1.0f32), (b, 1.0f32)] {
                    let m = &mut self.mechs[k];
                    let body = body_of(&m.ms);
                    let (dx, dy) = (dir[0] * push * sgn, dir[1] * push * sgn);
                    if let Some(z) = phys::try_move(&st.w, m.pos, dx, dy, &body, m.grounded) {
                        m.pos[0] += dx;
                        m.pos[1] += dy;
                        m.pos[2] = m.pos[2].max(z);
                    }
                }
            }
        }
    }

    // ------------------------------------------------------------------ NPCs

    fn update_npcs(&mut self) {
        let st = self.st.clone();
        let w = &st.w;
        let leash = self.cfg.npc("leash");
        let chase = self.cfg.npc("chase_leash");
        let aggro = self.cfg.npc("aggro_range");
        let deaggro = self.cfg.npc("deaggro_range");
        let speed = self.cfg.npc("speed");
        let nh = self.cfg.npc("height");
        let body = Body { r: self.cfg.npc("radius"), h: nh, step: 1.2, slope_tan: 1.19 };
        for k in 0..self.npcs.len() {
            if !self.npcs[k].alive {
                continue;
            }
            let pos = self.npcs[k].pos;
            let eye = [pos[0], pos[1], pos[2] + nh * 0.8];
            let home = self.npcs[k].home;
            let dh = dist2d(&pos, &home);
            if let Some(tg) = self.npcs[k].target {
                let m = &self.mechs[tg];
                if !m.alive || dist(m.pos, pos) > deaggro || dh > chase {
                    self.npcs[k].target = None;
                    self.npcs[k].returning = true;
                }
            }
            self.npcs[k].think -= DT;
            if self.npcs[k].target.is_none() && dh <= leash && self.npcs[k].think <= 0.0 {
                self.npcs[k].think = 0.5;
                let mut best: Option<(f32, usize)> = None;
                for (j, m) in self.mechs.iter().enumerate() {
                    if !m.alive {
                        continue;
                    }
                    let d = dist(m.pos, pos);
                    if d <= aggro && best.map(|b| d < b.0).unwrap_or(true) && w.los(eye, m.center()) {
                        best = Some((d, j));
                    }
                }
                if let Some((_, j)) = best {
                    self.npcs[k].target = Some(j);
                    self.npcs[k].returning = false;
                }
            }
            // Movement goal.
            let n = &mut self.npcs[k];
            let goal: [f32; 2];
            let mut spd = speed;
            if let Some(tg) = n.target {
                let tp = self.mechs[tg].pos;
                let d = dist2d(&tp, &pos);
                goal = if d > 18.0 { [tp[0], tp[1]] } else { [pos[0], pos[1]] };
                let c = self.mechs[tg].center();
                n.yaw = yaw_to(&eye, &c);
                n.pitch = (c[2] - eye[2]).atan2(dist2d(&eye, &c));
            } else if n.returning {
                goal = [home[0], home[1]];
                if dh < leash * 0.5 {
                    n.returning = false;
                }
            } else {
                if dist2d(&pos, &n.wander) < 2.0 || self.rng.f32() < 0.004 {
                    let o = self.rng.in_disc(leash);
                    n.wander = [home[0] + o[0], home[1] + o[1]];
                }
                goal = n.wander;
                spd *= 0.6;
            }
            let dx = goal[0] - pos[0];
            let dy = goal[1] - pos[1];
            let l = (dx * dx + dy * dy).sqrt();
            if l > 0.5 {
                n.vel = [dx / l * spd, dy / l * spd, 0.0];
                if n.target.is_none() {
                    n.yaw = dy.atan2(dx);
                    n.pitch = 0.0;
                }
            } else {
                n.vel = [0.0, 0.0, 0.0];
            }
            let (mx, my) = (n.vel[0] * DT, n.vel[1] * DT);
            if mx != 0.0 || my != 0.0 {
                match phys::try_move(w, n.pos, mx, my, &body, true) {
                    Some(z) => {
                        n.pos[0] += mx;
                        n.pos[1] += my;
                        n.pos[2] = z;
                    }
                    None => {
                        n.vel = [0.0; 3];
                        let o = self.rng.in_disc(leash);
                        n.wander = [home[0] + o[0], home[1] + o[1]];
                    }
                }
                let s = w.surface_below(n.pos[0], n.pos[1], n.pos[2], 0.05);
                if n.pos[2] - s <= 1.5 {
                    n.pos[2] = s;
                }
            }
            // Shooting.
            n.cooldown -= DT;
            if let Some(tg) = n.target {
                if n.cooldown <= 0.0 {
                    let c = self.mechs[tg].center();
                    let eye = [n.pos[0], n.pos[1], n.pos[2] + nh * 0.8];
                    if dist(c, eye) < deaggro && w.los(eye, c) {
                        n.cooldown = self.cfg.npc("cooldown");
                        let sp = self.cfg.npc("spread_deg").to_radians();
                        let yaw = yaw_to(&eye, &c) + self.rng.normal() * sp;
                        let pitch = (c[2] - eye[2]).atan2(dist2d(&eye, &c)) + self.rng.normal() * sp;
                        let d = dir_of(yaw, pitch);
                        let id = self.next_proj;
                        self.next_proj += 1;
                        let ps = self.cfg.npc("projectile_speed");
                        let origin = add(eye, scale(d, body.r + 0.5));
                        self.projs.push(Proj { id, kind: PK::Npc, owner: Src::Npc(k), team: None, pos: origin, vel: scale(d, ps), left: deaggro * 1.3, dmg: self.cfg.npc("damage"), radius: self.cfg.npc_proj_radius, mws: None, scale: 1.0, alive: true });
                        self.sounds.push(Sound { kind: "npc_gun", pos: eye, t: self.t + DT, src: None });
                    }
                }
            }
            if n.cooldown < 0.0 {
                n.cooldown = 0.0;
            }
        }
    }

    // ------------------------------------------------------------------ projectiles

    fn update_projs(&mut self) {
        let st = self.st.clone();
        let w = &st.w;
        let mut projs = std::mem::take(&mut self.projs);
        for p in projs.iter_mut() {
            if !p.alive {
                continue;
            }
            match p.kind {
                PK::Gun | PK::Npc => {
                    let sp = len(p.vel);
                    let seg = (sp * DT).min(p.left);
                    let d = scale(p.vel, 1.0 / sp.max(1e-6));
                    let dw = w.raycast(p.pos, d, seg);
                    let hits = self.ray_actors(p.pos, d, dw, p.owner, p.radius, true);
                    if let Some(&(t, tg)) = hits.first() {
                        let wpn = if p.kind == PK::Gun { "gun" } else { "npc" };
                        let from = sub(p.pos, scale(d, 5.0));
                        let _ = t;
                        if let (Src::Mech(a), Tgt::Mech(_), PK::Gun) = (p.owner, tg, p.kind) {
                            self.mechs[a].hits += 1;
                        }
                        self.hurt(tg, p.dmg, wpn, p.owner, from, None);
                        p.alive = false;
                    } else if dw < seg - 1e-3 {
                        p.alive = false;
                    } else {
                        p.pos = add(p.pos, scale(d, seg));
                        p.left -= seg;
                        if p.left <= 1e-3 {
                            p.alive = false;
                        }
                    }
                }
                PK::Mortar => {
                    let ws = p.mws.clone().unwrap();
                    p.vel[2] -= ws.gravity * DT;
                    let step = scale(p.vel, DT);
                    let sl = len(step);
                    let d = scale(step, 1.0 / sl.max(1e-6));
                    let dw = w.raycast(p.pos, d, sl);
                    let hits = self.ray_actors(p.pos, d, dw, p.owner, p.radius, true);
                    let prev = p.pos;
                    p.left -= DT;
                    if let Some(&(t, tg)) = hits.first() {
                        let hp = add(p.pos, scale(d, t));
                        p.alive = false;
                        self.explode(hp, prev, p.owner, p.team, Some(tg), &ws, p.scale);
                    } else if dw < sl - 1e-3 {
                        let hp = add(p.pos, scale(d, (dw - 0.2).max(0.0)));
                        p.alive = false;
                        self.explode(hp, prev, p.owner, p.team, None, &ws, p.scale);
                    } else {
                        p.pos = add(p.pos, step);
                        if p.left <= 0.0 {
                            p.alive = false;
                            let pp = p.pos;
                            self.explode(pp, prev, p.owner, p.team, None, &ws, p.scale);
                        }
                    }
                }
            }
        }
        projs.retain(|p| p.alive);
        projs.append(&mut self.projs);
        self.projs = projs;
    }

    // ------------------------------------------------------------------ cubes, drops, offers

    fn update_cubes(&mut self) {
        let r = self.cfg.cube_pickup_radius;
        let mut gets: Vec<usize> = vec![];
        for c in self.cubes.iter_mut() {
            if !c.alive {
                continue;
            }
            let mut best: Option<(f32, usize)> = None;
            for (j, m) in self.mechs.iter().enumerate() {
                if !m.alive {
                    continue;
                }
                let d = dist2d(&m.pos, &c.pos);
                if d <= r && (c.pos[2] - m.pos[2]).abs() <= 4.0 && best.map(|b| d < b.0).unwrap_or(true) {
                    best = Some((d, j));
                }
            }
            if let Some((_, j)) = best {
                c.alive = false;
                gets.push(j);
            }
        }
        for j in gets {
            self.give_cubes(j, 1, true);
        }
        self.cubes.retain(|c| c.alive);
    }

    fn update_drops(&mut self) {
        let Some(dc) = self.cfg.drops.clone() else { return };
        if self.drops_called < dc.times.len() && self.t >= dc.times[self.drops_called] {
            self.drops_called += 1;
            let z = self.zone_at(self.t);
            let p = self.random_ground_in(z.next_center, z.next_radius * dc.spread, 3000);
            let id = self.next_drop;
            self.next_drop += 1;
            self.drops.push(SupplyDrop { id, pos: p, land_t: self.t + dc.fall_s, holder: None, progress: 0.0, taken: false });
        }
        for k in 0..self.drops.len() {
            if self.drops[k].taken {
                continue;
            }
            let land = self.drops[k].land_t;
            if self.t < land {
                continue;
            }
            if self.t - DT < land {
                let p = self.drops[k].pos;
                self.sound("drop", p, None);
            }
            let p = self.drops[k].pos;
            let inside: Vec<usize> = self.mechs.iter().filter(|m| m.alive && dist2d(&m.pos, &p) <= dc.radius && (m.pos[2] - p[2]).abs() < 10.0).map(|m| m.id).collect();
            let d = &mut self.drops[k];
            match inside.len() {
                0 => {
                    d.holder = None;
                    d.progress = 0.0;
                }
                1 => {
                    if d.holder == Some(inside[0]) {
                        d.progress += DT;
                    } else {
                        d.holder = Some(inside[0]);
                        d.progress = DT;
                    }
                }
                _ => {}
            }
            if d.progress >= dc.capture_s - 1e-4 {
                if let Some(h) = d.holder {
                    d.taken = true;
                    let id = d.id;
                    self.give_cubes(h, dc.cubes, false);
                    let m = &mut self.mechs[h];
                    if dc.repair > 0.0 {
                        m.hp = m.max_hp;
                        m.energy = m.ms.energy;
                    }
                    let c = m.cubes;
                    self.ev(h, format!(r#"{{"type":"drop","id":{id},"cubes":{c}}}"#));
                }
            }
        }
        self.drops.retain(|d| !d.taken);
    }

    // ------------------------------------------------------------------ deaths

    fn resolve_deaths(&mut self) {
        let dead: Vec<usize> = self.mechs.iter().filter(|m| m.alive && m.hp <= 0.0).map(|m| m.id).collect();
        for k in 0..self.npcs.len() {
            if self.npcs[k].alive && self.npcs[k].hp <= 0.0 {
                self.npcs[k].alive = false;
                let p = self.npcs[k].pos;
                for _ in 0..(self.cfg.npc("cubes") as u32) {
                    let q = self.scatter(p, 1.5);
                    self.spawn_cube(q);
                }
            }
        }
        if dead.is_empty() {
            return;
        }
        let place = self.alive_count() as u32;
        for &i in &dead {
            let t = self.t;
            let m = &mut self.mechs[i];
            m.alive = false;
            m.hp = 0.0;
            m.place = place;
            m.death_t = t;
            m.cause = match m.last_hit_by {
                Some(Src::Zone) => "zone",
                Some(Src::Npc(_)) => "npc",
                _ => m.last_hit_weapon,
            };
            m.killer = match m.last_hit_by {
                Some(Src::Mech(k)) if k != i => Some(k),
                _ => None,
            };
            let p = m.pos;
            let n = m.cubes + self.cfg.death_drop_extra;
            let weps: Vec<Weapon> = m.slots.iter_mut().filter_map(|s| s.take()).filter(|w| w.kind != WK::Gun).collect();
            for _ in 0..n {
                let q = self.scatter(p, 3.0);
                self.spawn_cube(q);
            }
            for mut w in weps {
                w.charging = false;
                w.charge = 0.0;
                let q = self.scatter(p, 2.0);
                self.spawn_pickup(q, w);
            }
            self.sound("death", p, Some(i));
            if let Some(k) = self.mechs[i].killer {
                self.mechs[k].kills += 1;
                self.ev(k, format!(r#"{{"type":"kill","target":{i}}}"#));
            }
        }
        let alive = self.alive_count();
        if alive <= 1 {
            for m in self.mechs.iter_mut().filter(|m| m.alive) {
                m.place = 1;
            }
            self.over = true;
        }
    }

    /// Ranks survivors by HP when the clock runs out.
    pub fn finish_timeout(&mut self) {
        let hps: Vec<f32> = self.mechs.iter().filter(|m| m.alive).map(|m| m.hp).collect();
        for m in self.mechs.iter_mut().filter(|m| m.alive) {
            m.place = 1 + hps.iter().filter(|&&h| h > m.hp).count() as u32;
        }
        self.over = true;
    }

    // ------------------------------------------------------------------ tick

    pub fn step(&mut self) {
        self.t = self.tick as f32 * DT;
        let t = self.t;
        let z = self.zone_at(t);
        let n = self.mechs.len();
        for i in 0..n {
            if self.mechs[i].alive {
                self.mech_move(i);
            }
        }
        self.push_apart();
        for i in 0..n {
            if self.mechs[i].alive {
                self.mech_weapons(i);
            }
        }
        for i in 0..n {
            if !self.mechs[i].alive {
                continue;
            }
            let inz = dist2d(&self.mechs[i].pos, &z.center) <= z.radius;
            self.mechs[i].in_zone = inz;
            if !inz {
                let p = self.mechs[i].center();
                self.hurt(Tgt::Mech(i), z.dps * DT, "zone", Src::Zone, p, None);
            }
            let m = &mut self.mechs[i];
            if inz && t - m.last_combat >= m.ms.regen_delay && m.hp < m.max_hp {
                m.hp = (m.hp + m.ms.regen_per_s * DT).min(m.max_hp);
            }
            if t - m.last_spend >= m.ms.energy_regen_delay && m.energy < m.ms.energy {
                m.energy = (m.energy + m.ms.energy_regen * DT).min(m.ms.energy);
            }
            if m.offer.is_none() && m.pending_offers > 0 {
                m.pending_offers -= 1;
                self.draw_offer(i);
            }
            let exp = self.mechs[i].offer.as_ref().map(|o| t >= o.expires).unwrap_or(false);
            if exp {
                let u = self.mechs[i].offer.as_ref().unwrap().choices[0];
                self.mechs[i].offer = None;
                self.apply_upgrade(i, u);
            }
        }
        self.update_npcs();
        self.update_projs();
        self.update_cubes();
        self.update_drops();
        self.resolve_deaths();
        for m in self.mechs.iter_mut() {
            if m.alive {
                m.hist.push_back((m.pos, m.vel));
                if m.hist.len() > HIST {
                    m.hist.pop_front();
                }
            }
        }
        self.sounds.retain(|s| t - s.t < 1.0);
        self.beams.retain(|b| t - b.t < 0.5);
        self.tick += 1;
        if !self.over && self.tick as f32 * DT >= self.cfg.duration_s {
            self.t = self.tick as f32 * DT;
            self.finish_timeout();
        }
    }
}
