package entryconf

import (
	"errors"
	"os"
	"path/filepath"
	"testing"
)

// watchCasesDir is the shared watching suite (SPEC §13).
const watchCasesDir = "../testdata/watchcases"

// TestWatchConformance walks every case in ../testdata/watchcases: open a copy,
// take its watch set, apply change.json, refresh, and compare.
func TestWatchConformance(t *testing.T) {
	entries, err := os.ReadDir(watchCasesDir)
	if err != nil {
		t.Fatalf("cannot read watching suite %s: %v", watchCasesDir, err)
	}
	n := 0
	for _, e := range entries {
		if !e.IsDir() {
			continue
		}
		n++
		dir := filepath.Join(watchCasesDir, e.Name())
		t.Run(e.Name(), func(t *testing.T) { runWatchCase(t, dir) })
	}
	if n == 0 {
		t.Fatalf("no cases found in %s", watchCasesDir)
	}
}

func runWatchCase(t *testing.T, caseDir string) {
	t.Helper()
	work := t.TempDir()
	config := filepath.Join(work, "config")
	if err := os.MkdirAll(config, 0o755); err != nil {
		t.Fatal(err)
	}
	copyTree(t, filepath.Join(caseDir, "config"), config)
	if _, err := os.Stat(filepath.Join(caseDir, "outside")); err == nil {
		copyTree(t, filepath.Join(caseDir, "outside"), work)
	}

	env := map[string]string{}
	if v, ok := readJSONFile(t, filepath.Join(caseDir, "procenv.json")); ok {
		for k, val := range v.(map[string]any) {
			env[k] = val.(string)
		}
	}
	snapshotEnv := func() map[string]string {
		out := map[string]string{}
		for k, v := range env {
			out[k] = v
		}
		return out
	}
	live := func(name string) (string, bool) { v, ok := env[name]; return v, ok }

	snap, err := open(config, snapshotEnv(), live)
	if err != nil {
		t.Fatalf("open: %v", err)
	}
	watch := toJSONShape(t, snap.WatchSet())

	changeV, ok := readJSONFile(t, filepath.Join(caseDir, "change.json"))
	if !ok {
		t.Fatalf("case has no change.json")
	}
	change := changeV.(map[string]any)
	if files, ok := change["files"].(map[string]any); ok {
		for key, text := range files {
			path := filepath.Join(config, filepath.FromSlash(key))
			if text == nil {
				if err := os.Remove(path); err != nil {
					t.Fatalf("change: %v", err)
				}
				continue
			}
			if err := os.MkdirAll(filepath.Dir(path), 0o755); err != nil {
				t.Fatal(err)
			}
			if err := os.WriteFile(path, []byte(text.(string)), 0o644); err != nil {
				t.Fatalf("change: %v", err)
			}
		}
	}
	if pe, ok := change["procenv"].(map[string]any); ok {
		for k, v := range pe {
			if v == nil {
				delete(env, k)
			} else {
				env[k] = v.(string)
			}
		}
	}

	r := refresh(snap, snapshotEnv(), live)
	got := map[string]any{
		"watch":     watch,
		"stale":     r.Stale,
		"changed":   toJSONShape(t, r.Changed),
		"documents": toJSONShape(t, r.Documents),
	}
	if r.Err != nil {
		var ecErr *Error
		if !errors.As(r.Err, &ecErr) {
			t.Fatalf("refresh error is %T, not *Error: %v", r.Err, r.Err)
		}
		got["error"] = ecErr.Code()
		if r.Snapshot != nil {
			t.Fatalf("a failed refresh carries a snapshot")
		}
	} else {
		got["tree"] = map[string]any(r.Snapshot.Tree)
		if !r.Stale && r.Snapshot != snap {
			t.Fatalf("a refresh that is not stale must return the old snapshot")
		}
	}
	expected, _ := readJSONFile(t, filepath.Join(caseDir, "expected.json"))
	if d := valueDiff(expected, got, "$"); d != "" {
		t.Fatalf("result mismatch: %s", d)
	}
}
