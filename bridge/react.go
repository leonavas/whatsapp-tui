package main

// Reactions: kept per message and sender (one each, as WhatsApp allows),
// shown under the bubble as WhatsApp Web does, the emojis most used first
// and how many reacted.

import (
	"sort"
	"strings"
	"time"

	"go.mau.fi/whatsmeow/proto/waCommon"
	"go.mau.fi/whatsmeow/types"
)

// Reaction is one emoji under a message: how many sent it, and whether
// you're one of them.
type Reaction struct {
	Emoji string `json:"emoji"`
	Count int    `json:"count"`
	Mine  bool   `json:"mine,omitempty"`
}

// react keeps sender's reaction to a message; an empty emoji takes it back.
func (b *Bridge) react(chat, id, sender, emoji string, ts int64) {
	if emoji == "" {
		b.db.Exec(`DELETE FROM reactions WHERE chat = ? AND id = ? AND sender = ?`, chat, id, sender)
		return
	}
	// An older one (a history sync's) doesn't undo a newer.
	b.db.Exec(`INSERT INTO reactions (chat, id, sender, emoji, ts) VALUES (?, ?, ?, ?, ?)
		ON CONFLICT (chat, id, sender) DO UPDATE SET emoji = excluded.emoji, ts = excluded.ts WHERE excluded.ts >= ts`,
		chat, id, sender, emoji, ts)
}

// reactionSender is who a reaction's key says sent it: you, the group's
// participant, or the other person in a chat of two.
func (b *Bridge) reactionSender(chat types.JID, key *waCommon.MessageKey) string {
	if key.GetFromMe() {
		return b.me().String()
	}
	if p := key.GetParticipant(); p != "" {
		if j, err := types.ParseJID(p); err == nil {
			return b.norm(j).String()
		}
	}
	return chat.String()
}

// withReactions fills in the messages' reactions, a query per chat.
func (b *Bridge) withReactions(ms []Msg) {
	byChat := map[string][]int{}
	for i := range ms {
		byChat[ms[i].Chat] = append(byChat[ms[i].Chat], i)
	}
	me := b.me().String()
	for chat, idx := range byChat {
		at := map[string]int{}
		args := []any{chat}
		for _, i := range idx {
			at[ms[i].ID] = i
			args = append(args, ms[i].ID)
		}
		rows, err := b.db.Query(`SELECT id, sender, emoji FROM reactions WHERE chat = ? AND id IN (?`+strings.Repeat(", ?", len(idx)-1)+`) ORDER BY ts`, args...)
		if err != nil {
			continue
		}
		for rows.Next() {
			var id, sender, emoji string
			if rows.Scan(&id, &sender, &emoji) != nil {
				continue
			}
			m := &ms[at[id]]
			k := -1
			for j, r := range m.Reactions {
				if r.Emoji == emoji {
					k = j
				}
			}
			if k < 0 {
				m.Reactions = append(m.Reactions, Reaction{Emoji: emoji})
				k = len(m.Reactions) - 1
			}
			m.Reactions[k].Count++
			if sender == me {
				m.Reactions[k].Mine = true
			}
		}
		rows.Close()
	}
	for i := range ms {
		// The most used first; the first sent first among equals.
		sort.SliceStable(ms[i].Reactions, func(a, c int) bool { return ms[i].Reactions[a].Count > ms[i].Reactions[c].Count })
	}
}

// sendReaction reacts to a message with an emoji, or takes yours back with
// an empty one: here at once, then to WhatsApp.
func (b *Bridge) sendReaction(chat, id, emoji string) {
	jid, err := types.ParseJID(chat)
	if err != nil || id == "" {
		return
	}
	var fromMe bool
	var sender, waSender string
	if b.db.QueryRow(`SELECT from_me, sender, wa_sender FROM messages WHERE chat = ? AND id = ?`, chat, id).Scan(&fromMe, &sender, &waSender) != nil {
		return
	}
	// The key names who wrote it as WhatsApp has them.
	by := b.me()
	if !fromMe {
		for _, s := range []string{waSender, sender} {
			if j, err := types.ParseJID(s); err == nil && s != "" {
				by = j
				break
			}
		}
	}
	var old string
	me := b.me().String()
	b.db.QueryRow(`SELECT emoji FROM reactions WHERE chat = ? AND id = ? AND sender = ?`, chat, id, me).Scan(&old)
	now := time.Now()
	b.react(chat, id, me, emoji, now.Unix())
	b.emitMsg(chat, id)

	if _, err := b.cli.SendMessage(b.ctx, jid, b.cli.BuildReaction(jid, by, id, emoji)); err != nil {
		b.react(chat, id, me, old, now.Unix())
		b.emitMsg(chat, id)
		emit(map[string]any{"t": "error", "text": "não reagiu: " + err.Error()})
	}
}

// emitMsg sends the TUI a message as it's stored now.
func (b *Bridge) emitMsg(chat, id string) {
	if m := b.load(chat, id); m != nil {
		emit(map[string]any{"t": "message", "msg": m})
	}
}
