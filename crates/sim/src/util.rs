//! Small shared helpers: RNG, vector math, JSON number formatting.

use std::f32::consts::PI;
use std::fmt::Write;

pub type V3 = [f32; 3];

/// SplitMix64: tiny, fast, good enough for a game sim, fully reproducible.
#[derive(Clone)]
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Rng {
        let mut r = Rng(seed ^ 0x5DEE_CE66_D1CE_4E5B);
        r.next_u64();
        r
    }
    #[inline]
    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
    /// Uniform in [0, 1).
    #[inline]
    pub fn f32(&mut self) -> f32 {
        (self.next_u64() >> 40) as f32 / (1u64 << 24) as f32
    }
    #[inline]
    pub fn range(&mut self, a: f32, b: f32) -> f32 {
        a + (b - a) * self.f32()
    }
    #[inline]
    pub fn below(&mut self, n: usize) -> usize {
        if n == 0 {
            0
        } else {
            (self.next_u64() % n as u64) as usize
        }
    }
    #[inline]
    pub fn chance(&mut self, p: f32) -> bool {
        self.f32() < p
    }
    /// Standard normal (Box-Muller).
    pub fn normal(&mut self) -> f32 {
        let u1 = self.f32().max(1e-7);
        let u2 = self.f32();
        (-2.0 * u1.ln()).sqrt() * (2.0 * PI * u2).cos()
    }
    /// Uniform point in a disc of radius r.
    pub fn in_disc(&mut self, r: f32) -> [f32; 2] {
        let a = self.range(-PI, PI);
        let d = r * self.f32().sqrt();
        [d * a.cos(), d * a.sin()]
    }
}

pub fn mix_seed(a: u64, b: u64) -> u64 {
    let mut r = Rng::new(a.wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ b.rotate_left(17));
    r.next_u64()
}

#[inline]
pub fn add(a: V3, b: V3) -> V3 {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}
#[inline]
pub fn sub(a: V3, b: V3) -> V3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
#[inline]
pub fn scale(a: V3, s: f32) -> V3 {
    [a[0] * s, a[1] * s, a[2] * s]
}
#[inline]
pub fn dot(a: V3, b: V3) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
#[inline]
pub fn len(a: V3) -> f32 {
    dot(a, a).sqrt()
}
#[inline]
pub fn dist(a: V3, b: V3) -> f32 {
    len(sub(a, b))
}
#[inline]
pub fn dist2d(a: &[f32], b: &[f32]) -> f32 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2)).sqrt()
}
#[inline]
pub fn norm(a: V3) -> V3 {
    let l = len(a);
    if l < 1e-9 {
        [0.0, 0.0, 0.0]
    } else {
        scale(a, 1.0 / l)
    }
}
#[inline]
pub fn wrap(a: f32) -> f32 {
    let mut x = a % (2.0 * PI);
    if x > PI {
        x -= 2.0 * PI;
    } else if x < -PI {
        x += 2.0 * PI;
    }
    x
}
/// Signed smallest difference a - b, in [-pi, pi].
#[inline]
pub fn angdiff(a: f32, b: f32) -> f32 {
    wrap(a - b)
}
#[inline]
pub fn dir_of(yaw: f32, pitch: f32) -> V3 {
    let c = pitch.cos();
    [c * yaw.cos(), c * yaw.sin(), pitch.sin()]
}
#[inline]
pub fn yaw_to(a: &[f32], b: &[f32]) -> f32 {
    (b[1] - a[1]).atan2(b[0] - a[0])
}
/// Angle between two directions given as (yaw, pitch).
pub fn ang_between(y1: f32, p1: f32, y2: f32, p2: f32) -> f32 {
    let a = dir_of(y1, p1);
    let b = dir_of(y2, p2);
    dot(a, b).clamp(-1.0, 1.0).acos()
}

/// Appends a number rounded to `dec` decimals in compact JSON form.
pub fn jnum(s: &mut String, v: f32, dec: i32) {
    let m = 10f64.powi(dec);
    let x = ((v as f64) * m).round() / m;
    if !x.is_finite() {
        s.push('0');
    } else if x == x.trunc() && x.abs() < 1e15 {
        let _ = write!(s, "{}", x as i64);
    } else {
        let _ = write!(s, "{}", x);
    }
}
pub fn jv3(s: &mut String, v: V3, dec: i32) {
    s.push('[');
    jnum(s, v[0], dec);
    s.push(',');
    jnum(s, v[1], dec);
    s.push(',');
    jnum(s, v[2], dec);
    s.push(']');
}
pub fn jv2(s: &mut String, a: f32, b: f32, dec: i32) {
    s.push('[');
    jnum(s, a, dec);
    s.push(',');
    jnum(s, b, dec);
    s.push(']');
}
pub fn jstr(s: &mut String, t: &str) {
    s.push('"');
    for c in t.chars() {
        match c {
            '"' => s.push_str("\\\""),
            '\\' => s.push_str("\\\\"),
            '\n' => s.push_str("\\n"),
            c if (c as u32) < 0x20 => {
                let _ = write!(s, "\\u{:04x}", c as u32);
            }
            c => s.push(c),
        }
    }
    s.push('"');
}
