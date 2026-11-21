use arena::{Nav, World};
use std::time::Instant;
fn main() {
    let dir = std::env::args().nth(1).unwrap_or("kit/arena".into());
    let t = Instant::now();
    let w = World::load(&dir).unwrap();
    println!("load {:?} prisms {} trees {}", t.elapsed(), w.prisms.len(), w.trees.len());
    let (mn, mx) = w.terrain.iter().fold((f32::MAX, f32::MIN), |a, &h| (a.0.min(h), a.1.max(h)));
    println!("terrain {mn}..{mx}");
    let t = Instant::now();
    let mut nav = Nav::build(&w);
    let walk = nav.base_cost.iter().filter(|&&c| c >= 0.0).count();
    println!("nav {:?} cells {} walkable {:.1}%", t.elapsed(), nav.base_cost.len(), 100.0 * walk as f32 / nav.base_cost.len() as f32);
    for (a, b) in [([-217.98f32, -22.04], [41.3f32, -72.8]), ([-400.0, 200.0], [400.0, -200.0]), ([0.0, 0.0], [300.0, 300.0])] {
        let t = Instant::now();
        let p = nav.path(a, b, 200_000);
        println!("path {:?}->{:?}: {} pts in {:?}, end {:?}", a, b, p.len(), t.elapsed(), p.last());
    }
    let t = Instant::now();
    let mut c = 0;
    for k in 0..10000 {
        let f = k as f32;
        if w.los([-200.0 + f % 400.0, -100.0, 8.0], [100.0, 50.0 + f % 100.0, 5.0]) { c += 1; }
    }
    println!("10k los {:?} clear {c}", t.elapsed());
    // ascii map of nav at 8m
    let mut s = String::new();
    for cj in (0..nav.cny).rev().step_by(6) {
        for ci in (0..nav.cnx).step_by(3) {
            s.push(if nav.base_cost[cj * nav.cnx + ci] < 0.0 { '#' } else { '.' });
        }
        s.push('\n');
    }
    std::fs::write("target/navmap.txt", s).unwrap();
}
