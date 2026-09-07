package entryconf

import (
	"errors"
	"path/filepath"
	"strconv"
	"strings"
)

const (
	includePrefix = "@file:"

	// maxIncludeDepth bounds how deeply includes may nest.
	maxIncludeDepth = 100
)

// walk is the position of the include resolver: the file whose value is being
// walked and its directory (paths are relative to the referencing file), the
// pointer within that file (src) and within the effective tree (eff), the
// stack of files currently being included for cycle detection (innermost
// last), and the @file: references traversed so far (SPEC §10.3 "chain").
type walk struct {
	doc   string
	dir   string
	src   string
	eff   string
	files []string
	chain []Reference
}

func (w *walk) step(token string) *walk {
	next := *w
	next.src = w.src + "/" + escapePointerToken(token)
	next.eff = w.eff + "/" + escapePointerToken(token)
	return &next
}

// resolveIncludes walks a parsed document and replaces every "@file:<path>"
// string with the parsed tree of the referenced file (SPEC §5).
func (l *loader) resolveIncludes(v any, w *walk) (any, error) {
	switch t := v.(type) {
	case string:
		return l.resolveIncludeString(t, w)
	case map[string]any:
		out := make(map[string]any, len(t))
		for k, val := range t {
			// Keys are never includes and are never interpolated (SPEC §6).
			resolved, err := l.resolveIncludes(val, w.step(k))
			if err != nil {
				return nil, err
			}
			out[k] = resolved
		}
		return out, nil
	case []any:
		out := make([]any, len(t))
		for i, val := range t {
			resolved, err := l.resolveIncludes(val, w.step(strconv.Itoa(i)))
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

func (l *loader) resolveIncludeString(s string, w *walk) (any, error) {
	dir, chain := w.dir, w.files
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

	abs := includeTarget(s, dir)

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

	doc, err := l.parseDocument(abs)
	if err != nil {
		var ecErr *Error
		if errors.As(err, &ecErr) {
			return nil, ecErr // E_PARSE for an unparseable target
		}
		return nil, wrapf(CodeInclude, err, "cannot read include target %q", abs)
	}
	next := make([]string, len(chain), len(chain)+1)
	copy(next, chain)
	refs := make([]Reference, len(w.chain), len(w.chain)+1)
	copy(refs, w.chain)
	refs = append(refs, Reference{Document: w.doc, Pointer: w.src})
	if l.rec != nil {
		l.rec.graft(abs, Graft{Effective: w.eff, Chain: refs})
	}
	return l.resolveIncludes(doc, &walk{doc: abs, dir: filepath.Dir(abs), src: "", eff: w.eff, files: append(next, abs), chain: refs})
}

// includeTarget resolves the path of an "@file:<path>" string relative to the
// directory of the file holding it (SPEC §5), cleaned.
func includeTarget(s, dir string) string {
	target := s[len(includePrefix):]
	abs := target
	if !filepath.IsAbs(abs) {
		abs = filepath.Join(dir, target)
	}
	return filepath.Clean(abs)
}

// isInclude reports whether an authored string is an "@file:" reference (as
// opposed to an "@@" escape, a literal, or a reserved directive).
func isInclude(s string) bool {
	return strings.HasPrefix(s, includePrefix)
}
