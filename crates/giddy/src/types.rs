//! Port of mech/types.go: protocol messages. Decoding is lenient in the same places Go's
//! encoding/json is (missing fields and JSON null leave the zero value).
#![allow(dead_code)]

use std::collections::HashMap;

use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;

pub type Vec3 = [f64; 3];

// ---------------------------------------------------------------- lenient decoders
/// JSON number (or null/missing) into a Go `int`.
pub fn de_int<'de, D: Deserializer<'de>>(d: D) -> Result<i64, D::Error> {
    Ok(value_int(&Value::deserialize(d)?))
}

pub fn value_int(v: &Value) -> i64 {
    match v {
        Value::Number(n) => n
            .as_i64()
            .or_else(|| n.as_u64().map(|u| u as i64))
            .unwrap_or_else(|| n.as_f64().unwrap_or(0.0) as i64),
        _ => 0,
    }
}

/// JSON number or null into a Go `*int`.
pub fn de_opt_int<'de, D: Deserializer<'de>>(d: D) -> Result<Option<i64>, D::Error> {
    Ok(match Value::deserialize(d)? {
        Value::Null => None,
        v => Some(value_int(&v)),
    })
}

/// JSON number (or null) into a Go `float64`.
pub fn de_num<'de, D: Deserializer<'de>>(d: D) -> Result<f64, D::Error> {
    Ok(match Value::deserialize(d)? {
        Value::Number(n) => n.as_f64().unwrap_or(0.0),
        _ => 0.0,
    })
}

/// JSON string or null into a Go `string`.
pub fn de_str<'de, D: Deserializer<'de>>(d: D) -> Result<String, D::Error> {
    Ok(match Value::deserialize(d)? {
        Value::String(s) => s,
        _ => String::new(),
    })
}

/// JSON array of strings (null entries become "") or null into a Go `[]string`.
pub fn de_strs<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<String>, D::Error> {
    Ok(match Value::deserialize(d)? {
        Value::Array(a) => a
            .into_iter()
            .map(|v| match v {
                Value::String(s) => s,
                _ => String::new(),
            })
            .collect(),
        _ => Vec::new(),
    })
}

/// null into the zero value (Go leaves slices/structs untouched on null).
pub fn de_null<'de, D: Deserializer<'de>, T: Deserialize<'de> + Default>(d: D) -> Result<T, D::Error> {
    Ok(Option::<T>::deserialize(d)?.unwrap_or_default())
}

// ---------------------------------------------------------------- config
#[derive(Deserialize, Clone, Debug, Default)]
#[serde(default)]
pub struct MechStats {
    pub radius: f64,
    pub height: f64,
    pub eye_height: f64,
    pub muzzle_height: f64,
    pub hp: f64,
    pub walk_speed: f64,
    pub boost_speed: f64,
    pub ground_accel: f64,
    pub air_accel: f64,
    pub gravity: f64,
    pub jump_speed: f64,
    pub jet_accel: f64,
    pub max_rise_speed: f64,
    pub step_up: f64,
    pub max_slope_deg: f64,
    pub energy: f64,
    pub energy_regen: f64,
    pub energy_regen_delay: f64,
    pub boost_drain: f64,
    pub jet_drain: f64,
    pub turn_rate: f64,
    pub pitch_rate: f64,
    pub min_pitch: f64,
    pub max_pitch: f64,
    pub regen_delay: f64,
    pub regen_per_s: f64,
    pub charge_move_mult: f64,
    pub self_damage_mult: f64,
    pub armor: f64,
    pub rear_mult: f64,
    pub front_arc_deg: f64,
    pub near_range_bonus: f64,
    pub vision_bonus: f64,
    pub hearing: f64,
}

#[derive(Deserialize, Clone, Debug, Default)]
#[serde(default)]
pub struct WeaponStats {
    pub damage: f64,
    pub cooldown: f64,
    pub speed: f64,
    pub range: f64,
    pub spread_deg: f64,
    pub mag: f64,
    pub reload: f64,
    pub splash: f64,
    pub splash_radius: f64,
    pub splash_edge: f64,
    pub gravity: f64,
    pub ttl: f64,
    pub knock: f64,
    pub knock_up: f64,
    pub drain: f64,
    pub charge: f64,
    pub min_charge: f64,
    pub min_frac: f64,
    pub hold_s: f64,
    pub pierce: f64,
    pub cookoff: f64,
    pub cone_deg: f64,
    pub chain: f64,
    pub chain_range: f64,
    pub chain_falloff: f64,
    pub energy: f64,
    pub short_s: f64,
}

#[derive(Deserialize, Clone, Debug, Default)]
#[serde(default)]
pub struct ZonePhase {
    pub hold_s: f64,
    pub shrink_s: f64,
    pub radius: f64,
    pub dps: f64,
}

#[derive(Deserialize, Clone, Debug, Default)]
#[serde(default)]
pub struct UpgradeEffect {
    #[serde(deserialize_with = "de_str")]
    pub path: String,
    pub mul: Option<f64>,
    pub add: Option<f64>,
}

#[derive(Deserialize, Clone, Debug, Default)]
#[serde(default)]
pub struct Upgrade {
    #[serde(deserialize_with = "de_str")]
    pub id: String,
    #[serde(deserialize_with = "de_str")]
    pub name: String,
    #[serde(deserialize_with = "de_str")]
    pub desc: String,
    #[serde(deserialize_with = "de_str")]
    pub requires: String,
    #[serde(deserialize_with = "de_int")]
    pub max: i64,
    #[serde(deserialize_with = "de_null")]
    pub effects: Vec<UpgradeEffect>,
}

#[derive(Deserialize, Clone, Debug, Default)]
#[serde(default)]
pub struct MatchCfg {
    pub tick_hz: f64,
    #[serde(deserialize_with = "de_int")]
    pub decide_every: i64,
    pub duration_s: f64,
}

#[derive(Deserialize, Clone, Debug, Default)]
#[serde(default)]
pub struct BotCfg {
    pub tick_budget_ms: f64,
}

#[derive(Deserialize, Clone, Debug, Default)]
#[serde(default)]
pub struct VisionCfg {
    pub range: f64,
    pub fov_deg: f64,
    pub near_range: f64,
    pub memory_s: f64,
}

#[derive(Deserialize, Clone, Debug, Default)]
#[serde(default)]
pub struct CubesCfg {
    pub pickup_radius: f64,
    pub hp_per_cube: f64,
    pub damage_per_cube: f64,
    #[serde(deserialize_with = "de_int")]
    pub stat_cap: i64,
    #[serde(deserialize_with = "de_int")]
    pub death_drop_extra: i64,
    pub size_per_cube: f64,
    #[serde(deserialize_with = "de_null")]
    pub upgrade_thresholds: Vec<f64>,
}

#[derive(Deserialize, Clone, Debug, Default)]
#[serde(default)]
pub struct CratesCfg {
    #[serde(deserialize_with = "de_int")]
    pub count: i64,
    pub hp: f64,
    pub radius: f64,
    pub height: f64,
}

#[derive(Deserialize, Clone, Debug, Default)]
#[serde(default)]
pub struct LootCfg {
    pub pickup_radius: f64,
    pub equip_s: f64,
    #[serde(deserialize_with = "de_int")]
    pub floor_count: i64,
    pub crate_weapon_chance: f64,
}

#[derive(Deserialize, Clone, Debug, Default)]
#[serde(default)]
pub struct NpcCfg {
    #[serde(deserialize_with = "de_int")]
    pub count: i64,
    pub hp: f64,
    pub radius: f64,
    pub height: f64,
    pub aggro_range: f64,
    pub deaggro_range: f64,
    pub damage: f64,
    pub cooldown: f64,
    pub projectile_speed: f64,
    pub spread_deg: f64,
    #[serde(deserialize_with = "de_int")]
    pub cubes: i64,
}

#[derive(Deserialize, Clone, Debug, Default)]
#[serde(default)]
pub struct DropsCfg {
    #[serde(deserialize_with = "de_null")]
    pub times: Vec<f64>,
    pub fall_s: f64,
    pub radius: f64,
    pub capture_s: f64,
    #[serde(deserialize_with = "de_int")]
    pub cubes: i64,
}

#[derive(Deserialize, Clone, Debug, Default)]
#[serde(default)]
pub struct ZoneCfg {
    pub start_center: [f64; 2],
    pub start_radius: f64,
    #[serde(deserialize_with = "de_null")]
    pub phases: Vec<ZonePhase>,
}

#[derive(Deserialize, Clone, Debug, Default)]
#[serde(default)]
pub struct Config {
    #[serde(rename = "match")]
    pub match_: MatchCfg,
    pub bot: BotCfg,
    pub mech: MechStats,
    pub vision: VisionCfg,
    #[serde(deserialize_with = "de_null")]
    pub weapons: HashMap<String, WeaponStats>,
    pub cubes: CubesCfg,
    pub crates: CratesCfg,
    pub loot: LootCfg,
    pub npc: NpcCfg,
    pub drops: DropsCfg,
    pub zone: ZoneCfg,
    #[serde(deserialize_with = "de_null")]
    pub upgrades: Vec<Upgrade>,
}

#[derive(Deserialize, Clone, Debug, Default)]
#[serde(default)]
pub struct MapRef {
    #[serde(deserialize_with = "de_str")]
    pub dir: String,
}

#[derive(Deserialize, Clone, Debug, Default)]
#[serde(default)]
pub struct Hello {
    #[serde(rename = "type", deserialize_with = "de_str")]
    pub kind: String,
    #[serde(deserialize_with = "de_int")]
    pub slot: i64,
    /// Go: `Seed uint64`, used as `int64(h.Seed)`.
    #[serde(deserialize_with = "de_int")]
    pub seed: i64,
    pub map: MapRef,
    pub config: Config,
}

// ---------------------------------------------------------------- observation
#[derive(Deserialize, Clone, Debug, Default)]
#[serde(default)]
pub struct WeaponState {
    #[serde(deserialize_with = "de_str")]
    pub kind: String,
    pub cooldown: f64,
    pub ammo: f64,
    pub reload: f64,
    pub charge: f64,
    pub stats: WeaponStats,
}

#[derive(Deserialize, Clone, Debug, Default)]
#[serde(default)]
pub struct SelfObs {
    #[serde(deserialize_with = "de_int")]
    pub id: i64,
    pub pos: Vec3,
    pub vel: Vec3,
    pub aim: [f64; 2],
    pub hp: f64,
    pub max_hp: f64,
    pub energy: f64,
    pub max_energy: f64,
    pub grounded: bool,
    #[serde(deserialize_with = "de_int")]
    pub cubes: i64,
    #[serde(deserialize_with = "de_strs")]
    pub upgrades: Vec<String>,
    pub in_zone: bool,
    /// Go `[]*WeaponState`: None = empty slot.
    #[serde(deserialize_with = "de_null")]
    pub weapons: Vec<Option<WeaponState>>,
    pub stats: MechStats,
}

#[derive(Deserialize, Clone, Debug, Default)]
#[serde(default)]
pub struct MechSeen {
    #[serde(deserialize_with = "de_int")]
    pub id: i64,
    #[serde(deserialize_with = "de_str")]
    pub kind: String,
    pub pos: Vec3,
    pub vel: Vec3,
    pub aim: [f64; 2],
    pub hp: f64,
    pub max_hp: f64,
    #[serde(deserialize_with = "de_int")]
    pub cubes: i64,
    #[serde(deserialize_with = "de_strs")]
    pub weapons: Vec<String>,
    pub charging: bool,
}

#[derive(Deserialize, Clone, Debug, Default)]
#[serde(default)]
pub struct Proj {
    #[serde(deserialize_with = "de_int")]
    pub id: i64,
    #[serde(deserialize_with = "de_str")]
    pub kind: String,
    pub pos: Vec3,
    pub vel: Vec3,
}

#[derive(Deserialize, Clone, Debug, Default)]
#[serde(default)]
pub struct Item {
    #[serde(deserialize_with = "de_int")]
    pub id: i64,
    #[serde(deserialize_with = "de_str")]
    pub kind: String,
    pub pos: Vec3,
    #[serde(deserialize_with = "de_num")]
    pub hp: f64,
}

#[derive(Deserialize, Clone, Debug, Default)]
#[serde(default)]
pub struct Sound {
    #[serde(deserialize_with = "de_str")]
    pub kind: String,
    #[serde(deserialize_with = "de_num")]
    pub bearing: f64,
    #[serde(deserialize_with = "de_str")]
    pub dist: String,
    #[serde(deserialize_with = "de_num")]
    pub age: f64,
}

#[derive(Deserialize, Clone, Debug, Default)]
#[serde(default)]
pub struct Event {
    #[serde(rename = "type", deserialize_with = "de_str")]
    pub kind_: String,
    #[serde(deserialize_with = "de_num")]
    pub amount: f64,
    #[serde(deserialize_with = "de_str")]
    pub weapon: String,
    pub bearing: Option<f64>,
    #[serde(deserialize_with = "de_opt_int")]
    pub by: Option<i64>,
    pub rear: bool,
    #[serde(deserialize_with = "de_int")]
    pub target: i64,
    #[serde(deserialize_with = "de_str")]
    pub effect: String,
    /// Go: json.RawMessage
    pub id: Value,
    #[serde(deserialize_with = "de_str")]
    pub kind: String,
    #[serde(deserialize_with = "de_str")]
    pub reason: String,
    #[serde(deserialize_with = "de_int")]
    pub cubes: i64,
}

#[derive(Deserialize, Clone, Debug, Default)]
#[serde(default)]
pub struct Drop {
    #[serde(deserialize_with = "de_int")]
    pub id: i64,
    pub pos: Vec3,
    #[serde(deserialize_with = "de_num")]
    pub radius: f64,
    #[serde(deserialize_with = "de_num")]
    pub lands_in: f64,
    #[serde(deserialize_with = "de_num")]
    pub capture_s: f64,
    #[serde(deserialize_with = "de_num")]
    pub progress: f64,
    #[serde(deserialize_with = "de_opt_int")]
    pub holder: Option<i64>,
}

#[derive(Deserialize, Clone, Debug, Default)]
#[serde(default)]
pub struct Zone {
    pub center: [f64; 2],
    pub radius: f64,
    pub next_center: [f64; 2],
    pub next_radius: f64,
    pub shrink_start: f64,
    pub shrink_end: f64,
    pub dps: f64,
}

#[derive(Deserialize, Clone, Debug, Default)]
#[serde(default)]
pub struct OfferChoice {
    #[serde(deserialize_with = "de_str")]
    pub id: String,
    #[serde(deserialize_with = "de_str")]
    pub name: String,
    #[serde(deserialize_with = "de_str")]
    pub desc: String,
}

#[derive(Deserialize, Clone, Debug, Default)]
#[serde(default)]
pub struct Offer {
    #[serde(deserialize_with = "de_int")]
    pub id: i64,
    pub expires: f64,
    #[serde(deserialize_with = "de_null")]
    pub choices: Vec<OfferChoice>,
}

#[derive(Deserialize, Clone, Debug, Default)]
#[serde(default)]
pub struct Obs {
    #[serde(rename = "type", deserialize_with = "de_str")]
    pub kind: String,
    #[serde(deserialize_with = "de_int")]
    pub tick: i64,
    pub t: f64,
    #[serde(deserialize_with = "de_int")]
    pub alive: i64,
    #[serde(rename = "self", deserialize_with = "de_null")]
    pub self_: SelfObs,
    #[serde(deserialize_with = "de_null")]
    pub mechs: Vec<MechSeen>,
    #[serde(deserialize_with = "de_null")]
    pub projectiles: Vec<Proj>,
    #[serde(deserialize_with = "de_null")]
    pub crates: Vec<Item>,
    #[serde(deserialize_with = "de_null")]
    pub cubes: Vec<Item>,
    #[serde(deserialize_with = "de_null")]
    pub pickups: Vec<Item>,
    #[serde(deserialize_with = "de_null")]
    pub sounds: Vec<Sound>,
    #[serde(deserialize_with = "de_null")]
    pub events: Vec<Event>,
    #[serde(deserialize_with = "de_null")]
    pub drops: Vec<Drop>,
    #[serde(deserialize_with = "de_null")]
    pub zone: Zone,
    pub offer: Option<Offer>,
}

/// Field order and omitempty match the Go struct's json encoding.
#[derive(Serialize, Clone, Debug, Default)]
pub struct Action {
    pub tick: i64,
    #[serde(rename = "move")]
    pub move_: [f64; 2],
    pub boost: bool,
    pub jump: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub aim: Option<[f64; 2]>,
    pub fire: [bool; 2],
    pub reload: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pick: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub take: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub drop: Option<i64>,
}
