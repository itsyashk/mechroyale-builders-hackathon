//! Terrain heightfield raycast for the giddybot port (crates/giddy), exported with the C ABI.
//!
//! `giddy_ray_terrain` is a line-for-line translation of `rayTerrain` from the MIT-licensed
//! Go bot (mech/world.go): 8 m block skipping against a conservative per-block max height,
//! 0.5 m marching along the horizontal component, 20 bisection steps at the first
//! underground sample, and a bilinear-gradient normal. It must stay bit-identical to the
//! Rust twin `ray_terrain_impl` in crates/giddy/src/world.rs (checked by a unit test), so
//! it reproduces Go's math.Min/Max NaN rules, Go's Hypot algorithm and saturating
//! float->int conversion instead of using Zig builtins with different edge cases.
//!
//! No allocator and no I/O: reads caller-owned buffers only.

const std = @import("std");

pub const Terrain = extern struct {
    nx: i64,
    ny: i64,
    min_x: f64,
    min_y: f64,
    ground: [*]const f32, // ground height at vertex (i, j), row-major j * nx + i
    bnx: i64,
    bny: i64,
    block_max: [*]const f32, // max ground over each 8x8 block and its +x/+y neighbours
};

pub const THit = extern struct {
    t: f64,
    x: f64,
    y: f64,
    z: f64,
    nx: f64,
    ny: f64,
    nz: f64,
};

const inf = std.math.inf(f64);

inline fn isNan(v: f64) bool {
    return v != v;
}

/// Go math.Min.
inline fn gmin(x: f64, y: f64) f64 {
    if (x == -inf or y == -inf) return -inf;
    if (isNan(x) or isNan(y)) return std.math.nan(f64);
    if (x == 0 and x == y) {
        if (std.math.signbit(x)) return x;
        return y;
    }
    return if (x < y) x else y;
}

/// Go math.Max.
inline fn gmax(x: f64, y: f64) f64 {
    if (x == inf or y == inf) return inf;
    if (isNan(x) or isNan(y)) return std.math.nan(f64);
    if (x == 0 and x == y) {
        if (std.math.signbit(x)) return y;
        return x;
    }
    return if (x > y) x else y;
}

/// Go math.Hypot (pure-Go algorithm).
inline fn ghypot(p0: f64, q0: f64) f64 {
    var p = @abs(p0);
    var q = @abs(q0);
    if (p == inf or q == inf) return inf;
    if (isNan(p) or isNan(q)) return std.math.nan(f64);
    if (p < q) {
        const t = p;
        p = q;
        q = t;
    }
    if (p == 0) return 0;
    q = q / p;
    return p * @sqrt(1 + q * q);
}

/// Saturating truncation toward zero, NaN -> 0 (Rust `as i64`; Go on arm64).
inline fn toInt(v: f64) i64 {
    if (isNan(v)) return 0;
    if (v >= 9223372036854775807.0) return std.math.maxInt(i64);
    if (v <= -9223372036854775808.0) return std.math.minInt(i64);
    return @intFromFloat(v);
}

inline fn at(buf: [*]const f32, k: i64) f64 {
    return @floatCast(buf[@intCast(k)]);
}

/// Go World.GroundAt: bilinear height, clamped to the grid.
inline fn groundAt(t: *const Terrain, x: f64, y: f64) f64 {
    var fx = x - t.min_x;
    var fy = y - t.min_y;
    fx = gmax(0, gmin(fx, @as(f64, @floatFromInt(t.nx - 1)) - 1e-6));
    fy = gmax(0, gmin(fy, @as(f64, @floatFromInt(t.ny - 1)) - 1e-6));
    const i = toInt(fx);
    const j = toInt(fy);
    const tx = fx - @as(f64, @floatFromInt(i));
    const ty = fy - @as(f64, @floatFromInt(j));
    const nx = t.nx;
    const a = at(t.ground, j * nx + i);
    const b = at(t.ground, j * nx + i + 1);
    const c = at(t.ground, (j + 1) * nx + i);
    const d = at(t.ground, (j + 1) * nx + i + 1);
    return (a * (1 - tx) + b * tx) * (1 - ty) + (c * (1 - tx) + d * tx) * ty;
}

inline fn above(t: *const Terrain, x: f64, y: f64, z: f64, dx: f64, dy: f64, dz: f64, s: f64) f64 {
    return z + dz * s - groundAt(t, x + dx * s, y + dy * s);
}

/// Go World.rayTerrain(x, y, z, x1, y1, z1, tmax). Returns true and fills `out` on a hit.
export fn giddy_ray_terrain(tr: *const Terrain, x: f64, y: f64, z: f64, x1: f64, y1: f64, z1: f64, tmax: f64, out: *THit) bool {
    const dx = x1 - x;
    const dy = y1 - y;
    const dz = z1 - z;
    const hl = ghypot(dx, dy);
    const blk: f64 = 8.0;
    if (above(tr, x, y, z, dx, dy, dz, 0) < 0) {
        out.* = .{ .t = 0, .x = x, .y = y, .z = z, .nx = 0, .ny = 0, .nz = 1 };
        return true;
    }
    var step: f64 = 1.0;
    if (hl > 1e-9) step = 0.5 / hl;
    var t: f64 = 0.0;
    var it: i64 = 0;
    while (t < tmax) {
        if (it > 100000) return false;
        it += 1;
        const px = x + dx * t;
        const py = y + dy * t;
        const bi = toInt(@floor((px - tr.min_x) / blk));
        const bj = toInt(@floor((py - tr.min_y) / blk));
        const inb = bi >= 0 and bj >= 0 and bi < tr.bnx and bj < tr.bny;
        var tb = tmax;
        if (hl > 1e-9) {
            const ex = tr.min_x + @as(f64, @floatFromInt(bi)) * blk;
            const ey = tr.min_y + @as(f64, @floatFromInt(bj)) * blk;
            var nx = inf;
            var ny = inf;
            if (dx > 1e-9) {
                nx = (ex + blk - px) / dx;
            } else if (dx < -1e-9) {
                nx = (ex - px) / dx;
            }
            if (dy > 1e-9) {
                ny = (ey + blk - py) / dy;
            } else if (dy < -1e-9) {
                ny = (ey - py) / dy;
            }
            tb = gmin(t + gmax(0, gmin(nx, ny)) + 1e-6, tmax);
        }
        if (inb and gmin(z + dz * t, z + dz * tb) > at(tr.block_max, bj * tr.bnx + bi)) {
            t = tb;
            continue;
        }
        const s = gmin(t + step, tmax);
        if (above(tr, x, y, z, dx, dy, dz, s) < 0) {
            var lo = t;
            var hi = s;
            var k: u32 = 0;
            while (k < 20) : (k += 1) {
                const m = (lo + hi) / 2;
                if (above(tr, x, y, z, dx, dy, dz, m) < 0) {
                    hi = m;
                } else {
                    lo = m;
                }
            }
            const hx = x + dx * hi;
            const hy = y + dy * hi;
            const gx = (groundAt(tr, hx + 1, hy) - groundAt(tr, hx - 1, hy)) / 2;
            const gy = (groundAt(tr, hx, hy + 1) - groundAt(tr, hx, hy - 1)) / 2;
            const l = @sqrt(gx * gx + gy * gy + 1);
            out.* = .{ .t = hi, .x = hx, .y = hy, .z = groundAt(tr, hx, hy), .nx = -gx / l, .ny = -gy / l, .nz = 1 / l };
            return true;
        }
        if (s >= tmax) break;
        t = s;
    }
    return false;
}
