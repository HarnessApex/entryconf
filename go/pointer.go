package entryconf

import (
	"strconv"
	"strings"
)

// parsePointer splits an RFC 6901 JSON Pointer into its tokens (SPEC §10.3).
// "" is the root (no tokens). A pointer that does not start with "/" or has a
// "~" not followed by "0" or "1" is E_PATH.
func parsePointer(p string) ([]string, error) {
	if p == "" {
		return []string{}, nil
	}
	if !strings.HasPrefix(p, "/") {
		return nil, errf(CodePath, "pointer %q must be empty or start with %q", p, "/")
	}
	parts := strings.Split(p[1:], "/")
	tokens := make([]string, len(parts))
	for i, raw := range parts {
		var b strings.Builder
		for j := 0; j < len(raw); j++ {
			c := raw[j]
			if c != '~' {
				b.WriteByte(c)
				continue
			}
			if j+1 >= len(raw) {
				return nil, errf(CodePath, "pointer %q: %q must be followed by 0 or 1", p, "~")
			}
			switch raw[j+1] {
			case '0':
				b.WriteByte('~')
			case '1':
				b.WriteByte('/')
			default:
				return nil, errf(CodePath, "pointer %q: %q must be followed by 0 or 1", p, "~")
			}
			j++
		}
		tokens[i] = b.String()
	}
	return tokens, nil
}

// escapePointerToken applies RFC 6901 escaping to one reference token.
func escapePointerToken(t string) string {
	return strings.ReplaceAll(strings.ReplaceAll(t, "~", "~0"), "/", "~1")
}

func joinPointer(tokens []string) string {
	var b strings.Builder
	for _, t := range tokens {
		b.WriteByte('/')
		b.WriteString(escapePointerToken(t))
	}
	return b.String()
}

// arrayIndex parses an array index token: decimal digits with no leading
// zeros (RFC 6901). ok is false for anything else, including "-".
func arrayIndex(token string) (int, bool) {
	if token == "" || (len(token) > 1 && token[0] == '0') {
		return 0, false
	}
	for i := 0; i < len(token); i++ {
		if token[i] < '0' || token[i] > '9' {
			return 0, false
		}
	}
	n, err := strconv.Atoi(token)
	if err != nil {
		return 0, false
	}
	return n, true
}

// deepCopy clones a JSON-shaped value so edits never touch the snapshot.
func deepCopy(v any) any {
	switch t := v.(type) {
	case map[string]any:
		out := make(map[string]any, len(t))
		for k, val := range t {
			out[k] = deepCopy(val)
		}
		return out
	case []any:
		out := make([]any, len(t))
		for i, val := range t {
			out[i] = deepCopy(val)
		}
		return out
	default:
		return v
	}
}

// setAt applies a "set" edit (SPEC §10.5) to root and returns the new root.
// Missing parents are created as objects; array steps must exist; the final
// token may append with the index equal to the length or "-".
func setAt(root any, pointer string, value any) (any, error) {
	tokens, err := parsePointer(pointer)
	if err != nil {
		return nil, err
	}
	if len(tokens) == 0 {
		return value, nil
	}
	// Walk to the parent, creating missing object members along the way.
	parent := root
	var parents []any // the containers on the path, for write-back of arrays
	var steps []string
	for _, tok := range tokens[:len(tokens)-1] {
		switch c := parent.(type) {
		case map[string]any:
			child, ok := c[tok]
			if !ok {
				child = map[string]any{}
				c[tok] = child
			}
			parents, steps = append(parents, parent), append(steps, tok)
			parent = child
		case []any:
			i, ok := arrayIndex(tok)
			if !ok || i >= len(c) {
				return nil, errf(CodePath, "pointer %q: array index %q does not exist", pointer, tok)
			}
			parents, steps = append(parents, parent), append(steps, tok)
			parent = c[i]
		default:
			return nil, errf(CodePath, "pointer %q: cannot descend into a scalar at %q", pointer, tok)
		}
	}
	last := tokens[len(tokens)-1]
	var replaced any
	switch c := parent.(type) {
	case map[string]any:
		c[last] = value
		return root, nil
	case []any:
		if last == "-" {
			replaced = append(c, value)
		} else {
			i, ok := arrayIndex(last)
			if !ok || i > len(c) {
				return nil, errf(CodePath, "pointer %q: array index %q is out of range (0..%d)", pointer, last, len(c))
			}
			if i == len(c) {
				replaced = append(c, value)
			} else {
				c[i] = value
				return root, nil
			}
		}
	default:
		return nil, errf(CodePath, "pointer %q: cannot set a member of a scalar", pointer)
	}
	// An append produced a new slice header; write it back into its holder.
	return writeBack(root, parents, steps, replaced), nil
}

// removeAt applies a "remove" edit (SPEC §10.5): every step must exist, the
// root cannot be removed, and array elements shift down.
func removeAt(root any, pointer string) (any, error) {
	tokens, err := parsePointer(pointer)
	if err != nil {
		return nil, err
	}
	if len(tokens) == 0 {
		return nil, errf(CodePath, "the root value cannot be removed")
	}
	parent := root
	var parents []any
	var steps []string
	for _, tok := range tokens[:len(tokens)-1] {
		switch c := parent.(type) {
		case map[string]any:
			child, ok := c[tok]
			if !ok {
				return nil, errf(CodePath, "pointer %q: no member %q", pointer, tok)
			}
			parents, steps = append(parents, parent), append(steps, tok)
			parent = child
		case []any:
			i, ok := arrayIndex(tok)
			if !ok || i >= len(c) {
				return nil, errf(CodePath, "pointer %q: array index %q does not exist", pointer, tok)
			}
			parents, steps = append(parents, parent), append(steps, tok)
			parent = c[i]
		default:
			return nil, errf(CodePath, "pointer %q: cannot descend into a scalar at %q", pointer, tok)
		}
	}
	last := tokens[len(tokens)-1]
	switch c := parent.(type) {
	case map[string]any:
		if _, ok := c[last]; !ok {
			return nil, errf(CodePath, "pointer %q: no member %q to remove", pointer, last)
		}
		delete(c, last)
		return root, nil
	case []any:
		i, ok := arrayIndex(last)
		if !ok || i >= len(c) {
			return nil, errf(CodePath, "pointer %q: array index %q does not exist", pointer, last)
		}
		shorter := append(append([]any{}, c[:i]...), c[i+1:]...)
		return writeBack(root, parents, steps, shorter), nil
	default:
		return nil, errf(CodePath, "pointer %q: cannot remove a member of a scalar", pointer)
	}
}

// writeBack stores a replaced container at the position described by
// parents/steps and returns the (possibly new) root.
func writeBack(root any, parents []any, steps []string, replaced any) any {
	if len(parents) == 0 {
		return replaced
	}
	holder := parents[len(parents)-1]
	step := steps[len(steps)-1]
	switch h := holder.(type) {
	case map[string]any:
		h[step] = replaced
		return root
	case []any:
		i, _ := arrayIndex(step)
		h[i] = replaced
		return root
	}
	return root
}

// resolvePointer reads the value at pointer within v, or E_PATH.
func resolvePointer(v any, pointer string) (any, error) {
	tokens, err := parsePointer(pointer)
	if err != nil {
		return nil, err
	}
	cur := v
	for _, tok := range tokens {
		switch c := cur.(type) {
		case map[string]any:
			child, ok := c[tok]
			if !ok {
				return nil, errf(CodePath, "pointer %q: no member %q", pointer, tok)
			}
			cur = child
		case []any:
			i, ok := arrayIndex(tok)
			if !ok || i >= len(c) {
				return nil, errf(CodePath, "pointer %q: array index %q does not exist", pointer, tok)
			}
			cur = c[i]
		default:
			return nil, errf(CodePath, "pointer %q: cannot descend into a scalar at %q", pointer, tok)
		}
	}
	return cur, nil
}
