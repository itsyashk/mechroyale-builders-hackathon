//! Static world model shared by the bot and the local simulator.
//!
//! The map (`v2/world.json` + `v2/terrain.i16`) is rasterised onto the 1 m terrain vertex
//! grid as up to two solid spans per cell. Line of sight and A* run in Zig (`zig/geom.zig`).

use std::path::Path;

pub mod nav;
pub use nav::Nav;

#[repr(C)]
pub struct Grid {
    pub nx: u32,
    pub ny: u32,
    pub min_x: f32,
    pub min_y: f32,
    pub step: f32,
    pub terrain: *const f32,
    pub lo0: *const f32,
    pub hi0: *const f32,
    pub lo1: *const f32,
    pub hi1: *const f32,
}

#[repr(C)]
pub struct AStarState {
    pub nx: u32,
    pub ny: u32,
    pub cost: *const f32,
    pub g: *mut f32,
    pub f: *mut f32,
    pub parent: *mut i32,
    pub stamp: *mut u32,
    pub closed: *mut u32,
    pub heap: *mut u32,
    pub heap_pos: *mut i32,
    pub cur: u32,
}

extern "C" {
    fn geom_terrain(g: *const Grid, x: f32, y: f32) -> f32;
    fn geom_solid(g: *const Grid, x: f32, y: f32, z: f32) -> bool;
    fn geom_raycast(g: *const Grid, ax: f32, ay: f32, az: f32, dx: f32, dy: f32, dz: f32, max_d: f32) -> f32;
    fn geom_los(g: *const Grid, ax: f32, ay: f32, az: f32, bx: f32, by: f32, bz: f32) -> bool;
    pub(crate) fn geom_astar(s: *mut AStarState, start: u32, goal: u32, max_expand: u32, out: *mut u32, out_cap: u32) -> u32;
}

#[derive(Clone, Debug)]
pub struct Prism {
    pub kind: String,
    pub base: f32,
    pub top: f32,
    pub ring: Vec<[f32; 2]>,
    pub holes: Vec<Vec<[f32; 2]>>,
    pub bbox: [f32; 4],
}

#[derive(Clone, Copy, Debug)]
pub struct Tree {
    pub x: f32,
    pub y: f32,
    pub z: f32,
    pub height: f32,
    pub crown_r: f32,
    pub crown_base: f32,
    pub trunk_r: f32,
    pub crown_x: f32,
    pub crown_y: f32,
}

pub struct World {
    pub nx: usize,
    pub ny: usize,
    pub min_x: f32,
    pub min_y: f32,
    pub step: f32,
    pub bounds: [f32; 4], // minX, maxX, minY, maxY
    pub terrain: Vec<f32>,
    pub lo0: Vec<f32>,
    pub hi0: Vec<f32>,
    pub lo1: Vec<f32>,
    pub hi1: Vec<f32>,
    pub prisms: Vec<Prism>,
    pub trees: Vec<Tree>,
    grid: Grid,
}

// The raw pointers in `grid` point into the Vecs owned by the same struct, which never
// reallocate after construction.
unsafe impl Send for World {}
unsafe impl Sync for World {}

fn flat_pts(v: &serde_json::Value) -> Vec<[f32; 2]> {
    let a = v.as_array().map(|a| a.as_slice()).unwrap_or(&[]);
    a.chunks(2)
        .filter(|c| c.len() == 2)
        .map(|c| [c[0].as_f64().unwrap_or(0.0) as f32, c[1].as_f64().unwrap_or(0.0) as f32])
        .collect()
}

pub fn point_in_poly(p: &[[f32; 2]], x: f32, y: f32) -> bool {
    let mut inside = false;
    let n = p.len();
    if n < 3 {
        return false;
    }
    let mut j = n - 1;
    for i in 0..n {
        let (xi, yi) = (p[i][0], p[i][1]);
        let (xj, yj) = (p[j][0], p[j][1]);
        if (yi > y) != (yj > y) && x < (xj - xi) * (y - yi) / (yj - yi) + xi {
            inside = !inside;
        }
        j = i;
    }
    inside
}

impl Prism {
    pub fn contains_xy(&self, x: f32, y: f32) -> bool {
        if x < self.bbox[0] || x > self.bbox[1] || y < self.bbox[2] || y > self.bbox[3] {
            return false;
        }
        point_in_poly(&self.ring, x, y) && !self.holes.iter().any(|h| point_in_poly(h, x, y))
    }
}

impl World {
    pub fn load(dir: impl AsRef<Path>) -> Result<World, String> {
        let dir = dir.as_ref();
        let wpath = dir.join("v2/world.json");
        let txt = std::fs::read_to_string(&wpath).map_err(|e| format!("{}: {e}", wpath.display()))?;
        let w: serde_json::Value = serde_json::from_str(&txt).map_err(|e| e.to_string())?;
        let t = &w["terrain"];
        let nx = t["nx"].as_u64().ok_or("terrain.nx")? as usize;
        let ny = t["ny"].as_u64().ok_or("terrain.ny")? as usize;
        let min_x = t["minX"].as_f64().ok_or("terrain.minX")? as f32;
        let min_y = t["minY"].as_f64().ok_or("terrain.minY")? as f32;
        let step = t["step"].as_f64().unwrap_or(1.0) as f32;
        let scale = t["scale"].as_f64().unwrap_or(0.01) as f32;
        let file = t["file"].as_str().unwrap_or("terrain.i16");
        let tpath = dir.join("v2").join(file);
        let raw = std::fs::read(&tpath).map_err(|e| format!("{}: {e}", tpath.display()))?;
        if raw.len() < nx * ny * 2 {
            return Err("terrain too short".into());
        }
        let terrain: Vec<f32> = raw
            .chunks_exact(2)
            .take(nx * ny)
            .map(|c| i16::from_le_bytes([c[0], c[1]]) as f32 * scale)
            .collect();
        let b = &w["bounds"];
        let bounds = [
            b["minX"].as_f64().unwrap_or(min_x as f64) as f32,
            b["maxX"].as_f64().unwrap_or((min_x + nx as f32) as f64) as f32,
            b["minY"].as_f64().unwrap_or(min_y as f64) as f32,
            b["maxY"].as_f64().unwrap_or((min_y + ny as f32) as f64) as f32,
        ];

        let mut prisms = Vec::new();
        for p in w["prisms"].as_array().cloned().unwrap_or_default() {
            let ring = flat_pts(&p["ring"]);
            if ring.len() < 3 {
                continue;
            }
            let holes: Vec<Vec<[f32; 2]>> = match &p["holes"] {
                serde_json::Value::Array(hs) => {
                    // Either [[x,y,...], ...] or a single flat array.
                    if hs.first().map(|h| h.is_array()).unwrap_or(false) {
                        hs.iter().map(flat_pts).filter(|h| h.len() >= 3).collect()
                    } else if !hs.is_empty() {
                        vec![flat_pts(&p["holes"])]
                    } else {
                        vec![]
                    }
                }
                _ => vec![],
            };
            let mut bb = [f32::MAX, f32::MIN, f32::MAX, f32::MIN];
            for q in &ring {
                bb[0] = bb[0].min(q[0]);
                bb[1] = bb[1].max(q[0]);
                bb[2] = bb[2].min(q[1]);
                bb[3] = bb[3].max(q[1]);
            }
            prisms.push(Prism {
                kind: p["kind"].as_str().unwrap_or("building").to_string(),
                base: p["base"].as_f64().unwrap_or(0.0) as f32,
                top: p["top"].as_f64().unwrap_or(0.0) as f32,
                ring,
                holes,
                bbox: bb,
            });
        }

        let mut trees = Vec::new();
        let tr = &w["trees"];
        if let (Some(fields), Some(data)) = (tr["fields"].as_array(), tr["data"].as_array()) {
            let f: Vec<&str> = fields.iter().map(|v| v.as_str().unwrap_or("")).collect();
            let idx = |name: &str| f.iter().position(|n| *n == name);
            let nf = f.len().max(1);
            let get = |row: &[serde_json::Value], name: &str, d: f32| -> f32 {
                idx(name).and_then(|i| row.get(i)).and_then(|v| v.as_f64()).map(|v| v as f32).unwrap_or(d)
            };
            for row in data.chunks(nf) {
                let x = get(row, "x", 0.0);
                let y = get(row, "y", 0.0);
                trees.push(Tree {
                    x,
                    y,
                    z: get(row, "z", 0.0),
                    height: get(row, "height", 10.0),
                    crown_r: get(row, "crownR", 3.0),
                    crown_base: get(row, "crownBase", 3.0),
                    trunk_r: get(row, "trunkR", 0.3),
                    crown_x: get(row, "crownX", x),
                    crown_y: get(row, "crownY", y),
                });
            }
        }

        // Rasterise prisms into per-vertex span lists.
        let n = nx * ny;
        let mut spans: Vec<Vec<(f32, f32)>> = vec![Vec::new(); n];
        for p in &prisms {
            let i0 = (((p.bbox[0] - min_x) / step).floor().max(0.0)) as usize;
            let i1 = (((p.bbox[1] - min_x) / step).ceil() as usize).min(nx - 1);
            let j0 = (((p.bbox[2] - min_y) / step).floor().max(0.0)) as usize;
            let j1 = (((p.bbox[3] - min_y) / step).ceil() as usize).min(ny - 1);
            for j in j0..=j1 {
                let y = min_y + j as f32 * step;
                for i in i0..=i1 {
                    let x = min_x + i as f32 * step;
                    if p.contains_xy(x, y) {
                        spans[j * nx + i].push((p.base, p.top));
                    }
                }
            }
        }
        let mut lo0 = vec![1.0f32; n];
        let mut hi0 = vec![-1.0f32; n];
        let mut lo1 = vec![1.0f32; n];
        let mut hi1 = vec![-1.0f32; n];
        for (k, s) in spans.iter_mut().enumerate() {
            if s.is_empty() {
                continue;
            }
            s.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
            let mut m: Vec<(f32, f32)> = Vec::new();
            for &(lo, hi) in s.iter() {
                if let Some(last) = m.last_mut() {
                    if lo <= last.1 + 0.05 {
                        last.1 = last.1.max(hi);
                        continue;
                    }
                }
                m.push((lo, hi));
            }
            while m.len() > 2 {
                // Merge the pair with the smallest gap (conservative: fills the gap).
                let mut bi = 0;
                let mut bg = f32::MAX;
                for i in 0..m.len() - 1 {
                    let gap = m[i + 1].0 - m[i].1;
                    if gap < bg {
                        bg = gap;
                        bi = i;
                    }
                }
                m[bi].1 = m[bi].1.max(m[bi + 1].1);
                m.remove(bi + 1);
            }
            lo0[k] = m[0].0;
            hi0[k] = m[0].1;
            if m.len() > 1 {
                lo1[k] = m[1].0;
                hi1[k] = m[1].1;
            }
        }

        let mut w = World {
            nx,
            ny,
            min_x,
            min_y,
            step,
            bounds,
            terrain,
            lo0,
            hi0,
            lo1,
            hi1,
            prisms,
            trees,
            grid: Grid {
                nx: nx as u32,
                ny: ny as u32,
                min_x,
                min_y,
                step,
                terrain: std::ptr::null(),
                lo0: std::ptr::null(),
                hi0: std::ptr::null(),
                lo1: std::ptr::null(),
                hi1: std::ptr::null(),
            },
        };
        w.grid.terrain = w.terrain.as_ptr();
        w.grid.lo0 = w.lo0.as_ptr();
        w.grid.hi0 = w.hi0.as_ptr();
        w.grid.lo1 = w.lo1.as_ptr();
        w.grid.hi1 = w.hi1.as_ptr();
        Ok(w)
    }

    #[inline]
    pub fn ground(&self, x: f32, y: f32) -> f32 {
        unsafe { geom_terrain(&self.grid, x, y) }
    }

    #[inline]
    pub fn solid(&self, x: f32, y: f32, z: f32) -> bool {
        unsafe { geom_solid(&self.grid, x, y, z) }
    }

    /// Distance to the first solid point along `d` from `a`, capped at `max_d`.
    #[inline]
    pub fn raycast(&self, a: [f32; 3], d: [f32; 3], max_d: f32) -> f32 {
        unsafe { geom_raycast(&self.grid, a[0], a[1], a[2], d[0], d[1], d[2], max_d) }
    }

    #[inline]
    pub fn los(&self, a: [f32; 3], b: [f32; 3]) -> bool {
        unsafe { geom_los(&self.grid, a[0], a[1], a[2], b[0], b[1], b[2]) }
    }

    /// Spans of the 1 m vertex nearest (x, y).
    pub fn spans_at(&self, x: f32, y: f32) -> [(f32, f32); 2] {
        match self.vidx(x, y) {
            Some(k) => [(self.lo0[k], self.hi0[k]), (self.lo1[k], self.hi1[k])],
            None => [(1.0, -1.0), (1.0, -1.0)],
        }
    }

    pub fn vidx(&self, x: f32, y: f32) -> Option<usize> {
        let fx = (x - self.min_x) / self.step + 0.5;
        let fy = (y - self.min_y) / self.step + 0.5;
        if fx < 0.0 || fy < 0.0 {
            return None;
        }
        let (i, j) = (fx as usize, fy as usize);
        if i >= self.nx || j >= self.ny {
            return None;
        }
        Some(j * self.nx + i)
    }

    /// Highest walkable surface under (x, y) that is at or below `z + up` (feet height).
    /// Considers terrain and the tops of solid spans.
    pub fn surface_below(&self, x: f32, y: f32, z: f32, up: f32) -> f32 {
        let mut s = self.ground(x, y);
        for (lo, hi) in self.spans_at(x, y) {
            if lo <= hi && hi <= z + up && hi > s {
                s = hi;
            }
        }
        s
    }

    pub fn in_bounds(&self, x: f32, y: f32, margin: f32) -> bool {
        x > self.bounds[0] + margin && x < self.bounds[1] - margin && y > self.bounds[2] + margin && y < self.bounds[3] - margin
    }
}
