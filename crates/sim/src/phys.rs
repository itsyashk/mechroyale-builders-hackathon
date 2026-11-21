//! Cylinder-vs-world movement: wall sliding, step-up, slopes, roofs, ceilings, map edge.

use crate::util::V3;
use arena::World;

const RING_N: usize = 16;

pub struct Body {
    pub r: f32,
    pub h: f32,
    pub step: f32,
    pub slope_tan: f32,
}

fn ring(r: f32) -> impl Iterator<Item = (f32, f32)> {
    (0..RING_N).map(move |k| {
        let a = k as f32 * std::f32::consts::TAU / RING_N as f32;
        (a.cos() * r, a.sin() * r)
    })
}

/// True when a cylinder with feet at z would overlap a solid span (anything taller than
/// `step` above the feet and below the head), or the map edge.
pub fn blocked(w: &World, x: f32, y: f32, z: f32, b: &Body) -> bool {
    if !w.in_bounds(x, y, b.r) {
        return true;
    }
    // Every 1 m vertex within the body radius (distance-based, so moving away from an
    // obstacle never blocks).
    let r = b.r;
    let r2 = r * r;
    let i0 = ((x - r - w.min_x) / w.step).ceil().max(0.0) as usize;
    let i1 = (((x + r - w.min_x) / w.step).floor().max(0.0) as usize).min(w.nx - 1);
    let j0 = ((y - r - w.min_y) / w.step).ceil().max(0.0) as usize;
    let j1 = (((y + r - w.min_y) / w.step).floor().max(0.0) as usize).min(w.ny - 1);
    let (zlo, zhi) = (z + b.step, z + b.h - 0.05);
    for j in j0..=j1 {
        let vy = w.min_y + j as f32 * w.step - y;
        for i in i0..=i1 {
            let vx = w.min_x + i as f32 * w.step - x;
            if vx * vx + vy * vy > r2 {
                continue;
            }
            let k = j * w.nx + i;
            if (w.lo0[k] <= w.hi0[k] && w.lo0[k] < zhi && w.hi0[k] > zlo) || (w.lo1[k] <= w.hi1[k] && w.lo1[k] < zhi && w.hi1[k] > zlo) {
                return true;
            }
        }
    }
    false
}

/// Highest surface (terrain or span top) at (x, y) at or below z + up.
#[inline]
pub fn surface(w: &World, x: f32, y: f32, z: f32, up: f32) -> f32 {
    w.surface_below(x, y, z, up)
}

/// Lowest span underside above the head (z + h), over the body footprint.
pub fn ceiling(w: &World, x: f32, y: f32, z: f32, b: &Body) -> f32 {
    let mut c = f32::MAX;
    let mut chk = |sx: f32, sy: f32| {
        for (lo, hi) in w.spans_at(sx, sy) {
            if lo <= hi && lo >= z + b.h - 0.3 && lo < c {
                c = lo;
            }
        }
    };
    chk(x, y);
    for (dx, dy) in ring(b.r * 0.9) {
        chk(x + dx, y + dy);
    }
    c
}

/// Support under the body: the centre surface, plus ring samples for roof edges.
/// Returns (support height, centre surface, direction away from the edge support if hanging).
pub fn support(w: &World, x: f32, y: f32, z: f32, b: &Body) -> (f32, f32, Option<[f32; 2]>) {
    let c = surface(w, x, y, z, 0.05);
    let mut best = c;
    let mut bdir = None;
    for (dx, dy) in ring(b.r * 0.85).step_by(2) {
        let s = surface(w, x + dx, y + dy, z, 0.05);
        if s > best + 0.01 {
            best = s;
            let l = (dx * dx + dy * dy).sqrt().max(1e-6);
            bdir = Some([-dx / l, -dy / l]);
        }
    }
    if best > c + b.step {
        (best, c, bdir)
    } else {
        (c, c, None)
    }
}

/// Tries to move horizontally by (dx, dy). Returns the new feet z on success.
pub fn try_move(w: &World, p: V3, dx: f32, dy: f32, b: &Body, grounded: bool) -> Option<f32> {
    let nx = p[0] + dx;
    let ny = p[1] + dy;
    let z = p[2];
    let stuck = blocked(w, p[0], p[1], z, b);
    if !stuck && blocked(w, nx, ny, z, b) {
        return None;
    }
    if stuck && !w.in_bounds(nx, ny, b.r * 0.5) {
        return None;
    }
    // Terrain cliffs taller than a step are walls.
    if w.ground(nx, ny) > z + b.step {
        return None;
    }
    // Steep terrain (slopes over max_slope) acts as a wall while walking.
    let hd = (dx * dx + dy * dy).sqrt();
    if grounded && hd > 1e-4 {
        let g0 = w.ground(p[0], p[1]);
        let la = 1.0 / hd;
        let g1 = w.ground(p[0] + dx * la, p[1] + dy * la);
        if g1 - g0 > b.slope_tan && g1 > z + 0.2 {
            let s = surface(w, nx, ny, z, b.step);
            if (s - w.ground(nx, ny)).abs() < 0.05 {
                return None;
            }
        }
    }
    let s = surface(w, nx, ny, z, b.step);
    if s > z + 0.001 {
        // Step up: the head must fit at the new height.
        if !stuck && blocked(w, nx, ny, s, b) {
            return None;
        }
        return Some(s);
    }
    Some(z)
}

/// Integrates one tick of motion with collision. `vel` is updated (blocked axes zeroed).
pub fn integrate(w: &World, pos: &mut V3, vel: &mut V3, grounded: &mut bool, dt: f32, b: &Body, toward: [f32; 2]) {
    // Horizontal.
    let dx = vel[0] * dt;
    let dy = vel[1] * dt;
    if dx != 0.0 || dy != 0.0 {
        if let Some(z) = try_move(w, *pos, dx, dy, b, *grounded) {
            pos[0] += dx;
            pos[1] += dy;
            if z > pos[2] {
                pos[2] = z;
                if vel[2] < 0.0 {
                    vel[2] = 0.0;
                }
            }
        } else {
            // Slide: keep the part of the move that goes along the wall (approximated by
            // the smallest rotation of the move that is free, scaled by its projection).
            let mut done = false;
            'outer: for deg in [12.0f32, 25.0, 38.0, 50.0, 62.0, 74.0, 85.0] {
                let (sn, cs) = deg.to_radians().sin_cos();
                for sg in [1.0f32, -1.0] {
                    let rx = (dx * cs - dy * sn * sg) * cs;
                    let ry = (dx * sn * sg + dy * cs) * cs;
                    if let Some(z) = try_move(w, *pos, rx, ry, b, *grounded) {
                        pos[0] += rx;
                        pos[1] += ry;
                        pos[2] = pos[2].max(z);
                        let (vx, vy) = (vel[0], vel[1]);
                        vel[0] = (vx * cs - vy * sn * sg) * cs;
                        vel[1] = (vx * sn * sg + vy * cs) * cs;
                        done = true;
                        break 'outer;
                    }
                }
            }
            if !done {
                vel[0] = 0.0;
                vel[1] = 0.0;
            }
        }
    }
    // Vertical.
    if vel[2] > 0.0 {
        let c = ceiling(w, pos[0], pos[1], pos[2], b);
        if pos[2] + vel[2] * dt + b.h > c {
            pos[2] = (c - b.h).max(pos[2]);
            vel[2] = 0.0;
        }
    }
    let (sup, _centre, hang) = support(w, pos[0], pos[1], pos[2], b);
    if *grounded && vel[2] <= 0.0 {
        if pos[2] - sup <= b.step + 0.05 {
            pos[2] = sup;
            vel[2] = 0.0;
        } else {
            *grounded = false;
        }
    } else {
        let nz = pos[2] + vel[2] * dt;
        if vel[2] <= 0.0 && nz <= sup {
            pos[2] = sup;
            vel[2] = 0.0;
            *grounded = true;
        } else {
            pos[2] = nz;
            *grounded = false;
        }
    }
    // Hanging on a roof edge with the centre off it: slide off unless moving onto the roof.
    if *grounded {
        if let Some(dir) = hang {
            let onto = -(toward[0] * dir[0] + toward[1] * dir[1]);
            if onto < 0.3 {
                let s = 0.6f32;
                if try_move(w, *pos, dir[0] * s, dir[1] * s, b, false).is_some() {
                    pos[0] += dir[0] * s;
                    pos[1] += dir[1] * s;
                }
            }
        }
    }
    // Never below terrain.
    let g = w.ground(pos[0], pos[1]);
    if pos[2] < g {
        pos[2] = g;
        if vel[2] < 0.0 {
            vel[2] = 0.0;
        }
        *grounded = true;
    }
}

/// Segment p + d*t (d unit, t in [0, len]) against a vertical cylinder with feet c.
/// Returns the entry distance.
pub fn seg_cyl(p: V3, d: V3, len: f32, c: V3, r: f32, h: f32) -> Option<f32> {
    let ox = p[0] - c[0];
    let oy = p[1] - c[1];
    let a = d[0] * d[0] + d[1] * d[1];
    let cc = ox * ox + oy * oy - r * r;
    let (mut lo, mut hi);
    if a < 1e-9 {
        if cc > 0.0 {
            return None;
        }
        lo = 0.0;
        hi = len;
    } else {
        let b = 2.0 * (ox * d[0] + oy * d[1]);
        let disc = b * b - 4.0 * a * cc;
        if disc < 0.0 {
            return None;
        }
        let sq = disc.sqrt();
        lo = (-b - sq) / (2.0 * a);
        hi = (-b + sq) / (2.0 * a);
    }
    // z slab
    if d[2].abs() < 1e-9 {
        if p[2] < c[2] || p[2] > c[2] + h {
            return None;
        }
    } else {
        let t0 = (c[2] - p[2]) / d[2];
        let t1 = (c[2] + h - p[2]) / d[2];
        let (a0, a1) = if t0 < t1 { (t0, t1) } else { (t1, t0) };
        lo = lo.max(a0);
        hi = hi.min(a1);
    }
    lo = lo.max(0.0);
    hi = hi.min(len);
    if lo <= hi {
        Some(lo)
    } else {
        None
    }
}
