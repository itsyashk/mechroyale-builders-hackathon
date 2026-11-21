//! Rule set: the `config` object of `hello`, kept verbatim for bots and parsed into typed
//! numbers for the engine.

use serde_json::Value;

pub const DEFAULT_CONFIG: &str = include_str!("../config.json");

#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub enum WK {
    Gun = 0,
    Mortar = 1,
    Laser = 2,
    Lightning = 3,
}

impl WK {
    pub const ALL: [WK; 4] = [WK::Gun, WK::Mortar, WK::Laser, WK::Lightning];
    pub fn name(self) -> &'static str {
        match self {
            WK::Gun => "gun",
            WK::Mortar => "mortar",
            WK::Laser => "laser",
            WK::Lightning => "lightning",
        }
    }
    pub fn parse(s: &str) -> Option<WK> {
        match s {
            "gun" => Some(WK::Gun),
            "mortar" => Some(WK::Mortar),
            "laser" => Some(WK::Laser),
            "lightning" => Some(WK::Lightning),
            _ => None,
        }
    }
    #[inline]
    pub fn idx(self) -> usize {
        self as usize
    }
}

pub type KV = Vec<(String, f32)>;

pub fn kv_of(v: &Value) -> KV {
    let mut out = Vec::new();
    if let Some(m) = v.as_object() {
        for (k, x) in m {
            if let Some(f) = x.as_f64() {
                out.push((k.clone(), f as f32));
            }
        }
    }
    out
}

pub fn kv_get(kv: &KV, k: &str) -> f32 {
    kv.iter().find(|(n, _)| n == k).map(|(_, v)| *v).unwrap_or(0.0)
}

pub fn kv_set(kv: &mut KV, k: &str, v: f32) {
    if let Some(e) = kv.iter_mut().find(|(n, _)| n == k) {
        e.1 = v;
    } else {
        kv.push((k.to_string(), v));
    }
}

/// Typed mech numbers (after upgrades and cube scaling).
#[derive(Clone, Default, Debug)]
pub struct MS {
    pub radius: f32,
    pub height: f32,
    pub eye: f32,
    pub muzzle: f32,
    pub hp: f32,
    pub walk: f32,
    pub boost: f32,
    pub ground_accel: f32,
    pub air_accel: f32,
    pub gravity: f32,
    pub jump: f32,
    pub jet_accel: f32,
    pub max_rise: f32,
    pub step_up: f32,
    pub max_slope_deg: f32,
    pub energy: f32,
    pub energy_regen: f32,
    pub energy_regen_delay: f32,
    pub boost_drain: f32,
    pub jet_drain: f32,
    pub turn_rate: f32,
    pub pitch_rate: f32,
    pub min_pitch: f32,
    pub max_pitch: f32,
    pub regen_delay: f32,
    pub regen_per_s: f32,
    pub charge_move_mult: f32,
    pub self_damage_mult: f32,
    pub armor: f32,
    pub rear_mult: f32,
    pub front_arc_deg: f32,
    pub near_bonus: f32,
    pub vision_bonus: f32,
    pub hearing: f32,
}

impl MS {
    pub fn from_kv(kv: &KV, size: f32) -> MS {
        let g = |k: &str| kv_get(kv, k);
        MS {
            radius: g("radius") * size,
            height: g("height") * size,
            eye: g("eye_height") * size,
            muzzle: g("muzzle_height") * size,
            hp: g("hp"),
            walk: g("walk_speed"),
            boost: g("boost_speed"),
            ground_accel: g("ground_accel"),
            air_accel: g("air_accel"),
            gravity: g("gravity"),
            jump: g("jump_speed"),
            jet_accel: g("jet_accel"),
            max_rise: g("max_rise_speed"),
            step_up: g("step_up"),
            max_slope_deg: g("max_slope_deg"),
            energy: g("energy"),
            energy_regen: g("energy_regen"),
            energy_regen_delay: g("energy_regen_delay"),
            boost_drain: g("boost_drain"),
            jet_drain: g("jet_drain"),
            turn_rate: g("turn_rate"),
            pitch_rate: g("pitch_rate"),
            min_pitch: g("min_pitch"),
            max_pitch: g("max_pitch"),
            regen_delay: g("regen_delay"),
            regen_per_s: g("regen_per_s"),
            charge_move_mult: g("charge_move_mult"),
            self_damage_mult: g("self_damage_mult"),
            armor: g("armor"),
            rear_mult: g("rear_mult"),
            front_arc_deg: g("front_arc_deg"),
            near_bonus: g("near_range_bonus"),
            vision_bonus: g("vision_bonus"),
            hearing: g("hearing"),
        }
    }
}

/// Typed weapon numbers (union of all kinds' fields; missing ones are 0).
#[derive(Clone, Default, Debug)]
pub struct WS {
    pub damage: f32,
    pub cooldown: f32,
    pub speed: f32,
    pub range: f32,
    pub spread_deg: f32,
    pub mag: f32,
    pub reload: f32,
    pub splash: f32,
    pub splash_radius: f32,
    pub splash_edge: f32,
    pub gravity: f32,
    pub ttl: f32,
    pub knock: f32,
    pub knock_up: f32,
    pub drain: f32,
    pub charge: f32,
    pub min_charge: f32,
    pub min_frac: f32,
    pub hold_s: f32,
    pub pierce: f32,
    pub charge_turn_mult: f32,
    pub cookoff: f32,
    pub cone_deg: f32,
    pub chain: f32,
    pub chain_range: f32,
    pub chain_falloff: f32,
    pub energy: f32,
    pub short_s: f32,
}

impl WS {
    pub fn from_kv(kv: &KV) -> WS {
        let g = |k: &str| kv_get(kv, k);
        WS {
            damage: g("damage"),
            cooldown: g("cooldown"),
            speed: g("speed"),
            range: g("range"),
            spread_deg: g("spread_deg"),
            mag: g("mag"),
            reload: g("reload"),
            splash: g("splash"),
            splash_radius: g("splash_radius"),
            splash_edge: g("splash_edge"),
            gravity: g("gravity"),
            ttl: g("ttl"),
            knock: g("knock"),
            knock_up: g("knock_up"),
            drain: g("drain"),
            charge: g("charge"),
            min_charge: g("min_charge"),
            min_frac: g("min_frac"),
            hold_s: g("hold_s"),
            pierce: g("pierce"),
            charge_turn_mult: g("charge_turn_mult"),
            cookoff: g("cookoff"),
            cone_deg: g("cone_deg"),
            chain: g("chain"),
            chain_range: g("chain_range"),
            chain_falloff: g("chain_falloff"),
            energy: g("energy"),
            short_s: g("short_s"),
        }
    }
}

#[derive(Clone, Debug)]
pub struct Effect {
    pub path: String,
    pub mul: Option<f32>,
    pub add: Option<f32>,
}

#[derive(Clone, Debug)]
pub struct UpgradeDef {
    pub id: String,
    pub name: String,
    pub desc: String,
    pub requires: Option<WK>,
    pub max: u32,
    pub effects: Vec<Effect>,
}

#[derive(Clone, Debug)]
pub struct Phase {
    pub hold_s: f32,
    pub shrink_s: f32,
    pub radius: f32,
    pub dps: f32,
}

#[derive(Clone, Debug)]
#[allow(dead_code)]
pub struct Cfg {
    pub raw_text: String,
    pub tick_hz: f32,
    pub decide_every: u32,
    pub duration_s: f32,
    pub mechs: usize,
    pub spawn_min_sep: f32,
    pub spawn_zone_frac: f32,
    pub arena_margin: f32,
    pub tick_budget_ms: f32,
    pub hello_budget_ms: f32,
    pub coast_s: f32,
    pub offer_timeout_s: f32,
    pub max_line_bytes: usize,
    pub mech_kv: KV,
    pub vis_range: f32,
    pub vis_fov_deg: f32,
    pub vis_near: f32,
    pub canopy_blocks: bool,
    pub show_enemy_hp: bool,
    pub memory_s: f32,
    pub bearing_noise_deg: f32,
    pub snd_near: f32,
    pub snd_mid: f32,
    pub step_speed: f32,
    pub step_every: f32,
    pub snd_range: KV,
    pub weapons_kv: [KV; 4],
    pub gun_radius: f32,
    pub npc_proj_radius: f32,
    pub cube_pickup_radius: f32,
    pub hp_per_cube: f32,
    pub damage_per_cube: f32,
    pub stat_cap: f32,
    pub death_drop_extra: u32,
    pub size_per_cube: f32,
    pub thresholds: Vec<u32>,
    pub crate_count: usize,
    pub crate_roof_fraction: f32,
    pub crate_hp: f32,
    pub crate_radius: f32,
    pub crate_height: f32,
    pub crate_cubes: u32,
    pub loot_pickup_radius: f32,
    pub equip_s: f32,
    pub floor_count: usize,
    pub floor_roof_fraction: f32,
    pub crate_weapon_chance: f32,
    pub loot_weights: [f32; 4],
    pub npc: KV,
    pub drops: Option<Drops>,
    pub zone_center: [f32; 2],
    pub zone_radius: f32,
    pub phases: Vec<Phase>,
    pub upgrades: Vec<UpgradeDef>,
}

#[derive(Clone, Debug)]
pub struct Drops {
    pub times: Vec<f32>,
    pub fall_s: f32,
    pub radius: f32,
    pub capture_s: f32,
    pub cubes: u32,
    pub repair: f32,
    pub spread: f32,
}

fn f(v: &Value, k: &str, d: f32) -> f32 {
    v.get(k).and_then(|x| x.as_f64()).map(|x| x as f32).unwrap_or(d)
}

impl Cfg {
    pub fn parse(text: &str) -> Result<Cfg, String> {
        let v: Value = serde_json::from_str(text).map_err(|e| format!("config: {e}"))?;
        let m = &v["match"];
        let b = &v["bot"];
        let vi = &v["vision"];
        let so = &v["sound"];
        let w = &v["weapons"];
        let cu = &v["cubes"];
        let cr = &v["crates"];
        let lo = &v["loot"];
        let z = &v["zone"];
        let weights = &lo["weights"];
        let drops = if v.get("drops").map(|d| d.is_object()).unwrap_or(false) {
            let d = &v["drops"];
            Some(Drops {
                times: d["times"].as_array().map(|a| a.iter().filter_map(|x| x.as_f64()).map(|x| x as f32).collect()).unwrap_or_default(),
                fall_s: f(d, "fall_s", 25.0),
                radius: f(d, "radius", 8.0),
                capture_s: f(d, "capture_s", 4.0),
                cubes: f(d, "cubes", 3.0) as u32,
                repair: f(d, "repair", 1.0),
                spread: f(d, "spread", 0.7),
            })
        } else {
            None
        };
        let mut upgrades = Vec::new();
        for u in v["upgrades"].as_array().cloned().unwrap_or_default() {
            upgrades.push(UpgradeDef {
                id: u["id"].as_str().unwrap_or("").to_string(),
                name: u["name"].as_str().unwrap_or("").to_string(),
                desc: u["desc"].as_str().unwrap_or("").to_string(),
                requires: u.get("requires").and_then(|r| r.as_str()).and_then(WK::parse),
                max: u["max"].as_f64().unwrap_or(1.0) as u32,
                effects: u["effects"]
                    .as_array()
                    .cloned()
                    .unwrap_or_default()
                    .iter()
                    .map(|e| Effect {
                        path: e["path"].as_str().unwrap_or("").to_string(),
                        mul: e.get("mul").and_then(|x| x.as_f64()).map(|x| x as f32),
                        add: e.get("add").and_then(|x| x.as_f64()).map(|x| x as f32),
                    })
                    .collect(),
            });
        }
        let phases = z["phases"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|p| Phase { hold_s: f(p, "hold_s", 30.0), shrink_s: f(p, "shrink_s", 30.0), radius: f(p, "radius", 0.0), dps: f(p, "dps", 10.0) })
            .collect();
        let sc = z["start_center"].as_array().cloned().unwrap_or_default();
        Ok(Cfg {
            raw_text: text.trim().to_string(),
            tick_hz: f(m, "tick_hz", 20.0),
            decide_every: f(m, "decide_every", 2.0) as u32,
            duration_s: f(m, "duration_s", 480.0),
            mechs: f(m, "mechs", 16.0) as usize,
            spawn_min_sep: f(m, "spawn_min_separation", 130.0),
            spawn_zone_frac: f(m, "spawn_zone_fraction", 0.8),
            arena_margin: f(m, "arena_margin", 30.0),
            tick_budget_ms: f(b, "tick_budget_ms", 50.0),
            hello_budget_ms: f(b, "hello_budget_ms", 20000.0),
            coast_s: f(b, "coast_s", 1.0),
            offer_timeout_s: f(b, "offer_timeout_s", 5.0),
            max_line_bytes: f(b, "max_line_bytes", 65536.0) as usize,
            mech_kv: kv_of(&v["mech"]),
            vis_range: f(vi, "range", 200.0),
            vis_fov_deg: f(vi, "fov_deg", 120.0),
            vis_near: f(vi, "near_range", 45.0),
            canopy_blocks: vi.get("canopy_blocks").and_then(|x| x.as_bool()).unwrap_or(true),
            show_enemy_hp: vi.get("show_enemy_hp").and_then(|x| x.as_bool()).unwrap_or(true),
            memory_s: f(vi, "memory_s", 5.0),
            bearing_noise_deg: f(so, "bearing_noise_deg", 7.0),
            snd_near: f(so, "near", 60.0),
            snd_mid: f(so, "mid", 160.0),
            step_speed: f(so, "step_speed", 5.0),
            step_every: f(so, "step_every", 0.6),
            snd_range: kv_of(&so["range"]),
            weapons_kv: [kv_of(&w["gun"]), kv_of(&w["mortar"]), kv_of(&w["laser"]), kv_of(&w["lightning"])],
            gun_radius: f(&v["projectile"], "gun_radius", 0.3),
            npc_proj_radius: f(&v["projectile"], "npc_radius", 0.3),
            cube_pickup_radius: f(cu, "pickup_radius", 3.5),
            hp_per_cube: f(cu, "hp_per_cube", 0.04),
            damage_per_cube: f(cu, "damage_per_cube", 0.04),
            stat_cap: f(cu, "stat_cap", 10.0),
            death_drop_extra: f(cu, "death_drop_extra", 2.0) as u32,
            size_per_cube: f(cu, "size_per_cube", 0.05),
            thresholds: cu["upgrade_thresholds"].as_array().map(|a| a.iter().filter_map(|x| x.as_f64()).map(|x| x as u32).collect()).unwrap_or_default(),
            crate_count: f(cr, "count", 36.0) as usize,
            crate_roof_fraction: f(cr, "roof_fraction", 0.25),
            crate_hp: f(cr, "hp", 300.0),
            crate_radius: f(cr, "radius", 1.4),
            crate_height: f(cr, "height", 2.4),
            crate_cubes: f(cr, "cubes", 1.0) as u32,
            loot_pickup_radius: f(lo, "pickup_radius", 4.0),
            equip_s: f(lo, "equip_s", 1.0),
            floor_count: f(lo, "floor_count", 40.0) as usize,
            floor_roof_fraction: f(lo, "floor_roof_fraction", 0.25),
            crate_weapon_chance: f(lo, "crate_weapon_chance", 0.5),
            loot_weights: [f(weights, "gun", 0.0), f(weights, "mortar", 1.0), f(weights, "laser", 1.0), f(weights, "lightning", 1.0)],
            npc: kv_of(&v["npc"]),
            drops,
            zone_center: [sc.first().and_then(|x| x.as_f64()).unwrap_or(0.0) as f32, sc.get(1).and_then(|x| x.as_f64()).unwrap_or(0.0) as f32],
            zone_radius: f(z, "start_radius", 560.0),
            phases,
            upgrades,
        })
    }

    pub fn snd_range(&self, kind: &str) -> f32 {
        kv_get(&self.snd_range, kind)
    }
    pub fn npc(&self, k: &str) -> f32 {
        kv_get(&self.npc, k)
    }
}
