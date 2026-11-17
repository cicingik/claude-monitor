package scan

import (
	"bufio"
	"encoding/json"
	"errors"
	"io"
	"os"
	"regexp"
	"strings"
	"time"
)

// line holds the fields of one transcript line that the monitor uses.
// Everything else in the line is ignored.
type line struct {
	Type      string          `json:"type"`
	Timestamp time.Time       `json:"timestamp"`
	Cwd       string          `json:"cwd"`
	AITitle   string          `json:"aiTitle"`
	Message   json.RawMessage `json:"message"`
}

type assistantMessage struct {
	ID    string `json:"id"`
	Model string `json:"model"`
	Usage *struct {
		Input      int64 `json:"input_tokens"`
		Output     int64 `json:"output_tokens"`
		CacheRead  int64 `json:"cache_read_input_tokens"`
		CacheWrite int64 `json:"cache_creation_input_tokens"`
	} `json:"usage"`
	Content []contentItem `json:"content"`
}

type userMessage struct {
	Content json.RawMessage `json:"content"` // string or []contentItem
}

type contentItem struct {
	Type      string          `json:"type"`
	Name      string          `json:"name"`
	ID        string          `json:"id"`
	Text      string          `json:"text"`
	Input     json.RawMessage `json:"input"`
	ToolUseID string          `json:"tool_use_id"`
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

func (f *fileState) parseLine(b []byte) {
	var l line
	if err := json.Unmarshal(b, &l); err != nil {
		return // malformed line: skip
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
		f.parseUser(l)
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
	var m assistantMessage
	if err := json.Unmarshal(l.Message, &m); err != nil {
		return
	}
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
	var in struct {
		Description  string `json:"description"`
		SubagentType string `json:"subagent_type"`
		Name         string `json:"name"`
		To           string `json:"to"`
		Summary      string `json:"summary"`
		Message      any    `json:"message"`
	}
	_ = json.Unmarshal(c.Input, &in)
	switch c.Name {
	case "Agent", "Task":
		f.spawns[c.ID] = spawn{At: at, Description: in.Description, SubagentType: in.SubagentType, Name: in.Name}
	case "SendMessage":
		if in.To == "" {
			return
		}
		label := in.Summary
		if s, ok := in.Message.(string); ok && label == "" {
			label = s
		}
		f.sends = append(f.sends, sent{At: at, To: in.To, Label: oneLine(label, 40)})
	}
}

var agentMessageRe = regexp.MustCompile(`<agent-message from="([A-Za-z0-9_-]+)"`)

func (f *fileState) parseUser(l line) {
	var m userMessage
	if err := json.Unmarshal(l.Message, &m); err != nil || len(m.Content) == 0 {
		return
	}
	var texts []string
	var s string
	if json.Unmarshal(m.Content, &s) == nil {
		texts = append(texts, s)
	} else {
		var items []contentItem
		if json.Unmarshal(m.Content, &items) != nil {
			return
		}
		for _, it := range items {
			switch it.Type {
			case "tool_result":
				if it.ToolUseID != "" {
					f.resolved[it.ToolUseID] = true
				}
			case "text":
				texts = append(texts, it.Text)
			}
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
func toolSummary(name string, raw json.RawMessage) string {
	var in map[string]any
	_ = json.Unmarshal(raw, &in)
	str := func(k string) string { s, _ := in[k].(string); return s }
	var arg string
	switch name {
	case "Bash":
		arg = str("command")
	case "Read", "Write", "Edit", "NotebookEdit":
		arg = shortPath(str("file_path"))
	case "Agent", "Task":
		arg = str("description")
		if t := str("subagent_type"); t != "" {
			arg += " (" + t + ")"
		}
	case "SendMessage":
		arg = "to " + str("to")
	case "WebFetch":
		arg = str("url")
	case "WebSearch":
		arg = str("query")
	case "Skill":
		arg = str("skill")
	default:
		for _, k := range []string{"description", "file_path", "path", "pattern", "query", "skill", "name", "url"} {
			if arg = str(k); arg != "" {
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
