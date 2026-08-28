// Command lintcases validates the entryconf conformance fixture suite.
//
// It checks the structure of testdata/cases/ and the coupling between the
// failure cases and the error-code list in testdata/errors.json:
//
//   - every entry under testdata/cases/ is a directory named NN-kebab-name,
//     with the numeric prefix unique across the suite;
//   - every case directory contains a config/ directory;
//   - every case directory contains exactly one of expected.json or
//     expected_error.txt;
//   - every expected.json parses as JSON;
//   - every expected_error.txt holds exactly one error code, and that code is
//     listed in testdata/errors.json;
//   - every code listed in testdata/errors.json has at least one failure case.
//
// Usage:
//
//	go run . -root ../..
//
// With no -root the repo root is inferred from this file's compile-time path,
// then from the working directory. The tool is stdlib-only by design: it must
// run before, and independently of, any language implementation.
package main

import (
	"encoding/json"
	"errors"
	"flag"
	"fmt"
	"os"
	"path/filepath"
	"regexp"
	"runtime"
	"sort"
	"strconv"
	"strings"
)

// caseNameRe matches a case directory name: a zero-padded number, a hyphen,
// then a kebab-cased label (lowercase alphanumerics joined by single hyphens).
var caseNameRe = regexp.MustCompile(`^([0-9]{2,})-([a-z0-9]+(?:-[a-z0-9]+)*)$`)

// codeRe matches an error code as spelled in SPEC §7.
var codeRe = regexp.MustCompile(`^E_[A-Z0-9]+(?:_[A-Z0-9]+)*$`)

// linter accumulates violations so one run reports every problem, not just
// the first.
type linter struct {
	root       string
	violations []string
}

func (l *linter) errorf(where, format string, args ...any) {
	l.violations = append(l.violations, fmt.Sprintf("%s: %s", where, fmt.Sprintf(format, args...)))
}

func main() {
	root := flag.String("root", "", "path to the repository root (default: inferred from the source path, then the working directory)")
	flag.Parse()

	resolved, err := resolveRoot(*root)
	if err != nil {
		fmt.Fprintf(os.Stderr, "lintcases: %v\n", err)
		os.Exit(2)
	}

	l := &linter{root: resolved}
	if err := l.run(); err != nil {
		fmt.Fprintf(os.Stderr, "lintcases: %v\n", err)
		os.Exit(2)
	}

	if len(l.violations) > 0 {
		sort.Strings(l.violations)
		for _, v := range l.violations {
			fmt.Fprintf(os.Stderr, "%s\n", v)
		}
		fmt.Fprintf(os.Stderr, "\nlintcases: %d violation(s) in %s\n", len(l.violations), l.root)
		os.Exit(1)
	}
	fmt.Printf("lintcases: fixture suite OK (%s)\n", l.root)
}

// resolveRoot picks the repository root: the -root flag if given, otherwise
// the directory three levels above this source file (tools/lintcases/main.go),
// otherwise the nearest ancestor of the working directory that has a
// testdata/cases directory.
func resolveRoot(flagValue string) (string, error) {
	if flagValue != "" {
		abs, err := filepath.Abs(flagValue)
		if err != nil {
			return "", fmt.Errorf("resolving -root %q: %w", flagValue, err)
		}
		if !hasSuite(abs) {
			return "", fmt.Errorf("-root %s does not contain testdata/cases", abs)
		}
		return abs, nil
	}

	if _, file, _, ok := runtime.Caller(0); ok {
		candidate := filepath.Dir(filepath.Dir(filepath.Dir(file))) // .../tools/lintcases -> .../tools -> repo root
		if hasSuite(candidate) {
			return candidate, nil
		}
	}

	cwd, err := os.Getwd()
	if err != nil {
		return "", fmt.Errorf("determining working directory: %w", err)
	}
	for dir := cwd; ; {
		if hasSuite(dir) {
			return dir, nil
		}
		parent := filepath.Dir(dir)
		if parent == dir {
			break
		}
		dir = parent
	}
	return "", fmt.Errorf("could not find a repository root with testdata/cases (searched upward from %s); pass -root", cwd)
}

func hasSuite(root string) bool {
	info, err := os.Stat(filepath.Join(root, "testdata", "cases"))
	return err == nil && info.IsDir()
}

func (l *linter) run() error {
	casesDir := filepath.Join(l.root, "testdata", "cases")
	errorsPath := filepath.Join(l.root, "testdata", "errors.json")

	knownCodes, err := loadErrorCodes(errorsPath)
	if err != nil {
		return err
	}

	entries, err := os.ReadDir(casesDir)
	if err != nil {
		return fmt.Errorf("reading %s: %w", casesDir, err)
	}

	numbers := map[int]string{}       // numeric prefix -> first case that used it
	codeUsed := map[string][]string{} // error code -> cases expecting it
	caseCount := 0

	for _, entry := range entries {
		name := entry.Name()
		if strings.HasPrefix(name, ".") {
			continue
		}
		rel := filepath.Join("testdata", "cases", name)

		if !entry.IsDir() {
			if name == "README.md" {
				continue
			}
			l.errorf(rel, "stray file in cases/: every entry must be a case directory")
			continue
		}
		caseCount++

		match := caseNameRe.FindStringSubmatch(name)
		if match == nil {
			l.errorf(rel, "case directory name must be NN-kebab-name (zero-padded number, hyphen, lowercase words joined by single hyphens)")
		} else {
			n, convErr := strconv.Atoi(match[1])
			if convErr != nil {
				l.errorf(rel, "case number %q is not a number", match[1])
			} else if prev, dup := numbers[n]; dup {
				l.errorf(rel, "case number %02d is already used by %s", n, prev)
			} else {
				numbers[n] = name
			}
		}

		dir := filepath.Join(casesDir, name)
		l.checkCase(dir, rel, knownCodes, codeUsed)
	}

	if caseCount == 0 {
		l.errorf(filepath.Join("testdata", "cases"), "no case directories found")
	}

	for _, code := range sortedKeys(knownCodes) {
		if len(codeUsed[code]) == 0 {
			l.errorf(filepath.Join("testdata", "errors.json"),
				"error code %s has no failure case: add a case with expected_error.txt containing %s", code, code)
		}
	}
	return nil
}

// checkCase validates one case directory.
func (l *linter) checkCase(dir, rel string, knownCodes map[string]bool, codeUsed map[string][]string) {
	if info, err := os.Stat(filepath.Join(dir, "config")); err != nil || !info.IsDir() {
		l.errorf(rel, "missing config/ directory")
	}

	expectedPath := filepath.Join(dir, "expected.json")
	errorPath := filepath.Join(dir, "expected_error.txt")
	hasExpected := isFile(expectedPath)
	hasError := isFile(errorPath)

	switch {
	case hasExpected && hasError:
		l.errorf(rel, "has both expected.json and expected_error.txt: a case is either a success case or a failure case")
	case !hasExpected && !hasError:
		l.errorf(rel, "has neither expected.json nor expected_error.txt")
	}

	if hasExpected {
		data, err := os.ReadFile(expectedPath)
		if err != nil {
			l.errorf(filepath.Join(rel, "expected.json"), "unreadable: %v", err)
		} else if !json.Valid(data) {
			// Decode again for a message that points at the offending offset.
			var v any
			l.errorf(filepath.Join(rel, "expected.json"), "invalid JSON: %v", json.Unmarshal(data, &v))
		}
	}

	if hasError {
		data, err := os.ReadFile(errorPath)
		if err != nil {
			l.errorf(filepath.Join(rel, "expected_error.txt"), "unreadable: %v", err)
			return
		}
		fields := strings.Fields(string(data))
		switch {
		case len(fields) == 0:
			l.errorf(filepath.Join(rel, "expected_error.txt"), "empty: must contain exactly one error code")
		case len(fields) > 1:
			l.errorf(filepath.Join(rel, "expected_error.txt"),
				"contains %d tokens (%s): must contain exactly one error code", len(fields), strings.Join(fields, " "))
		default:
			code := fields[0]
			if !codeRe.MatchString(code) {
				l.errorf(filepath.Join(rel, "expected_error.txt"), "%q is not an error code (expected E_UPPER_SNAKE)", code)
			}
			if !knownCodes[code] {
				l.errorf(filepath.Join(rel, "expected_error.txt"),
					"error code %s is not listed in testdata/errors.json", code)
			}
			codeUsed[code] = append(codeUsed[code], rel)
		}
	}
}

func isFile(path string) bool {
	info, err := os.Stat(path)
	return err == nil && info.Mode().IsRegular()
}

// loadErrorCodes reads testdata/errors.json. The file is owned elsewhere in
// the repo, so several plausible shapes are accepted: an array of code
// strings, an array of objects with a "code" (or "name") field, an object
// keyed by code, or any of those nested under an "errors"/"codes" key.
func loadErrorCodes(path string) (map[string]bool, error) {
	data, err := os.ReadFile(path)
	if err != nil {
		if errors.Is(err, os.ErrNotExist) {
			return nil, fmt.Errorf("%s does not exist: the error-code list is required to lint failure cases", path)
		}
		return nil, fmt.Errorf("reading %s: %w", path, err)
	}
	var doc any
	if err := json.Unmarshal(data, &doc); err != nil {
		return nil, fmt.Errorf("parsing %s: %w", path, err)
	}
	codes := map[string]bool{}
	if err := collectCodes(doc, codes); err != nil {
		return nil, fmt.Errorf("%s: %w", path, err)
	}
	if len(codes) == 0 {
		return nil, fmt.Errorf("%s lists no error codes", path)
	}
	return codes, nil
}

func collectCodes(doc any, out map[string]bool) error {
	switch v := doc.(type) {
	case []any:
		for _, item := range v {
			switch e := item.(type) {
			case string:
				if !codeRe.MatchString(e) {
					return fmt.Errorf("%q is not an error code (expected E_UPPER_SNAKE)", e)
				}
				out[e] = true
			case map[string]any:
				code, ok := stringField(e, "code", "name", "id")
				if !ok {
					return fmt.Errorf("list entry has no \"code\" field")
				}
				if !codeRe.MatchString(code) {
					return fmt.Errorf("%q is not an error code (expected E_UPPER_SNAKE)", code)
				}
				out[code] = true
			default:
				return fmt.Errorf("unexpected list entry of type %T", item)
			}
		}
		return nil
	case map[string]any:
		for _, key := range []string{"errors", "codes"} {
			if nested, ok := v[key]; ok {
				return collectCodes(nested, out)
			}
		}
		for key := range v {
			if !codeRe.MatchString(key) {
				return fmt.Errorf("key %q is not an error code (expected E_UPPER_SNAKE)", key)
			}
			out[key] = true
		}
		return nil
	default:
		return fmt.Errorf("unexpected top-level JSON value of type %T; expected an array or object of error codes", doc)
	}
}

func stringField(m map[string]any, keys ...string) (string, bool) {
	for _, key := range keys {
		if raw, ok := m[key]; ok {
			if s, ok := raw.(string); ok {
				return s, true
			}
		}
	}
	return "", false
}

func sortedKeys(m map[string]bool) []string {
	keys := make([]string, 0, len(m))
	for k := range m {
		keys = append(keys, k)
	}
	sort.Strings(keys)
	return keys
}
