//! `~/.config/whatsapp-tui/config`, `key = value` lines as slick keeps its
//! own, and the switches the settings dialog flips there.

use std::collections::HashMap;
use std::path::PathBuf;

pub fn path() -> PathBuf {
    crate::bridge::config_dir().join("config")
}

/// A `key = value` line of the config.
pub fn value(key: &str) -> Option<String> {
    let text = std::fs::read_to_string(path()).ok()?;
    text.lines().find_map(|line| {
        let (k, v) = line.split_once('=')?;
        (k.trim() == key).then(|| v.trim().trim_matches('"').to_string()).filter(|v| !v.is_empty())
    })
}

/// Rewrites these keys, keeping every other line as it was; written beside
/// and renamed over.
pub fn set(values: &[(&str, String)]) -> std::io::Result<()> {
    let p = path();
    std::fs::create_dir_all(crate::bridge::config_dir())?;
    let old = std::fs::read_to_string(&p).unwrap_or_default();
    let mut text: String = old
        .lines()
        .filter(|l| !l.split_once('=').is_some_and(|(k, _)| values.iter().any(|(key, _)| k.trim() == *key)))
        .map(|l| format!("{l}\n"))
        .collect();
    for (k, v) in values {
        if !v.is_empty() {
            text.push_str(&format!("{k} = {v}\n"));
        }
    }
    let tmp = p.with_extension("tmp");
    std::fs::write(&tmp, text)?;
    std::fs::rename(tmp, p)
}

/// An on/off setting: its key in the config, its words in the dialog, and
/// what it is when the config doesn't say.
#[derive(Clone, Copy, PartialEq)]
pub struct Opt {
    pub key: &'static str,
    pub label: &'static str,
    pub default: bool,
}

pub const PHOTOS: Opt = Opt { key: "sidebar_avatars", label: "Mostrar fotos na lista", default: true };
pub const RECEIPTS: Opt = Opt { key: "read_receipts", label: "Confirmações de leitura", default: true };
pub const ENTER_SENDS: Opt = Opt { key: "enter_sends", label: "Enter envia a mensagem", default: true };
pub const TRAY: Opt = Opt { key: "tray", label: "Mostrar o ícone na bandeja", default: true };
pub const VIDEO_INLINE: Opt = Opt { key: "video_inline", label: "Tocar vídeos no app", default: true };
pub const AUTO_UPDATE: Opt = Opt { key: "auto_update", label: "Procurar atualizações automaticamente", default: true };

/// The settings dialog, top to bottom: a heading over each group's switches.
/// A new setting is a const above and its place here.
pub const SECTIONS: &[(&str, &[Opt])] = &[
    ("Barra lateral", &[PHOTOS]),
    ("Conversas", &[RECEIPTS]),
    ("Mensagens", &[ENTER_SENDS, VIDEO_INLINE]),
    ("Bandeja do sistema", &[TRAY]),
    ("Atualizações", &[AUTO_UPDATE]),
];

/// Looking for new versions at start-up and every 6h (`update`); read fresh
/// each time, so the settings switch counts without a restart.
pub fn auto_update() -> bool {
    value(AUTO_UPDATE.key).is_none_or(|v| v != "false")
}

/// Every switch, in the dialog's order.
pub fn opts() -> Vec<Opt> {
    SECTIONS.iter().flat_map(|(_, o)| o.iter().copied()).collect()
}

/// The switches as the config had them at start, then as flipped here.
pub struct Settings(HashMap<&'static str, bool>);

impl Settings {
    pub fn load() -> Settings {
        Settings(opts().into_iter().map(|o| (o.key, value(o.key).map_or(o.default, |v| v != "false"))).collect())
    }

    pub fn on(&self, o: Opt) -> bool {
        self.0.get(o.key).copied().unwrap_or(o.default)
    }

    /// Flips it here and in the config; here it stays flipped either way.
    pub fn flip(&mut self, o: Opt) -> std::io::Result<()> {
        let on = !self.on(o);
        self.0.insert(o.key, on);
        set(&[(o.key, on.to_string())])
    }
}
