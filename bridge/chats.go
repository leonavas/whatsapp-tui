package main

// A chat's own settings, as WhatsApp keeps them in its app state: pinned,
// archived, muted, read or not, favorite, cleared and deleted. Changes from
// the phone arrive as app state events; ours go out as patches, applied here
// first so the list moves at once.

import (
	"bytes"
	"encoding/json"
	"os"
	"strconv"
	"time"

	"go.mau.fi/whatsmeow/appstate"
	"go.mau.fi/whatsmeow/proto/waCommon"
	"go.mau.fi/whatsmeow/proto/waSyncAction"
	"go.mau.fi/whatsmeow/types"
	"go.mau.fi/whatsmeow/types/events"
	"google.golang.org/protobuf/proto"
)

// mute_until for "Sempre".
const muteForever int64 = -1

// WhatsApp lets no more than this many chats be pinned.
const maxPinned = 3

func (b *Bridge) meta(k string) string {
	var v string
	b.db.QueryRow(`SELECT v FROM meta WHERE k = ?`, k).Scan(&v)
	return v
}

func (b *Bridge) setMeta(k, v string) {
	b.db.Exec(`INSERT INTO meta (k, v) VALUES (?, ?) ON CONFLICT (k) DO UPDATE SET v = excluded.v`, k, v)
}

// flush sends the chat list now, not on the next tick: what the menu did shows at once.
func (b *Bridge) flush() {
	b.dirty.Store(false)
	emit(map[string]any{"t": "chats", "chats": b.chats()})
}

// waJID is the chat as WhatsApp's app state names it: a patch under another
// JID (the number where the phone has the LID) would be another chat to it.
func (b *Bridge) waJID(chat string) types.JID {
	var wa string
	b.db.QueryRow(`SELECT wa_jid FROM chats WHERE jid = ?`, chat).Scan(&wa)
	for _, s := range []string{wa, chat} {
		if j, err := types.ParseJID(s); err == nil && s != "" {
			return j
		}
	}
	return types.EmptyJID
}

// lastKey is the chat's newest message, as the ranges in archive, read,
// clear and delete patches carry it; zero and nil when it has none.
func (b *Bridge) lastKey(chat string) (time.Time, *waCommon.MessageKey) {
	var id, sender, waSender string
	var fromMe bool
	var ts int64
	if b.db.QueryRow(`SELECT id, from_me, ts, sender, wa_sender FROM messages WHERE chat = ? ORDER BY ts DESC LIMIT 1`, chat).
		Scan(&id, &fromMe, &ts, &sender, &waSender) != nil {
		return time.Time{}, nil
	}
	wa := b.waJID(chat)
	key := &waCommon.MessageKey{RemoteJID: proto.String(wa.String()), FromMe: proto.Bool(fromMe), ID: proto.String(id)}
	if wa.Server == types.GroupServer && !fromMe {
		if waSender == "" {
			waSender = sender
		}
		key.Participant = proto.String(waSender)
	}
	return time.Unix(ts, 0), key
}

// messageRange is appstate's own (unexported), for the patch built here.
func messageRange(ts time.Time, key *waCommon.MessageKey) *waSyncAction.SyncActionMessageRange {
	if ts.IsZero() {
		ts = time.Now()
	}
	r := &waSyncAction.SyncActionMessageRange{LastMessageTimestamp: proto.Int64(ts.Unix())}
	if key != nil {
		r.Messages = []*waSyncAction.SyncActionMessage{{Key: key, Timestamp: proto.Int64(ts.Unix())}}
	}
	return r
}

// buildClearChat: whatsmeow has no builder for it. The index is WhatsApp's:
// the chat, then "0" to keep starred messages and "0" to keep the phone's
// media files; version 6, as deleteChat's.
func buildClearChat(target types.JID, ts time.Time, key *waCommon.MessageKey) appstate.PatchInfo {
	return appstate.PatchInfo{
		Type: appstate.WAPatchRegularHigh,
		Mutations: []appstate.MutationInfo{{
			Index:   []string{appstate.IndexClearChat, target.String(), "0", "0"},
			Version: 6,
			Value:   &waSyncAction.SyncActionValue{ClearChatAction: &waSyncAction.ClearChatAction{MessageRange: messageRange(ts, key)}},
		}},
	}
}

// buildFavorites: the whole list goes each time, in its order, with the
// version and collection the phone's own favorites came with (favLog).
func buildFavorites(ids []string, version int32, collection string) appstate.PatchInfo {
	favs := make([]*waSyncAction.FavoritesAction_Favorite, len(ids))
	for i, id := range ids {
		favs[i] = &waSyncAction.FavoritesAction_Favorite{ID: proto.String(id)}
	}
	return appstate.PatchInfo{
		Type: appstate.WAPatchName(collection),
		Mutations: []appstate.MutationInfo{{
			Index:   []string{appstate.IndexFavorites},
			Version: version,
			Value:   &waSyncAction.SyncActionValue{FavoritesAction: &waSyncAction.FavoritesAction{Favorites: favs}},
		}},
	}
}

// favLog is whatsmeow's context logger. Its events leave out each app state
// mutation's version, which a patch must carry and which whatsmeow has no
// builder (so no number) for favorites; its log of each mutation has it, so
// the one the phone used is kept. Warnings and errors go to stderr; the rest
// is dropped.
type favLog struct{ b *Bridge }

func (l favLog) Write(p []byte) (int, error) {
	switch {
	case bytes.Contains(p, []byte(`"index":["favorites"]`)):
		var r struct {
			Message string `json:"message"`
			Patch   string `json:"patch_name"`
			Version int32  `json:"version"`
		}
		if json.Unmarshal(p, &r) == nil && r.Message == "Received app state mutation" && r.Version > 0 && r.Patch != "" {
			l.b.setMeta("fav_version", strconv.Itoa(int(r.Version)))
			l.b.setMeta("fav_patch", r.Patch)
		}
	case bytes.Contains(p, []byte(`"level":"warn"`)), bytes.Contains(p, []byte(`"level":"error"`)):
		os.Stderr.Write(p)
	}
	return len(p), nil
}

// resyncOnce reads the chats' app state (pins, archives, mutes, favorites)
// in full, once per account: a session linked before the bridge listened to
// it missed what changed since, and favorites come in no other way.
func (b *Bridge) resyncOnce() {
	if b.meta("resynced") != "" {
		return
	}
	for _, name := range []appstate.WAPatchName{appstate.WAPatchRegularLow, appstate.WAPatchRegularHigh} {
		if err := b.cli.FetchAppState(b.ctx, name, true, false); err != nil {
			logf("app state resync %s: %v", name, err)
			return
		}
	}
	b.setMeta("resynced", "1")
	b.dirty.Store(true)
}

// unarchiveOnNews: with "Manter conversas arquivadas" off on the phone, a
// new message brings an archived chat back to the list, as WhatsApp does.
func (b *Bridge) unarchiveOnNews(chat string) {
	if b.meta("unarchive") == "1" {
		b.db.Exec(`UPDATE chats SET archived = 0 WHERE jid = ?`, chat)
	}
}

// stateChat is the chat an app state event is about, its row there (how
// WhatsApp named it kept); ok false for what the TUI doesn't list.
func (b *Bridge) stateChat(j types.JID) (string, bool) {
	chat := b.norm(j)
	if skip(chat) {
		return "", false
	}
	b.db.Exec(`INSERT INTO chats (jid, wa_jid) VALUES (?, ?) ON CONFLICT (jid) DO UPDATE SET wa_jid = excluded.wa_jid`,
		chat.String(), j.ToNonAD().String())
	return chat.String(), true
}

// forget drops a chat's messages up to `upTo` (what a clear or delete on
// some device covered: newer ones stay) and tells the TUI. A deleted chat
// with nothing newer is gone; a cleared one stays, empty, as on the phone.
// An old clear or delete seen again in a full sync finds nothing to drop
// and changes nothing.
func (b *Bridge) forget(chat string, upTo int64, deleted bool) {
	res, err := b.db.Exec(`DELETE FROM messages WHERE chat = ? AND ts <= ?`, chat, upTo)
	if err != nil {
		return
	}
	n, _ := res.RowsAffected()
	b.db.Exec(`DELETE FROM media WHERE chat = ? AND id NOT IN (SELECT id FROM messages WHERE chat = ?)`, chat, chat)
	var left int
	b.db.QueryRow(`SELECT count(*) FROM messages WHERE chat = ?`, chat).Scan(&left)
	switch {
	case left == 0 && deleted:
		b.db.Exec(`UPDATE chats SET unread = 0, marked = 0, pinned = 0, archived = 0, cleared = 0 WHERE jid = ?`, chat)
	case left == 0:
		b.db.Exec(`UPDATE chats SET unread = 0, marked = 0 WHERE jid = ?`, chat)
	}
	if !deleted && n > 0 {
		b.db.Exec(`UPDATE chats SET cleared = ? WHERE jid = ?`, max(upTo, 1), chat)
	}
	b.dirty.Store(true)
	if n > 0 {
		emit(map[string]any{"t": "cleared", "chat": chat, "deleted": deleted && left == 0})
	}
}

// upTo is where a range ends: its newest message, else when it was made.
func upTo(r *waSyncAction.SyncActionMessageRange, at time.Time) int64 {
	if ts := r.GetLastMessageTimestamp(); ts > 0 {
		return ts
	}
	return at.Unix()
}

// appStateEvent keeps what the phone (or this bridge, its patches echoed
// back) changed in a chat's settings.
func (b *Bridge) appStateEvent(raw any) {
	switch e := raw.(type) {
	case *events.Pin:
		chat, ok := b.stateChat(e.JID)
		if !ok {
			return
		}
		// pinned is when: the newest pin is on top.
		var pin int64
		if e.Action.GetPinned() {
			pin = max(e.Timestamp.Unix(), 1)
		}
		b.db.Exec(`UPDATE chats SET pinned = ? WHERE jid = ?`, pin, chat)
	case *events.Archive:
		chat, ok := b.stateChat(e.JID)
		if !ok {
			return
		}
		// Archived chats aren't pinned.
		if e.Action.GetArchived() {
			b.db.Exec(`UPDATE chats SET archived = 1, pinned = 0 WHERE jid = ?`, chat)
		} else {
			b.db.Exec(`UPDATE chats SET archived = 0 WHERE jid = ?`, chat)
		}
	case *events.Mute:
		chat, ok := b.stateChat(e.JID)
		if !ok {
			return
		}
		var until int64
		if e.Action.GetMuted() {
			until = muteForever
			if end := e.Action.GetMuteEndTimestamp(); end >= 0 {
				until = end / 1000
			}
		}
		b.db.Exec(`UPDATE chats SET mute_until = ? WHERE jid = ?`, until, chat)
	case *events.MarkChatAsRead:
		chat, ok := b.stateChat(e.JID)
		if !ok {
			return
		}
		if !e.Action.GetRead() {
			b.db.Exec(`UPDATE chats SET marked = 1, unread = max(unread, 1) WHERE jid = ?`, chat)
			break
		}
		// Read up to its range: what came after (an old mark seen again in a
		// full sync) is still unread.
		var newer int
		b.db.QueryRow(`SELECT count(*) FROM messages WHERE chat = ? AND from_me = 0 AND ts > ?`,
			chat, upTo(e.Action.GetMessageRange(), e.Timestamp)).Scan(&newer)
		b.db.Exec(`UPDATE chats SET marked = 0, unread = min(unread, ?) WHERE jid = ?`, newer, chat)
		if newer == 0 {
			emit(map[string]any{"t": "read", "chat": chat})
		}
	case *events.ClearChat:
		if chat, ok := b.stateChat(e.JID); ok {
			b.forget(chat, upTo(e.Action.GetMessageRange(), e.Timestamp), false)
		}
	case *events.DeleteChat:
		if chat, ok := b.stateChat(e.JID); ok {
			b.forget(chat, upTo(e.Action.GetMessageRange(), e.Timestamp), true)
		}
	case *events.UnarchiveChatsSetting:
		on := "0"
		if e.Action.GetUnarchiveChats() {
			on = "1"
		}
		b.setMeta("unarchive", on)
		return
	case *events.AppState:
		if len(e.Index) == 0 || e.Index[0] != appstate.IndexFavorites || e.SyncActionValue == nil || e.GetFavoritesAction() == nil {
			return
		}
		b.favorites(e.GetFavoritesAction())
	default:
		return
	}
	b.dirty.Store(true)
}

// favorites takes the phone's list, the whole of it each time.
func (b *Bridge) favorites(a *waSyncAction.FavoritesAction) {
	b.db.Exec(`UPDATE chats SET favorite = 0, fav_id = ''`)
	for i, f := range a.GetFavorites() {
		j, err := types.ParseJID(f.GetID())
		if err != nil {
			continue
		}
		chat := b.norm(j)
		if skip(chat) {
			continue
		}
		b.db.Exec(`INSERT INTO chats (jid, favorite, fav_id) VALUES (?, ?, ?)
			ON CONFLICT (jid) DO UPDATE SET favorite = excluded.favorite, fav_id = excluded.fav_id`, chat.String(), i+1, f.GetID())
	}
}

// favID names a new favorite as the list names the others: by LID when
// they are, else as WhatsApp knows the chat.
func (b *Bridge) favID(chat string) string {
	var lids int
	b.db.QueryRow(`SELECT count(*) FROM chats WHERE fav_id LIKE '%@lid'`).Scan(&lids)
	jid, err := types.ParseJID(chat)
	if err == nil && lids > 0 && jid.Server == types.DefaultUserServer && b.stored() {
		if lid, err := b.cli.Store.LIDs.GetLIDForPN(b.ctx, jid); err == nil && !lid.IsEmpty() {
			return lid.ToNonAD().String()
		}
	}
	return b.waJID(chat).String()
}

func (b *Bridge) chatActions() {
	for c := range b.actions {
		b.chatAction(c)
	}
}

// chatAction is one choice of the chat menu: the database first and the
// list sent at once, then the patch to WhatsApp, so the phone and the other
// devices follow. If WhatsApp refuses it, the settings go back.
func (b *Bridge) chatAction(c Cmd) {
	chat := c.Chat
	if _, err := types.ParseJID(chat); err != nil {
		return
	}
	var pinned, mute, fav int64
	var archived, marked bool
	var unread int
	var favID string
	if b.db.QueryRow(`SELECT pinned, archived, mute_until, unread, marked, favorite, fav_id FROM chats WHERE jid = ?`, chat).
		Scan(&pinned, &archived, &mute, &unread, &marked, &fav, &favID) != nil {
		return
	}
	undo := func() {
		b.db.Exec(`UPDATE chats SET pinned = ?, archived = ?, mute_until = ?, unread = ?, marked = ?, favorite = ?, fav_id = ? WHERE jid = ?`,
			pinned, archived, mute, unread, marked, fav, favID, chat)
	}
	wa := b.waJID(chat)
	ts, key := b.lastKey(chat)
	now := time.Now()
	var patch appstate.PatchInfo
	what := ""
	switch c.Action {
	case "archive", "unarchive":
		on := c.Action == "archive"
		if on {
			b.db.Exec(`UPDATE chats SET archived = 1, pinned = 0 WHERE jid = ?`, chat)
		} else {
			b.db.Exec(`UPDATE chats SET archived = 0 WHERE jid = ?`, chat)
		}
		patch, what = appstate.BuildArchive(wa, on, ts, key), "não arquivou"
	case "pin", "unpin":
		on := c.Action == "pin"
		if on {
			var n int
			b.db.QueryRow(`SELECT count(*) FROM chats WHERE pinned > 0 AND jid != ?`, chat).Scan(&n)
			if n >= maxPinned {
				emit(map[string]any{"t": "error", "text": "Você só pode fixar até 3 conversas"})
				return
			}
			b.db.Exec(`UPDATE chats SET pinned = ? WHERE jid = ?`, now.Unix(), chat)
		} else {
			b.db.Exec(`UPDATE chats SET pinned = 0 WHERE jid = ?`, chat)
		}
		patch, what = appstate.BuildPin(wa, on), "não fixou"
	case "read":
		// Receipts and the patch, as opening it would.
		b.markRead(chat, c.Quiet, true)
		b.flush()
		return
	case "unread":
		b.db.Exec(`UPDATE chats SET marked = 1, unread = max(unread, 1) WHERE jid = ?`, chat)
		patch, what = appstate.BuildMarkChatAsRead(wa, false, ts, key), "não marcou como não lida"
	case "mute", "unmute":
		on := c.Action == "mute"
		until, dur := int64(0), time.Duration(0)
		if on {
			until = muteForever
			if c.Secs > 0 {
				dur = time.Duration(c.Secs) * time.Second
				until = now.Add(dur).Unix()
			}
		}
		b.db.Exec(`UPDATE chats SET mute_until = ? WHERE jid = ?`, until, chat)
		patch, what = appstate.BuildMute(wa, on, dur), "não silenciou"
	case "favorite", "unfavorite":
		if c.Action == "favorite" {
			b.db.Exec(`UPDATE chats SET favorite = (SELECT coalesce(max(favorite), 0) + 1 FROM chats), fav_id = ? WHERE jid = ?`, b.favID(chat), chat)
		} else {
			b.db.Exec(`UPDATE chats SET favorite = 0, fav_id = '' WHERE jid = ?`, chat)
		}
		version, _ := strconv.Atoi(b.meta("fav_version"))
		collection := b.meta("fav_patch")
		if version <= 0 || collection == "" {
			// No favorites came from the phone yet, so neither did the
			// version to write them with: here only, until they do.
			logf("favorites %s: kept here only, no version from the phone yet", chat)
			b.flush()
			return
		}
		var ids []string
		if rows, err := b.db.Query(`SELECT fav_id FROM chats WHERE favorite > 0 AND fav_id != '' ORDER BY favorite`); err == nil {
			for rows.Next() {
				var id string
				if rows.Scan(&id) == nil {
					ids = append(ids, id)
				}
			}
			rows.Close()
		}
		patch, what = buildFavorites(ids, int32(version), collection), "não mudou as favoritas"
	case "clear":
		end := max(ts.Unix(), now.Unix())
		b.forget(chat, end, false)
		patch, what, undo = buildClearChat(wa, ts, key), "não limpou a conversa", nil
	case "delete":
		end := max(ts.Unix(), now.Unix())
		b.forget(chat, end, true)
		patch, what, undo = appstate.BuildDeleteChat(wa, ts, key, false), "não apagou a conversa", nil
	default:
		return
	}
	b.flush()
	if err := b.cli.SendAppState(b.ctx, patch); err != nil {
		logf("chat %s %s: %v", c.Action, chat, err)
		emit(map[string]any{"t": "error", "text": what + ": " + err.Error()})
		if undo != nil {
			undo()
			b.flush()
		}
	}
}
