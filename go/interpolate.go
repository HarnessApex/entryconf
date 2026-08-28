package entryconf

import (
	"regexp"
	"strings"
)

// interpolate walks the assembled tree and substitutes variable references in
// every string value (SPEC §6). Object keys are never interpolated.
func (l *loader) interpolate(v any) (any, error) {
	switch t := v.(type) {
	case string:
		return l.interpolateString(t)
	case map[string]any:
		out := make(map[string]any, len(t))
		for k, val := range t {
			iv, err := l.interpolate(val)
			if err != nil {
				return nil, err
			}
			out[k] = iv
		}
		return out, nil
	case []any:
		out := make([]any, len(t))
		for i, val := range t {
			iv, err := l.interpolate(val)
			if err != nil {
				return nil, err
			}
			out[i] = iv
		}
		return out, nil
	default:
		return v, nil
	}
}

// interpolateString scans one string. Every use of '$' must be one of the four
// forms in SPEC §6; anything else is E_SUBSTITUTION. Substituted text is inert:
// it is written straight to the output and never re-scanned.
func (l *loader) interpolateString(s string) (any, error) {
	if !strings.ContainsRune(s, '$') {
		return s, nil
	}

	var b strings.Builder
	refs := 0        // number of variable references seen
	wholeRef := true // the string is exactly one reference and nothing else

	i := 0
	for i < len(s) {
		c := s[i]
		if c != '$' {
			b.WriteByte(c)
			i++
			wholeRef = false
			continue
		}
		if i+1 >= len(s) {
			return nil, errf(CodeSubstitution, "trailing %q in %q", "$", s)
		}
		switch next := s[i+1]; {
		case next == '$': // "$$" is a literal "$"
			b.WriteByte('$')
			i += 2
			wholeRef = false
		case next == '{':
			end := strings.IndexByte(s[i+2:], '}')
			if end < 0 {
				return nil, errf(CodeSubstitution, "unterminated %q in %q", "${", s)
			}
			name, def, hasDef, err := parseBraced(s[i+2:i+2+end], s)
			if err != nil {
				return nil, err
			}
			val, err := l.value(name, def, hasDef, s)
			if err != nil {
				return nil, err
			}
			b.WriteString(val)
			refs++
			if i != 0 || i+2+end+1 != len(s) {
				wholeRef = false
			}
			i += 2 + end + 1
		case isNameStart(next):
			j := i + 1
			for j < len(s) && isNameChar(s[j]) {
				j++
			}
			name := s[i+1 : j]
			val, err := l.value(name, "", false, s)
			if err != nil {
				return nil, err
			}
			b.WriteString(val)
			refs++
			if i != 0 || j != len(s) {
				wholeRef = false
			}
			i = j
		default:
			return nil, errf(CodeSubstitution, "malformed %q reference in %q", "$", s)
		}
	}

	out := b.String()
	// Whole-value typing (SPEC §6): exactly one reference and nothing else.
	if refs == 1 && wholeRef {
		if typed, ok := typedScalar(out); ok {
			return typed, nil
		}
	}
	return out, nil
}

// parseBraced splits the text between "${" and "}". Only the ":-" modifier is
// allowed; the rest of the "${NAME:...}" namespace is reserved.
func parseBraced(inner, whole string) (name, def string, hasDef bool, err error) {
	if inner == "" {
		return "", "", false, errf(CodeSubstitution, "empty %q in %q", "${}", whole)
	}
	if colon := strings.IndexByte(inner, ':'); colon >= 0 {
		name, def, hasDef = inner[:colon], inner[colon+1:], true
		if !strings.HasPrefix(def, "-") {
			return "", "", false, errf(CodeSubstitution,
				"reserved modifier in %q (only %q is defined)", "${"+inner+"}", ":-")
		}
		def = def[1:]
	} else {
		name = inner
	}
	if !envNameRe.MatchString(name) {
		return "", "", false, errf(CodeSubstitution, "invalid variable name %q in %q", name, whole)
	}
	return name, def, hasDef, nil
}

// value looks a variable up, applying the literal default if there is one.
func (l *loader) value(name, def string, hasDef bool, whole string) (string, error) {
	if v, ok := l.vars.lookup(name); ok {
		return v, nil
	}
	if hasDef {
		return def, nil // the default text is literal: no nested substitution
	}
	return "", errf(CodeMissingVar, "variable %q is not set (referenced in %q)", name, whole)
}

func isNameStart(c byte) bool {
	return c == '_' || (c >= 'a' && c <= 'z') || (c >= 'A' && c <= 'Z')
}

func isNameChar(c byte) bool {
	return isNameStart(c) || (c >= '0' && c <= '9')
}

// jsonNumberRe is the JSON number grammar (RFC 8259 §6).
var jsonNumberRe = regexp.MustCompile(`^-?(0|[1-9][0-9]*)(\.[0-9]+)?([eE][-+]?[0-9]+)?$`)

// typedScalar implements whole-value typing: a lone reference whose substituted
// text is exactly true, false, null or a JSON number becomes that scalar.
func typedScalar(s string) (any, bool) {
	switch s {
	case "true":
		return true, true
	case "false":
		return false, true
	case "null":
		return nil, true
	}
	if jsonNumberRe.MatchString(s) {
		n, err := numberFromString(s)
		if err == nil {
			return n, true
		}
	}
	return nil, false
}
