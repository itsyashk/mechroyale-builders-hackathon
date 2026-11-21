use arena::{Nav, World};
fn main() {
    let w = World::load("kit/arena").unwrap();
    let nav = Nav::build(&w);
    let txt = std::fs::read_to_string("target/pos.txt").unwrap();
    let (mut n, mut ok, mut gz) = (0, 0, 0.0f32);
    for l in txt.lines() {
        let f: Vec<f32> = l.split_whitespace().map(|x| x.parse().unwrap()).collect();
        if f[3] < 0.5 { continue; }
        n += 1;
        if nav.walkable_at(f[0], f[1]) { ok += 1 } else { println!("blocked at {:?} clear={:.1}", &f[..3], nav.clearance_at(f[0], f[1])); }
        gz += (w.surface_below(f[0], f[1], f[2], 1.3) - f[2]).abs();
    }
    println!("{ok}/{n} grounded positions walkable; mean |surface-z| = {:.3}", gz / n as f32);
}
