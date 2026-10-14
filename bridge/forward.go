package main

// "Encaminhar": messages of one chat sent again to others, as WhatsApp does
// it (whatsmeow has no helper for it): the same content, marked forwarded,
// without the original's reply or @s. Media goes with the keys it came
// with, nothing is uploaded again.

import (
	"context"
	"os"
	"sort"
	"strings"
	"time"

	"go.mau.fi/whatsmeow"
	"go.mau.fi/whatsmeow/proto/waE2E"
	"go.mau.fi/whatsmeow/types"
	"google.golang.org/protobuf/proto"
)

// What a message whose content wasn't kept reads as: textOf's labels, not
// text to send again.
var labels = []string{"📷 Foto", "🎥 Vídeo", "🎤 Áudio", "📄 ", "💟 Figurinha", "👤 ", "📍 Localização", "📊 "}

// fwdMsg is one message to forward: what goes, and the original's place.
type fwdMsg struct {
	id    string
	ts    int64
	score uint32
	text  string
	// Its media's part, keys and all; nil for a text.
	part *waE2E.Message
	mime string
}

// fwdContent reads a stored message as something to send again; nil for
// what can't be: deleted, a business's card, media without its keys, the
// labels of what wasn't kept.
func (b *Bridge) fwdContent(chat, id string) *fwdMsg {
	var text, card, kind, mime string
	var score, ts int64
	var blob []byte
	if b.db.QueryRow(`SELECT text, card, forwarded, ts, COALESCE(kind, ''), COALESCE(mime, ''), proto
		FROM messages LEFT JOIN media USING (chat, id) WHERE chat = ? AND id = ?`, chat, id).
		Scan(&text, &card, &score, &ts, &kind, &mime, &blob) != nil {
		return nil
	}
	if card != "" || text == revokedMine || text == revokedOthers {
		return nil
	}
	f := &fwdMsg{id: id, ts: ts, score: uint32(score), mime: mime}
	if kind != "" {
		part := partOf(blob)
		d := downloadable(part)
		if d == nil || len(d.GetMediaKey()) == 0 || d.GetDirectPath() == "" {
			return nil
		}
		f.part = part
		return f
	}
	text = strings.TrimSuffix(text, editedMark)
	for _, l := range labels {
		if strings.HasPrefix(text, l) {
			return nil
		}
	}
	if strings.TrimSpace(text) == "" {
		return nil
	}
	f.text = text
	return f
}

// build is the message to send: a copy, forwarded once more.
func (f *fwdMsg) build() *waE2E.Message {
	ci := &waE2E.ContextInfo{IsForwarded: proto.Bool(true), ForwardingScore: proto.Uint32(f.score + 1)}
	if f.part == nil {
		return &waE2E.Message{ExtendedTextMessage: &waE2E.ExtendedTextMessage{Text: proto.String(f.text), ContextInfo: ci}}
	}
	// The stored part keeps the original's context: the reply and @s go.
	m := proto.Clone(f.part).(*waE2E.Message)
	switch {
	case m.ImageMessage != nil:
		m.ImageMessage.ContextInfo = ci
	case m.VideoMessage != nil:
		m.VideoMessage.ContextInfo = ci
	case m.AudioMessage != nil:
		m.AudioMessage.ContextInfo = ci
	case m.DocumentMessage != nil:
		m.DocumentMessage.ContextInfo = ci
	case m.StickerMessage != nil:
		m.StickerMessage.ContextInfo = ci
	}
	return m
}

// forward sends the messages, oldest first, to each chat in `to`: all shown
// at once, pending, then sent one by one.
func (b *Bridge) forward(chat string, ids, to []string) {
	var msgs []*fwdMsg
	for _, id := range ids {
		if f := b.fwdContent(chat, id); f != nil {
			msgs = append(msgs, f)
		}
	}
	if len(msgs) < len(ids) {
		emit(map[string]any{"t": "error", "text": "Algumas mensagens não podem ser encaminhadas"})
	}
	if len(msgs) == 0 {
		return
	}
	sort.SliceStable(msgs, func(i, j int) bool { return msgs[i].ts < msgs[j].ts })
	type out struct {
		jid types.JID
		id  string
		msg *waE2E.Message
	}
	var outs []out
	me := b.me().String()
	now := time.Now().Unix()
	for _, t := range to {
		jid, err := types.ParseJID(t)
		if err != nil || skip(jid) {
			continue
		}
		var readOnly string
		if b.db.QueryRow(`SELECT read_only FROM chats WHERE jid = ?`, t).Scan(&readOnly) == nil && readOnly != "" {
			emit(map[string]any{"t": "error", "text": readOnly})
			continue
		}
		for _, f := range msgs {
			id := b.cli.GenerateMessageID()
			msg := f.build()
			text, _ := textOf(msg)
			b.db.Exec(`INSERT INTO messages (chat, id, sender, from_me, ts, text, status, forwarded) VALUES (?, ?, ?, 1, ?, ?, 0, ?)`,
				t, id, me, now, text, f.score+1)
			if md, part := mediaOf(msg); md != nil {
				b.storeMedia(t, id, md, part, jid.String(), me)
				// The file already here plays in the new bubble without a download.
				from, dst := mediaFile(chat, f.id, f.mime), mediaFile(t, id, md.Mime)
				for _, ext := range []string{"", ".frame.jpg"} {
					if _, err := os.Stat(from + ext); err == nil {
						linkOrCopy(from+ext, dst+ext)
					}
				}
			}
			b.db.Exec(`INSERT INTO chats (jid, last_ts) VALUES (?, ?) ON CONFLICT (jid) DO UPDATE SET last_ts = max(last_ts, excluded.last_ts)`, t, now)
			b.emitMsg(t, id)
			outs = append(outs, out{jid, id, msg})
		}
	}
	b.dirty.Store(true)
	for _, o := range outs {
		if _, err := b.cli.SendMessage(b.ctx, o.jid, o.msg, whatsmeow.SendRequestExtra{ID: o.id}); err != nil {
			emit(map[string]any{"t": "error", "text": "não encaminhou: " + err.Error()})
			continue
		}
		b.setStatus(o.jid, []types.MessageID{o.id}, 1)
	}
	b.dirty.Store(true)
}

// chatSubs answers the forward dialog: under each chat's name, a group's
// members (first names, you last) and a contact's recado.
func (b *Bridge) chatSubs() {
	subs := map[string]string{}
	ctx, cancel := context.WithTimeout(b.ctx, 20*time.Second)
	defer cancel()
	me, myLID := b.me(), b.cli.Store.GetLID().ToNonAD()
	if groups, err := b.cli.GetJoinedGroups(ctx); err == nil {
		for _, g := range groups {
			var names []string
			you := false
			for _, p := range g.Participants {
				if p.JID.User == me.User || p.PhoneNumber.User == me.User || (!myLID.IsEmpty() && p.LID.User == myLID.User) {
					you = true
					continue
				}
				pj := p.PhoneNumber
				if pj.IsEmpty() {
					pj = b.norm(p.JID)
				}
				names = append(names, b.shortName(pj))
			}
			sort.Slice(names, func(i, j int) bool { return fold(names[i]) < fold(names[j]) })
			if len(names) > 30 {
				names = names[:30]
			}
			if you {
				names = append(names, "Você")
			}
			subs[g.JID.String()] = strings.Join(names, ", ")
		}
	}
	var users []types.JID
	if rows, err := b.db.Query(`SELECT jid FROM chats WHERE jid LIKE '%@s.whatsapp.net'`); err == nil {
		for rows.Next() {
			var s string
			if rows.Scan(&s) == nil {
				if j, err := types.ParseJID(s); err == nil && j.User != me.User && j != types.PSAJID {
					users = append(users, j)
				}
			}
		}
		rows.Close()
	}
	if len(users) > 0 {
		if info, err := b.cli.GetUserInfo(ctx, users); err == nil {
			for j, u := range info {
				if u.Status != "" {
					subs[b.norm(j).String()] = u.Status
				}
			}
		} else {
			logf("chat subs: %v", err)
		}
	}
	emit(map[string]any{"t": "chat_subs", "subs": subs})
}

// shortName is how a group's members are listed: contacts by their first
// name, the rest by their number.
func (b *Bridge) shortName(j types.JID) string {
	if b.stored() {
		if c, err := b.cli.Store.Contacts.GetContact(b.ctx, j); err == nil && c.Found {
			for _, n := range []string{c.FirstName, c.FullName, c.BusinessName} {
				if n != "" {
					return n
				}
			}
		}
	}
	if j.Server == types.DefaultUserServer {
		return "+" + j.User
	}
	return j.User
}
