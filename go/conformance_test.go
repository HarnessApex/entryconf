package entryconf

import (
	"errors"
	"fmt"
	"math"
	"os"
	"path/filepath"
	"strconv"
	"strings"
	"testing"
)

// casesDir is the shared, language-neutral fixture suite (SPEC §8).
const casesDir = "../testdata/cases"

// TestConformance walks every case in ../testdata/cases and runs it as a
// subtest named after the case directory. There are deliberately no
// hand-written per-case tests: the fixtures are the only source of truth.
//
// The harness contract (SPEC §8) requires that the variables named by a case
// are set to exactly what procenv.json says and are otherwise unset. Rather
// than mutating the real process environment, the harness injects the case's
// environment through the load seam, so cases cannot see — or race on — the
// real environment at all.
func TestConformance(t *testing.T) {
	entries, err := os.ReadDir(casesDir)
	if err != nil {
		t.Fatalf("cannot read fixture suite %s: %v", casesDir, err)
	}

	cases := 0
	for _, e := range entries {
		if !e.IsDir() {
			continue
		}
		cases++
		name := e.Name()
		dir := filepath.Join(casesDir, name)
		t.Run(name, func(t *testing.T) {
			runCase(t, dir)
		})
	}
	if cases == 0 {
		t.Fatalf("no cases found in %s", casesDir)
	}
	t.Logf("ran %d conformance cases", cases)
}

func runCase(t *testing.T, dir string) {
	t.Helper()

	procEnv := map[string]string{}
	if data, err := os.ReadFile(filepath.Join(dir, "procenv.json")); err == nil {
		v, err := parseJSON("procenv.json", data)
		if err != nil {
			t.Fatalf("bad procenv.json: %v", err)
		}
		obj, ok := v.(map[string]any)
		if !ok {
			t.Fatalf("procenv.json is not an object")
		}
		for k, val := range obj {
			s, ok := val.(string)
			if !ok {
				t.Fatalf("procenv.json value for %q is not a string", k)
			}
			procEnv[k] = s
		}
	} else if !os.IsNotExist(err) {
		t.Fatalf("cannot read procenv.json: %v", err)
	}

	env := func(name string) (string, bool) {
		v, ok := procEnv[name]
		return v, ok
	}

	got, loadErr := load(filepath.Join(dir, "config"), env)

	if data, err := os.ReadFile(filepath.Join(dir, "expected_error.txt")); err == nil {
		want := strings.TrimSpace(string(data))
		if loadErr == nil {
			t.Fatalf("expected error %s, got tree %#v", want, got)
		}
		var ecErr *Error
		if !errors.As(loadErr, &ecErr) {
			t.Fatalf("expected *entryconf.Error with code %s, got %T: %v", want, loadErr, loadErr)
		}
		if ecErr.Code() != want {
			t.Fatalf("expected code %s, got %s (%v)", want, ecErr.Code(), loadErr)
		}
		return
	} else if !os.IsNotExist(err) {
		t.Fatalf("cannot read expected_error.txt: %v", err)
	}

	if loadErr != nil {
		t.Fatalf("unexpected error: %v", loadErr)
	}
	data, err := os.ReadFile(filepath.Join(dir, "expected.json"))
	if err != nil {
		t.Fatalf("case has neither expected.json nor expected_error.txt: %v", err)
	}
	want, err := parseJSON("expected.json", data)
	if err != nil {
		t.Fatalf("bad expected.json: %v", err)
	}
	if diff := valueDiff(want, map[string]any(got), "$"); diff != "" {
		t.Fatalf("tree mismatch: %s", diff)
	}
}

// valueDiff compares structurally; numbers compare numerically, so 8080 and
// 8080.0 are equal (SPEC §8).
func valueDiff(want, got any, path string) string {
	if wn, ok := asFloat(want); ok {
		gn, ok := asFloat(got)
		if !ok {
			return path + ": want number " + render(want) + ", got " + render(got)
		}
		if wn != gn {
			return path + ": want " + render(want) + ", got " + render(got)
		}
		return ""
	}
	switch w := want.(type) {
	case map[string]any:
		g, ok := got.(map[string]any)
		if !ok {
			return path + ": want object, got " + render(got)
		}
		for k, wv := range w {
			gv, present := g[k]
			if !present {
				return path + "." + k + ": missing from result"
			}
			if d := valueDiff(wv, gv, path+"."+k); d != "" {
				return d
			}
		}
		for k := range g {
			if _, present := w[k]; !present {
				return path + "." + k + ": unexpected key in result"
			}
		}
		return ""
	case []any:
		g, ok := got.([]any)
		if !ok {
			return path + ": want array, got " + render(got)
		}
		if len(w) != len(g) {
			return path + ": want " + render(want) + ", got " + render(got)
		}
		for i := range w {
			if d := valueDiff(w[i], g[i], path+"["+strconv.Itoa(i)+"]"); d != "" {
				return d
			}
		}
		return ""
	default:
		if want != got {
			return path + ": want " + render(want) + ", got " + render(got)
		}
		return ""
	}
}

func asFloat(v any) (float64, bool) {
	switch n := v.(type) {
	case int64:
		return float64(n), true
	case float64:
		if math.IsNaN(n) {
			return 0, false
		}
		return n, true
	}
	return 0, false
}

func render(v any) string {
	switch t := v.(type) {
	case string:
		return "\"" + t + "\""
	case nil:
		return "null"
	}
	return fmt.Sprintf("%v", v)
}

// TestLoadUsesProcessEnvironment covers the one thing the injected-environment
// harness cannot: that the public Load reads the real process environment and
// that it overrides a *.env definition (SPEC §4).
func TestLoadUsesProcessEnvironment(t *testing.T) {
	dir := filepath.Join(casesDir, "10-process-env-override", "config")
	if _, err := os.Stat(dir); err != nil {
		t.Skipf("fixture missing: %v", err)
	}
	t.Setenv("EC_TEST_HOST", "prod.example.com") // t.Setenv forbids t.Parallel
	tree, err := Load(dir)
	if err != nil {
		t.Fatalf("Load: %v", err)
	}
	if tree["host"] != "prod.example.com" {
		t.Fatalf("process env did not override app.env: %#v", tree["host"])
	}
}
