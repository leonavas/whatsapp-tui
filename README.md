# whatsapp-tui

A lightweight WhatsApp client for the terminal. It links to your phone as
another device (like WhatsApp Web) and shows your chats, groups, pictures,
voice notes and contact details in a terminal UI.

The WhatsApp side is a small Go helper built on
[whatsmeow](https://github.com/tulir/whatsmeow), carried inside the
binary; the interface is Rust ([ratatui](https://ratatui.rs)).

> **Unofficial.** whatsapp-tui is not affiliated with, endorsed by or
> connected to WhatsApp or Meta. "WhatsApp" is a trademark of its owner, used
> here only to say what this connects to. Use it at your own risk.

## Building

You need:

- Rust (stable) and Cargo
- Go, with CGO and a C compiler (the helper uses SQLite through cgo). The
  repository pins Go with [mise](https://mise.jdx.dev): `mise install`, and the
  build finds it through `mise exec` when `go` isn't on your `PATH`.
- At run time, optionally: `mpv` for voice notes and audio, `wl-paste` or
  `xclip` to paste pictures, `zenity`, `yad` or `kdialog` to pick files.

```sh
cargo build --release
./target/release/whatsapp-tui
```

The first start shows a QR code: scan it in WhatsApp on your phone, under
**Linked devices**. The session is kept in `~/.local/share/whatsapp-tui/`.

`assets/` has the icon and a desktop entry (`whatsapp-tui.desktop`, which
opens it in a terminal with the window class `org.omarchy.whatsapp-tui`).

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
| `:` | emoji by name |
| `Ctrl+V` | paste a picture or files |
| `q`, `Ctrl+C` | quit |

The mouse works too: click chats, links, audio players, the clip and the
smiley in the message box.

## Events and scripting

whatsapp-tui runs once per session (running it again brings the open one to
the front) and tells other programs what is unread: a snapshot in
`$XDG_RUNTIME_DIR/whatsapp-tui/state.json`, an event stream
(`whatsapp-tui events`), hooks in `~/.config/whatsapp-tui/hooks/`, and
commands such as `whatsapp-tui open <chat>` and `whatsapp-tui pause 1h`. See
[docs/events.md](docs/events.md).

It shows a tray icon (StatusNotifierItem) with a dot while chats wait; put
`tray = false` in `~/.config/whatsapp-tui/config` to turn it off.

## License

Apache-2.0. See [NOTICE](NOTICE) for the trademarks and third-party data.
