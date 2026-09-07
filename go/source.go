package entryconf

import (
	"os"
	"path/filepath"
	"sort"
	"strings"
)

// fileSource is where the loader gets its bytes. Load reads the real
// filesystem; Open reads it while recording, and a plan's candidate evaluation
// (SPEC §10.6) reads the snapshot's recorded bytes with the edited document
// replaced in memory — falling back to disk only for a file the snapshot never
// saw, such as a newly written @file: reference.
type fileSource interface {
	readFile(path string) ([]byte, error)
	isFile(path string) bool
	// envFileNames lists the *.env files directly in dir (SPEC §4).
	envFileNames(dir string) ([]string, error)
}

// osSource is the real filesystem.
type osSource struct{}

func (osSource) readFile(path string) ([]byte, error) { return os.ReadFile(path) }

func (osSource) isFile(path string) bool {
	info, err := os.Stat(path)
	return err == nil && !info.IsDir()
}

func (osSource) envFileNames(dir string) ([]string, error) {
	entries, err := os.ReadDir(dir)
	if err != nil {
		return nil, err
	}
	names := make([]string, 0, len(entries))
	for _, e := range entries {
		if e.IsDir() || !strings.HasSuffix(e.Name(), ".env") {
			continue
		}
		names = append(names, e.Name())
	}
	return names, nil
}

// overlaySource serves recorded bytes first and disk second. envNames is the
// *.env listing the snapshot saw, so a candidate is evaluated against exactly
// the variable files the snapshot loaded.
type overlaySource struct {
	files    map[string][]byte
	envNames []string
	fallback fileSource
}

func (o *overlaySource) readFile(path string) ([]byte, error) {
	if data, ok := o.files[path]; ok {
		return data, nil
	}
	return o.fallback.readFile(path)
}

func (o *overlaySource) isFile(path string) bool {
	if _, ok := o.files[path]; ok {
		return true
	}
	return o.fallback.isFile(path)
}

func (o *overlaySource) envFileNames(string) ([]string, error) {
	return append([]string(nil), o.envNames...), nil
}

// recorder captures what one load touched (SPEC §10.2): each document's bytes,
// format, and raw parsed value, plus every graft of its root into the
// effective tree.
type recorder struct {
	docs  map[string]*docRecord // absolute cleaned path -> record
	order []string
}

type docRecord struct {
	path   string
	data   []byte
	format string
	parsed any
	grafts []Graft
}

func newRecorder() *recorder {
	return &recorder{docs: map[string]*docRecord{}}
}

func (r *recorder) record(path string, data []byte, format string, parsed any) {
	if _, seen := r.docs[path]; seen {
		return // a shared include is parsed once per reference; recorded once
	}
	r.docs[path] = &docRecord{path: path, data: data, format: format, parsed: parsed}
	r.order = append(r.order, path)
}

func (r *recorder) graft(path string, g Graft) {
	// The entrypoint is grafted before it is recorded; make the slot.
	if _, ok := r.docs[path]; !ok {
		r.docs[path] = &docRecord{path: path}
		r.order = append(r.order, path)
	}
	r.docs[path].grafts = append(r.docs[path].grafts, g)
}

// documentKey is SPEC §10.2's key: the path relative to the config directory,
// slash-separated, or the absolute path when no relative form exists.
func documentKey(dir, path string) string {
	rel, err := filepath.Rel(dir, path)
	if err != nil {
		return filepath.ToSlash(path)
	}
	return filepath.ToSlash(rel)
}

func sortedGrafts(grafts []Graft) []Graft {
	out := append([]Graft(nil), grafts...)
	sort.Slice(out, func(i, j int) bool { return out[i].Effective < out[j].Effective })
	if out == nil {
		out = []Graft{}
	}
	return out
}
