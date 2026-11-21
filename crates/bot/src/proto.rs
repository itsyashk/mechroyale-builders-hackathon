//! mechfight/1 message types. Every field defaults so missing/extra fields never fail.
use serde::Deserialize;
use serde_json::Value;

pub type V3 = [f32; 3];

#[derive(Deserialize, Default, Clone, Debug)]
#[serde(default)]
pub struct Obs {
    pub tick: i64,
    pub t: f32,
    pub alive: i32,
    #[serde(rename = "self")]
    pub me: Me,
    pub mechs: Vec<Mech>,
    pub projectiles: Vec<Proj>,
    pub crates: Vec<Crate>,
    pub cubes: Vec<Cube>,
    pub pickups: Vec<Pickup>,
    pub sounds: Vec<Sound>,
    pub beams: Vec<Beam>,
    pub events: Vec<Event>,
    pub drops: Vec<SupplyDrop>,
    pub zone: Zone,
    pub offer: Option<Offer>,
}

#[derive(Deserialize, Default, Clone, Debug)]
#[serde(default)]
pub struct Me {
    pub id: i64,
    pub pos: V3,
    pub vel: V3,
    pub aim: [f32; 2],
    pub hp: f32,
    pub max_hp: f32,
    pub energy: f32,
    pub max_energy: f32,
    pub grounded: bool,
    pub cubes: i32,
    pub in_zone: bool,
    pub weapons: Vec<Option<Weapon>>,
    pub stats: Value,
}

#[derive(Deserialize, Default, Clone, Debug)]
#[serde(default)]
pub struct Weapon {
    pub kind: String,
    pub cooldown: f32,
    pub ammo: Option<f32>,
    pub reload: Option<f32>,
    pub charge: Option<f32>,
    pub stats: Value,
}

impl Weapon {
    pub fn stat(&self, k: &str, d: f32) -> f32 {
        self.stats.get(k).and_then(|v| v.as_f64()).map(|v| v as f32).unwrap_or(d)
    }
}

#[derive(Deserialize, Default, Clone, Debug)]
#[serde(default)]
pub struct Mech {
    pub id: i64,
    pub kind: String,
    pub pos: V3,
    pub vel: V3,
    pub aim: [f32; 2],
    pub hp: Option<f32>,
    pub max_hp: Option<f32>,
    pub cubes: Option<i32>,
    pub weapons: Vec<String>,
    pub charging: bool,
}

impl Mech {
    pub fn is_mech(&self) -> bool {
        self.kind == "mech"
    }
    pub fn has(&self, w: &str) -> bool {
        self.weapons.iter().any(|x| x == w)
    }
}

#[derive(Deserialize, Default, Clone, Debug)]
#[serde(default)]
pub struct Proj {
    pub id: i64,
    pub kind: String,
    pub pos: V3,
    pub vel: V3,
}

#[derive(Deserialize, Default, Clone, Debug)]
#[serde(default)]
pub struct Crate {
    pub id: i64,
    pub pos: V3,
    pub hp: f32,
}

#[derive(Deserialize, Default, Clone, Debug)]
#[serde(default)]
pub struct Cube {
    pub id: i64,
    pub pos: V3,
}

#[derive(Deserialize, Default, Clone, Debug)]
#[serde(default)]
pub struct Pickup {
    pub id: i64,
    pub kind: String,
    pub pos: V3,
}

#[derive(Deserialize, Default, Clone, Debug)]
#[serde(default)]
pub struct Sound {
    pub kind: String,
    pub bearing: f32,
    pub dist: String,
    pub age: f32,
}

#[derive(Deserialize, Default, Clone, Debug)]
#[serde(default)]
pub struct Beam {
    pub by: i64,
    pub kind: String,
    pub path: Vec<V3>,
    pub age: f32,
}

#[derive(Deserialize, Default, Clone, Debug)]
#[serde(default)]
pub struct Event {
    #[serde(rename = "type")]
    pub kind: String,
    pub amount: Option<f32>,
    pub weapon: Option<String>,
    pub bearing: Option<f32>,
    pub by: Option<i64>,
    pub target: Option<i64>,
    pub rear: Option<bool>,
    pub id: Option<Value>,
    pub effect: Option<String>,
    pub cubes: Option<i32>,
    pub slot: Option<i32>,
    pub reason: Option<String>,
}

#[derive(Deserialize, Default, Clone, Debug)]
#[serde(default)]
pub struct SupplyDrop {
    pub id: i64,
    pub pos: V3,
    pub radius: f32,
    pub lands_in: f32,
    pub capture_s: f32,
    pub progress: f32,
    pub holder: Option<i64>,
}

#[derive(Deserialize, Default, Clone, Debug)]
#[serde(default)]
pub struct Zone {
    pub center: [f32; 2],
    pub radius: f32,
    pub next_center: [f32; 2],
    pub next_radius: f32,
    pub shrink_start: f32,
    pub shrink_end: f32,
    pub dps: f32,
}

#[derive(Deserialize, Default, Clone, Debug)]
#[serde(default)]
pub struct Offer {
    pub id: i64,
    pub expires: f32,
    pub choices: Vec<Choice>,
}

#[derive(Deserialize, Default, Clone, Debug)]
#[serde(default)]
pub struct Choice {
    pub id: String,
    pub name: String,
    pub desc: String,
}

#[derive(Default, Clone, Debug)]
pub struct Action {
    pub mv: [f32; 2],
    pub boost: bool,
    pub jump: bool,
    pub aim: Option<[f32; 2]>,
    pub fire: [bool; 2],
    pub reload: bool,
    pub pick: Option<usize>,
    pub take: Option<i64>,
    pub drop: Option<usize>,
    pub label: Option<String>,
}

fn r3(v: f32) -> f64 {
    if !v.is_finite() {
        return 0.0;
    }
    ((v as f64) * 1000.0).round() / 1000.0
}

impl Action {
    pub fn to_line(&self, tick: i64) -> String {
        let mut s = String::with_capacity(160);
        s.push_str(&format!("{{\"tick\":{tick},\"move\":[{},{}]", r3(self.mv[0]), r3(self.mv[1])));
        if self.boost {
            s.push_str(",\"boost\":true");
        }
        if self.jump {
            s.push_str(",\"jump\":true");
        }
        if let Some(a) = self.aim {
            if a[0].is_finite() && a[1].is_finite() {
                s.push_str(&format!(",\"aim\":[{},{}]", r3(a[0]), r3(a[1])));
            }
        }
        s.push_str(&format!(",\"fire\":[{},{}]", self.fire[0], self.fire[1]));
        if self.reload {
            s.push_str(",\"reload\":true");
        }
        if let Some(p) = self.pick {
            s.push_str(&format!(",\"pick\":{p}"));
        }
        if let Some(t) = self.take {
            s.push_str(&format!(",\"take\":{t}"));
        }
        if let Some(d) = self.drop {
            s.push_str(&format!(",\"drop\":{d}"));
        }
        if let Some(l) = &self.label {
            let clean: String = l.chars().filter(|c| c.is_ascii_graphic() || *c == ' ').take(78).collect();
            s.push_str(&format!(",\"debug\":{{\"label\":\"{}\"}}", clean.replace('"', "'").replace('\\', "/")));
        }
        s.push('}');
        s
    }
}
