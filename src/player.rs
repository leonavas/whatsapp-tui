//! Voice notes and audio files, played by mpv with no window and steered
//! over its JSON IPC socket. One plays at a time; at the end it's back to
//! the start, as on WhatsApp.

use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::Sender;
use std::time::{Duration, Instant};

/// From the thread reading mpv; `gen` tells an mpv since replaced.
pub enum Ev {
    Pos { gen: u64, pos: f64 },
    Duration { gen: u64, dur: f64 },
    Paused { gen: u64, paused: bool },
    Ended { gen: u64 },
}

#[derive(Clone, Copy, PartialEq)]
pub enum State {
    /// Downloading, or mpv starting.
    Loading,
    Playing,
    Paused,
}

/// The audio playing (or paused, or on its way).
pub struct Now {
    pub chat: String,
    pub id: String,
    pub state: State,
    pub pos: f64,
    pub dur: f64,
    /// Where to start once it's downloaded: a click on the track.
    start: f64,
}

pub struct Player {
    pub now: Option<Now>,
    child: Option<Child>,
    ipc: Option<UnixStream>,
    gen: u64,
    sock: PathBuf,
    tx: Sender<crate::Ev>,
}

impl Player {
    pub fn new(tx: Sender<crate::Ev>) -> Player {
        let dir = std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
        let sock = dir.join(format!("whatsapp-tui-mpv-{}.sock", std::process::id()));
        Player { now: None, child: None, ipc: None, gen: 0, sock, tx }
    }

    /// The state of one message's audio, when it's the one.
    pub fn of(&self, chat: &str, id: &str) -> Option<&Now> {
        self.now.as_ref().filter(|n| n.chat == chat && n.id == id)
    }

    /// Stops what plays and waits for this one's file, to start at `start` seconds.
    pub fn load(&mut self, chat: &str, id: &str, dur: f64, start: f64) {
        self.stop();
        self.now = Some(Now { chat: chat.into(), id: id.into(), state: State::Loading, pos: start, dur, start });
    }

    /// The file's here: play it, if it's still the one wanted.
    pub fn ready(&mut self, chat: &str, id: &str, path: &str) -> Result<(), String> {
        let Some(start) = self.of(chat, id).filter(|n| n.state == State::Loading).map(|n| n.start) else { return Ok(()) };
        let res = self.spawn(path, start);
        if res.is_err() {
            self.stop();
        }
        res
    }

    fn spawn(&mut self, path: &str, start: f64) -> Result<(), String> {
        self.kill();
        self.gen += 1;
        let _ = std::fs::remove_file(&self.sock);
        let child = Command::new("mpv")
            .args(["--no-video", "--no-terminal", "--force-window=no", "--idle=no"])
            .arg(format!("--input-ipc-server={}", self.sock.display()))
            .arg(format!("--start={start:.2}"))
            .arg("--")
            .arg(path)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| match e.kind() {
                std::io::ErrorKind::NotFound => "Instale o mpv para ouvir áudios".to_string(),
                _ => format!("não tocou: {e}"),
            })?;
        self.child = Some(child);
        // mpv makes the socket within a few dozen milliseconds of starting.
        let until = Instant::now() + Duration::from_secs(3);
        let stream = loop {
            match UnixStream::connect(&self.sock) {
                Ok(s) => break s,
                Err(_) if Instant::now() < until && self.child.as_mut().is_some_and(|c| matches!(c.try_wait(), Ok(None))) => {
                    std::thread::sleep(Duration::from_millis(15))
                }
                Err(_) => return Err("o mpv não abriu o áudio".into()),
            }
        };
        let reader = stream.try_clone().map_err(|e| e.to_string())?;
        self.ipc = Some(stream);
        for (i, prop) in ["time-pos", "duration", "pause"].iter().enumerate() {
            self.send(json!({"command": ["observe_property", i + 1, prop]}));
        }
        let (tx, gen) = (self.tx.clone(), self.gen);
        std::thread::spawn(move || read(reader, gen, tx));
        if let Some(n) = self.now.as_mut() {
            n.state = State::Playing;
        }
        Ok(())
    }

    fn send(&mut self, cmd: Value) {
        if let Some(s) = &mut self.ipc {
            let _ = writeln!(s, "{cmd}");
        }
    }

    pub fn toggle(&mut self) {
        let Some(state) = self.now.as_ref().map(|n| n.state) else { return };
        let pause = match state {
            State::Loading => return,
            State::Playing => true,
            State::Paused => false,
        };
        self.send(json!({"command": ["set_property", "pause", pause]}));
        if let Some(n) = self.now.as_mut() {
            n.state = if pause { State::Paused } else { State::Playing };
        }
    }

    pub fn seek(&mut self, to: f64) {
        let Some(n) = self.now.as_mut() else { return };
        n.pos = to;
        if n.state == State::Loading {
            n.start = to;
        } else {
            self.send(json!({"command": ["seek", to, "absolute"]}));
        }
    }

    pub fn stop(&mut self) {
        self.kill();
        self.now = None;
    }

    fn kill(&mut self) {
        self.ipc = None;
        if let Some(mut c) = self.child.take() {
            let _ = c.kill();
            let _ = c.wait();
        }
    }

    pub fn on(&mut self, ev: Ev) {
        let gen = match ev {
            Ev::Pos { gen, .. } | Ev::Duration { gen, .. } | Ev::Paused { gen, .. } | Ev::Ended { gen } => gen,
        };
        if gen != self.gen {
            return;
        }
        if let Ev::Ended { .. } = ev {
            return self.stop();
        }
        let Some(n) = self.now.as_mut() else { return };
        match ev {
            Ev::Pos { pos, .. } => n.pos = pos,
            Ev::Duration { dur, .. } => n.dur = dur,
            Ev::Paused { paused, .. } => n.state = if paused { State::Paused } else { State::Playing },
            Ev::Ended { .. } => {}
        }
    }
}

impl Drop for Player {
    fn drop(&mut self) {
        self.kill();
        let _ = std::fs::remove_file(&self.sock);
    }
}

/// mpv's events until it exits; the position at most 5 times a second.
fn read(stream: UnixStream, gen: u64, tx: Sender<crate::Ev>) {
    let send = |e: Ev| tx.send(crate::Ev::Player(e)).is_ok();
    let mut last = Instant::now() - Duration::from_secs(1);
    for line in BufReader::new(stream).lines().map_while(Result::ok) {
        let Ok(v) = serde_json::from_str::<Value>(&line) else { continue };
        let ok = match (v["event"].as_str(), v["name"].as_str()) {
            (Some("property-change"), Some("time-pos")) => match v["data"].as_f64() {
                Some(pos) if last.elapsed() >= Duration::from_millis(200) => {
                    last = Instant::now();
                    send(Ev::Pos { gen, pos })
                }
                _ => true,
            },
            (Some("property-change"), Some("duration")) => v["data"].as_f64().is_none_or(|dur| send(Ev::Duration { gen, dur })),
            (Some("property-change"), Some("pause")) => v["data"].as_bool().is_none_or(|paused| send(Ev::Paused { gen, paused })),
            _ => true,
        };
        if !ok {
            return;
        }
    }
    let _ = tx.send(crate::Ev::Player(Ev::Ended { gen }));
}
