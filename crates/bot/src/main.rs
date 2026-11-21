//! mechfight/1 bot: stdin/stdout loop. Reads lines on a thread, always answers the newest
//! observation, never panics out of the loop.

mod aim;
mod brain;
mod proto;

use std::io::{BufRead, Write};
use std::sync::mpsc;
use std::time::Instant;

use serde::Deserialize;

#[derive(Deserialize)]
struct Head {
    #[serde(rename = "type", default)]
    kind: String,
}

fn send(line: &str) {
    let out = std::io::stdout();
    let mut o = out.lock();
    let _ = o.write_all(line.as_bytes());
    let _ = o.write_all(b"\n");
    let _ = o.flush();
}

fn main() {
    let (tx, rx) = mpsc::channel::<String>();
    std::thread::spawn(move || {
        let stdin = std::io::stdin();
        let mut lock = stdin.lock();
        let mut buf = String::new();
        loop {
            buf.clear();
            match lock.read_line(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(_) => {
                    if tx.send(buf.trim_end().to_string()).is_err() {
                        break;
                    }
                }
            }
        }
    });

    let mut bot: Option<brain::Brain> = None;
    let mut last_line_cont = String::new();
    loop {
        let first = match rx.recv() {
            Ok(l) => l,
            Err(_) => return,
        };
        let mut batch = vec![first];
        while let Ok(l) = rx.try_recv() {
            batch.push(l);
        }
        let mut latest: Option<String> = None;
        for raw in batch {
            let kind = match serde_json::from_str::<Head>(&raw) {
                Ok(h) => h.kind,
                Err(_) => continue,
            };
            match kind.as_str() {
                "hello" => {
                    let start = Instant::now();
                    let b = std::panic::catch_unwind(|| brain::Brain::new(&raw));
                    match b {
                        Ok(Ok(b)) => bot = Some(b),
                        Ok(Err(e)) => eprintln!("brain init failed: {e}"),
                        Err(_) => eprintln!("brain init panicked"),
                    }
                    eprintln!("init {:?}", start.elapsed());
                    send("{\"type\":\"ready\",\"name\":\"rustzig\"}");
                }
                "obs" => latest = Some(raw),
                "end" => {
                    if let Some(b) = &bot {
                        b.log_end(&raw);
                    }
                    return;
                }
                _ => {}
            }
        }
        let Some(raw) = latest else { continue };
        let obs: proto::Obs = match serde_json::from_str(&raw) {
            Ok(o) => o,
            Err(e) => {
                eprintln!("bad obs: {e}");
                continue;
            }
        };
        let tick = obs.tick;
        let line = match bot.as_mut() {
            Some(b) => {
                let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| b.act(&obs)));
                match r {
                    Ok(a) => {
                        last_line_cont = String::new();
                        a.to_line(tick)
                    }
                    Err(_) => {
                        eprintln!("act panicked at tick {tick}");
                        b.recover();
                        format!("{{\"tick\":{tick}{last_line_cont}}}")
                    }
                }
            }
            None => format!("{{\"tick\":{tick}}}"),
        };
        send(&line);
    }
}
