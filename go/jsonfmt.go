package entryconf

import (
	"math"
	"sort"
	"strconv"
	"strings"
)

// formatJSON writes a JSON-shaped value in the normalized form of SPEC §10.8:
// two-space indentation, one member or element per line, keys sorted by code
// point, minimal string escaping, integers as integers, one trailing newline.
func formatJSON(v any) string {
	var b strings.Builder
	writeJSON(&b, v, 0)
	b.WriteByte('\n')
	return b.String()
}

func writeJSON(b *strings.Builder, v any, depth int) {
	switch t := v.(type) {
	case nil:
		b.WriteString("null")
	case bool:
		if t {
			b.WriteString("true")
		} else {
			b.WriteString("false")
		}
	case string:
		writeJSONString(b, t)
	case int64:
		b.WriteString(strconv.FormatInt(t, 10))
	case float64:
		writeJSONNumber(b, t)
	case map[string]any:
		if len(t) == 0 {
			b.WriteString("{}")
			return
		}
		keys := make([]string, 0, len(t))
		for k := range t {
			keys = append(keys, k)
		}
		sort.Strings(keys) // Go compares strings bytewise = by code point for UTF-8
		b.WriteString("{\n")
		for i, k := range keys {
			indent(b, depth+1)
			writeJSONString(b, k)
			b.WriteString(": ")
			writeJSON(b, t[k], depth+1)
			if i+1 < len(keys) {
				b.WriteByte(',')
			}
			b.WriteByte('\n')
		}
		indent(b, depth)
		b.WriteByte('}')
	case []any:
		if len(t) == 0 {
			b.WriteString("[]")
			return
		}
		b.WriteString("[\n")
		for i, item := range t {
			indent(b, depth+1)
			writeJSON(b, item, depth+1)
			if i+1 < len(t) {
				b.WriteByte(',')
			}
			b.WriteByte('\n')
		}
		indent(b, depth)
		b.WriteByte(']')
	default:
		// Unreachable: values are normalized to the shapes above before use.
		b.WriteString("null")
	}
}

func indent(b *strings.Builder, depth int) {
	for i := 0; i < depth; i++ {
		b.WriteString("  ")
	}
}

// writeJSONNumber renders a float: integral values below 2^53 as plain
// integers, everything else in the shortest round-trip form (SPEC §10.8).
func writeJSONNumber(b *strings.Builder, f float64) {
	if f == math.Trunc(f) && math.Abs(f) < 1<<53 {
		b.WriteString(strconv.FormatInt(int64(f), 10))
		return
	}
	b.WriteString(strconv.FormatFloat(f, 'g', -1, 64))
}

func writeJSONString(b *strings.Builder, s string) {
	const hex = "0123456789abcdef"
	b.WriteByte('"')
	for i := 0; i < len(s); i++ {
		c := s[i]
		switch c {
		case '"':
			b.WriteString(`\"`)
		case '\\':
			b.WriteString(`\\`)
		case '\b':
			b.WriteString(`\b`)
		case '\f':
			b.WriteString(`\f`)
		case '\n':
			b.WriteString(`\n`)
		case '\r':
			b.WriteString(`\r`)
		case '\t':
			b.WriteString(`\t`)
		default:
			if c < 0x20 {
				b.WriteString(`\u00`)
				b.WriteByte(hex[c>>4])
				b.WriteByte(hex[c&0xf])
			} else {
				b.WriteByte(c) // UTF-8 bytes pass through literally
			}
		}
	}
	b.WriteByte('"')
}
