package main

// Sending files: photos, videos, audio and documents, one message each, as
// WhatsApp Web sends what's attached.

import (
	"bytes"
	"image"
	"image/color"
	_ "image/gif"
	"image/jpeg"
	_ "image/png"
	"mime"
	"net/http"
	"os"
	"os/exec"
	"path/filepath"
	"strconv"
	"strings"
	"sync"
	"time"

	"go.mau.fi/whatsmeow"
	"go.mau.fi/whatsmeow/proto/waE2E"
	"go.mau.fi/whatsmeow/types"
	"google.golang.org/protobuf/proto"
)

// What WhatsApp plays or shows inline; anything else goes as a document.
var inline = map[string][]string{
	"image": {"image/jpeg", "image/png"},
	"video": {"video/mp4", "video/3gpp"},
	"audio": {"audio/mpeg", "audio/mp4", "audio/aac", "audio/ogg", "audio/amr", "audio/x-m4a"},
}

// One file on its way: shown pending at once, uploaded in order.
type outFile struct {
	cmd  Cmd
	jid  types.JID
	id   types.MessageID
	ts   int64
	kind string
	mime string
	name string
	msg  *waE2E.Message
	// Text to send once the file's gone.
	after string
	// The message it replies to.
	quote *quoting
}

var (
	fileQ    = make(chan outFile, 256)
	fileOnce sync.Once
)

// sendFile shows the message right away, pending, and queues the upload:
// files attached together leave in the order they were given.
func (b *Bridge) sendFile(c Cmd) {
	jid, err := types.ParseJID(c.Chat)
	if err != nil || c.Path == "" {
		return
	}
	var readOnly string
	if b.db.QueryRow(`SELECT read_only FROM chats WHERE jid = ?`, c.Chat).Scan(&readOnly) == nil && readOnly != "" {
		emit(map[string]any{"t": "error", "text": readOnly})
		return
	}
	f, err := os.Open(c.Path)
	if err != nil {
		emit(map[string]any{"t": "error", "text": "não enviou: " + err.Error()})
		return
	}
	head := make([]byte, 512)
	n, _ := f.Read(head)
	f.Close()
	o := outFile{cmd: c, jid: jid, id: b.cli.GenerateMessageID(), ts: time.Now().Unix(), name: c.Name}
	if o.name == "" {
		o.name = filepath.Base(c.Path)
	}
	// By the file on disk: a WebP picked is sent from a PNG made of it.
	o.mime = mimeOf(c.Path, head[:n])
	o.kind = c.Kind
	if !contains(inline[o.kind], o.mime) {
		o.kind = "document"
	}
	caption := strings.TrimSpace(c.Caption)
	switch o.kind {
	case "image":
		o.msg = &waE2E.Message{ImageMessage: &waE2E.ImageMessage{Mimetype: proto.String(o.mime), Caption: opt(caption)}}
	case "video":
		o.msg = &waE2E.Message{VideoMessage: &waE2E.VideoMessage{Mimetype: proto.String(o.mime), Caption: opt(caption)}}
	case "audio":
		o.msg = &waE2E.Message{AudioMessage: &waE2E.AudioMessage{Mimetype: proto.String(o.mime), PTT: proto.Bool(false)}}
	default:
		title := strings.TrimSuffix(o.name, filepath.Ext(o.name))
		o.msg = &waE2E.Message{DocumentMessage: &waE2E.DocumentMessage{
			Mimetype: proto.String(o.mime), FileName: proto.String(o.name), Title: proto.String(title), Caption: opt(caption),
		}}
	}
	// An audio takes no caption on WhatsApp: it follows the audio as text.
	if o.kind == "audio" {
		o.after = caption
	}
	if o.quote = b.quoting(c.Chat, c.Reply); o.quote != nil {
		setContext(o.msg, o.quote.ctx)
	}
	b.storeFile(o, 0)
	fileOnce.Do(func() {
		go func() {
			for o := range fileQ {
				b.uploadFile(o)
			}
		}()
	})
	fileQ <- o
}

// storeFile writes (or rewrites) the message as the TUI reads it.
func (b *Bridge) storeFile(o outFile, status int) {
	text, _ := textOf(o.msg)
	m := &Msg{Chat: o.cmd.Chat, ID: o.id, Sender: b.me().String(), SenderName: "Você", FromMe: true, TS: o.ts, Text: text, Status: status}
	quote, quoteFrom, quoteID := o.quote.cols()
	b.db.Exec(`INSERT INTO messages (chat, id, sender, from_me, ts, text, quote, quote_from, quote_id, status) VALUES (?, ?, ?, 1, ?, ?, ?, ?, ?, ?)
		ON CONFLICT (chat, id) DO UPDATE SET text = excluded.text`, m.Chat, m.ID, m.Sender, m.TS, m.Text, quote, quoteFrom, quoteID, status)
	b.quote(m, o.quote)
	// No keys yet: it plays from the copy in the cache until it's uploaded.
	m.Media, _ = mediaOf(o.msg)
	b.storeMedia(m.Chat, m.ID, m.Media, nil, o.jid.String(), b.me().String())
	b.db.Exec(`INSERT INTO chats (jid, last_ts) VALUES (?, ?) ON CONFLICT (jid) DO UPDATE SET last_ts = max(last_ts, excluded.last_ts)`, m.Chat, m.TS)
	b.dirty.Store(true)
	emit(map[string]any{"t": "message", "msg": m})
}

func (b *Bridge) uploadFile(o outFile) {
	if o.cmd.Temp {
		defer os.Remove(o.cmd.Path)
	}
	fail := func(err error) {
		emit(map[string]any{"t": "error", "text": "não enviou " + o.name + ": " + err.Error()})
	}
	// Ours plays (or opens) without a download.
	if md, _ := mediaOf(o.msg); md != nil {
		if err := linkOrCopy(o.cmd.Path, mediaFile(o.cmd.Chat, o.id, md.Mime)); err != nil {
			logf("media cache %s: %v", o.id, err)
		}
	}
	media := map[string]whatsmeow.MediaType{
		"image": whatsmeow.MediaImage, "video": whatsmeow.MediaVideo, "audio": whatsmeow.MediaAudio, "document": whatsmeow.MediaDocument,
	}[o.kind]
	// What the bubble shows before it's downloaded: size, length, a thumbnail.
	var thumb []byte
	var w, h, secs uint32
	switch o.kind {
	case "image":
		if f, err := os.Open(o.cmd.Path); err == nil {
			if img, _, err := image.Decode(f); err == nil {
				w, h = uint32(img.Bounds().Dx()), uint32(img.Bounds().Dy())
				thumb = jpegThumb(img, 96)
			}
			f.Close()
		}
	case "video":
		secs = duration(o.cmd.Path)
		if frame := videoFrame(o.cmd.Path); frame != nil {
			w, h = uint32(frame.Bounds().Dx()), uint32(frame.Bounds().Dy())
			thumb = jpegThumb(frame, 96)
		}
	case "audio":
		secs = duration(o.cmd.Path)
	}

	f, err := os.Open(o.cmd.Path)
	if err != nil {
		fail(err)
		return
	}
	up, err := b.cli.UploadReader(b.ctx, f, nil, media)
	f.Close()
	if err != nil {
		fail(err)
		return
	}
	now := proto.Int64(time.Now().Unix())
	switch m := o.msg; {
	case m.ImageMessage != nil:
		x := m.ImageMessage
		x.URL, x.DirectPath, x.MediaKey, x.FileEncSHA256, x.FileSHA256, x.FileLength = &up.URL, &up.DirectPath, up.MediaKey, up.FileEncSHA256, up.FileSHA256, &up.FileLength
		x.MediaKeyTimestamp, x.JPEGThumbnail = now, thumb
		if w > 0 {
			x.Width, x.Height = &w, &h
		}
	case m.VideoMessage != nil:
		x := m.VideoMessage
		x.URL, x.DirectPath, x.MediaKey, x.FileEncSHA256, x.FileSHA256, x.FileLength = &up.URL, &up.DirectPath, up.MediaKey, up.FileEncSHA256, up.FileSHA256, &up.FileLength
		x.MediaKeyTimestamp, x.JPEGThumbnail, x.Seconds = now, thumb, &secs
		if w > 0 {
			x.Width, x.Height = &w, &h
		}
	case m.AudioMessage != nil:
		x := m.AudioMessage
		x.URL, x.DirectPath, x.MediaKey, x.FileEncSHA256, x.FileSHA256, x.FileLength = &up.URL, &up.DirectPath, up.MediaKey, up.FileEncSHA256, up.FileSHA256, &up.FileLength
		x.MediaKeyTimestamp, x.Seconds = now, &secs
		// The length shows in the stored text too.
		b.storeFile(o, 0)
	case m.DocumentMessage != nil:
		x := m.DocumentMessage
		x.URL, x.DirectPath, x.MediaKey, x.FileEncSHA256, x.FileSHA256, x.FileLength = &up.URL, &up.DirectPath, up.MediaKey, up.FileEncSHA256, up.FileSHA256, &up.FileLength
		x.MediaKeyTimestamp = now
	}
	if md, part := mediaOf(o.msg); md != nil {
		b.storeMedia(o.cmd.Chat, o.id, md, part, o.jid.String(), b.me().String())
		// Its bubble gets the picture now that there's a thumbnail.
		if o.kind == "image" || o.kind == "video" {
			if m := b.load(o.cmd.Chat, o.id); m != nil {
				emit(map[string]any{"t": "message", "msg": m})
			}
		}
	}
	if _, err := b.cli.SendMessage(b.ctx, o.jid, o.msg, whatsmeow.SendRequestExtra{ID: o.id}); err != nil {
		fail(err)
		return
	}
	b.setStatus(o.jid, []types.MessageID{o.id}, 1)
	b.dirty.Store(true)
	if o.after != "" {
		b.send(o.cmd.Chat, o.after, "")
	}
}

// mimeOf goes by the extension, then by the content.
func mimeOf(name string, head []byte) string {
	t := mime.TypeByExtension(strings.ToLower(filepath.Ext(name)))
	if t == "" {
		t = http.DetectContentType(head)
	}
	if i := strings.IndexByte(t, ';'); i >= 0 {
		t = strings.TrimSpace(t[:i])
	}
	if t == "application/ogg" || t == "audio/opus" {
		t = "audio/ogg"
	}
	return t
}

func contains(list []string, s string) bool {
	for _, x := range list {
		if x == s {
			return true
		}
	}
	return false
}

func opt(s string) *string {
	if s == "" {
		return nil
	}
	return &s
}

// jpegThumb scales the picture to fit `side`, averaging each box of pixels.
func jpegThumb(img image.Image, side int) []byte {
	r := img.Bounds()
	sw, sh := r.Dx(), r.Dy()
	if sw == 0 || sh == 0 {
		return nil
	}
	tw, th := side, side*sh/sw
	if sh > sw {
		tw, th = side*sw/sh, side
	}
	tw, th = max(tw, 1), max(th, 1)
	out := image.NewRGBA(image.Rect(0, 0, tw, th))
	for y := range th {
		y0, y1 := r.Min.Y+y*sh/th, r.Min.Y+max((y+1)*sh/th, y*sh/th+1)
		for x := range tw {
			x0, x1 := r.Min.X+x*sw/tw, r.Min.X+max((x+1)*sw/tw, x*sw/tw+1)
			var rs, gs, bs, n uint32
			// Some 8×8 samples a box are plenty, even for a big photo.
			step := max((x1-x0)/8, 1)
			for yy := y0; yy < y1; yy += step {
				for xx := x0; xx < x1; xx += step {
					cr, cg, cb, _ := img.At(xx, yy).RGBA()
					rs, gs, bs, n = rs+cr, gs+cg, bs+cb, n+1
				}
			}
			out.Set(x, y, color.RGBA{uint8(rs / n >> 8), uint8(gs / n >> 8), uint8(bs / n >> 8), 255})
		}
	}
	var buf bytes.Buffer
	if jpeg.Encode(&buf, out, &jpeg.Options{Quality: 75}) != nil {
		return nil
	}
	return buf.Bytes()
}

// duration asks ffprobe, when it's there; 0 otherwise.
func duration(path string) uint32 {
	out, err := exec.Command("ffprobe", "-v", "error", "-show_entries", "format=duration", "-of", "csv=p=0", path).Output()
	if err != nil {
		return 0
	}
	s, _ := strconv.ParseFloat(strings.TrimSpace(string(out)), 64)
	return uint32(s + 0.5)
}

// videoFrame is the video's first frame, through ffmpeg when it's there.
func videoFrame(path string) image.Image {
	out, err := exec.Command("ffmpeg", "-v", "error", "-i", path, "-frames:v", "1", "-f", "image2pipe", "-vcodec", "mjpeg", "-").Output()
	if err != nil {
		return nil
	}
	img, err := jpeg.Decode(bytes.NewReader(out))
	if err != nil {
		return nil
	}
	return img
}
