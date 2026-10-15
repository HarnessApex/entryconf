// Package watch is entryconf's Watch (SPEC §12.3): it holds a current
// snapshot of a config directory and hands each change on disk to a callback
// that accepts or refuses it.
//
// It lives apart from the entryconf package because it depends on
// github.com/fsnotify/fsnotify; a program that only loads or edits never
// imports it. Changes are reported by the platform (inotify, kqueue,
// ReadDirectoryChangesW, FEN); nothing is polled. fsnotify does not see
// changes made on another machine to a network filesystem (NFS, SMB), and so
// neither does a watcher.
package watch

import (
	"errors"
	"os"
	"path/filepath"
	"strings"
	"sync"
	"time"

	entryconf "github.com/HarnessApex/entryconf/go"
	"github.com/fsnotify/fsnotify"
)

// DefaultSettle is how long a watcher waits after the last event of a burst
// before it refreshes (SPEC §12.3).
const DefaultSettle = 100 * time.Millisecond

// Change is what a callback is handed (SPEC §12.3).
type Change struct {
	// Old is the current snapshot.
	Old *entryconf.Snapshot
	// New is the refreshed snapshot; nil when Err is set.
	New *entryconf.Snapshot
	// Changed are the affected pointers between Old and New (SPEC §10.6).
	Changed []string
	// Documents are the document keys added, removed and changed.
	Documents entryconf.DocumentChanges
	// Err is the open's failure (its SPEC §7 code), or E_WATCH when the
	// platform refused to watch a path. Old stays current whatever the
	// callback returns.
	Err error
}

// Callback takes a change. Returning nil accepts it: New becomes the current
// snapshot. Returning an error refuses it: Old stays current and the next
// change is refreshed from Old. Callbacks never overlap. A callback must not
// call Close on its own watcher: Close waits for the callback.
type Callback func(Change) error

// Options tunes a watcher. A nil *Options means the defaults.
type Options struct {
	// Settle is how long after the last event of a burst a refresh runs;
	// zero means DefaultSettle.
	Settle time.Duration
}

// Watcher watches one config directory. Make one with Watch.
type Watcher struct {
	callback Callback
	settle   time.Duration
	fw       *fsnotify.Watcher

	mu      sync.Mutex
	current *entryconf.Snapshot
	// dirs maps each watched directory to the entry names in it that matter;
	// the config directory also takes any entrypoint and *.env name.
	dirs   map[string]map[string]bool
	config string

	done      chan struct{}
	exited    chan struct{}
	closeOnce sync.Once
}

// addWatch is the seam the tests use to make the platform refuse a path.
var addWatch = func(fw *fsnotify.Watcher, dir string) error { return fw.Add(dir) }

// Watch opens dir (entryconf.Open) and watches it. A failed open is returned
// with its code; a watch the platform refuses is E_WATCH. callback is called
// with each stale refresh until Close.
func Watch(dir string, callback Callback, opts *Options) (*Watcher, error) {
	snap, err := entryconf.Open(dir)
	if err != nil {
		return nil, err
	}
	fw, err := fsnotify.NewWatcher()
	if err != nil {
		return nil, entryconf.NewError(entryconf.CodeWatch, err, "cannot start watching %q", dir)
	}
	w := &Watcher{
		callback: callback,
		settle:   DefaultSettle,
		fw:       fw,
		current:  snap,
		dirs:     map[string]map[string]bool{},
		config:   snap.Dir,
		done:     make(chan struct{}),
		exited:   make(chan struct{}),
	}
	if opts != nil && opts.Settle > 0 {
		w.settle = opts.Settle
	}
	if _, err := w.arm(targets(snap.Dir, documentPaths(snap))); err != nil {
		fw.Close()
		return nil, err
	}
	go w.loop()
	return w, nil
}

// Current is the current snapshot: the one Watch opened, or the last one a
// callback accepted.
func (w *Watcher) Current() *entryconf.Snapshot {
	w.mu.Lock()
	defer w.mu.Unlock()
	return w.current
}

// Close stops the watch. A callback running is waited for; none starts
// after Close returns.
func (w *Watcher) Close() error {
	var err error
	w.closeOnce.Do(func() {
		close(w.done)
		<-w.exited
		err = w.fw.Close()
	})
	return err
}

func (w *Watcher) loop() {
	defer close(w.exited)
	timer := time.NewTimer(time.Hour)
	timer.Stop()
	// A change made between Open and the watches being placed is caught by
	// one refresh at the start; it calls nothing if nothing changed.
	timer.Reset(w.settle)
	for {
		select {
		case <-w.done:
			timer.Stop()
			return
		case ev, ok := <-w.fw.Events:
			if !ok {
				return
			}
			if w.relevant(ev.Name) {
				timer.Reset(w.settle)
			}
		case err, ok := <-w.fw.Errors:
			if !ok {
				return
			}
			if errors.Is(err, fsnotify.ErrEventOverflow) {
				// Events were lost: refresh, which finds whatever they were.
				timer.Reset(w.settle)
				continue
			}
			w.deliver(Change{Old: w.Current(), Documents: emptyDocuments(),
				Changed: []string{}, Err: entryconf.NewError(entryconf.CodeWatch, err, "watching %q", w.config)})
		case <-timer.C:
			if w.refresh() {
				timer.Reset(w.settle)
			}
		}
	}
}

// refresh refreshes the current snapshot and, when stale, delivers the
// change. It reports whether a refresh is due again (a directory newly
// watched, where a change may have landed before its watch).
func (w *Watcher) refresh() bool {
	old := w.Current()
	r := entryconf.Refresh(old)
	if !r.Stale {
		return false
	}
	c := Change{Old: old, Changed: r.Changed, Documents: r.Documents, Err: r.Err}
	if r.Err == nil {
		c.New = r.Snapshot
	}
	accepted := w.deliver(c)
	var paths []string
	switch {
	case r.Err != nil:
		paths = append(documentPaths(old), r.Tried...)
	case accepted:
		w.mu.Lock()
		w.current = r.Snapshot
		w.mu.Unlock()
		paths = documentPaths(r.Snapshot)
	default:
		paths = append(documentPaths(old), documentPaths(r.Snapshot)...)
	}
	added, err := w.arm(targets(w.config, paths))
	if err != nil {
		w.deliver(Change{Old: w.Current(), Documents: emptyDocuments(), Changed: []string{}, Err: err})
	}
	return added
}

// deliver calls the callback unless the watcher is closing, and reports
// whether the change was accepted.
func (w *Watcher) deliver(c Change) bool {
	select {
	case <-w.done:
		return false
	default:
	}
	err := w.callback(c)
	return err == nil && c.Err == nil
}

// relevant reports whether an event on path can change a refresh.
func (w *Watcher) relevant(path string) bool {
	dir, name := filepath.Split(filepath.Clean(path))
	dir = filepath.Clean(dir)
	w.mu.Lock()
	defer w.mu.Unlock()
	if names, ok := w.dirs[dir]; ok && names[name] {
		return true
	}
	if dir == w.config && isEntrypointOrEnv(name) {
		return true
	}
	return false
}

func isEntrypointOrEnv(name string) bool {
	switch name {
	case "entrypoint.json", "entrypoint.yaml", "entrypoint.yml", "entrypoint.toml":
		return true
	}
	return strings.HasSuffix(name, ".env")
}

// documentPaths is the snapshot's watch set (SPEC §12.1) as the paths the
// load resolved; the directory itself is always watched.
func documentPaths(s *entryconf.Snapshot) []string {
	keys := s.WatchSet().Documents
	out := make([]string, len(keys))
	for i, key := range keys {
		out[i] = s.Documents[key].Path
	}
	return out
}

// targets maps the config directory and the paths to watch to directories
// and the entry names in each. A path's own directory is watched (an
// editor's rename replaces the file, so a watch on the file itself would be
// lost); a symbolic link is also watched where it points. A path that does
// not exist (what a failed open could not read) is watched at its nearest
// existing ancestor, by the name of the next step down, so its creation is
// seen.
func targets(config string, paths []string) map[string]map[string]bool {
	out := map[string]map[string]bool{config: {}}
	add := func(dir, name string) {
		if out[dir] == nil {
			out[dir] = map[string]bool{}
		}
		out[dir][name] = true
	}
	for _, p := range paths {
		p = filepath.Clean(p)
		if p == config {
			continue
		}
		for q := p; ; {
			dir, name := filepath.Dir(q), filepath.Base(q)
			if info, err := os.Stat(dir); err == nil && info.IsDir() {
				add(dir, name)
				break
			}
			if dir == q {
				break
			}
			q = dir
		}
		if real, err := filepath.EvalSymlinks(p); err == nil && real != p {
			add(filepath.Dir(real), filepath.Base(real))
		}
	}
	return out
}

// arm makes the watched directories exactly want: new ones are added, ones
// no longer wanted are removed. It reports whether any was added, and
// E_WATCH if the platform refused one (the others stay watched).
func (w *Watcher) arm(want map[string]map[string]bool) (bool, error) {
	w.mu.Lock()
	defer w.mu.Unlock()
	var failed error
	added := false
	for dir := range want {
		if _, ok := w.dirs[dir]; ok {
			continue
		}
		if err := addWatch(w.fw, dir); err != nil {
			if failed == nil {
				failed = entryconf.NewError(entryconf.CodeWatch, err, "cannot watch %q", dir)
			}
			delete(want, dir)
			continue
		}
		added = true
	}
	for dir := range w.dirs {
		if _, ok := want[dir]; !ok {
			_ = w.fw.Remove(dir) // a directory gone is no longer watched anyway
		}
	}
	w.dirs = want
	return added, failed
}

func emptyDocuments() entryconf.DocumentChanges {
	return entryconf.DocumentChanges{Added: []string{}, Removed: []string{}, Changed: []string{}}
}
