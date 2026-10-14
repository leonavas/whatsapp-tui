//! New versions from the GitHub releases, as in slick: looked for at
//! start-up and every few hours, downloaded and then installed over this
//! binary from the account menu (or the button by your name), unless
//! something else (cargo, the system's package manager, nix) owns it; then it
//! only tells how to update. The Go
//! helper rides inside the binary, so the new one brings its own
//! (`bridge::program` writes it to a new `bridge-<hash>/`).

use crate::Ev;
use sha2::{Digest, Sha256};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::sync::OnceLock;

const VERSION: &str = env!("CARGO_PKG_VERSION");
const LATEST: &str = "https://api.github.com/repos/leonavas/whatsapp-tui/releases/latest";
const REPO: &str = "https://github.com/leonavas/whatsapp-tui";
const NAME: &str = "whatsapp-tui";
/// Between checks while whatsapp-tui is open.
const EVERY: std::time::Duration = std::time::Duration::from_secs(6 * 3600);

/// The app speaks Portuguese; `whatsapp-tui update` speaks English, as the
/// rest of the CLI.
static ENGLISH: AtomicBool = AtomicBool::new(false);

fn tr(en: &str, pt: &str) -> String {
    if ENGLISH.load(Ordering::Relaxed) { en } else { pt }.to_string()
}

/// Download links by file name.
pub type Assets = Vec<(String, String)>;

/// This machine's archive of a release, downloaded and checked, waiting to be installed.
pub struct Package {
    name: String,
    tar: Vec<u8>,
}

pub enum Note {
    /// A newer release than this binary.
    Found { ver: String, assets: Assets },
    /// How far the download is, in percent.
    Progress(u8),
    /// The download asked for finished: the version and its package, or why not.
    Downloaded(Result<(String, Package), String>),
    /// The install asked for finished: the version, or why not.
    Installed(Result<String, String>),
    /// A check asked for from the menu: a newer version, none, or why not.
    Checked(Result<Option<(String, Assets)>, String>),
}

/// Where the update stands, shown in the account menu and as the button by your name.
pub enum State {
    /// Found; a click downloads it.
    Available { ver: String, assets: Assets },
    /// Downloading; the percent once the server tells the size.
    Downloading { ver: String, assets: Assets, pct: Option<u8> },
    /// Downloaded and checked; a click installs it and restarts.
    Downloaded { ver: String, assets: Assets, pkg: Package },
    /// Swapping the binary.
    Installing { ver: String, assets: Assets },
    /// Installed over the running binary; a restart picks it up.
    Installed(String),
}

impl State {
    /// After a check found `ver`: kept while it works, and once downloaded
    /// or installed unless `ver` is newer still.
    pub fn found(prev: Option<State>, ver: String, assets: Assets) -> State {
        match prev {
            Some(s @ (State::Downloading { .. } | State::Installing { .. })) => s,
            Some(s @ State::Downloaded { .. }) if !newer(&ver, s.ver()) => s,
            Some(State::Installed(v)) if !newer(&ver, &v) => State::Installed(v),
            _ => State::Available { ver, assets },
        }
    }

    pub fn ver(&self) -> &str {
        match self {
            State::Available { ver, .. }
            | State::Downloading { ver, .. }
            | State::Downloaded { ver, .. }
            | State::Installing { ver, .. }
            | State::Installed(ver) => ver,
        }
    }

    /// Back to a click away from downloading, after a download or an install failed.
    pub fn retry(self) -> State {
        match self {
            State::Downloading { ver, assets, .. } | State::Downloaded { ver, assets, .. } | State::Installing { ver, assets } => {
                State::Available { ver, assets }
            }
            s => s,
        }
    }
}

#[derive(Clone, Copy)]
enum Mode {
    /// A cargo build (`target/`): nothing is checked.
    Off,
    /// Owned by another tool, or in a folder we can't write to.
    Managed,
    /// Ours to replace.
    Own,
}

/// The installed binary; once replaced, Linux names the running one "… (deleted)".
pub fn exe() -> Option<PathBuf> {
    let p = std::env::current_exe().ok()?;
    let s = p.to_string_lossy();
    Some(s.strip_suffix(" (deleted)").map(PathBuf::from).unwrap_or(p))
}

fn home() -> PathBuf {
    PathBuf::from(std::env::var_os("HOME").unwrap_or_default())
}

fn cargo_bin() -> PathBuf {
    std::env::var_os("CARGO_HOME").map(PathBuf::from).unwrap_or_else(|| home().join(".cargo")).join("bin")
}

/// From a release (install.sh in ~/.local/bin, or any folder you can write
/// to) it is ours. Once per run: the menu asks on every draw.
fn mode() -> Mode {
    static MODE: OnceLock<Mode> = OnceLock::new();
    *MODE.get_or_init(find_mode)
}

fn find_mode() -> Mode {
    let Some(exe) = exe().filter(|_| cfg!(target_os = "linux")) else { return Mode::Off };
    if exe.components().any(|c| c.as_os_str() == "target") {
        return Mode::Off;
    }
    let s = exe.to_string_lossy();
    let system = ["/usr/", "/nix/", "/opt/"].iter().any(|o| s.starts_with(o));
    let dir = exe.parent().unwrap_or(Path::new("/"));
    if exe.starts_with(cargo_bin()) || system || !writable(dir) {
        return Mode::Managed;
    }
    Mode::Own
}

/// `WHATSAPP_TUI_UPDATE_DEV=1` checks even from `target/`, to see the button.
fn dev() -> bool {
    std::env::var("WHATSAPP_TUI_UPDATE_DEV").is_ok_and(|v| v == "1")
}

/// None when a click can install; else how to update, short for the menu
/// and in full for the status line.
fn manager() -> Option<(&'static str, String)> {
    let exe = exe().unwrap_or_default();
    let s = exe.to_string_lossy();
    let dir = exe.parent().map(|d| d.display().to_string()).unwrap_or_default();
    let msg = match mode() {
        Mode::Own => return None,
        Mode::Off => ("Compile de novo", "build de desenvolvimento: não se atualiza, compile de novo".to_string()),
        Mode::Managed if exe.starts_with(cargo_bin()) => {
            ("Atualize via cargo", format!("instalado via cargo: rode cargo install --locked --git {REPO}"))
        }
        Mode::Managed if s.starts_with("/nix/") => ("Atualize pelo nix", "instalado via nix: atualize pelo nix".into()),
        Mode::Managed if s.starts_with("/usr/") || s.starts_with("/opt/") => {
            ("Atualize pelo gerenciador", "instalado pelo sistema: atualize pelo gerenciador de pacotes".into())
        }
        Mode::Managed => ("Baixe do GitHub", format!("{dir} não é gravável: baixe a versão nova em {REPO}/releases")),
    };
    Some(msg)
}

/// None when a click can install; else how to update, for the status line.
pub fn how() -> Option<String> {
    manager().map(|(_, long)| long)
}

/// `how`, a few words for the menu.
pub fn how_short() -> Option<&'static str> {
    manager().map(|(short, _)| short)
}

fn writable(dir: &Path) -> bool {
    let probe = dir.join(format!(".{NAME}-probe-{}", std::process::id()));
    std::fs::write(&probe, b"").and_then(|_| std::fs::remove_file(&probe)).is_ok()
}

/// "1.10.0" > "1.9.2"; whatever follows the digits of a part ("0-rc1") is ignored.
fn newer(a: &str, b: &str) -> bool {
    let parts = |v: &str| -> Vec<u64> {
        v.split('.').map(|p| p.chars().take_while(char::is_ascii_digit).collect::<String>().parse().unwrap_or(0)).collect()
    };
    parts(a) > parts(b)
}

/// The latest release's version (without the "v") and its download links by file name.
fn latest() -> Result<(String, Assets), String> {
    let res = ureq::get(LATEST)
        .set("User-Agent", concat!("whatsapp-tui/", env!("CARGO_PKG_VERSION")))
        .set("Accept", "application/vnd.github+json")
        .call()
        .map_err(|e| e.to_string())?;
    let v: serde_json::Value = serde_json::from_reader(res.into_reader().take(4 << 20)).map_err(|e| e.to_string())?;
    let tag = v["tag_name"].as_str().ok_or(tr("release without a tag", "release sem tag"))?.trim_start_matches('v').to_string();
    let assets = v["assets"].as_array().into_iter().flatten();
    let assets = assets
        .filter_map(|a| Some((a["name"].as_str()?.to_string(), a["browser_download_url"].as_str()?.to_string())))
        .collect();
    Ok((tag, assets))
}

/// `url`'s bytes, telling `progress` each new percent when the size is known.
fn download(url: &str, progress: &dyn Fn(u8)) -> Result<Vec<u8>, String> {
    let res = ureq::get(url).set("User-Agent", NAME).call().map_err(|e| e.to_string())?;
    let total = res.header("Content-Length").and_then(|n| n.parse::<u64>().ok()).filter(|n| *n > 0);
    let mut reader = res.into_reader().take(300 << 20);
    let (mut bytes, mut buf, mut last) = (Vec::new(), vec![0; 64 << 10], None);
    loop {
        let n = match reader.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e.to_string()),
        };
        bytes.extend_from_slice(&buf[..n]);
        if let Some(total) = total {
            let pct = (bytes.len() as u64 * 100 / total).min(100) as u8;
            if last != Some(pct) {
                last = Some(pct);
                progress(pct);
            }
        }
    }
    Ok(bytes)
}

/// This machine's archive in a release.
fn archive(ver: &str) -> String {
    format!("{NAME}-{ver}-{}-unknown-linux-gnu.tar.gz", std::env::consts::ARCH)
}

/// Downloads this machine's archive and checks its sha256.
fn fetch(ver: &str, assets: &[(String, String)], log: &dyn Fn(&str), progress: &dyn Fn(u8)) -> Result<Package, String> {
    let name = archive(ver);
    let url = |n: &str| {
        assets.iter().find(|(a, _)| a == n).map(|(_, u)| u.clone()).ok_or(tr(&format!("release without {n}"), &format!("release sem {n}")))
    };
    let (tar_url, sum_url) = (url(&name)?, url(&format!("{name}.sha256"))?);
    log(&tr(&format!("downloading {name}…"), &format!("baixando {name}…")));
    let tar = download(&tar_url, progress)?;
    let sum = String::from_utf8_lossy(&download(&sum_url, &|_| {})?).split_whitespace().next().unwrap_or("").to_lowercase();
    let got: String = Sha256::digest(&tar).iter().map(|b| format!("{b:02x}")).collect();
    if got != sum {
        return Err(tr(&format!("sha256 doesn't match ({got} ≠ {sum})"), &format!("sha256 não confere ({got} ≠ {sum})")));
    }
    Ok(Package { name, tar })
}

/// Swaps `whatsapp-tui` for the package's; the desktop entry and icons too,
/// where install.sh put them.
fn put(pkg: &Package) -> Result<(), String> {
    let exe = exe().ok_or(tr("couldn't find the executable", "não achou o executável"))?;
    let dir = exe.parent().ok_or(tr("executable without a folder", "executável sem pasta"))?;
    let Package { name, tar } = pkg;
    // Unpacked next to the binary, so the rename below stays on one filesystem.
    let tmp = dir.join(format!(".{NAME}-update-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).map_err(|e| e.to_string())?;
    let done = (|| {
        std::fs::write(tmp.join(name), tar).map_err(|e| e.to_string())?;
        let ok = std::process::Command::new("tar").arg("-xzf").arg(name).current_dir(&tmp).status();
        if !ok.is_ok_and(|s| s.success()) {
            return Err(tr("tar didn't extract the package", "tar não extraiu o pacote"));
        }
        // Replaced by a rename: the running one keeps its own copy.
        let new = PathBuf::from(format!("{}.new", exe.display()));
        std::fs::rename(tmp.join(NAME), &new).map_err(|e| format!("{NAME}: {e}"))?;
        std::fs::set_permissions(&new, std::os::unix::fs::PermissionsExt::from_mode(0o755)).map_err(|e| e.to_string())?;
        std::fs::rename(&new, &exe).map_err(|e| format!("{NAME}: {e}"))?;
        desktop(&tmp);
        Ok(())
    })();
    let _ = std::fs::remove_dir_all(&tmp);
    done
}

/// The launcher entry and icons from the archive, over those install.sh
/// left in ~/.local/share; nothing when there are none. Best effort.
fn desktop(pkg: &Path) {
    let share = crate::bridge::xdg("XDG_DATA_HOME", ".local/share");
    let entry = share.join("applications").join(format!("{NAME}.desktop"));
    if !entry.exists() {
        return;
    }
    let icons = share.join("icons/hicolor");
    let files = [
        (format!("{NAME}.desktop"), entry),
        (format!("{NAME}.svg"), icons.join("scalable/apps").join(format!("{NAME}.svg"))),
        (format!("{NAME}-256.png"), icons.join("256x256/apps").join(format!("{NAME}.png"))),
    ];
    for (from, to) in files {
        let from = pkg.join(from);
        if from.exists() {
            if let Some(d) = to.parent() {
                let _ = std::fs::create_dir_all(d);
            }
            let _ = std::fs::copy(from, to);
        }
    }
}

/// At start-up and every `EVERY` after, while `auto_update` isn't `false`
/// in the config (read each round, so the settings switch counts). Quiet on
/// errors: the next round tries again.
pub fn background(tx: Sender<Ev>) {
    let dev = dev();
    if matches!(mode(), Mode::Off) && !dev {
        return;
    }
    std::thread::spawn(move || loop {
        let found = match crate::config::auto_update().then(latest) {
            Some(Ok((ver, assets))) if dev || newer(&ver, VERSION) => Some((ver, assets)),
            // No release to show: a made-up one, so the button still shows.
            Some(Err(_)) if dev => Some((format!("{VERSION}-dev"), vec![])),
            _ => None,
        };
        if let Some((ver, assets)) = found {
            if tx.send(Ev::Update(Note::Found { ver, assets })).is_err() {
                return;
            }
        }
        std::thread::sleep(EVERY);
    });
}

/// This version, as the menu's check reports it.
pub fn current() -> &'static str {
    VERSION
}

/// A development build checks only with `WHATSAPP_TUI_UPDATE_DEV=1`.
pub fn check_disabled() -> bool {
    matches!(mode(), Mode::Off) && !dev()
}

/// "Procurar atualizações" in the account menu: looks now, even with
/// `auto_update = false`, and reports errors. It blocks, so it runs in a thread.
pub fn check_now() -> Note {
    let dev = dev();
    let res = match latest() {
        Ok((ver, assets)) if dev || newer(&ver, VERSION) => Ok(Some((ver, assets))),
        Ok(_) => Ok(None),
        // No release to show: a made-up one, as in `background`.
        Err(_) if dev => Ok(Some((format!("{VERSION}-dev"), vec![]))),
        Err(e) => Err(e),
    };
    Note::Checked(res)
}

/// Downloads `ver` for a click, telling `progress` how far; it blocks, so it runs in a thread.
pub fn download_now(ver: String, assets: Assets, progress: impl Fn(u8)) -> Note {
    Note::Downloaded(fetch(&ver, &assets, &|_| {}, &progress).map(|pkg| (ver, pkg)))
}

/// Installs the downloaded `ver` for a click; it blocks, so it runs in a thread.
pub fn install_now(ver: String, pkg: Package) -> Note {
    Note::Installed(put(&pkg).map(|()| ver))
}

/// `whatsapp-tui update`: checks now and installs, telling what it does.
pub fn cli() -> i32 {
    ENGLISH.store(true, Ordering::Relaxed);
    let path = exe().map(|p| p.display().to_string()).unwrap_or_default();
    if matches!(mode(), Mode::Off) {
        eprintln!("{path}: development build, doesn't update itself");
        return 1;
    }
    eprintln!("whatsapp-tui {VERSION} · looking for a new version…");
    let (ver, assets) = match latest() {
        Ok(l) => l,
        Err(e) => {
            eprintln!("couldn't check the releases: {e}");
            return 1;
        }
    };
    if !newer(&ver, VERSION) {
        eprintln!("already the latest");
        return 0;
    }
    if matches!(mode(), Mode::Managed) {
        eprintln!("whatsapp-tui {ver} available; {path} belongs to another tool (or the folder isn't writable): update through it");
        return 1;
    }
    match fetch(&ver, &assets, &|s| eprintln!("{s}"), &|_| {}).and_then(|pkg| put(&pkg)) {
        Ok(()) => {
            eprintln!("updated to {ver}; a running whatsapp-tui picks it up when restarted");
            0
        }
        Err(e) => {
            eprintln!("didn't update: {e}");
            1
        }
    }
}

/// Runs the new binary in this process's place, in the same terminal, with
/// the same arguments. Returns only when that fails.
pub fn restart() -> std::io::Error {
    let Some(exe) = exe() else { return std::io::Error::other("não achou o executável") };
    let mut cmd = std::process::Command::new(exe);
    cmd.args(std::env::args_os().skip(1));
    std::os::unix::process::CommandExt::exec(&mut cmd)
}
