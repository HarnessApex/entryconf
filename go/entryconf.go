// Package entryconf loads a config directory — one entrypoint file, any number
// of *.env variable files, "@file:" includes and "$VAR" interpolation — into a
// single tree.
//
// It implements the entryconf specification, version 0.3.0. See SPEC.md in the
// repository root; the fixture suite in testdata/cases defines conformance, and
// testdata/editcases defines conformance of the editing surface (Open,
// Snapshot.Inspect, Snapshot.Plan, Plan.Commit — SPEC §10).
package entryconf

import (
	"errors"
	"os"
	"path/filepath"
	"sort"
)

var _ = os.LookupEnv // the public Load reads the real process environment

// entrypointNames are the only accepted entrypoint file names (SPEC §3).
var entrypointNames = []string{
	"entrypoint.json",
	"entrypoint.yaml",
	"entrypoint.yml",
	"entrypoint.toml",
}

// Load reads the config directory dir and returns the assembled tree.
//
// Values are Go's natural JSON shapes: map[string]any, []any, string, bool,
// int64 or float64 for numbers, and nil for null. Every failure is reported as
// an *Error carrying a SPEC §7 code; nothing is ever partially loaded.
func Load(dir string) (map[string]any, error) {
	return load(dir, os.LookupEnv)
}

// load is the seam used by the conformance harness, which injects the process
// environment instead of mutating the real one. It also holds the package's
// error invariant: every failure that leaves here is an *Error carrying a
// SPEC §7 code, which is what the CLI's "first stderr line is the code"
// contract rests on.
func load(dir string, procEnv envSource) (map[string]any, error) {
	l := &loader{src: osSource{}, procEnv: procEnv}
	tree, err := l.loadTree(dir)
	if err != nil {
		return nil, asError(err)
	}
	return tree, nil
}

// asError is the floor of the error invariant. Every internal path already
// builds an *Error; this catches one that ever escapes unwrapped and gives it
// the code SPEC §2 assigns to unreadable or malformed input.
func asError(err error) *Error {
	var ecErr *Error
	if errors.As(err, &ecErr) {
		return ecErr
	}
	return wrapf(CodeParse, err, "load failed")
}

// loadTree runs the five steps of SPEC §1 against l.src. When l.rec is set,
// every document read, every graft, and every variable lookup is recorded on
// the way through — that is what Open (SPEC §10.2) and a plan's candidate
// evaluation (SPEC §10.6) are built on; a plain Load records nothing.
func (l *loader) loadTree(dir string) (map[string]any, error) {
	// 1. Locate the entrypoint.
	entrypoint, err := l.findEntrypoint(dir)
	if err != nil {
		return nil, err
	}

	// 2. Build the variable namespace.
	fileVars, fileOrigin, err := l.loadEnvFiles(dir)
	if err != nil {
		return nil, err
	}
	l.vars = &vars{files: fileVars, origin: fileOrigin, proc: l.procEnv, used: map[string]bool{}}

	// 3. Parse the entrypoint and resolve every include.
	doc, err := l.parseDocument(entrypoint)
	if err != nil {
		var ecErr *Error
		if errors.As(err, &ecErr) {
			return nil, ecErr
		}
		return nil, wrapf(CodeParse, err, "cannot read entrypoint %q", entrypoint)
	}
	// SPEC §3: the entrypoint *document's* top-level value must be an object.
	// The check is made on the parsed document, before includes and
	// interpolation, so neither can launder a non-object root into a tree.
	// Included files are unconstrained (SPEC §5); only this one file is.
	if _, ok := doc.(map[string]any); !ok {
		return nil, errf(CodeParse,
			"entrypoint %q must hold an object at the top level, not %s", entrypoint, kindOf(doc))
	}
	if l.rec != nil {
		l.rec.graft(entrypoint, Graft{Effective: "", Chain: []Reference{}})
	}
	grafted, err := l.resolveIncludes(doc, &walk{doc: entrypoint, dir: filepath.Dir(entrypoint), files: []string{entrypoint}, chain: []Reference{}})
	if err != nil {
		return nil, err
	}

	// 4. Interpolate.
	interpolated, err := l.interpolate(grafted)
	if err != nil {
		return nil, err
	}

	// 5. Return the tree. The root was checked to be an object above, and
	// neither grafting nor interpolation replaces the root value.
	tree, ok := interpolated.(map[string]any)
	if !ok {
		return nil, errf(CodeParse, "entrypoint %q is not a mapping", entrypoint)
	}
	return tree, nil
}

// kindOf names a value's data-model kind for error messages (SPEC §2).
func kindOf(v any) string {
	switch v.(type) {
	case nil:
		return "null (an empty document counts as null)"
	case bool:
		return "a boolean"
	case string:
		return "a string"
	case []any:
		return "an array"
	case map[string]any:
		return "an object"
	}
	return "a number"
}

// loader carries the per-load state: where files come from, the process
// environment, the variable namespace once built, and (for Open and plans)
// the recorder.
type loader struct {
	src     fileSource
	procEnv envSource
	vars    *vars
	rec     *recorder
}

func (l *loader) findEntrypoint(dir string) (string, error) {
	var found []string
	for _, name := range entrypointNames {
		path := filepath.Join(dir, name)
		if !l.src.isFile(path) {
			continue
		}
		found = append(found, path)
	}
	switch len(found) {
	case 0:
		return "", errf(CodeNoEntrypoint, "no entrypoint file in %q (expected one of entrypoint.json, .yaml, .yml, .toml)", dir)
	case 1:
		return found[0], nil
	default:
		sort.Strings(found)
		return "", errf(CodeMultipleEntrypoints, "%d entrypoint files in %q: %v", len(found), dir, found)
	}
}
