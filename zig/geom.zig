//! Geometry kernels for the mech bot, exported with the C ABI and linked into Rust.
//!
//! * `geom_los` / `geom_raycast`: ray marching against the terrain height field and the
//!   per-cell solid spans rasterized from the map prisms.
//! * `geom_astar`: 8-connected A* over a float cost grid (cost < 0 means blocked), using
//!   caller-owned scratch buffers and generation stamps so nothing is allocated or cleared
//!   per query.
//!
//! No allocator and no std I/O: everything works on caller-provided memory.

pub const Grid = extern struct {
    nx: u32,
    ny: u32,
    min_x: f32,
    min_y: f32,
    step: f32,
    terrain: [*]const f32, // ground height at vertex (i, j), row-major j * nx + i
    lo0: [*]const f32, // first solid span [lo0, hi0] (lo > hi when empty)
    hi0: [*]const f32,
    lo1: [*]const f32, // second solid span
    hi1: [*]const f32,
};

inline fn clampf(v: f32, lo: f32, hi: f32) f32 {
    return @max(lo, @min(hi, v));
}

export fn geom_terrain(g: *const Grid, x: f32, y: f32) f32 {
    const fx = clampf((x - g.min_x) / g.step, 0.0, @as(f32, @floatFromInt(g.nx - 1)) - 0.001);
    const fy = clampf((y - g.min_y) / g.step, 0.0, @as(f32, @floatFromInt(g.ny - 1)) - 0.001);
    const i: u32 = @intFromFloat(@floor(fx));
    const j: u32 = @intFromFloat(@floor(fy));
    const tx = fx - @as(f32, @floatFromInt(i));
    const ty = fy - @as(f32, @floatFromInt(j));
    const k = j * g.nx + i;
    const h00 = g.terrain[k];
    const h10 = g.terrain[k + 1];
    const h01 = g.terrain[k + g.nx];
    const h11 = g.terrain[k + g.nx + 1];
    const a = h00 + (h10 - h00) * tx;
    const b = h01 + (h11 - h01) * tx;
    return a + (b - a) * ty;
}

/// True when the point is inside terrain or a solid span. Off-map counts as solid.
export fn geom_solid(g: *const Grid, x: f32, y: f32, z: f32) bool {
    const fx = (x - g.min_x) / g.step + 0.5;
    const fy = (y - g.min_y) / g.step + 0.5;
    if (fx < 0.0 or fy < 0.0) return true;
    const i: u32 = @intFromFloat(fx);
    const j: u32 = @intFromFloat(fy);
    if (i >= g.nx or j >= g.ny) return true;
    if (z < geom_terrain(g, x, y) - 0.05) return true;
    const k = j * g.nx + i;
    if (g.lo0[k] <= z and z <= g.hi0[k]) return true;
    if (g.lo1[k] <= z and z <= g.hi1[k]) return true;
    return false;
}

/// Distance along the (normalised internally) direction to the first solid sample,
/// or `max_d` if the ray is clear. Samples every 0.4 m.
export fn geom_raycast(g: *const Grid, ax: f32, ay: f32, az: f32, dx: f32, dy: f32, dz: f32, max_d: f32) f32 {
    const len = @sqrt(dx * dx + dy * dy + dz * dz);
    if (len < 1e-6) return 0.0;
    const ux = dx / len;
    const uy = dy / len;
    const uz = dz / len;
    const stride: f32 = 0.4;
    var d: f32 = stride;
    while (d < max_d) : (d += stride) {
        if (geom_solid(g, ax + ux * d, ay + uy * d, az + uz * d)) return d;
    }
    return max_d;
}

/// Line of sight between two points (endpoints themselves are not tested).
export fn geom_los(g: *const Grid, ax: f32, ay: f32, az: f32, bx: f32, by: f32, bz: f32) bool {
    const dx = bx - ax;
    const dy = by - ay;
    const dz = bz - az;
    const len = @sqrt(dx * dx + dy * dy + dz * dz);
    if (len < 0.5) return true;
    return geom_raycast(g, ax, ay, az, dx, dy, dz, len - 0.3) >= len - 0.3;
}

// ---------------------------------------------------------------- A* ------------------

pub const AStar = extern struct {
    nx: u32,
    ny: u32,
    cost: [*]const f32, // per-cell traversal multiplier (>= 1), < 0 blocked
    g: [*]f32,
    f: [*]f32,
    parent: [*]i32,
    stamp: [*]u32, // stamp[i] == cur: g/f/parent valid
    closed: [*]u32, // closed[i] == cur: expanded
    heap: [*]u32,
    heap_pos: [*]i32,
    cur: u32,
};

inline fn heur(nx: u32, a: u32, b: u32) f32 {
    const ax: i32 = @intCast(a % nx);
    const ay: i32 = @intCast(a / nx);
    const bx: i32 = @intCast(b % nx);
    const by: i32 = @intCast(b / nx);
    const dx: f32 = @floatFromInt(@abs(ax - bx));
    const dy: f32 = @floatFromInt(@abs(ay - by));
    const mn = @min(dx, dy);
    const mx = @max(dx, dy);
    return (mx - mn) + 1.41421356 * mn;
}

fn heapUp(s: *AStar, start: u32) void {
    var i = start;
    const node = s.heap[i];
    const key = s.f[node];
    while (i > 0) {
        const p = (i - 1) / 2;
        const pn = s.heap[p];
        if (s.f[pn] <= key) break;
        s.heap[i] = pn;
        s.heap_pos[pn] = @intCast(i);
        i = p;
    }
    s.heap[i] = node;
    s.heap_pos[node] = @intCast(i);
}

fn heapDown(s: *AStar, len: u32) void {
    var i: u32 = 0;
    const node = s.heap[0];
    const key = s.f[node];
    while (true) {
        const l = 2 * i + 1;
        if (l >= len) break;
        var c = l;
        if (l + 1 < len and s.f[s.heap[l + 1]] < s.f[s.heap[l]]) c = l + 1;
        const cn = s.heap[c];
        if (s.f[cn] >= key) break;
        s.heap[i] = cn;
        s.heap_pos[cn] = @intCast(i);
        i = c;
    }
    s.heap[i] = node;
    s.heap_pos[node] = @intCast(i);
}

/// Runs A* from `start` to `goal` (cell indices). Writes the path (start..goal, inclusive)
/// into `out` and returns its length. If the goal is unreachable or `max_expand` nodes are
/// expanded first, returns the path to the expanded node closest to the goal (by heuristic).
/// Returns 0 when start is invalid. Caller must bump `s.cur` before each call.
export fn geom_astar(s: *AStar, start: u32, goal: u32, max_expand: u32, out: [*]u32, out_cap: u32) u32 {
    const n = s.nx * s.ny;
    if (start >= n or goal >= n) return 0;
    const cur = s.cur;
    var len: u32 = 0;

    s.stamp[start] = cur;
    s.g[start] = 0.0;
    s.f[start] = heur(s.nx, start, goal);
    s.parent[start] = -1;
    s.heap[0] = start;
    s.heap_pos[start] = 0;
    len = 1;

    var best = start;
    var best_h = s.f[start];
    var expanded: u32 = 0;
    const dxs = [8]i32{ 1, -1, 0, 0, 1, 1, -1, -1 };
    const dys = [8]i32{ 0, 0, 1, -1, 1, -1, 1, -1 };

    while (len > 0) {
        const node = s.heap[0];
        len -= 1;
        if (len > 0) {
            s.heap[0] = s.heap[len];
            heapDown(s, len);
        }
        s.closed[node] = cur;
        s.heap_pos[node] = -1;
        if (node == goal) {
            best = goal;
            break;
        }
        const h = heur(s.nx, node, goal);
        if (h < best_h) {
            best_h = h;
            best = node;
        }
        expanded += 1;
        if (expanded >= max_expand) break;

        const x: i32 = @intCast(node % s.nx);
        const y: i32 = @intCast(node / s.nx);
        const gn = s.g[node];
        var k: usize = 0;
        while (k < 8) : (k += 1) {
            const nxp = x + dxs[k];
            const nyp = y + dys[k];
            if (nxp < 0 or nyp < 0 or nxp >= @as(i32, @intCast(s.nx)) or nyp >= @as(i32, @intCast(s.ny))) continue;
            const m: u32 = @intCast(nyp * @as(i32, @intCast(s.nx)) + nxp);
            const cm = s.cost[m];
            if (cm < 0.0) continue;
            var step: f32 = 1.0;
            if (k >= 4) {
                // No corner cutting: both orthogonal neighbours must be open.
                const o1: u32 = @intCast(y * @as(i32, @intCast(s.nx)) + nxp);
                const o2: u32 = @intCast(nyp * @as(i32, @intCast(s.nx)) + x);
                if (s.cost[o1] < 0.0 or s.cost[o2] < 0.0) continue;
                step = 1.41421356;
            }
            if (s.closed[m] == cur) continue;
            const ng = gn + step * 0.5 * (cm + s.cost[node]);
            if (s.stamp[m] == cur and ng >= s.g[m]) continue;
            const fresh = s.stamp[m] != cur or s.heap_pos[m] < 0;
            s.stamp[m] = cur;
            s.g[m] = ng;
            s.f[m] = ng + heur(s.nx, m, goal);
            s.parent[m] = @intCast(node);
            if (fresh) {
                s.heap[len] = m;
                len += 1;
                heapUp(s, len - 1);
            } else {
                heapUp(s, @intCast(s.heap_pos[m]));
            }
        }
    }

    // Reconstruct best..start, then reverse into out.
    var count: u32 = 0;
    var c: i32 = @intCast(best);
    while (c >= 0 and count < out_cap) {
        out[count] = @intCast(c);
        count += 1;
        c = s.parent[@intCast(c)];
    }
    var a: u32 = 0;
    var b: u32 = if (count > 0) count - 1 else 0;
    while (a < b) {
        const t = out[a];
        out[a] = out[b];
        out[b] = t;
        a += 1;
        b -= 1;
    }
    return count;
}
