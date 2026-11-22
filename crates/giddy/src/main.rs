//! giddybot: Rust port of the MIT-licensed Go mechfight/1 bot "giddygiddyboy"
//! (cmd/bot/main.go + mech/*.go; license in LICENSE-MIT-giddy).
//!
//! Port of cmd/bot/main.go: a reader thread feeds stdin lines into a channel; the main loop
//! takes everything queued, handles hello/end, answers only the newest observation with one
//! JSON line (echoing its tick). Logs go to stderr only.

mod brain;
mod gomath;
mod types;
mod world;

use std::io::{BufRead, Write};
use std::sync::mpsc;

use serde::Deserialize;

#[derive(Deserialize, Default)]
struct Probe {
    #[serde(rename = "type", default, deserialize_with = "types::de_str")]
    kind: String,
}

#[derive(Deserialize, Default)]
struct TickOnly {
    #[serde(default, deserialize_with = "types::de_int")]
    tick: i64,
}

fn send<T: serde::Serialize>(out: &mut impl Write, v: &T) {
    let Ok(mut b) = serde_json::to_vec(v) else { return };
    b.push(b'\n');
    let _ = out.write_all(&b);
    let _ = out.flush();
}

fn main() {
    let (tx, rx) = mpsc::sync_channel::<String>(256);
    std::thread::spawn(move || {
        let stdin = std::io::stdin();
        let mut r = std::io::BufReader::with_capacity(1 << 20, stdin.lock());
        loop {
            let mut line = String::new();
            match r.read_line(&mut line) {
                Ok(0) | Err(_) => return,
                Ok(_) => {
                    if tx.send(line).is_err() {
                        return;
                    }
                }
            }
        }
    });
    let stdout = std::io::stdout();
    let mut out = std::io::BufWriter::with_capacity(1 << 16, stdout.lock());
    let mut brain: Option<brain::Brain> = None;
    loop {
        let Ok(first) = rx.recv() else { return };
        let mut batch = vec![first];
        while let Ok(l) = rx.try_recv() {
            batch.push(l);
        }
        let mut latest: Option<String> = None;
        for raw in batch {
            let Ok(probe) = serde_json::from_str::<Probe>(&raw) else { continue };
            match probe.kind.as_str() {
                "hello" => {
                    let h: types::Hello = match serde_json::from_str(&raw) {
                        Ok(h) => h,
                        Err(e) => {
                            eprintln!("hello: {e}");
                            // Go keeps whatever decoded; at least recover the map dir.
                            let v: serde_json::Value = serde_json::from_str(&raw).unwrap_or_default();
                            types::Hello {
                                map: types::MapRef { dir: v["map"]["dir"].as_str().unwrap_or("").to_string() },
                                ..Default::default()
                            }
                        }
                    };
                    let mut dir = h.map.dir.clone();
                    if dir.is_empty() {
                        dir = "/arena".into();
                    }
                    let w = match world::World::load_world(&dir) {
                        Ok(w) => w,
                        Err(e) => {
                            eprintln!("map: {e} - playing on a flat map");
                            world::World::flat_world()
                        }
                    };
                    brain = Some(brain::Brain::new(&h, w));
                    let _ = out.write_all(b"{\"name\":\"giddygiddyboy\",\"type\":\"ready\"}\n");
                    let _ = out.flush();
                }
                "obs" => latest = Some(raw),
                "end" => return,
                _ => {}
            }
        }
        let Some(latest) = latest else { continue };
        let parsed = serde_json::from_str::<types::Obs>(&latest);
        match (parsed, brain.as_mut()) {
            (Ok(mut o), Some(b)) => {
                let a = b.act(&mut o);
                send(&mut out, &a);
            }
            (p, _) => {
                if let Err(e) = p {
                    eprintln!("obs: {e}");
                }
                let t = serde_json::from_str::<TickOnly>(&latest).unwrap_or_default();
                send(&mut out, &types::Action { tick: t.tick, ..Default::default() });
            }
        }
    }
}
