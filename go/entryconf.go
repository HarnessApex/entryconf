// Package entryconf loads a config directory — one entrypoint file, any number
// of *.env variable files, "@file:" includes and "$VAR" interpolation — into a
// single tree.
//
// It implements the entryconf specification, version 0.2.0. See SPEC.md in the
// repository root; the fixture suite in testdata/cases defines conformance.
package entryconf

import (
	"errors"
	"os"
	"path/filepath"
	"sort"
)

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
	tree, err := loadTree(dir, procEnv)
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

func loadTree(dir string, procEnv envSource) (map[string]any, error) {
	// 1. Locate the entrypoint.
	entrypoint, err := findEntrypoint(dir)
	if err != nil {
		return nil, err
	}

	// 2. Build the variable namespace.
	fileVars, err := loadEnvFiles(dir)
	if err != nil {
		return nil, err
	}
	l := &loader{vars: &vars{files: fileVars, proc: procEnv}}

	// 3. Parse the entrypoint and resolve every include.
	doc, err := parseDocument(entrypoint)
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
	grafted, err := l.resolveIncludes(doc, filepath.Dir(entrypoint), []string{entrypoint})
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

// loader carries the per-Load state: the variable namespace.
type loader struct {
	vars *vars
}

func findEntrypoint(dir string) (string, error) {
	var found []string
	for _, name := range entrypointNames {
		path := filepath.Join(dir, name)
		info, err := os.Stat(path)
		if err != nil || info.IsDir() {
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
