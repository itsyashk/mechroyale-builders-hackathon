//! 2 m navigation grid at ground level with obstacle clearance, plus A* (in Zig) and
//! line-of-walk path smoothing.

use crate::{geom_astar, AStarState, World};

pub struct Nav {
    pub res: f32,
    pub cnx: usize,
    pub cny: usize,
    pub ox: f32,
    pub oy: f32,
    /// Clearance (metres to nearest blocked 1 m vertex) at each nav cell centre.
    pub clearance: Vec<f32>,
    /// Ground height at each nav cell centre.
    pub height: Vec<f32>,
    /// Static cost (>= 1, or -1 blocked).
    pub base_cost: Vec<f32>,
    /// Working cost the A* reads: base_cost plus dynamic penalties.
    pub cost: Vec<f32>,
    g: Vec<f32>,
    f: Vec<f32>,
    parent: Vec<i32>,
    stamp: Vec<u32>,
    closed: Vec<u32>,
    heap: Vec<u32>,
    heap_pos: Vec<i32>,
    cur: u32,
    out: Vec<u32>,
    dirty: Vec<usize>,
}

pub const MECH_CLEARANCE: f32 = 2.7;

impl Nav {
    pub fn build(w: &World) -> Nav {
        let (nx, ny) = (w.nx, w.ny);
        // 1 m blocked raster on terrain vertices.
        let max_rise = (50.0f32).to_radians().tan() * w.step * 0.95;
        let mut blocked = vec![false; nx * ny];
        for j in 0..ny {
            for i in 0..nx {
                let k = j * nx + i;
                let x = w.min_x + i as f32 * w.step;
                let y = w.min_y + j as f32 * w.step;
                if !w.in_bounds(x, y, 1.0) {
                    blocked[k] = true;
                    continue;
                }
                let g = w.terrain[k];
                let mut b = false;
                for (lo, hi) in [(w.lo0[k], w.hi0[k]), (w.lo1[k], w.hi1[k])] {
                    if lo <= hi && hi > g + 1.15 && lo < g + 9.3 {
                        b = true;
                    }
                }
                if !b {
                    // steep terrain
                    for (di, dj) in [(1i32, 0i32), (0, 1)] {
                        let (ii, jj) = (i as i32 + di, j as i32 + dj);
                        if ii < nx as i32 && jj < ny as i32 {
                            let h2 = w.terrain[jj as usize * nx + ii as usize];
                            if (h2 - g).abs() > max_rise {
                                b = true;
                            }
                        }
                    }
                }
                blocked[k] = b;
            }
        }
        // Chamfer distance transform (3-4 weights approx Euclidean), in metres.
        let inf = 1e9f32;
        let mut d: Vec<f32> = blocked.iter().map(|&b| if b { 0.0 } else { inf }).collect();
        let (a, dg) = (1.0f32, 1.41421356f32);
        for j in 0..ny {
            for i in 0..nx {
                let k = j * nx + i;
                let mut v = d[k];
                if i > 0 {
                    v = v.min(d[k - 1] + a);
                }
                if j > 0 {
                    v = v.min(d[k - nx] + a);
                    if i > 0 {
                        v = v.min(d[k - nx - 1] + dg);
                    }
                    if i + 1 < nx {
                        v = v.min(d[k - nx + 1] + dg);
                    }
                }
                d[k] = v;
            }
        }
        for j in (0..ny).rev() {
            for i in (0..nx).rev() {
                let k = j * nx + i;
                let mut v = d[k];
                if i + 1 < nx {
                    v = v.min(d[k + 1] + a);
                }
                if j + 1 < ny {
                    v = v.min(d[k + nx] + a);
                    if i + 1 < nx {
                        v = v.min(d[k + nx + 1] + dg);
                    }
                    if i > 0 {
                        v = v.min(d[k + nx - 1] + dg);
                    }
                }
                d[k] = v;
            }
        }

        let res = 2.0f32;
        let cnx = nx / 2;
        let cny = ny / 2;
        let ox = w.min_x + 1.0; // centre of cell 0 sits on vertex 1
        let oy = w.min_y + 1.0;
        let n = cnx * cny;
        let mut clearance = vec![0.0f32; n];
        let mut height = vec![0.0f32; n];
        let mut base_cost = vec![-1.0f32; n];
        for cj in 0..cny {
            for ci in 0..cnx {
                let (vi, vj) = (ci * 2 + 1, cj * 2 + 1);
                if vi >= nx || vj >= ny {
                    continue;
                }
                let k = vj * nx + vi;
                let c = cj * cnx + ci;
                clearance[c] = d[k];
                height[c] = w.terrain[k];
                if d[k] >= MECH_CLEARANCE {
                    base_cost[c] = if d[k] < 5.0 { 1.4 } else { 1.0 };
                }
            }
        }
        Nav {
            res,
            cnx,
            cny,
            ox,
            oy,
            clearance,
            height,
            cost: base_cost.clone(),
            base_cost,
            g: vec![0.0; n],
            f: vec![0.0; n],
            parent: vec![-1; n],
            stamp: vec![0; n],
            closed: vec![0; n],
            heap: vec![0; n + 8],
            heap_pos: vec![-1; n],
            cur: 0,
            out: vec![0; 8192],
            dirty: Vec::new(),
        }
    }

    #[inline]
    pub fn cell(&self, x: f32, y: f32) -> Option<usize> {
        let fi = ((x - self.ox) / self.res + 0.5).floor();
        let fj = ((y - self.oy) / self.res + 0.5).floor();
        if fi < 0.0 || fj < 0.0 || fi >= self.cnx as f32 || fj >= self.cny as f32 {
            return None;
        }
        Some(fj as usize * self.cnx + fi as usize)
    }

    #[inline]
    pub fn center(&self, c: usize) -> [f32; 2] {
        [self.ox + (c % self.cnx) as f32 * self.res, self.oy + (c / self.cnx) as f32 * self.res]
    }

    #[inline]
    pub fn walkable(&self, c: usize) -> bool {
        self.base_cost[c] >= 0.0
    }

    pub fn walkable_at(&self, x: f32, y: f32) -> bool {
        self.cell(x, y).map(|c| self.walkable(c)).unwrap_or(false)
    }

    pub fn clearance_at(&self, x: f32, y: f32) -> f32 {
        self.cell(x, y).map(|c| self.clearance[c]).unwrap_or(0.0)
    }

    /// Nearest walkable cell within `max_r` metres (spiral search).
    pub fn nearest_walkable(&self, x: f32, y: f32, max_r: f32) -> Option<usize> {
        let ci = ((x - self.ox) / self.res).round() as i32;
        let cj = ((y - self.oy) / self.res).round() as i32;
        let rmax = (max_r / self.res).ceil() as i32;
        let mut best: Option<(f32, usize)> = None;
        for r in 0..=rmax {
            for dj in -r..=r {
                for di in -r..=r {
                    if di.abs() != r && dj.abs() != r {
                        continue;
                    }
                    let (i, j) = (ci + di, cj + dj);
                    if i < 0 || j < 0 || i >= self.cnx as i32 || j >= self.cny as i32 {
                        continue;
                    }
                    let c = j as usize * self.cnx + i as usize;
                    if self.walkable(c) {
                        let p = self.center(c);
                        let dd = (p[0] - x).powi(2) + (p[1] - y).powi(2);
                        if best.map(|b| dd < b.0).unwrap_or(true) {
                            best = Some((dd, c));
                        }
                    }
                }
            }
            if best.is_some() {
                // one more ring could hold a closer cell diagonally; good enough
                break;
            }
        }
        best.map(|b| b.1)
    }

    /// Clears dynamic penalties.
    pub fn reset_cost(&mut self) {
        for &c in &self.dirty {
            self.cost[c] = self.base_cost[c];
        }
        self.dirty.clear();
    }

    /// Adds `amount` to walkable cells within `r` of (x, y), falling off linearly.
    pub fn add_cost_disc(&mut self, x: f32, y: f32, r: f32, amount: f32) {
        let ri = (r / self.res).ceil() as i32;
        let ci = ((x - self.ox) / self.res).round() as i32;
        let cj = ((y - self.oy) / self.res).round() as i32;
        for dj in -ri..=ri {
            for di in -ri..=ri {
                let (i, j) = (ci + di, cj + dj);
                if i < 0 || j < 0 || i >= self.cnx as i32 || j >= self.cny as i32 {
                    continue;
                }
                let c = j as usize * self.cnx + i as usize;
                if self.base_cost[c] < 0.0 {
                    continue;
                }
                let d = ((di * di + dj * dj) as f32).sqrt() * self.res;
                if d > r {
                    continue;
                }
                if self.cost[c] == self.base_cost[c] {
                    self.dirty.push(c);
                }
                self.cost[c] += amount * (1.0 - d / r);
            }
        }
    }

    /// True when a mech can walk straight from a to b (every 1 m sample walkable).
    pub fn line_walkable(&self, a: [f32; 2], b: [f32; 2]) -> bool {
        let dx = b[0] - a[0];
        let dy = b[1] - a[1];
        let len = (dx * dx + dy * dy).sqrt();
        let n = (len / 1.0).ceil().max(1.0) as usize;
        for k in 0..=n {
            let t = k as f32 / n as f32;
            if !self.walkable_at(a[0] + dx * t, a[1] + dy * t) {
                return false;
            }
        }
        true
    }

    /// A* path from `from` to `to`, smoothed. Returns waypoints excluding the start.
    /// If `to` is unreachable, the path ends at the closest reachable point.
    pub fn path(&mut self, from: [f32; 2], to: [f32; 2], max_expand: u32) -> Vec<[f32; 2]> {
        let s = match self.cell(from[0], from[1]).filter(|&c| self.walkable(c)).or_else(|| self.nearest_walkable(from[0], from[1], 12.0)) {
            Some(c) => c,
            None => return vec![to],
        };
        let g = match self.cell(to[0], to[1]).filter(|&c| self.walkable(c)).or_else(|| self.nearest_walkable(to[0], to[1], 40.0)) {
            Some(c) => c,
            None => return vec![to],
        };
        self.cur = self.cur.wrapping_add(1);
        if self.cur == 0 {
            self.stamp.iter_mut().for_each(|v| *v = 0);
            self.closed.iter_mut().for_each(|v| *v = 0);
            self.cur = 1;
        }
        let mut st = AStarState {
            nx: self.cnx as u32,
            ny: self.cny as u32,
            cost: self.cost.as_ptr(),
            g: self.g.as_mut_ptr(),
            f: self.f.as_mut_ptr(),
            parent: self.parent.as_mut_ptr(),
            stamp: self.stamp.as_mut_ptr(),
            closed: self.closed.as_mut_ptr(),
            heap: self.heap.as_mut_ptr(),
            heap_pos: self.heap_pos.as_mut_ptr(),
            cur: self.cur,
        };
        let cap = self.out.len() as u32;
        let n = unsafe { geom_astar(&mut st, s as u32, g as u32, max_expand, self.out.as_mut_ptr(), cap) } as usize;
        let cells: Vec<[f32; 2]> = self.out[..n].iter().map(|&c| self.center(c as usize)).collect();
        if cells.is_empty() {
            return vec![to];
        }
        // String pulling.
        let mut pts = Vec::new();
        let mut anchor = from;
        let mut i = 0;
        while i < cells.len() {
            let mut j = cells.len() - 1;
            while j > i && !self.line_walkable(anchor, cells[j]) {
                j = (i + j) / 2 + if (i + j) / 2 == i { 0 } else { 0 };
                if j <= i + 1 {
                    j = i;
                    break;
                }
            }
            // linear scan back from the furthest visible to refine
            let mut k = j;
            while k + 1 < cells.len() && self.line_walkable(anchor, cells[k + 1]) {
                k += 1;
            }
            pts.push(cells[k]);
            anchor = cells[k];
            i = k + 1;
        }
        if *self.out[..n].last().unwrap() as usize == g {
            if let Some(last) = pts.last_mut() {
                if self.walkable_at(to[0], to[1]) {
                    *last = to;
                }
            }
        }
        pts
    }
}
