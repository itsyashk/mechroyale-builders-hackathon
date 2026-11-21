//! Vector helpers, projectile intercept and ballistic (mortar) solutions.
use crate::proto::V3;
use std::f32::consts::PI;

#[inline]
pub fn sub(a: V3, b: V3) -> V3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
#[inline]
pub fn add(a: V3, b: V3) -> V3 {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}
#[inline]
pub fn scale(a: V3, s: f32) -> V3 {
    [a[0] * s, a[1] * s, a[2] * s]
}
#[inline]
pub fn len3(a: V3) -> f32 {
    (a[0] * a[0] + a[1] * a[1] + a[2] * a[2]).sqrt()
}
#[inline]
pub fn dist2(a: [f32; 2], b: [f32; 2]) -> f32 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2)).sqrt()
}
#[inline]
pub fn xy(a: V3) -> [f32; 2] {
    [a[0], a[1]]
}
#[inline]
pub fn hdist(a: V3, b: V3) -> f32 {
    dist2(xy(a), xy(b))
}
#[inline]
pub fn wrap(a: f32) -> f32 {
    let mut x = (a + PI) % (2.0 * PI);
    if x < 0.0 {
        x += 2.0 * PI;
    }
    x - PI
}
#[inline]
pub fn yaw_to(from: [f32; 2], to: [f32; 2]) -> f32 {
    (to[1] - from[1]).atan2(to[0] - from[0])
}
#[inline]
pub fn dir(yaw: f32, pitch: f32) -> V3 {
    [pitch.cos() * yaw.cos(), pitch.cos() * yaw.sin(), pitch.sin()]
}
#[inline]
pub fn unit2(v: [f32; 2]) -> [f32; 2] {
    let l = (v[0] * v[0] + v[1] * v[1]).sqrt();
    if l < 1e-6 {
        [0.0, 0.0]
    } else {
        [v[0] / l, v[1] / l]
    }
}

/// Yaw/pitch from `from` to `to`.
pub fn aim_at(from: V3, to: V3) -> [f32; 2] {
    let d = sub(to, from);
    let h = (d[0] * d[0] + d[1] * d[1]).sqrt();
    [d[1].atan2(d[0]), d[2].atan2(h)]
}

/// Point to aim a straight projectile (speed `v`, leaves `lead_out` m out of `from`) at,
/// to hit a target at `p` moving at `vel`. `extra_t` adds latency (decision delay).
/// Returns (aim point, flight time).
pub fn intercept(from: V3, p: V3, vel: V3, v: f32, lead_out: f32, extra_t: f32, grounded: bool) -> (V3, f32) {
    let mut t = (len3(sub(p, from)) - lead_out).max(0.0) / v;
    let mut q = p;
    for _ in 0..4 {
        let tt = t + extra_t;
        q = add(p, scale(vel, tt));
        if !grounded {
            // falling targets: crude gravity on z (jets may counter it)
            q[2] -= 0.5 * 11.0 * tt * tt * 0.5;
        }
        t = (len3(sub(q, from)) - lead_out).max(0.0) / v;
    }
    (q, t)
}

/// Mortar pitch to land at horizontal distance `d` and height difference `h` with speed `v`
/// and gravity `g`. `high` picks the lob. Returns (pitch, flight time).
pub fn ballistic(d: f32, h: f32, v: f32, g: f32, high: bool) -> Option<(f32, f32)> {
    let v2 = v * v;
    let disc = v2 * v2 - g * (g * d * d + 2.0 * h * v2);
    if disc < 0.0 || d < 0.5 {
        return None;
    }
    let s = disc.sqrt();
    let tan = if high { (v2 + s) / (g * d) } else { (v2 - s) / (g * d) };
    let pitch = tan.atan();
    let t = d / (v * pitch.cos()).max(1e-3);
    Some((pitch, t))
}
