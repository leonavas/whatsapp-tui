//! The account button at the foot of the sidebar and its menu, as in slick:
//! your picture, name and connection, pausing notifications, your profile,
//! whatsapp-tui's settings, about, and unlinking this device.

use crate::app::{App, Phase};
use crate::bridge::Profile;
use crate::config::{self, Opt};
use crate::ui::{avatar_cells, dim, draw_avatar, phone, truncate, wrap, GREEN, LINK_IN, TEXT};
use chrono::{Datelike, Days, Local, TimeZone};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::{Margin, Position, Rect};
use ratatui::style::{Color, Modifier, Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Paragraph};
use ratatui::Frame;
use serde_json::json;
use unicode_width::UnicodeWidthStr;

/// Nerd Font bell with a slash: notifications paused.
const PAUSED: &str = "\u{f1f6}";
const MENU_W: u16 = 40;
/// Rows the button takes at the foot of the sidebar, its top line included.
pub const BUTTON_H: u16 = 3;
const REPO: &str = "github.com/leonavas/whatsapp-tui";
/// Under a menu row whose submenu is open.
const OPENED: Color = Color::Indexed(236);

fn version() -> String {
    format!("WhatsApp TUI v{}", env!("CARGO_PKG_VERSION"))
}

#[derive(Clone, Copy, PartialEq)]
enum Item {
    Sep,
    Notifications,
    Profile,
    Settings,
    About,
    Logout,
    // The notifications submenu.
    Resume,
    /// Minutes; 0 is "until tomorrow", 9 AM.
    Pause(i64),
    /// Until Monday, 9 AM.
    PauseWeek,
}

const MAIN: [Item; 7] = [Item::Notifications, Item::Sep, Item::Profile, Item::Settings, Item::Sep, Item::About, Item::Logout];

impl Item {
    fn in_sub(self) -> bool {
        matches!(self, Item::Resume | Item::Pause(_) | Item::PauseWeek)
    }
}

pub struct Popup {
    sel: Item,
    /// The notifications submenu, open, with its selected row.
    sub: Option<Item>,
    /// "Desconectar" was chosen once; choosing it again unlinks.
    confirm: bool,
}

#[derive(Default)]
pub struct Account {
    pub menu: Option<Popup>,
    /// The settings dialog, open, with its selected row in `config::opts()`.
    pub settings: Option<usize>,
    pub about: bool,
    /// "Perfil", open; what the bridge says about you once it answers.
    pub profile: Option<Option<Profile>>,
    /// "Desconectar" went out: the logout that follows is ours, not the phone's.
    pub signing_out: bool,
    // Where the last draw put things, for the mouse.
    button: Rect,
    menu_area: Rect,
    sub_area: Rect,
    rows: Vec<(Rect, Item)>,
    dialog: Rect,
    settings_rows: Vec<(Rect, usize)>,
    link: Rect,
}

impl Account {
    /// Something of it is on screen: it takes every key and click.
    fn open(&self) -> bool {
        self.menu.is_some() || self.settings.is_some() || self.about || self.profile.is_some()
    }
}

// ---- time ------------------------------------------------------------------

/// 9 AM `days` from today, local time.
fn nine_am(days: u64) -> i64 {
    let now = Local::now();
    let at = (now.date_naive() + Days::new(days)).and_hms_opt(9, 0, 0).and_then(|t| Local.from_local_datetime(&t).earliest());
    at.map_or(now.timestamp() + 12 * 3600, |t| t.timestamp())
}

/// "até 14:30", "até amanhã, 09:00", "até 17/10, 09:00".
fn until(ts: i64) -> String {
    let Some(t) = Local.timestamp_opt(ts, 0).single() else { return String::new() };
    let today = Local::now().date_naive();
    let hhmm = t.format("%H:%M");
    match t.date_naive() {
        d if d == today => format!("até {hhmm}"),
        d if today.succ_opt() == Some(d) => format!("até amanhã, {hhmm}"),
        _ => format!("até {}, {hhmm}", t.format("%d/%m")),
    }
}

// ---- state -----------------------------------------------------------------

fn step(items: &[Item], cur: Item, delta: isize) -> Item {
    let n = items.len() as isize;
    let mut i = items.iter().position(|x| *x == cur).unwrap_or(0) as isize;
    for _ in 0..n {
        i = (i + delta).rem_euclid(n);
        if items[i as usize] != Item::Sep {
            break;
        }
    }
    items[i as usize]
}

/// The notifications submenu's rows; those without an item are only text.
fn sub_rows(app: &App) -> Vec<(Option<Item>, String)> {
    let mut rows = Vec::new();
    // Paused: until when, and a way back, before pausing anew.
    if let Some(t) = app.events.paused {
        rows.push((None, format!("{PAUSED} Pausadas {}", until(t))));
        rows.push((Some(Item::Sep), String::new()));
        rows.push((Some(Item::Resume), "Retomar notificações".into()));
        rows.push((Some(Item::Sep), String::new()));
    }
    rows.push((None, "Pausar notificações por…".into()));
    let pauses = [
        (Item::Pause(30), "30 minutos"),
        (Item::Pause(60), "1 hora"),
        (Item::Pause(120), "2 horas"),
        (Item::Pause(0), "Até amanhã"),
        (Item::PauseWeek, "Até a próxima semana"),
    ];
    rows.extend(pauses.map(|(i, l)| (Some(i), l.to_string())));
    rows
}

fn sub_items(app: &App) -> Vec<Item> {
    sub_rows(app).into_iter().filter_map(|(i, _)| i).collect()
}

fn first_sub(app: &App) -> Option<Item> {
    sub_items(app).into_iter().find(|i| *i != Item::Sep)
}

/// Your WhatsApp name, else your number.
fn my_name(app: &App) -> String {
    if !app.me_name.is_empty() {
        return app.me_name.clone();
    }
    app.my_jid().map(|j| phone(&j)).filter(|p| !p.is_empty()).unwrap_or_else(|| "Você".into())
}

/// The line under your name: the connection, or the pause.
fn state(app: &App) -> String {
    let paused = app.events.paused.filter(|t| *t > Local::now().timestamp());
    match (app.phase == Phase::Ready, app.sync, paused) {
        (false, ..) => "conectando…".into(),
        (true, Some(p), _) => format!("sincronizando {p}%"),
        (true, None, Some(t)) => format!("{PAUSED} pausado {}", until(t)),
        _ => "conectado".into(),
    }
}

impl App {
    /// Your JID, once the bridge said who you are.
    pub fn my_jid(&self) -> Option<String> {
        (!self.me.is_empty()).then(|| format!("{}@s.whatsapp.net", self.me))
    }

    pub fn open_account_menu(&mut self) {
        // One popup at a time.
        self.attach.menu = None;
        self.emoji_panel.open = false;
        self.account.menu = Some(Popup { sel: Item::Notifications, sub: None, confirm: false });
    }

    fn choose(&mut self, item: Item) {
        let mut close = true;
        match item {
            Item::Sep => close = false,
            Item::Notifications => {
                close = false;
                let first = first_sub(self);
                if let Some(p) = self.account.menu.as_mut() {
                    (p.sel, p.sub) = (item, first);
                }
            }
            Item::Profile => self.open_my_profile(),
            Item::Settings => self.account.settings = Some(0),
            Item::About => self.account.about = true,
            Item::Logout => {
                if self.account.menu.as_ref().is_some_and(|p| p.confirm) {
                    self.sign_out();
                } else if let Some(p) = self.account.menu.as_mut() {
                    p.confirm = true;
                    close = false;
                }
            }
            Item::Resume => self.set_pause(None),
            Item::Pause(0) => self.set_pause(Some(nine_am(1))),
            Item::Pause(min) => self.set_pause(Some(Local::now().timestamp() + min * 60)),
            Item::PauseWeek => self.set_pause(Some(nine_am(7 - Local::now().weekday().num_days_from_monday() as u64))),
        }
        if close {
            self.account.menu = None;
        }
    }

    /// Unlinks this device: the bridge answers logged_out and exits, and the
    /// next one shows a QR code.
    fn sign_out(&mut self) {
        self.account.signing_out = true;
        self.cmd(json!({"c": "logout"}));
        self.note("Desconectando…");
    }

    fn open_my_profile(&mut self) {
        let Some(jid) = self.my_jid() else { return };
        self.account.profile = Some(None);
        self.cmd(json!({"c": "profile", "chat": jid}));
    }

    /// The bridge's answer about you, for "Perfil"; false when it's someone else's.
    pub(crate) fn on_my_profile(&mut self, p: &Profile) -> bool {
        let mine = self.my_jid().as_deref() == Some(p.chat.as_str()) && self.account.profile.is_some();
        if mine {
            self.account.profile = Some(Some(p.clone()));
        }
        mine
    }

    /// Flips a switch; the dialog stays open. The tray follows in main's loop.
    fn choose_opt(&mut self, o: Opt) {
        if let Err(e) = self.settings.flip(o) {
            self.note(format!("não salvou a configuração: {e}"));
        }
    }

    /// The menu and its dialogs take every key while open; Alt+S opens or
    /// closes the menu. True when taken.
    pub fn account_key(&mut self, k: KeyEvent) -> bool {
        let alt_s = k.modifiers.contains(KeyModifiers::ALT) && matches!(k.code, KeyCode::Char('s' | 'S'));
        if !self.account.open() {
            if alt_s {
                self.open_account_menu();
            }
            return alt_s;
        }
        if self.account.profile.is_some() {
            if matches!(k.code, KeyCode::Esc | KeyCode::Enter | KeyCode::Char('q')) {
                self.account.profile = None;
            }
        } else if self.account.about {
            match k.code {
                KeyCode::Enter => crate::links::open(&format!("https://{REPO}")),
                KeyCode::Esc | KeyCode::Char('q') => self.account.about = false,
                _ => {}
            }
        } else if let Some(sel) = self.account.settings {
            let all = config::opts();
            let n = all.len();
            match k.code {
                KeyCode::Esc | KeyCode::Char('q') => self.account.settings = None,
                KeyCode::Up | KeyCode::Char('k') | KeyCode::BackTab => self.account.settings = Some((sel + n - 1) % n),
                KeyCode::Down | KeyCode::Char('j') | KeyCode::Tab => self.account.settings = Some((sel + 1) % n),
                KeyCode::Enter | KeyCode::Char(' ') => self.choose_opt(all[sel]),
                _ => {}
            }
        } else {
            self.menu_key(k, alt_s);
        }
        true
    }

    fn menu_key(&mut self, k: KeyEvent, alt_s: bool) {
        let sub = sub_items(self);
        let Some(p) = self.account.menu.as_mut() else { return };
        let go = matches!(k.code, KeyCode::Enter | KeyCode::Right | KeyCode::Char('l' | ' '));
        if alt_s {
            self.account.menu = None;
            return;
        }
        match p.sub {
            Some(s) => match k.code {
                KeyCode::Up | KeyCode::Char('k') => p.sub = Some(step(&sub, s, -1)),
                KeyCode::Down | KeyCode::Char('j') => p.sub = Some(step(&sub, s, 1)),
                KeyCode::Esc | KeyCode::Left | KeyCode::Char('h') => p.sub = None,
                _ if go => self.choose(s),
                _ => {}
            },
            None => match k.code {
                KeyCode::Up | KeyCode::Char('k') => (p.sel, p.confirm) = (step(&MAIN, p.sel, -1), false),
                KeyCode::Down | KeyCode::Char('j') => (p.sel, p.confirm) = (step(&MAIN, p.sel, 1), false),
                KeyCode::Esc => self.account.menu = None,
                _ if go => {
                    let sel = p.sel;
                    self.choose(sel);
                }
                _ => {}
            },
        }
    }

    /// The button, and while open, the menu and dialogs: hovering, clicks,
    /// a click away closes. True when the event was theirs.
    pub fn account_mouse(&mut self, m: MouseEvent) -> bool {
        // Only over the chats: the QR and the error screens have no button.
        if self.phase != Phase::Ready && self.chats.is_empty() {
            return false;
        }
        let at = Position::new(m.column, m.row);
        let left = m.kind == MouseEventKind::Down(MouseButton::Left);
        let down = matches!(m.kind, MouseEventKind::Down(_));
        let a = &self.account;
        let outside = !a.dialog.contains(at);
        if a.profile.is_some() {
            if down && outside {
                self.account.profile = None;
            }
            return true;
        }
        if a.about {
            if left && a.link.contains(at) {
                crate::links::open(&format!("https://{REPO}"));
            } else if down && outside {
                self.account.about = false;
            }
            return true;
        }
        if a.settings.is_some() {
            let hit = a.settings_rows.iter().find(|(r, _)| r.contains(at)).map(|(_, i)| *i);
            match (m.kind, hit) {
                (MouseEventKind::Moved, Some(i)) => self.account.settings = Some(i),
                (_, Some(i)) if left => {
                    self.account.settings = Some(i);
                    self.choose_opt(config::opts()[i]);
                }
                _ if down && outside => self.account.settings = None,
                _ => {}
            }
            return true;
        }
        if a.menu.is_some() {
            let hit = a.rows.iter().find(|(r, _)| r.contains(at)).map(|(_, i)| *i);
            let inside = a.menu_area.contains(at) || a.sub_area.contains(at);
            let button = a.button.contains(at);
            let first = first_sub(self);
            let Some(p) = self.account.menu.as_mut() else { return true };
            match m.kind {
                MouseEventKind::Moved | MouseEventKind::Drag(_) => match hit {
                    Some(i) if i.in_sub() => p.sub = Some(i),
                    // As in slick, the submenu opens under the pointer.
                    Some(Item::Notifications) if p.sel != Item::Notifications || p.sub.is_none() => {
                        (p.sel, p.sub, p.confirm) = (Item::Notifications, first, false);
                    }
                    Some(i) if i != p.sel => (p.sel, p.sub, p.confirm) = (i, None, false),
                    _ => {}
                },
                _ if left && hit.is_some() => self.choose(hit.unwrap_or(Item::Sep)),
                _ if down && (button || !inside) => self.account.menu = None,
                _ => {}
            }
            return true;
        }
        if left && self.account.button.contains(at) {
            self.open_account_menu();
            return true;
        }
        false
    }
}

// ---- drawing ---------------------------------------------------------------

/// Your picture, name and the state under it.
pub fn button(f: &mut Frame, app: &mut App, area: Rect) {
    app.account.button = Rect::default();
    let block = Block::new().borders(Borders::TOP).border_style(dim());
    let inner = block.inner(area);
    f.render_widget(block, area);
    if inner.height < 2 || inner.width < 8 {
        return;
    }
    app.account.button = inner;
    let under = state(app);
    head(f, app, inner, &under);
}

/// The picture, then your name over `under`, from `area`'s top left.
fn head(f: &mut Frame, app: &mut App, area: Rect, under: &str) {
    let mut x = area.x + 1;
    if let (Some((cols, rows)), Some(jid)) = (avatar_cells(app), app.my_jid()) {
        draw_avatar(f, app, &jid, false, Rect::new(x, area.y, cols, rows));
        x += cols + 1;
    }
    let w = area.right().saturating_sub(x + 1);
    let name_style = if app.account.menu.is_some() { Style::new().fg(GREEN).bold() } else { Style::new().fg(TEXT).bold() };
    let lines = vec![
        Line::from(Span::styled(truncate(&my_name(app), w as usize), name_style)),
        Line::from(Span::styled(truncate(under, w as usize), dim())),
    ];
    f.render_widget(Paragraph::new(lines), Rect::new(x, area.y, w, 2));
}

pub fn draw(f: &mut Frame, app: &mut App) {
    let a = &mut app.account;
    (a.menu_area, a.sub_area, a.dialog, a.link) = Default::default();
    a.rows.clear();
    a.settings_rows.clear();
    if a.menu.is_some() {
        draw_menu(f, app);
    }
    if app.account.settings.is_some() {
        draw_settings(f, app);
    }
    if app.account.about {
        draw_about(f, app);
    }
    if app.account.profile.is_some() {
        draw_profile(f, app);
    }
}

/// ` left      right `, filling `w` cells.
fn padded(left: &str, right: &str, w: usize) -> String {
    let left = truncate(left, w.saturating_sub(right.width() + 3));
    let gap = w.saturating_sub(left.width() + right.width() + 2);
    format!(" {left}{}{right} ", " ".repeat(gap))
}

fn draw_menu(f: &mut Frame, app: &mut App) {
    let Some(p) = &app.account.menu else { return };
    let (sel, sub, confirm) = (p.sel, p.sub, p.confirm);
    let area = f.area();
    let b = app.account.button;
    // You, a gap and the items.
    let (w, h) = (MENU_W.min(area.width), (MAIN.len() as u16 + 5).min(area.height));
    // Over the button's top line, as if it grew out of it.
    let rect = Rect::new(b.x, b.y.saturating_sub(h).max(area.y), w, h).intersection(area);
    f.render_widget(Clear, rect);
    let block = Block::bordered().border_type(BorderType::Rounded).border_style(dim());
    let inner = block.inner(rect);
    f.render_widget(block, rect);
    app.account.menu_area = rect;
    if inner.height < 3 {
        return;
    }
    let under = state(app);
    head(f, app, inner, &under);

    let iw = inner.width as usize;
    let mut sub_y = inner.y;
    for (n, item) in MAIN.iter().enumerate() {
        let row = Rect::new(inner.x, inner.y + 3 + n as u16, inner.width, 1);
        if row.y >= inner.bottom() {
            break;
        }
        if *item == Item::Sep {
            f.render_widget(Paragraph::new("─".repeat(iw)).style(dim()), row);
            continue;
        }
        let mut style = if *item == sel { Style::new().add_modifier(Modifier::REVERSED) } else { Style::new() };
        let label = match item {
            Item::Notifications => {
                if *item == sel {
                    sub_y = row.y;
                    if sub.is_some() {
                        style = Style::new().bg(OPENED);
                    }
                }
                let right = if app.paused() { format!("{PAUSED} ›") } else { "›".into() };
                padded("Notificações", &right, iw)
            }
            Item::Profile => padded("Perfil", "", iw),
            Item::Settings => padded("Configurações", "", iw),
            Item::About => padded("Sobre o WhatsApp TUI", "", iw),
            Item::Logout if confirm => {
                style = style.fg(Color::Yellow).bold();
                padded("Desconectar? Enter confirma", "", iw)
            }
            _ => padded("Desconectar", "", iw),
        };
        f.render_widget(Paragraph::new(Line::from(label).style(style)), row);
        app.account.rows.push((row, *item));
    }
    if let Some(s) = sub {
        draw_sub(f, app, rect, sub_y, s);
    }
}

/// The notifications submenu, beside the menu.
fn draw_sub(f: &mut Frame, app: &mut App, main: Rect, y: u16, sel: Item) {
    let rows = sub_rows(app);
    let area = f.area();
    let w = (rows.iter().map(|(_, l)| l.width() + 4).max().unwrap_or(20) as u16).max(30).min(area.width);
    let h = (rows.len() as u16 + 2).min(area.height);
    let x = main.right().min(area.right().saturating_sub(w));
    let y = y.saturating_sub(1).min(area.bottom().saturating_sub(h));
    let rect = Rect::new(x, y, w, h);
    f.render_widget(Clear, rect);
    let block = Block::bordered().border_type(BorderType::Rounded).border_style(dim());
    let inner = block.inner(rect);
    f.render_widget(block, rect);
    app.account.sub_area = rect;
    let iw = inner.width as usize;
    let paused = app.paused();
    for (n, (item, label)) in rows.iter().enumerate() {
        let row = Rect::new(inner.x, inner.y + n as u16, inner.width, 1);
        if row.y >= inner.bottom() {
            break;
        }
        let line = match item {
            Some(Item::Sep) => Line::from("─".repeat(iw)).style(dim()),
            None => {
                let style = if paused && n == 0 { Style::new().fg(Color::Yellow) } else { dim() };
                Line::from(padded(label, "", iw)).style(style)
            }
            Some(i) => {
                app.account.rows.push((row, *i));
                let style = if *i == sel { Style::new().add_modifier(Modifier::REVERSED) } else { Style::new() };
                Line::from(padded(label, "", iw)).style(style)
            }
        };
        f.render_widget(Paragraph::new(line), row);
    }
}

/// A rounded box in the middle of the screen, cleared, with its title; it
/// and the space inside, a column in from each side.
fn dialog(f: &mut Frame, app: &mut App, w: u16, h: u16, title: &str) -> (Rect, Rect) {
    let area = f.area();
    let (w, h) = (w.min(area.width), h.min(area.height));
    let rect = Rect::new(area.x + (area.width - w) / 2, area.y + (area.height - h) / 2, w, h);
    f.render_widget(Clear, rect);
    let mut block = Block::bordered().border_type(BorderType::Rounded).border_style(dim());
    if !title.is_empty() {
        block = block.title(Line::from(title.to_string()).bold());
    }
    let inner = block.inner(rect).inner(Margin { horizontal: 1, vertical: 0 });
    f.render_widget(block, rect);
    app.account.dialog = rect;
    (rect, inner)
}

/// whatsapp-tui's settings: a heading over each group of `config::SECTIONS`, its switches under it.
fn draw_settings(f: &mut Frame, app: &mut App) {
    let Some(sel) = app.account.settings else { return };
    let mut lines: Vec<(Option<usize>, Line)> = vec![(None, Line::default())];
    let mut i = 0;
    for (n, (heading, opts)) in config::SECTIONS.iter().enumerate() {
        if n > 0 {
            lines.push((None, Line::default()));
        }
        lines.push((None, Line::from(format!(" {heading}")).style(Style::new().fg(GREEN).bold())));
        for o in *opts {
            let mark = if app.settings.on(*o) { "[✓]" } else { "[ ]" };
            lines.push((Some(i), Line::from(format!("{mark} {}", o.label))));
            i += 1;
        }
    }
    lines.push((None, Line::default()));
    lines.push((None, Line::from(" ↑↓ move · Enter escolhe · Esc fecha").style(dim())));
    lines.push((None, Line::from(format!(" {}", version())).style(dim())));

    let content_w = lines.iter().map(|(_, l)| l.width()).max().unwrap_or(0) as u16;
    let (_, inner) = dialog(f, app, (content_w + 8).max(44), lines.len() as u16 + 2, " Configurações ");
    for (n, (opt, line)) in lines.into_iter().enumerate() {
        let row = Rect::new(inner.x, inner.y + n as u16, inner.width, 1);
        if row.y >= inner.bottom() {
            break;
        }
        let Some(i) = opt else {
            f.render_widget(Paragraph::new(line), row);
            continue;
        };
        let mark = if i == sel { Span::styled("› ", Style::new().fg(GREEN).bold()) } else { Span::raw("  ") };
        let style = if i == sel { Style::new().add_modifier(Modifier::REVERSED) } else { Style::new() };
        let mut spans = vec![mark];
        spans.extend(line.spans.into_iter().map(|s| s.patch_style(style)));
        f.render_widget(Paragraph::new(Line::from(spans)), row);
        app.account.settings_rows.push((row, i));
    }
}

/// "Sobre o WhatsApp TUI": the name, the version and where the code is.
fn draw_about(f: &mut Frame, app: &mut App) {
    let hint = "Enter abre o link · Esc fecha";
    let lines = vec![
        Line::default(),
        Line::from("WhatsApp TUI").style(Style::new().fg(GREEN).bold()),
        Line::from(format!("versão {}", env!("CARGO_PKG_VERSION"))).style(dim()),
        Line::default(),
        Line::from(REPO).style(Style::new().fg(LINK_IN).underlined()),
        Line::default(),
        Line::from(hint).style(dim()),
    ];
    let w = REPO.width().max(hint.width()) as u16 + 8;
    let (_, inner) = dialog(f, app, w, lines.len() as u16 + 2, "");
    let link_y = inner.y + 4;
    f.render_widget(Paragraph::new(lines).centered(), inner);
    let lw = REPO.width() as u16;
    let link = Rect::new(inner.x + (inner.width / 2).saturating_sub(lw / 2), link_y, lw, 1).intersection(inner);
    app.account.link = link;
    // The terminal's own Ctrl+click too.
    crate::links::osc8(f.buffer_mut(), link, &format!("https://{REPO}"), |c| c.modifier.contains(Modifier::UNDERLINED));
}

/// "Perfil": your picture, name and number, and your recado.
fn draw_profile(f: &mut Frame, app: &mut App) {
    let Some(data) = app.account.profile.clone() else { return };
    let w = 52u16.min(f.area().width.saturating_sub(4));
    let iw = w.saturating_sub(4) as usize;
    let about = match &data {
        None => "carregando…".to_string(),
        Some(p) if !p.error.is_empty() => p.error.clone(),
        Some(p) if p.about.is_empty() => "—".into(),
        Some(p) => p.about.clone(),
    };
    let about = wrap(&about, iw.saturating_sub(2));
    // A gap, you (two rows), a gap, the heading, the recado, a gap, the hint.
    let h = 7 + about.len() as u16 + 2;
    let (_, inner) = dialog(f, app, w, h, " Perfil ");
    if inner.height < 4 {
        return;
    }
    // Your number where the button shows the connection.
    let number = app.my_jid().map(|j| phone(&j)).unwrap_or_default();
    head(f, app, Rect::new(inner.x.saturating_sub(1), inner.y + 1, inner.width + 1, 2), &number);
    let mut lines = vec![Line::from(Span::styled("Recado", Style::new().fg(GREEN).bold()))];
    lines.extend(about.into_iter().map(|l| Line::from(format!(" {l}"))));
    lines.push(Line::default());
    lines.push(Line::from("Esc fecha").style(dim()));
    f.render_widget(Paragraph::new(lines), Rect::new(inner.x, inner.y + 4, inner.width, inner.height.saturating_sub(4)));
}
