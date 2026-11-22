//! Go `math` semantics the port relies on (NaN propagation of math.Min/Max, Go's Hypot
//! algorithm) and a deterministic stand-in for `math/rand`.

/// Go's math.Min: -Inf wins, then NaN propagates, then -0 < +0.
#[inline]
pub fn fmin(x: f64, y: f64) -> f64 {
    if x == f64::NEG_INFINITY || y == f64::NEG_INFINITY {
        return f64::NEG_INFINITY;
    }
    if x.is_nan() || y.is_nan() {
        return f64::NAN;
    }
    if x == 0.0 && x == y {
        return if x.is_sign_negative() { x } else { y };
    }
    if x < y {
        x
    } else {
        y
    }
}

/// Go's math.Max: +Inf wins, then NaN propagates, then +0 > -0.
#[inline]
pub fn fmax(x: f64, y: f64) -> f64 {
    if x == f64::INFINITY || y == f64::INFINITY {
        return f64::INFINITY;
    }
    if x.is_nan() || y.is_nan() {
        return f64::NAN;
    }
    if x == 0.0 && x == y {
        return if x.is_sign_negative() { y } else { x };
    }
    if x > y {
        x
    } else {
        y
    }
}

/// Go's builtin max for float32 (NaN propagates).
#[inline]
pub fn fmax32(x: f32, y: f32) -> f32 {
    if x.is_nan() || y.is_nan() {
        return f32::NAN;
    }
    if y > x {
        y
    } else {
        x
    }
}

/// Go's math.Hypot (pure-Go algorithm).
#[inline]
pub fn hypot(p: f64, q: f64) -> f64 {
    let (mut p, mut q) = (p.abs(), q.abs());
    if p == f64::INFINITY || q == f64::INFINITY {
        return f64::INFINITY;
    }
    if p.is_nan() || q.is_nan() {
        return f64::NAN;
    }
    if p < q {
        std::mem::swap(&mut p, &mut q);
    }
    if p == 0.0 {
        return 0.0;
    }
    let q = q / p;
    p * (1.0 + q * q).sqrt()
}

/// Go's math.Mod for finite inputs (fmod); Rust's `%` on f64 is exactly fmod.
#[inline]
pub fn fmod(a: f64, b: f64) -> f64 {
    a % b
}

/// Stand-in for `*rand.Rand` from `rand.New(rand.NewSource(seed))`. Bit-equality with Go's
/// additive lagged Fibonacci source is not attempted; only `Float64` is used by the brain.
pub struct Rand {
    s: u64,
}

impl Rand {
    pub fn new(seed: i64) -> Rand {
        Rand { s: (seed as u64) ^ 0x5DEE_CE66_D1CE_4E5B }
    }

    #[inline]
    fn next_u64(&mut self) -> u64 {
        // splitmix64
        self.s = self.s.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.s;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Uniform in [0, 1), like rand.Float64.
    pub fn float64(&mut self) -> f64 {
        loop {
            let f = (self.next_u64() >> 11) as f64 * (1.0 / (1u64 << 53) as f64);
            if f < 1.0 {
                return f;
            }
        }
    }
}
