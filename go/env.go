package entryconf

import (
	"os"
	"path/filepath"
	"regexp"
	"sort"
	"strings"
)

// SPEC §4: NAME matches [A-Za-z_][A-Za-z0-9_]*.
var envNameRe = regexp.MustCompile(`^[A-Za-z_][A-Za-z0-9_]*$`)

// envSource is the process-environment seam. The public Load uses the real
// process environment; the conformance harness injects a map instead so that
// no variable named by a fixture can leak in from the outside.
type envSource func(name string) (string, bool)

// vars is the single global variable namespace for a whole include tree
// (SPEC §4): the config directory's own *.env files, with the process
// environment taking precedence.
type vars struct {
	files map[string]string
	proc  envSource
}

func (v *vars) lookup(name string) (string, bool) {
	if val, ok := v.proc(name); ok {
		return val, true
	}
	val, ok := v.files[name]
	return val, ok
}

// loadEnvFiles reads every *.env file directly in dir (non-recursive). The
// files are unordered peers: a name defined twice, in one file or across two,
// is E_ENV_CONFLICT.
func loadEnvFiles(dir string) (map[string]string, error) {
	entries, err := os.ReadDir(dir)
	if err != nil {
		return nil, wrapf(CodeParse, err, "cannot read config directory %q", dir)
	}

	names := make([]string, 0, len(entries))
	for _, e := range entries {
		if e.IsDir() || !strings.HasSuffix(e.Name(), ".env") {
			continue
		}
		names = append(names, e.Name())
	}
	sort.Strings(names) // deterministic error messages only; files are peers

	values := make(map[string]string)
	origin := make(map[string]string)
	for _, name := range names {
		path := filepath.Join(dir, name)
		data, err := os.ReadFile(path)
		if err != nil {
			return nil, wrapf(CodeParse, err, "cannot read env file %q", path)
		}
		if err := checkUTF8(path, data); err != nil {
			return nil, err
		}
		fileVals, err := parseEnvFile(path, string(data))
		if err != nil {
			return nil, err
		}
		for _, kv := range fileVals {
			if prev, dup := origin[kv.name]; dup {
				return nil, errf(CodeEnvConflict,
					"variable %q defined in both %q and %q", kv.name, prev, path)
			}
			origin[kv.name] = path
			values[kv.name] = kv.value
		}
	}
	return values, nil
}

type envPair struct{ name, value string }

// parseEnvFile implements the strict dotenv subset of SPEC §4. Anything that
// is not blank, a comment, or NAME=value is E_PARSE.
//
// The whole line is trimmed before classification, so indentation and
// trailing whitespace are fine; the name itself is then taken verbatim, so
// whitespace between NAME and "=" ("FOO = bar") is E_PARSE.
func parseEnvFile(path, text string) ([]envPair, error) {
	var out []envPair
	seen := make(map[string]int)
	for i, raw := range strings.Split(text, "\n") {
		lineNo := i + 1
		line := strings.TrimSpace(strings.TrimSuffix(raw, "\r"))
		if line == "" || strings.HasPrefix(line, "#") {
			continue
		}
		eq := strings.IndexByte(line, '=')
		if eq < 0 {
			return nil, errf(CodeParse, "%s:%d: not a NAME=value line: %q", path, lineNo, line)
		}
		name := line[:eq]
		if !envNameRe.MatchString(name) {
			return nil, errf(CodeParse, "%s:%d: invalid variable name %q", path, lineNo, name)
		}
		if prev, dup := seen[name]; dup {
			return nil, errf(CodeEnvConflict,
				"variable %q defined twice in %q (lines %d and %d)", name, path, prev, lineNo)
		}
		seen[name] = lineNo
		out = append(out, envPair{name: name, value: unquoteEnvValue(strings.TrimSpace(line[eq+1:]))})
	}
	return out, nil
}

// unquoteEnvValue strips one layer of matching single or double quotes. No
// escape processing: this is a strict subset of dotenv, all values are strings.
func unquoteEnvValue(v string) string {
	if len(v) >= 2 {
		q := v[0]
		if (q == '\'' || q == '"') && v[len(v)-1] == q {
			return v[1 : len(v)-1]
		}
	}
	return v
}
