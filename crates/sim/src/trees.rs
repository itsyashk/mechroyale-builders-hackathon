//! Tree occlusion for vision: trunks block, crowns attenuate (Beer-Lambert style: a line is
//! blocked once exp(-density * chord_length_through_crowns) drops below `block_t`).

use crate::util::V3;
use arena::World;

pub struct TreeGrid {
    cell: f32,
    nx: usize,
    ny: usize,
    min_x: f32,
    min_y: f32,
    cells: Vec<Vec<u32>>,
    trees: Vec<arena::Tree>,
    density: f32,
    block_t: f32,
}

impl TreeGrid {
    pub fn build(w: &World, density: f32, block_t: f32) -> TreeGrid {
        let cell = 8.0f32;
        let min_x = w.bounds[0] - 10.0;
        let min_y = w.bounds[2] - 10.0;
        let nx = ((w.bounds[1] - min_x + 10.0) / cell).ceil() as usize + 1;
        let ny = ((w.bounds[3] - min_y + 10.0) / cell).ceil() as usize + 1;
        let mut cells = vec![Vec::new(); nx * ny];
        for (k, t) in w.trees.iter().enumerate() {
            let r = t.crown_r.max(t.trunk_r) + 0.5;
            let xs = [t.x.min(t.crown_x) - r, t.x.max(t.crown_x) + r];
            let ys = [t.y.min(t.crown_y) - r, t.y.max(t.crown_y) + r];
            let i0 = ((xs[0] - min_x) / cell).floor().max(0.0) as usize;
            let i1 = (((xs[1] - min_x) / cell).floor() as usize).min(nx - 1);
            let j0 = ((ys[0] - min_y) / cell).floor().max(0.0) as usize;
            let j1 = (((ys[1] - min_y) / cell).floor() as usize).min(ny - 1);
            for j in j0..=j1 {
                for i in i0..=i1 {
                    cells[j * nx + i].push(k as u32);
                }
            }
        }
        TreeGrid { cell, nx, ny, min_x, min_y, cells, trees: w.trees.clone(), density, block_t }
    }

    /// True when trees block sight between a and b.
    pub fn blocks(&self, a: V3, b: V3) -> bool {
        let dx = b[0] - a[0];
        let dy = b[1] - a[1];
        let dz = b[2] - a[2];
        let l2 = dx * dx + dy * dy;
        let len = (l2 + dz * dz).sqrt();
        if len < 0.5 {
            return false;
        }
        // Visit grid cells along the segment (sample at half-cell spacing, plus neighbours
        // implicitly covered because trees are registered in every cell their crown touches).
        let n = ((l2.sqrt() / (self.cell * 0.5)).ceil() as usize).max(1);
        let mut last = usize::MAX;
        let mut seen: [u32; 64] = [u32::MAX; 64];
        let mut nseen = 0usize;
        let mut optical = 0.0f32;
        let limit = -(self.block_t.max(1e-6)).ln() / self.density.max(1e-6);
        for s in 0..=n {
            let t = s as f32 / n as f32;
            let x = a[0] + dx * t;
            let y = a[1] + dy * t;
            let i = ((x - self.min_x) / self.cell).floor();
            let j = ((y - self.min_y) / self.cell).floor();
            if i < 0.0 || j < 0.0 || i as usize >= self.nx || j as usize >= self.ny {
                continue;
            }
            let c = j as usize * self.nx + i as usize;
            if c == last {
                continue;
            }
            last = c;
            for &k in &self.cells[c] {
                if seen[..nseen.min(64)].contains(&k) {
                    continue;
                }
                if nseen < 64 {
                    seen[nseen] = k;
                }
                nseen += 1;
                let tr = &self.trees[k as usize];
                // Trunk: vertical cylinder from z to z + crown_base (it continues inside the crown,
                // but the crown already handles that part).
                if seg_hits_vcyl(a, [dx, dy, dz], tr.x, tr.y, tr.trunk_r, tr.z, tr.z + tr.crown_base.max(1.0)) {
                    return true;
                }
                // Crown: ellipsoid between crown_base and height.
                let hz = (tr.height - tr.crown_base).max(0.5) * 0.5;
                let cz = tr.z + tr.crown_base + hz;
                let r = tr.crown_r.max(0.3);
                let ox = (a[0] - tr.crown_x) / r;
                let oy = (a[1] - tr.crown_y) / r;
                let oz = (a[2] - cz) / hz;
                let ux = dx / r;
                let uy = dy / r;
                let uz = dz / hz;
                let qa = ux * ux + uy * uy + uz * uz;
                let qb = 2.0 * (ox * ux + oy * uy + oz * uz);
                let qc = ox * ox + oy * oy + oz * oz - 1.0;
                let disc = qb * qb - 4.0 * qa * qc;
                if disc > 0.0 {
                    let sq = disc.sqrt();
                    let t0 = ((-qb - sq) / (2.0 * qa)).max(0.0);
                    let t1 = ((-qb + sq) / (2.0 * qa)).min(1.0);
                    if t1 > t0 {
                        optical += (t1 - t0) * len;
                        if optical > limit {
                            return true;
                        }
                    }
                }
            }
        }
        false
    }
}

/// Segment a + d*t, t in [0,1], against a vertical cylinder (axis at x, y; z in [z0, z1]).
fn seg_hits_vcyl(a: V3, d: V3, x: f32, y: f32, r: f32, z0: f32, z1: f32) -> bool {
    let ox = a[0] - x;
    let oy = a[1] - y;
    let qa = d[0] * d[0] + d[1] * d[1];
    if qa < 1e-9 {
        return false;
    }
    let qb = 2.0 * (ox * d[0] + oy * d[1]);
    let qc = ox * ox + oy * oy - r * r;
    let disc = qb * qb - 4.0 * qa * qc;
    if disc <= 0.0 {
        return false;
    }
    let sq = disc.sqrt();
    let t0 = ((-qb - sq) / (2.0 * qa)).max(0.02);
    let t1 = ((-qb + sq) / (2.0 * qa)).min(0.98);
    if t1 <= t0 {
        return false;
    }
    let za = a[2] + d[2] * t0;
    let zb = a[2] + d[2] * t1;
    za.max(zb) >= z0 && za.min(zb) <= z1
}
