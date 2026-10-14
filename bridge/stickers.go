package main

// Favorite stickers, as WhatsApp keeps them in its app state: one
// "favoriteSticker" mutation each, indexed by the file's SHA-256 and with
// what's needed to download it. The viewer adds or removes the one it shows;
// the phone's changes come as app state events (removals only in the log).

import (
	"encoding/base64"
	"encoding/json"
	"strconv"

	"go.mau.fi/whatsmeow/appstate"
	"go.mau.fi/whatsmeow/proto/waE2E"
	"go.mau.fi/whatsmeow/proto/waSyncAction"
	"google.golang.org/protobuf/proto"
)

// stickerVersion is the mutation's version until one comes from the phone
// (see stickerLog): whatsmeow has no builder, so no number, for it.
const stickerVersion = 7

func b64(b []byte) string {
	if len(b) == 0 {
		return ""
	}
	return base64.StdEncoding.EncodeToString(b)
}

// favSticker keeps one from the phone; a false isFavorite takes it out.
func (b *Bridge) favSticker(hash string, a *waSyncAction.StickerAction) {
	if a == nil || hash == "" {
		return
	}
	if a.IsFavorite != nil && !a.GetIsFavorite() {
		b.db.Exec(`DELETE FROM fav_stickers WHERE hash = ?`, hash)
		return
	}
	blob, _ := proto.Marshal(a)
	b.db.Exec(`INSERT INTO fav_stickers (hash, enc_hash, action) VALUES (?, ?, ?)
		ON CONFLICT (hash) DO UPDATE SET enc_hash = excluded.enc_hash, action = excluded.action`, hash, b64(a.GetFileEncSHA256()), blob)
}

// stickerLog reads a favoriteSticker line of whatsmeow's log (favLog): the
// version and collection the phone writes them with, and the removals,
// which whatsmeow sends no event for.
func (b *Bridge) stickerLog(p []byte) {
	var r struct {
		Message   string   `json:"message"`
		Patch     string   `json:"patch_name"`
		Version   int32    `json:"version"`
		Operation string   `json:"operation"`
		Index     []string `json:"index"`
	}
	if json.Unmarshal(p, &r) != nil || r.Message != "Received app state mutation" {
		return
	}
	if r.Version > 0 && r.Patch != "" {
		b.setMeta("sticker_version", strconv.Itoa(int(r.Version)))
		b.setMeta("sticker_patch", r.Patch)
	}
	if r.Operation == "REMOVE" && len(r.Index) > 1 {
		b.db.Exec(`DELETE FROM fav_stickers WHERE hash = ?`, r.Index[1])
	}
}

// stickersOnce reads regular_low in full, once per account: the syncs
// from before favorite stickers were kept left them out.
func (b *Bridge) stickersOnce() {
	if b.meta("stickers") != "" {
		return
	}
	if err := b.cli.FetchAppState(b.ctx, appstate.WAPatchRegularLow, true, false); err != nil {
		logf("app state resync for stickers: %v", err)
		return
	}
	b.setMeta("stickers", "1")
}

// stickerOf is the message's sticker as stored, keys and all; nil when it
// isn't one or its keys weren't kept.
func (b *Bridge) stickerOf(chat, id string) *waE2E.StickerMessage {
	var blob []byte
	b.db.QueryRow(`SELECT proto FROM media WHERE chat = ? AND id = ? AND kind = 'sticker'`, chat, id).Scan(&blob)
	x := partOf(blob).GetStickerMessage()
	if len(x.GetFileSHA256()) == 0 || len(x.GetMediaKey()) == 0 {
		return nil
	}
	return x
}

// stickerFav says whether the viewer's sticker is a favorite, or with
// favorite/unfavorite makes it one or not: here first, then the patch,
// undone if WhatsApp refuses it. The answer is a `sticker_fav` event.
func (b *Bridge) stickerFav(c Cmd) {
	x := b.stickerOf(c.Chat, c.ID)
	if x == nil {
		return
	}
	hash, enc := b64(x.GetFileSHA256()), b64(x.GetFileEncSHA256())
	// The phone's by its index or, should it index them otherwise, by the encrypted file's hash.
	var key string
	var stored []byte
	b.db.QueryRow(`SELECT hash, action FROM fav_stickers WHERE hash = ? OR (enc_hash != '' AND enc_hash = ?)`, hash, enc).Scan(&key, &stored)
	if key != "" && key != hash {
		logf("favorite sticker indexed as %s, not by its file hash %s", key, hash)
	}
	state := func(on bool) {
		emit(map[string]any{"t": "sticker_fav", "chat": c.Chat, "id": c.ID, "fav": on})
	}
	on := c.Action == "favorite"
	if (c.Action != "favorite" && c.Action != "unfavorite") || on == (key != "") {
		state(key != "")
		return
	}
	a := &waSyncAction.StickerAction{
		URL:             proto.String(x.GetURL()),
		FileEncSHA256:   x.GetFileEncSHA256(),
		MediaKey:        x.GetMediaKey(),
		Mimetype:        proto.String(x.GetMimetype()),
		Height:          proto.Uint32(x.GetHeight()),
		Width:           proto.Uint32(x.GetWidth()),
		DirectPath:      proto.String(x.GetDirectPath()),
		FileLength:      proto.Uint64(x.GetFileLength()),
		IsLottie:        proto.Bool(x.GetIsLottie()),
		IsAvatarSticker: proto.Bool(x.GetIsAvatar()),
	}
	if on {
		key = hash
		blob, _ := proto.Marshal(a)
		b.db.Exec(`INSERT OR REPLACE INTO fav_stickers (hash, enc_hash, action) VALUES (?, ?, ?)`, key, enc, blob)
	} else {
		// Taken out as the phone put it in.
		if old := (&waSyncAction.StickerAction{}); stored != nil && proto.Unmarshal(stored, old) == nil {
			a = old
		}
		b.db.Exec(`DELETE FROM fav_stickers WHERE hash = ?`, key)
	}
	a.IsFavorite = proto.Bool(on)
	state(on)
	version, _ := strconv.Atoi(b.meta("sticker_version"))
	if version <= 0 {
		version = stickerVersion
	}
	collection := b.meta("sticker_patch")
	if collection == "" {
		collection = string(appstate.WAPatchRegularLow)
	}
	patch := appstate.PatchInfo{
		Type: appstate.WAPatchName(collection),
		Mutations: []appstate.MutationInfo{{
			Index:   []string{appstate.IndexFavoriteSticker, key},
			Version: int32(version),
			Value:   &waSyncAction.SyncActionValue{StickerAction: a},
		}},
	}
	if err := b.cli.SendAppState(b.ctx, patch); err != nil {
		logf("sticker %s %s %s: %v", c.Action, c.Chat, c.ID, err)
		emit(map[string]any{"t": "error", "text": "não mudou as figurinhas favoritas: " + err.Error()})
		if on {
			b.db.Exec(`DELETE FROM fav_stickers WHERE hash = ?`, key)
		} else {
			a.IsFavorite = proto.Bool(true)
			blob, _ := proto.Marshal(a)
			b.db.Exec(`INSERT OR REPLACE INTO fav_stickers (hash, enc_hash, action) VALUES (?, ?, ?)`, key, b64(a.GetFileEncSHA256()), blob)
		}
		state(!on)
	}
}
