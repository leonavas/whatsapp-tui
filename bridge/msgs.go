package main

// A message's own menu: edit yours, delete for you or for everyone. As with
// the chats, the change is made here first and sent to the TUI at once, then
// to WhatsApp; if WhatsApp refuses it, it goes back.

import (
	"strings"

	"go.mau.fi/whatsmeow/appstate"
	"go.mau.fi/whatsmeow/proto/waE2E"
	"go.mau.fi/whatsmeow/proto/waSyncAction"
	"go.mau.fi/whatsmeow/types"
	"google.golang.org/protobuf/proto"
)

// What a message deleted for everyone reads as.
const (
	revokedMine   = "🚫 Você apagou esta mensagem"
	revokedOthers = "🚫 Mensagem apagada"
)

// What an edited message's text ends with.
const editedMark = " (editada)"

// removeMsgs drops messages from here and tells the TUI.
func (b *Bridge) removeMsgs(chat string, ids []string) {
	if len(ids) == 0 {
		return
	}
	args := []any{chat}
	for _, id := range ids {
		args = append(args, id)
	}
	in := `chat = ? AND id IN (?` + strings.Repeat(", ?", len(ids)-1) + `)`
	for _, t := range []string{"messages", "media", "reactions"} {
		b.db.Exec(`DELETE FROM `+t+` WHERE `+in, args...)
	}
	b.dirty.Store(true)
	emit(map[string]any{"t": "removed", "chat": chat, "ids": ids})
}

// deleteForMe drops the messages here and tells your other devices, through
// the app state, as the phone's "Apagar para mim" does.
func (b *Bridge) deleteForMe(chat string, ids []string) {
	wa := b.waJID(chat)
	if wa.IsEmpty() {
		return
	}
	var muts []appstate.MutationInfo
	for _, id := range ids {
		var fromMe bool
		var ts int64
		var sender, waSender string
		if b.db.QueryRow(`SELECT from_me, ts, sender, wa_sender FROM messages WHERE chat = ? AND id = ?`, chat, id).
			Scan(&fromMe, &ts, &sender, &waSender) != nil {
			continue
		}
		mine, participant := "0", "0"
		if fromMe {
			mine = "1"
		} else if wa.Server == types.GroupServer {
			participant = waSender
			if participant == "" {
				participant = sender
			}
		}
		muts = append(muts, appstate.MutationInfo{
			Index:   []string{appstate.IndexDeleteMessageForMe, wa.String(), id, mine, participant},
			Version: 3,
			Value: &waSyncAction.SyncActionValue{DeleteMessageForMeAction: &waSyncAction.DeleteMessageForMeAction{
				DeleteMedia:      proto.Bool(false),
				MessageTimestamp: proto.Int64(ts),
			}},
		})
	}
	b.removeMsgs(chat, ids)
	b.flush()
	if len(muts) == 0 {
		return
	}
	if err := b.cli.SendAppState(b.ctx, appstate.PatchInfo{Type: appstate.WAPatchRegularHigh, Mutations: muts}); err != nil {
		logf("delete for me %s: %v", chat, err)
		emit(map[string]any{"t": "error", "text": "não apagou nos outros aparelhos: " + err.Error()})
	}
}

// revoke deletes your messages for everyone.
func (b *Bridge) revoke(chat string, ids []string) {
	jid, err := types.ParseJID(chat)
	if err != nil {
		return
	}
	for _, id := range ids {
		var text string
		var fromMe bool
		if b.db.QueryRow(`SELECT text, from_me FROM messages WHERE chat = ? AND id = ?`, chat, id).Scan(&text, &fromMe) != nil || !fromMe {
			continue
		}
		if _, err := b.cli.SendMessage(b.ctx, jid, b.cli.BuildRevoke(jid, types.EmptyJID, id)); err != nil {
			// The TUI already shows it gone: it comes back as it was.
			b.emitMsg(chat, id)
			emit(map[string]any{"t": "error", "text": "não apagou para todos: " + err.Error()})
			continue
		}
		b.db.Exec(`UPDATE messages SET text = ? WHERE chat = ? AND id = ?`, revokedMine, chat, id)
		b.db.Exec(`DELETE FROM media WHERE chat = ? AND id = ?`, chat, id)
		b.db.Exec(`DELETE FROM reactions WHERE chat = ? AND id = ?`, chat, id)
		b.emitMsg(chat, id)
	}
	b.dirty.Store(true)
}

// edit changes the text of one of your messages.
func (b *Bridge) edit(chat, id, text string) {
	text = strings.TrimSpace(text)
	jid, err := types.ParseJID(chat)
	if err != nil || text == "" {
		return
	}
	var old string
	var fromMe bool
	if b.db.QueryRow(`SELECT text, from_me FROM messages WHERE chat = ? AND id = ?`, chat, id).Scan(&old, &fromMe) != nil || !fromMe {
		return
	}
	b.db.Exec(`UPDATE messages SET text = ? WHERE chat = ? AND id = ?`, text+editedMark, chat, id)
	b.emitMsg(chat, id)
	b.dirty.Store(true)
	edit := b.cli.BuildEdit(jid, id, &waE2E.Message{Conversation: proto.String(text)})
	if _, err := b.cli.SendMessage(b.ctx, jid, edit); err != nil {
		b.db.Exec(`UPDATE messages SET text = ? WHERE chat = ? AND id = ?`, old, chat, id)
		b.emitMsg(chat, id)
		emit(map[string]any{"t": "error", "text": "não editou: " + err.Error()})
	}
}

// deleteMsgs is the TUI's "Apagar": for everyone, or for you only.
func (b *Bridge) deleteMsgs(chat string, ids []string, everyone bool) {
	if everyone {
		b.revoke(chat, ids)
	} else {
		b.deleteForMe(chat, ids)
	}
}
