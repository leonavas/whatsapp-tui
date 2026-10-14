package main

// The profile picture at full size, for the info panel and to open big: in
// the media cache, so the sweep takes it once it's not been seen in a while.

import (
	"context"
	"errors"
	"os"
	"path/filepath"
	"strings"
	"time"

	"go.mau.fi/whatsmeow"
	"go.mau.fi/whatsmeow/types"
)

// photo tells the TUI where chat's full-size picture is, fetched once per
// picture (it's named by its id); an empty path when there's none to show:
// not set, hidden from you, or the official account's.
func (b *Bridge) photo(chat string, jid types.JID) {
	out := map[string]any{"t": "photo", "chat": chat, "path": ""}
	defer func() { emit(out) }()
	if jid == types.PSAJID {
		return
	}
	ctx, cancel := context.WithTimeout(b.ctx, 20*time.Second)
	defer cancel()
	info, err := b.cli.GetProfilePictureInfo(ctx, jid, &whatsmeow.GetProfilePictureParams{})
	if errors.Is(err, whatsmeow.ErrProfilePictureNotSet) || errors.Is(err, whatsmeow.ErrProfilePictureUnauthorized) {
		return
	}
	if err != nil || info == nil || info.URL == "" {
		logf("photo %s: %v", chat, err)
		out["error"] = "não carregou a foto"
		return
	}
	dir := mediaDir()
	os.MkdirAll(dir, 0o700)
	name := "photo_" + strings.NewReplacer("@", "_", ".", "_", ":", "_", "/", "_").Replace(chat) + "_"
	path := filepath.Join(dir, name+info.ID+".jpg")
	if st, err := os.Stat(path); err == nil && st.Size() > 0 {
		touch(path)
		out["path"] = path
		return
	}
	if err := download(info.URL, path); err != nil {
		logf("photo %s: %v", chat, err)
		out["error"] = "não carregou a foto"
		return
	}
	// The ones it had before.
	old, _ := filepath.Glob(filepath.Join(dir, name+"*.jpg"))
	for _, p := range old {
		if p != path {
			os.Remove(p)
		}
	}
	out["path"] = path
}
