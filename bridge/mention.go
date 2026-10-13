package main

// Mentions: WhatsApp writes an @ in the text with the person's number (or
// hidden id), and the apps show their name in its place.

import (
	"strings"

	"go.mau.fi/whatsmeow/types"
)

// Mention is one @ in a message's text: the digits it's written with, who
// that is and the name shown for it.
type Mention struct {
	User string `json:"user"`
	JID  string `json:"jid"`
	Name string `json:"name"`
}

// mentionAt finds the digits of an @ starting at i: at a word's start, five
// digits or more. Returns them and where they end.
func mentionAt(t string, i int) (string, int) {
	if t[i] != '@' || (i > 0 && isWord(t[i-1])) {
		return "", i
	}
	j := i + 1
	for j < len(t) && t[j] >= '0' && t[j] <= '9' {
		j++
	}
	if j-i-1 < 5 || (j < len(t) && isWord(t[j])) {
		return "", i
	}
	return t[i+1 : j], j
}

func isWord(c byte) bool {
	return c == '_' || c >= '0' && c <= '9' || c >= 'a' && c <= 'z' || c >= 'A' && c <= 'Z'
}

// mentions are the @s in text that name someone; `seen` keeps them across
// a page of messages.
func (b *Bridge) mentions(text string, seen map[string]*Mention) []Mention {
	var out []Mention
	got := map[string]bool{}
	for i := 0; i < len(text); i++ {
		user, end := mentionAt(text, i)
		if user == "" || got[user] {
			continue
		}
		got[user] = true
		i = end - 1
		if m := b.mention(user, seen); m != nil {
			out = append(out, *m)
		}
	}
	return out
}

// mention is who the digits are: by the hidden id's number when it's known,
// else as a number; nil for a hidden id nobody has a name for.
func (b *Bridge) mention(user string, seen map[string]*Mention) *Mention {
	if m, ok := seen[user]; ok {
		return m
	}
	var m *Mention
	defer func() { seen[user] = m }()
	if !b.stored() {
		return nil
	}
	if me := b.me(); user == me.User || user == b.cli.Store.LID.User {
		m = &Mention{User: user, JID: me.String(), Name: "Você"}
		return m
	}
	lid := types.NewJID(user, types.HiddenUserServer)
	pn := types.NewJID(user, types.DefaultUserServer)
	who := lid
	if j := b.norm(lid); j.Server == types.DefaultUserServer {
		who = j
	} else if b.contactName(pn) != "" {
		who = pn
	} else if b.contactName(lid) == "" {
		// Someone who wrote here has a name to show, by the push one.
		var push string
		for _, j := range []types.JID{lid, pn} {
			if b.db.QueryRow(`SELECT push_name FROM messages WHERE (sender = ? OR wa_sender = ?) AND push_name != '' ORDER BY ts DESC LIMIT 1`, j.String(), j.String()).Scan(&push) == nil {
				who = j
				break
			}
		}
		if push == "" {
			// A hidden id is no number to show; a phone number is.
			if len(user) > 13 {
				return nil
			}
			who = pn
		}
	}
	var push string
	if b.contactName(who) == "" {
		b.db.QueryRow(`SELECT push_name FROM messages WHERE (sender = ? OR wa_sender = ?) AND push_name != '' ORDER BY ts DESC LIMIT 1`, who.String(), who.String()).Scan(&push)
	}
	m = &Mention{User: user, JID: who.String(), Name: b.senderName(who, push)}
	return m
}

// withMentions is text with each @'s digits replaced by the name, for where
// it's only read: a chat's preview, a quote.
func withMentions(text string, ms []Mention) string {
	if len(ms) == 0 {
		return text
	}
	names := map[string]string{}
	for _, m := range ms {
		names[m.User] = m.Name
	}
	var sb strings.Builder
	for i := 0; i < len(text); i++ {
		user, end := mentionAt(text, i)
		if n, ok := names[user]; ok && user != "" {
			sb.WriteString("@" + n)
			i = end - 1
			continue
		}
		sb.WriteByte(text[i])
	}
	return sb.String()
}
