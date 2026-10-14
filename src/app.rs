//! State and input: what the bridge says, what the keys and mouse do.

use crate::bridge::{Bridge, Chat, Contact, Event, Msg, Number, Profile};
use crate::Ev;
use crossterm::event::{Event as TermEvent, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use image::DynamicImage;
use ratatui::layout::{Position, Rect};
use ratatui_image::picker::Picker;
use ratatui_image::protocol::Protocol;
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::sync::mpsc::Sender;
use std::time::{Duration, Instant};

#[derive(PartialEq)]
pub enum Phase {
    Starting,
    Qr(String),
    Connecting,
    Ready,
    LoggedOut,
    Dead(String),
}

/// The chips over the chat list.
#[derive(PartialEq, Clone, Copy)]
pub enum Filter {
    All,
    Unread,
    Favorites,
    Groups,
}

pub enum Avatar {
    Requested,
    Ready(DynamicImage),
    None,
}

/// A row of the sidebar: the chats, or while searching, what the search found.
#[derive(Clone)]
pub enum Item {
    /// "Arquivadas" at the top of the list, into the archived chats.
    Archived,
    /// "← Arquivadas" over the archived chats, back out.
    Back,
    Header(String),
    Info(String),
    Chat(Chat),
    Contact(Contact),
    Message(Msg),
    Number(Number),
}

impl Item {
    /// What the cursor holds on to; None for the rows it skips.
    pub fn key(&self) -> Option<String> {
        match self {
            Item::Header(_) | Item::Info(_) => None,
            Item::Archived => Some("a:".into()),
            Item::Back => Some("b:".into()),
            Item::Chat(c) => Some(format!("c:{}", c.jid)),
            Item::Contact(c) => Some(format!("p:{}", c.jid)),
            Item::Message(m) => Some(format!("m:{}:{}", m.chat, m.id)),
            Item::Number(n) => Some(format!("n:{}", n.jid)),
        }
    }

    pub fn height(&self) -> u16 {
        match self {
            Item::Info(_) => 1,
            // A line and a row of air: under a header, the air the chat below
            // pads its highlight with, as in the list.
            Item::Header(_) | Item::Archived | Item::Back => 2,
            // Two lines and a row of air, as WhatsApp Web spaces them.
            _ => 3,
        }
    }

    /// Where the cursor lands by itself: not on the rows that only lead in or out.
    fn lands(&self) -> bool {
        self.key().is_some() && !matches!(self, Item::Archived | Item::Back)
    }
}

/// An audio message as its bubble shows it.
pub struct Audio {
    pub secs: u32,
    /// A voice note: the sender's picture beside it, not a headphone.
    pub ptt: bool,
    /// Not yet found to be gone: a click downloads it (or asks the phone for it).
    pub playable: bool,
}

pub fn audio_of(m: &Msg) -> Option<Audio> {
    match &m.media {
        Some(md) if md.kind == "audio" => Some(Audio { secs: md.seconds, ptt: md.ptt, playable: !m.unavailable }),
        Some(_) => None,
        // Stored before the keys were kept: the text says what it was, and a
        // click asks the phone for the rest. Its text can't tell a voice note
        // from a file: the headphone until it's known.
        None => {
            let (mm, ss) = m.text.strip_prefix("🎤 Áudio ")?.split_once(':')?;
            Some(Audio { secs: mm.parse::<u32>().ok()? * 60 + ss.parse::<u32>().ok()?, ptt: false, playable: !m.unavailable })
        }
    }
}

/// A playable audio bubble on screen: its button and its track.
#[derive(Clone)]
pub struct AudioHit {
    pub play: Rect,
    pub track: Rect,
    pub chat: String,
    pub id: String,
    pub secs: u32,
}

/// A photo or a video's picture, read from disk off the UI thread.
pub enum Picture {
    Loading,
    Ready(DynamicImage),
    Failed,
}

/// A photo or video in a bubble on screen: a click opens it in the viewer.
#[derive(Clone)]
pub struct MediaHit {
    pub rect: Rect,
    pub chat: String,
    pub id: String,
    pub video: bool,
    pub gif: bool,
}

/// The search's answer from the bridge, for the text it was asked about.
pub struct Found {
    pub query: String,
    pub contacts: Vec<Contact>,
    pub messages: Vec<Msg>,
    pub number: Option<Number>,
}

/// The contact (or group) info panel: loading until `data` comes.
pub struct ProfileView {
    pub chat: String,
    pub data: Option<Profile>,
    pub photo: Option<DynamicImage>,
    /// The full-size picture's file: None until the bridge says, empty when
    /// there's none to show.
    pub pic: Option<String>,
    /// The picture open big, over everything.
    pub big: Option<crate::photo::View>,
    /// Where the picture was on the last draw, for a click.
    pub pic_area: Rect,
    pub scroll: usize,
    /// A business's whole week shown, not just today.
    pub week: bool,
}

#[derive(PartialEq, Clone, Copy)]
pub enum Focus {
    Sidebar,
    Search,
    Input,
}

pub struct App {
    pub phase: Phase,
    pub chats: Vec<Chat>,
    /// The sidebar's highlighted row (an `Item::key`); `open` is the chat on the right.
    pub cursor: Option<String>,
    pub open: Option<String>,
    pub msgs: HashMap<String, Vec<Msg>>,
    pub input: String,
    /// In chars.
    pub caret: usize,
    pub focus: Focus,
    pub search: String,
    pub filter: Filter,
    /// The sidebar shows the archived chats.
    pub archived: bool,
    /// The chat menu (archive, pin, mute…).
    pub chat_menu: crate::chatmenu::ChatMenu,
    /// A message's menu, the messages picked to delete, the one being edited.
    pub msg_menu: crate::msgmenu::MsgMenu,
    pub found: Option<Found>,
    /// When to ask the bridge about the search text (after a pause in typing).
    search_due: Option<Instant>,
    /// Names for chats opened from the search that have no messages yet.
    pub names: HashMap<String, String>,
    /// A message to scroll to once its chat is on screen.
    pub jump: Option<String>,
    pub profile: Option<ProfileView>,
    /// Lines up from the newest message.
    pub scroll: usize,
    pub side_offset: usize,
    /// Keep the cursor on screen (off while the wheel scrolls the sidebar).
    pub follow: bool,
    pub typing: HashMap<String, (String, Instant)>,
    pub sync: Option<u32>,
    pub notice: Option<(String, Instant)>,
    pub quit: bool,
    // Where things were on the last draw, for the mouse.
    pub side_list: Rect,
    pub msgs_area: Rect,
    pub input_area: Rect,
    /// The message box's text width on the last draw, for Up and Down.
    pub input_width: usize,
    /// The clip and smiley at the start of the message box.
    pub attach_btn: Rect,
    pub emoji_btn: Rect,
    pub chips: Vec<(Rect, Filter)>,
    pub search_area: Rect,
    /// The sidebar's rows on screen, by key.
    pub side_hits: Vec<(Rect, String)>,
    pub header_area: Rect,
    /// The round button down to the newest message, while scrolled up.
    pub newest_btn: Rect,
    /// A message just gone to from its quote, lit behind for a moment.
    pub flash: Option<(String, Instant)>,
    /// The bubbles on screen, from the last draw.
    pub bubbles: Vec<crate::react::Hit>,
    /// The message the pointer's over: its quick reactions show.
    pub hover_msg: Option<String>,
    /// Those quick reactions, from the last draw.
    pub react_btn: Option<crate::react::Quick>,
    /// The reactions' bar, open over a message.
    pub react_bar: Option<crate::react::Bar>,
    /// Who reacted to a message, over its reactions.
    pub react_who: Option<crate::react::Who>,
    pub panel_area: Rect,
    pub panel_close: Rect,
    /// Group members in the panel, to open a chat with.
    pub panel_hits: Vec<(Rect, String)>,
    /// In a group, the senders' names and pictures by their bubbles: a click opens their info.
    pub sender_hits: Vec<(Rect, String)>,
    /// A business's hours row, a click shows the week.
    pub panel_week: Rect,
    /// A business's address and its Google Maps link, a click opens it.
    pub panel_maps: Rect,
    /// Links in the messages on screen, a piece per row; a click opens it.
    pub msg_links: Vec<(Rect, String)>,
    /// Long messages opened past their "Ler mais", by (chat, id).
    pub expanded: HashSet<(String, String)>,
    /// The "Ler mais" on screen: where, its message and how many rows it grows by.
    pub more_hits: Vec<(Rect, String, usize)>,
    /// Text selected in the conversation with the mouse.
    pub text_sel: crate::select::TextSel,
    pub audio_hits: Vec<AudioHit>,
    pub player: crate::player::Player,
    /// Photos and videos on screen.
    pub media_hits: Vec<MediaHit>,
    pub video: crate::video::Video,
    /// The photo or video open big, over everything.
    pub viewer: Option<crate::viewer::View>,
    /// A video played inside the terminal is gone: everything is drawn again.
    pub repaint: bool,
    /// Photos and videos' pictures by path, read once.
    pub pictures: HashMap<String, Picture>,
    /// Photos asked of the bridge as they showed, once each.
    fetched: HashSet<(String, String)>,
    /// A photo or video opened outside the app once it's downloaded.
    pub(crate) open_when_ready: Option<(String, String)>,
    /// Downloading to save to the downloads folder; true to show it there after.
    pub(crate) save_when_ready: Option<(String, String, bool)>,
    /// Where each message's media was saved, by (chat, id).
    pub(crate) saved: HashMap<(String, String), std::path::PathBuf>,
    /// Real pictures (kitty, sixel, iTerm2); None in terminals without them.
    pub picker: Option<Picker>,
    pub avatars: HashMap<String, Avatar>,
    /// Encoded for the terminal, per chat and size in cells.
    pub protocols: HashMap<(String, u16, u16), Protocol>,
    /// A resize or the window coming back: the cell's pixels may have changed (zoom).
    pub recheck_cell: bool,
    /// The `:` emoji autocomplete over the message box.
    pub emoji: crate::emoji::Popup,
    /// The emoji panel the smiley opens.
    pub emoji_panel: crate::emoji_panel::Panel,
    /// Files attached to the message box, and the clip's menu.
    pub attach: crate::attach::Attach,
    /// What's typed in the chats not open.
    pub drafts: crate::drafts::Drafts,
    /// The snapshot and events for other programs (`events`).
    pub events: crate::events::State,
    /// The terminal has the keyboard: only then does the open chat count as read.
    pub focused: bool,
    /// Your WhatsApp name.
    pub me_name: String,
    /// Your number, the user part of your JID; empty until the bridge says.
    pub me: String,
    /// The account button's menu and dialogs.
    pub account: crate::account::Account,
    /// The settings dialog's switches.
    pub settings: crate::config::Settings,
    /// The column between the list and the conversation, dragged to resize the list.
    pub divider: crate::divider::Divider,
    bridge: Option<Bridge>,
    pub(crate) tx: Sender<Ev>,
}

impl App {
    pub fn new(tx: Sender<Ev>, picker: Option<Picker>, owner: bool) -> App {
        let mut app = App {
            phase: Phase::Starting,
            chats: Vec::new(),
            cursor: None,
            open: None,
            msgs: HashMap::new(),
            input: String::new(),
            caret: 0,
            focus: Focus::Sidebar,
            search: String::new(),
            filter: Filter::All,
            archived: false,
            chat_menu: Default::default(),
            msg_menu: Default::default(),
            found: None,
            search_due: None,
            names: HashMap::new(),
            jump: None,
            profile: None,
            scroll: 0,
            side_offset: 0,
            follow: true,
            typing: HashMap::new(),
            sync: None,
            notice: None,
            quit: false,
            side_list: Rect::default(),
            msgs_area: Rect::default(),
            input_area: Rect::default(),
            input_width: 40,
            attach_btn: Rect::default(),
            emoji_btn: Rect::default(),
            chips: Vec::new(),
            search_area: Rect::default(),
            side_hits: Vec::new(),
            header_area: Rect::default(),
            panel_area: Rect::default(),
            panel_close: Rect::default(),
            panel_hits: Vec::new(),
            sender_hits: Vec::new(),
            panel_week: Rect::default(),
            panel_maps: Rect::default(),
            msg_links: Vec::new(),
            expanded: HashSet::new(),
            more_hits: Vec::new(),
            text_sel: Default::default(),
            newest_btn: Rect::default(),
            flash: None,
            bubbles: Vec::new(),
            hover_msg: None,
            react_btn: None,
            react_bar: None,
            react_who: None,
            audio_hits: Vec::new(),
            player: crate::player::Player::new(tx.clone()),
            media_hits: Vec::new(),
            video: crate::video::Video::new(tx.clone()),
            viewer: None,
            repaint: false,
            pictures: HashMap::new(),
            fetched: HashSet::new(),
            open_when_ready: None,
            save_when_ready: None,
            saved: HashMap::new(),
            picker,
            avatars: HashMap::new(),
            protocols: HashMap::new(),
            recheck_cell: false,
            emoji: Default::default(),
            emoji_panel: crate::emoji_panel::Panel::load(),
            attach: Default::default(),
            drafts: Default::default(),
            events: crate::events::State::new(owner),
            focused: false,
            me_name: String::new(),
            me: String::new(),
            account: Default::default(),
            settings: crate::config::Settings::load(),
            divider: crate::divider::Divider::load(),
            bridge: None,
            tx,
        };
        app.start();
        app.load_drafts();
        app
    }

    fn start(&mut self) {
        self.bridge = None;
        match Bridge::spawn(self.tx.clone()) {
            Ok(b) => {
                self.bridge = Some(b);
                self.phase = Phase::Starting;
            }
            Err(e) => self.phase = Phase::Dead(e),
        }
    }

    pub(crate) fn cmd(&mut self, v: Value) {
        if let Some(b) = &mut self.bridge {
            b.send(v);
        }
    }

    pub(crate) fn note(&mut self, s: impl Into<String>) {
        self.notice = Some((s.into(), Instant::now()));
    }

    /// The terminal's cell size changed (zoom): a new picker for it, and
    /// what was encoded for the old size is dropped, to be encoded again as
    /// it's drawn.
    pub fn set_font_size(&mut self, w: u16, h: u16) {
        let Some(old) = self.picker.as_ref() else { return };
        // No setter in ratatui-image: a new picker, same protocol.
        #[allow(deprecated)]
        let mut p = Picker::from_fontsize(ratatui_image::FontSize::new(w, h));
        p.set_protocol_type(old.protocol_type());
        self.picker = Some(p);
        self.protocols.clear();
    }

    pub fn tick(&mut self) {
        self.select_tick();
        self.typing.retain(|_, (_, at)| at.elapsed() < Duration::from_secs(6));
        if self.notice.as_ref().is_some_and(|(_, at)| at.elapsed() > Duration::from_secs(6)) {
            self.notice = None;
        }
        if self.search_due.is_some_and(|t| Instant::now() >= t) {
            self.search_due = None;
            let q = self.search.trim().to_string();
            if !q.is_empty() {
                self.cmd(json!({"c": "search", "text": q}));
            }
        }
        self.drafts_tick();
        self.video.tick();
    }

    /// The name to show for a chat, also one opened from the search.
    pub fn name_of(&self, jid: &str) -> Option<String> {
        self.chat(jid).map(|c| c.name.clone()).or_else(|| self.names.get(jid).cloned())
    }

    /// Asks the bridge for a chat's picture, once.
    pub fn want_avatar(&mut self, jid: &str) {
        if !self.avatars.contains_key(jid) {
            self.avatars.insert(jid.to_string(), Avatar::Requested);
            self.cmd(json!({"c": "avatar", "chat": jid}));
        }
    }

    pub fn chat(&self, jid: &str) -> Option<&Chat> {
        self.chats.iter().find(|c| c.jid == jid)
    }

    /// The chats the sidebar shows: the chip's and the search's matches.
    /// Archived ones only in their own list, or last in a search, as on
    /// WhatsApp Web. The open chat stays while it's read.
    pub fn visible(&self) -> Vec<&Chat> {
        let words: Vec<String> = self.search.to_lowercase().split_whitespace().map(String::from).collect();
        let mut v: Vec<&Chat> = self
            .chats
            .iter()
            .filter(|c| if self.archived { c.archived } else { !c.archived || !words.is_empty() })
            .filter(|c| match self.filter {
                Filter::All => true,
                Filter::Unread => c.unread > 0 || self.open.as_ref() == Some(&c.jid),
                Filter::Favorites => c.favorite,
                Filter::Groups => c.group,
            })
            .filter(|c| {
                let n = c.name.to_lowercase();
                words.iter().all(|w| n.contains(w.as_str()))
            })
            .collect();
        v.sort_by_key(|c| c.archived);
        v
    }

    /// The sidebar's rows: the chats, or while searching, the chats that
    /// match, the contacts and the messages the bridge found, and a number
    /// to start a chat with.
    pub fn items(&self) -> Vec<Item> {
        let chats = self.visible().into_iter().cloned().map(Item::Chat);
        let mut v = Vec::new();
        if self.archived {
            v.push(Item::Back);
        }
        if self.search.trim().is_empty() {
            if !self.archived && self.filter == Filter::All && self.chats.iter().any(|c| c.archived) {
                v.push(Item::Archived);
            }
            v.extend(chats);
            if self.archived && v.len() == 1 {
                v.push(Item::Info("Nenhuma conversa arquivada".into()));
            }
            return v;
        }
        let found = self.found.as_ref().filter(|f| f.query == self.search.trim());
        if let Some(n) = found.and_then(|f| f.number.clone()) {
            v.push(Item::Number(n));
        }
        let chats: Vec<Item> = chats.collect();
        if !chats.is_empty() {
            v.push(Item::Header("Conversas".into()));
            v.extend(chats);
        }
        match found {
            Some(f) => {
                if !f.contacts.is_empty() {
                    v.push(Item::Header("Contatos".into()));
                    v.extend(f.contacts.iter().cloned().map(Item::Contact));
                }
                if !f.messages.is_empty() {
                    v.push(Item::Header("Mensagens".into()));
                    v.extend(f.messages.iter().cloned().map(Item::Message));
                }
                if v.iter().all(|i| matches!(i, Item::Back)) {
                    v.push(Item::Info("Nenhuma conversa, contato ou mensagem encontrada".into()));
                }
            }
            None => v.push(Item::Info("Pesquisando…".into())),
        }
        v
    }

    /// The cursor's row, or the first one it can be on.
    pub fn cursor_index(&self, items: &[Item]) -> usize {
        let at = self.cursor.as_ref().and_then(|k| items.iter().position(|i| i.key().as_ref() == Some(k)));
        at.or_else(|| items.iter().position(Item::lands)).or_else(|| items.iter().position(|i| i.key().is_some())).unwrap_or(0)
    }

    fn move_cursor(&mut self, delta: isize) {
        let items = self.items();
        let keyed: Vec<&Item> = items.iter().filter(|i| i.key().is_some()).collect();
        let keys: Vec<String> = keyed.iter().filter_map(|i| i.key()).collect();
        if keys.is_empty() {
            return;
        }
        // From nothing lit, the first step lands on the first chat.
        let i = match self.cursor.as_ref().and_then(|k| keys.iter().position(|x| x == k)) {
            Some(cur) => (cur as isize + delta).clamp(0, keys.len() as isize - 1) as usize,
            None => keyed.iter().position(|i| i.lands()).unwrap_or(0),
        };
        self.cursor = Some(keys[i].clone());
        self.follow = true;
    }

    pub(crate) fn current(&self) -> Option<Item> {
        let items = self.items();
        let i = self.cursor_index(&items);
        items.into_iter().nth(i).filter(|i| i.key().is_some())
    }

    /// Enter on a row: open the chat (from a message, scrolled to it; from a
    /// contact or a number, a new one).
    fn activate(&mut self, item: Item) {
        match item {
            Item::Header(_) | Item::Info(_) => return,
            Item::Archived => return self.show_archived(true),
            Item::Back => return self.show_archived(false),
            Item::Chat(c) => self.open_chat(c.jid),
            Item::Message(m) => {
                self.jump = Some(m.id.clone());
                // What was found may be past its "Ler mais".
                self.expanded.insert((m.chat.clone(), m.id.clone()));
                self.open_chat(m.chat);
            }
            Item::Contact(c) => {
                self.names.insert(c.jid.clone(), c.name);
                self.open_chat(c.jid);
            }
            Item::Number(n) if !n.exists => return self.note("Esse número não está no WhatsApp."),
            Item::Number(n) => {
                self.names.entry(n.jid.clone()).or_insert_with(|| format!("+{}", n.phone));
                self.open_chat(n.jid);
            }
        }
        self.search.clear();
        self.found = None;
        self.focus_input();
    }

    /// Into the archived chats, or back to the list (onto "Arquivadas").
    fn show_archived(&mut self, on: bool) {
        self.archived = on;
        self.cursor_to_first();
        if !on {
            self.cursor = Some("a:".into());
        }
    }

    fn open_chat(&mut self, jid: String) {
        if self.open.as_ref() != Some(&jid) {
            self.scroll = 0;
            self.profile = None;
            self.switch_draft(&jid);
        }
        self.cmd(json!({"c": "history", "chat": jid, "limit": 400}));
        // Opened from outside while the window is elsewhere: read once it's seen.
        if self.focused {
            self.mark_read(&jid);
        }
        self.cursor = Some(format!("c:{jid}"));
        self.open = Some(jid);
        self.follow = true;
    }

    /// Ctrl+N / Ctrl+P: the next or previous chat, opened.
    fn step_open(&mut self, delta: isize) {
        self.move_cursor(delta);
        if let Some(Item::Chat(c)) = self.current() {
            self.open_chat(c.jid);
        }
    }

    /// Opens (or closes) the info panel for the open chat.
    fn toggle_profile(&mut self) {
        let Some(chat) = self.open.clone() else { return };
        if self.profile.as_ref().is_some_and(|p| p.chat == chat) {
            self.profile = None;
            return;
        }
        self.show_profile(chat);
    }

    /// The info panel for `jid`: the open chat, or a group member's.
    pub(crate) fn show_profile(&mut self, jid: String) {
        if self.profile.as_ref().is_some_and(|p| p.chat == jid) {
            return;
        }
        self.profile = Some(ProfileView { chat: jid.clone(), data: None, photo: None, pic: None, big: None, pic_area: Rect::default(), scroll: 0, week: false });
        self.cmd(json!({"c": "profile", "chat": jid}));
    }

    /// The terminal got or lost the keyboard. Coming back reads what arrived
    /// in the open chat meanwhile.
    pub fn set_focused(&mut self, focused: bool) {
        if self.focused == focused {
            return;
        }
        self.focused = focused;
        if let Some(j) = self.open.clone().filter(|_| focused) {
            self.mark_read(&j);
        }
    }

    fn mark_read(&mut self, jid: &str) {
        if let Some(c) = self.chats.iter_mut().find(|c| c.jid == jid && c.unread > 0) {
            c.unread = 0;
            self.send_read(jid);
        }
    }

    /// Read here; the sender hears of it only with read receipts on.
    fn send_read(&mut self, jid: &str) {
        let quiet = !self.settings.on(crate::config::RECEIPTS);
        self.cmd(json!({"c": "read", "chat": jid, "quiet": quiet}));
    }

    /// A command from another process (`ipc`).
    fn on_remote(&mut self, cmd: crate::ipc::Cmd) {
        use crate::ipc::Cmd;
        match cmd {
            Cmd::Focus => crate::tray::focus(),
            Cmd::Open { chat, id, focus } => {
                if focus {
                    crate::tray::focus();
                }
                self.emoji_panel.open = false;
                self.search.clear();
                self.found = None;
                self.jump = id;
                self.open_chat(chat);
                self.focus_input();
            }
            Cmd::MarkRead(chat) => self.mark_read(&chat),
            Cmd::Pause(until) => self.set_pause(Some(until)),
            Cmd::Resume => self.set_pause(None),
            Cmd::Quit => self.quit = true,
        }
    }

    pub fn on(&mut self, ev: Ev) {
        // Typing or clicking means the window is seen, also in terminals
        // that never report focus.
        // A pointer passing over doesn't count.
        let seen = match &ev {
            Ev::Term(TermEvent::Mouse(m)) => m.kind != MouseEventKind::Moved,
            Ev::Term(TermEvent::Key(_) | TermEvent::Paste(_)) => true,
            _ => false,
        };
        if seen {
            self.set_focused(true);
        }
        match ev {
            Ev::Remote(cmd) => self.on_remote(cmd),
            Ev::Update(note) => self.on_update(note),
            Ev::Term(TermEvent::FocusGained) => {
                self.recheck_cell = true;
                self.set_focused(true)
            }
            Ev::Term(TermEvent::Resize(..)) => {
                // Its place on screen moves: the video stops.
                self.video.stop();
                self.recheck_cell = true
            }
            Ev::Term(TermEvent::FocusLost) => self.set_focused(false),
            Ev::Bridge(e) => self.on_bridge(e),
            Ev::Attach(e) => self.on_attach(e),
            Ev::Player(e) => self.player.on(e),
            Ev::Video(e) => {
                if self.video.on(e) {
                    self.repaint = true;
                    // Over: the viewer shows its picture again, to play once more.
                    if let Some(v) = self.viewer.as_mut().filter(|v| v.state == crate::viewer::State::Playing) {
                        v.state = crate::viewer::State::Idle;
                    }
                }
            }
            Ev::Picture(path, img) => {
                self.pictures.insert(path, img.map_or(Picture::Failed, Picture::Ready));
            }
            // An empty paste: the clipboard holds a picture or files.
            Ev::Term(TermEvent::Paste(s)) if self.focus == Focus::Input && s.is_empty() => self.paste_clipboard(),
            Ev::Term(TermEvent::Key(k)) => self.key(k),
            Ev::Term(TermEvent::Mouse(m)) => self.mouse(m),
            Ev::Term(TermEvent::Paste(s)) if self.focus == Focus::Input => self.insert(&s),
            Ev::Term(_) => {}
        }
    }

    fn on_bridge(&mut self, e: Event) {
        match e {
            Event::Qr { code } => {
                self.account.signing_out = false;
                // No session: whatever is still here was the last account's.
                self.forget_account();
                self.phase = Phase::Qr(code);
            }
            Event::Paired => self.phase = Phase::Connecting,
            Event::Connecting { name, me } => {
                // Kept when the session doesn't have it (yet).
                if !name.is_empty() {
                    self.me_name = name;
                }
                self.me = me;
                if self.phase == Phase::Starting {
                    self.phase = Phase::Connecting
                }
            }
            Event::Ready { name, me } => {
                if !name.is_empty() {
                    self.me_name = name;
                }
                if !me.is_empty() {
                    self.me = me;
                }
                self.phase = Phase::Ready
            }
            Event::LoggedOut => {
                self.phase = Phase::LoggedOut;
                self.forget_account();
                (self.me, self.me_name) = Default::default();
                // Only whether it was ours stays, for the screen that follows.
                let ours = self.account.signing_out;
                self.account = Default::default();
                self.account.signing_out = ours;
            }
            Event::Exited => {
                if self.phase == Phase::LoggedOut {
                    self.start();
                } else if !matches!(self.phase, Phase::Dead(_)) {
                    let log = crate::bridge::data_dir().join("bridge.log");
                    self.phase = Phase::Dead(format!("O bridge parou. Log em {}", log.display()));
                }
            }
            Event::Sync { progress } => {
                self.sync = (progress < 100).then_some(progress);
                if let Some(j) = self.open.clone() {
                    self.cmd(json!({"c": "history", "chat": j, "limit": 400}));
                }
            }
            Event::Chats { chats } => {
                self.chats = chats;
                self.events.chats_loaded = true;
                // Counted by the bridge while it was already open here.
                if let Some(j) = self.open.clone().filter(|_| self.focused) {
                    if let Some(c) = self.chats.iter_mut().find(|c| c.jid == j && c.unread > 0) {
                        c.unread = 0;
                        self.send_read(&j);
                    }
                }
            }
            Event::History { chat, msgs } => {
                self.msgs.insert(chat, msgs);
            }
            Event::Message { msg, new } => {
                if new {
                    self.announce(&msg);
                }
                self.upsert(msg, new);
            }
            Event::Unread { chat, msgs } => self.on_unread(chat, msgs),
            Event::Status { chat, ids, status } => {
                if let Some(list) = self.msgs.get_mut(&chat) {
                    for m in list.iter_mut().filter(|m| m.from_me && ids.contains(&m.id)) {
                        m.status = m.status.max(status);
                    }
                    // The sidebar's ticks follow the last message.
                    if let (Some(last), Some(c)) = (list.last(), self.chats.iter_mut().find(|c| c.jid == chat)) {
                        if last.from_me && ids.contains(&last.id) {
                            c.status = c.status.max(status);
                        }
                    }
                }
            }
            Event::Cleared { chat, deleted } => {
                if deleted && self.open.as_ref() == Some(&chat) {
                    self.close_chat();
                }
                // Gone from the cache; open, what's left of it comes again.
                self.msgs.remove(&chat);
                if self.open.as_ref() == Some(&chat) {
                    self.cmd(json!({"c": "history", "chat": chat, "limit": 400}));
                }
            }
            Event::Removed { chat, ids } => self.removed(&chat, &ids),
            Event::Read { chat } => {
                if let Some(c) = self.chats.iter_mut().find(|c| c.jid == chat) {
                    c.unread = 0;
                }
            }
            Event::Typing { chat, who, on } => {
                if on {
                    self.typing.insert(chat, (who, Instant::now()));
                } else {
                    self.typing.remove(&chat);
                }
            }
            Event::Search { query, contacts, messages, number } => {
                if query == self.search.trim() {
                    self.found = Some(Found { query, contacts, messages, number });
                }
            }
            Event::Profile(p) => {
                if self.on_my_profile(&p) {
                    return;
                }
                if let Some(view) = self.profile.as_mut().filter(|v| v.chat == p.chat) {
                    view.data = Some(p);
                }
            }
            Event::Photo { chat, path, error } => {
                let Some(view) = self.profile.as_mut().filter(|v| v.chat == chat) else { return };
                view.photo = (!path.is_empty()).then(|| image::open(&path).ok()).flatten();
                view.pic = Some(path.clone());
                self.protocols.retain(|(k, _, _), _| !k.starts_with("big:") && !k.starts_with("photo:"));
                self.photo_arrived(&path, &error);
            }
            Event::Avatar { chat, path } => {
                let img = (!path.is_empty()).then(|| image::open(&path).ok()).flatten();
                self.avatars.insert(chat.clone(), img.map_or(Avatar::None, Avatar::Ready));
                // Its picture, and with an unread count over it (`badge`).
                let counted = format!("badge:{chat}:");
                self.protocols.retain(|(j, _, _), _| j != &chat && !j.starts_with(&counted));
            }
            Event::Media { chat, id, path, error } => {
                if !error.is_empty() {
                    if let Some(m) = self.msgs.get_mut(&chat).and_then(|l| l.iter_mut().find(|m| m.id == id)) {
                        m.unavailable = true;
                    }
                    if self.player.of(&chat, &id).is_some() {
                        self.player.stop();
                        self.note(error.clone());
                    }
                } else if let Err(e) = self.player.ready(&chat, &id, &path) {
                    self.note(e);
                }
                self.media_arrived(&chat, &id, &path, &error);
            }
            Event::Thumb { chat, id, path } => {
                if let Some(md) = self.msgs.get_mut(&chat).and_then(|l| l.iter_mut().find(|m| m.id == id)).and_then(|m| m.media.as_mut()) {
                    md.thumb = path;
                }
            }
            Event::StickerFav { chat, id, fav } => self.viewer_fav_state(&chat, &id, fav),
            Event::ChatSubs { subs } => self.msg_menu.fwd.subs = subs,
            Event::Error { text, fatal } => {
                if fatal {
                    self.phase = Phase::Dead(text);
                } else {
                    // A logout that didn't go through leaves you linked.
                    self.account.signing_out = false;
                    self.note(text);
                }
            }
            Event::Other => {}
        }
    }

    /// A new message, or a new version of one (sent → delivered, edited, deleted).
    fn upsert(&mut self, m: Msg, new: bool) {
        // Read as it arrives only while it's in front of you.
        let seen = self.focused && self.open.as_ref() == Some(&m.chat);
        let mut existed = false;
        if let Some(list) = self.msgs.get_mut(&m.chat) {
            if let Some(old) = list.iter_mut().find(|x| x.id == m.id) {
                let status = old.status.max(m.status);
                *old = m.clone();
                old.status = status;
                existed = true;
            } else {
                list.push(m.clone());
                if list.len() > 1 && list[list.len() - 2].ts > m.ts {
                    list.sort_by_key(|x| x.ts);
                }
            }
        }
        if !m.from_me {
            self.typing.remove(&m.chat);
        }
        match self.chats.iter_mut().find(|c| c.jid == m.chat) {
            Some(c) => {
                if m.ts >= c.last_ts {
                    c.last_ts = m.ts;
                    // As the bridge's list has it: names for the @s, and in a group who wrote it.
                    let text = crate::markup::mentions(&m.text, &m.mentions).0;
                    c.preview = if c.group && !m.from_me && !text.is_empty() { format!("{}: {text}", m.sender_name) } else { text };
                    c.from_me = m.from_me;
                    c.status = m.status;
                }
                if new && !existed && !seen {
                    c.unread += 1;
                }
            }
            None => {
                let group = m.chat.ends_with("@g.us");
                self.chats.push(Chat {
                    jid: m.chat.clone(),
                    name: if group { "Grupo".into() } else { m.sender_name.clone() },
                    group,
                    last_ts: m.ts,
                    unread: u32::from(new && !seen),
                    marked: false,
                    pinned: false,
                    pin_ts: 0,
                    archived: false,
                    muted: false,
                    favorite: false,
                    preview: m.text.clone(),
                    from_me: m.from_me,
                    status: m.status,
                    read_only: String::new(),
                    verified: false,
                });
            }
        }
        // As the bridge sorts them: pinned, the last pinned first, then by the newest message.
        self.chats.sort_by(|a, b| b.pin_ts.cmp(&a.pin_ts).then(b.last_ts.cmp(&a.last_ts)));
        if seen && new && !existed {
            if let Some(c) = self.chats.iter_mut().find(|c| c.jid == m.chat) {
                c.unread = 0;
            }
            self.send_read(&m.chat);
        }
    }

    /// A click on an audio's button (`at` None) or on its track, `at` seconds in.
    fn play(&mut self, chat: &str, id: &str, secs: u32, at: Option<f64>) {
        match (self.player.of(chat, id).is_some(), at) {
            (true, Some(t)) => self.player.seek(t),
            (true, None) => self.player.toggle(),
            (false, at) => {
                self.player.load(chat, id, secs as f64, at.unwrap_or(0.0));
                self.cmd(json!({"c": "download", "chat": chat, "id": id}));
            }
        }
    }

    /// A photo or video downloaded (or not): it shows, opens or plays.
    fn media_arrived(&mut self, chat: &str, id: &str, path: &str, error: &str) {
        if error.is_empty() {
            if let Some(md) = self.msgs.get_mut(chat).and_then(|l| l.iter_mut().find(|m| m.id == id)).and_then(|m| m.media.as_mut()) {
                md.file = path.to_string();
            }
        }
        let key = (chat.to_string(), id.to_string());
        if self.open_when_ready.as_ref() == Some(&key) {
            self.open_when_ready = None;
            if error.is_empty() {
                self.open_outside(chat, id, path);
            } else {
                self.note(error);
            }
        }
        if let Some((_, _, reveal)) = self.save_when_ready.clone().filter(|(c, i, _)| c == chat && i == id) {
            self.save_when_ready = None;
            if error.is_empty() {
                self.save_media(chat, id, reveal);
            } else {
                self.note(error);
            }
        }
        self.viewer_media(chat, id, error);
    }

    /// Opens it outside the app: a video in mpv's window, a photo in the
    /// image viewer.
    pub(crate) fn open_outside(&mut self, chat: &str, id: &str, path: &str) {
        let md = self.msgs.get(chat).and_then(|l| l.iter().find(|m| m.id == id)).and_then(|m| m.media.as_ref());
        match md.filter(|md| md.kind == "video") {
            Some(md) => {
                if let Err(e) = crate::video::external(path, md.gif) {
                    self.note(e);
                }
            }
            None => crate::links::open(path),
        }
    }

    /// Downloading to open outside the app.
    pub(crate) fn opening(&self, chat: &str, id: &str) -> bool {
        self.open_when_ready.as_ref().is_some_and(|(c, i)| c == chat && i == id)
    }

    /// The message's file, when it's on disk.
    pub(crate) fn media_file(&self, chat: &str, id: &str) -> Option<String> {
        let md = self.msgs.get(chat).and_then(|l| l.iter().find(|m| m.id == id)).and_then(|m| m.media.as_ref())?;
        Some(md.file.clone()).filter(|f| !f.is_empty() && std::path::Path::new(f).exists())
    }

    /// Videos play in the viewer where the terminal speaks kitty's
    /// graphics and the setting's on; elsewhere in mpv's window.
    pub(crate) fn plays_inside(&self) -> bool {
        let kitty = self.picker.as_ref().is_some_and(|p| p.protocol_type() == ratatui_image::picker::ProtocolType::Kitty);
        kitty && self.settings.on(crate::config::VIDEO_INLINE)
    }

    /// A click on a photo or video opens it in the viewer, a video playing
    /// at once; without pictures (or, for a video, without kitty's or with
    /// the setting off) it opens outside, once it's downloaded.
    fn open_media(&mut self, h: MediaHit) {
        let inside = if h.video { self.plays_inside() } else { self.picker.is_some() };
        if inside {
            return self.open_viewer(&h.chat, &h.id, h.video, h.gif);
        }
        if self.opening(&h.chat, &h.id) {
            self.open_when_ready = None;
            return;
        }
        match self.media_file(&h.chat, &h.id) {
            Some(f) => self.open_outside(&h.chat, &h.id, &f),
            None => {
                self.open_when_ready = Some((h.chat.clone(), h.id.clone()));
                self.cmd(json!({"c": "download", "chat": h.chat, "id": h.id}));
            }
        }
    }

    /// A photo on screen is fetched by itself, as WhatsApp Web does; only
    /// what WhatsApp's servers still have, the phone isn't asked.
    pub(crate) fn want_photo(&mut self, chat: &str, id: &str) {
        if self.fetched.insert((chat.to_string(), id.to_string())) {
            self.cmd(json!({"c": "download", "chat": chat, "id": id, "auto": true}));
        }
    }

    /// A picture on disk, read off the UI thread the first time it's wanted.
    pub(crate) fn picture(&mut self, path: &str) -> Option<&DynamicImage> {
        if path.is_empty() {
            return None;
        }
        if !self.pictures.contains_key(path) {
            self.pictures.insert(path.to_string(), Picture::Loading);
            let (tx, path) = (self.tx.clone(), path.to_string());
            std::thread::spawn(move || {
                // Big photos come down to what a bubble can show.
                let img = image::open(&path).ok().map(|i| if i.width().max(i.height()) > 1280 { i.thumbnail(1280, 1280) } else { i });
                let _ = tx.send(Ev::Picture(path, img));
            });
        }
        match self.pictures.get(path) {
            Some(Picture::Ready(img)) => Some(img),
            _ => None,
        }
    }

    /// `v` on the sidebar: the open chat's last video, in the viewer.
    fn play_last_video(&mut self) {
        let Some(chat) = self.open.clone() else { return };
        let last = self.msgs.get(&chat).and_then(|l| {
            l.iter().rev().find_map(|m| m.media.as_ref().filter(|md| md.kind == "video").map(|md| (m.id.clone(), md.gif)))
        });
        if let Some((id, gif)) = last {
            self.open_media(MediaHit { rect: Rect::default(), chat, id, video: true, gif });
        }
    }

    /// Space on the sidebar: the open chat's last audio, played or paused.
    fn play_last(&mut self) {
        let Some(chat) = self.open.clone() else { return };
        let last = self.msgs.get(&chat).and_then(|l| l.iter().rev().find_map(|m| audio_of(m).filter(|a| a.playable).map(|a| (m.id.clone(), a.secs))));
        if let Some((id, secs)) = last {
            self.play(&chat, &id, secs, None);
        }
    }

    fn key(&mut self, k: KeyEvent) {
        if k.kind == KeyEventKind::Release {
            return;
        }
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        if ctrl && k.code == KeyCode::Char('c') {
            self.quit = true;
            return;
        }
        // The viewer's over everything, also while its video has the screen.
        if self.viewer_key(k) || self.photo_key(k) {
            return;
        }
        let main_view = self.phase == Phase::Ready || (!self.chats.is_empty() && matches!(self.phase, Phase::Starting | Phase::Connecting));
        if !main_view {
            match k.code {
                KeyCode::Char('q') | KeyCode::Esc => self.quit = true,
                KeyCode::Char('r') if matches!(self.phase, Phase::Dead(_)) => self.start(),
                _ => {}
            }
            return;
        }
        // The account menu's and dialogs' first: they cover everything.
        if self.account_key(k) || self.chat_menu_key(k) || self.msg_menu_key(k) {
            return;
        }
        // The emoji panel's and popup's keys come first, their Esc before the info panel's.
        if self.react_key(k) || self.emoji_panel_key(k) || self.emoji_key(k) || self.attach_key(k) {
            return;
        }
        let page = (self.msgs_area.height as usize / 2).max(1);
        if k.code == KeyCode::Esc && self.text_sel.active() {
            self.text_sel.clear();
            return;
        }
        if k.code == KeyCode::Esc && self.profile.is_some() {
            self.profile = None;
            return;
        }
        match k.code {
            KeyCode::Char('n') if ctrl => return self.step_open(1),
            KeyCode::Char('p') if ctrl => return self.step_open(-1),
            // The search, from anywhere, the message box too.
            KeyCode::Char('k' | 'K') if ctrl => {
                self.emoji_panel.open = false;
                self.focus = Focus::Search;
                return;
            }
            KeyCode::PageUp => {
                self.scroll += page;
                return;
            }
            KeyCode::PageDown => {
                self.scroll = self.scroll.saturating_sub(page);
                return;
            }
            _ => {}
        }
        match self.focus {
            Focus::Sidebar => self.key_sidebar(k),
            Focus::Search => self.key_search(k),
            Focus::Input => self.key_input(k),
        }
    }

    fn key_sidebar(&mut self, k: KeyEvent) {
        match k.code {
            KeyCode::Up | KeyCode::Char('k') => self.move_cursor(-1),
            KeyCode::Down | KeyCode::Char('j') => self.move_cursor(1),
            KeyCode::Home | KeyCode::Char('g') => self.move_cursor(isize::MIN / 2),
            KeyCode::End | KeyCode::Char('G') => self.move_cursor(isize::MAX / 2),
            KeyCode::Enter | KeyCode::Right | KeyCode::Char('l') => {
                if let Some(item) = self.current() {
                    self.activate(item);
                }
            }
            KeyCode::Char('/') => self.focus = Focus::Search,
            KeyCode::Char('1') => self.set_filter(Filter::All),
            KeyCode::Char('2') => self.set_filter(Filter::Unread),
            KeyCode::Char('3') => self.set_filter(Filter::Favorites),
            KeyCode::Char('4') => self.set_filter(Filter::Groups),
            KeyCode::Char('m') => self.chat_menu_here(),
            KeyCode::Left | KeyCode::Char('h') if self.archived => self.show_archived(false),
            KeyCode::Tab if self.open.is_some() => self.focus_input(),
            // Ctrl+I is Tab to the terminal; `i` here instead.
            KeyCode::Char('i') => self.toggle_profile(),
            KeyCode::Char('I') => self.chat_photo(),
            KeyCode::Char(',') => self.open_account_menu(),
            KeyCode::Char('?') => self.open_shortcuts(),
            KeyCode::Char(' ') => self.play_last(),
            KeyCode::Char('v') => self.play_last_video(),
            // Out of the archived chats once nothing's searched.
            KeyCode::Esc if self.archived && self.search.is_empty() => self.show_archived(false),
            KeyCode::Esc => {
                self.search.clear();
                self.found = None;
            }
            KeyCode::Char('q') => self.quit = true,
            _ => {}
        }
    }

    fn key_search(&mut self, k: KeyEvent) {
        match k.code {
            KeyCode::Esc => {
                self.search.clear();
                self.found = None;
                self.focus = Focus::Sidebar;
                self.cursor_to_first();
            }
            KeyCode::Up => self.move_cursor(-1),
            KeyCode::Down => self.move_cursor(1),
            KeyCode::Enter => {
                if let Some(item) = self.current() {
                    self.activate(item);
                }
            }
            KeyCode::Backspace => {
                if self.search.pop().is_none() {
                    self.focus = Focus::Sidebar;
                }
                self.searched();
            }
            KeyCode::Char(c) if !k.modifiers.contains(KeyModifiers::CONTROL) => {
                self.search.push(c);
                self.searched();
            }
            _ => {}
        }
    }

    /// To the message box, unless the open chat takes no messages.
    pub fn focus_input(&mut self) {
        let read_only = self.open.as_deref().and_then(|j| self.chat(j)).is_some_and(|c| !c.read_only.is_empty());
        self.focus = if read_only { Focus::Sidebar } else { Focus::Input };
    }

    fn set_filter(&mut self, f: Filter) {
        self.filter = f;
        self.cursor_to_first();
    }

    fn cursor_to_first(&mut self) {
        self.cursor = None;
        self.side_offset = 0;
        self.follow = true;
    }

    /// The search text changed: the old answer no longer fits; ask again
    /// once typing pauses.
    fn searched(&mut self) {
        self.found = None;
        self.search_due = (!self.search.trim().is_empty()).then(|| Instant::now() + Duration::from_millis(250));
        self.cursor_to_first();
    }

    /// Up or down a line of the message box as it's drawn, keeping the
    /// column. Past the first or last line the conversation scrolls, as in slick.
    fn caret_line(&mut self, delta: isize) {
        let len = self.input.chars().count();
        let (row, col) = crate::ui::caret_pos(&self.input, self.caret, self.input_width);
        let target = row.checked_add_signed(delta);
        let mut best = None;
        for i in 0..=len {
            let (r, c) = crate::ui::caret_pos(&self.input, i, self.input_width);
            if Some(r) == target && c <= col {
                best = Some(i);
            } else if target.is_some_and(|t| r > t) {
                break;
            }
        }
        match best {
            Some(i) => self.caret = i,
            None if delta < 0 => self.scroll += 1,
            None => self.scroll = self.scroll.saturating_sub(1),
        }
    }

    /// Ctrl+←: the start of this word, or of the one before.
    fn word_left(&self) -> usize {
        let chars: Vec<char> = self.input.chars().collect();
        let mut i = self.caret;
        while i > 0 && !chars[i - 1].is_alphanumeric() {
            i -= 1;
        }
        while i > 0 && chars[i - 1].is_alphanumeric() {
            i -= 1;
        }
        i
    }

    /// Ctrl+→: the end of this word, or of the next one.
    fn word_right(&self) -> usize {
        let chars: Vec<char> = self.input.chars().collect();
        let mut i = self.caret;
        while i < chars.len() && !chars[i].is_alphanumeric() {
            i += 1;
        }
        while i < chars.len() && chars[i].is_alphanumeric() {
            i += 1;
        }
        i
    }

    fn key_input(&mut self, k: KeyEvent) {
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        if self.format_key(k) {
            return;
        }
        match k.code {
            KeyCode::Enter if !self.sends(k) => self.newline(),
            KeyCode::Enter if self.msg_menu.editing.is_some() => self.send_edit(),
            KeyCode::Enter => self.send(),
            KeyCode::Esc if self.msg_menu.editing.is_some() => self.cancel_edit(),
            KeyCode::Esc if self.replying() => self.cancel_reply(),
            KeyCode::Esc | KeyCode::Tab => self.focus = Focus::Sidebar,
            KeyCode::Backspace if self.caret > 0 => {
                self.caret -= 1;
                self.remove_at(self.caret);
            }
            KeyCode::Delete if self.caret < self.input.chars().count() => self.remove_at(self.caret),
            KeyCode::Left if ctrl => self.caret = self.word_left(),
            KeyCode::Right if ctrl => self.caret = self.word_right(),
            KeyCode::Left => self.caret = self.caret.saturating_sub(1),
            KeyCode::Right => self.caret = (self.caret + 1).min(self.input.chars().count()),
            // As on WhatsApp Web: Up in an empty box edits your last message.
            KeyCode::Up if self.input.is_empty() && self.edit_last() => {}
            KeyCode::Up => self.caret_line(-1),
            KeyCode::Down => self.caret_line(1),
            KeyCode::Home => self.caret = 0,
            KeyCode::End => self.caret = self.input.chars().count(),
            KeyCode::Char('u') if ctrl => {
                self.input.clear();
                self.caret = 0;
            }
            KeyCode::Char('w') if ctrl => {
                let chars: Vec<char> = self.input.chars().collect();
                let mut i = self.caret;
                while i > 0 && chars[i - 1] == ' ' {
                    i -= 1;
                }
                while i > 0 && chars[i - 1] != ' ' {
                    i -= 1;
                }
                self.input = chars[..i].iter().chain(&chars[self.caret..]).collect();
                self.caret = i;
            }
            KeyCode::Char(c) if !ctrl => self.insert(&c.to_string()),
            _ => {}
        }
    }

    /// Enter sends and Alt or Shift+Enter breaks the line, or the other way
    /// round, as the settings have it.
    pub(crate) fn sends(&self, k: KeyEvent) -> bool {
        let modified = k.modifiers.intersects(KeyModifiers::ALT | KeyModifiers::SHIFT);
        modified != self.settings.on(crate::config::ENTER_SENDS)
    }

    fn byte_at(&self, caret: usize) -> usize {
        self.input.char_indices().nth(caret).map_or(self.input.len(), |(i, _)| i)
    }

    pub(crate) fn insert(&mut self, s: &str) {
        let at = self.byte_at(self.caret);
        let s = s.replace("\r\n", "\n").replace('\r', "\n");
        self.input.insert_str(at, &s);
        self.caret += s.chars().count();
    }

    fn remove_at(&mut self, caret: usize) {
        let at = self.byte_at(caret);
        if at < self.input.len() {
            self.input.remove(at);
        }
    }

    fn send(&mut self) {
        let Some(chat) = self.open.clone() else { return };
        let text = self.input.trim().to_string();
        if text.is_empty() {
            return;
        }
        if self.phase != Phase::Ready {
            return self.note("Ainda conectando; tente de novo em instantes.");
        }
        let reply = self.take_reply(&chat);
        self.cmd(json!({"c": "send", "chat": chat, "text": text, "reply": reply}));
        self.input.clear();
        self.caret = 0;
        self.scroll = 0;
    }

    fn mouse(&mut self, m: MouseEvent) {
        // The divider being dragged keeps the mouse until let go; so does a
        // selection being dragged over the messages.
        if self.divider_held(m) || self.select_held(m) {
            return;
        }
        // A click anywhere drops the selection; one on text may start another.
        if matches!(m.kind, MouseEventKind::Down(_)) {
            self.text_sel.clear();
        }
        if self.viewer_mouse(m) || self.photo_mouse(m) || self.who_mouse(m) || self.chat_menu_mouse(m) || self.account_mouse(m) || self.msg_menu_mouse(m) || self.emoji_panel_mouse(m) || self.emoji_click(m) || self.attach_mouse(m) || self.react_mouse(m) {
            return;
        }
        if self.divider_mouse(m) {
            return;
        }
        let at = Position::new(m.column, m.row);
        match m.kind {
            MouseEventKind::ScrollUp if self.panel_area.contains(at) => {
                if let Some(p) = self.profile.as_mut() {
                    p.scroll = p.scroll.saturating_sub(3);
                }
            }
            MouseEventKind::ScrollDown if self.panel_area.contains(at) => {
                if let Some(p) = self.profile.as_mut() {
                    p.scroll += 3;
                }
            }
            MouseEventKind::Down(MouseButton::Left) if self.panel_close.contains(at) => self.profile = None,
            MouseEventKind::Down(MouseButton::Left) if self.panel_maps.contains(at) => {
                let url = self.profile.as_ref().and_then(|p| p.data.as_ref()).map(|d| d.maps.clone()).unwrap_or_default();
                if !url.is_empty() {
                    crate::links::open(&url);
                }
            }
            MouseEventKind::Down(MouseButton::Left) if self.profile.as_ref().is_some_and(|p| p.pic_area.contains(at)) => self.open_photo(),
            MouseEventKind::Down(MouseButton::Left) if self.panel_week.contains(at) => {
                if let Some(p) = self.profile.as_mut() {
                    p.week = !p.week;
                }
            }
            MouseEventKind::Down(MouseButton::Left) if self.panel_area.contains(at) => {
                let hit = self.panel_hits.iter().find(|(r, _)| r.contains(at)).map(|(_, j)| j.clone());
                if let Some(jid) = hit {
                    let name = self.profile.as_ref().and_then(|p| p.data.as_ref()).and_then(|d| d.members.iter().find(|m| m.jid == jid)).map(|m| m.name.clone());
                    if let Some(n) = name {
                        self.names.entry(jid.clone()).or_insert(n);
                    }
                    self.open_chat(jid);
                    self.focus_input();
                }
            }
            MouseEventKind::Down(MouseButton::Left) if self.newest_btn.contains(at) => self.scroll = 0,
            MouseEventKind::Down(MouseButton::Left) if self.audio_hits.iter().any(|h| h.play.contains(at) || h.track.contains(at)) => {
                let h = self.audio_hits.iter().find(|h| h.play.contains(at) || h.track.contains(at)).cloned().unwrap();
                // The track's last cell is the end; mpv's length is better than the message's.
                let dur = self.player.of(&h.chat, &h.id).map(|n| n.dur).filter(|&d| d > 0.0).unwrap_or(h.secs as f64);
                let seek = (h.track.contains(at) && dur > 0.0).then(|| {
                    let frac = (m.column - h.track.x) as f64 / h.track.width.saturating_sub(1).max(1) as f64;
                    frac.clamp(0.0, 1.0) * dur
                });
                self.play(&h.chat, &h.id, h.secs, seek);
            }
            MouseEventKind::Down(MouseButton::Left) if self.media_hits.iter().any(|h| h.rect.contains(at)) => {
                let h = self.media_hits.iter().find(|h| h.rect.contains(at)).cloned().unwrap();
                self.open_media(h);
            }
            MouseEventKind::Down(MouseButton::Left) if self.more_hits.iter().any(|(r, ..)| r.contains(at)) => {
                let hit = self.more_hits.iter().find(|(r, ..)| r.contains(at)).map(|(_, id, grow)| (id.clone(), *grow));
                if let (Some((id, grow)), Some(chat)) = (hit, self.open.clone()) {
                    // It grows downward: what's over it stays put.
                    self.scroll += grow;
                    self.expanded.insert((chat, id));
                }
            }
            MouseEventKind::Down(MouseButton::Left) if self.msgs_area.contains(at) && self.msg_links.iter().any(|(r, _)| r.contains(at)) => {
                if let Some((_, url)) = self.msg_links.iter().find(|(r, _)| r.contains(at)).cloned() {
                    match url.strip_prefix(crate::markup::MENTION) {
                        Some(jid) => {
                            let jid = jid.to_string();
                            let name = self.open.as_ref().and_then(|c| self.msgs.get(c)).and_then(|l| l.iter().flat_map(|m| &m.mentions).find(|x| x.jid == jid)).map(|x| x.name.clone());
                            if let Some(n) = name {
                                self.names.entry(jid.clone()).or_insert(n);
                            }
                            self.show_profile(jid);
                        }
                        None => crate::links::open(&url),
                    }
                }
            }
            MouseEventKind::Down(MouseButton::Left) if self.msgs_area.contains(at) && self.sender_hits.iter().any(|(r, _)| r.contains(at)) => {
                if let Some((_, jid)) = self.sender_hits.iter().find(|(r, _)| r.contains(at)).cloned() {
                    // Their name in the group while the panel loads, not just a number.
                    let name = self.open.as_ref().and_then(|c| self.msgs.get(c)).and_then(|l| l.iter().rev().find(|m| m.sender == jid)).map(|m| m.sender_name.clone());
                    if let Some(n) = name.filter(|n| !n.is_empty()) {
                        self.names.entry(jid.clone()).or_insert(n);
                    }
                    self.show_profile(jid);
                }
            }
            // On text but none of the above: a selection, or a double or triple click.
            MouseEventKind::Down(MouseButton::Left) if self.msgs_area.contains(at) => self.select_press(at),
            MouseEventKind::Down(MouseButton::Left) if self.header_area.contains(at) => self.toggle_profile(),
            MouseEventKind::Down(MouseButton::Left) if self.search_area.contains(at) => self.focus = Focus::Search,
            MouseEventKind::ScrollUp if self.msgs_area.contains(at) => self.scroll += 3,
            MouseEventKind::ScrollDown if self.msgs_area.contains(at) => self.scroll = self.scroll.saturating_sub(3),
            MouseEventKind::ScrollUp if self.side_list.contains(at) => {
                self.side_offset = self.side_offset.saturating_sub(1);
                self.follow = false;
            }
            MouseEventKind::ScrollDown if self.side_list.contains(at) => {
                self.side_offset += 1;
                self.follow = false;
            }
            MouseEventKind::Down(MouseButton::Left) if self.side_list.contains(at) => {
                let key = self.side_hits.iter().find(|(r, _)| r.contains(at)).map(|(_, k)| k.clone());
                if let Some(item) = key.and_then(|k| self.items().into_iter().find(|i| i.key().as_ref() == Some(&k))) {
                    self.activate(item);
                }
            }
            MouseEventKind::Down(MouseButton::Left) if self.chips.iter().any(|(r, _)| r.contains(at)) => {
                let f = self.chips.iter().find(|(r, _)| r.contains(at)).map(|(_, f)| *f).unwrap();
                self.set_filter(f);
            }
            MouseEventKind::Down(MouseButton::Left) if self.input_area.contains(at) && self.open.is_some() => {
                self.focus_input();
            }
            _ => {}
        }
    }
}
