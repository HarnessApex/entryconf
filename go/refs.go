package entryconf

// reference is one $ form found in an authored string (SPEC §6).
type reference struct {
	name       string
	hasDefault bool
}

// scanReferences lists the variable references of an authored string in order
// of appearance, tolerating malformed forms (it is used for provenance of a
// value that already loaded, so the string is known to be well-formed).
func scanReferences(s string) []reference {
	var out []reference
	i := 0
	for i < len(s) {
		if s[i] != '$' || i+1 >= len(s) {
			i++
			continue
		}
		switch next := s[i+1]; {
		case next == '$':
			i += 2
		case next == '{':
			end := indexByteFrom(s, '}', i+2)
			if end < 0 {
				return out
			}
			inner := s[i+2 : end]
			name, hasDefault := inner, false
			if colon := indexByteFrom(inner, ':', 0); colon >= 0 {
				name, hasDefault = inner[:colon], true
			}
			out = append(out, reference{name: name, hasDefault: hasDefault})
			i = end + 1
		case isNameStart(next):
			j := i + 1
			for j < len(s) && isNameChar(s[j]) {
				j++
			}
			out = append(out, reference{name: s[i+1 : j]})
			i = j
		default:
			i++
		}
	}
	return out
}

func indexByteFrom(s string, c byte, from int) int {
	for i := from; i < len(s); i++ {
		if s[i] == c {
			return i
		}
	}
	return -1
}
