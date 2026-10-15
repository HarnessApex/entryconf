package entryconf

import (
	"os"
	"sort"
)

// WatchSet is what can change the result of opening a snapshot's directory
// again (SPEC §12.1): the directory itself, for entrypoints and *.env files
// appearing, going or renamed, and every document by its key.
type WatchSet struct {
	// Directory is always ".": the config directory.
	Directory string `json:"directory"`
	// Documents are the snapshot's document keys, sorted by code point.
	Documents []string `json:"documents"`
}

// WatchSet is the snapshot's watch set. It reads nothing from disk.
func (s *Snapshot) WatchSet() WatchSet {
	keys := make([]string, 0, len(s.Documents))
	for key := range s.Documents {
		keys = append(keys, key)
	}
	sort.Strings(keys)
	return WatchSet{Directory: ".", Documents: keys}
}

// DocumentChanges lists document keys between two snapshots, each sorted by
// code point (SPEC §12.2).
type DocumentChanges struct {
	Added   []string `json:"added"`
	Removed []string `json:"removed"`
	Changed []string `json:"changed"`
}

// Refreshed is what Refresh reports (SPEC §12.2).
type Refreshed struct {
	// Stale is true iff the open failed, read other documents, read a
	// document whose revision differs, or looked up a variable whose value
	// differs.
	Stale bool `json:"stale"`
	// Snapshot is the new snapshot (the old one when not stale); nil when the
	// open failed.
	Snapshot *Snapshot `json:"-"`
	// Err is the open's failure, an *Error; the old snapshot stays valid.
	Err error `json:"-"`
	// Changed are the affected pointers between the old and the new tree.
	Changed []string `json:"changed"`
	// Documents are the keys added, removed and changed.
	Documents DocumentChanges `json:"documents"`
	// Tried, after a failed open, is every absolute path it asked about,
	// including those it could not read: a watcher watches them so that
	// creating a missing include is seen.
	Tried []string `json:"-"`
}

// Refresh opens the snapshot's directory again, against the process
// environment as it is now, and reports whether and what changed (SPEC
// §12.2). It never writes a file.
func Refresh(s *Snapshot) *Refreshed {
	return refresh(s, captureEnv(), os.LookupEnv)
}

func refresh(s *Snapshot, captured map[string]string, live envSource) *Refreshed {
	r := &Refreshed{
		Changed:   []string{},
		Documents: DocumentChanges{Added: []string{}, Removed: []string{}, Changed: []string{}},
	}
	next, tried, err := openTracked(s.Dir, captured, live)
	if err != nil {
		r.Stale, r.Err, r.Tried = true, err, tried
		return r
	}
	for key, d := range next.Documents {
		old, ok := s.Documents[key]
		switch {
		case !ok:
			r.Documents.Added = append(r.Documents.Added, key)
		case old.Revision != d.Revision:
			r.Documents.Changed = append(r.Documents.Changed, key)
		}
	}
	for key := range s.Documents {
		if _, ok := next.Documents[key]; !ok {
			r.Documents.Removed = append(r.Documents.Removed, key)
		}
	}
	sort.Strings(r.Documents.Added)
	sort.Strings(r.Documents.Removed)
	sort.Strings(r.Documents.Changed)
	r.Stale = len(r.Documents.Added)+len(r.Documents.Removed)+len(r.Documents.Changed) > 0 ||
		next.varsRevision != s.varsRevision
	if !r.Stale {
		r.Snapshot = s
		return r
	}
	r.Snapshot = next
	r.Changed = affectedPointers(s.Tree, next.Tree)
	return r
}
