package entryconf

import (
	"errors"
	"path/filepath"
	"strings"
)

const (
	includePrefix = "@file:"

	// maxIncludeDepth bounds how deeply includes may nest.
	maxIncludeDepth = 100
)

// resolveIncludes walks a parsed document and replaces every "@file:<path>"
// string with the parsed tree of the referenced file (SPEC §5).
//
// dir is the directory of the file that contains v, since paths are resolved
// relative to the referencing file. chain is the stack of files currently
// being included, innermost last; it is used both for cycle detection and to
// report the cycle.
func (l *loader) resolveIncludes(v any, dir string, chain []string) (any, error) {
	switch t := v.(type) {
	case string:
		return l.resolveIncludeString(t, dir, chain)
	case map[string]any:
		out := make(map[string]any, len(t))
		for k, val := range t {
			// Keys are never includes and are never interpolated (SPEC §6).
			resolved, err := l.resolveIncludes(val, dir, chain)
			if err != nil {
				return nil, err
			}
			out[k] = resolved
		}
		return out, nil
	case []any:
		out := make([]any, len(t))
		for i, val := range t {
			resolved, err := l.resolveIncludes(val, dir, chain)
			if err != nil {
				return nil, err
			}
			out[i] = resolved
		}
		return out, nil
	default:
		return v, nil
	}
}

func (l *loader) resolveIncludeString(s string, dir string, chain []string) (any, error) {
	if !strings.HasPrefix(s, "@") {
		return s, nil
	}
	// SPEC §5 escaping: a leading "@@" becomes a literal "@" and the string is
	// never treated as an include.
	if strings.HasPrefix(s, "@@") {
		return "@" + s[2:], nil
	}
	if !strings.HasPrefix(s, includePrefix) {
		// Reserved for future directives.
		return nil, errf(CodeSubstitution, "unknown directive %q (write %q to mean a literal leading @)", s, "@"+s)
	}

	target := s[len(includePrefix):]
	abs := target
	if !filepath.IsAbs(abs) {
		abs = filepath.Join(dir, target)
	}
	abs = filepath.Clean(abs)

	if _, ok := parserFor(abs); !ok {
		return nil, errf(CodeInclude, "unsupported include extension: %q", s)
	}
	for _, seen := range chain {
		if seen == abs {
			return nil, errf(CodeIncludeCycle, "include cycle: %s", strings.Join(append(append([]string{}, chain...), abs), " -> "))
		}
	}
	// Backstop for a cycle that path comparison cannot see, e.g. one made of
	// symlinks pointing at each other under different names.
	if len(chain) > maxIncludeDepth {
		return nil, errf(CodeIncludeCycle, "include nesting deeper than %d files: %s",
			maxIncludeDepth, strings.Join(append(append([]string{}, chain...), abs), " -> "))
	}

	doc, err := parseDocument(abs)
	if err != nil {
		var ecErr *Error
		if errors.As(err, &ecErr) {
			return nil, ecErr // E_PARSE for an unparseable target
		}
		return nil, wrapf(CodeInclude, err, "cannot read include target %q", abs)
	}
	next := make([]string, len(chain), len(chain)+1)
	copy(next, chain)
	return l.resolveIncludes(doc, filepath.Dir(abs), append(next, abs))
}
