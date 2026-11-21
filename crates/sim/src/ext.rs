//! External bots: one subprocess per slot speaking mechfight/1 over stdin/stdout.

use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{channel, Receiver, RecvTimeoutError};
use std::time::{Duration, Instant};

pub struct ExtBot {
    child: Child,
    stdin: Option<ChildStdin>,
    rx: Receiver<(Instant, String)>,
    pub name: String,
    pub times_ms: Vec<f32>,
    pub late: u32,
    pub missing: u32,
    pub bad: u32,
    pub dead: bool,
}

/// Splits a command line, honouring single and double quotes.
pub fn split_cmd(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut q: Option<char> = None;
    let mut any = false;
    for c in s.chars() {
        match q {
            Some(qc) if c == qc => q = None,
            Some(_) => cur.push(c),
            None if c == '"' || c == '\'' => {
                q = Some(c);
                any = true;
            }
            None if c.is_whitespace() => {
                if !cur.is_empty() || any {
                    out.push(std::mem::take(&mut cur));
                    any = false;
                }
            }
            None => cur.push(c),
        }
    }
    if !cur.is_empty() || any {
        out.push(cur);
    }
    out
}

impl ExtBot {
    pub fn spawn(cmdline: &str, stderr_path: Option<PathBuf>) -> Result<ExtBot, String> {
        let parts = split_cmd(cmdline);
        if parts.is_empty() {
            return Err("empty bot command".into());
        }
        let mut cmd = Command::new(&parts[0]);
        cmd.args(&parts[1..]).stdin(Stdio::piped()).stdout(Stdio::piped());
        match stderr_path {
            Some(p) => {
                let f = std::fs::File::create(&p).map_err(|e| format!("{}: {e}", p.display()))?;
                cmd.stderr(Stdio::from(f));
            }
            None => {
                cmd.stderr(Stdio::null());
            }
        }
        let mut child = cmd.spawn().map_err(|e| format!("spawn `{cmdline}`: {e}"))?;
        let stdout = child.stdout.take().unwrap();
        let stdin = child.stdin.take();
        let (tx, rx) = channel();
        std::thread::spawn(move || {
            let mut r = BufReader::new(stdout);
            let mut line = String::new();
            loop {
                line.clear();
                match r.read_line(&mut line) {
                    Ok(0) | Err(_) => break,
                    Ok(_) => {
                        let s = line.trim().to_string();
                        if !s.is_empty() && tx.send((Instant::now(), s)).is_err() {
                            break;
                        }
                    }
                }
            }
        });
        Ok(ExtBot { child, stdin, rx, name: String::new(), times_ms: Vec::new(), late: 0, missing: 0, bad: 0, dead: false })
    }

    pub fn send(&mut self, line: &str) -> bool {
        if self.dead {
            return false;
        }
        let ok = match self.stdin.as_mut() {
            Some(s) => s.write_all(line.as_bytes()).and_then(|_| s.write_all(b"\n")).and_then(|_| s.flush()).is_ok(),
            None => false,
        };
        if !ok {
            self.dead = true;
        }
        ok
    }

    /// Next line before `deadline`, or None.
    pub fn recv_until(&mut self, deadline: Instant) -> Option<(Instant, String)> {
        let now = Instant::now();
        let wait = deadline.saturating_duration_since(now);
        match self.rx.recv_timeout(wait) {
            Ok(x) => Some(x),
            Err(RecvTimeoutError::Timeout) => None,
            Err(RecvTimeoutError::Disconnected) => {
                self.dead = true;
                None
            }
        }
    }

    /// Drains lines already queued (stale answers).
    pub fn drain(&mut self) -> Vec<(Instant, String)> {
        let mut v = vec![];
        while let Ok(x) = self.rx.try_recv() {
            v.push(x);
        }
        v
    }

    pub fn finish(&mut self) {
        self.stdin = None; // closes the pipe
        let t0 = Instant::now();
        while t0.elapsed() < Duration::from_millis(1500) {
            if let Ok(Some(_)) = self.child.try_wait() {
                return;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Drop for ExtBot {
    fn drop(&mut self) {
        if let Ok(None) = self.child.try_wait() {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}
