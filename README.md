# WhatsApp TUI

A lightweight WhatsApp client for the terminal. It links to your phone as
another device (like WhatsApp Web) and shows your chats, groups, pictures,
voice notes and contact details in a terminal UI.

The WhatsApp side is a small Go helper built on
[whatsmeow](https://github.com/tulir/whatsmeow), carried inside the
binary; the interface is Rust ([ratatui](https://ratatui.rs)).

The interface is in Brazilian Portuguese for now.

> **Unofficial.** whatsapp-tui is not affiliated with, endorsed by or
> connected to WhatsApp or Meta. "WhatsApp" is a trademark of its owner, used
> here only to say what this connects to. Use it at your own risk.

## Requirements

- Linux (x86_64 or aarch64).
- A terminal with an image protocol is recommended for pictures and avatars:
  kitty, ghostty, WezTerm, foot or any sixel terminal. Others fall back to
  half-block images.
- A [Nerd Font](https://www.nerdfonts.com/) for the icons.
- Optional, at run time: `mpv` for voice notes, audio and videos, `ffmpeg`
  for sharper video previews, `wl-paste` or
  `xclip` to paste pictures, `zenity`, `yad` or `kdialog` to pick files.

## Install

```sh
curl -fsSL https://raw.githubusercontent.com/leonavas/whatsapp-tui/master/install.sh | sh
```

The script ([install.sh](install.sh)) downloads the latest
[release](https://github.com/leonavas/whatsapp-tui/releases) for your
architecture (built on Ubuntu 24.04, so glibc 2.39 or newer), checks its
SHA-256 and puts `whatsapp-tui` in `~/.local/bin`, without sudo, plus its
desktop entry and icon in `~/.local/share`, so it shows in your app launcher.
It warns when that folder is not on your `PATH`. `WHATSAPP_TUI_INSTALL_DIR`
changes the folder and `WHATSAPP_TUI_VERSION=v0.1.0` picks a version.
Installed this way, whatsapp-tui updates itself (see [Auto-update](#auto-update));
running the script again also updates it.

### From source

You need Rust (stable) and Go, with CGO and a C compiler (the helper uses
SQLite through cgo). The repository pins Go with [mise](https://mise.jdx.dev):
`mise install`, and the build finds it through `mise exec` when `go` isn't on
your `PATH`.

Then either:

```sh
cargo install --locked --git https://github.com/leonavas/whatsapp-tui
```

or, from a clone:

```sh
cargo build --release
./target/release/whatsapp-tui
```

`assets/` has the icon and a desktop entry (`whatsapp-tui.desktop`, which
opens it in a terminal with the window class `org.omarchy.whatsapp-tui`).

## Link your phone

The first start shows a QR code: scan it in WhatsApp on your phone, under
**Linked devices**. The session is kept in `~/.local/share/whatsapp-tui/`.

## Keys

| key | does |
| --- | --- |
| `↑` `↓` / `k` `j`, `g` `G` | move through the chats |
| `Enter` / `→` / `l` | open the chat |
| `Ctrl+N` / `Ctrl+P` | open the next or previous chat |
| `/`, `Ctrl+K` | search chats, contacts, messages and phone numbers (`Ctrl+K` from anywhere, the message box too) |
| `1` `2` `3` `4` | all chats, unread, favorites, groups |
| `m` | the chat's menu: archive, pin, mark read or unread, mute, favorite, clear, delete |
| `Esc` / `←` | out of the archived chats |
| `Tab` | between the chat list and the message box |
| `i` | the contact's or group's details |
| `Space` | play or pause the open chat's last audio |
| `v` | open the open chat's last video in the viewer |
| `Space` / `Enter`, `b`, `p`, `o`, `Esc` / `q` | in the viewer: play or stop the video, save it to your downloads folder, save it and show it in the file manager, open it outside the app, close |
| `PgUp` `PgDn` | scroll the conversation |
| `Enter` / `Shift+Enter` | send / new line |
| `↑` in an empty message box | edit your last message (within 15 minutes; `Esc` gives up) |
| `Ctrl+B` / `Ctrl+I` | bold / italic (`Alt+B` / `Alt+I` where the terminal keeps them) |
| `Ctrl+Shift+X` `M` `C` | strikethrough, monospace, code (`Alt+X` `Alt+M` `Alt+C` too) |
| `Ctrl+Shift+9` | make the line a quote, or not |
| `:` | emoji by name |
| `Alt+E` / `Alt+A` | the emoji panel / attach files |
| `Ctrl+V` | paste a picture or files |
| `Alt+S` / `,` | your account: pause notifications, profile, settings, shortcuts, disconnect |
| `Ctrl+/` / `?` | the keyboard shortcuts (`?` from the chat list) |
| `q`, `Ctrl+C` | quit |

All of them are also in your name › **Atalhos de teclado**, which follows
whether Enter sends.

The mouse works too: click chats, links, audio players, photos and videos (open
them in the viewer; a click outside the picture closes it), a business message's link buttons, the clip
and the smiley in the message box, and your name at the foot of the chat
list; right-click a chat (or click the chevron by its time) for its menu.

Hovering a message shows a chevron in its corner (a right-click does the
same) with its menu: **Copiar**, **Editar** (yours, for 15 minutes),
**Reagir** and **Apagar**. Apagar puts a checkbox by each message to pick
more of them (`Esc` leaves, `Enter` or the trash asks); then **Apagar para
mim**, or **Apagar para todos** when every one picked is yours, about two and
a half days old at most, and not in the chat with yourself.

## Photos and videos

Photos and videos show inside their bubbles, as on WhatsApp Web: the
message's thumbnail at once, the photo itself once it's downloaded (photos on
screen download by themselves; for older ones the phone isn't asked). A click
opens one in the viewer, as WhatsApp Web's: big over the whole window, with
who sent it, when, and its caption. A video (GIFs too, looped and muted)
plays there through `mpv --vo=kitty` in terminals that speak kitty's graphics
protocol (kitty, ghostty); the rest of the screen waits while it plays, and
when it ends its picture stays, to play again. `o` opens it outside the app.
Elsewhere, or with **Tocar vídeos no app** off in the settings, a video opens
in an mpv window, and without mpv in whatever `xdg-open` picks; photos open
in your image viewer in terminals without pictures. With `ffmpeg` installed, a downloaded video's bubble gets a
sharper frame than the thumbnail. Messages from businesses (and from
WhatsApp itself) show their header picture, title, footer and buttons; link
buttons open in the browser.

## Chats

As on WhatsApp Web, pinned chats come first (the last pinned on top, three at
most), and a chat's row shows when it's muted or pinned. Archived chats leave
the list for **Arquivadas** at its top, with how many of them are unread; new
messages keep them there unless the phone is set to unarchive them. The chat
menu archives, pins, marks read or unread, mutes (8 hours, a week or always),
adds to Favorites (the **Favoritas** chip), clears and deletes, and the
change goes to WhatsApp, so your phone and other devices follow; changes made
there show here too. A favorite added here reaches the phone once WhatsApp
has sent this device its favorites (that is, once you have at least one
favorite set on the phone); until then it's kept here only.

## Events and scripting

whatsapp-tui runs once per session (running it again brings the open one to
the front) and tells other programs what is unread: a snapshot in
`$XDG_RUNTIME_DIR/whatsapp-tui/state.json`, an event stream
(`whatsapp-tui events`), hooks in `~/.config/whatsapp-tui/hooks/`, and
commands such as `whatsapp-tui open <chat>` and `whatsapp-tui pause 1h`. See
[docs/events.md](docs/events.md).

It shows a tray icon (StatusNotifierItem) with a dot while chats wait. The
settings (your name › Configurações) turn it off, and with it the sidebar's
photos, read receipts, whether Enter sends and the update check; they're kept
as `key = value` lines in `~/.config/whatsapp-tui/config` (`tray`,
`sidebar_avatars`, `read_receipts`, `enter_sends`, `auto_update`).

## Omarchy plugin

![WhatsApp in the Omarchy bar: badges, the hover preview and the menu](https://raw.githubusercontent.com/leonavas/omarchy-whatsapp-tui/master/preview.png)

On [Omarchy](https://omarchy.org), the
[WhatsApp TUI bar plugin](https://github.com/leonavas/omarchy-whatsapp-tui)
(`leonavas.whatsapp-tui`) runs on whatsapp-tui and puts WhatsApp in the bar in
place of the tray icon:

- red and blue badges for unread direct chats and groups (muted chats stay
  quiet);
- a preview of the unread chats and their newest messages on hover, a click
  jumps to the chat;
- desktop notifications for new messages, with the contact's picture;
- whatsapp-tui parked in the scratchpad (SUPER+S), a click shows or hides it;
- a right-click menu: Launch on Login, Launch in Foreground, Open in
  Scratchpad, pause notifications.

Install whatsapp-tui first (above), then:

```sh
omarchy plugin add https://github.com/leonavas/omarchy-whatsapp-tui.git
omarchy plugin enable leonavas.whatsapp-tui --section right
```

Update it with `omarchy plugin update leonavas.whatsapp-tui`, then
`omarchy restart shell`. With the plugin in the bar, turn whatsapp-tui's own
tray icon off in Settings (your name › Configurações), or with `tray = false`
in the config. Remove it with `omarchy plugin remove leonavas.whatsapp-tui`.

## Auto-update

Every time it starts, and every 6 hours while open, whatsapp-tui asks GitHub
for the latest release in the background (errors are ignored). When there is
a newer one, a small green "↑ X.Y.Z" button shows at the right of your name in
the account button, at the bottom of the sidebar. Nothing is downloaded until
you click it:

- If whatsapp-tui was installed from a release into a folder you can write to
  (like `~/.local/bin`, where install.sh puts it), the click downloads the
  archive for your architecture, checks its SHA-256 and replaces
  `whatsapp-tui` in place (and the desktop entry and icons, if install.sh put
  them in `~/.local/share`). The button reads "atualizando…" meanwhile, then
  "↻ reiniciar": click it (or press `Ctrl+O`) to restart into the new version
  in the same terminal; the WhatsApp helper stops and the new one starts with
  its own. On failure the button comes back and the error shows in the status
  line.
- If the binary belongs to something else (`~/.cargo/bin`, `/usr`, `/opt`,
  `/nix`) or the folder is read-only, the click only tells in the status line
  how to update (for cargo, the `cargo install` command above).

Development builds (`target/`) never check, unless `WHATSAPP_TUI_UPDATE_DEV=1`
is set: then the button shows (with a made-up version when there is no
release) and a click only says it is a development build.

`whatsapp-tui update` checks and installs right away, printing what it does,
and "Procurar atualizações" in the account menu checks right away from the app
(also with `auto_update = false`). Turn the automatic check off in Settings
(your name › Configurações › Atualizações), or with `auto_update = false` in
the config.

## License

Apache-2.0. See [NOTICE](NOTICE) for the trademarks and third-party data.
