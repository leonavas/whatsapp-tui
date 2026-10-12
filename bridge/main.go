// whatsapp-tui-bridge: the WhatsApp side of whatsapp-tui. It speaks WhatsApp's multi-device
// protocol through whatsmeow and talks to the TUI over JSON lines: events on
// stdout, commands on stdin. Logs go to stderr.
//
// Usage: whatsapp-tui-bridge <data dir>
package main

import (
	"bufio"
	"context"
	"database/sql"
	"encoding/json"
	"errors"
	"fmt"
	"go.mau.fi/whatsmeow/proto/waCompanionReg"
	"io"
	"net/http"
	"os"
	"path/filepath"
	"sort"
	"strings"
	"sync"
	"sync/atomic"
	"time"

	_ "github.com/mattn/go-sqlite3"
	"github.com/rs/zerolog"
	"go.mau.fi/whatsmeow"
	"go.mau.fi/whatsmeow/appstate"
	"go.mau.fi/whatsmeow/proto/waE2E"
	"go.mau.fi/whatsmeow/proto/waHistorySync"
	"go.mau.fi/whatsmeow/store"
	"go.mau.fi/whatsmeow/store/sqlstore"
	"go.mau.fi/whatsmeow/types"
	"go.mau.fi/whatsmeow/types/events"
	waLog "go.mau.fi/whatsmeow/util/log"
	"google.golang.org/protobuf/proto"
)

// Msg is one message as the TUI sees it.
type Msg struct {
	Chat       string `json:"chat"`
	ID         string `json:"id"`
	Sender     string `json:"sender"`
	SenderName string `json:"sender_name"`
	FromMe     bool   `json:"from_me"`
	TS         int64  `json:"ts"`
	Text       string `json:"text"`
	Quote      string `json:"quote,omitempty"`
	// A photo, video, audio, document or sticker; nil for the rest.
	Media *Media `json:"media,omitempty"`
	// 0 pending, 1 sent, 2 delivered, 3 read.
	Status int `json:"status"`
	// Stored just now, not a new version of one we had.
	fresh bool
}

// Chat is one row of the sidebar.
type Chat struct {
	JID    string `json:"jid"`
	Name   string `json:"name"`
	Group  bool   `json:"group"`
	LastTS int64  `json:"last_ts"`
	Unread int    `json:"unread"`
	Pinned bool   `json:"pinned"`
	// When it was pinned: the newest pin comes first.
	PinTS    int64  `json:"pin_ts"`
	Archived bool   `json:"archived"`
	Muted    bool   `json:"muted"`
	Favorite bool   `json:"favorite"`
	Preview  string `json:"preview"`
	FromMe   bool   `json:"from_me"`
	// The last message's, when it's ours: 0 pending … 3 read.
	Status int `json:"status"`
	// Why nothing can be sent here; empty when it can.
	ReadOnly string `json:"read_only"`
}

type Cmd struct {
	C     string `json:"c"`
	Chat  string `json:"chat"`
	Text  string `json:"text"`
	Limit int    `json:"limit"`
	// download: the message whose media to fetch.
	ID string `json:"id"`
	// send_file: a file on disk, its name as sent, and what it goes as
	// (image, video, audio, document); Temp ones are removed once sent.
	Path    string `json:"path"`
	Name    string `json:"name"`
	Caption string `json:"caption"`
	Kind    string `json:"kind"`
	Temp    bool   `json:"temp"`
	// read: without read receipts, only your other devices hear of it.
	Quiet bool `json:"quiet"`
	// chat_action: archive, unarchive, pin, unpin, read, unread, mute
	// (for Secs, 0 is forever), unmute, favorite, unfavorite, clear, delete.
	Action string `json:"action"`
	Secs   int64  `json:"secs"`
}

var (
	outMu sync.Mutex
	enc   = json.NewEncoder(os.Stdout)
)

func emit(v map[string]any) {
	outMu.Lock()
	defer outMu.Unlock()
	_ = enc.Encode(v)
}

// stderrLog is whatsmeow's logger, on stderr: stdout belongs to the protocol.
type stderrLog struct {
	mod string
	min int
}

var levels = map[string]int{"DEBUG": 0, "INFO": 1, "WARN": 2, "ERROR": 3}

func (l stderrLog) log(lvl, msg string, args ...any) {
	if levels[lvl] < l.min {
		return
	}
	fmt.Fprintf(os.Stderr, "%s [%s %s] %s\n", time.Now().Format("15:04:05.000"), l.mod, lvl, fmt.Sprintf(msg, args...))
}
func (l stderrLog) Debugf(m string, a ...any)   { l.log("DEBUG", m, a...) }
func (l stderrLog) Infof(m string, a ...any)    { l.log("INFO", m, a...) }
func (l stderrLog) Warnf(m string, a ...any)    { l.log("WARN", m, a...) }
func (l stderrLog) Errorf(m string, a ...any)   { l.log("ERROR", m, a...) }
func (l stderrLog) Sub(mod string) waLog.Logger { return stderrLog{l.mod + "/" + mod, l.min} }
func newLog(mod string, min string) stderrLog   { return stderrLog{mod, levels[min]} }

type Bridge struct {
	cli     *whatsmeow.Client
	db      *sql.DB
	ctx     context.Context
	dir     string
	dirty   atomic.Bool
	avatars chan avatarReq
	// Chat actions, one at a time and in order: a pin and its undo can't swap.
	actions chan Cmd
}

type avatarReq struct {
	jid   string
	force bool
}

// What the TUI says where nothing can be sent.
const (
	officialOnly = "Somente o WhatsApp pode enviar mensagens"
	adminsOnly   = "Somente admins podem enviar mensagens"
	notMember    = "Você não pode enviar mensagens para este grupo porque não é mais participante"
	cantSend     = "Não é possível enviar mensagens para esta conversa"
)

const schema = `
CREATE TABLE IF NOT EXISTS chats (
	jid TEXT PRIMARY KEY,
	name TEXT NOT NULL DEFAULT '',
	last_ts INTEGER NOT NULL DEFAULT 0,
	unread INTEGER NOT NULL DEFAULT 0,
	pinned INTEGER NOT NULL DEFAULT 0,
	archived INTEGER NOT NULL DEFAULT 0,
	mute_until INTEGER NOT NULL DEFAULT 0,
	read_only TEXT NOT NULL DEFAULT '',
	-- The JID WhatsApp knows the chat by (maybe a LID), for app state patches.
	wa_jid TEXT NOT NULL DEFAULT '',
	-- Its place among the favorites, from 1; 0 when it isn't one. fav_id is
	-- how WhatsApp's list names it.
	favorite INTEGER NOT NULL DEFAULT 0,
	fav_id TEXT NOT NULL DEFAULT '',
	-- Marked as unread by hand.
	marked INTEGER NOT NULL DEFAULT 0,
	-- Cleared: listed without messages until one comes.
	cleared INTEGER NOT NULL DEFAULT 0
);
CREATE TABLE IF NOT EXISTS messages (
	chat TEXT NOT NULL,
	id TEXT NOT NULL,
	sender TEXT NOT NULL,
	push_name TEXT NOT NULL DEFAULT '',
	from_me INTEGER NOT NULL,
	ts INTEGER NOT NULL,
	text TEXT NOT NULL,
	quote TEXT NOT NULL DEFAULT '',
	status INTEGER NOT NULL DEFAULT 0,
	-- The sender as WhatsApp has it (maybe a LID), for message keys.
	wa_sender TEXT NOT NULL DEFAULT '',
	PRIMARY KEY (chat, id)
);
CREATE INDEX IF NOT EXISTS messages_chat_ts ON messages (chat, ts);
CREATE TABLE IF NOT EXISTS media (
	chat TEXT NOT NULL,
	id TEXT NOT NULL,
	kind TEXT NOT NULL,
	mime TEXT NOT NULL DEFAULT '',
	seconds INTEGER NOT NULL DEFAULT 0,
	ptt INTEGER NOT NULL DEFAULT 0,
	-- A waE2E.Message with only the media part, keys and path: enough to download it.
	proto BLOB,
	-- The chat and sender as WhatsApp has them (maybe LIDs), to ask the phone for a new upload.
	wa_chat TEXT NOT NULL DEFAULT '',
	wa_sender TEXT NOT NULL DEFAULT '',
	PRIMARY KEY (chat, id)
);
CREATE TABLE IF NOT EXISTS meta (
	k TEXT PRIMARY KEY,
	v TEXT NOT NULL
);
`

func main() {
	if len(os.Args) < 2 {
		fmt.Fprintln(os.Stderr, "usage: whatsapp-tui-bridge <data dir>")
		os.Exit(2)
	}
	dir := os.Args[1]
	if err := os.MkdirAll(dir, 0o700); err != nil {
		fatal(err)
	}
	ctx := context.Background()

	// How the phone lists it under Linked devices: the name, and a computer
	// icon (unknown platforms get a generic one). Read only when pairing.
	store.SetOSInfo("WhatsApp TUI", [3]uint32{0, 1, 0})
	store.DeviceProps.PlatformType = waCompanionReg.DeviceProps_DESKTOP.Enum()
	container, err := sqlstore.New(ctx, "sqlite3", "file:"+filepath.Join(dir, "session.db")+"?_foreign_keys=on&_busy_timeout=5000", newLog("db", "WARN"))
	if err != nil {
		fatal(err)
	}
	device, err := container.GetFirstDevice(ctx)
	if err != nil {
		fatal(err)
	}
	db, err := sql.Open("sqlite3", "file:"+filepath.Join(dir, "whatsapp-tui.db")+"?_busy_timeout=5000&_journal_mode=WAL")
	if err != nil {
		fatal(err)
	}
	if _, err := db.Exec(schema); err != nil {
		fatal(err)
	}
	// Databases from before read_only existed.
	db.Exec(`ALTER TABLE chats ADD COLUMN read_only TEXT NOT NULL DEFAULT ''`)
	db.Exec(`UPDATE chats SET read_only = ? WHERE jid = ?`, officialOnly, types.PSAJID.String())
	// And from before the chat menu.
	for _, col := range []string{
		`chats ADD COLUMN wa_jid TEXT NOT NULL DEFAULT ''`,
		`chats ADD COLUMN favorite INTEGER NOT NULL DEFAULT 0`,
		`chats ADD COLUMN fav_id TEXT NOT NULL DEFAULT ''`,
		`chats ADD COLUMN marked INTEGER NOT NULL DEFAULT 0`,
		`chats ADD COLUMN cleared INTEGER NOT NULL DEFAULT 0`,
		`messages ADD COLUMN wa_sender TEXT NOT NULL DEFAULT ''`,
	} {
		db.Exec(`ALTER TABLE ` + col)
	}

	b := &Bridge{cli: whatsmeow.NewClient(device, newLog("wa", "INFO")), db: db, dir: dir, avatars: make(chan avatarReq, 4096), actions: make(chan Cmd, 64)}
	// whatsmeow's context logger, for the favorites' version (see favLog):
	// each app state mutation logged at debug, zerolog's lowest by default.
	b.ctx = zerolog.New(favLog{b}).Level(zerolog.DebugLevel).WithContext(ctx)
	b.cli.BackgroundEventCtx = b.ctx
	b.cli.AppStateDebugLogs = true
	// Pins, archives, mutes and favorites also from the first full sync, not
	// only from the changes after it.
	b.cli.EmitAppStateEventsOnFullSync = true
	b.cli.AddEventHandler(b.handle)
	if b.cli.Store.ID == nil {
		b.wipe()
	}
	go b.flushChats()
	go b.chatActions()
	for range 3 {
		go b.fetchAvatars()
	}
	b.dirty.Store(true)

	if b.cli.Store.ID == nil {
		go b.pair()
	} else {
		emit(map[string]any{"t": "connecting", "name": b.cli.Store.PushName, "me": b.cli.Store.ID.User})
		if err := b.cli.Connect(); err != nil {
			emit(map[string]any{"t": "error", "text": err.Error()})
		}
	}
	b.commands()
}

func fatal(err error) {
	emit(map[string]any{"t": "error", "text": err.Error(), "fatal": true})
	os.Exit(1)
}

// pair shows QR codes until the phone scans one, starting over when a round
// of codes runs out.
func (b *Bridge) pair() {
	for {
		qr, err := b.cli.GetQRChannel(b.ctx)
		if err != nil {
			fatal(err)
		}
		if err := b.cli.Connect(); err != nil {
			fatal(err)
		}
		for item := range qr {
			switch item.Event {
			case "code":
				emit(map[string]any{"t": "qr", "code": item.Code})
			case "success":
				// A new link syncs the app state in full, events and all.
				b.setMeta("resynced", "1")
				emit(map[string]any{"t": "paired"})
				return
			case "timeout":
			default:
				if item.Error != nil {
					emit(map[string]any{"t": "error", "text": item.Error.Error()})
				}
			}
		}
		b.cli.Disconnect()
	}
}

func (b *Bridge) commands() {
	in := bufio.NewScanner(os.Stdin)
	in.Buffer(make([]byte, 1<<20), 16<<20)
	for in.Scan() {
		var c Cmd
		if err := json.Unmarshal(in.Bytes(), &c); err != nil {
			continue
		}
		switch c.C {
		case "chats":
			b.dirty.Store(true)
		case "history":
			b.history(c.Chat, c.Limit)
		case "unread":
			go b.unread(c.Chat, c.Limit)
		case "send":
			go b.send(c.Chat, c.Text)
		case "send_file":
			b.sendFile(c)
		case "read":
			go b.markRead(c.Chat, c.Quiet, false)
		case "chat_action":
			b.actions <- c
		case "search":
			go b.search(c.Text)
		case "profile":
			go b.profile(c.Chat)
		case "avatar":
			select {
			case b.avatars <- avatarReq{jid: c.Chat}:
			default:
			}
		case "download":
			go b.download(c.Chat, c.ID)
		case "logout":
			// Unlinks this device on the phone too; offline, nothing changes.
			if err := b.cli.Logout(b.ctx); err != nil {
				emit(map[string]any{"t": "error", "text": "não desconectou: " + err.Error()})
				continue
			}
			b.wipe()
			emit(map[string]any{"t": "logged_out"})
			os.Exit(0)
		}
	}
	// The TUI went away.
	b.cli.Disconnect()
}

func (b *Bridge) handle(raw any) {
	switch e := raw.(type) {
	case *events.Connected:
		emit(map[string]any{"t": "ready", "me": b.cli.Store.ID.User, "name": b.cli.Store.PushName})
		go b.syncGroups()
		go b.resyncOnce()
	case *events.LoggedOut:
		_ = b.cli.Store.Delete(b.ctx)
		b.wipe()
		emit(map[string]any{"t": "logged_out"})
		os.Exit(0)
	case *events.HistorySync:
		b.historySync(e)
	case *events.Message:
		if m := b.message(e, 1); m != nil {
			// Someone else's message we didn't have: edits, deletions and
			// what the phone resends are not news.
			news := m.fresh && !m.FromMe && e.UnavailableRequestID == ""
			if e.UnavailableRequestID != "" {
				logf("recover: phone resent %s %s (request %s, media %v)", m.Chat, m.ID, e.UnavailableRequestID, m.Media != nil)
			} else if news {
				b.db.Exec(`UPDATE chats SET unread = unread + 1 WHERE jid = ?`, m.Chat)
				b.unarchiveOnNews(m.Chat)
			}
			emit(map[string]any{"t": "message", "msg": m, "new": news})
		}
	case *events.Receipt:
		b.receipt(e)
	case *events.MediaRetry:
		go b.mediaRetry(e)
	case *events.ChatPresence:
		chat := b.norm(e.Chat).String()
		emit(map[string]any{
			"t": "typing", "chat": chat,
			"who": b.senderName(b.norm(e.Sender), ""),
			"on":  e.State == types.ChatPresenceComposing,
		})
	case *events.GroupInfo:
		go b.syncGroups()
	case *events.Picture:
		select {
		case b.avatars <- avatarReq{jid: b.norm(e.JID).String(), force: true}:
		default:
		}
	case *events.PushName, *events.Contact:
		b.dirty.Store(true)
	default:
		b.appStateEvent(raw)
	}
}

// norm turns a LID (WhatsApp's hidden user ids) into the phone number JID
// when the mapping is known, so one person is one chat.
func (b *Bridge) norm(j types.JID) types.JID {
	j = j.ToNonAD()
	if j.Server == types.HiddenUserServer && b.stored() {
		if pn, err := b.cli.Store.LIDs.GetPNForLID(b.ctx, j); err == nil && !pn.IsEmpty() {
			return pn.ToNonAD()
		}
	}
	return j
}

func (b *Bridge) me() types.JID {
	if b.cli.Store.ID == nil {
		return types.EmptyJID
	}
	return b.cli.Store.ID.ToNonAD()
}

// wipe forgets the account: its chats, messages, media keys, pictures and
// downloads. For a logout, from here or from the phone, and for a start
// without a session, so a new one never shows the last one's chats.
func (b *Bridge) wipe() {
	for _, t := range []string{"chats", "messages", "media", "meta"} {
		b.db.Exec("DELETE FROM " + t)
	}
	b.db.Exec("PRAGMA wal_checkpoint(TRUNCATE)")
	b.db.Exec("VACUUM")
	dirs := []string{filepath.Join(b.dir, "avatars")}
	if cache, err := os.UserCacheDir(); err == nil {
		dirs = append(dirs, filepath.Join(cache, "whatsapp-tui", "media"))
	}
	// The folders stay: the avatar workers write into theirs.
	for _, dir := range dirs {
		entries, _ := os.ReadDir(dir)
		for _, e := range entries {
			os.RemoveAll(filepath.Join(dir, e.Name()))
		}
	}
}

// stored says the device has its stores: a fresh one (never paired, or just
// logged out) has none until pairing saves it, and chats left in the database
// from before must not reach them.
func (b *Bridge) stored() bool {
	return b.cli.Store.Contacts != nil && b.cli.Store.LIDs != nil
}

func (b *Bridge) contactName(j types.JID) string {
	if !b.stored() {
		return ""
	}
	if c, err := b.cli.Store.Contacts.GetContact(b.ctx, j); err == nil && c.Found {
		for _, n := range []string{c.FullName, c.FirstName, c.BusinessName, c.PushName} {
			if n != "" {
				return n
			}
		}
	}
	return ""
}

func (b *Bridge) senderName(j types.JID, push string) string {
	if j.User == b.me().User {
		return "Você"
	}
	if n := b.contactName(j); n != "" {
		return n
	}
	if push != "" {
		return "~" + push
	}
	if j.Server == types.DefaultUserServer {
		return "+" + j.User
	}
	return j.User
}

func skip(chat types.JID) bool {
	return chat.Server == types.BroadcastServer || chat.Server == types.NewsletterServer || chat.IsEmpty()
}

// message stores a received (or history) message and returns it, or nil
// when there's nothing to show. Edits and deletions update the stored one.
func (b *Bridge) message(e *events.Message, status int) *Msg {
	chat := b.norm(e.Info.Chat)
	if skip(chat) {
		return nil
	}
	if pm := e.Message.GetProtocolMessage(); pm != nil {
		var text string
		switch pm.GetType() {
		case waE2E.ProtocolMessage_REVOKE:
			text = "🚫 Mensagem apagada"
			b.db.Exec(`DELETE FROM media WHERE chat = ? AND id = ?`, chat.String(), pm.GetKey().GetID())
		case waE2E.ProtocolMessage_MESSAGE_EDIT:
			t, ok := textOf(pm.GetEditedMessage())
			if !ok {
				return nil
			}
			text = t + " (editada)"
		default:
			return nil
		}
		id := pm.GetKey().GetID()
		if _, err := b.db.Exec(`UPDATE messages SET text = ? WHERE chat = ? AND id = ?`, text, chat.String(), id); err != nil {
			return nil
		}
		return b.load(chat.String(), id)
	}
	text, ok := textOf(e.Message)
	if !ok {
		return nil
	}
	sender := b.norm(e.Info.Sender)
	if e.Info.IsFromMe {
		sender = b.me()
	}
	quote := ""
	if q := contextOf(e.Message).GetQuotedMessage(); q != nil {
		quote, _ = textOf(q)
	}
	m := &Msg{
		Chat: chat.String(), ID: e.Info.ID, Sender: sender.String(), FromMe: e.Info.IsFromMe,
		TS: e.Info.Timestamp.Unix(), Text: text, Quote: quote, Status: status,
	}
	var had int
	m.fresh = b.db.QueryRow(`SELECT 1 FROM messages WHERE chat = ? AND id = ?`, m.Chat, m.ID).Scan(&had) != nil
	_, err := b.db.Exec(`INSERT INTO messages (chat, id, sender, push_name, from_me, ts, text, quote, status, wa_sender)
		VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
		ON CONFLICT (chat, id) DO UPDATE SET text = excluded.text, status = max(status, excluded.status)`,
		m.Chat, m.ID, m.Sender, e.Info.PushName, m.FromMe, m.TS, m.Text, m.Quote, m.Status, e.Info.Sender.ToNonAD().String())
	if err != nil {
		return nil
	}
	// What WhatsApp calls the chat, unless its app state already said.
	b.db.Exec(`INSERT INTO chats (jid, last_ts, wa_jid) VALUES (?, ?, ?)
		ON CONFLICT (jid) DO UPDATE SET last_ts = max(last_ts, excluded.last_ts),
			wa_jid = CASE WHEN wa_jid = '' THEN excluded.wa_jid ELSE wa_jid END`, m.Chat, m.TS, e.Info.Chat.ToNonAD().String())
	if md, part := mediaOf(e.Message); md != nil {
		b.storeMedia(m.Chat, m.ID, md, part, e.Info.Chat.String(), e.Info.Sender.String())
		m.Media = md
	}
	m.SenderName = b.senderName(sender, e.Info.PushName)
	b.dirty.Store(true)
	return m
}

func contextOf(m *waE2E.Message) *waE2E.ContextInfo {
	switch {
	case m.GetExtendedTextMessage() != nil:
		return m.GetExtendedTextMessage().GetContextInfo()
	case m.GetImageMessage() != nil:
		return m.GetImageMessage().GetContextInfo()
	case m.GetVideoMessage() != nil:
		return m.GetVideoMessage().GetContextInfo()
	case m.GetAudioMessage() != nil:
		return m.GetAudioMessage().GetContextInfo()
	case m.GetStickerMessage() != nil:
		return m.GetStickerMessage().GetContextInfo()
	}
	return nil
}

func withCaption(label, caption string) string {
	if caption == "" {
		return label
	}
	return label + " · " + caption
}

// textOf is how a message reads in the TUI; false for the ones that aren't
// shown (reactions, key exchanges, protocol messages).
func textOf(m *waE2E.Message) (string, bool) {
	switch {
	case m == nil:
		return "", false
	case m.GetConversation() != "":
		return m.GetConversation(), true
	case m.GetExtendedTextMessage() != nil:
		return m.GetExtendedTextMessage().GetText(), true
	case m.GetImageMessage() != nil:
		return withCaption("📷 Foto", m.GetImageMessage().GetCaption()), true
	case m.GetVideoMessage() != nil:
		return withCaption("🎥 Vídeo", m.GetVideoMessage().GetCaption()), true
	case m.GetAudioMessage() != nil:
		s := m.GetAudioMessage().GetSeconds()
		return fmt.Sprintf("🎤 Áudio %d:%02d", s/60, s%60), true
	case m.GetDocumentMessage() != nil:
		d := m.GetDocumentMessage()
		return withCaption("📄 "+d.GetFileName(), d.GetCaption()), true
	case m.GetStickerMessage() != nil:
		return "💟 Figurinha", true
	case m.GetContactMessage() != nil:
		return "👤 " + m.GetContactMessage().GetDisplayName(), true
	case m.GetLocationMessage() != nil:
		return "📍 Localização", true
	case m.GetLiveLocationMessage() != nil:
		return "📍 Localização em tempo real", true
	case m.GetPollCreationMessage() != nil:
		return "📊 " + m.GetPollCreationMessage().GetName(), true
	case m.GetPollCreationMessageV3() != nil:
		return "📊 " + m.GetPollCreationMessageV3().GetName(), true
	}
	return "", false
}

func (b *Bridge) historySync(e *events.HistorySync) {
	// An answer to recoverMedia: only its messages count, not the chat's
	// state in it (unread, archived, names), nor any progress to show.
	onDemand := e.Data.GetSyncType() == waHistorySync.HistorySync_ON_DEMAND
	if onDemand {
		logf("recover: on-demand history with %d chats", len(e.Data.GetConversations()))
	}
	for _, conv := range e.Data.GetConversations() {
		jid, err := types.ParseJID(conv.GetID())
		if err != nil {
			continue
		}
		chat := b.norm(jid)
		if skip(chat) {
			continue
		}
		if onDemand {
			logf("recover: on-demand %s: %d messages", conv.GetID(), len(conv.GetMessages()))
			for _, hm := range conv.GetMessages() {
				if evt, err := b.cli.ParseWebMessage(jid, hm.GetMessage()); err == nil {
					b.message(evt, 1)
				}
			}
			continue
		}
		// Forever comes as -1 (all bits set); milliseconds at times.
		mute := int64(conv.GetMuteEndTime())
		if mute > 1e12 {
			mute /= 1000
		}
		if mute < 0 {
			mute = muteForever
		}
		unread, marked := conv.GetUnreadCount(), conv.GetMarkedAsUnread()
		if marked {
			unread = max(unread, 1)
		}
		readOnly := ""
		switch {
		case chat == types.PSAJID:
			readOnly = officialOnly
		case conv.GetReadOnly() && chat.Server == types.GroupServer:
			readOnly = notMember
		case conv.GetReadOnly():
			readOnly = cantSend
		}
		// pinned is when it was pinned.
		b.db.Exec(`INSERT INTO chats (jid, name, last_ts, unread, pinned, archived, mute_until, read_only, wa_jid, marked) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
			ON CONFLICT (jid) DO UPDATE SET
				name = CASE WHEN excluded.name != '' THEN excluded.name ELSE name END,
				last_ts = max(last_ts, excluded.last_ts), unread = excluded.unread,
				pinned = excluded.pinned, archived = excluded.archived, mute_until = excluded.mute_until,
				read_only = excluded.read_only, wa_jid = excluded.wa_jid, marked = excluded.marked`,
			chat.String(), conv.GetName(), int64(conv.GetConversationTimestamp()), unread,
			int64(conv.GetPinned()), conv.GetArchived(), mute, readOnly, jid.ToNonAD().String(), marked)
		for _, hm := range conv.GetMessages() {
			web := hm.GetMessage()
			evt, err := b.cli.ParseWebMessage(jid, web)
			if err != nil {
				continue
			}
			// WebMessageInfo: 2 server ack, 3 delivered, 4 read, 5 played.
			status := 1
			switch s := int(web.GetStatus()); {
			case s >= 4:
				status = 3
			case s == 3:
				status = 2
			}
			b.message(evt, status)
		}
	}
	if !onDemand {
		emit(map[string]any{"t": "sync", "progress": e.Data.GetProgress()})
	}
	b.dirty.Store(true)
}

func (b *Bridge) receipt(e *events.Receipt) {
	chat := b.norm(e.Chat)
	switch e.Type {
	case types.ReceiptTypeReadSelf:
		// Read on another device: no longer unread here either.
		b.db.Exec(`UPDATE chats SET unread = 0 WHERE jid = ?`, chat.String())
		emit(map[string]any{"t": "read", "chat": chat.String()})
		return
	case types.ReceiptTypeDelivered:
		b.setStatus(chat, e.MessageIDs, 2)
	case types.ReceiptTypeRead, types.ReceiptTypePlayed:
		// In groups a read from one member isn't "read"; v0 settles for delivered.
		if e.IsGroup {
			b.setStatus(chat, e.MessageIDs, 2)
		} else {
			b.setStatus(chat, e.MessageIDs, 3)
		}
	}
}

func (b *Bridge) setStatus(chat types.JID, ids []types.MessageID, status int) {
	for _, id := range ids {
		b.db.Exec(`UPDATE messages SET status = max(status, ?) WHERE chat = ? AND id = ? AND from_me = 1`, status, chat.String(), id)
	}
	emit(map[string]any{"t": "status", "chat": chat.String(), "ids": ids, "status": status})
}

// syncGroups takes the names of the groups we're in, and who may send in
// each: groups set to admins only, and the ones we left, are read-only.
func (b *Bridge) syncGroups() {
	groups, err := b.cli.GetJoinedGroups(b.ctx)
	if err != nil {
		return
	}
	me, myLID := b.me(), b.cli.Store.GetLID().ToNonAD()
	joined := map[string]bool{}
	for _, g := range groups {
		readOnly := ""
		if g.IsAnnounce {
			readOnly = adminsOnly
			for _, p := range g.Participants {
				mine := p.JID.User == me.User || p.PhoneNumber.User == me.User || (!myLID.IsEmpty() && p.LID.User == myLID.User)
				if mine && (p.IsAdmin || p.IsSuperAdmin) {
					readOnly = ""
				}
			}
		}
		joined[g.JID.String()] = true
		b.db.Exec(`INSERT INTO chats (jid, name, read_only) VALUES (?, ?, ?)
			ON CONFLICT (jid) DO UPDATE SET name = excluded.name, read_only = excluded.read_only`, g.JID.String(), g.Name, readOnly)
	}
	rows, err := b.db.Query(`SELECT jid FROM chats WHERE jid LIKE '%@g.us'`)
	if err == nil {
		var left []string
		for rows.Next() {
			var j string
			if rows.Scan(&j) == nil && !joined[j] {
				left = append(left, j)
			}
		}
		rows.Close()
		for _, j := range left {
			b.db.Exec(`UPDATE chats SET read_only = ? WHERE jid = ?`, notMember, j)
		}
	}
	b.dirty.Store(true)
}

// fetchAvatars downloads profile pictures (the small preview) one at a time
// into <data>/avatars, at most once a day each, and tells the TUI where they
// are; an empty path means there's no picture to show.
func (b *Bridge) fetchAvatars() {
	dir := filepath.Join(b.dir, "avatars")
	os.MkdirAll(dir, 0o700)
	for req := range b.avatars {
		name := strings.NewReplacer("@", "_", ".", "_", ":", "_").Replace(req.jid)
		pic, none := filepath.Join(dir, name+".jpg"), filepath.Join(dir, name+".none")
		fresh := func(p string) bool {
			st, err := os.Stat(p)
			return err == nil && time.Since(st.ModTime()) < 24*time.Hour
		}
		if !req.force {
			if fresh(pic) {
				emit(map[string]any{"t": "avatar", "chat": req.jid, "path": pic})
				continue
			}
			if fresh(none) {
				emit(map[string]any{"t": "avatar", "chat": req.jid, "path": ""})
				continue
			}
		}
		for !b.cli.IsConnected() {
			time.Sleep(time.Second)
		}
		jid, err := types.ParseJID(req.jid)
		// The official account never answers: it gets its logo in the TUI.
		if err != nil || jid == types.PSAJID {
			emit(map[string]any{"t": "avatar", "chat": req.jid, "path": ""})
			continue
		}
		ctx, cancel := context.WithTimeout(b.ctx, 15*time.Second)
		info, err := b.cli.GetProfilePictureInfo(ctx, jid, &whatsmeow.GetProfilePictureParams{Preview: true})
		cancel()
		hidden := errors.Is(err, whatsmeow.ErrProfilePictureNotSet) || errors.Is(err, whatsmeow.ErrProfilePictureUnauthorized)
		if err != nil && !hidden {
			fmt.Fprintf(os.Stderr, "%s avatar %s: %v\n", time.Now().Format("15:04:05.000"), req.jid, err)
		}
		if err != nil || info == nil {
			if err == nil || hidden {
				os.Remove(pic)
				os.WriteFile(none, nil, 0o600)
				emit(map[string]any{"t": "avatar", "chat": req.jid, "path": ""})
			}
			continue
		}
		if err := download(info.URL, pic); err != nil {
			fmt.Fprintln(os.Stderr, "avatar:", req.jid, err)
			continue
		}
		os.Remove(none)
		emit(map[string]any{"t": "avatar", "chat": req.jid, "path": pic})
		time.Sleep(150 * time.Millisecond)
	}
}

func download(url, path string) error {
	resp, err := http.Get(url)
	if err != nil {
		return err
	}
	defer resp.Body.Close()
	if resp.StatusCode != http.StatusOK {
		return fmt.Errorf("HTTP %d", resp.StatusCode)
	}
	tmp := path + ".tmp"
	f, err := os.Create(tmp)
	if err != nil {
		return err
	}
	if _, err := io.Copy(f, resp.Body); err != nil {
		f.Close()
		return err
	}
	f.Close()
	return os.Rename(tmp, path)
}

// flushChats sends the whole chat list when something changed, at most a
// few times a second (a history sync changes it hundreds of times).
func (b *Bridge) flushChats() {
	for range time.Tick(500 * time.Millisecond) {
		if b.dirty.Swap(false) {
			emit(map[string]any{"t": "chats", "chats": b.chats()})
		}
	}
}

// chats lists the conversations that have messages, as WhatsApp does: the
// groups we're in and the contacts we know stay out until someone writes.
func (b *Bridge) chats() []Chat {
	// Cleared ones stay, empty, as on the phone.
	rows, err := b.db.Query(`SELECT c.jid, c.name, c.last_ts, c.unread, c.pinned, c.archived, c.mute_until, c.read_only, c.favorite,
		COALESCE(m.text, ''), COALESCE(m.from_me, 0), COALESCE(m.status, 0)
		FROM chats c LEFT JOIN messages m ON m.rowid = (
			SELECT rowid FROM messages WHERE chat = c.jid ORDER BY ts DESC LIMIT 1)
		WHERE m.rowid IS NOT NULL OR c.cleared > 0`)
	if err != nil {
		return nil
	}
	defer rows.Close()
	now := time.Now().Unix()
	out := []Chat{}
	for rows.Next() {
		var c Chat
		var mute, fav int64
		if rows.Scan(&c.JID, &c.Name, &c.LastTS, &c.Unread, &c.PinTS, &c.Archived, &mute, &c.ReadOnly, &fav, &c.Preview, &c.FromMe, &c.Status) != nil {
			continue
		}
		jid, _ := types.ParseJID(c.JID)
		c.Group = jid.Server == types.GroupServer
		c.Muted = mute == muteForever || mute > now
		c.Pinned = c.PinTS > 0
		c.Favorite = fav > 0
		if !c.Group {
			if jid.User == b.me().User {
				c.Name = "Você (mensagens para si)"
			} else if n := b.contactName(jid); n != "" {
				c.Name = n
			} else if c.Name == "" {
				c.Name = b.senderName(jid, "")
			}
		} else if c.Name == "" {
			c.Name = "Grupo"
		}
		out = append(out, c)
	}
	// Pinned first, the last pinned on top, then by the newest message.
	sort.SliceStable(out, func(i, j int) bool {
		if out[i].PinTS != out[j].PinTS {
			return out[i].PinTS > out[j].PinTS
		}
		return out[i].LastTS > out[j].LastTS
	})
	return out
}

func (b *Bridge) load(chat, id string) *Msg {
	ms := b.query(`WHERE chat = ? AND id = ?`, chat, id)
	if len(ms) == 0 {
		return nil
	}
	return &ms[0]
}

func (b *Bridge) query(where string, args ...any) []Msg {
	// Media's columns don't clash with what callers filter on: chat and id are the join's.
	rows, err := b.db.Query(`SELECT chat, id, sender, push_name, from_me, ts, text, quote, status,
		COALESCE(kind, ''), COALESCE(mime, ''), COALESCE(seconds, 0), COALESCE(ptt, 0)
		FROM messages LEFT JOIN media USING (chat, id) `+where, args...)
	if err != nil {
		return nil
	}
	defer rows.Close()
	names := map[string]string{}
	var out []Msg
	for rows.Next() {
		var m Msg
		var push string
		var md Media
		if rows.Scan(&m.Chat, &m.ID, &m.Sender, &push, &m.FromMe, &m.TS, &m.Text, &m.Quote, &m.Status, &md.Kind, &md.Mime, &md.Seconds, &md.PTT) != nil {
			continue
		}
		if md.Kind != "" {
			m.Media = &md
		}
		key := m.Sender + "\x00" + push
		if n, ok := names[key]; ok {
			m.SenderName = n
		} else {
			j, _ := types.ParseJID(m.Sender)
			m.SenderName = b.senderName(j, push)
			names[key] = m.SenderName
		}
		out = append(out, m)
	}
	return out
}

func (b *Bridge) history(chat string, limit int) {
	if limit <= 0 {
		limit = 300
	}
	ms := b.query(`WHERE chat = ? ORDER BY ts DESC LIMIT ?`, chat, limit)
	for i, j := 0, len(ms)-1; i < j; i, j = i+1, j-1 {
		ms[i], ms[j] = ms[j], ms[i]
	}
	if ms == nil {
		ms = []Msg{}
	}
	emit(map[string]any{"t": "history", "chat": chat, "msgs": ms})
}

// unread answers with a chat's newest messages from other people, up to
// `limit` and no more than it has unread, oldest first: the preview other
// programs show.
func (b *Bridge) unread(chat string, limit int) {
	var n int
	b.db.QueryRow(`SELECT unread FROM chats WHERE jid = ?`, chat).Scan(&n)
	ms := []Msg{}
	if n = min(n, limit); n > 0 {
		ms = b.query(`WHERE chat = ? AND from_me = 0 ORDER BY ts DESC LIMIT ?`, chat, n)
		for i, j := 0, len(ms)-1; i < j; i, j = i+1, j-1 {
			ms[i], ms[j] = ms[j], ms[i]
		}
	}
	emit(map[string]any{"t": "unread", "chat": chat, "msgs": ms})
}

// send shows the message right away (pending) and again once the server took it.
func (b *Bridge) send(chat, text string) {
	text = strings.TrimSpace(text)
	jid, err := types.ParseJID(chat)
	if err != nil || text == "" {
		return
	}
	var readOnly string
	if b.db.QueryRow(`SELECT read_only FROM chats WHERE jid = ?`, chat).Scan(&readOnly) == nil && readOnly != "" {
		emit(map[string]any{"t": "error", "text": readOnly})
		return
	}
	id := b.cli.GenerateMessageID()
	me := b.me()
	m := &Msg{Chat: chat, ID: id, Sender: me.String(), SenderName: "Você", FromMe: true, TS: time.Now().Unix(), Text: text}
	b.db.Exec(`INSERT INTO messages (chat, id, sender, from_me, ts, text, status) VALUES (?, ?, ?, 1, ?, ?, 0)`, chat, id, m.Sender, m.TS, text)
	b.db.Exec(`INSERT INTO chats (jid, last_ts) VALUES (?, ?) ON CONFLICT (jid) DO UPDATE SET last_ts = excluded.last_ts`, chat, m.TS)
	emit(map[string]any{"t": "message", "msg": m})

	_, err = b.cli.SendMessage(b.ctx, jid, &waE2E.Message{Conversation: proto.String(text)}, whatsmeow.SendRequestExtra{ID: id})
	if err != nil {
		emit(map[string]any{"t": "error", "text": "não enviou: " + err.Error()})
		return
	}
	b.setStatus(jid, []types.MessageID{id}, 1)
	b.dirty.Store(true)
}

// markRead sends read receipts for the chat's unread messages (the last
// `unread` received ones), grouped by sender as groups require. Quiet ones
// reach only your other devices, so the sender sees no blue ticks. A chat
// marked unread by hand (or `patch`, the menu's "Marcar como lida") also
// tells the app state, or the phone keeps its mark.
func (b *Bridge) markRead(chat string, quiet, patch bool) {
	var unread int
	var marked bool
	if b.db.QueryRow(`SELECT unread, marked FROM chats WHERE jid = ?`, chat).Scan(&unread, &marked) != nil || unread == 0 && !marked {
		return
	}
	b.db.Exec(`UPDATE chats SET unread = 0, marked = 0 WHERE jid = ?`, chat)
	b.dirty.Store(true)
	jid, err := types.ParseJID(chat)
	if err != nil {
		return
	}
	if marked || patch {
		ts, key := b.lastKey(chat)
		if err := b.cli.SendAppState(b.ctx, appstate.BuildMarkChatAsRead(b.waJID(chat), true, ts, key)); err != nil {
			logf("mark read %s: %v", chat, err)
		}
	}
	ms := b.query(`WHERE chat = ? AND from_me = 0 ORDER BY ts DESC LIMIT ?`, chat, min(unread, 50))
	bySender := map[string][]types.MessageID{}
	for _, m := range ms {
		bySender[m.Sender] = append(bySender[m.Sender], m.ID)
	}
	for sender, ids := range bySender {
		s := types.EmptyJID
		if jid.Server == types.GroupServer {
			s, _ = types.ParseJID(sender)
		}
		kind := types.ReceiptTypeRead
		if quiet {
			kind = types.ReceiptTypeReadSelf
		}
		if err := b.cli.MarkRead(b.ctx, ids, time.Now(), jid, s, kind); err != nil {
			fmt.Fprintln(os.Stderr, "mark read:", err)
		}
	}
}

// fold lowercases and drops Portuguese accents, for searching.
var unaccent = strings.NewReplacer("á", "a", "à", "a", "â", "a", "ã", "a", "é", "e", "ê", "e", "í", "i", "ó", "o", "ô", "o", "õ", "o", "ú", "u", "ü", "u", "ç", "c")

func fold(s string) string { return unaccent.Replace(strings.ToLower(s)) }

func digitsOf(s string) string {
	var d strings.Builder
	for _, r := range s {
		if r >= '0' && r <= '9' {
			d.WriteRune(r)
		}
	}
	return d.String()
}

// looksLikePhone: only what people type in a number (digits, +, spaces, dashes, parentheses).
func looksLikePhone(s string) bool {
	return strings.Trim(s, "0123456789+-() ") == ""
}

func firstName(c types.ContactInfo) string {
	for _, n := range []string{c.FullName, c.FirstName, c.BusinessName, c.PushName} {
		if n != "" {
			return n
		}
	}
	return ""
}

// search answers the sidebar's field: contacts we have no chat with yet,
// messages with the text, and, for a phone number, whether it's on WhatsApp.
func (b *Bridge) search(q string) {
	q = strings.TrimSpace(q)
	out := map[string]any{"t": "search", "query": q, "contacts": []any{}, "messages": []Msg{}}
	if q == "" {
		emit(out)
		return
	}
	lq, digits := fold(q), digitsOf(q)

	chats := map[string]bool{}
	if rows, err := b.db.Query(`SELECT jid FROM chats`); err == nil {
		for rows.Next() {
			var j string
			if rows.Scan(&j) == nil {
				chats[j] = true
			}
		}
		rows.Close()
	}
	type contact struct {
		JID   string `json:"jid"`
		Name  string `json:"name"`
		Phone string `json:"phone"`
	}
	var contacts []contact
	seen := map[string]bool{}
	if !b.stored() {
		// Not paired: no contacts yet.
	} else if all, err := b.cli.Store.Contacts.GetAllContacts(b.ctx); err == nil {
		for jid, c := range all {
			j := b.norm(jid)
			name := firstName(c)
			if j.Server != types.DefaultUserServer || name == "" || seen[j.User] || chats[j.String()] || j.User == b.me().User {
				continue
			}
			if strings.Contains(fold(name), lq) || (len(digits) >= 3 && strings.Contains(j.User, digits)) {
				seen[j.User] = true
				contacts = append(contacts, contact{j.String(), name, j.User})
			}
		}
	}
	sort.Slice(contacts, func(i, j int) bool { return fold(contacts[i].Name) < fold(contacts[j].Name) })
	if len(contacts) > 30 {
		contacts = contacts[:30]
	}
	if contacts != nil {
		out["contacts"] = contacts
	}
	if ms := b.query(`WHERE text LIKE ? ORDER BY ts DESC LIMIT 40`, "%"+q+"%"); ms != nil {
		out["messages"] = ms
	}

	// A number: Brazilian ones may come without the country code.
	if looksLikePhone(q) && len(digits) >= 10 && len(digits) <= 15 {
		if len(digits) <= 11 {
			digits = "55" + digits
		}
		ctx, cancel := context.WithTimeout(b.ctx, 10*time.Second)
		res, err := b.cli.IsOnWhatsApp(ctx, []string{"+" + digits})
		cancel()
		if err == nil && len(res) > 0 {
			jid := res[0].JID
			if jid.IsEmpty() {
				jid = types.NewJID(digits, types.DefaultUserServer)
			}
			out["number"] = map[string]any{"jid": jid.String(), "phone": digits, "exists": res[0].IsIn}
		}
	}
	emit(out)
}

// profile is what WhatsApp's contact (or group) info panel shows, with the
// picture at full size.
func (b *Bridge) profile(chat string) {
	jid, err := types.ParseJID(chat)
	if err != nil {
		return
	}
	p := map[string]any{"t": "profile", "chat": chat}
	if jid == types.PSAJID {
		p["name"] = "WhatsApp"
		p["about"] = "Conta oficial do WhatsApp"
		emit(p)
		return
	}
	ctx, cancel := context.WithTimeout(b.ctx, 20*time.Second)
	defer cancel()
	if jid.Server == types.GroupServer {
		g, err := b.cli.GetGroupInfo(ctx, jid)
		if err != nil {
			p["error"] = err.Error()
			emit(p)
			return
		}
		p["name"] = g.Name
		p["about"] = g.Topic
		p["created"] = g.GroupCreated.Unix()
		me := b.me()
		type member struct {
			JID   string `json:"jid"`
			Name  string `json:"name"`
			Phone string `json:"phone"`
			Admin bool   `json:"admin"`
			Me    bool   `json:"me"`
		}
		members := []member{}
		for _, m := range g.Participants {
			pj := m.PhoneNumber
			if pj.IsEmpty() {
				pj = b.norm(m.JID)
			}
			phone := ""
			if pj.Server == types.DefaultUserServer {
				phone = pj.User
			}
			members = append(members, member{pj.String(), b.senderName(pj, m.DisplayName), phone, m.IsAdmin || m.IsSuperAdmin, pj.User == me.User})
		}
		// You first, then admins, then by name.
		sort.SliceStable(members, func(i, j int) bool {
			a, c := members[i], members[j]
			if a.Me != c.Me {
				return a.Me
			}
			if a.Admin != c.Admin {
				return a.Admin
			}
			return fold(a.Name) < fold(c.Name)
		})
		p["members"] = members
	} else {
		var c types.ContactInfo
		if b.stored() {
			c, _ = b.cli.Store.Contacts.GetContact(b.ctx, jid)
		}
		p["push_name"] = c.PushName
		verified := ""
		if info, err := b.cli.GetUserInfo(ctx, []types.JID{jid}); err != nil {
			logf("user info %s: %v", jid, err)
		} else {
			for _, u := range info {
				p["about"] = u.Status
				if u.VerifiedName != nil {
					verified = u.VerifiedName.Details.GetVerifiedName()
				}
			}
		}
		if verified == "" {
			verified = c.BusinessName
		}
		if verified == "" && jid.Server == types.DefaultUserServer {
			if r, err := b.cli.IsOnWhatsApp(ctx, []string{"+" + jid.User}); err == nil && len(r) > 0 && r[0].VerifiedName != nil {
				verified = r[0].VerifiedName.Details.GetVerifiedName()
			}
		}
		// The verified name, then what you saved or they set, then the number.
		for _, n := range []string{verified, c.FullName, c.FirstName, c.PushName} {
			if n != "" {
				p["name"] = n
				break
			}
		}
		if jid.Server == types.DefaultUserServer {
			p["phone"] = jid.User
			if p["name"] == nil {
				p["name"] = "+" + jid.User
			}
		} else if p["name"] == nil {
			p["name"] = jid.User
		}
		p["business"] = verified
		lat, lon, biz := b.bizProfile(ctx, jid, p)
		p["biz"] = biz || verified != ""
		if addr, _ := p["address"].(string); biz && (addr != "" || lat != 0 || lon != 0) {
			p["maps"] = mapsLink(addr, lat, lon)
		}
	}
	if info, err := b.cli.GetProfilePictureInfo(ctx, jid, &whatsmeow.GetProfilePictureParams{}); err == nil && info != nil {
		name := strings.NewReplacer("@", "_", ".", "_", ":", "_").Replace(chat)
		path := filepath.Join(b.dir, "avatars", name+"-full.jpg")
		if download(info.URL, path) == nil {
			p["photo"] = path
		}
	}
	emit(p)
}
