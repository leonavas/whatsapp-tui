package main

// Businesses' (and WhatsApp's own) laid-out messages: templates, buttons,
// lists and interactive ones, read as WhatsApp Web shows them: a picture or
// a title on top, the text, a footer and buttons at the bottom.

import (
	"encoding/json"
	"strings"

	"go.mau.fi/whatsmeow/proto/waE2E"
)

// Card is what such a message has besides its text, which goes in Msg.Text.
type Card struct {
	Title   string   `json:"title,omitempty"`
	Footer  string   `json:"footer,omitempty"`
	Buttons []Button `json:"buttons,omitempty"`
}

// Button is one of the rows at the bottom: a link when it has a URL; the
// rest (quick replies, calls, lists) only show.
type Button struct {
	Text string `json:"text"`
	URL  string `json:"url,omitempty"`
	// url, reply, call, copy or list: its icon.
	Kind string `json:"kind,omitempty"`
}

// laidOut is a card's parts: its text, and a message with only its header's
// photo, video or document, if it has one, for mediaOf.
type laidOut struct {
	card   Card
	body   string
	header *waE2E.Message
}

// cardOf reads the message's card; nil when it isn't one.
func cardOf(m *waE2E.Message) *laidOut {
	switch {
	case m == nil:
		return nil
	case m.GetTemplateMessage() != nil:
		t := m.GetTemplateMessage()
		if h := t.GetHydratedTemplate(); h != nil {
			return hydrated(h)
		}
		if h := t.GetHydratedFourRowTemplate(); h != nil {
			return hydrated(h)
		}
		if i := t.GetInteractiveMessageTemplate(); i != nil {
			return interactive(i)
		}
	case m.GetInteractiveMessage() != nil:
		return interactive(m.GetInteractiveMessage())
	case m.GetButtonsMessage() != nil:
		b := m.GetButtonsMessage()
		c := &laidOut{body: b.GetContentText(), card: Card{Title: b.GetText(), Footer: b.GetFooterText()}}
		c.header = headerOf(b.GetImageMessage(), b.GetVideoMessage(), b.GetDocumentMessage())
		for _, x := range b.GetButtons() {
			btn := Button{Text: x.GetButtonText().GetDisplayText(), Kind: "reply"}
			if f := x.GetNativeFlowInfo(); f != nil {
				btn = flowButton(f.GetName(), f.GetParamsJSON())
			}
			c.add(btn)
		}
		return c.done()
	case m.GetListMessage() != nil:
		l := m.GetListMessage()
		c := &laidOut{body: l.GetDescription(), card: Card{Title: l.GetTitle(), Footer: l.GetFooterText()}}
		c.add(Button{Text: l.GetButtonText(), Kind: "list"})
		return c.done()
	}
	return nil
}

func hydrated(h *waE2E.TemplateMessage_HydratedFourRowTemplate) *laidOut {
	c := &laidOut{body: h.GetHydratedContentText(), card: Card{Title: h.GetHydratedTitleText(), Footer: h.GetHydratedFooterText()}}
	c.header = headerOf(h.GetImageMessage(), h.GetVideoMessage(), h.GetDocumentMessage())
	for _, b := range h.GetHydratedButtons() {
		switch {
		case b.GetUrlButton() != nil:
			c.add(Button{Text: b.GetUrlButton().GetDisplayText(), URL: b.GetUrlButton().GetURL(), Kind: "url"})
		case b.GetCallButton() != nil:
			c.add(Button{Text: b.GetCallButton().GetDisplayText(), Kind: "call"})
		case b.GetQuickReplyButton() != nil:
			c.add(Button{Text: b.GetQuickReplyButton().GetDisplayText(), Kind: "reply"})
		}
	}
	return c.done()
}

func interactive(i *waE2E.InteractiveMessage) *laidOut {
	h := i.GetHeader()
	c := &laidOut{body: i.GetBody().GetText(), card: Card{Title: h.GetTitle(), Footer: i.GetFooter().GetText()}}
	c.header = headerOf(h.GetImageMessage(), h.GetVideoMessage(), h.GetDocumentMessage())
	for _, b := range i.GetNativeFlowMessage().GetButtons() {
		c.add(flowButton(b.GetName(), b.GetButtonParamsJSON()))
	}
	// A carousel: its first card's.
	if cards := i.GetCarouselMessage().GetCards(); len(cards) > 0 && c.body == "" {
		return interactive(cards[0])
	}
	return c.done()
}

// flowButton reads a native flow button: cta_url carries the link,
// quick_reply, cta_call, cta_copy and single_select only a label.
func flowButton(name, params string) Button {
	var p struct {
		DisplayText string `json:"display_text"`
		Title       string `json:"title"`
		URL         string `json:"url"`
	}
	_ = json.Unmarshal([]byte(params), &p)
	b := Button{Text: p.DisplayText, Kind: "reply"}
	if b.Text == "" {
		b.Text = p.Title
	}
	switch name {
	case "cta_url":
		b.Kind, b.URL = "url", p.URL
	case "cta_call":
		b.Kind = "call"
	case "cta_copy":
		b.Kind = "copy"
	case "single_select":
		b.Kind = "list"
	}
	// Only links a browser opens.
	if l := strings.ToLower(b.URL); !strings.HasPrefix(l, "https://") && !strings.HasPrefix(l, "http://") {
		b.URL = ""
	}
	return b
}

func (c *laidOut) add(b Button) {
	b.Text = strings.TrimSpace(b.Text)
	if b.Text != "" {
		c.card.Buttons = append(c.card.Buttons, b)
	}
}

// done tidies it: with no text, the title is the text (what the chat list
// previews); nil when there's nothing to show.
func (c *laidOut) done() *laidOut {
	c.body = strings.TrimSpace(c.body)
	c.card.Title = strings.TrimSpace(c.card.Title)
	c.card.Footer = strings.TrimSpace(c.card.Footer)
	if c.body == "" {
		c.body, c.card.Title = c.card.Title, ""
	}
	if c.body == "" && c.header == nil && len(c.card.Buttons) == 0 {
		return nil
	}
	return c
}

func headerOf(img *waE2E.ImageMessage, vid *waE2E.VideoMessage, doc *waE2E.DocumentMessage) *waE2E.Message {
	switch {
	case img != nil:
		return &waE2E.Message{ImageMessage: img}
	case vid != nil:
		return &waE2E.Message{VideoMessage: vid}
	case doc != nil:
		return &waE2E.Message{DocumentMessage: doc}
	}
	return nil
}

// text is how the card reads in the chat list and in quotes: its text, or
// what its header is.
func (c *laidOut) text() string {
	if c.body != "" {
		return c.body
	}
	if c.header != nil {
		if t, ok := textOf(c.header); ok {
			return t
		}
	}
	return c.card.Buttons[0].Text
}

// cardJSON is the card as the messages table keeps it; empty for none.
func cardJSON(c *laidOut) string {
	if c == nil || (c.card.Title == "" && c.card.Footer == "" && len(c.card.Buttons) == 0) {
		return ""
	}
	b, _ := json.Marshal(c.card)
	return string(b)
}
