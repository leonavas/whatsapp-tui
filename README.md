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
- Optional, at run time: `mpv` for voice notes and audio, `wl-paste` or
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
changes the folder and `WHATSAPP_TUI_VERSION=v0.1.0` picks a version. Run it
again to update.

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
| `/` | search chats, contacts, messages and phone numbers |
| `1` `2` `3` | all chats, unread, groups |
| `Tab` | between the chat list and the message box |
| `i` | the contact's or group's details |
| `Space` | play or pause the open chat's last audio |
| `PgUp` `PgDn` | scroll the conversation |
| `Enter` / `Shift+Enter` | send / new line |
| `Ctrl+B` / `Ctrl+I` | bold / italic (`Alt+B` / `Alt+I` where the terminal keeps them) |
| `Ctrl+Shift+X` `M` `C` | strikethrough, monospace, code (`Alt+X` `Alt+M` `Alt+C` too) |
| `Ctrl+Shift+9` | make the line a quote, or not |
| `:` | emoji by name |
| `Ctrl+V` | paste a picture or files |
| `Alt+S` / `,` | your account: pause notifications, profile, settings, disconnect |
| `q`, `Ctrl+C` | quit |

The mouse works too: click chats, links, audio players, the clip and the
smiley in the message box, and your name at the foot of the chat list.

## Events and scripting

whatsapp-tui runs once per session (running it again brings the open one to
the front) and tells other programs what is unread: a snapshot in
`$XDG_RUNTIME_DIR/whatsapp-tui/state.json`, an event stream
(`whatsapp-tui events`), hooks in `~/.config/whatsapp-tui/hooks/`, and
commands such as `whatsapp-tui open <chat>` and `whatsapp-tui pause 1h`. See
[docs/events.md](docs/events.md).

It shows a tray icon (StatusNotifierItem) with a dot while chats wait. The
settings (your name › Configurações) turn it off, and with it the sidebar's
photos, read receipts and whether Enter sends; they're kept as `key = value`
lines in `~/.config/whatsapp-tui/config` (`tray`, `sidebar_avatars`,
`read_receipts`, `enter_sends`).

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

## License

Apache-2.0. See [NOTICE](NOTICE) for the trademarks and third-party data.
