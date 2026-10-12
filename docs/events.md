# Events and the running whatsapp-tui

whatsapp-tui tells other programs what is going on, and takes commands from
them, so you can build your own bar widget, notifier or automation on top of
it. The WhatsApp plugin for the Omarchy bar is built this way, using nothing
private.

There are three ways in:

| what | where | good for |
| --- | --- | --- |
| snapshot | `$XDG_RUNTIME_DIR/whatsapp-tui/state.json`, or `whatsapp-tui status --json` | bars that poll or watch a file |
| event stream | `whatsapp-tui events` (JSON lines) | long-running scripts |
| hooks | `~/.config/whatsapp-tui/hooks/<event>.d/*` | one-shot reactions (a sound, a notification) |

whatsapp-tui runs once per session. Running `whatsapp-tui` again brings the
open one to the front; `whatsapp-tui --new-instance` opens a second one anyway.
That one publishes nothing and takes no commands, and since WhatsApp lets a
linked device hold one connection at a time, it takes the connection over from
the first one.

whatsapp-tui sends no desktop notifications itself: that is left to the bar
plugin and to hooks, which get everything they need in `message.new`. It does
show a tray icon (StatusNotifierItem), white, with a green dot while chats
that ping wait (`attention`), plain during a pause; a click brings the window
up. A bar widget that shows its own icon can turn it off with `tray = false`
in `~/.config/whatsapp-tui/config`.

## The window

Launchers open whatsapp-tui in a terminal whose window class is
`org.omarchy.whatsapp-tui`:

```sh
omarchy-launch-tui --app-id=org.omarchy.whatsapp-tui whatsapp-tui
# or
xdg-terminal-exec --app-id=org.omarchy.whatsapp-tui -e whatsapp-tui
```

`whatsapp-tui focus` and `whatsapp-tui open` bring the window with that class
to the front (Hyprland). Set `WHATSAPP_TUI_WINDOW_CLASS` to use another class.

## Snapshot

Rewritten (atomically: written beside and renamed over) whenever something in
it changes. Message text never touches the disk: `$XDG_RUNTIME_DIR` is a tmpfs.

```json
{
  "v": 1,
  "running": true,
  "pid": 12345,
  "version": "0.0.1",
  "user": "Ana",
  "updated": 1760000000,
  "connected": true,
  "paused": { "paused": true, "until": 1760003600 },
  "attention": 2,
  "totals": { "chats": 3, "messages": 7, "direct": 1, "groups": 2, "muted": 1,
              "direct_messages": 2, "group_messages": 5, "archived": 1 },
  "conversations": [
    {
      "id": "5511900000000@s.whatsapp.net", "kind": "dm", "name": "Bruno Lima",
      "unread": 2, "latest": 1760000000, "muted": false, "quiet": false,
      "avatar": "/home/ana/.local/share/whatsapp-tui/avatars/5511900000000_s_whatsapp_net.jpg",
      "messages": [
        { "id": "3EB0A1B2C3D4E5F60718", "ts": 1759999900, "sender": "5511900000000@s.whatsapp.net",
          "author": "Bruno Lima", "text": "Oi!", "html": "Oi!" },
        { "id": "3EB0A1B2C3D4E5F60719", "ts": 1760000000, "sender": "5511900000000@s.whatsapp.net",
          "author": "Bruno Lima", "text": "Can you look at the *deploy*?", "html": "Can you look at the <b>deploy</b>?" }
      ]
    },
    {
      "id": "120363000000000000@g.us", "kind": "group", "name": "Acme",
      "unread": 4, "latest": 1759990000, "muted": true, "quiet": true, "avatar": null,
      "messages": [
        { "id": "3EB0F0E1D2C3B4A59687", "ts": 1759990000, "sender": "5511900000001@s.whatsapp.net",
          "author": "Carla", "text": "📷 Foto", "html": "📷 Foto" }
      ]
    }
  ]
}
```

- `id` is the chat's WhatsApp JID: `<number>@s.whatsapp.net` for a person,
  `<id>@g.us` for a group. `kind` is `dm` or `group`.
- `conversations` holds the chats with unread messages that are not archived,
  newest first. `unread` is how many messages wait there, as WhatsApp counts
  them; `latest` is the time of its newest message (Unix seconds).
- `messages` are the newest unread ones, up to 5, oldest first. They fill in a
  moment after the conversation appears. `sender` is the author's JID, and
  `author` the name to show (the contact's name, or their WhatsApp name).
- `text` is plain text, exactly as people wrote it, WhatsApp's `*bold*` and
  `_italic_` marks included: `<b>` in a message is those three characters.
  Escape it before showing it as HTML or markup (`notify-send` bodies, Qt rich
  text, a web page). `html` is the same text ready for that: escaped, with the
  message's own bold (`<b>`), italic (`<i>`), strikethrough (`<s>`) and
  monospace (`<tt>`) as tags. Photos, voice notes, documents and the like are a
  label with an emoji (`📷 Foto`, `🎤 Áudio 0:12`), with their caption when they
  have one. Both are cut at 500 characters.
- `muted` is the chat muted in WhatsApp. Like WhatsApp, a muted chat still
  counts in `totals` and is listed here, but does not ping: it is out of
  `attention`, and its `message.new` events say `notify: false`. `quiet` is the
  same thing, under the name other widgets know.
- `avatar` is the chat's profile picture on disk, or `null` when there is none
  (yet: pictures are fetched as the chat list shows them).
- `attention` is what the badge counts: unread chats that are neither muted
  nor archived.
- `totals.chats` counts the unread chats listed in `conversations` (muted ones
  too); `direct` and `groups` split them; `muted` is how many of them are muted.
  `messages` adds up their `unread`, and `direct_messages` and `group_messages`
  split that. `archived` counts the unread chats that are archived, which are
  in nothing else.
- `paused` is the pause you set (`whatsapp-tui pause`): `{ "paused": false,
  "until": null }` while notifications are on. WhatsApp has no do-not-disturb
  of its own, so this pause lives here only: it survives restarts, keeps the
  badge counting and only turns off `notify` in `message.new`.
- `connected` is false while whatsapp-tui is connecting or reconnecting to
  WhatsApp; the counts are then the last ones it knew.
- The chat open in whatsapp-tui only counts as unread while its window doesn't
  have focus: getting focus reads it.
- After `whatsapp-tui quit` the file says `"running": false`. After a crash it
  can be left over: `whatsapp-tui status --json` checks that whatsapp-tui
  really is running.

## Events

Each event is one line of JSON:

```json
{ "v": 1, "event": "message.new", "at": 1760000000, "data": { … } }
```

| event | `data` |
| --- | --- |
| `app.started` | `pid`, `version`, `user` |
| `app.stopped` | `pid` |
| `counts.changed` | `attention`, `totals` (as in the snapshot) |
| `message.new` | `conversation` (`id`, `kind`, `name`, `muted`, `avatar`), `message` (as in the snapshot), `muted`, `notify` |
| `conversation.read` | `conversation` (`id`, `kind`, `name`, `muted`, `avatar`) |
| `pause.changed` | `paused`, `until` (as in the snapshot) |

`message.new` covers messages other people send you, as they arrive. It does
not replay what was already waiting when whatsapp-tui started, nor messages
you send (from here or your phone), edits or deletions. `notify` is whether it
deserves a notification: false when the chat is muted, while notifications are
paused, and for the chat open in whatsapp-tui while its window has focus (it is
read there). Archived chats still notify, as on WhatsApp.

`conversation.read` says a chat left the snapshot's unread ones: read here,
read on another device (your phone), or archived.

`pause.changed` comes when a pause starts, is changed, ends by itself or is
taken back.

```sh
# every new message that should notify, as it arrives
whatsapp-tui events | jq -c 'select(.event == "message.new" and .data.notify)'
```

`whatsapp-tui events` waits for whatsapp-tui when it isn't running and
reconnects when it restarts, so it can run from your session's autostart.

## Hooks

Every executable file in `~/.config/whatsapp-tui/hooks/<event>.d/` runs for
that event, with the event line on stdin and its name in
`$WHATSAPP_TUI_EVENT`. They run in the background, in name order, and nothing
waits for them.

```sh
#!/bin/sh
# ~/.config/whatsapp-tui/hooks/message.new.d/notify
ev=$(cat)
echo "$ev" | jq -e '.data.notify' >/dev/null || exit 0
notify-send -a WhatsApp \
  "$(echo "$ev" | jq -r '.data.conversation.name')" \
  "$(echo "$ev" | jq -r '.data.message.html')"
```

```sh
#!/bin/sh
# ~/.config/whatsapp-tui/hooks/message.new.d/dm-sound
jq -e '.data.notify and .data.conversation.kind == "dm"' >/dev/null &&
  pw-play /usr/share/sounds/freedesktop/stereo/message.oga
```

## Commands

```sh
whatsapp-tui open <chat> [message-id] [--background]
whatsapp-tui focus
whatsapp-tui mark-read <chat>
whatsapp-tui pause <minutes | 30m | 1h | 8h | tomorrow>
whatsapp-tui resume
whatsapp-tui quit
whatsapp-tui status [--json]
whatsapp-tui events
```

`<chat>` is a JID as the snapshot has it. `whatsapp-tui open` shows the chat,
scrolled to the message when its id is given, and brings the window to the
front (Hyprland) unless `--background` is given. With whatsapp-tui closed,
`open` and `focus` start it, in a new terminal window (`xdg-terminal-exec`,
with the window class above) when not run from one.

`pause` takes minutes (`45`), a duration (`30m`, `1h`, `8h`, `1h30m`) or
`tomorrow` (9 AM tomorrow); a new pause replaces the old one. `pause` and
`resume` work with whatsapp-tui closed too: the pause is kept in
`~/.local/share/whatsapp-tui/pause.json`.

Underneath, these write JSON lines to the socket at
`$XDG_RUNTIME_DIR/whatsapp-tui/whatsapp-tui.sock`, which you can also do
directly:

```sh
echo '{"cmd":"open","chat":"5511900000000@s.whatsapp.net","id":"3EB0A1B2C3D4E5F60719"}' |
  socat - UNIX-CONNECT:$XDG_RUNTIME_DIR/whatsapp-tui/whatsapp-tui.sock
```

Commands: `focus`, `open` (`chat`, `id`, `focus`: false to stay in the
background), `mark-read` (`chat`), `pause` (`until`, Unix seconds, or
`minutes`), `resume`, `quit`, and `subscribe`, which turns the connection into
the event stream.

## Files

| what | where |
| --- | --- |
| snapshot, socket, lock | `$XDG_RUNTIME_DIR/whatsapp-tui/` |
| session, chats, pictures, drafts, pause | `~/.local/share/whatsapp-tui/` |
| downloaded media, the bridge | `~/.cache/whatsapp-tui/` |
| hooks | `~/.config/whatsapp-tui/hooks/` |

Folders from before the rename (`~/.local/share/zap`, `~/.cache/zap`) are moved
to these on the first start, session included.
