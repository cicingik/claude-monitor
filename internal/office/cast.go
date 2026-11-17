package office

import (
	"fmt"
	"image"
	"os"
	"path/filepath"
	"sort"
	"strings"

	"github.com/cicingik/claude-monitor/internal/pixel"
)

// Cast holds custom character sprites loaded from a directory of .gif and
// .png files. For each actor it picks, in order: the file named after the
// agent type (general-purpose.gif, session.png, ...), else one of the files
// not named after a type seen so far, chosen by a hash of the agent id, else
// nothing, which draws the built-in character. A nil Cast is valid and empty.
type Cast struct {
	// Note is one line for the status bar, "" when there is nothing to say.
	Note  string
	files []castFile // sorted by name
	seen  map[string]bool
}

type castFile struct {
	stem   string // lower-case file name without extension
	sprite *pixel.Sprite
}

// LoadCast loads every sprite in dir. It never fails: unreadable or broken
// files are skipped and summed up in Note. explicit says the user named dir,
// so a missing or empty one deserves a note.
func LoadCast(dir string, explicit bool) *Cast {
	c := &Cast{seen: map[string]bool{}}
	entries, err := os.ReadDir(dir)
	if err != nil {
		if explicit {
			c.Note = "sprites: cannot read " + dir + ", using built-in characters"
		}
		return c
	}
	var bad []string
	for _, e := range entries {
		ext := strings.ToLower(filepath.Ext(e.Name()))
		if e.IsDir() || (ext != ".gif" && ext != ".png") {
			continue
		}
		s, err := pixel.Load(filepath.Join(dir, e.Name()), image.Pt(CharW, CharH))
		if err != nil {
			bad = append(bad, e.Name())
			continue
		}
		c.files = append(c.files, castFile{strings.ToLower(strings.TrimSuffix(e.Name(), filepath.Ext(e.Name()))), s})
	}
	sort.Slice(c.files, func(i, j int) bool { return c.files[i].stem < c.files[j].stem })
	switch {
	case len(bad) > 0:
		c.Note = fmt.Sprintf("sprites: %d loaded, %d skipped (%s)", len(c.files), len(bad), strings.Join(bad, ", "))
	case len(c.files) > 0:
		c.Note = fmt.Sprintf("sprites: %d from %s", len(c.files), dir)
	case explicit:
		c.Note = "sprites: no .gif or .png in " + dir
	}
	return c
}

// Dress sets the custom sprite of every actor in sc that has one.
func (c *Cast) Dress(sc *Scene) {
	if c == nil || len(c.files) == 0 {
		return
	}
	for _, a := range sc.Actors {
		if a.Type != "" {
			c.seen[strings.ToLower(a.Type)] = true
		}
	}
	var pool []*pixel.Sprite
	for _, f := range c.files {
		if !c.seen[f.stem] {
			pool = append(pool, f.sprite)
		}
	}
	for i := range sc.Actors {
		sc.Actors[i].Sprite = c.pick(sc.Actors[i], pool)
	}
}

func (c *Cast) pick(a Actor, pool []*pixel.Sprite) *pixel.Sprite {
	t := strings.ToLower(a.Type)
	for _, f := range c.files {
		if t != "" && f.stem == t {
			return f.sprite
		}
	}
	if len(pool) == 0 {
		return nil
	}
	return pool[hash(a.AgentID)%uint32(len(pool))]
}
