//! The system tray icon (StatusNotifierItem over D-Bus): a dot while chats
//! wait, a click focuses the window, the menu opens or quits. Without a
//! session bus or a tray host it quietly does nothing.

use crate::ipc::Cmd;
use crate::Ev;
use ksni::blocking::TrayMethods;
use std::process::{Command, Stdio};
use std::sync::mpsc::{self, Sender};

/// The app's side: hands the tray thread a new count only when it changes.
pub struct Tray {
    tx: Sender<usize>,
    last: Option<usize>,
}

impl Tray {
    /// Connects in its own thread, so a slow or missing bus never holds up the TUI.
    pub fn start(events: Sender<Ev>) -> Tray {
        let (tx, rx) = mpsc::channel::<usize>();
        std::thread::spawn(move || {
            let item = Item { events, unread: 0 };
            // A bar started later still finds us; no bus at all ends here.
            let Ok(handle) = item.assume_sni_available(true).spawn() else { return };
            for n in rx {
                if handle.update(|i| i.unread = n).is_none() {
                    return;
                }
            }
            handle.shutdown().wait();
        });
        Tray { tx, last: None }
    }

    /// Unread chats that ping: neither muted nor archived.
    pub fn set(&mut self, n: usize) {
        if self.last != Some(n) {
            self.last = Some(n);
            let _ = self.tx.send(n);
        }
    }
}

struct Item {
    events: Sender<Ev>,
    unread: usize,
}

impl Item {
    fn label(&self) -> String {
        match self.unread {
            0 => "WhatsApp".into(),
            1 => "WhatsApp · 1 conversa não lida".into(),
            n => format!("WhatsApp · {n} conversas não lidas"),
        }
    }
}

impl ksni::Tray for Item {
    fn id(&self) -> String {
        crate::bridge::NAME.into()
    }

    fn title(&self) -> String {
        self.label()
    }

    fn category(&self) -> ksni::Category {
        ksni::Category::Communications
    }

    fn icon_pixmap(&self) -> Vec<ksni::Icon> {
        let set: [(i32, &[u8]); 3] = if self.unread > 0 {
            [
                (22, include_bytes!("../assets/tray/unread-22.argb")),
                (32, include_bytes!("../assets/tray/unread-32.argb")),
                (64, include_bytes!("../assets/tray/unread-64.argb")),
            ]
        } else {
            [
                (22, include_bytes!("../assets/tray/normal-22.argb")),
                (32, include_bytes!("../assets/tray/normal-32.argb")),
                (64, include_bytes!("../assets/tray/normal-64.argb")),
            ]
        };
        set.iter().map(|(n, data)| ksni::Icon { width: *n, height: *n, data: data.to_vec() }).collect()
    }

    fn tool_tip(&self) -> ksni::ToolTip {
        ksni::ToolTip { title: self.label(), ..Default::default() }
    }

    fn activate(&mut self, _x: i32, _y: i32) {
        focus();
    }

    fn menu(&self) -> Vec<ksni::MenuItem<Self>> {
        use ksni::menu::StandardItem;
        vec![
            StandardItem { label: "Abrir".into(), activate: Box::new(|_: &mut Self| focus()), ..Default::default() }.into(),
            StandardItem {
                label: "Sair".into(),
                activate: Box::new(|i: &mut Self| {
                    let _ = i.events.send(Ev::Remote(Cmd::Quit));
                }),
                ..Default::default()
            }
            .into(),
        ]
    }
}

/// The class of the terminal window whatsapp-tui runs in, as launchers set it.
pub fn window_class() -> String {
    std::env::var("WHATSAPP_TUI_WINDOW_CLASS").ok().filter(|c| !c.is_empty()).unwrap_or_else(|| "org.omarchy.whatsapp-tui".into())
}

/// Brings the terminal running whatsapp-tui to the front. Hyprland only.
pub fn focus() {
    if std::env::var_os("HYPRLAND_INSTANCE_SIGNATURE").is_none() {
        return;
    }
    let class = window_class();
    std::thread::spawn(move || {
        // Lua configs take `eval`; the classic dispatcher is the fallback.
        let lua = format!("hl.dispatch(hl.dsp.focus({{ window = \"class:{class}\" }}))");
        if !hyprctl(&["eval", &lua]) {
            hyprctl(&["dispatch", "focuswindow", &format!("class:{class}")]);
        }
    });
}

/// Whether hyprctl ran and didn't answer with an error.
fn hyprctl(args: &[&str]) -> bool {
    Command::new("hyprctl")
        .args(args)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .is_ok_and(|o| o.status.success() && !String::from_utf8_lossy(&o.stdout).to_lowercase().contains("error"))
}
