//! Port of mech/world.go: map loading, rasterised obstacle grid, 2 m nav grid with A*,
//! prism/tree/terrain ray casts. The terrain heightfield raycast (`rayTerrain`) runs in
//! Zig (zig/giddy_geom.zig) when the `zig` feature is on; `ray_terrain_rust` is the
//! line-for-line Rust twin.
#![allow(dead_code)]
#![allow(clippy::too_many_arguments)]

use std::collections::HashMap;
use std::path::Path;

use serde_json::Value;

use crate::gomath::{fmax, fmax32, fmin, hypot};
use crate::types::Vec3;

#[derive(Clone, Copy, Debug)]
pub struct Slab {
    pub base: f32,
    pub top: f32,
}

#[derive(Clone, Copy)]
struct HeapItem {
    f: f32,
    idx: i32,
}

#[derive(Clone, Copy)]
struct Interval {
    base: f32,
    top: f32,
    bridge: bool,
}

#[derive(Clone, Debug)]
pub struct Prism {
    /// Interned (leaked) so hits can carry `&'static str` like Go's string Kind.
    pub kind: &'static str,
    pub base: f64,
    pub top: f64,
    pub edges: Vec<f64>,
    pub min_x: f64,
    pub min_y: f64,
    pub max_x: f64,
    pub max_y: f64,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Tree {
    pub x: f64,
    pub y: f64,
    pub z: f64,
    pub height: f64,
    pub crown_r: f64,
    pub crown_base: f64,
    pub trunk_r: f64,
    pub cx: f64,
    pub cy: f64,
    pub trunk_top: f64,
    pub cz: f64,
    pub rz: f64,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Rules {
    pub max_step: f64,
    pub max_slope_deg: f64,
    pub canopy_density: f64,
    pub canopy_block_t: f64,
}

#[derive(Default)]
pub struct Geo {
    pub prisms: Vec<Prism>,
    pub trees: Vec<Tree>,
    pub rules: Rules,
    pub max_slope_tan: f64,
    pub gx0: f64,
    pub gy0: f64,
    pub gnx: i64,
    pub gny: i64,
    pub grid_p: Vec<Vec<i32>>,
    pub grid_t: Vec<Vec<i32>>,
    pub bnx: i64,
    pub bny: i64,
    pub block_max: Vec<f32>,
}

#[derive(Default)]
pub struct World {
    pub min_x: f64,
    pub min_y: f64,
    pub nx: i64,
    pub ny: i64,
    pub ground: Vec<f32>,
    pub top: Vec<f32>,
    pub walk: Vec<f32>,
    pub obst: Vec<bool>,
    pub low: Vec<bool>,
    pub has_slab: Vec<bool>,
    pub slabs: HashMap<i32, Vec<Slab>>,
    pub bx0: f64,
    pub bx1: f64,
    pub by0: f64,
    pub by1: f64,

    pub ns: f64,
    pub nnx: i64,
    pub nny: i64,
    pub blocked: Vec<bool>,
    pub surf: Vec<f32>,
    pub clearance: Vec<f32>,
    pub comp: Vec<i32>,
    pub main_comp: i32,
    main_near: Option<Vec<i32>>,

    pub geo: Geo,

    pub g: Vec<f32>,
    pub came: Vec<i32>,
    pub stamp: Vec<u32>,
    pub closed: Vec<u32>,
    pub gen: u32,
    heap: Vec<HeapItem>,
}

fn intern(s: &str) -> &'static str {
    match s {
        "bridge" => "bridge",
        "building" => "building",
        "stadium" => "stadium",
        "" => "",
        other => Box::leak(other.to_string().into_boxed_str()),
    }
}

fn num(v: &Value) -> f64 {
    v.as_f64().unwrap_or(0.0)
}

fn nums(v: &Value) -> Result<Vec<f64>, String> {
    match v {
        Value::Null => Ok(Vec::new()),
        Value::Array(a) => a.iter().map(|x| x.as_f64().ok_or_else(|| "non-number in array".to_string())).collect(),
        _ => Err("expected array".into()),
    }
}

/// Go's encoding/json matches field names case-insensitively.
fn field<'a>(obj: &'a Value, name: &str) -> &'a Value {
    if let Some(m) = obj.as_object() {
        if let Some(v) = m.get(name) {
            return v;
        }
        for (k, v) in m {
            if k.eq_ignore_ascii_case(name) {
                return v;
            }
        }
    }
    &Value::Null
}

pub const NAV_RADIUS: f64 = 2.6; // Go: var NavRadius = 2.6
pub const GEO_CELL: f64 = 16.0;
pub const STEP_UP: f64 = 1.2;
pub const TRUNK_WALLS: bool = false; // Go: var TrunkWalls = false

pub const NBRS: [[i64; 2]; 8] = [[1, 0], [-1, 0], [0, 1], [0, -1], [1, 1], [1, -1], [-1, 1], [-1, -1]];

#[inline]
fn iabs(a: i64) -> i64 {
    if a < 0 {
        -a
    } else {
        a
    }
}

impl World {
    pub fn load_world(dir: &str) -> Result<World, String> {
        let base = Path::new(dir).join("v2");
        let raw = std::fs::read(base.join("world.json")).map_err(|e| e.to_string())?;
        let wj: Value = serde_json::from_slice(&raw).map_err(|e| e.to_string())?;
        let bounds = field(&wj, "bounds");
        let terrain = field(&wj, "terrain");
        let tb = std::fs::read(base.join("terrain.i16")).map_err(|e| e.to_string())?;
        let mut w = World {
            min_x: num(field(terrain, "minX")),
            min_y: num(field(terrain, "minY")),
            nx: num(field(terrain, "nx")) as i64,
            ny: num(field(terrain, "ny")) as i64,
            ..Default::default()
        };
        let n = (w.nx * w.ny) as usize;
        if tb.len() < n * 2 || n == 0 {
            return Err("terrain size mismatch".into());
        }
        w.bx0 = num(field(bounds, "MinX"));
        w.bx1 = num(field(bounds, "MaxX"));
        w.by0 = num(field(bounds, "MinY"));
        w.by1 = num(field(bounds, "MaxY"));
        w.ground = vec![0.0; n];
        for i in 0..n {
            w.ground[i] = (i16::from_le_bytes([tb[i * 2], tb[i * 2 + 1]]) as f32) * 0.01f32;
        }

        struct RawPrism {
            kind: String,
            base: f64,
            top: f64,
            ring: Vec<f64>,
            holes: Vec<Vec<f64>>,
        }
        let mut raw_prisms = Vec::new();
        if let Some(arr) = field(&wj, "prisms").as_array() {
            for p in arr {
                let holes = match field(p, "holes") {
                    Value::Array(h) => h.iter().map(nums).collect::<Result<Vec<_>, _>>()?,
                    _ => Vec::new(),
                };
                raw_prisms.push(RawPrism {
                    kind: field(p, "kind").as_str().unwrap_or("").to_string(),
                    base: num(field(p, "base")),
                    top: num(field(p, "top")),
                    ring: nums(field(p, "ring"))?,
                    holes,
                });
            }
        }
        let mut prisms = Vec::new();
        for p in &raw_prisms {
            let mut q = Prism {
                kind: intern(&p.kind),
                base: p.base,
                top: p.top,
                edges: Vec::new(),
                min_x: f64::INFINITY,
                min_y: f64::INFINITY,
                max_x: f64::NEG_INFINITY,
                max_y: f64::NEG_INFINITY,
            };
            let rings = std::iter::once(&p.ring).chain(p.holes.iter());
            for r in rings {
                let m = r.len() / 2;
                for a in 0..m {
                    let b = (a + 1) % m;
                    q.edges.extend_from_slice(&[r[a * 2], r[a * 2 + 1], r[b * 2], r[b * 2 + 1]]);
                    q.min_x = fmin(q.min_x, r[a * 2]);
                    q.max_x = fmax(q.max_x, r[a * 2]);
                    q.min_y = fmin(q.min_y, r[a * 2 + 1]);
                    q.max_y = fmax(q.max_y, r[a * 2 + 1]);
                }
            }
            prisms.push(q);
        }

        let mut trees = Vec::new();
        let tj = field(&wj, "trees");
        let fields: Vec<String> = field(tj, "fields")
            .as_array()
            .map(|a| a.iter().map(|v| v.as_str().unwrap_or("").to_string()).collect())
            .unwrap_or_default();
        let nf = fields.len();
        if nf > 0 {
            let data = nums(field(tj, "data"))?;
            let mut col: HashMap<&str, usize> = HashMap::new();
            for (i, f) in fields.iter().enumerate() {
                col.insert(f.as_str(), i);
            }
            let get = |row: &[f64], name: &str, def: f64| -> f64 {
                match col.get(name) {
                    Some(&i) => row[i],
                    None => def,
                }
            };
            let mut k = 0;
            while k + nf <= data.len() {
                let row = &data[k..k + nf];
                let mut t = Tree {
                    x: get(row, "x", 0.0),
                    y: get(row, "y", 0.0),
                    z: get(row, "z", 0.0),
                    height: get(row, "height", 0.0),
                    crown_r: get(row, "crownR", 1.0),
                    crown_base: get(row, "crownBase", 0.0),
                    trunk_r: get(row, "trunkR", 0.0),
                    ..Default::default()
                };
                t.cx = get(row, "crownX", t.x);
                t.cy = get(row, "crownY", t.y);
                let (cb, top) = (t.z + t.crown_base, t.z + t.height);
                t.trunk_top = t.z + fmax(t.crown_base + 0.5 * (t.height - t.crown_base), 3.0);
                t.cz = (cb + top) / 2.0;
                t.rz = fmax((top - cb) / 2.0, 0.5);
                trees.push(t);
                k += nf;
            }
        }
        let rj = field(&wj, "rules");
        let mut rules = Rules {
            max_step: num(field(rj, "maxStep")),
            max_slope_deg: num(field(rj, "maxSlopeDeg")),
            canopy_density: num(field(rj, "canopyDensity")),
            canopy_block_t: num(field(rj, "canopyBlockT")),
        };
        if rules.max_step == 0.0 {
            rules.max_step = 1.5;
        }
        w.init_geo(prisms, trees, rules);

        let mut cells: HashMap<i32, Vec<Interval>> = HashMap::new();
        for p in &raw_prisms {
            let rings: Vec<&Vec<f64>> = std::iter::once(&p.ring).chain(p.holes.iter()).collect();
            let (mut min_y, mut max_y) = (f64::INFINITY, f64::NEG_INFINITY);
            let mut k = 1;
            while k < p.ring.len() {
                min_y = fmin(min_y, p.ring[k]);
                max_y = fmax(max_y, p.ring[k]);
                k += 2;
            }
            let j0 = (min_y - w.min_y).ceil() as i64;
            let j1 = (max_y - w.min_y).floor() as i64;
            let iv = Interval { base: p.base as f32, top: p.top as f32, bridge: p.kind == "bridge" };
            let mut xs: Vec<f64> = Vec::new();
            let mut j = j0.max(0);
            while j <= j1 && j < w.ny {
                let y = w.min_y + j as f64 + 1e-4;
                xs.clear();
                for r in &rings {
                    let m = r.len() / 2;
                    for a in 0..m {
                        let b = (a + 1) % m;
                        let (x0, y0, x1, y1) = (r[a * 2], r[a * 2 + 1], r[b * 2], r[b * 2 + 1]);
                        if (y0 <= y) != (y1 <= y) {
                            xs.push(x0 + (y - y0) / (y1 - y0) * (x1 - x0));
                        }
                    }
                }
                xs.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
                let mut k = 0;
                while k + 1 < xs.len() {
                    let i0 = (xs[k] - w.min_x).ceil() as i64;
                    let i1 = (xs[k + 1] - w.min_x).floor() as i64;
                    let mut i = i0.max(0);
                    while i <= i1 && i < w.nx {
                        let idx = (j * w.nx + i) as i32;
                        cells.entry(idx).or_default().push(iv);
                        i += 1;
                    }
                    k += 2;
                }
                j += 1;
            }
        }
        w.top = w.ground.clone();
        w.walk = w.ground.clone();
        w.obst = vec![false; n];
        w.low = vec![false; n];
        w.has_slab = vec![false; n];
        w.slabs = HashMap::new();
        for (idx, mut ivs) in cells {
            // sort.Slice is an insertion sort (stable) for the short lists seen here.
            ivs.sort_by(|a, b| a.base.partial_cmp(&b.base).unwrap_or(std::cmp::Ordering::Equal));
            let u = idx as usize;
            let g = w.ground[u];
            let mut top = g;
            let mut deck: f32 = -1e9;
            let mut low = false;
            for iv in &ivs {
                if iv.top <= top {
                    continue;
                }
                if iv.base <= top + 1.5 {
                    top = iv.top;
                    continue;
                }
                w.slabs.entry(idx).or_default().push(Slab { base: iv.base, top: iv.top });
                w.has_slab[u] = true;
                if iv.base - top < 10.5 {
                    if iv.bridge && iv.top - g < 6.5 {
                        deck = iv.top;
                    } else {
                        low = true;
                    }
                }
            }
            w.top[u] = top;
            w.walk[u] = top;
            if deck > top {
                w.walk[u] = deck;
            }
            w.low[u] = low;
            w.obst[u] = top - g > 1.2 || low;
        }
        w.build_nav();
        Ok(w)
    }

    pub fn vidx(&self, x: f64, y: f64) -> usize {
        let mut i = (x - self.min_x).round() as i64;
        let mut j = (y - self.min_y).round() as i64;
        if i < 0 {
            i = 0;
        } else if i >= self.nx {
            i = self.nx - 1;
        }
        if j < 0 {
            j = 0;
        } else if j >= self.ny {
            j = self.ny - 1;
        }
        (j * self.nx + i) as usize
    }

    pub fn in_bounds(&self, x: f64, y: f64) -> bool {
        x > self.bx0 && x < self.bx1 && y > self.by0 && y < self.by1
    }

    pub fn ground_at(&self, x: f64, y: f64) -> f64 {
        let mut fx = x - self.min_x;
        let mut fy = y - self.min_y;
        fx = fmax(0.0, fmin(fx, (self.nx - 1) as f64 - 1e-6));
        fy = fmax(0.0, fmin(fy, (self.ny - 1) as f64 - 1e-6));
        let (i, j) = (fx as i64, fy as i64);
        let (tx, ty) = (fx - i as f64, fy - j as f64);
        let nx = self.nx;
        let a = self.ground[(j * nx + i) as usize] as f64;
        let b = self.ground[(j * nx + i + 1) as usize] as f64;
        let c = self.ground[((j + 1) * nx + i) as usize] as f64;
        let d = self.ground[((j + 1) * nx + i + 1) as usize] as f64;
        (a * (1.0 - tx) + b * tx) * (1.0 - ty) + (c * (1.0 - tx) + d * tx) * ty
    }

    fn stack(&self, x: f64, y: f64, deck: bool) -> f64 {
        self.stack_kind(x, y, deck).0
    }

    fn stack_kind(&self, x: f64, y: f64, deck: bool) -> (f64, i64) {
        let g = self.ground_at(x, y);
        let mut z = g;
        let mut id: i64 = -1;
        let c = self.cell_of(x, y);
        if c < 0 {
            return (z, id);
        }
        let ps = &self.geo.grid_p[c as usize];
        for _pass in 0..8 {
            let mut moved = false;
            for &pi in ps {
                let p = &self.geo.prisms[pi as usize];
                if p.top <= z || x < p.min_x || x > p.max_x || y < p.min_y || y > p.max_y {
                    continue;
                }
                if !(p.base <= z + 1.5 || deck && p.kind == "bridge" && p.top - g < 6.5) {
                    continue;
                }
                if point_in_poly(&p.edges, x, y) {
                    z = p.top;
                    id = pi as i64;
                    moved = true;
                }
            }
            if !moved {
                break;
            }
        }
        (z, id)
    }

    pub fn walk_at(&self, x: f64, y: f64) -> f64 {
        self.stack(x, y, true)
    }

    pub fn top_at(&self, x: f64, y: f64) -> f64 {
        self.stack(x, y, false)
    }

    pub fn solid(&self, x: f64, y: f64, z: f64) -> bool {
        if z < self.ground_at(x, y) {
            return true;
        }
        let c = self.cell_of(x, y);
        if c < 0 {
            return false;
        }
        for &pi in &self.geo.grid_p[c as usize] {
            let p = &self.geo.prisms[pi as usize];
            if z > p.base
                && z < p.top
                && x >= p.min_x
                && x <= p.max_x
                && y >= p.min_y
                && y <= p.max_y
                && point_in_poly(&p.edges, x, y)
            {
                return true;
            }
        }
        false
    }

    pub fn solid_raster(&self, x: f64, y: f64, z: f64) -> bool {
        let idx = self.vidx(x, y);
        if z < self.top[idx] as f64 - 0.05 {
            return true;
        }
        if self.has_slab[idx] {
            if let Some(ss) = self.slabs.get(&(idx as i32)) {
                for s in ss {
                    if z > s.base as f64 && z < s.top as f64 {
                        return true;
                    }
                }
            }
        }
        false
    }

    pub fn clear(&self, a: Vec3, b: Vec3) -> bool {
        self.shot_clear(a, b)
    }

    pub fn clear_raster(&self, a: Vec3, b: Vec3) -> bool {
        let (dx, dy, dz) = (b[0] - a[0], b[1] - a[1], b[2] - a[2]);
        let d = (dx * dx + dy * dy + dz * dz).sqrt();
        let n = (d / 1.0) as i64 + 1;
        let inv = 1.0 / n as f64;
        for k in 1..n {
            let t = k as f64 * inv;
            if self.solid_raster(a[0] + dx * t, a[1] + dy * t, a[2] + dz * t) {
                return false;
            }
        }
        true
    }

    fn standable(&self, x: f64, y: f64, z: f64, r: f64) -> bool {
        if x < self.bx0 + r || x > self.bx1 - r || y < self.by0 + r || y > self.by1 - r {
            return false;
        }
        !self.wall_within(x, y, z, r, 9.0)
    }

    fn wall_within(&self, x: f64, y: f64, z: f64, r: f64, h: f64) -> bool {
        let (lo, hi) = (z + STEP_UP, z + h);
        let r2 = r * r;
        let mut found = false;
        self.for_cells(x - r, y - r, x + r, y + r, |c| {
            if found {
                return;
            }
            for &pi in &self.geo.grid_p[c] {
                let p = &self.geo.prisms[pi as usize];
                if p.top <= lo || p.base >= hi || x + r < p.min_x || x - r > p.max_x || y + r < p.min_y || y - r > p.max_y {
                    continue;
                }
                let e = &p.edges;
                let mut i = 0;
                while i < e.len() {
                    let (x0, y0) = (e[i], e[i + 1]);
                    let (ux, uy) = (e[i + 2] - x0, e[i + 3] - y0);
                    let l2 = ux * ux + uy * uy;
                    let mut t = 0.0;
                    if l2 > 1e-9 {
                        t = fmax(0.0, fmin(1.0, ((x - x0) * ux + (y - y0) * uy) / l2));
                    }
                    let (mx, my) = (x0 + ux * t - x, y0 + uy * t - y);
                    if mx * mx + my * my < r2 {
                        found = true;
                        return;
                    }
                    i += 4;
                }
                if point_in_poly(e, x, y) {
                    found = true;
                    return;
                }
            }
        });
        found
    }

    fn build_nav(&mut self) {
        self.ns = 2.0;
        self.nnx = (self.nx + 1) / 2;
        self.nny = (self.ny + 1) / 2;
        let n = (self.nnx * self.nny) as usize;
        self.blocked = vec![false; n];
        self.surf = vec![0.0; n];
        for c in 0..n {
            let (x, y) = self.cell_pos(c as i64);
            let (z, id) = self.stack_kind(x, y, true);
            self.surf[c] = z as f32;

            let roof = id >= 0 && self.geo.prisms[id as usize].kind != "bridge" && z - self.ground_at(x, y) > 1.5;
            self.blocked[c] = roof || !self.standable(x, y, z, NAV_RADIUS);
        }

        self.clearance = vec![0.0; n];
        let mut q: Vec<i32> = Vec::with_capacity(n);
        for c in 0..n {
            if self.blocked[c] {
                self.clearance[c] = 0.0;
                q.push(c as i32);
            } else {
                self.clearance[c] = 1e9;
            }
        }
        let mut h = 0;
        while h < q.len() {
            let c = q[h] as i64;
            h += 1;
            let d = self.clearance[c as usize] + self.ns as f32;
            if d > 20.0 {
                continue;
            }
            let (ci, cj) = (c % self.nnx, c / self.nnx);
            for o in [[1i64, 0i64], [-1, 0], [0, 1], [0, -1]] {
                let (i, j) = (ci + o[0], cj + o[1]);
                if i < 0 || j < 0 || i >= self.nnx || j >= self.nny {
                    continue;
                }
                let nb = (j * self.nnx + i) as usize;
                if self.clearance[nb] > d {
                    self.clearance[nb] = d;
                    q.push(nb as i32);
                }
            }
        }
        for c in 0..n {
            if self.clearance[c] > 20.0 {
                self.clearance[c] = 20.0;
            }
        }

        self.comp = vec![0; n];
        let (mut label, mut best_n): (i32, i32) = (0, 0);
        let mut best_size = 0usize;
        for c0 in 0..n {
            if self.blocked[c0] || self.comp[c0] != 0 {
                continue;
            }
            label += 1;
            self.comp[c0] = label;
            q.clear();
            q.push(c0 as i32);
            let mut h = 0;
            while h < q.len() {
                let c = q[h] as i64;
                h += 1;
                let (ci, cj) = (c % self.nnx, c / self.nnx);
                for o in [[1i64, 0i64], [-1, 0], [0, 1], [0, -1]] {
                    let (i, j) = (ci + o[0], cj + o[1]);
                    if i < 0 || j < 0 || i >= self.nnx || j >= self.nny {
                        continue;
                    }
                    let nb = j * self.nnx + i;
                    if self.comp[nb as usize] == 0 && self.can_step(c, nb, false) && self.can_step(nb, c, false) {
                        self.comp[nb as usize] = label;
                        q.push(nb as i32);
                    }
                }
            }
            if q.len() > best_size {
                best_size = q.len();
                best_n = label;
            }
        }
        self.main_comp = best_n;
        self.g = vec![0.0; n];
        self.came = vec![0; n];
        self.stamp = vec![0; n];
        self.closed = vec![0; n];
    }

    pub fn cell(&self, x: f64, y: f64) -> i64 {
        let mut ci = ((x - self.min_x) / self.ns).round() as i64;
        let mut cj = ((y - self.min_y) / self.ns).round() as i64;
        if ci < 0 {
            ci = 0;
        } else if ci >= self.nnx {
            ci = self.nnx - 1;
        }
        if cj < 0 {
            cj = 0;
        } else if cj >= self.nny {
            cj = self.nny - 1;
        }
        cj * self.nnx + ci
    }

    pub fn cell_pos(&self, c: i64) -> (f64, f64) {
        (self.min_x + (c % self.nnx) as f64 * self.ns, self.min_y + (c / self.nnx) as f64 * self.ns)
    }

    pub fn free(&self, x: f64, y: f64) -> bool {
        !self.blocked[self.cell(x, y) as usize]
    }

    pub fn nearest_free(&self, c: i64) -> i64 {
        if !self.blocked[c as usize] {
            return c;
        }
        let (ci, cj) = (c % self.nnx, c / self.nnx);
        let (mut best, mut best_d): (i64, i64) = (-1, 1 << 30);
        for r in 1..=20i64 {
            for dj in -r..=r {
                for di in -r..=r {
                    if iabs(di).max(iabs(dj)) != r {
                        continue;
                    }
                    let (i, j) = (ci + di, cj + dj);
                    if i < 0 || j < 0 || i >= self.nnx || j >= self.nny {
                        continue;
                    }
                    if !self.blocked[(j * self.nnx + i) as usize] {
                        let d = di * di + dj * dj;
                        if d < best_d {
                            best = j * self.nnx + i;
                            best_d = d;
                        }
                    }
                }
            }
            if best >= 0 {
                return best;
            }
        }
        -1
    }

    pub fn can_step(&self, a: i64, b: i64, diag: bool) -> bool {
        if self.blocked[b as usize] {
            return false;
        }
        let rise = self.surf[b as usize] - self.surf[a as usize];
        let mut lim: f32 = 2.2;
        if diag {
            lim = 3.1;
        }
        rise <= lim
    }

    fn push(&mut self, it: HeapItem) {
        self.heap.push(it);
        let mut i = self.heap.len() - 1;
        while i > 0 {
            let p = (i - 1) / 2;
            if self.heap[p].f <= self.heap[i].f {
                break;
            }
            self.heap.swap(p, i);
            i = p;
        }
    }

    fn pop(&mut self) -> HeapItem {
        let top = self.heap[0];
        let last = self.heap.len() - 1;
        self.heap[0] = self.heap[last];
        self.heap.truncate(last);
        let mut i = 0;
        loop {
            let (l, r) = (2 * i + 1, 2 * i + 2);
            let mut m = i;
            if l < last && self.heap[l].f < self.heap[m].f {
                m = l;
            }
            if r < last && self.heap[r].f < self.heap[m].f {
                m = r;
            }
            if m == i {
                break;
            }
            self.heap.swap(m, i);
            i = m;
        }
        top
    }

    /// Go returns (nil, false) when start/goal have no free cell; `None` here is that nil.
    pub fn path(&mut self, sx: f64, sy: f64, gx: f64, gy: f64, max_expand: i64) -> (Option<Vec<i32>>, bool) {
        let s = self.nearest_free(self.cell(sx, sy));
        let g = self.nearest_free(self.cell(gx, gy));
        if s < 0 || g < 0 {
            return (None, false);
        }
        if s == g {
            return (Some(vec![s as i32]), true);
        }
        self.gen = self.gen.wrapping_add(1);
        let gen = self.gen;
        self.heap.clear();
        let nnx = self.nnx;
        let ns = self.ns;
        let (gi, gj) = (g % nnx, g / nnx);
        let h = |c: i64| -> f32 {
            let dx = iabs(c % nnx - gi) as f64;
            let dy = iabs(c / nnx - gj) as f64;
            let (mn, mx) = (fmin(dx, dy), fmax(dx, dy));
            ((mx + 0.4142 * mn) * ns * 1.15) as f32
        };
        let su = s as usize;
        self.g[su] = 0.0;
        self.came[su] = -1;
        self.stamp[su] = gen;
        self.push(HeapItem { f: h(s), idx: s as i32 });
        let (mut best, mut best_h) = (s, h(s));
        let mut found = false;
        let mut n = 0;
        while !self.heap.is_empty() && n < max_expand {
            n += 1;
            let it = self.pop();
            let c = it.idx as i64;
            if self.closed[c as usize] == gen {
                continue;
            }
            self.closed[c as usize] = gen;
            if c == g {
                found = true;
                best = c;
                break;
            }
            let hc = h(c);
            if hc < best_h {
                best = c;
                best_h = hc;
            }
            let (ci, cj) = (c % nnx, c / nnx);
            for (k, o) in NBRS.iter().enumerate() {
                let (i, j) = (ci + o[0], cj + o[1]);
                if i < 0 || j < 0 || i >= nnx || j >= self.nny {
                    continue;
                }
                let nb = j * nnx + i;
                let diag = k >= 4;
                if self.closed[nb as usize] == gen || !self.can_step(c, nb, diag) {
                    continue;
                }
                let mut step = ns as f32;
                if diag {
                    if self.blocked[(cj * nnx + i) as usize] || self.blocked[(j * nnx + ci) as usize] {
                        continue;
                    }
                    step *= 1.4142f32;
                }

                let cl = self.clearance[nb as usize];
                if cl < 4.0 {
                    step *= 1.0 + (4.0 - cl) * 0.15f32;
                }
                let ng = self.g[c as usize] + step;
                let nbu = nb as usize;
                if self.stamp[nbu] != gen || ng < self.g[nbu] {
                    self.g[nbu] = ng;
                    self.came[nbu] = c as i32;
                    self.stamp[nbu] = gen;
                    self.push(HeapItem { f: ng + h(nb), idx: nb as i32 });
                }
            }
        }
        let mut path = Vec::new();
        let mut c = best;
        while c >= 0 {
            path.push(c as i32);
            if c == s {
                break;
            }
            c = self.came[c as usize] as i64;
        }
        path.reverse();
        (Some(path), found)
    }

    pub fn line_walkable(&self, x0: f64, y0: f64, x1: f64, y1: f64) -> bool {
        let (dx, dy) = (x1 - x0, y1 - y0);
        let d = hypot(dx, dy);
        let n = (d / 1.0) as i64 + 1;
        let mut prev = self.cell(x0, y0);
        for k in 1..=n {
            let t = k as f64 / n as f64;
            let c = self.cell(x0 + dx * t, y0 + dy * t);
            if c == prev {
                continue;
            }
            if self.blocked[c as usize] || self.surf[c as usize] - self.surf[prev as usize] > 3.1 {
                return false;
            }
            prev = c;
        }
        true
    }

    pub fn can_stand(&self, x: f64, y: f64, z: f64, r: f64) -> bool {
        if x < self.bx0 + r || x > self.bx1 - r || y < self.by0 + r || y > self.by1 - r {
            return false;
        }
        !self.wall_within(x, y, z, r, 9.0)
    }

    pub fn flat_world() -> World {
        let mut w = World {
            min_x: -513.0,
            min_y: -334.0,
            nx: 1070,
            ny: 724,
            bx0: -512.0,
            bx1: 555.0,
            by0: -333.0,
            by1: 388.0,
            ..Default::default()
        };
        let n = (w.nx * w.ny) as usize;
        w.ground = vec![0.0; n];
        w.top = vec![0.0; n];
        w.walk = vec![0.0; n];
        w.obst = vec![false; n];
        w.low = vec![false; n];
        w.has_slab = vec![false; n];
        w.slabs = HashMap::new();
        w.init_geo(Vec::new(), Vec::new(), Rules { max_step: 1.5, ..Default::default() });
        w.build_nav();
        w
    }

    pub fn main_near(&mut self, c: i64) -> i64 {
        if self.main_near.is_none() {
            let n = (self.nnx * self.nny) as usize;
            let mut near = vec![-1i32; n];
            let mut depth = vec![0u8; n];
            let mut q: Vec<i32> = Vec::with_capacity(n);
            for i in 0..n {
                near[i] = -1;
                if !self.blocked[i] && self.comp[i] == self.main_comp {
                    near[i] = i as i32;
                    q.push(i as i32);
                }
            }
            let mut h = 0;
            while h < q.len() {
                let c = q[h] as i64;
                h += 1;
                if depth[c as usize] >= 20 {
                    continue;
                }
                let (ci, cj) = (c % self.nnx, c / self.nnx);
                for o in NBRS.iter() {
                    let (i, j) = (ci + o[0], cj + o[1]);
                    if i < 0 || j < 0 || i >= self.nnx || j >= self.nny {
                        continue;
                    }
                    let nb = (j * self.nnx + i) as usize;
                    if near[nb] < 0 {
                        near[nb] = near[c as usize];
                        depth[nb] = depth[c as usize] + 1;
                        q.push(nb as i32);
                    }
                }
            }
            self.main_near = Some(near);
        }
        self.main_near.as_ref().unwrap()[c as usize] as i64
    }

    pub fn off_main(&self, x: f64, y: f64) -> bool {
        let c = self.nearest_free(self.cell(x, y));
        c < 0 || self.comp[c as usize] != self.main_comp
    }

    pub fn escape_plan(&mut self, x: f64, y: f64) -> ([f64; 2], [f64; 2], bool) {
        let (launch, land) = ([0.0; 2], [0.0; 2]);
        let s = self.nearest_free(self.cell(x, y));
        if s < 0 {
            return (launch, land, false);
        }
        self.gen = self.gen.wrapping_add(1);
        let gen = self.gen;
        let mut queue: Vec<i32> = vec![s as i32];
        self.stamp[s as usize] = gen;
        self.g[s as usize] = 0.0;
        let (mut best, mut best_s): (i64, f64) = (-1, f64::INFINITY);
        let mut h = 0;
        while h < queue.len() && h < 60000 {
            let c = queue[h] as i64;
            h += 1;
            let (ci, cj) = (c % self.nnx, c / self.nnx);
            let m = self.main_near(c);
            if m >= 0 {
                let (cx, cy) = self.cell_pos(c);
                let (mx, my) = self.cell_pos(m);
                let sc = self.g[c as usize] as f64 * self.ns + 4.0 * hypot(mx - cx, my - cy);
                if sc < best_s {
                    best = c;
                    best_s = sc;
                }
            }
            for (k, o) in NBRS.iter().enumerate() {
                let (i, j) = (ci + o[0], cj + o[1]);
                if i < 0 || j < 0 || i >= self.nnx || j >= self.nny {
                    continue;
                }
                let nb = j * self.nnx + i;
                if self.stamp[nb as usize] == gen || !self.can_step(c, nb, k >= 4) {
                    continue;
                }
                if k >= 4 && (self.blocked[(cj * self.nnx + i) as usize] || self.blocked[(j * self.nnx + ci) as usize]) {
                    continue;
                }
                self.stamp[nb as usize] = gen;
                self.g[nb as usize] = self.g[c as usize] + 1.0;
                queue.push(nb as i32);
            }
        }
        if best < 0 {
            return (launch, land, false);
        }
        let (lx, ly) = self.cell_pos(best);
        let mn = self.main_near(best);
        let (mut mx, mut my) = self.cell_pos(mn);

        let d = hypot(mx - lx, my - ly);
        if d > 0.1 {
            let (ex, ey) = (mx + (mx - lx) / d * 5.0, my + (my - ly) / d * 5.0);
            if self.in_bounds(ex, ey) && self.reachable(ex, ey) {
                mx = ex;
                my = ey;
            }
        }
        ([lx, ly], [mx, my], true)
    }

    pub fn reachable(&self, x: f64, y: f64) -> bool {
        let c = self.cell(x, y) as usize;
        !self.blocked[c] && self.comp[c] == self.main_comp
    }

    pub fn exposure(&self, x: f64, y: f64) -> f64 {
        let z = self.walk_at(x, y) + 7.0;
        let mut total = 0.0;
        for k in 0..24 {
            let a = k as f64 * std::f64::consts::PI / 12.0;
            let (c, s) = (a.cos(), a.sin());
            let mut d = 2.5;
            while d < 200.0 {
                let (px, py) = (x + c * d, y + s * d);
                if !self.in_bounds(px, py) || self.solid_raster(px, py, z) {
                    break;
                }
                d += 2.5;
            }
            total += d;
        }
        total / (24.0 * 200.0)
    }

    fn init_geo(&mut self, prisms: Vec<Prism>, trees: Vec<Tree>, rules: Rules) {
        self.geo.prisms = prisms;
        self.geo.trees = trees;
        self.geo.rules = rules;
        let g = &mut self.geo;
        if g.rules.max_slope_deg == 0.0 {
            g.rules.max_slope_deg = 50.0;
        }
        if g.rules.canopy_density == 0.0 {
            g.rules.canopy_density = 0.35;
            g.rules.canopy_block_t = 0.25;
        }
        g.max_slope_tan = (g.rules.max_slope_deg * std::f64::consts::PI / 180.0).tan();
        g.gx0 = self.bx0 - GEO_CELL;
        g.gy0 = self.by0 - GEO_CELL;
        g.gnx = ((self.bx1 - self.bx0) / GEO_CELL).ceil() as i64 + 3;
        g.gny = ((self.by1 - self.by0) / GEO_CELL).ceil() as i64 + 3;
        g.grid_p = vec![Vec::new(); (g.gnx * g.gny) as usize];
        g.grid_t = vec![Vec::new(); (g.gnx * g.gny) as usize];
        let mut grid_p = std::mem::take(&mut self.geo.grid_p);
        let mut grid_t = std::mem::take(&mut self.geo.grid_t);
        for (i, p) in self.geo.prisms.iter().enumerate() {
            self.for_cells(p.min_x, p.min_y, p.max_x, p.max_y, |c| grid_p[c].push(i as i32));
        }
        for (i, t) in self.geo.trees.iter().enumerate() {
            let r = t.crown_r;
            self.for_cells(
                fmin(t.cx - r, t.x - t.trunk_r),
                fmin(t.cy - r, t.y - t.trunk_r),
                fmax(t.cx + r, t.x + t.trunk_r),
                fmax(t.cy + r, t.y + t.trunk_r),
                |c| grid_t[c].push(i as i32),
            );
        }
        self.geo.grid_p = grid_p;
        self.geo.grid_t = grid_t;
        let g = &mut self.geo;
        g.bnx = (self.nx + 7) / 8;
        g.bny = (self.ny + 7) / 8;
        let mut bm = vec![f32::NEG_INFINITY; (g.bnx * g.bny) as usize];
        for j in 0..self.ny {
            for i in 0..self.nx {
                let b = ((j / 8) * g.bnx + i / 8) as usize;
                let h = self.ground[(j * self.nx + i) as usize];
                if h > bm[b] {
                    bm[b] = h;
                }
            }
        }
        g.block_max = vec![0.0; bm.len()];
        for j in 0..g.bny {
            for i in 0..g.bnx {
                let mut v = bm[(j * g.bnx + i) as usize];
                if i + 1 < g.bnx {
                    v = fmax32(v, bm[(j * g.bnx + i + 1) as usize]);
                }
                if j + 1 < g.bny {
                    v = fmax32(v, bm[((j + 1) * g.bnx + i) as usize]);
                }
                if i + 1 < g.bnx && j + 1 < g.bny {
                    v = fmax32(v, bm[((j + 1) * g.bnx + i + 1) as usize]);
                }
                g.block_max[(j * g.bnx + i) as usize] = v;
            }
        }
    }

    fn cell_of(&self, x: f64, y: f64) -> i64 {
        let g = &self.geo;
        let i = ((x - g.gx0) / GEO_CELL).floor() as i64;
        let j = ((y - g.gy0) / GEO_CELL).floor() as i64;
        if i < 0 || j < 0 || i >= g.gnx || j >= g.gny {
            return -1;
        }
        j * g.gnx + i
    }

    fn for_cells<F: FnMut(usize)>(&self, x0: f64, y0: f64, x1: f64, y1: f64, mut f: F) {
        let g = &self.geo;
        let i0 = 0i64.max(((x0 - g.gx0) / GEO_CELL).floor() as i64);
        let i1 = (g.gnx - 1).min(((x1 - g.gx0) / GEO_CELL).floor() as i64);
        let j0 = 0i64.max(((y0 - g.gy0) / GEO_CELL).floor() as i64);
        let j1 = (g.gny - 1).min(((y1 - g.gy0) / GEO_CELL).floor() as i64);
        let mut j = j0;
        while j <= j1 {
            let mut i = i0;
            while i <= i1 {
                f((j * g.gnx + i) as usize);
                i += 1;
            }
            j += 1;
        }
    }

    fn walk_cells<F: FnMut(usize) -> f64>(&self, x0: f64, y0: f64, x1: f64, y1: f64, mut f: F) {
        self.walk_cells_t(x0, y0, x1, y1, |c, _, _| f(c));
    }

    fn walk_cells_t<F: FnMut(usize, f64, f64) -> f64>(&self, x0: f64, y0: f64, x1: f64, y1: f64, mut f: F) {
        let g = &self.geo;
        let (dx, dy) = (x1 - x0, y1 - y0);
        let mut ci = ((x0 - g.gx0) / GEO_CELL).floor() as i64;
        let mut cj = ((y0 - g.gy0) / GEO_CELL).floor() as i64;
        let ei = ((x1 - g.gx0) / GEO_CELL).floor() as i64;
        let ej = ((y1 - g.gy0) / GEO_CELL).floor() as i64;
        let (mut si, mut sj) = (-1i64, -1i64);
        if dx > 0.0 {
            si = 1;
        }
        if dy > 0.0 {
            sj = 1;
        }
        let inf = f64::INFINITY;
        let (mut tdx, mut tdy, mut tmx, mut tmy) = (inf, inf, inf, inf);
        if dx.abs() > 1e-9 {
            tdx = GEO_CELL / dx.abs();
            if dx > 0.0 {
                tmx = ((ci + 1) as f64 * GEO_CELL + g.gx0 - x0) / dx.abs();
            } else {
                tmx = (x0 - (ci as f64 * GEO_CELL + g.gx0)) / dx.abs();
            }
        }
        if dy.abs() > 1e-9 {
            tdy = GEO_CELL / dy.abs();
            if dy > 0.0 {
                tmy = ((cj + 1) as f64 * GEO_CELL + g.gy0 - y0) / dy.abs();
            } else {
                tmy = (y0 - (cj as f64 * GEO_CELL + g.gy0)) / dy.abs();
            }
        }
        let mut best = inf;
        let mut tin = 0.0;
        for _k in 0..4096 {
            if ci >= 0 && cj >= 0 && ci < g.gnx && cj < g.gny {
                best = f((cj * g.gnx + ci) as usize, tin, fmin(1.0, fmin(tmx, tmy)));
            }
            if ci == ei && cj == ej {
                return;
            }
            let t;
            if tmx < tmy {
                t = tmx;
                tmx += tdx;
                ci += si;
            } else {
                t = tmy;
                tmy += tdy;
                cj += sj;
            }
            if t > 1.0 || best < t {
                return;
            }
            tin = t;
        }
    }

    pub fn ground_gradient(&self, x: f64, y: f64) -> (f64, f64) {
        (
            (self.ground_at(x + 1.0, y) - self.ground_at(x - 1.0, y)) / 2.0,
            (self.ground_at(x, y + 1.0) - self.ground_at(x, y - 1.0)) / 2.0,
        )
    }

    pub fn surface_at(&self, x: f64, y: f64, max_z: f64) -> f64 {
        self.surface_kind(x, y, max_z).0
    }

    pub fn surface_kind(&self, x: f64, y: f64, max_z: f64) -> (f64, i64) {
        let mut z = self.ground_at(x, y);
        let mut id = -1i64;
        let c = self.cell_of(x, y);
        if c < 0 {
            return (z, id);
        }
        for &pi in &self.geo.grid_p[c as usize] {
            let p = &self.geo.prisms[pi as usize];
            if p.top > max_z
                || p.top <= z
                || x < p.min_x
                || x > p.max_x
                || y < p.min_y
                || y > p.max_y
                || !point_in_poly(&p.edges, x, y)
            {
                continue;
            }
            z = p.top;
            id = pi as i64;
        }
        (z, id)
    }

    pub fn ceiling_above(&self, x: f64, y: f64, z: f64, r: f64) -> f64 {
        let mut best = f64::INFINITY;
        self.for_cells(x - r, y - r, x + r, y + r, |c| {
            for &pi in &self.geo.grid_p[c] {
                let p = &self.geo.prisms[pi as usize];
                if p.base < z - 0.01 || p.base >= best || x < p.min_x || x > p.max_x || y < p.min_y || y > p.max_y {
                    continue;
                }
                if point_in_poly(&p.edges, x, y) {
                    best = p.base;
                }
            }
        });
        best
    }

    pub fn penetration(&self, x: f64, y: f64, z: f64, r: f64, h: f64) -> (f64, f64, f64, &'static str, i64) {
        let (mut depth, mut nx, mut ny, mut kind, mut id) = (0.0f64, 0.0f64, 0.0f64, "terrain", -1i64);
        let (lo, hi) = (z + STEP_UP, z + h);
        let mut put = |d: f64, ax: f64, ay: f64, k: &'static str, i: i64| {
            if d > depth {
                depth = d;
                nx = ax;
                ny = ay;
                kind = k;
                id = i;
            }
        };
        self.for_cells(x - r, y - r, x + r, y + r, |c| {
            for &pi in &self.geo.grid_p[c] {
                let p = &self.geo.prisms[pi as usize];
                if p.top <= lo || p.base >= hi || x + r < p.min_x || x - r > p.max_x || y + r < p.min_y || y - r > p.max_y {
                    continue;
                }
                let (cx, cy, d) = closest_on_edges(&p.edges, x, y);
                if point_in_poly(&p.edges, x, y) {
                    let (ux, uy) = (cx - x, cy - y);
                    let mut l = hypot(ux, uy);
                    if l == 0.0 {
                        l = 1e-9;
                    }
                    put(l + r, ux / l, uy / l, p.kind, pi as i64);
                } else if d < r {
                    let mut l = d;
                    if l == 0.0 {
                        l = 1e-9;
                    }
                    put(r - d, (x - cx) / l, (y - cy) / l, p.kind, pi as i64);
                }
            }
            for &ti in &self.geo.grid_t[c] {
                if !TRUNK_WALLS {
                    break;
                }
                let t = &self.geo.trees[ti as usize];
                if t.trunk_top <= lo || t.z - 1.0 >= hi {
                    continue;
                }
                let (ux, uy) = (x - t.x, y - t.y);
                let d = hypot(ux, uy);
                let f = r + t.trunk_r;
                if d < f {
                    if d > 1e-9 {
                        put(f - d, ux / d, uy / d, "trunk", ti as i64);
                    } else {
                        put(f - d, 1.0, 0.0, "trunk", ti as i64);
                    }
                }
            }
        });
        if self.ground_at(x, y) > lo {
            let (gx, gy) = self.ground_gradient(x, y);
            let n = hypot(gx, gy);
            if n > 0.001 {
                let (ux, uy) = (-gx / n, -gy / n);
                let mut l = 0.0;
                while l < 2.0 * r && self.ground_at(x + ux * l, y + uy * l) > lo {
                    l += 0.25;
                }
                put(fmax(l, 0.05), ux, uy, "terrain", -1);
            }
        }
        put(self.bx0 + r - x, 1.0, 0.0, "terrain", -2);
        put(x + r - self.bx1, -1.0, 0.0, "terrain", -2);
        put(self.by0 + r - y, 0.0, 1.0, "terrain", -2);
        put(y + r - self.by1, 0.0, -1.0, "terrain", -2);
        (depth, nx, ny, kind, id)
    }

    pub fn is_walkable(&self, x: f64, y: f64, r: f64, mut max_z: f64, h: f64) -> bool {
        if x - r < self.bx0 || x + r > self.bx1 || y - r < self.by0 || y + r > self.by1 {
            return false;
        }
        if max_z.is_nan() {
            max_z = self.ground_at(x, y) + STEP_UP;
        }
        let z = self.surface_at(x, y, max_z);
        if z <= self.ground_at(x, y) + 0.01 {
            let (gx, gy) = self.ground_gradient(x, y);
            if hypot(gx, gy) > self.geo.max_slope_tan {
                return false;
            }
        }
        let (d, _, _, _, _) = self.penetration(x, y, z, r, h);
        d <= 0.001
    }

    pub fn move_and_slide(&self, mut x: f64, mut y: f64, mut z: f64, r: f64, h: f64, dx: f64, dy: f64, dz: f64) -> MoveResult {
        let (ox, oy, oz) = (x, y, z);
        let n = 1i64.max((hypot(dx, dy) / (r * 0.4)).ceil() as i64);
        let (sx, sy) = (dx / n as f64, dy / n as f64);
        let ms = STEP_UP;
        let mut walls = 0usize;
        let mut seen = [0i64; 8];
        for _k in 0..n {
            x += sx;
            y += sy;
            for _it in 0..4 {
                let (d, nx, ny, _, id) = self.penetration(x, y, z, r, h);
                if d <= 1e-4 {
                    break;
                }
                x += nx * (d + 0.001);
                y += ny * (d + 0.001);
                let mut dup = false;
                let mut i = 0;
                while i < walls && i < seen.len() {
                    if seen[i] == id {
                        dup = true;
                    }
                    i += 1;
                }
                if !dup && walls < seen.len() {
                    seen[walls] = id;
                    walls += 1;
                }
            }
            let s = self.surface_at(x, y, z + ms);
            if s > z && dz <= 0.0 {
                z = s;
            }
        }
        let mut res = MoveResult { x, y, walls: walls as i64, ..Default::default() };
        let surf = self.surface_at(x, y, z + ms);
        let mut nz = z + dz;
        if dz > 0.0 {
            let c = self.ceiling_above(x, y, z + h, r);
            if nz + h > c {
                nz = c - h;
                res.stopped_z = true;
            }
        }
        if nz <= surf + 1e-4 {
            nz = surf;
            res.grounded = true;
            res.stopped_z = res.stopped_z || dz < 0.0;
        } else if dz <= 0.0 && nz - surf <= 0.75 && oz - self.surface_at(ox, oy, oz + 0.01) < 0.05 {
            nz = surf;
            res.grounded = true;
        }
        res.z = nz;
        res
    }

    fn ray_prism(&self, pi: usize, x: f64, y: f64, z: f64, dx: f64, dy: f64, dz: f64, tmax: f64) -> (Hit, bool) {
        let p = &self.geo.prisms[pi];
        let (mut t0, mut t1) = (0.0f64, tmax);
        let mut slab = |o: f64, d: f64, lo: f64, hi: f64| -> bool {
            if d.abs() < 1e-9 {
                return o >= lo && o <= hi;
            }
            let (mut a, mut b) = ((lo - o) / d, (hi - o) / d);
            if a > b {
                std::mem::swap(&mut a, &mut b);
            }
            t0 = fmax(t0, a);
            t1 = fmin(t1, b);
            t0 <= t1
        };
        if !slab(x, dx, p.min_x, p.max_x) || !slab(y, dy, p.min_y, p.max_y) || !slab(z, dz, p.base, p.top) {
            return (Hit::default(), false);
        }
        if z >= p.base && z <= p.top && point_in_poly(&p.edges, x, y) {
            let l = (dx * dx + dy * dy + dz * dz).sqrt() + 1e-300;
            return (
                Hit { t: 0.0, x, y, z, nx: -dx / l, ny: -dy / l, nz: -dz / l, kind: p.kind, id: pi as i64 },
                true,
            );
        }
        let mut h = Hit::default();
        let mut ok = false;
        let mut best = tmax;
        let e = &p.edges;
        let mut i = 0;
        while i < e.len() {
            let (ex, ey) = (e[i + 2] - e[i], e[i + 3] - e[i + 1]);
            let den = dx * ey - dy * ex;
            if den.abs() < 1e-9 {
                i += 4;
                continue;
            }
            let (px, py) = (e[i] - x, e[i + 1] - y);
            let t = (px * ey - py * ex) / den;
            let s = (px * dy - py * dx) / den;
            if t < 0.0 || t >= best || s < 0.0 || s > 1.0 {
                i += 4;
                continue;
            }
            let hz = z + dz * t;
            if hz < p.base || hz > p.top {
                i += 4;
                continue;
            }
            let l = hypot(ex, ey);
            best = t;
            h = Hit { t, x: x + dx * t, y: y + dy * t, z: hz, nx: ey / l, ny: -ex / l, nz: 0.0, kind: p.kind, id: pi as i64 };
            ok = true;
            i += 4;
        }
        if dz.abs() > 1e-9 {
            for (cz, cn) in [(p.top, 1.0f64), (p.base, -1.0f64)] {
                let t = (cz - z) / dz;
                if t < 0.0 || t >= best || (cn > 0.0 && dz > 0.0) || (cn < 0.0 && dz < 0.0) {
                    continue;
                }
                let (hx, hy) = (x + dx * t, y + dy * t);
                if point_in_poly(e, hx, hy) {
                    best = t;
                    h = Hit { t, x: hx, y: hy, z: cz, nx: 0.0, ny: 0.0, nz: cn, kind: p.kind, id: pi as i64 };
                    ok = true;
                }
            }
        }
        (h, ok)
    }

    /// Pointer bundle handed to the Zig kernel (and mirrored by the Rust twin).
    fn terrain_view(&self) -> TerrainView {
        TerrainView {
            nx: self.nx,
            ny: self.ny,
            min_x: self.min_x,
            min_y: self.min_y,
            ground: self.ground.as_ptr(),
            bnx: self.geo.bnx,
            bny: self.geo.bny,
            block_max: self.geo.block_max.as_ptr(),
        }
    }

    /// Go's rayTerrain. Dispatches to Zig when built with the `zig` feature.
    pub fn ray_terrain(&self, x: f64, y: f64, z: f64, x1: f64, y1: f64, z1: f64, tmax: f64) -> (Hit, bool) {
        #[cfg(feature = "zig")]
        {
            self.ray_terrain_zig(x, y, z, x1, y1, z1, tmax)
        }
        #[cfg(not(feature = "zig"))]
        {
            self.ray_terrain_rust(x, y, z, x1, y1, z1, tmax)
        }
    }

    #[cfg(feature = "zig")]
    pub fn ray_terrain_zig(&self, x: f64, y: f64, z: f64, x1: f64, y1: f64, z1: f64, tmax: f64) -> (Hit, bool) {
        let tv = self.terrain_view();
        let mut th = THit::default();
        // SAFETY: tv points into self's live, unchanged ground/block_max buffers; the
        // kernel only reads within nx*ny and bnx*bny.
        let ok = unsafe { giddy_ray_terrain(&tv, x, y, z, x1, y1, z1, tmax, &mut th) };
        (th.to_hit(), ok)
    }

    /// Pure-Rust twin of zig/giddy_geom.zig `giddy_ray_terrain` (= Go rayTerrain).
    pub fn ray_terrain_rust(&self, x: f64, y: f64, z: f64, x1: f64, y1: f64, z1: f64, tmax: f64) -> (Hit, bool) {
        let tv = self.terrain_view();
        let mut th = THit::default();
        let ok = ray_terrain_impl(&tv, &self.ground, &self.geo.block_max, x, y, z, x1, y1, z1, tmax, &mut th);
        (th.to_hit(), ok)
    }

    pub fn raycast(&self, a: Vec3, b: Vec3, o: RayOpts) -> (Hit, bool) {
        let (x, y, z) = (a[0], a[1], a[2]);
        let (dx, dy, dz) = (b[0] - x, b[1] - y, b[2] - z);
        let mut best = Hit::default();
        let mut found = false;
        let mut seen_p: Vec<i32> = Vec::with_capacity(64);
        let mut seen_t: Vec<i32> = Vec::with_capacity(64);
        self.walk_cells(x, y, b[0], b[1], |c| {
            for &pi in &self.geo.grid_p[c] {
                if seen_p.contains(&pi) {
                    continue;
                }
                seen_p.push(pi);
                let tmax = if found { best.t } else { 1.0 };
                let (h, ok) = self.ray_prism(pi as usize, x, y, z, dx, dy, dz, tmax);
                if ok {
                    best = h;
                    found = true;
                }
            }
            for &ti in &self.geo.grid_t[c] {
                if seen_t.contains(&ti) {
                    continue;
                }
                seen_t.push(ti);
                let tr = &self.geo.trees[ti as usize];
                if !o.no_trunks {
                    let tmax = if found { best.t } else { 1.0 };
                    let (t, ok) = ray_trunk(tr.x, tr.y, tr.trunk_r, tr.z - 1.0, tr.trunk_top, x, y, z, dx, dy, dz, tmax);
                    if ok {
                        best = Hit { t, x: x + dx * t, y: y + dy * t, z: z + dz * t, kind: "trunk", id: ti as i64, ..Default::default() };
                        found = true;
                    }
                }
                if o.canopy {
                    let (t0, _, ok) = ray_crown(tr, x, y, z, dx, dy, dz);
                    let tmax = if found { best.t } else { 1.0 };
                    if ok && t0 < tmax {
                        let t = fmax(t0, 0.0);
                        best = Hit { t, x: x + dx * t, y: y + dy * t, z: z + dz * t, kind: "canopy", id: ti as i64, ..Default::default() };
                        found = true;
                    }
                }
            }
            if found {
                best.t
            } else {
                1.0
            }
        });
        if !o.no_terrain {
            let tmax = if found { best.t } else { 1.0 };
            let (h, ok) = self.ray_terrain(x, y, z, b[0], b[1], b[2], tmax);
            if ok {
                best = h;
                found = true;
            }
        }
        (best, found)
    }

    pub fn canopy_transmittance(&self, a: Vec3, b: Vec3) -> f64 {
        (-self.geo.rules.canopy_density * self.crown_meters(a, b, f64::INFINITY)).exp()
    }

    fn crown_meters(&self, a: Vec3, b: Vec3, limit: f64) -> f64 {
        let (x, y, z) = (a[0], a[1], a[2]);
        let (dx, dy, dz) = (b[0] - x, b[1] - y, b[2] - z);
        let l = (dx * dx + dy * dy + dz * dz).sqrt();
        let mut sum = 0.0;
        self.walk_cells_t(x, y, b[0], b[1], |c, c0, c1| {
            for &ti in &self.geo.grid_t[c] {
                let (t0, t1, ok) = ray_crown(&self.geo.trees[ti as usize], x, y, z, dx, dy, dz);
                if ok {
                    let (t0, t1) = (fmax(t0, c0), fmin(t1, c1));
                    if t1 > t0 {
                        sum += (t1 - t0) * l;
                    }
                }
            }
            if sum > limit {
                return -1.0;
            }
            1.0
        });
        sum
    }

    pub fn line_of_sight(&self, a: Vec3, b: Vec3) -> bool {
        if self.any_hit(a, b, true) {
            return false;
        }

        let lim = -self.geo.rules.canopy_block_t.ln() / self.geo.rules.canopy_density;
        self.crown_meters(a, b, lim) <= lim
    }

    pub fn shot_clear(&self, a: Vec3, b: Vec3) -> bool {
        !self.any_hit(a, b, false)
    }

    fn any_hit(&self, a: Vec3, b: Vec3, trunks: bool) -> bool {
        let (x, y, z) = (a[0], a[1], a[2]);
        let (dx, dy, dz) = (b[0] - x, b[1] - y, b[2] - z);
        let mut buf_p = [0i32; 64];
        let mut buf_t = [0i32; 64];
        let (mut np, mut nt) = (0usize, 0usize);
        let mut hit = false;
        self.walk_cells(x, y, b[0], b[1], |c| {
            'next_p: for &pi in &self.geo.grid_p[c] {
                for &u in &buf_p[..np] {
                    if u == pi {
                        continue 'next_p;
                    }
                }
                if np < buf_p.len() {
                    buf_p[np] = pi;
                    np += 1;
                }
                if self.ray_prism(pi as usize, x, y, z, dx, dy, dz, 1.0).1 {
                    hit = true;
                    return -1.0;
                }
            }
            if trunks {
                'next_t: for &ti in &self.geo.grid_t[c] {
                    for &u in &buf_t[..nt] {
                        if u == ti {
                            continue 'next_t;
                        }
                    }
                    if nt < buf_t.len() {
                        buf_t[nt] = ti;
                        nt += 1;
                    }
                    let tr = &self.geo.trees[ti as usize];
                    if ray_trunk(tr.x, tr.y, tr.trunk_r, tr.z - 1.0, tr.trunk_top, x, y, z, dx, dy, dz, 1.0).1 {
                        hit = true;
                        return -1.0;
                    }
                }
            }
            1.0
        });
        if hit {
            return true;
        }
        self.ray_terrain(x, y, z, b[0], b[1], b[2], 1.0).1
    }

    pub fn in_canopy(&self, x: f64, y: f64, z: f64) -> i64 {
        let c = self.cell_of(x, y);
        if c < 0 {
            return -1;
        }
        for &ti in &self.geo.grid_t[c as usize] {
            let t = &self.geo.trees[ti as usize];
            let (a, b, d) = ((x - t.cx) / t.crown_r, (y - t.cy) / t.crown_r, (z - t.cz) / t.rz);
            if a * a + b * b + d * d <= 1.0 {
                return ti as i64;
            }
        }
        -1
    }

    pub fn support_at(&self, x: f64, y: f64, feet_z: f64, r: f64) -> f64 {
        let z0 = self.surface_at(x, y, feet_z + STEP_UP);
        let mut z = z0;
        let lim = feet_z + 0.05;
        let re = r + 0.02;
        self.for_cells(x - re, y - re, x + re, y + re, |c| {
            for &pi in &self.geo.grid_p[c] {
                let p = &self.geo.prisms[pi as usize];

                if p.top <= z
                    || p.top > lim
                    || p.top <= z0 + STEP_UP
                    || x + re < p.min_x
                    || x - re > p.max_x
                    || y + re < p.min_y
                    || y - re > p.max_y
                {
                    continue;
                }
                let (_, _, d) = closest_on_edges(&p.edges, x, y);
                if d <= re || point_in_poly(&p.edges, x, y) {
                    z = p.top;
                }
            }
        });
        z
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct MoveResult {
    pub x: f64,
    pub y: f64,
    pub z: f64,
    pub grounded: bool,
    pub stopped_z: bool,
    pub walls: i64,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct RayOpts {
    pub no_trunks: bool,
    pub no_terrain: bool,
    pub canopy: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Hit {
    pub t: f64,
    pub x: f64,
    pub y: f64,
    pub z: f64,
    pub nx: f64,
    pub ny: f64,
    pub nz: f64,
    pub kind: &'static str,
    pub id: i64,
}

pub fn point_in_poly(e: &[f64], x: f64, y: f64) -> bool {
    let mut inside = false;
    let mut i = 0;
    while i < e.len() {
        let (x0, y0, x1, y1) = (e[i], e[i + 1], e[i + 2], e[i + 3]);
        if (y0 > y) != (y1 > y) && x < (x1 - x0) * (y - y0) / (y1 - y0) + x0 {
            inside = !inside;
        }
        i += 4;
    }
    inside
}

fn closest_on_edges(e: &[f64], x: f64, y: f64) -> (f64, f64, f64) {
    let mut best = f64::INFINITY;
    let (mut cx, mut cy) = (0.0, 0.0);
    let mut i = 0;
    while i < e.len() {
        let (x0, y0) = (e[i], e[i + 1]);
        let (ux, uy) = (e[i + 2] - x0, e[i + 3] - y0);
        let l2 = ux * ux + uy * uy;
        let mut p = 0.0;
        if l2 > 1e-9 {
            p = ((x - x0) * ux + (y - y0) * uy) / l2;
        }
        p = fmax(0.0, fmin(1.0, p));
        let (mx, my) = (x0 + ux * p, y0 + uy * p);
        let g = (mx - x) * (mx - x) + (my - y) * (my - y);
        if g < best {
            best = g;
            cx = mx;
            cy = my;
        }
        i += 4;
    }
    (cx, cy, best.sqrt())
}

fn ray_trunk(cx: f64, cy: f64, r: f64, z0: f64, z1: f64, x: f64, y: f64, z: f64, dx: f64, dy: f64, dz: f64, tmax: f64) -> (f64, bool) {
    let (px, py) = (x - cx, y - cy);
    let a = dx * dx + dy * dy;
    if a < 1e-9 {
        return (0.0, false);
    }
    let b = 2.0 * (px * dx + py * dy);
    let c = px * px + py * py - r * r;
    let disc = b * b - 4.0 * a * c;
    if disc < 0.0 {
        return (0.0, false);
    }
    let t = (-b - disc.sqrt()) / (2.0 * a);
    if t < 0.0 || t >= tmax {
        return (0.0, false);
    }
    let hz = z + dz * t;
    if hz < z0 || hz > z1 {
        return (0.0, false);
    }
    (t, true)
}

fn ray_crown(tr: &Tree, x: f64, y: f64, z: f64, dx: f64, dy: f64, dz: f64) -> (f64, f64, bool) {
    let (ox, oy, oz) = ((x - tr.cx) / tr.crown_r, (y - tr.cy) / tr.crown_r, (z - tr.cz) / tr.rz);
    let (ux, uy, uz) = (dx / tr.crown_r, dy / tr.crown_r, dz / tr.rz);
    let a = ux * ux + uy * uy + uz * uz;
    if a < 1e-9 {
        return (0.0, 0.0, false);
    }
    let b = 2.0 * (ox * ux + oy * uy + oz * uz);
    let c = ox * ox + oy * oy + oz * oz - 1.0;
    let disc = b * b - 4.0 * a * c;
    if disc <= 0.0 {
        return (0.0, 0.0, false);
    }
    let s = disc.sqrt();
    let (t0, t1) = ((-b - s) / (2.0 * a), (-b + s) / (2.0 * a));
    if t1 < 0.0 || t0 > 1.0 {
        return (0.0, 0.0, false);
    }
    (t0, t1, true)
}

// ------------------------------------------------------------------ terrain ray kernel
/// C layout shared with zig/giddy_geom.zig `Terrain`.
#[repr(C)]
pub struct TerrainView {
    pub nx: i64,
    pub ny: i64,
    pub min_x: f64,
    pub min_y: f64,
    pub ground: *const f32,
    pub bnx: i64,
    pub bny: i64,
    pub block_max: *const f32,
}

/// C layout shared with zig/giddy_geom.zig `THit` (Kind is always "terrain", ID -1).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct THit {
    pub t: f64,
    pub x: f64,
    pub y: f64,
    pub z: f64,
    pub nx: f64,
    pub ny: f64,
    pub nz: f64,
}

impl THit {
    fn to_hit(self) -> Hit {
        Hit { t: self.t, x: self.x, y: self.y, z: self.z, nx: self.nx, ny: self.ny, nz: self.nz, kind: "terrain", id: -1 }
    }
}

#[cfg(feature = "zig")]
extern "C" {
    fn giddy_ray_terrain(tv: *const TerrainView, x: f64, y: f64, z: f64, x1: f64, y1: f64, z1: f64, tmax: f64, out: *mut THit) -> bool;
}

#[inline]
fn tv_ground_at(tv: &TerrainView, ground: &[f32], x: f64, y: f64) -> f64 {
    let mut fx = x - tv.min_x;
    let mut fy = y - tv.min_y;
    fx = fmax(0.0, fmin(fx, (tv.nx - 1) as f64 - 1e-6));
    fy = fmax(0.0, fmin(fy, (tv.ny - 1) as f64 - 1e-6));
    let (i, j) = (fx as i64, fy as i64);
    let (tx, ty) = (fx - i as f64, fy - j as f64);
    let nx = tv.nx;
    let a = ground[(j * nx + i) as usize] as f64;
    let b = ground[(j * nx + i + 1) as usize] as f64;
    let c = ground[((j + 1) * nx + i) as usize] as f64;
    let d = ground[((j + 1) * nx + i + 1) as usize] as f64;
    (a * (1.0 - tx) + b * tx) * (1.0 - ty) + (c * (1.0 - tx) + d * tx) * ty
}

/// Go rayTerrain, line for line. Keep in lockstep with zig/giddy_geom.zig.
fn ray_terrain_impl(
    tv: &TerrainView,
    ground: &[f32],
    block_max: &[f32],
    x: f64,
    y: f64,
    z: f64,
    x1: f64,
    y1: f64,
    z1: f64,
    tmax: f64,
    out: &mut THit,
) -> bool {
    let (dx, dy, dz) = (x1 - x, y1 - y, z1 - z);
    let hl = hypot(dx, dy);
    const BLK: f64 = 8.0;
    let above = |t: f64| -> f64 { z + dz * t - tv_ground_at(tv, ground, x + dx * t, y + dy * t) };
    if above(0.0) < 0.0 {
        *out = THit { t: 0.0, x, y, z, nx: 0.0, ny: 0.0, nz: 1.0 };
        return true;
    }
    let mut step = 1.0;
    if hl > 1e-9 {
        step = 0.5 / hl;
    }
    let mut t = 0.0;
    let mut it: i64 = 0;
    while t < tmax {
        if it > 100000 {
            return false;
        }
        it += 1;
        let (px, py) = (x + dx * t, y + dy * t);
        let bi = ((px - tv.min_x) / BLK).floor() as i64;
        let bj = ((py - tv.min_y) / BLK).floor() as i64;
        let inb = bi >= 0 && bj >= 0 && bi < tv.bnx && bj < tv.bny;
        let mut tb = tmax;
        if hl > 1e-9 {
            let (ex, ey) = (tv.min_x + bi as f64 * BLK, tv.min_y + bj as f64 * BLK);
            let (mut nx, mut ny) = (f64::INFINITY, f64::INFINITY);
            if dx > 1e-9 {
                nx = (ex + BLK - px) / dx;
            } else if dx < -1e-9 {
                nx = (ex - px) / dx;
            }
            if dy > 1e-9 {
                ny = (ey + BLK - py) / dy;
            } else if dy < -1e-9 {
                ny = (ey - py) / dy;
            }
            tb = fmin(t + fmax(0.0, fmin(nx, ny)) + 1e-6, tmax);
        }
        if inb && fmin(z + dz * t, z + dz * tb) > block_max[(bj * tv.bnx + bi) as usize] as f64 {
            t = tb;
            continue;
        }
        let s = fmin(t + step, tmax);
        if above(s) < 0.0 {
            let (mut lo, mut hi) = (t, s);
            for _k in 0..20 {
                let m = (lo + hi) / 2.0;
                if above(m) < 0.0 {
                    hi = m;
                } else {
                    lo = m;
                }
            }
            let (hx, hy) = (x + dx * hi, y + dy * hi);
            let gx = (tv_ground_at(tv, ground, hx + 1.0, hy) - tv_ground_at(tv, ground, hx - 1.0, hy)) / 2.0;
            let gy = (tv_ground_at(tv, ground, hx, hy + 1.0) - tv_ground_at(tv, ground, hx, hy - 1.0)) / 2.0;
            let l = (gx * gx + gy * gy + 1.0).sqrt();
            *out = THit { t: hi, x: hx, y: hy, z: tv_ground_at(tv, ground, hx, hy), nx: -gx / l, ny: -gy / l, nz: 1.0 / l };
            return true;
        }
        if s >= tmax {
            break;
        }
        t = s;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gomath::Rand;

    fn kit_world() -> World {
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../kit/arena");
        World::load_world(dir).expect("load kit/arena")
    }

    #[test]
    fn loads_real_map() {
        let w = kit_world();
        assert!(w.geo.prisms.len() > 200);
        assert!(w.geo.trees.len() > 1000);
        assert!(w.main_comp > 0);
    }

    /// The Zig kernel and the Rust twin must agree bit for bit on many random rays over
    /// the real map (eye-height shots, ground-skimming rays, steep up/down rays, rays
    /// starting underground and leaving the map).
    #[cfg(feature = "zig")]
    #[test]
    fn zig_and_rust_ray_terrain_agree() {
        let w = kit_world();
        let mut r = Rand::new(12345);
        let (mut hits, mut n) = (0, 0);
        for k in 0..200_000 {
            let span = |r: &mut Rand, lo: f64, hi: f64| lo + (hi - lo) * r.float64();
            let x = span(&mut r, w.bx0 - 20.0, w.bx1 + 20.0);
            let y = span(&mut r, w.by0 - 20.0, w.by1 + 20.0);
            let len = match k % 4 {
                0 => span(&mut r, 0.0, 5.0),
                1 => span(&mut r, 5.0, 60.0),
                _ => span(&mut r, 60.0, 400.0),
            };
            let a = span(&mut r, 0.0, std::f64::consts::TAU);
            let g = w.ground_at(x, y);
            let z = match k % 5 {
                0 => g + span(&mut r, -2.0, 0.5),
                1 => g + span(&mut r, 0.0, 3.0),
                _ => g + span(&mut r, 3.0, 40.0),
            };
            let x1 = x + a.cos() * len;
            let y1 = y + a.sin() * len;
            let z1 = if k % 7 == 0 { z } else { w.ground_at(x1, y1) + span(&mut r, -5.0, 30.0) };
            let tmax = if k % 3 == 0 { span(&mut r, 0.0, 1.0) } else { 1.0 };
            let (hz, okz) = w.ray_terrain_zig(x, y, z, x1, y1, z1, tmax);
            let (hr, okr) = w.ray_terrain_rust(x, y, z, x1, y1, z1, tmax);
            assert_eq!(okz, okr, "ok differs for ray {k}: ({x},{y},{z})->({x1},{y1},{z1}) tmax {tmax}");
            if okz {
                assert_eq!(hz, hr, "hit differs for ray {k}");
                hits += 1;
            }
            n += 1;
        }
        assert!(hits > n / 20 && hits < n, "degenerate test: {hits}/{n} hits");
    }
}
