package main

// A business's profile as WhatsApp Web's contact info shows it.

import (
	"context"
	"fmt"
	"net/url"
	"os"
	"strconv"
	"strings"
	"time"

	"go.mau.fi/whatsmeow"
	waBinary "go.mau.fi/whatsmeow/binary"
	"go.mau.fi/whatsmeow/types"
)

func logf(format string, args ...any) {
	fmt.Fprintf(os.Stderr, "%s %s\n", time.Now().Format("15:04:05.000"), fmt.Sprintf(format, args...))
}

// bizHours is one opening of a business: day 0 is Monday, times are minutes
// after midnight.
type bizHours struct {
	Day   int    `json:"day"`
	Mode  string `json:"mode"`
	Open  int    `json:"open"`
	Close int    `json:"close"`
}

// bizProfile asks for a business's profile the way GetBusinessProfile does,
// but keeps what whatsmeow's parser drops: the description, websites and
// where it is. It fills p and returns the coordinates, if any; false when
// it isn't a business.
func (b *Bridge) bizProfile(ctx context.Context, jid types.JID, p map[string]any) (lat, lon float64, ok bool) {
	resp, err := b.cli.DangerousInternals().SendIQ(ctx, whatsmeow.DangerousInfoQuery{
		Namespace: "w:biz",
		Type:      "get",
		To:        types.ServerJID,
		Content: []waBinary.Node{{
			Tag:     "business_profile",
			Attrs:   waBinary.Attrs{"v": "244"},
			Content: []waBinary.Node{{Tag: "profile", Attrs: waBinary.Attrs{"jid": jid}}},
		}},
	})
	if err != nil {
		logf("business_profile %s: %v", jid, err)
		bp, err := b.cli.GetBusinessProfile(ctx, jid)
		if err != nil || bp == nil {
			return 0, 0, false
		}
		return 0, 0, putBiz(p, bp, "", nil)
	}
	node, found := resp.GetOptionalChildByTag("business_profile")
	if !found {
		return 0, 0, false
	}
	logf("business_profile %s: %s", jid, node.String())
	bp, err := b.cli.DangerousInternals().ParseBusinessProfile(&node)
	if err != nil {
		return 0, 0, false
	}
	prof := node.GetChildByTag("profile")
	text := func(n waBinary.Node) string {
		s, _ := n.Content.([]byte)
		return strings.TrimSpace(string(s))
	}
	var sites []string
	for _, w := range prof.GetChildrenByTag("website") {
		if s := text(w); s != "" {
			sites = append(sites, s)
		}
	}
	ok = putBiz(p, bp, text(prof.GetChildByTag("description")), sites)
	lat, _ = strconv.ParseFloat(text(prof.GetChildByTag("latitude")), 64)
	lon, _ = strconv.ParseFloat(text(prof.GetChildByTag("longitude")), 64)
	return lat, lon, ok || lat != 0 || lon != 0
}

// putBiz copies a business profile into the event; false when there's
// nothing in it, as for someone who isn't a business.
func putBiz(p map[string]any, bp *types.BusinessProfile, description string, sites []string) bool {
	var cats []string
	for _, c := range bp.Categories {
		cats = append(cats, c.Name)
	}
	days := map[string]int{"mon": 0, "tue": 1, "wed": 2, "thu": 3, "fri": 4, "sat": 5, "sun": 6}
	hours := []bizHours{}
	for _, h := range bp.BusinessHours {
		d, known := days[h.DayOfWeek]
		if !known {
			continue
		}
		open, _ := strconv.Atoi(h.OpenTime)
		closes, _ := strconv.Atoi(h.CloseTime)
		hours = append(hours, bizHours{d, h.Mode, open, closes})
	}
	p["description"] = description
	p["website"] = sites
	p["address"] = bp.Address
	p["email"] = bp.Email
	p["category"] = strings.Join(cats, ", ")
	p["hours"] = hours
	// The business's clock, for whether it's open now.
	if loc, err := time.LoadLocation(bp.BusinessHoursTimeZone); err == nil && bp.BusinessHoursTimeZone != "" {
		_, off := time.Now().In(loc).Zone()
		p["tz"] = off
	}
	return description != "" || len(sites) > 0 || bp.Address != "" || bp.Email != "" || len(cats) > 0 || len(hours) > 0
}


// mapsLink opens Google Maps on a business: on its coordinates when they
// came, else on its address.
func mapsLink(address string, lat, lon float64) string {
	q := address
	if lat != 0 || lon != 0 {
		q = fmt.Sprintf("%f,%f", lat, lon)
	}
	return "https://www.google.com/maps/search/?api=1&query=" + url.QueryEscape(q)
}
