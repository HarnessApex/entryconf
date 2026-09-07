package entryconf

import (
	"bytes"
	"encoding/json"
	"errors"
	"os"
	"path/filepath"
	"runtime"
	"sort"
	"strings"
	"testing"
	"time"
)

// editCasesDir is the shared editing suite (SPEC §11).
const editCasesDir = "../testdata/editcases"

// TestEditConformance walks every case in ../testdata/editcases. Each case runs
// against a fresh copy of its config/ directory; afterwards every file the
// case did not expect to be written must be byte-identical to the copy and no
// file may have been added.
func TestEditConformance(t *testing.T) {
	entries, err := os.ReadDir(editCasesDir)
	if err != nil {
		t.Fatalf("cannot read editing suite %s: %v", editCasesDir, err)
	}
	n := 0
	for _, e := range entries {
		if !e.IsDir() {
			continue
		}
		n++
		dir := filepath.Join(editCasesDir, e.Name())
		t.Run(e.Name(), func(t *testing.T) { runEditCase(t, dir) })
	}
	if n == 0 {
		t.Fatalf("no cases found in %s", editCasesDir)
	}
}

func readJSONFile(t *testing.T, path string) (any, bool) {
	t.Helper()
	data, err := os.ReadFile(path)
	if errors.Is(err, os.ErrNotExist) {
		return nil, false
	}
	if err != nil {
		t.Fatalf("cannot read %s: %v", path, err)
	}
	v, err := parseJSON(path, data)
	if err != nil {
		t.Fatalf("bad JSON in %s: %v", path, err)
	}
	return v, true
}

// copyTree copies src into dst (which must exist), preserving file modes.
func copyTree(t *testing.T, src, dst string) {
	t.Helper()
	err := filepath.Walk(src, func(path string, info os.FileInfo, err error) error {
		if err != nil {
			return err
		}
		rel, _ := filepath.Rel(src, path)
		target := filepath.Join(dst, rel)
		if info.IsDir() {
			return os.MkdirAll(target, 0o755)
		}
		data, err := os.ReadFile(path)
		if err != nil {
			return err
		}
		return os.WriteFile(target, data, info.Mode().Perm())
	})
	if err != nil {
		t.Fatalf("copying fixture: %v", err)
	}
}

// fileBytes maps every file under root (relative, slash-separated) to its bytes.
func fileBytes(t *testing.T, root string) map[string][]byte {
	t.Helper()
	out := map[string][]byte{}
	err := filepath.Walk(root, func(path string, info os.FileInfo, err error) error {
		if err != nil || info.IsDir() {
			return err
		}
		rel, _ := filepath.Rel(root, path)
		data, err := os.ReadFile(path)
		if err != nil {
			return err
		}
		out[filepath.ToSlash(rel)] = data
		return nil
	})
	if err != nil {
		t.Fatalf("walking %s: %v", root, err)
	}
	return out
}

// toJSONShape round-trips a Go value through encoding/json into the loader's
// shapes, so structs with json tags can be compared against expected.json.
func toJSONShape(t *testing.T, v any) any {
	t.Helper()
	data, err := json.Marshal(v)
	if err != nil {
		t.Fatalf("marshal: %v", err)
	}
	out, err := parseJSON("result", data)
	if err != nil {
		t.Fatalf("re-parse: %v", err)
	}
	return out
}

func runEditCase(t *testing.T, caseDir string) {
	t.Helper()
	work := t.TempDir()
	copyTree(t, filepath.Join(caseDir, "config"), work)
	original := fileBytes(t, work)

	env := map[string]string{}
	if v, ok := readJSONFile(t, filepath.Join(caseDir, "procenv.json")); ok {
		for k, val := range v.(map[string]any) {
			env[k] = val.(string)
		}
	}
	captured := map[string]string{}
	for k, v := range env {
		captured[k] = v
	}
	live := func(name string) (string, bool) { v, ok := env[name]; return v, ok }

	reqV, ok := readJSONFile(t, filepath.Join(caseDir, "request.json"))
	if !ok {
		t.Fatalf("case has no request.json")
	}
	req := reqV.(map[string]any)
	wantErrData, _ := os.ReadFile(filepath.Join(caseDir, "expected_error.txt"))
	wantErr := strings.TrimSpace(string(wantErrData))
	expected, _ := readJSONFile(t, filepath.Join(caseDir, "expected.json"))

	// Files the harness itself overwrites between plan and commit are expected
	// to hold that text afterwards; everything else must be untouched.
	expectWritten := map[string]bool{}

	var got any
	var opErr error
	snap, err := open(work, captured, live)
	if err != nil {
		opErr = err
	} else if ptr, isInspect := req["inspect"]; isInspect {
		origin, err := snap.Inspect(ptr.(string))
		opErr = err
		if err == nil {
			got = map[string]any{"origin": toJSONShape(t, origin)}
		}
	} else if _, isDocs := req["documents"]; isDocs {
		docs := map[string]any{}
		for key, d := range snap.Documents {
			docs[key] = map[string]any{"format": d.Format, "writable": d.Writable, "grafts": toJSONShape(t, d.Grafts)}
		}
		got = map[string]any{"documents": docs}
	} else {
		var edits []Edit
		for _, raw := range req["edits"].([]any) {
			e := raw.(map[string]any)
			edit := Edit{Op: str(e["op"]), Pointer: str(e["pointer"]), Document: str(e["document"]), Mode: str(e["mode"])}
			edit.Value = e["value"]
			edits = append(edits, edit)
		}
		plan, err := snap.Plan(edits)
		opErr = err
		if err == nil {
			if bc, ok := req["before_commit"].(map[string]any); ok {
				if files, ok := bc["files"].(map[string]any); ok {
					for key, text := range files {
						if err := os.WriteFile(filepath.Join(work, filepath.FromSlash(key)), []byte(text.(string)), 0o644); err != nil {
							t.Fatalf("before_commit: %v", err)
						}
						original[key] = []byte(text.(string))
					}
				}
				if pe, ok := bc["procenv"].(map[string]any); ok {
					for k, v := range pe {
						env[k] = v.(string)
					}
				}
			}
			receipt, err := plan.Commit(nil)
			opErr = err
			if err == nil {
				expectWritten[plan.Document] = true
				if receipt.Document != plan.Document {
					t.Fatalf("receipt names %q, plan %q", receipt.Document, plan.Document)
				}
				reloaded, err := load(work, live)
				if err != nil {
					t.Fatalf("reload after commit: %v", err)
				}
				if d := valueDiff(map[string]any(plan.Candidate), map[string]any(reloaded), "$"); d != "" {
					t.Fatalf("committed tree differs from the plan's candidate: %s", d)
				}
				written, err := parseJSON(plan.Document, []byte(plan.After))
				if err != nil {
					t.Fatalf("plan.After is not JSON: %v", err)
				}
				onDisk, _ := os.ReadFile(filepath.Join(work, filepath.FromSlash(plan.Document)))
				if !bytes.Equal(onDisk, []byte(plan.After)) {
					t.Fatalf("file on disk is not plan.After")
				}
				if receipt.Revision != revisionOf(onDisk) {
					t.Fatalf("receipt revision does not match the written file")
				}
				got = map[string]any{
					"tree":      reloaded,
					"affected":  toJSONShape(t, plan.Affected),
					"documents": map[string]any{plan.Document: written},
				}
			}
		}
	}

	// Verdict.
	if wantErr != "" {
		if opErr == nil {
			t.Fatalf("expected %s, got success: %v", wantErr, got)
		}
		var ecErr *Error
		if !errors.As(opErr, &ecErr) {
			t.Fatalf("expected *Error %s, got %T: %v", wantErr, opErr, opErr)
		}
		if ecErr.Code() != wantErr {
			t.Fatalf("expected %s, got %s (%v)", wantErr, ecErr.Code(), opErr)
		}
	} else {
		if opErr != nil {
			t.Fatalf("unexpected error: %v", opErr)
		}
		if d := valueDiff(expected, got, "$"); d != "" {
			t.Fatalf("result mismatch: %s", d)
		}
	}

	// Filesystem discipline (SPEC §11).
	after := fileBytes(t, work)
	for rel, data := range after {
		orig, existed := original[rel]
		if !existed {
			t.Errorf("file %s was created (lock or temporary file left behind?)", rel)
			continue
		}
		if !expectWritten[rel] && !bytes.Equal(orig, data) {
			t.Errorf("file %s changed but was not the edited document", rel)
		}
	}
	for rel := range original {
		if _, still := after[rel]; !still {
			t.Errorf("file %s disappeared", rel)
		}
	}
}

func str(v any) string {
	s, _ := v.(string)
	return s
}

// --- what fixtures cannot express (SPEC §10.9) -----------------------------

func scratchConfig(t *testing.T, entrypoint string) string {
	t.Helper()
	dir := t.TempDir()
	if err := os.WriteFile(filepath.Join(dir, "entrypoint.json"), []byte(entrypoint), 0o644); err != nil {
		t.Fatal(err)
	}
	return dir
}

func mustCode(t *testing.T, err error, code string) {
	t.Helper()
	var ecErr *Error
	if err == nil || !errors.As(err, &ecErr) || ecErr.Code() != code {
		t.Fatalf("want %s, got %v", code, err)
	}
}

// TestCommitTwoCooperatingWriters: two plans from the same snapshot; the
// second commit finds the document's revision changed and is E_STALE_PLAN.
func TestCommitTwoCooperatingWriters(t *testing.T) {
	dir := scratchConfig(t, `{"a": 1, "b": 2}`)
	snap, err := Open(dir)
	if err != nil {
		t.Fatal(err)
	}
	p1, err := snap.Plan([]Edit{{Document: "entrypoint.json", Op: OpSet, Pointer: "/a", Value: 10}})
	if err != nil {
		t.Fatal(err)
	}
	p2, err := snap.Plan([]Edit{{Document: "entrypoint.json", Op: OpSet, Pointer: "/b", Value: 20}})
	if err != nil {
		t.Fatal(err)
	}
	if _, err := p1.Commit(nil); err != nil {
		t.Fatal(err)
	}
	_, err = p2.Commit(nil)
	mustCode(t, err, CodeStalePlan)
	// Committing the same plan twice is also stale: its own write moved the revision.
	_, err = p1.Commit(nil)
	mustCode(t, err, CodeStalePlan)
	tree, _ := Load(dir)
	if tree["a"] != int64(10) || tree["b"] != int64(2) {
		t.Fatalf("unexpected tree %v", tree)
	}
}

// TestCommitLockContention: a lock file held by another writer is E_LOCKED
// within the timeout, and nothing is written; a stale lock is broken.
func TestCommitLockContention(t *testing.T) {
	dir := scratchConfig(t, `{"a": 1}`)
	snap, _ := Open(dir)
	plan, err := snap.Plan([]Edit{{Document: "entrypoint.json", Op: OpSet, Pointer: "/a", Value: 2}})
	if err != nil {
		t.Fatal(err)
	}
	lock := filepath.Join(dir, lockFileName)
	if err := os.WriteFile(lock, []byte("{}"), 0o644); err != nil {
		t.Fatal(err)
	}
	start := time.Now()
	_, err = plan.Commit(&CommitOptions{LockTimeout: 150 * time.Millisecond})
	mustCode(t, err, CodeLocked)
	if time.Since(start) > 2*time.Second {
		t.Fatalf("lock wait did not respect the timeout")
	}
	data, _ := os.ReadFile(filepath.Join(dir, "entrypoint.json"))
	if string(data) != `{"a": 1}` {
		t.Fatalf("source changed under a held lock")
	}
	if _, err := os.Stat(lock); err != nil {
		t.Fatalf("another writer's lock was removed")
	}
	// A stale lock (older than lockStaleAfter) is broken and the commit proceeds.
	old := time.Now().Add(-2 * lockStaleAfter)
	if err := os.Chtimes(lock, old, old); err != nil {
		t.Fatal(err)
	}
	if _, err := plan.Commit(&CommitOptions{LockTimeout: time.Second}); err != nil {
		t.Fatalf("stale lock was not broken: %v", err)
	}
	if _, err := os.Stat(lock); !errors.Is(err, os.ErrNotExist) {
		t.Fatalf("lock file left behind after commit")
	}
}

// TestCommitPreservesPermissions: the replaced file keeps the original mode.
func TestCommitPreservesPermissions(t *testing.T) {
	if runtime.GOOS == "windows" {
		t.Skip("no permission bits on windows")
	}
	dir := scratchConfig(t, `{"a": 1}`)
	target := filepath.Join(dir, "entrypoint.json")
	if err := os.Chmod(target, 0o600); err != nil {
		t.Fatal(err)
	}
	snap, _ := Open(dir)
	plan, _ := snap.Plan([]Edit{{Document: "entrypoint.json", Op: OpSet, Pointer: "/a", Value: 2}})
	if _, err := plan.Commit(nil); err != nil {
		t.Fatal(err)
	}
	info, _ := os.Stat(target)
	if info.Mode().Perm() != 0o600 {
		t.Fatalf("mode %o, want 0600", info.Mode().Perm())
	}
}

// TestCommitWriteFailureLeavesSourceAndNoTemp: a read-only directory makes
// the temporary file impossible; the result is E_WRITE, the source is intact,
// and no temporary or lock file remains.
func TestCommitWriteFailureLeavesSourceAndNoTemp(t *testing.T) {
	if runtime.GOOS == "windows" || os.Getuid() == 0 {
		t.Skip("needs POSIX permissions and a non-root user")
	}
	dir := scratchConfig(t, `{"x": "@file:sub/x.json"}`)
	sub := filepath.Join(dir, "sub")
	os.Mkdir(sub, 0o755)
	os.WriteFile(filepath.Join(sub, "x.json"), []byte(`{"v": 1}`), 0o644)
	snap, err := Open(dir)
	if err != nil {
		t.Fatal(err)
	}
	plan, err := snap.Plan([]Edit{{Document: "sub/x.json", Op: OpSet, Pointer: "/v", Value: 2}})
	if err != nil {
		t.Fatal(err)
	}
	if err := os.Chmod(sub, 0o555); err != nil {
		t.Fatal(err)
	}
	defer os.Chmod(sub, 0o755)
	_, err = plan.Commit(nil)
	mustCode(t, err, CodeWrite)
	entries, _ := os.ReadDir(sub)
	if len(entries) != 1 {
		t.Fatalf("temporary file left in %s: %v", sub, entries)
	}
	data, _ := os.ReadFile(filepath.Join(sub, "x.json"))
	if string(data) != `{"v": 1}` {
		t.Fatalf("source changed on a failed write")
	}
	if _, err := os.Stat(filepath.Join(dir, lockFileName)); !errors.Is(err, os.ErrNotExist) {
		t.Fatalf("lock file left behind after a failed commit")
	}
}

// TestCommitThroughSymlink: the link survives and its target is replaced.
func TestCommitThroughSymlink(t *testing.T) {
	if runtime.GOOS == "windows" {
		t.Skip("symlink creation needs privilege on windows")
	}
	dir := t.TempDir()
	real := filepath.Join(t.TempDir(), "real.json")
	os.WriteFile(real, []byte(`{"v": 1}`), 0o644)
	os.WriteFile(filepath.Join(dir, "entrypoint.json"), []byte(`{"x": "@file:link.json"}`), 0o644)
	if err := os.Symlink(real, filepath.Join(dir, "link.json")); err != nil {
		t.Skip("cannot create symlink here")
	}
	snap, err := Open(dir)
	if err != nil {
		t.Fatal(err)
	}
	plan, err := snap.Plan([]Edit{{Document: "link.json", Op: OpSet, Pointer: "/v", Value: 2}})
	if err != nil {
		t.Fatal(err)
	}
	if _, err := plan.Commit(nil); err != nil {
		t.Fatal(err)
	}
	if info, _ := os.Lstat(filepath.Join(dir, "link.json")); info.Mode()&os.ModeSymlink == 0 {
		t.Fatalf("the symlink was replaced by a regular file")
	}
	data, _ := os.ReadFile(real)
	if !strings.Contains(string(data), `"v": 2`) {
		t.Fatalf("link target not updated: %s", data)
	}
}

// TestPlanDoesNotTouchDisk pins SPEC §10.6: planning writes nothing.
func TestPlanDoesNotTouchDisk(t *testing.T) {
	dir := scratchConfig(t, `{"a": 1}`)
	before := fileBytes(t, dir)
	snap, _ := Open(dir)
	if _, err := snap.Plan([]Edit{{Document: "entrypoint.json", Op: OpSet, Pointer: "/a", Value: 2}}); err != nil {
		t.Fatal(err)
	}
	after := fileBytes(t, dir)
	if len(after) != len(before) || !bytes.Equal(after["entrypoint.json"], before["entrypoint.json"]) {
		t.Fatalf("Plan changed the directory")
	}
}

// TestOpenUsesProcessEnvironment: the public Open captures the real
// environment, Inspect reports it, and a live change makes a plan stale.
func TestOpenUsesProcessEnvironment(t *testing.T) {
	dir := scratchConfig(t, `{"host": "${EC_EDIT_HOST}"}`)
	t.Setenv("EC_EDIT_HOST", "prod")
	snap, err := Open(dir)
	if err != nil {
		t.Fatal(err)
	}
	if snap.Tree["host"] != "prod" {
		t.Fatalf("captured env not used: %v", snap.Tree)
	}
	origin, err := snap.Inspect("/host")
	if err != nil {
		t.Fatal(err)
	}
	if len(origin.Variables) != 1 || origin.Variables[0].Origin != "process" {
		t.Fatalf("origin %+v", origin)
	}
	plan, _ := snap.Plan([]Edit{{Document: "entrypoint.json", Op: OpSet, Pointer: "/x", Value: 1}})
	t.Setenv("EC_EDIT_HOST", "other")
	_, err = plan.Commit(nil)
	mustCode(t, err, CodeStalePlan)
}

// TestFormatJSONNormalization pins SPEC §10.8's byte-level rules.
func TestFormatJSONNormalization(t *testing.T) {
	v := map[string]any{
		"b":   []any{int64(1), float64(2), 1.5, "x"},
		"a":   map[string]any{},
		"c":   []any{},
		"s":   "q\"\\\n\t\x01é",
		"big": float64(1<<53 - 1),
	}
	got := formatJSON(v)
	want := "{\n" +
		"  \"a\": {},\n" +
		"  \"b\": [\n    1,\n    2,\n    1.5,\n    \"x\"\n  ],\n" +
		"  \"big\": 9007199254740991,\n" +
		"  \"c\": [],\n" +
		"  \"s\": \"q\\\"\\\\\\n\\t\\u0001é\"\n" +
		"}\n"
	if got != want {
		t.Fatalf("got:\n%s\nwant:\n%s", got, want)
	}
	keys := []string{"é", "z", "Z", "a"}
	sort.Strings(keys)
	if strings.Join(keys, "") != "Zazé" {
		t.Fatalf("code point order broken: %v", keys)
	}
}
