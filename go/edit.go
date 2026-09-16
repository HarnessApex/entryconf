package entryconf

import (
	"crypto/sha256"
	"encoding/hex"
	"math"
	"os"
	"path/filepath"
	"reflect"
	"sort"
	"strconv"
	"strings"
	"time"
)

// Snapshot is what Open returns (SPEC §10.2): the effective tree, every source
// document the load read, and the process environment as captured at open
// time. It is immutable and reads nothing from disk after Open returns; edits
// are prepared against it with Plan and written with Plan.Commit.
type Snapshot struct {
	// Dir is the absolute path of the config directory.
	Dir string `json:"dir"`
	// Tree is the effective tree — exactly what Load(Dir) returns.
	Tree map[string]any `json:"tree"`
	// Documents holds every document the load read, keyed by document key
	// (SPEC §10.2): the entrypoint, each @file: target, each *.env file.
	Documents map[string]*Document `json:"documents"`

	byPath   map[string]*docRecord // absolute path -> record
	envNames []string
	captured map[string]string // process environment at open time
	live     envSource         // the live process environment, for Commit
}

// Document describes one source file of a snapshot (SPEC §10.2).
type Document struct {
	Key      string  `json:"key"`
	Path     string  `json:"path"`
	Format   string  `json:"format"` // "json", "yaml", "toml", or "env"
	Revision string  `json:"revision"`
	Writable bool    `json:"writable"` // true iff Format == "json"
	Grafts   []Graft `json:"grafts"`
}

// Graft is one position the document's root value occupies in the effective
// tree, with the @file: references traversed to reach it (SPEC §10.3).
type Graft struct {
	Effective string      `json:"effective"`
	Chain     []Reference `json:"chain"`
}

// Reference locates an @file: string: the document holding it and the
// string's pointer within that document.
type Reference struct {
	Document string `json:"document"`
	Pointer  string `json:"pointer"`
}

// Origin is the provenance of one effective value (SPEC §10.3).
type Origin struct {
	Effective string      `json:"effective"`
	Document  string      `json:"document"`
	Pointer   string      `json:"pointer"`
	Authored  any         `json:"authored"`
	Chain     []Reference `json:"chain"`
	Variables []Variable  `json:"variables"`
	Writable  bool        `json:"writable"`
}

// Variable is one $ reference an authored string makes and which layer of
// SPEC §4 supplies its value: "process", "file" (File is the *.env document
// key), or "default".
type Variable struct {
	Name   string `json:"name"`
	Origin string `json:"origin"`
	File   string `json:"file,omitempty"`
}

// Edit is one operation on one source document (SPEC §10.4).
type Edit struct {
	// Document is the key of the document to edit. Required.
	Document string `json:"document"`
	// Op is "set" or "remove".
	Op string `json:"op"`
	// Pointer is an RFC 6901 pointer within Document; "" is the root.
	Pointer string `json:"pointer"`
	// Value is the value to set: nil, bool, string, int/float, []any, or
	// map[string]any (other Go integer, float, slice and string-keyed map
	// types are accepted and normalized). Ignored for "remove".
	Value any `json:"value"`
	// Mode is "literal" (the default: strings are escaped so they load back
	// as themselves) or "expression" (a string written verbatim as an
	// entryconf expression).
	Mode string `json:"mode,omitempty"`
}

// Edit modes and ops.
const (
	OpSet          = "set"
	OpRemove       = "remove"
	ModeLiteral    = "literal"
	ModeExpression = "expression"
)

// Plan is a prepared, validated change to one document (SPEC §10.6). Nothing
// has been written; Commit writes After if every dependency is unchanged.
type Plan struct {
	Document          string            `json:"document"`
	Before            string            `json:"before"`
	After             string            `json:"after"`
	Candidate         map[string]any    `json:"candidate"`
	Affected          []string          `json:"affected"`
	Grafts            []Graft           `json:"grafts"`
	Revisions         map[string]string `json:"revisions"`
	Variables         []string          `json:"variables"`
	VariablesRevision string            `json:"variables_revision"`

	dir   string
	path  string            // absolute path of the edited document
	paths map[string]string // document key -> absolute path
	live  envSource
}

// Receipt is what a successful Commit returns.
type Receipt struct {
	Document  string            `json:"document"`
	Revision  string            `json:"revision"`
	Revisions map[string]string `json:"revisions"`
}

// CommitOptions tunes Commit. A nil *CommitOptions means the defaults.
type CommitOptions struct {
	// LockTimeout bounds the wait for the directory lock; zero means
	// DefaultLockTimeout.
	LockTimeout time.Duration
}

// Open loads dir like Load and returns a Snapshot for inspecting provenance
// and preparing edits (SPEC §10.2). The process environment is captured now;
// plans resolve variables against that capture, and Commit fails with
// E_STALE_PLAN if a variable the candidate depends on has since changed.
func Open(dir string) (*Snapshot, error) {
	captured := map[string]string{}
	for _, kv := range os.Environ() {
		if i := strings.IndexByte(kv, '='); i > 0 {
			captured[kv[:i]] = kv[i+1:]
		}
	}
	return open(dir, captured, os.LookupEnv)
}

// open is the seam the editing harness uses: captured is the environment as
// of the snapshot, live is what Commit consults when rechecking.
func open(dir string, captured map[string]string, live envSource) (*Snapshot, error) {
	abs, err := filepath.Abs(dir)
	if err != nil {
		return nil, wrapf(CodeNoEntrypoint, err, "cannot resolve %q", dir)
	}
	envNames, _ := osSource{}.envFileNames(abs) // a failure surfaces from the load itself
	sort.Strings(envNames)
	rec := newRecorder()
	l := &loader{src: osSource{}, procEnv: mapEnv(captured), rec: rec}
	tree, err := l.loadTree(abs)
	if err != nil {
		return nil, asError(err)
	}
	s := &Snapshot{
		Dir:       abs,
		Tree:      tree,
		Documents: map[string]*Document{},
		byPath:    rec.docs,
		envNames:  envNames,
		captured:  captured,
		live:      live,
	}
	for _, path := range rec.order {
		d := rec.docs[path]
		key := documentKey(abs, path)
		// The resolver records references by absolute path; documents are
		// identified by key everywhere the API is concerned.
		grafts := sortedGrafts(d.grafts)
		for i := range grafts {
			chain := make([]Reference, len(grafts[i].Chain))
			for j, ref := range grafts[i].Chain {
				chain[j] = Reference{Document: documentKey(abs, ref.Document), Pointer: ref.Pointer}
			}
			grafts[i].Chain = chain
		}
		s.Documents[key] = &Document{
			Key:      key,
			Path:     path,
			Format:   d.format,
			Revision: revisionOf(d.data),
			Writable: d.format == "json",
			Grafts:   grafts,
		}
	}
	return s, nil
}

func mapEnv(m map[string]string) envSource {
	return func(name string) (string, bool) {
		v, ok := m[name]
		return v, ok
	}
}

// revisionOf is SPEC §10.2's revision: "sha256:" + hex(SHA-256(bytes)).
func revisionOf(data []byte) string {
	sum := sha256.Sum256(data)
	return "sha256:" + hex.EncodeToString(sum[:])
}

// Inspect reports where the effective value at pointer comes from (SPEC §10.3):
// the document and source pointer that author it, the authored value, the
// include chain that reaches it, and the variables it depends on. A pointer
// that does not resolve in the effective tree is E_PATH.
func (s *Snapshot) Inspect(pointer string) (*Origin, error) {
	tokens, err := parsePointer(pointer)
	if err != nil {
		return nil, err
	}
	entrypoint, err := s.entrypointPath()
	if err != nil {
		return nil, err
	}
	doc := s.byPath[entrypoint]
	node := doc.parsed
	src := ""
	chain := []Reference{}

	follow := func() error {
		for {
			str, ok := node.(string)
			if !ok || !isInclude(str) {
				return nil
			}
			target := includeTarget(str, filepath.Dir(doc.path))
			next, ok := s.byPath[target]
			if !ok {
				return errf(CodePath, "include %q was not loaded by this snapshot", target)
			}
			chain = append(chain, Reference{Document: documentKey(s.Dir, doc.path), Pointer: src})
			doc, node, src = next, next.parsed, ""
		}
	}
	for _, tok := range tokens {
		if err := follow(); err != nil {
			return nil, err
		}
		switch c := node.(type) {
		case map[string]any:
			child, ok := c[tok]
			if !ok {
				return nil, errf(CodePath, "effective pointer %q: no member %q", pointer, tok)
			}
			node = child
		case []any:
			i, ok := arrayIndex(tok)
			if !ok || i >= len(c) {
				return nil, errf(CodePath, "effective pointer %q: array index %q does not exist", pointer, tok)
			}
			node = c[i]
		default:
			return nil, errf(CodePath, "effective pointer %q: cannot descend into a scalar at %q", pointer, tok)
		}
		src += "/" + escapePointerToken(tok)
	}
	if err := follow(); err != nil {
		return nil, err
	}

	key := documentKey(s.Dir, doc.path)
	origin := &Origin{
		Effective: pointer,
		Document:  key,
		Pointer:   src,
		Authored:  deepCopy(node),
		Chain:     chain,
		Variables: []Variable{},
		Writable:  s.Documents[key].Writable,
	}
	if str, ok := node.(string); ok {
		origin.Variables = s.variablesOf(str)
	}
	return origin, nil
}

// variablesOf lists the $ references of an authored string in order of
// appearance, with the SPEC §4 layer that supplies each (SPEC §10.3).
func (s *Snapshot) variablesOf(authored string) []Variable {
	out := []Variable{}
	fileVars, fileOrigin, err := (&loader{src: s.source(nil), procEnv: mapEnv(s.captured)}).loadEnvFiles(s.Dir)
	if err != nil {
		return out // the snapshot loaded, so this cannot happen
	}
	v := &vars{files: fileVars, origin: fileOrigin, proc: mapEnv(s.captured), used: map[string]bool{}}
	for _, ref := range scanReferences(authored) {
		where, file := v.where(ref.name)
		switch {
		case where == "process":
			out = append(out, Variable{Name: ref.name, Origin: "process"})
		case where == "file":
			out = append(out, Variable{Name: ref.name, Origin: "file", File: documentKey(s.Dir, file)})
		case ref.hasDefault:
			out = append(out, Variable{Name: ref.name, Origin: "default"})
		default:
			out = append(out, Variable{Name: ref.name, Origin: "unset"}) // unreachable: the load would have failed
		}
	}
	return out
}

func (s *Snapshot) entrypointPath() (string, error) {
	for _, name := range entrypointNames {
		if d, ok := s.byPath[filepath.Join(s.Dir, name)]; ok && d.parsed != nil {
			return d.path, nil
		}
	}
	return "", errf(CodeNoEntrypoint, "snapshot of %q has no entrypoint", s.Dir)
}

// source builds the overlay a candidate is evaluated against: the snapshot's
// bytes, with overrides replacing documents in memory, and disk for anything
// the snapshot never saw.
func (s *Snapshot) source(overrides map[string][]byte) fileSource {
	files := make(map[string][]byte, len(s.byPath)+len(overrides))
	for path, d := range s.byPath {
		files[path] = d.data
	}
	for path, data := range overrides {
		files[path] = data
	}
	return &overlaySource{files: files, envNames: s.envNames, fallback: osSource{}}
}

// Plan validates edits (SPEC §10.4), applies them to a copy of the selected
// document (SPEC §10.5), and evaluates the candidate configuration with the
// new text substituted in memory (SPEC §10.6). No file is changed. A candidate
// that fails to load fails the plan with that load's code.
func (s *Snapshot) Plan(edits []Edit) (*Plan, error) {
	if len(edits) == 0 {
		return nil, errf(CodeEdit, "a plan needs at least one edit")
	}
	key := edits[0].Document
	for _, e := range edits {
		if e.Document != key {
			return nil, errf(CodeUnsupportedEdit, "edits name both %q and %q; a plan edits exactly one document", key, e.Document)
		}
	}
	doc, ok := s.Documents[key]
	if !ok {
		return nil, errf(CodeEdit, "no document %q in the snapshot of %q", key, s.Dir)
	}
	if !doc.Writable {
		return nil, errf(CodeUnsupportedEdit, "document %q is %s; only JSON documents are writable", key, doc.Format)
	}
	rec := s.byPath[doc.Path]

	// Apply the edits to a copy of the parsed document.
	value := deepCopy(rec.parsed)
	for i, e := range edits {
		switch e.Op {
		case OpSet:
			v, err := normalizeValue(e.Value)
			if err != nil {
				return nil, wrapf(CodeEdit, err, "edit %d", i)
			}
			switch e.Mode {
			case "", ModeLiteral:
				v = escapeLiteral(v)
			case ModeExpression:
				if _, isStr := v.(string); !isStr {
					return nil, errf(CodeEdit, "edit %d: expression mode requires a string value", i)
				}
			default:
				return nil, errf(CodeEdit, "edit %d: unknown mode %q", i, e.Mode)
			}
			value, err = setAt(value, e.Pointer, v)
			if err != nil {
				return nil, err
			}
		case OpRemove:
			var err error
			value, err = removeAt(value, e.Pointer)
			if err != nil {
				return nil, err
			}
		default:
			return nil, errf(CodeEdit, "edit %d: unknown op %q (want %q or %q)", i, e.Op, OpSet, OpRemove)
		}
	}
	after := formatJSON(value)

	// Evaluate the candidate against the snapshot with the new text in place.
	candRec := newRecorder()
	l := &loader{
		src:     s.source(map[string][]byte{doc.Path: []byte(after)}),
		procEnv: mapEnv(s.captured),
		rec:     candRec,
	}
	candidate, err := l.loadTree(s.Dir)
	if err != nil {
		return nil, asError(err)
	}

	p := &Plan{
		Document:  key,
		Before:    string(rec.data),
		After:     after,
		Candidate: candidate,
		Affected:  affectedPointers(s.Tree, candidate),
		Grafts:    doc.Grafts,
		Revisions: map[string]string{},
		dir:       s.Dir,
		path:      doc.Path,
		paths:     map[string]string{},
		live:      s.live,
	}
	for _, path := range candRec.order {
		d := candRec.docs[path]
		k := documentKey(s.Dir, path)
		p.Revisions[k] = revisionOf(d.data)
		p.paths[k] = path
	}
	// The edited document's own revision is what is on disk now, not the new
	// text: Commit must find the file as the snapshot saw it.
	p.Revisions[key] = doc.Revision
	for name := range l.vars.used {
		p.Variables = append(p.Variables, name)
	}
	sort.Strings(p.Variables)
	p.VariablesRevision = variablesRevision(p.Variables, l.vars)
	return p, nil
}

// variablesRevision is SPEC §10.6's fingerprint over the named variables.
func variablesRevision(names []string, v *vars) string {
	var b strings.Builder
	for _, name := range names {
		if val, ok := v.lookup(name); ok {
			b.WriteString("=" + name + "=" + val + "\n")
		} else {
			b.WriteString("-" + name + "\n")
		}
	}
	return revisionOf([]byte(b.String()))
}

// Commit writes the plan's After to its document (SPEC §10.7): lock, recheck
// every dependency's revision and the variables fingerprint (E_STALE_PLAN on
// any change), write atomically (E_WRITE on failure, target untouched), unlock.
func (p *Plan) Commit(opts *CommitOptions) (*Receipt, error) {
	timeout := DefaultLockTimeout
	if opts != nil && opts.LockTimeout > 0 {
		timeout = opts.LockTimeout
	}
	release, err := acquireLock(p.dir, timeout)
	if err != nil {
		return nil, err
	}
	defer release()

	// Recheck documents.
	fileVars := map[string]string{}
	fileOrigin := map[string]string{}
	for key, want := range p.Revisions {
		data, err := os.ReadFile(p.paths[key])
		if err != nil {
			return nil, wrapf(CodeStalePlan, err, "document %q can no longer be read", key)
		}
		if got := revisionOf(data); got != want {
			return nil, errf(CodeStalePlan, "document %q changed since the plan was made (%s, plan expected %s)", key, got, want)
		}
		if strings.HasSuffix(key, ".env") {
			pairs, err := parseEnvFile(p.paths[key], string(data))
			if err != nil {
				return nil, wrapf(CodeStalePlan, err, "document %q", key)
			}
			for _, kv := range pairs {
				fileVars[kv.name] = kv.value
				fileOrigin[kv.name] = p.paths[key]
			}
		}
	}
	// Recheck variables against the live environment.
	v := &vars{files: fileVars, origin: fileOrigin, proc: p.live, used: map[string]bool{}}
	if got := variablesRevision(p.Variables, v); got != p.VariablesRevision {
		return nil, errf(CodeStalePlan, "a variable the candidate depends on changed since the plan was made (%v)", p.Variables)
	}

	if err := atomicWrite(p.path, []byte(p.After)); err != nil {
		return nil, err
	}
	revisions := make(map[string]string, len(p.Revisions))
	for k, r := range p.Revisions {
		revisions[k] = r
	}
	revisions[p.Document] = revisionOf([]byte(p.After))
	return &Receipt{Document: p.Document, Revision: revisions[p.Document], Revisions: revisions}, nil
}

// escapeLiteral makes a value load back as itself (SPEC §10.5 literal mode):
// every string, recursively but never an object key, has each "$" doubled and
// then a leading "@" doubled.
func escapeLiteral(v any) any {
	switch t := v.(type) {
	case string:
		s := strings.ReplaceAll(t, "$", "$$")
		if strings.HasPrefix(s, "@") {
			s = "@" + s
		}
		return s
	case map[string]any:
		out := make(map[string]any, len(t))
		for k, val := range t {
			out[k] = escapeLiteral(val)
		}
		return out
	case []any:
		out := make([]any, len(t))
		for i, val := range t {
			out[i] = escapeLiteral(val)
		}
		return out
	default:
		return v
	}
}

// normalizeValue converts a caller-supplied value into the loader's JSON
// shapes (nil, bool, string, int64, float64, []any, map[string]any), or
// reports E_EDIT for anything with no JSON-equivalent form.
func normalizeValue(v any) (any, error) {
	switch t := v.(type) {
	case nil, bool, string, int64, map[string]any, []any:
		switch c := t.(type) {
		case map[string]any:
			out := make(map[string]any, len(c))
			for k, val := range c {
				nv, err := normalizeValue(val)
				if err != nil {
					return nil, err
				}
				out[k] = nv
			}
			return out, nil
		case []any:
			out := make([]any, len(c))
			for i, val := range c {
				nv, err := normalizeValue(val)
				if err != nil {
					return nil, err
				}
				out[i] = nv
			}
			return out, nil
		}
		return t, nil
	case float64:
		if math.IsInf(t, 0) || math.IsNaN(t) {
			return nil, errf(CodeEdit, "number %v has no JSON-equivalent form", t)
		}
		return t, nil
	case float32:
		return normalizeValue(float64(t))
	case int:
		return int64(t), nil
	case int8:
		return int64(t), nil
	case int16:
		return int64(t), nil
	case int32:
		return int64(t), nil
	case uint8:
		return int64(t), nil
	case uint16:
		return int64(t), nil
	case uint32:
		return int64(t), nil
	case uint, uint64, uintptr:
		u := reflect.ValueOf(t).Uint()
		if u > math.MaxInt64 {
			return float64(u), nil
		}
		return int64(u), nil
	}
	rv := reflect.ValueOf(v)
	switch rv.Kind() {
	case reflect.Slice, reflect.Array:
		out := make([]any, rv.Len())
		for i := range out {
			nv, err := normalizeValue(rv.Index(i).Interface())
			if err != nil {
				return nil, err
			}
			out[i] = nv
		}
		return out, nil
	case reflect.Map:
		if rv.Type().Key().Kind() != reflect.String {
			return nil, errf(CodeEdit, "map keys must be strings, not %s", rv.Type().Key())
		}
		out := make(map[string]any, rv.Len())
		iter := rv.MapRange()
		for iter.Next() {
			nv, err := normalizeValue(iter.Value().Interface())
			if err != nil {
				return nil, err
			}
			out[iter.Key().String()] = nv
		}
		return out, nil
	case reflect.Pointer, reflect.Interface:
		if rv.IsNil() {
			return nil, nil
		}
		return normalizeValue(rv.Elem().Interface())
	}
	return nil, errf(CodeEdit, "value of type %T has no JSON-equivalent form", v)
}

// affectedPointers is SPEC §10.6's diff: the shallowest effective pointers at
// which before and after differ, sorted by code point.
func affectedPointers(before, after any) []string {
	out := []string{}
	diffInto(before, after, "", &out)
	sort.Strings(out)
	return out
}

func diffInto(a, b any, ptr string, out *[]string) {
	switch x := a.(type) {
	case map[string]any:
		y, ok := b.(map[string]any)
		if !ok {
			*out = append(*out, ptr)
			return
		}
		keys := map[string]bool{}
		for k := range x {
			keys[k] = true
		}
		for k := range y {
			keys[k] = true
		}
		for k := range keys {
			av, inA := x[k]
			bv, inB := y[k]
			child := ptr + "/" + escapePointerToken(k)
			if !inA || !inB {
				*out = append(*out, child)
				continue
			}
			diffInto(av, bv, child, out)
		}
	case []any:
		y, ok := b.([]any)
		if !ok || len(x) != len(y) {
			*out = append(*out, ptr)
			return
		}
		for i := range x {
			diffInto(x[i], y[i], ptr+"/"+escapePointerToken(itoa(i)), out)
		}
	default:
		if !scalarEqual(a, b) {
			*out = append(*out, ptr)
		}
	}
}

func scalarEqual(a, b any) bool {
	if af, ok := asNumber(a); ok {
		bf, ok := asNumber(b)
		return ok && af == bf
	}
	switch b.(type) {
	case map[string]any, []any:
		return false
	}
	if _, isNum := asNumber(b); isNum {
		return false
	}
	return a == b
}

func asNumber(v any) (float64, bool) {
	switch n := v.(type) {
	case int64:
		return float64(n), true
	case float64:
		return n, true
	}
	return 0, false
}

func itoa(i int) string { return strconv.Itoa(i) }
