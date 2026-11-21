use arena::World;
fn main() {
    let w = World::load("kit/arena").unwrap();
    let txt = std::fs::read_to_string("target/vis.txt").unwrap();
    let (mut n, mut ok, mut far) = (0, 0, 0);
    for l in txt.lines() {
        let v: Vec<&str> = l.split_whitespace().collect();
        let f: Vec<f32> = v[..7].iter().map(|x| x.parse().unwrap()).collect();
        let eye = [f[0], f[1], f[2]];
        let d = ((f[3]-f[0]).powi(2)+(f[4]-f[1]).powi(2)).sqrt();
        let c = w.los(eye, [f[3], f[4], f[5] + f[6] * 0.5]) || w.los(eye, [f[3], f[4], f[5] + f[6]]);
        n += 1; if c { ok += 1 } else { if d > 45.0 { far += 1; } println!("blocked {} d={d:.0} {:?}", v[7], &f[..6]); }
    }
    println!("{ok}/{n} visible-by-server also clear in our LOS ({far} blocked beyond 45m)");
}
