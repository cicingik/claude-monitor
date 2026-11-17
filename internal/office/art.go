package office

import (
	"hash/fnv"
	"image"
	"image/color"
	"sync"

	"github.com/cicingik/claude-monitor/internal/pixel"
)

// All art here is original, drawn as pixel maps in code.

var rgb = pixel.RGB

// Scene colors.
var (
	cFrame     = rgb(0x11141c)
	cWallCap   = rgb(0x1a2131)
	cWallFace  = rgb(0x2b3a58)
	cWallLow   = rgb(0x253350)
	cBaseboard = rgb(0x1c2437)
	cWood      = rgb(0x8d5b36)
	cWoodAlt   = rgb(0x95633b)
	cWoodLine  = rgb(0x6f452a)
	cBlue      = rgb(0x3f6c9f)
	cBlueAlt   = rgb(0x3a6597)
	cCheckA    = rgb(0xe4e4e0)
	cCheckB    = rgb(0x2b2b2e)
	cOutline   = rgb(0x2a1d22)

	cDeskTop   = rgb(0xc99a62)
	cDeskHi    = rgb(0xdcb07a)
	cDeskFront = rgb(0xa06d40)
	cDeskLine  = rgb(0x4b2f1d)
	cLeadTop   = rgb(0xa8603f)
	cLeadHi    = rgb(0xbc7552)
	cLeadFront = rgb(0x7f4630)
	cGold      = rgb(0xf2c94c)
	cLegs      = rgb(0x45434f)

	cScreenOff = rgb(0x23332b)
	cScreenOn  = rgb(0x1f6e45)
	cScreenHot = rgb(0x2fcf6c)
	cScreenTxt = rgb(0xb8ffd2)
	cMeterBg   = rgb(0x2a1f1a)

	cSelect = rgb(0xfff27a)
	cCheck  = rgb(0x5fe07a)
	cWhite  = rgb(0xf6f6f2)
	cZ      = rgb(0xcfe3ff)
)

// LinkColor is the color of an interaction kind.
var LinkColor = map[string]color.RGBA{
	"spawn":    rgb(0xf5d442),
	"message":  rgb(0x4fdcec),
	"handback": rgb(0x5fe07a),
}

var (
	monitorArt = pixel.Grid(pixel.Palette{'o': rgb(0x5c533f), 'b': rgb(0xe8ddc0), 'B': rgb(0xc4b795), 'k': cScreenOff},
		".oooooooo.",
		"obbbbbbbbo",
		"obkkkkkkBo",
		"obkkkkkkBo",
		"obkkkkkkBo",
		"obkkkkkkBo",
		"obbbbbbbBo",
		".oooBBooo.",
		"..obbbbo..",
		".oBBBBBBo.",
	)
	monitorScreen = image.Rect(2, 2, 8, 6) // the k pixels above

	keyboardArt = pixel.Grid(pixel.Palette{'o': rgb(0x6f6c66), 'k': rgb(0xdcd9cf), 'K': rgb(0xa9a69c)},
		"oooooooo",
		"okKkKkKo",
		"oooooooo",
	)
	chairArt = pixel.Grid(pixel.Palette{'o': rgb(0x23452c), 'g': rgb(0x66b672), 'G': rgb(0x418553), 'l': cLegs},
		".oooooooo.",
		"oggggggggo",
		"oGGGGGGGGo",
		".oooooooo.",
		"..l....l..",
	)
	plantArt = pixel.Grid(pixel.Palette{'o': rgb(0x173d22), 'g': rgb(0x46a653), 'G': rgb(0x2f7c3c), 'p': rgb(0xbb6a3d), 'P': rgb(0x8a4a2a), 'd': rgb(0x4b2a1a)},
		"...o.o...",
		"..ogogo..",
		".oggoGgo.",
		"oGgGogGgo",
		".ogGgGgo.",
		"oGgogoGgo",
		".oGgGgGo.",
		"..ooooo..",
		".oppppPo.",
		".oppppPo.",
		"..opppo..",
		"..ddddd..",
	)
	deskPlantArt = pixel.Grid(pixel.Palette{'g': rgb(0x46a653), 'G': rgb(0x2f7c3c), 'p': rgb(0xbb6a3d)},
		".g.",
		"gGg",
		"GgG",
		"ppp",
	)
	clockArt = pixel.Grid(pixel.Palette{'r': rgb(0xd94a4a), 'w': cWhite, 'k': rgb(0x222222)},
		"..rrr..",
		".rwwwr.",
		"rwwkwwr",
		"rwwkkwr",
		"rwwwwwr",
		".rwwwr.",
		"..rrr..",
	)
	mugArt = pixel.Grid(pixel.Palette{'o': rgb(0x55555c), 'w': cWhite},
		"ooo.",
		"owwo",
		"owwo",
		"ooo.",
	)
	coffeeArt = pixel.Grid(pixel.Palette{'o': rgb(0x1d1d22), 'm': rgb(0x3d3d48), 'M': rgb(0xb8b8c4), 'r': rgb(0xe04a4a)},
		"oooooo",
		"ommmmo",
		"omMMmo",
		"omrmmo",
		"om..mo",
		"oooooo",
	)
	bubbleArt = pixel.Grid(pixel.Palette{'o': rgb(0x2a2a33), 'w': cWhite},
		".ooooooo.",
		"owwwwwwwo",
		"owwwwwwwo",
		".oowoooo.",
		"..oo.....",
	)
	checkArt = pixel.Grid(pixel.Palette{'o': rgb(0x1d4a2a), 'g': cCheck},
		"....og",
		"...ogo",
		"og.go.",
		".ogo..",
		"..o...",
	)
	arrowArt = pixel.Grid(pixel.Palette{'y': cSelect, 'o': rgb(0x3a3010)},
		"ooooooo",
		"oyyyyyo",
		".oyyyo.",
		"..oyo..",
		"...o...",
	)
	envelopeArt = pixel.Grid(pixel.Palette{'o': rgb(0x3a3a44), 'w': cWhite, 'f': rgb(0xc8c8d0)},
		"ooooooo",
		"owfwfwo",
		"owwfwwo",
		"ooooooo",
	)
)

// Character templates, 10 pixels wide. Runes: o outline, h/H hair, s/S skin,
// e eyes, c/C shirt, w collar, t tie, p pants, f shoes.
var (
	headFront = []string{
		"..oooooo..",
		".ohhhhhho.",
		"ohhhhhhhho",
		"ohhsssshho",
		"ohsessesho",
		"osssssssso",
		".oSssssSo.",
		"..oooooo..",
	}
	headBack = []string{
		"..oooooo..",
		".ohhhhhho.",
		"ohhhhhhhho",
		"ohhhhhhhho",
		"ohhhhhhhho",
		"ohHhhhhHho",
		".oHHHHHHo.",
		"..osSSso..",
	}
	// Hair style overrides: row index -> row, per style, for the front and back.
	styleFront = []map[int]string{
		{},
		{3: "ohhsssshho", 5: "ohssssssho", 6: "ohSssssSho", 7: "ohoooooho."},
		{0: "...oooo...", 1: "..ohhhho..", 2: ".ohhhhhho."},
	}
	styleBack = []map[int]string{
		{},
		{6: "ohHHHHHHho", 7: "ohhsSSshho"},
		{0: "...oooo...", 1: "..ohhhho..", 2: ".ohhhhhho."},
	}
	bodyFront = []string{
		".ocwttwco.",
		"occcttccco",
		"oscccccCso",
	}
	bodyBack = []string{
		".occcccco.",
		"occcccccco",
		"oCccccccCo",
		".oCCCCCCo.",
		"..oooooo..",
	}
	legsStand = [][]string{
		{".oppppppo.", ".opo..opo.", ".ofo..ofo."},
		{".oppppppo.", "..opooqo..", "..ofo.fo.."},
	}
	legsSit = []string{"oppppppppo", ".ofo..ofo."}
)

// look is the appearance of a built-in character.
type look struct {
	hair, skin, shirt color.RGBA
	style             int
	lead              bool
}

var (
	hairs  = []uint32{0x3d2516, 0x1d1b22, 0xe8c66a, 0xc4552c, 0xe6e2ee, 0x7a4a2e, 0x5868d0}
	skins  = []uint32{0xf7d2ad, 0xe8b48a, 0xc98e62, 0x8d5a3b}
	shirts = []uint32{0xe0783a, 0x4a86d6, 0x5fbf6a, 0xd64c74, 0x9a5ad6, 0xe7c04a, 0x39b2ac, 0xececec}
)

func hash(s string) uint32 {
	h := fnv.New32a()
	h.Write([]byte(s))
	return h.Sum32()
}

// lookFor derives a stable look: hair and skin from the agent id, shirt from
// its type, so agents of one type share a shirt color. The lead wears a suit.
func lookFor(a Actor) look {
	h := hash(a.AgentID)
	l := look{
		hair:  rgb(hairs[h%uint32(len(hairs))]),
		skin:  rgb(skins[(h/7)%uint32(len(skins))]),
		style: int((h / 31) % 3),
		shirt: rgb(shirts[hash(a.Type)%uint32(len(shirts))]),
		lead:  a.Lead,
	}
	if a.Lead {
		l.shirt = rgb(0x46578f)
	}
	return l
}

func shade(c color.RGBA, f float64) color.RGBA {
	return color.RGBA{uint8(float64(c.R) * f), uint8(float64(c.G) * f), uint8(float64(c.B) * f), 255}
}

func (l look) palette(eyesClosed bool) pixel.Palette {
	p := pixel.Palette{
		'o': cOutline, 'h': l.hair, 'H': shade(l.hair, 0.75), 's': l.skin, 'S': shade(l.skin, 0.85),
		'e': rgb(0x1c1418), 'c': l.shirt, 'C': shade(l.shirt, 0.8), 'w': l.shirt, 't': l.shirt,
		'p': rgb(0x3b3f5c), 'q': rgb(0x2e3149), 'f': rgb(0x22222b),
	}
	if l.lead {
		p['w'], p['t'] = cWhite, rgb(0xd64545)
	}
	if eyesClosed {
		p['e'] = shade(l.skin, 0.6)
	}
	return p
}

type charKey struct {
	l      look
	pose   Pose
	frame  int
	closed bool
}

var (
	charMu    sync.Mutex
	charCache = map[charKey]*image.RGBA{}
)

// character draws a built-in character. frame picks typing hands (SitBack) or
// the walking step (Stand).
func character(l look, pose Pose, frame int, closed bool) *image.RGBA {
	k := charKey{l, pose, frame & 1, closed}
	charMu.Lock()
	defer charMu.Unlock()
	if img, ok := charCache[k]; ok {
		return img
	}
	var rows []string
	switch pose {
	case SitBack:
		rows = override(headBack, styleBack[l.style])
		if frame&1 == 0 {
			rows[7] = "s" + rows[7][1:]
		} else {
			rows[7] = rows[7][:9] + "s"
		}
		rows = append(rows, bodyBack...)
	case SitFront:
		rows = append(override(headFront, styleFront[l.style]), bodyFront...)
		rows = append(rows, legsSit...)
	default:
		rows = append(override(headFront, styleFront[l.style]), bodyFront...)
		rows = append(rows, legsStand[frame&1]...)
	}
	img := pixel.Grid(l.palette(closed), rows...)
	charCache[k] = img
	return img
}

func override(base []string, o map[int]string) []string {
	out := append([]string(nil), base...)
	for i, r := range o {
		out[i] = r
	}
	return out
}
