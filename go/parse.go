package entryconf

import (
	"bytes"
	"encoding/json"
	"errors"
	"io"
	"math"
	"os"
	"path/filepath"
	"regexp"
	"strconv"
	"strings"
	"time"
	"unicode/utf8"

	toml "github.com/pelletier/go-toml/v2"
	yaml "gopkg.in/yaml.v3"
)

// parserFor maps a file extension to a parser (SPEC §5). Extensions are
// matched case-sensitively in their lowercase spelling, so ".JSON" is not a
// config document.
func parserFor(path string) (func(path string, data []byte) (any, error), bool) {
	switch filepath.Ext(path) {
	case ".json":
		return parseJSON, true
	case ".yaml", ".yml":
		return parseYAML, true
	case ".toml":
		return parseTOML, true
	}
	return nil, false
}

// parseDocument reads and parses a document. A missing or unreadable file is
// reported by the caller (which knows whether it is an entrypoint or an
// include); an unparseable one — including one whose bytes are not valid
// UTF-8 (SPEC §2) — is always E_PARSE.
func parseDocument(path string) (any, error) {
	parse, ok := parserFor(path)
	if !ok {
		return nil, errf(CodeInclude, "unsupported file extension: %q", path)
	}
	data, err := os.ReadFile(path)
	if err != nil {
		return nil, err // classified by the caller
	}
	if err := checkUTF8(path, data); err != nil {
		return nil, err
	}
	return parse(path, data)
}

// checkUTF8 enforces SPEC §2: file content anywhere that is not valid UTF-8 is
// E_PARSE. Stock parsers differ on whether they reject invalid bytes, replace
// them with U+FFFD, or pass them through, so the check is made up front.
func checkUTF8(path string, data []byte) error {
	if utf8.Valid(data) {
		return nil
	}
	return errf(CodeParse, "%s: content is not valid UTF-8", path)
}

// ---------------------------------------------------------------- JSON

// parseJSON walks the token stream instead of unmarshaling, because
// encoding/json silently keeps the last of a set of duplicate keys and SPEC §2
// requires E_PARSE. Numbers are decoded exactly (int64 when integral).
func parseJSON(path string, data []byte) (any, error) {
	dec := json.NewDecoder(bytes.NewReader(data))
	dec.UseNumber()
	v, err := jsonValue(dec, path)
	if err != nil {
		return nil, err
	}
	if _, err := dec.Token(); !errors.Is(err, io.EOF) {
		return nil, errf(CodeParse, "%s: trailing content after top-level JSON value", path)
	}
	return v, nil
}

func jsonValue(dec *json.Decoder, path string) (any, error) {
	tok, err := dec.Token()
	if err != nil {
		return nil, wrapf(CodeParse, err, "%s: invalid JSON", path)
	}
	return jsonValueFrom(dec, tok, path)
}

func jsonValueFrom(dec *json.Decoder, tok json.Token, path string) (any, error) {
	switch t := tok.(type) {
	case json.Delim:
		switch t {
		case '{':
			obj := make(map[string]any)
			for {
				keyTok, err := dec.Token()
				if err != nil {
					return nil, wrapf(CodeParse, err, "%s: invalid JSON", path)
				}
				if d, ok := keyTok.(json.Delim); ok && d == '}' {
					return obj, nil
				}
				key, ok := keyTok.(string)
				if !ok {
					return nil, errf(CodeParse, "%s: invalid JSON object key", path)
				}
				if _, dup := obj[key]; dup {
					return nil, errf(CodeParse, "%s: duplicate key %q in JSON object", path, key)
				}
				val, err := jsonValue(dec, path)
				if err != nil {
					return nil, err
				}
				obj[key] = val
			}
		case '[':
			arr := []any{}
			for {
				elemTok, err := dec.Token()
				if err != nil {
					return nil, wrapf(CodeParse, err, "%s: invalid JSON", path)
				}
				if d, ok := elemTok.(json.Delim); ok && d == ']' {
					return arr, nil
				}
				val, err := jsonValueFrom(dec, elemTok, path)
				if err != nil {
					return nil, err
				}
				arr = append(arr, val)
			}
		default:
			return nil, errf(CodeParse, "%s: unexpected %q in JSON", path, t)
		}
	case json.Number:
		n, err := numberFromString(t.String())
		if err != nil {
			return nil, errf(CodeParse, "%s: number %q has no JSON-equivalent value: %s", path, t.String(), err)
		}
		return n, nil
	case string, bool, nil:
		return t, nil
	default:
		return nil, errf(CodeParse, "%s: unexpected JSON token", path)
	}
}

// numberFromString keeps integers as int64 (no float64 rounding) and
// everything else as float64. A literal that does not land on a finite double
// has no value in the data model of SPEC §2 and is rejected: the tree stays
// JSON-encodable, and whole-value typing (SPEC §6) leaves such text a string.
func numberFromString(s string) (any, error) {
	if i, err := strconv.ParseInt(s, 10, 64); err == nil {
		return i, nil
	}
	f, err := strconv.ParseFloat(s, 64)
	if err != nil && !errors.Is(err, strconv.ErrRange) {
		return nil, err
	}
	if math.IsInf(f, 0) || math.IsNaN(f) {
		return nil, errors.New("out of IEEE-754 double range")
	}
	return f, nil
}

// ---------------------------------------------------------------- YAML

// parseYAML decodes the stream document by document: SPEC §2 allows at most
// one document per file, an empty document parses as null, and a
// multi-document stream is E_PARSE.
func parseYAML(path string, data []byte) (any, error) {
	dec := yaml.NewDecoder(bytes.NewReader(data))
	var doc *yaml.Node
	for {
		var node yaml.Node
		err := dec.Decode(&node)
		if errors.Is(err, io.EOF) {
			break
		}
		if err != nil {
			return nil, wrapf(CodeParse, err, "%s: invalid YAML", path)
		}
		if doc != nil {
			return nil, errf(CodeParse, "%s: multi-document YAML stream (at most one document per file)", path)
		}
		first := node
		doc = &first
	}
	if doc == nil || doc.Kind == 0 {
		return nil, nil // empty document (SPEC §2)
	}
	c := &yamlConv{path: path, active: map[*yaml.Node]bool{}}
	return c.value(doc)
}

// maxYAMLNodes is the alias-expansion budget of SPEC §2: a document whose
// fully expanded tree would exceed this many nodes is E_PARSE.
const maxYAMLNodes = 1_000_000

// yamlConv converts one YAML document.
//
// active holds the nodes on the current path, so an alias that reaches an
// ancestor is reported as the cycle SPEC §2 requires. nodes is the expansion
// budget: it counts the nodes actually produced — each scalar value, sequence
// element and mapping entry — as they are built, so an alias bomb (a layered
// anchor graph whose expansion is exponential in the source size) is rejected
// after a bounded amount of work rather than materialized. This is a real
// budget on output size, not a limit on depth or on the number of alias
// references: heavy but honest reuse expands to few nodes and loads.
type yamlConv struct {
	path   string
	active map[*yaml.Node]bool
	nodes  int
}

// charge accounts for n nodes about to be produced and fails once the
// document's expansion passes the budget.
func (c *yamlConv) charge(n int, at *yaml.Node) error {
	c.nodes += n
	if c.nodes > maxYAMLNodes {
		return errf(CodeParse,
			"%s:%d: expanded YAML tree exceeds the %d-node limit (alias expansion is bounded)",
			c.path, at.Line, maxYAMLNodes)
	}
	return nil
}

// yamlCoreTags is the tag set of the YAML 1.2 core schema. SPEC §2 makes any
// other tag (including 1.1 type-library tags such as !!timestamp) E_PARSE.
var yamlCoreTags = map[string]bool{
	"!!str": true, "!!int": true, "!!float": true, "!!bool": true,
	"!!null": true, "!!map": true, "!!seq": true,
}

// value converts one node, charging the expansion budget for every node it
// produces. An alias is expanded through Node.Alias, so the budget — not a
// depth heuristic — is what stops an exponential anchor graph.
func (c *yamlConv) value(n *yaml.Node) (any, error) {
	if c.active[n] {
		return nil, errf(CodeParse, "%s:%d: cyclic YAML alias", c.path, n.Line)
	}
	c.active[n] = true
	defer delete(c.active, n)

	switch n.Kind {
	case yaml.DocumentNode:
		if len(n.Content) == 0 {
			return nil, nil
		}
		return c.value(n.Content[0])
	case yaml.AliasNode:
		// SPEC §2: anchors and aliases are resolved at parse time and produce
		// plain values. The alias itself is not a node of the expanded tree —
		// what it expands to is, and that is charged below.
		if n.Alias == nil {
			return nil, errf(CodeParse, "%s: unresolved alias %q", c.path, n.Value)
		}
		return c.value(n.Alias)
	case yaml.MappingNode:
		if err := checkYAMLTag(n, "!!map", c.path); err != nil {
			return nil, err
		}
		obj := make(map[string]any, len(n.Content)/2)
		for i := 0; i+1 < len(n.Content); i += 2 {
			keyNode, valNode := n.Content[i], n.Content[i+1]
			if err := c.charge(1, keyNode); err != nil { // one mapping entry
				return nil, err
			}
			key, err := c.value(keyNode)
			if err != nil {
				return nil, err
			}
			ks, ok := key.(string)
			if !ok {
				// The tree is JSON-equivalent (SPEC §2): object keys are strings.
				return nil, errf(CodeParse, "%s:%d: non-string mapping key", c.path, keyNode.Line)
			}
			// SPEC §2: mapping keys are not counted — refund the scalar
			// charge value() just took for the key.
			c.nodes--
			if _, dup := obj[ks]; dup {
				return nil, errf(CodeParse, "%s:%d: duplicate key %q in mapping", c.path, keyNode.Line, ks)
			}
			v, err := c.value(valNode)
			if err != nil {
				return nil, err
			}
			obj[ks] = v
		}
		return obj, nil
	case yaml.SequenceNode:
		if err := checkYAMLTag(n, "!!seq", c.path); err != nil {
			return nil, err
		}
		if err := c.charge(len(n.Content), n); err != nil { // one per element
			return nil, err
		}
		arr := make([]any, 0, len(n.Content))
		for _, elem := range n.Content {
			v, err := c.value(elem)
			if err != nil {
				return nil, err
			}
			arr = append(arr, v)
		}
		return arr, nil
	case yaml.ScalarNode:
		if err := c.charge(1, n); err != nil { // one scalar value
			return nil, err
		}
		return yamlScalar(n, c.path)
	default:
		return nil, errf(CodeParse, "%s:%d: unsupported YAML node", c.path, n.Line)
	}
}

func checkYAMLTag(n *yaml.Node, want, path string) error {
	if n.Style&yaml.TaggedStyle == 0 {
		return nil // implicit tag, nothing was written in the document
	}
	if !yamlCoreTags[n.Tag] {
		return errf(CodeParse, "%s:%d: unsupported YAML tag %q", path, n.Line, n.Tag)
	}
	if n.Tag != want {
		return errf(CodeParse, "%s:%d: YAML tag %q does not apply to this node", path, n.Line, n.Tag)
	}
	return nil
}

// yamlScalar resolves a scalar under the YAML 1.2 core schema.
//
// yaml.v3 keeps several YAML 1.1 resolution rules (0777 as octal, 1_000 with
// underscore separators), so its own tag is deliberately ignored for plain
// scalars: the value text is re-resolved here with the core schema regexps.
// An explicit tag in the document is detectable via yaml.TaggedStyle and is
// honoured.
func yamlScalar(n *yaml.Node, path string) (any, error) {
	if n.Style&yaml.TaggedStyle != 0 {
		if !yamlCoreTags[n.Tag] {
			return nil, errf(CodeParse, "%s:%d: unsupported YAML tag %q", path, n.Line, n.Tag)
		}
		return yamlTagged(n, path)
	}
	// Quoted, literal and folded scalars are always strings.
	const quoted = yaml.DoubleQuotedStyle | yaml.SingleQuotedStyle | yaml.LiteralStyle | yaml.FoldedStyle
	if n.Style&quoted != 0 {
		return n.Value, nil
	}
	return checkFinite(yamlCoreResolve(n.Value), n, path)
}

func yamlTagged(n *yaml.Node, path string) (any, error) {
	switch n.Tag {
	case "!!str":
		return n.Value, nil
	case "!!null":
		return nil, nil
	case "!!bool":
		switch n.Value {
		case "true", "True", "TRUE":
			return true, nil
		case "false", "False", "FALSE":
			return false, nil
		}
		return nil, errf(CodeParse, "%s:%d: %q is not a core-schema boolean", path, n.Line, n.Value)
	case "!!int":
		if v, ok := yamlCoreResolve(n.Value).(int64); ok {
			return v, nil
		}
		return nil, errf(CodeParse, "%s:%d: %q is not a core-schema integer", path, n.Line, n.Value)
	case "!!float":
		switch v := yamlCoreResolve(n.Value).(type) {
		case float64:
			return checkFinite(v, n, path)
		case int64:
			return float64(v), nil
		}
		return nil, errf(CodeParse, "%s:%d: %q is not a core-schema float", path, n.Line, n.Value)
	}
	return nil, errf(CodeParse, "%s:%d: YAML tag %q does not apply to a scalar", path, n.Line, n.Tag)
}

// checkFinite rejects the core-schema floats that have no JSON-equivalent
// form: `.inf`, `.nan`, and any literal that overflows a double (SPEC §2).
func checkFinite(v any, n *yaml.Node, path string) (any, error) {
	if f, ok := v.(float64); ok && (math.IsInf(f, 0) || math.IsNaN(f)) {
		return nil, errf(CodeParse, "%s:%d: %q has no JSON-equivalent form", path, n.Line, n.Value)
	}
	return v, nil
}

// The YAML 1.2 core schema resolution regexps (spec §10.2.1.2).
var (
	yamlIntRe   = regexp.MustCompile(`^[-+]?[0-9]+$`)
	yamlOctRe   = regexp.MustCompile(`^0o[0-7]+$`)
	yamlHexRe   = regexp.MustCompile(`^0x[0-9a-fA-F]+$`)
	yamlFloatRe = regexp.MustCompile(`^[-+]?(\.[0-9]+|[0-9]+(\.[0-9]*)?)([eE][-+]?[0-9]+)?$`)
)

func yamlCoreResolve(v string) any {
	switch v {
	case "", "~", "null", "Null", "NULL":
		return nil
	case "true", "True", "TRUE":
		return true
	case "false", "False", "FALSE":
		return false
	case ".inf", ".Inf", ".INF", "+.inf", "+.Inf", "+.INF":
		return math.Inf(1)
	case "-.inf", "-.Inf", "-.INF":
		return math.Inf(-1)
	case ".nan", ".NaN", ".NAN":
		return math.NaN()
	}
	switch {
	case yamlIntRe.MatchString(v):
		if i, err := strconv.ParseInt(v, 10, 64); err == nil {
			return i
		}
		if f, ok := parseFloatOrInf(v); ok { // out of int64 range
			return f
		}
	case yamlOctRe.MatchString(v):
		if i, err := strconv.ParseInt(v[2:], 8, 64); err == nil {
			return i
		}
	case yamlHexRe.MatchString(v):
		if i, err := strconv.ParseInt(v[2:], 16, 64); err == nil {
			return i
		}
	case yamlFloatRe.MatchString(v):
		if f, ok := parseFloatOrInf(v); ok {
			return f
		}
	}
	return v
}

// parseFloatOrInf keeps an overflowing literal a *number* (±Inf) rather than
// silently demoting it to a string: the core schema resolved it as a number,
// and checkFinite then reports it as E_PARSE.
func parseFloatOrInf(v string) (float64, bool) {
	f, err := strconv.ParseFloat(v, 64)
	if err == nil || errors.Is(err, strconv.ErrRange) {
		return f, true
	}
	return 0, false
}

// ---------------------------------------------------------------- TOML

func parseTOML(path string, data []byte) (any, error) {
	var v any
	// go-toml/v2 reports duplicate keys and redefined tables as errors, which
	// is what SPEC §2 requires.
	if err := toml.Unmarshal(data, &v); err != nil {
		return nil, wrapf(CodeParse, err, "%s: invalid TOML", path)
	}
	return tomlNormalize(v, path)
}

// tomlNormalize converts the parts of go-toml's value model that are not
// JSON-equivalent: SPEC §2 requires datetimes to become RFC 3339-style
// strings, and TOML's `inf`/`nan` floats have no JSON form at all.
func tomlNormalize(v any, path string) (any, error) {
	switch t := v.(type) {
	case map[string]any:
		out := make(map[string]any, len(t))
		for k, val := range t {
			nv, err := tomlNormalize(val, path)
			if err != nil {
				return nil, err
			}
			out[k] = nv
		}
		return out, nil
	case []any:
		out := make([]any, len(t))
		for i, val := range t {
			nv, err := tomlNormalize(val, path)
			if err != nil {
				return nil, err
			}
			out[i] = nv
		}
		return out, nil
	case float64:
		if math.IsInf(t, 0) || math.IsNaN(t) {
			return nil, errf(CodeParse, "%s: %v has no JSON-equivalent form", path, t)
		}
		return t, nil
	case time.Time:
		// An offset date-time: uppercase T, a UTC offset written as "Z", any
		// other offset kept in its authored numeric form. RFC3339Nano also
		// drops trailing zeros from the fraction, and the "." with them.
		return t.Format(time.RFC3339Nano), nil
	case toml.LocalDateTime:
		return trimFraction(t.String()), nil
	case toml.LocalDate:
		return t.String(), nil
	case toml.LocalTime:
		return trimFraction(t.String()), nil
	}
	return v, nil
}

// trimFraction drops trailing zeros from a fractional-seconds group, and the
// "." along with them once the fraction reaches zero (SPEC §2). go-toml's
// local types render the fraction at the precision it was authored with.
func trimFraction(s string) string {
	dot := strings.IndexByte(s, '.')
	if dot < 0 {
		return s
	}
	end := dot + 1
	for end < len(s) && s[end] >= '0' && s[end] <= '9' {
		end++
	}
	frac := strings.TrimRight(s[dot+1:end], "0")
	if frac == "" {
		return s[:dot] + s[end:]
	}
	return s[:dot+1] + frac + s[end:]
}
