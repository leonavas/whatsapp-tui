//! The commands for scripts and the Omarchy widget: they talk to the
//! running whatsapp-tui (`ipc`), or do without it where they can.

use crate::ipc::{self, Cmd};
use serde_json::{json, Value};
use std::io::IsTerminal;

/// `println!` that doesn't panic when the reader went away (`whatsapp-tui status | head`).
macro_rules! out {
    ($($t:tt)*) => {{
        use std::io::Write;
        let _ = writeln!(std::io::stdout(), $($t)*);
    }};
}

pub enum Next {
    Exit(i32),
    /// Nothing running and a terminal here: open whatsapp-tui, then do this.
    Start(Option<Cmd>),
}

pub fn usage() -> &'static str {
    "\
usage:
  whatsapp-tui                  open whatsapp-tui, or bring the open one to the front
  whatsapp-tui --new-instance   open another one alongside the running one
  whatsapp-tui --version        show the version

talking to the running whatsapp-tui (see docs/events.md):
  whatsapp-tui open <chat> [message-id] [--background]
  whatsapp-tui focus
  whatsapp-tui mark-read <chat>
  whatsapp-tui pause <minutes | 30m | 1h | 8h | tomorrow>
  whatsapp-tui resume
  whatsapp-tui quit
  whatsapp-tui status [--json]  what is unread, as JSON for scripts
  whatsapp-tui events           follow new messages, counts and pauses (JSON lines)
"
}

/// `None` when the first argument is none of these commands.
pub fn run(args: &[String]) -> Option<Next> {
    let rest = &args[1.min(args.len())..];
    Some(match args.first()?.as_str() {
        "open" => open(rest),
        "focus" => focus(),
        "quit" => Next::Exit(if ipc::running() { send(json!({ "cmd": "quit" })) } else { 0 }),
        "mark-read" => match rest.first() {
            Some(chat) => Next::Exit(send_running(json!({ "cmd": "mark-read", "chat": chat }))),
            None => usage_error(),
        },
        "pause" => match rest.first().and_then(|d| until(d)) {
            Some(t) => Next::Exit(pause(Some(t))),
            None => usage_error(),
        },
        "resume" => Next::Exit(pause(None)),
        "status" => Next::Exit(status(rest.iter().any(|a| a == "--json"))),
        "events" => Next::Exit(ipc::follow()),
        _ => return None,
    })
}

fn usage_error() -> Next {
    eprint!("{}", usage());
    Next::Exit(2)
}

fn send(cmd: Value) -> i32 {
    match ipc::send(&cmd) {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("whatsapp-tui: didn't reach the running whatsapp-tui: {e}");
            1
        }
    }
}

fn send_running(cmd: Value) -> i32 {
    if ipc::running() {
        send(cmd)
    } else {
        eprintln!("whatsapp-tui isn't running");
        1
    }
}

/// `whatsapp-tui open <chat> [message-id] [--background]`.
fn open(args: &[String]) -> Next {
    let focus = !args.iter().any(|a| a == "--background");
    let pos: Vec<&String> = args.iter().filter(|a| !a.starts_with("--")).collect();
    let Some(chat) = pos.first().map(|s| s.to_string()) else { return usage_error() };
    let id = pos.get(1).map(|s| s.to_string());
    if ipc::running() {
        return Next::Exit(send(json!({ "cmd": "open", "chat": chat, "id": id, "focus": focus })));
    }
    if std::io::stdin().is_terminal() && std::io::stdout().is_terminal() {
        return Next::Start(Some(Cmd::Open { chat, id, focus }));
    }
    let mut again = vec!["open".to_string()];
    again.extend(args.iter().cloned());
    Next::Exit(launch(&again))
}

/// `whatsapp-tui focus`: the running one to the front, or a new one.
fn focus() -> Next {
    if ipc::running() {
        return Next::Exit(send(json!({ "cmd": "focus" })));
    }
    if std::io::stdin().is_terminal() && std::io::stdout().is_terminal() {
        return Next::Start(None);
    }
    Next::Exit(launch(&[]))
}

/// Opens whatsapp-tui in a new terminal window, with these arguments.
fn launch(args: &[String]) -> i32 {
    let exe = std::env::current_exe().unwrap_or_else(|_| "whatsapp-tui".into());
    let spawned = std::process::Command::new("xdg-terminal-exec")
        .arg(format!("--app-id={}", crate::tray::window_class()))
        .arg("-e")
        .arg(exe)
        .args(args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
    match spawned {
        Ok(_) => 0,
        Err(e) => {
            eprintln!("whatsapp-tui isn't running, and didn't open a terminal for it: {e}");
            1
        }
    }
}

/// `45`, `30m`, `1h`, `1h30m` from now, or `tomorrow` (9 AM): Unix seconds.
fn until(s: &str) -> Option<i64> {
    use chrono::{Days, Local, NaiveTime, TimeZone};
    let now = Local::now();
    if s == "tomorrow" {
        let day = now.date_naive().checked_add_days(Days::new(1))?;
        return Local.from_local_datetime(&day.and_time(NaiveTime::from_hms_opt(9, 0, 0)?)).earliest().map(|t| t.timestamp());
    }
    if let Ok(m) = s.parse::<i64>() {
        return (m > 0).then(|| now.timestamp() + m * 60);
    }
    let (mut secs, mut num) = (0i64, String::new());
    for c in s.chars() {
        match c {
            '0'..='9' => num.push(c),
            'h' | 'm' => {
                let n: i64 = std::mem::take(&mut num).parse().ok()?;
                secs += n * if c == 'h' { 3600 } else { 60 };
            }
            _ => return None,
        }
    }
    (num.is_empty() && secs > 0).then(|| now.timestamp() + secs)
}

/// With whatsapp-tui running it takes the pause; closed, the file is all there is.
fn pause(until: Option<i64>) -> i32 {
    let code = if ipc::running() {
        send(match until {
            Some(t) => json!({ "cmd": "pause", "until": t }),
            None => json!({ "cmd": "resume" }),
        })
    } else {
        match crate::events::save_pause(until) {
            Ok(()) => 0,
            Err(e) => {
                eprintln!("whatsapp-tui: didn't save the pause: {e}");
                1
            }
        }
    };
    if code == 0 {
        out!("{}", describe(until));
    }
    code
}

fn describe(until: Option<i64>) -> String {
    match until.and_then(|t| chrono::DateTime::from_timestamp(t, 0)) {
        Some(t) => {
            let t = t.with_timezone(&chrono::Local);
            let day = if t.date_naive() == chrono::Local::now().date_naive() { String::new() } else { t.format("%b %-d, ").to_string() };
            format!("notifications paused until {day}{}", t.format("%H:%M"))
        }
        None => "notifications on".into(),
    }
}

/// What the running whatsapp-tui last wrote; `{"running": false}` when none is.
fn snapshot() -> Value {
    let stopped = json!({ "v": 1, "running": false, "paused": { "paused": crate::events::load_pause().is_some(), "until": crate::events::load_pause() } });
    if !ipc::running() {
        return stopped;
    }
    std::fs::read_to_string(crate::events::state_path()).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or(stopped)
}

/// `whatsapp-tui status [--json]`: the unread chats, as the widget sees them.
fn status(as_json: bool) -> i32 {
    let s = snapshot();
    if as_json {
        out!("{s}");
        return 0;
    }
    if s["paused"]["paused"] == true {
        out!("{}", describe(s["paused"]["until"].as_i64()));
    }
    if s["running"] != true {
        out!("whatsapp-tui isn't running");
        return 0;
    }
    let mut head = format!("whatsapp-tui · {}", s["user"].as_str().unwrap_or(""));
    head += &match s["totals"]["chats"].as_u64().unwrap_or(0) {
        0 => " · nothing unread".to_string(),
        1 => " · 1 unread chat".to_string(),
        n => format!(" · {n} unread chats"),
    };
    out!("{head}");
    for c in s["conversations"].as_array().into_iter().flatten() {
        let name = c["name"].as_str().unwrap_or("");
        let last = c["messages"].as_array().and_then(|m| m.last());
        let line = last.map(|m| {
            let text: String = m["text"].as_str().unwrap_or("").split_whitespace().collect::<Vec<_>>().join(" ").chars().take(80).collect();
            if c["kind"] == "group" {
                format!(" · {}: {text}", m["author"].as_str().unwrap_or(""))
            } else {
                format!(" · {text}")
            }
        });
        let muted = if c["muted"] == true { " (muted)" } else { "" };
        out!("  {name} ({}){muted}{}", c["unread"], line.unwrap_or_default());
    }
    0
}
