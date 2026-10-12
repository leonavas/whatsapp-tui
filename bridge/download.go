package main

// Media kept to be downloaded later: what each message carries is stored
// with it, and `download` fetches it into the cache on demand.

import (
	"context"
	"errors"
	"io"
	"mime"
	"os"
	"path/filepath"
	"strings"
	"sync"
	"time"

	"go.mau.fi/whatsmeow"
	"go.mau.fi/whatsmeow/proto/waE2E"
	"go.mau.fi/whatsmeow/proto/waMmsRetry"
	"go.mau.fi/whatsmeow/types"
	"go.mau.fi/whatsmeow/types/events"
	"google.golang.org/protobuf/proto"
)

// Media is what the TUI shows of a file before it's downloaded.
type Media struct {
	// audio, image, video, document, sticker.
	Kind    string `json:"kind"`
	Mime    string `json:"mime,omitempty"`
	Seconds uint32 `json:"seconds,omitempty"`
	// A voice note, recorded in the app, rather than an audio file.
	PTT bool `json:"ptt,omitempty"`
}

const unavailable = "Mídia indisponível"

// mediaOf finds the downloadable part of a message: what it is, and a
// message with only that part, to keep for downloading it later.
func mediaOf(m *waE2E.Message) (*Media, *waE2E.Message) {
	switch {
	case m.GetAudioMessage() != nil:
		x := m.GetAudioMessage()
		return &Media{"audio", x.GetMimetype(), x.GetSeconds(), x.GetPTT()}, &waE2E.Message{AudioMessage: x}
	case m.GetImageMessage() != nil:
		x := m.GetImageMessage()
		return &Media{Kind: "image", Mime: x.GetMimetype()}, &waE2E.Message{ImageMessage: x}
	case m.GetVideoMessage() != nil:
		x := m.GetVideoMessage()
		return &Media{Kind: "video", Mime: x.GetMimetype(), Seconds: x.GetSeconds()}, &waE2E.Message{VideoMessage: x}
	case m.GetDocumentMessage() != nil:
		x := m.GetDocumentMessage()
		return &Media{Kind: "document", Mime: x.GetMimetype()}, &waE2E.Message{DocumentMessage: x}
	case m.GetStickerMessage() != nil:
		x := m.GetStickerMessage()
		return &Media{Kind: "sticker", Mime: x.GetMimetype()}, &waE2E.Message{StickerMessage: x}
	}
	return nil, nil
}

func downloadable(m *waE2E.Message) whatsmeow.DownloadableMessage {
	switch {
	case m.GetAudioMessage() != nil:
		return m.GetAudioMessage()
	case m.GetImageMessage() != nil:
		return m.GetImageMessage()
	case m.GetVideoMessage() != nil:
		return m.GetVideoMessage()
	case m.GetDocumentMessage() != nil:
		return m.GetDocumentMessage()
	case m.GetStickerMessage() != nil:
		return m.GetStickerMessage()
	}
	return nil
}

func setDirectPath(m *waE2E.Message, p string) {
	switch {
	case m.GetAudioMessage() != nil:
		m.AudioMessage.DirectPath = &p
	case m.GetImageMessage() != nil:
		m.ImageMessage.DirectPath = &p
	case m.GetVideoMessage() != nil:
		m.VideoMessage.DirectPath = &p
	case m.GetDocumentMessage() != nil:
		m.DocumentMessage.DirectPath = &p
	case m.GetStickerMessage() != nil:
		m.StickerMessage.DirectPath = &p
	}
}

// storeMedia keeps what a message's media is; a nil part keeps the keys
// stored before (a pending message rewritten).
func (b *Bridge) storeMedia(chat, id string, md *Media, part *waE2E.Message, waChat, waSender string) {
	if md == nil {
		return
	}
	var blob []byte
	if part != nil {
		blob, _ = proto.Marshal(part)
	}
	defer func() {
		if part != nil && takeRecovery(chat, id) {
			logf("recover %s %s: media arrived", chat, id)
			go func() {
				// Its bubble learns whether it's a voice note, then it plays.
				if m := b.load(chat, id); m != nil {
					emit(map[string]any{"t": "message", "msg": m})
				}
				b.download(chat, id)
			}()
		}
	}()
	_, err := b.db.Exec(`INSERT INTO media (chat, id, kind, mime, seconds, ptt, proto, wa_chat, wa_sender) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)
		ON CONFLICT (chat, id) DO UPDATE SET kind = excluded.kind, mime = excluded.mime,
			seconds = max(seconds, excluded.seconds), ptt = excluded.ptt, proto = COALESCE(excluded.proto, proto),
			wa_chat = excluded.wa_chat, wa_sender = excluded.wa_sender`,
		chat, id, md.Kind, md.Mime, md.Seconds, md.PTT, blob, waChat, waSender)
	if err != nil {
		logf("store media %s: %v", id, err)
	}
}

var exts = map[string]string{
	"audio/ogg": "ogg", "audio/mpeg": "mp3", "audio/mp4": "m4a", "audio/x-m4a": "m4a", "audio/aac": "aac", "audio/amr": "amr",
	"image/jpeg": "jpg", "image/png": "png", "image/webp": "webp", "video/mp4": "mp4", "video/3gpp": "3gp", "application/pdf": "pdf",
}

func extOf(mimeType string) string {
	t, _, _ := mime.ParseMediaType(mimeType)
	if e, ok := exts[t]; ok {
		return e
	}
	if es, _ := mime.ExtensionsByType(t); len(es) > 0 {
		return strings.TrimPrefix(es[0], ".")
	}
	return "bin"
}

// mediaFile is where a message's media goes: ~/.cache/whatsapp-tui/media/<chat>_<id>.<ext>.
func mediaFile(chat, id, mimeType string) string {
	dir, err := os.UserCacheDir()
	if err != nil {
		dir = os.TempDir()
	}
	name := strings.NewReplacer("@", "_", ".", "_", ":", "_", "/", "_").Replace(chat + "_" + id)
	return filepath.Join(dir, "whatsapp-tui", "media", name+"."+extOf(mimeType))
}

func writeFile(path string, data []byte) error {
	if err := os.MkdirAll(filepath.Dir(path), 0o700); err != nil {
		return err
	}
	tmp := path + ".tmp"
	if err := os.WriteFile(tmp, data, 0o600); err != nil {
		return err
	}
	return os.Rename(tmp, path)
}

// linkOrCopy: a hard link when it's the same disk, a copy when not.
func linkOrCopy(from, to string) error {
	if err := os.MkdirAll(filepath.Dir(to), 0o700); err != nil {
		return err
	}
	os.Remove(to)
	if os.Link(from, to) == nil {
		return nil
	}
	in, err := os.Open(from)
	if err != nil {
		return err
	}
	defer in.Close()
	tmp := to + ".tmp"
	out, err := os.Create(tmp)
	if err != nil {
		return err
	}
	if _, err := io.Copy(out, in); err != nil {
		out.Close()
		return err
	}
	out.Close()
	return os.Rename(tmp, to)
}

// A media the phone was asked to upload again, waiting for its answer.
type reupload struct {
	chat, id, path string
	part           *waE2E.Message
}

var (
	inFlight  sync.Map
	reMu      sync.Mutex
	reuploads = map[types.MessageID]reupload{}
	// Messages whose media's keys were asked of the phone, by chat and id.
	recovering = map[string]bool{}
)

func mediaDone(chat, id, path string) {
	emit(map[string]any{"t": "media", "chat": chat, "id": id, "path": path})
}

func mediaFailed(chat, id, why string) {
	emit(map[string]any{"t": "media", "chat": chat, "id": id, "error": why})
}

// download fetches a message's media into the cache, once; the answer is a
// `media` event with the file, or with why not.
func (b *Bridge) download(chat, id string) {
	key := chat + "\x00" + id
	if _, busy := inFlight.LoadOrStore(key, true); busy {
		return
	}
	defer inFlight.Delete(key)
	var mimeType, waChat, waSender, sender string
	var blob []byte
	var fromMe bool
	var ts int64
	err := b.db.QueryRow(`SELECT COALESCE(mime, ''), proto, COALESCE(wa_chat, ''), COALESCE(wa_sender, ''), sender, from_me, ts
		FROM messages LEFT JOIN media USING (chat, id) WHERE chat = ? AND id = ?`, chat, id).
		Scan(&mimeType, &blob, &waChat, &waSender, &sender, &fromMe, &ts)
	if err != nil {
		mediaFailed(chat, id, unavailable)
		return
	}
	path := mediaFile(chat, id, mimeType)
	if st, err := os.Stat(path); mimeType != "" && err == nil && st.Size() > 0 {
		mediaDone(chat, id, path)
		return
	}
	part := &waE2E.Message{}
	if blob == nil || proto.Unmarshal(blob, part) != nil || downloadable(part) == nil {
		// Stored before the keys were kept: the phone has them.
		b.recoverMedia(chat, id, sender, ts)
		return
	}
	err = b.fetch(part, path)
	switch {
	case err == nil:
		mediaDone(chat, id, path)
	case errors.Is(err, whatsmeow.ErrMediaDownloadFailedWith404) || errors.Is(err, whatsmeow.ErrMediaDownloadFailedWith410):
		// Expired on the server: the phone, if it still has it, uploads it again.
		if b.askReupload(chat, id, path, part, waChat, waSender, fromMe) != nil {
			mediaFailed(chat, id, unavailable)
		}
	default:
		logf("download %s: %v", id, err)
		mediaFailed(chat, id, unavailable)
	}
}

func (b *Bridge) fetch(part *waE2E.Message, path string) error {
	ctx, cancel := context.WithTimeout(b.ctx, 2*time.Minute)
	defer cancel()
	data, err := b.cli.Download(ctx, downloadable(part))
	if err != nil {
		return err
	}
	return writeFile(path, data)
}

func (b *Bridge) askReupload(chat, id, path string, part *waE2E.Message, waChat, waSender string, fromMe bool) error {
	c, err := types.ParseJID(waChat)
	if err != nil {
		return err
	}
	s, _ := types.ParseJID(waSender)
	info := &types.MessageInfo{
		MessageSource: types.MessageSource{Chat: c, Sender: s, IsFromMe: fromMe, IsGroup: c.Server == types.GroupServer},
		ID:            id,
	}
	if err := b.cli.SendMediaRetryReceipt(b.ctx, info, downloadable(part).GetMediaKey()); err != nil {
		logf("media retry %s: %v", id, err)
		return err
	}
	reMu.Lock()
	reuploads[id] = reupload{chat, id, path, part}
	reMu.Unlock()
	// The phone may be off, or no longer have it.
	time.AfterFunc(30*time.Second, func() {
		if _, ok := takeReupload(id); ok {
			mediaFailed(chat, id, unavailable)
		}
	})
	return nil
}

func takeReupload(id types.MessageID) (reupload, bool) {
	reMu.Lock()
	defer reMu.Unlock()
	r, ok := reuploads[id]
	delete(reuploads, id)
	return r, ok
}

// mediaRetry is the phone's answer: the media's new path, or that it's gone.
func (b *Bridge) mediaRetry(e *events.MediaRetry) {
	r, ok := takeReupload(e.MessageID)
	if !ok {
		return
	}
	n, err := whatsmeow.DecryptMediaRetryNotification(e, downloadable(r.part).GetMediaKey())
	if err != nil || n.GetResult() != waMmsRetry.MediaRetryNotification_SUCCESS || n.GetDirectPath() == "" {
		mediaFailed(r.chat, r.id, unavailable)
		return
	}
	setDirectPath(r.part, n.GetDirectPath())
	if blob, err := proto.Marshal(r.part); err == nil {
		b.db.Exec(`UPDATE media SET proto = ? WHERE chat = ? AND id = ?`, blob, r.chat, r.id)
	}
	if err := b.fetch(r.part, r.path); err != nil {
		logf("download %s after retry: %v", r.id, err)
		mediaFailed(r.chat, r.id, unavailable)
		return
	}
	mediaDone(r.chat, r.id, r.path)
}

func takeRecovery(chat, id string) bool {
	reMu.Lock()
	defer reMu.Unlock()
	ok := recovering[chat+"\x00"+id]
	delete(recovering, chat+"\x00"+id)
	return ok
}

// recoverMedia asks the phone again for a message stored without its
// media's keys, two ways: the message itself by its key (the request
// WhatsApp uses for messages a device couldn't decrypt), and an on-demand
// history sync around it. Whichever answers first stores the keys, and
// storeMedia carries on into the download.
func (b *Bridge) recoverMedia(chat, id, sender string, ts int64) {
	key := chat + "\x00" + id
	reMu.Lock()
	busy := recovering[key]
	recovering[key] = true
	reMu.Unlock()
	if busy {
		return
	}
	time.AfterFunc(20*time.Second, func() {
		if takeRecovery(chat, id) {
			logf("recover %s %s: no answer from the phone", chat, id)
			mediaFailed(chat, id, unavailable)
		}
	})
	jid, err := types.ParseJID(chat)
	if err != nil {
		return
	}
	// The phone may know a 1:1 chat by its LID: ask under both.
	chats := []types.JID{jid}
	if jid.Server == types.DefaultUserServer {
		if lid, err := b.cli.Store.LIDs.GetLIDForPN(b.ctx, jid); err == nil && !lid.IsEmpty() {
			chats = append(chats, lid)
		}
	}
	from, _ := types.ParseJID(sender)

	// A history request returns the messages before its anchor: the next
	// one we have, or, for the newest, one that doesn't exist, from now.
	anchor := types.MessageInfo{ID: b.cli.GenerateMessageID(), Timestamp: time.Now()}
	anchor.IsFromMe = true
	var aID string
	var aFromMe bool
	var aTS int64
	if b.db.QueryRow(`SELECT id, from_me, ts FROM messages WHERE chat = ? AND (ts > ? OR (ts = ? AND id > ?)) ORDER BY ts, id LIMIT 1`,
		chat, ts, ts, id).Scan(&aID, &aFromMe, &aTS) == nil {
		anchor = types.MessageInfo{ID: aID, Timestamp: time.Unix(aTS, 0)}
		anchor.IsFromMe = aFromMe
	}
	for _, c := range chats {
		_, err := b.cli.SendPeerMessage(b.ctx, b.cli.BuildUnavailableMessageRequest(c, from, id))
		logf("recover %s: asked the phone for message %s (sender %s): err=%v", c, id, from, err)
		a := anchor
		a.Chat = c
		_, err = b.cli.SendPeerMessage(b.ctx, b.cli.BuildHistorySyncRequest(&a, 10))
		logf("recover %s: asked for 10 messages before %s (ts %d, from me %v, real %v): err=%v", c, a.ID, a.Timestamp.Unix(), a.IsFromMe, aID != "", err)
	}
}
