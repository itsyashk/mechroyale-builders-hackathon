//! Builds the `obs` JSON line for one mech (vision, memory, sounds, beams, events).

use crate::cfg::{kv_get, WK};
use crate::game::*;
use crate::util::*;
use std::fmt::Write;

fn weapon_json(s: &mut String, m: &Mech, w: &Weapon) {
    let _ = write!(s, r#"{{"kind":"{}","cooldown":"#, w.kind.name());
    jnum(s, w.cooldown.max(0.0), 2);
    s.push_str(r#","stats":{"#);
    for (k, (name, v)) in m.wkv[w.kind.idx()].iter().enumerate() {
        if k > 0 {
            s.push(',');
        }
        jstr(s, name);
        s.push(':');
        jnum(s, *v, 4);
    }
    s.push('}');
    match w.kind {
        WK::Gun => {
            let _ = write!(s, r#","ammo":{}"#, w.ammo.max(0));
            s.push_str(r#","reload":"#);
            jnum(s, w.reload.max(0.0), 2);
        }
        WK::Laser => {
            s.push_str(r#","charge":"#);
            jnum(s, if w.charging { w.charge } else { 0.0 }, 2);
        }
        _ => {}
    }
    s.push('}');
}

impl Game {
    pub fn build_obs(&mut self, i: usize) -> String {
        let t = self.t;
        let mut s = String::with_capacity(4096);
        let alive = self.alive_count();
        let _ = write!(s, r#"{{"type":"obs","tick":{},"t":"#, self.tick);
        jnum(&mut s, t, 2);
        let _ = write!(s, r#","alive":{alive},"self":{{"id":{i},"pos":"#);
        {
            let m = &self.mechs[i];
            jv3(&mut s, m.pos, 2);
            s.push_str(r#","vel":"#);
            jv3(&mut s, m.vel, 2);
            s.push_str(r#","aim":"#);
            jv2(&mut s, m.yaw, m.pitch, 3);
            s.push_str(r#","hp":"#);
            jnum(&mut s, m.hp.max(0.0), 1);
            s.push_str(r#","max_hp":"#);
            jnum(&mut s, m.max_hp, 1);
            s.push_str(r#","energy":"#);
            jnum(&mut s, m.energy, 1);
            s.push_str(r#","max_energy":"#);
            jnum(&mut s, m.ms.energy, 1);
            let _ = write!(s, r#","grounded":{},"cubes":{},"upgrades":["#, m.grounded, m.cubes);
            for (k, u) in m.upgrades.iter().enumerate() {
                if k > 0 {
                    s.push(',');
                }
                jstr(&mut s, u);
            }
            s.push_str(r#"],"weapons":["#);
            for k in 0..2 {
                if k > 0 {
                    s.push(',');
                }
                match &m.slots[k] {
                    Some(w) => weapon_json(&mut s, m, w),
                    None => s.push_str("null"),
                }
            }
            let _ = write!(s, r#"],"in_zone":{},"stats":{{"#, m.in_zone);
            let size = 1.0 + self.cfg.size_per_cube * m.cubes as f32;
            let mut first = true;
            for (name, v) in &m.mech_kv {
                if !first {
                    s.push(',');
                }
                first = false;
                jstr(&mut s, name);
                s.push(':');
                let v = match name.as_str() {
                    "radius" | "height" | "eye_height" | "muzzle_height" => v * size,
                    _ => *v,
                };
                jnum(&mut s, v, 4);
            }
            s.push_str("}}");
        }

        // Mechs and NPCs: visible now, or tracked from memory.
        s.push_str(r#","mechs":["#);
        let mem = self.cfg.memory_s;
        let mut first = true;
        let n = self.mechs.len();
        for j in 0..n {
            if j == i || !self.mechs[j].alive {
                continue;
            }
            let vis = self.sees_mech(i, j);
            if vis {
                self.mechs[i].seen_until[j] = t + mem;
            }
            if !(vis || t <= self.mechs[i].seen_until[j]) {
                continue;
            }
            if !first {
                s.push(',');
            }
            first = false;
            let o = &self.mechs[j];
            let _ = write!(s, r#"{{"id":{j},"kind":"mech","pos":"#);
            jv3(&mut s, o.pos, 2);
            s.push_str(r#","vel":"#);
            jv3(&mut s, o.vel, 2);
            s.push_str(r#","aim":"#);
            jv2(&mut s, o.yaw, o.pitch, 3);
            if self.cfg.show_enemy_hp {
                s.push_str(r#","hp":"#);
                jnum(&mut s, o.hp.max(0.0), 1);
                s.push_str(r#","max_hp":"#);
                jnum(&mut s, o.max_hp, 1);
            }
            let _ = write!(s, r#","cubes":{},"weapons":["#, o.cubes);
            let mut f2 = true;
            for w in o.slots.iter().flatten() {
                if !f2 {
                    s.push(',');
                }
                f2 = false;
                let _ = write!(s, r#""{}""#, w.kind.name());
            }
            let _ = write!(s, r#"],"charging":{}}}"#, o.charging());
        }
        let nh = self.cfg.npc("height");
        for k in 0..self.npcs.len() {
            if !self.npcs[k].alive {
                continue;
            }
            let slot = n + k;
            let vis = {
                let np = self.npcs[k].pos;
                dist(np, self.mechs[i].pos) < 260.0 && self.sees_npc(i, k)
            };
            if vis && slot < self.mechs[i].seen_until.len() {
                self.mechs[i].seen_until[slot] = t + mem;
            }
            let remembered = slot < self.mechs[i].seen_until.len() && t <= self.mechs[i].seen_until[slot];
            if !(vis || remembered) {
                continue;
            }
            if !first {
                s.push(',');
            }
            first = false;
            let o = &self.npcs[k];
            let _ = write!(s, r#"{{"id":{},"kind":"npc","pos":"#, o.id);
            jv3(&mut s, o.pos, 2);
            s.push_str(r#","vel":"#);
            jv3(&mut s, o.vel, 2);
            s.push_str(r#","aim":"#);
            jv2(&mut s, o.yaw, o.pitch, 3);
            if self.cfg.show_enemy_hp {
                s.push_str(r#","hp":"#);
                jnum(&mut s, o.hp.max(0.0), 1);
                s.push_str(r#","max_hp":"#);
                jnum(&mut s, self.cfg.npc("hp"), 1);
            }
            s.push('}');
        }
        let _ = nh;
        s.push(']');

        // Projectiles (enemy only).
        s.push_str(r#","projectiles":["#);
        let mut first = true;
        let eye = self.mechs[i].eye();
        let far = self.cfg.vis_range + self.mechs[i].ms.vision_bonus;
        for p in &self.projs {
            if p.owner == Src::Mech(i) || dist(p.pos, eye) > far {
                continue;
            }
            if !self.sees(i, p.pos, None) {
                continue;
            }
            if !first {
                s.push(',');
            }
            first = false;
            let kind = match p.kind {
                PK::Gun => "gun",
                PK::Mortar => "mortar",
                PK::Npc => "npc",
            };
            let _ = write!(s, r#"{{"id":{},"kind":"{kind}","pos":"#, p.id);
            jv3(&mut s, p.pos, 2);
            s.push_str(r#","vel":"#);
            jv3(&mut s, p.vel, 2);
            s.push('}');
        }
        s.push(']');

        // Crates, cubes, pickups.
        let ch = self.cfg.crate_height;
        s.push_str(r#","crates":["#);
        let mut first = true;
        for c in &self.crates {
            if !c.alive || dist(c.pos, eye) > far {
                continue;
            }
            let ctr = [c.pos[0], c.pos[1], c.pos[2] + ch * 0.5];
            if !self.sees(i, ctr, Some([c.pos[0], c.pos[1], c.pos[2] + ch])) {
                continue;
            }
            if !first {
                s.push(',');
            }
            first = false;
            let _ = write!(s, r#"{{"id":{},"pos":"#, c.id);
            jv3(&mut s, c.pos, 2);
            s.push_str(r#","hp":"#);
            jnum(&mut s, c.hp, 1);
            s.push('}');
        }
        s.push_str(r#"],"cubes":["#);
        let mut first = true;
        for c in &self.cubes {
            if dist(c.pos, eye) > far || !self.sees(i, [c.pos[0], c.pos[1], c.pos[2] + 0.4], None) {
                continue;
            }
            if !first {
                s.push(',');
            }
            first = false;
            let _ = write!(s, r#"{{"id":{},"pos":"#, c.id);
            jv3(&mut s, c.pos, 2);
            s.push('}');
        }
        s.push_str(r#"],"pickups":["#);
        let mut first = true;
        for p in &self.pickups {
            if dist(p.pos, eye) > far || !self.sees(i, [p.pos[0], p.pos[1], p.pos[2] + 0.5], None) {
                continue;
            }
            if !first {
                s.push(',');
            }
            first = false;
            let _ = write!(s, r#"{{"id":{},"kind":"{}","pos":"#, p.id, p.w.kind.name());
            jv3(&mut s, p.pos, 2);
            s.push('}');
        }
        s.push(']');

        // Sounds since the last observation.
        s.push_str(r#","sounds":["#);
        let last = self.mechs[i].last_obs_t;
        let me = self.mechs[i].pos;
        let sigma = self.cfg.bearing_noise_deg.to_radians() * self.mechs[i].ms.hearing;
        let mut heard: Vec<(&'static str, f32, f32, f32)> = vec![];
        for snd in &self.sounds {
            if snd.t <= last || snd.src == Some(i) {
                continue;
            }
            let d = dist(snd.pos, me);
            if d > self.cfg.snd_range(snd.kind) {
                continue;
            }
            heard.push((snd.kind, yaw_to(&me, &snd.pos), d, t - snd.t));
        }
        let mut first = true;
        for (kind, b, d, age) in heard {
            if !first {
                s.push(',');
            }
            first = false;
            let b = wrap(b + self.rng.normal() * sigma);
            let db = if d < self.cfg.snd_near {
                "near"
            } else if d < self.cfg.snd_mid {
                "mid"
            } else {
                "far"
            };
            let _ = write!(s, r#"{{"kind":"{kind}","bearing":"#);
            jnum(&mut s, b, 3);
            let _ = write!(s, r#","dist":"{db}","age":"#);
            jnum(&mut s, age, 2);
            s.push('}');
        }
        s.push(']');

        // Beams seen since the last observation.
        s.push_str(r#","beams":["#);
        let mut first = true;
        for b in &self.beams {
            if b.t <= last || b.by == i {
                continue;
            }
            let mut seen = false;
            'pts: for (k, p) in b.path.iter().enumerate() {
                if self.sees(i, *p, None) {
                    seen = true;
                    break;
                }
                if k + 1 < b.path.len() {
                    let q = b.path[k + 1];
                    let mid = [(p[0] + q[0]) * 0.5, (p[1] + q[1]) * 0.5, (p[2] + q[2]) * 0.5];
                    if self.sees(i, mid, None) {
                        seen = true;
                        break 'pts;
                    }
                }
            }
            if !seen {
                continue;
            }
            if !first {
                s.push(',');
            }
            first = false;
            let _ = write!(s, r#"{{"by":{},"kind":"{}","path":["#, b.by, b.kind);
            for (k, p) in b.path.iter().enumerate() {
                if k > 0 {
                    s.push(',');
                }
                jv3(&mut s, *p, 2);
            }
            s.push_str(r#"],"age":"#);
            jnum(&mut s, t - b.t, 2);
            s.push('}');
        }
        s.push(']');

        // Events.
        s.push_str(r#","events":["#);
        let za = std::mem::take(&mut self.mechs[i].zone_acc);
        let mut evs = std::mem::take(&mut self.mechs[i].events);
        if za > 0.0 {
            let mut e = String::from(r#"{"type":"damaged","amount":"#);
            jnum(&mut e, za, 2);
            e.push_str(r#","weapon":"zone","bearing":null,"by":null}"#);
            evs.push(e);
        }
        s.push_str(&evs.join(","));
        s.push(']');

        // Zone, offer, drops.
        let z = self.zone_at(t);
        s.push_str(r#","zone":{"center":"#);
        jv2(&mut s, z.center[0], z.center[1], 2);
        s.push_str(r#","radius":"#);
        jnum(&mut s, z.radius, 2);
        s.push_str(r#","next_center":"#);
        jv2(&mut s, z.next_center[0], z.next_center[1], 2);
        s.push_str(r#","next_radius":"#);
        jnum(&mut s, z.next_radius, 2);
        s.push_str(r#","shrink_start":"#);
        jnum(&mut s, z.shrink_start, 2);
        s.push_str(r#","shrink_end":"#);
        jnum(&mut s, z.shrink_end, 2);
        s.push_str(r#","dps":"#);
        jnum(&mut s, z.dps, 2);
        s.push_str(r#"},"offer":"#);
        match &self.mechs[i].offer {
            None => s.push_str("null"),
            Some(o) => {
                let _ = write!(s, r#"{{"id":{},"expires":"#, o.id);
                jnum(&mut s, o.expires, 2);
                s.push_str(r#","choices":["#);
                for (k, &u) in o.choices.iter().enumerate() {
                    if k > 0 {
                        s.push(',');
                    }
                    let d = &self.cfg.upgrades[u];
                    s.push_str(r#"{"id":"#);
                    jstr(&mut s, &d.id);
                    s.push_str(r#","name":"#);
                    jstr(&mut s, &d.name);
                    s.push_str(r#","desc":"#);
                    jstr(&mut s, &d.desc);
                    s.push('}');
                }
                s.push_str("]}");
            }
        }
        s.push_str(r#","drops":["#);
        let (cap, rad) = self.cfg.drops.as_ref().map(|d| (d.capture_s, d.radius)).unwrap_or((4.0, 8.0));
        for (k, d) in self.drops.iter().enumerate() {
            if k > 0 {
                s.push(',');
            }
            let _ = write!(s, r#"{{"id":{},"pos":"#, d.id);
            jv3(&mut s, d.pos, 2);
            s.push_str(r#","radius":"#);
            jnum(&mut s, rad, 2);
            s.push_str(r#","lands_in":"#);
            jnum(&mut s, (d.land_t - t).max(0.0), 2);
            s.push_str(r#","capture_s":"#);
            jnum(&mut s, cap, 2);
            s.push_str(r#","progress":"#);
            jnum(&mut s, d.progress, 2);
            match d.holder {
                Some(h) => {
                    let _ = write!(s, r#","holder":{h}}}"#);
                }
                None => s.push_str(r#","holder":null}"#),
            }
        }
        s.push_str("]}");
        self.mechs[i].last_obs_t = t;
        let _ = kv_get;
        s
    }
}
