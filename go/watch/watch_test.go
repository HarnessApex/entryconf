package watch

import (
	"errors"
	"os"
	"path/filepath"
	"sync/atomic"
	"testing"
	"time"

	entryconf "github.com/HarnessApex/entryconf/go"
	"github.com/fsnotify/fsnotify"
)

const settle = 50 * time.Millisecond

// quiet is how long a test waits to be sure nothing more is delivered.
const quiet = 400 * time.Millisecond

func write(t *testing.T, path, text string) {
	t.Helper()
	if err := os.MkdirAll(filepath.Dir(path), 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(path, []byte(text), 0o644); err != nil {
		t.Fatal(err)
	}
}

// start watches a fresh directory holding files, sending each change on the
// returned channel and answering with what answer returns (nil accepts).
func start(t *testing.T, files map[string]string, answer func(Change) error) (string, *Watcher, chan Change) {
	t.Helper()
	dir := t.TempDir()
	for name, text := range files {
		write(t, filepath.Join(dir, name), text)
	}
	changes := make(chan Change, 16)
	w, err := Watch(dir, func(c Change) error {
		changes <- c
		if answer != nil {
			return answer(c)
		}
		return nil
	}, &Options{Settle: settle})
	if err != nil {
		t.Fatalf("Watch: %v", err)
	}
	t.Cleanup(func() { w.Close() })
	return dir, w, changes
}

func next(t *testing.T, changes chan Change) Change {
	t.Helper()
	select {
	case c := <-changes:
		return c
	case <-time.After(5 * time.Second):
		t.Fatal("no change delivered")
		return Change{}
	}
}

func none(t *testing.T, changes chan Change) {
	t.Helper()
	select {
	case c := <-changes:
		t.Fatalf("unexpected change: changed %v, documents %+v, err %v", c.Changed, c.Documents, c.Err)
	case <-time.After(quiet):
	}
}

func code(err error) string {
	var ecErr *entryconf.Error
	if errors.As(err, &ecErr) {
		return ecErr.Code()
	}
	return ""
}

func TestBurstIsOneChange(t *testing.T) {
	dir, w, changes := start(t, map[string]string{"entrypoint.json": `{"n": 0}`}, nil)
	for i := 1; i <= 5; i++ {
		write(t, filepath.Join(dir, "entrypoint.json"), `{"n": `+string(rune('0'+i))+`}`)
		time.Sleep(settle / 5)
	}
	c := next(t, changes)
	if c.Err != nil || c.New.Tree["n"] != int64(5) {
		t.Fatalf("change: tree %v, err %v", c.New.Tree, c.Err)
	}
	if len(c.Changed) != 1 || c.Changed[0] != "/n" {
		t.Fatalf("changed %v", c.Changed)
	}
	none(t, changes)
	if w.Current() != c.New {
		t.Fatal("an accepted change is not current")
	}
}

func TestRenameOverIsOneChange(t *testing.T) {
	dir, _, changes := start(t, map[string]string{"entrypoint.json": `{"db": "@file:db.json"}`, "db.json": `{"port": 1}`}, nil)
	tmp := filepath.Join(dir, ".db.json.swp")
	write(t, tmp, `{"port": 2}`)
	if err := os.Rename(tmp, filepath.Join(dir, "db.json")); err != nil {
		t.Fatal(err)
	}
	c := next(t, changes)
	if len(c.Changed) != 1 || c.Changed[0] != "/db/port" {
		t.Fatalf("changed %v", c.Changed)
	}
	none(t, changes)
	// The watch survived the rename: a second edit is seen too.
	write(t, filepath.Join(dir, "db.json"), `{"port": 3}`)
	if c := next(t, changes); c.New.Tree["db"].(map[string]any)["port"] != int64(3) {
		t.Fatalf("second edit: %v", c.New.Tree)
	}
}

func TestNoEmptyCalls(t *testing.T) {
	dir, _, changes := start(t, map[string]string{"entrypoint.json": `{"n": 1}`}, nil)
	write(t, filepath.Join(dir, "entrypoint.json"), `{"n": 1}`)
	write(t, filepath.Join(dir, "notes.txt"), `not config`)
	none(t, changes)
}

func TestRefusedKeepsOld(t *testing.T) {
	refuse := errors.New("port out of range")
	dir, w, changes := start(t, map[string]string{"entrypoint.json": `{"port": 80}`}, func(c Change) error {
		if c.New != nil && c.New.Tree["port"] == int64(0) {
			return refuse
		}
		return nil
	})
	first := w.Current()
	write(t, filepath.Join(dir, "entrypoint.json"), `{"port": 0}`)
	if c := next(t, changes); c.Old != first {
		t.Fatal("first change not from the opened snapshot")
	}
	if w.Current() != first {
		t.Fatal("a refused change became current")
	}
	write(t, filepath.Join(dir, "entrypoint.json"), `{"port": 81}`)
	c := next(t, changes)
	if c.Old != first {
		t.Fatal("the change after a refusal was not refreshed from the old snapshot")
	}
	if len(c.Changed) != 1 || c.Changed[0] != "/port" || w.Current() != c.New {
		t.Fatalf("changed %v", c.Changed)
	}
}

func TestRefusedNewIncludeIsWatched(t *testing.T) {
	dir, w, changes := start(t, map[string]string{"entrypoint.json": `{"port": 80}`}, func(c Change) error {
		if c.New != nil && c.New.Tree["db"] != nil && c.New.Tree["db"].(map[string]any)["port"] == int64(0) {
			return errors.New("refused")
		}
		return nil
	})
	write(t, filepath.Join(dir, "sub", "db.json"), `{"port": 0}`)
	write(t, filepath.Join(dir, "entrypoint.json"), `{"port": 80, "db": "@file:sub/db.json"}`)
	next(t, changes) // refused
	write(t, filepath.Join(dir, "sub", "db.json"), `{"port": 5432}`)
	c := next(t, changes)
	if c.Err != nil || w.Current() != c.New || c.New.Tree["db"].(map[string]any)["port"] != int64(5432) {
		t.Fatalf("fixing the refused include: %v %v", c.Err, c.New)
	}
}

func TestOneAtATime(t *testing.T) {
	var running, overlapped atomic.Bool
	release := make(chan struct{})
	dir, _, changes := start(t, map[string]string{"entrypoint.json": `{"n": 0}`}, func(c Change) error {
		if running.Swap(true) {
			overlapped.Store(true)
		}
		defer running.Store(false)
		if c.New.Tree["n"] == int64(1) {
			<-release
		}
		return nil
	})
	write(t, filepath.Join(dir, "entrypoint.json"), `{"n": 1}`)
	first := next(t, changes)
	// While the first callback runs, two more edits land.
	write(t, filepath.Join(dir, "entrypoint.json"), `{"n": 2}`)
	time.Sleep(quiet)
	write(t, filepath.Join(dir, "entrypoint.json"), `{"n": 3}`)
	time.Sleep(quiet)
	none(t, changes)
	close(release)
	second := next(t, changes)
	if second.Old != first.New || second.New.Tree["n"] != int64(3) {
		t.Fatalf("gathered change: old %v new %v", second.Old.Tree, second.New.Tree)
	}
	none(t, changes)
	if overlapped.Load() {
		t.Fatal("callbacks overlapped")
	}
}

func TestFailedOpenThenCreatedInclude(t *testing.T) {
	dir, w, changes := start(t, map[string]string{"entrypoint.json": `{"n": 1}`}, nil)
	first := w.Current()
	write(t, filepath.Join(dir, "entrypoint.json"), `{"n": 1, "db": "@file:conf/db.json"}`)
	c := next(t, changes)
	if code(c.Err) != entryconf.CodeInclude || c.New != nil {
		t.Fatalf("failed open: %v", c.Err)
	}
	if w.Current() != first {
		t.Fatal("a failed open changed the current snapshot")
	}
	// Creating the missing include, in a directory that did not exist, is seen.
	write(t, filepath.Join(dir, "conf", "db.json"), `{"port": 5432}`)
	c = next(t, changes)
	if c.Err != nil || c.Old != first || w.Current() != c.New {
		t.Fatalf("after creating the include: %v", c.Err)
	}
	if got := c.New.Tree["db"].(map[string]any)["port"]; got != int64(5432) {
		t.Fatalf("db.port %v", got)
	}
}

func TestSecondEntrypointAndEnvFile(t *testing.T) {
	dir, _, changes := start(t, map[string]string{"entrypoint.json": `{"host": "${EC_WT_HOST:-a}"}`}, nil)
	write(t, filepath.Join(dir, "host.env"), "EC_WT_HOST=b\n")
	c := next(t, changes)
	if c.New.Tree["host"] != "b" || len(c.Documents.Added) != 1 || c.Documents.Added[0] != "host.env" {
		t.Fatalf("env file: %v %+v", c.New.Tree, c.Documents)
	}
	write(t, filepath.Join(dir, "entrypoint.yaml"), "host: c\n")
	if c := next(t, changes); code(c.Err) != entryconf.CodeMultipleEntrypoints {
		t.Fatalf("second entrypoint: %v", c.Err)
	}
}

func TestCloseWaitsForCallback(t *testing.T) {
	entered := make(chan struct{})
	var finished, late atomic.Bool
	var closed atomic.Bool
	dir, w, _ := start(t, map[string]string{"entrypoint.json": `{"n": 0}`}, func(c Change) error {
		if closed.Load() {
			late.Store(true)
		}
		if c.New.Tree["n"] == int64(1) {
			close(entered)
			time.Sleep(quiet)
			finished.Store(true)
		}
		return nil
	})
	write(t, filepath.Join(dir, "entrypoint.json"), `{"n": 1}`)
	<-entered
	write(t, filepath.Join(dir, "entrypoint.json"), `{"n": 2}`)
	if err := w.Close(); err != nil {
		t.Fatal(err)
	}
	closed.Store(true)
	if !finished.Load() {
		t.Fatal("Close returned while a callback ran")
	}
	write(t, filepath.Join(dir, "entrypoint.json"), `{"n": 3}`)
	time.Sleep(quiet)
	if late.Load() {
		t.Fatal("a callback started after Close")
	}
	if err := w.Close(); err != nil {
		t.Fatal("a second Close:", err)
	}
}

func TestWatchRefused(t *testing.T) {
	saved := addWatch
	t.Cleanup(func() { addWatch = saved })
	refusedDir := ""
	addWatch = func(fw *fsnotify.Watcher, dir string) error {
		if dir == refusedDir {
			return errors.New("no space left on device (inotify watches)")
		}
		return saved(fw, dir)
	}

	// At the start: Watch itself fails.
	dir := t.TempDir()
	write(t, filepath.Join(dir, "entrypoint.json"), `{"n": 1}`)
	refusedDir = dir
	if _, err := Watch(dir, func(Change) error { return nil }, nil); code(err) != entryconf.CodeWatch {
		t.Fatalf("Watch on a refused directory: %v", err)
	}

	// Later: a change the snapshot takes needs a directory the platform
	// refuses; that is delivered as E_WATCH and the snapshot is kept.
	refusedDir = ""
	dir, w, changes := start(t, map[string]string{"entrypoint.json": `{"n": 1}`, "sub/x.json": `{}`}, nil)
	refusedDir = filepath.Join(dir, "sub")
	write(t, filepath.Join(dir, "entrypoint.json"), `{"n": 1, "x": "@file:sub/x.json"}`)
	accepted := next(t, changes)
	if accepted.Err != nil {
		t.Fatal(accepted.Err)
	}
	c := next(t, changes)
	if code(c.Err) != entryconf.CodeWatch || c.Old != accepted.New || w.Current() != accepted.New {
		t.Fatalf("refused watch: %v", c.Err)
	}
}
