package scan

import (
	"bufio"
	"bytes"
	"encoding/json"
	"errors"
	"io"
	"os"
	"regexp"
	"strings"
	"time"
)

// line holds the fields of one transcript line that the monitor uses. It is
// decoded in one pass; everything else in the line is skipped by the decoder.
type line struct {
	Type      string    `json:"type"`
	Timestamp time.Time `json:"timestamp"`
	Cwd       string    `json:"cwd"`
	AITitle   string    `json:"aiTitle"`
	Message   struct {
		ID    string `json:"id"`
		Model string `json:"model"`
		Usage *struct {
			Input      int64 `json:"input_tokens"`
			Output     int64 `json:"output_tokens"`
			CacheRead  int64 `json:"cache_read_input_tokens"`
			CacheWrite int64 `json:"cache_creation_input_tokens"`
		} `json:"usage"`
		// A list of blocks, or a plain string in user prompts. A string fails
		// to decode here and is recovered by stringContent.
		Content []contentItem `json:"content"`
	} `json:"message"`
}

type contentItem struct {
	Type      string    `json:"type"`
	Name      string    `json:"name"`
	ID        string    `json:"id"`
	Text      string    `json:"text"`
	Input     toolInput `json:"input"`
	ToolUseID string    `json:"tool_use_id"`
}

// toolInput is the union of tool_use input fields the monitor reads. Large
// fields such as a Write body or an Agent prompt are not listed, so they are
// skipped rather than allocated.
type toolInput struct {
	Command      string          `json:"command"`
	FilePath     string          `json:"file_path"`
	Path         string          `json:"path"`
	Pattern      string          `json:"pattern"`
	Query        string          `json:"query"`
	URL          string          `json:"url"`
	Skill        string          `json:"skill"`
	Description  string          `json:"description"`
	SubagentType string          `json:"subagent_type"`
	Name         string          `json:"name"`
	To           string          `json:"to"`
	Summary      string          `json:"summary"`
	Message      json.RawMessage `json:"message"` // string or object
}

// stringContent recovers message.content when it is a plain string.
func stringContent(b []byte) (string, bool) {
	var l struct {
		Message struct {
			Content string `json:"content"`
		} `json:"message"`
	}
	err := json.Unmarshal(b, &l)
	return l.Message.Content, err == nil && l.Message.Content != ""
}

// spawn is an Agent/Task tool_use seen in a transcript.
type spawn struct {
	At                        time.Time
	Description, SubagentType string
	Name                      string // optional input.name, used to resolve SendMessage targets
}

// sent is a SendMessage tool_use seen in a transcript.
type sent struct {
	At        time.Time
	To, Label string
}

// fileState is the parsed state of one transcript, advanced incrementally.
type fileState struct {
	offset int64 // bytes consumed: always ends just after a '\n'

	title, model, cwd string
	firstAt           time.Time
	lastAt            time.Time
	usage             map[string]Usage  // per message.id, so repeated lines count once
	msgModel          map[string]string // message.id -> model
	events            []Event
	lastToolID        string
	lastTool          string
	resolved          map[string]bool  // tool_use ids that have a tool_result
	spawns            map[string]spawn // tool_use id -> Agent/Task call
	sends             []sent
	handbacks         []sent // To holds the sender agent id
}

func newFileState() *fileState {
	return &fileState{
		usage:    map[string]Usage{},
		msgModel: map[string]string{},
		resolved: map[string]bool{},
		spawns:   map[string]spawn{},
	}
}

// pending reports whether the most recent tool_use still waits for its result.
func (f *fileState) pending() bool { return f.lastToolID != "" && !f.resolved[f.lastToolID] }

// readFrom consumes complete lines from f.offset to EOF. A trailing line
// without '\n' is left for the next call. If the file shrank, the caller resets.
func (f *fileState) readFrom(path string) error {
	fh, err := os.Open(path)
	if err != nil {
		return err
	}
	defer fh.Close()
	if _, err := fh.Seek(f.offset, io.SeekStart); err != nil {
		return err
	}
	r := bufio.NewReaderSize(fh, 256*1024)
	for {
		b, err := r.ReadBytes('\n')
		if err != nil {
			if errors.Is(err, io.EOF) {
				return nil // partial or empty tail: not consumed
			}
			return err
		}
		f.offset += int64(len(b))
		f.parseLine(b)
	}
}

var (
	typeKey       = []byte(`"type"`)
	relevantTypes = [][]byte{[]byte(`"assistant"`), []byte(`"user"`), []byte(`"ai-title"`)}
)

// mayBeRelevant is a byte-level filter run before any JSON decoding: it is
// false only when no "type" key in b has an assistant, user or ai-title value,
// so such lines (about 40% of transcript bytes) are never decoded.
func mayBeRelevant(b []byte) bool {
	for {
		i := bytes.Index(b, typeKey)
		if i < 0 {
			return false
		}
		b = bytes.TrimLeft(b[i+len(typeKey):], " \t")
		if len(b) == 0 || b[0] != ':' {
			continue
		}
		v := bytes.TrimLeft(b[1:], " \t")
		for _, t := range relevantTypes {
			if bytes.HasPrefix(v, t) {
				return true
			}
		}
	}
}

func (f *fileState) parseLine(b []byte) {
	if !mayBeRelevant(b) {
		return
	}
	var l line
	var texts []string
	if err := json.Unmarshal(b, &l); err != nil {
		var te *json.UnmarshalTypeError
		if !errors.As(err, &te) {
			return // malformed line: skip
		}
		// A field had an unexpected type; the rest was still decoded.
		if s, ok := stringContent(b); ok && l.Message.Content == nil {
			texts = append(texts, s)
		}
	}
	if f.cwd == "" && l.Cwd != "" {
		f.cwd = l.Cwd
	}
	switch l.Type {
	case "assistant":
		f.touch(l.Timestamp)
		f.parseAssistant(l)
	case "user":
		f.touch(l.Timestamp)
		f.parseUser(l, texts)
	case "ai-title":
		if l.AITitle != "" {
			f.title = l.AITitle
		}
	}
}

func (f *fileState) touch(t time.Time) {
	if t.IsZero() {
		return
	}
	if f.firstAt.IsZero() || t.Before(f.firstAt) {
		f.firstAt = t
	}
	if t.After(f.lastAt) {
		f.lastAt = t
	}
}

func (f *fileState) parseAssistant(l line) {
	m := l.Message
	if m.Model != "" && !strings.HasPrefix(m.Model, "<") { // skip "<synthetic>"
		f.model = m.Model
	}
	if m.ID != "" {
		if _, seen := f.usage[m.ID]; !seen || m.Usage != nil {
			var u Usage
			if m.Usage != nil {
				u = Usage{m.Usage.Input, m.Usage.Output, m.Usage.CacheRead, m.Usage.CacheWrite}
			}
			f.usage[m.ID] = u // same message.id on several lines: counted once
		}
		if m.Model != "" && !strings.HasPrefix(m.Model, "<") {
			f.msgModel[m.ID] = m.Model
		}
	}
	for _, c := range m.Content {
		switch c.Type {
		case "tool_use":
			sum := toolSummary(c.Name, c.Input)
			f.lastToolID, f.lastTool = c.ID, sum
			f.addEvent(Event{At: l.Timestamp, Kind: "tool_use", Tool: c.Name, Summary: sum})
			f.recordInteraction(l.Timestamp, c)
		case "text":
			if s := oneLine(c.Text, 60); s != "" {
				f.addEvent(Event{At: l.Timestamp, Kind: "text", Summary: s})
			}
		}
	}
}

func (f *fileState) recordInteraction(at time.Time, c contentItem) {
	in := c.Input
	switch c.Name {
	case "Agent", "Task":
		f.spawns[c.ID] = spawn{At: at, Description: in.Description, SubagentType: in.SubagentType, Name: in.Name}
	case "SendMessage":
		if in.To == "" {
			return
		}
		label := in.Summary
		if label == "" {
			_ = json.Unmarshal(in.Message, &label) // only when message is a string
		}
		f.sends = append(f.sends, sent{At: at, To: in.To, Label: oneLine(label, 40)})
	}
}

var agentMessageRe = regexp.MustCompile(`<agent-message from="([A-Za-z0-9_-]+)"`)

func (f *fileState) parseUser(l line, texts []string) {
	for _, it := range l.Message.Content {
		switch it.Type {
		case "tool_result":
			if it.ToolUseID != "" {
				f.resolved[it.ToolUseID] = true
			}
		case "text":
			texts = append(texts, it.Text)
		}
	}
	for _, t := range texts {
		if mm := agentMessageRe.FindStringSubmatch(t); mm != nil {
			f.handbacks = append(f.handbacks, sent{At: l.Timestamp, To: mm[1]})
			f.addEvent(Event{At: l.Timestamp, Kind: "handback", Summary: "agent-message from " + mm[1]})
			continue
		}
		if strings.HasPrefix(strings.TrimSpace(t), "<") {
			continue // system reminders, command wrappers, notifications
		}
		if s := oneLine(t, 60); s != "" {
			f.addEvent(Event{At: l.Timestamp, Kind: "prompt", Summary: s})
		}
	}
}

func (f *fileState) addEvent(e Event) {
	f.events = append(f.events, e)
	if n := len(f.events); n > MaxEvents {
		f.events = append(f.events[:0:0], f.events[n-MaxEvents:]...)
	}
}

// toolSummary renders "Tool: short arg" from a tool_use input.
func toolSummary(name string, in toolInput) string {
	var arg string
	switch name {
	case "Bash":
		arg = in.Command
	case "Read", "Write", "Edit", "NotebookEdit":
		arg = shortPath(in.FilePath)
	case "Agent", "Task":
		arg = in.Description
		if in.SubagentType != "" {
			arg += " (" + in.SubagentType + ")"
		}
	case "SendMessage":
		arg = "to " + in.To
	default:
		for _, v := range []string{in.Description, in.FilePath, in.Path, in.Pattern, in.Query, in.Skill, in.URL, in.Name} {
			if v != "" {
				arg = v
				break
			}
		}
	}
	if arg = oneLine(arg, 50); arg == "" {
		return name
	}
	return name + ": " + arg
}

// shortPath keeps the last two path segments.
func shortPath(p string) string {
	parts := strings.Split(strings.TrimRight(p, "/"), "/")
	if len(parts) <= 3 {
		return p
	}
	return "…/" + strings.Join(parts[len(parts)-2:], "/")
}

// oneLine returns the first non-empty line of s, trimmed and cut to max runes.
func oneLine(s string, max int) string {
	for _, ln := range strings.Split(s, "\n") {
		if ln = strings.TrimSpace(ln); ln != "" {
			return Truncate(ln, max)
		}
	}
	return ""
}

// Truncate cuts s to at most max runes, marking the cut with "…".
func Truncate(s string, max int) string {
	r := []rune(s)
	if len(r) <= max {
		return s
	}
	if max <= 1 {
		return string(r[:max])
	}
	return string(r[:max-1]) + "…"
}
