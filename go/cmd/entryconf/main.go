// Command entryconf loads an entryconf config directory and prints the
// resulting tree as JSON.
package main

import (
	"bytes"
	"encoding/json"
	"errors"
	"flag"
	"fmt"
	"io"
	"os"
	"time"

	entryconf "github.com/HarnessApex/entryconf/go"
)

// specVersion is the version of SPEC.md this build implements.
const specVersion = "0.3.0"

const usage = `entryconf — load a config directory into a single tree (entryconf spec ` + specVersion + `)

Usage:
  entryconf dump [flags] <dir>             load <dir> and print the resulting tree as JSON
  entryconf inspect <dir> [<pointer>]      show where an effective value comes from, or
                                           list the source documents when no pointer is given
  entryconf edit [flags] <dir> <request>   apply the edits in <request> (a JSON file, or - for
                                           stdin) to one source document; -n only previews
  entryconf help                           show this message
  entryconf version                        print the implemented spec version

Dump flags:
  -c, --compact                  print one line of JSON instead of indenting

Edit flags:
  -n, --dry-run                  plan and print the change, but do not write it
  --lock-timeout <duration>      how long to wait for the directory lock (default 5s)

A request is {"edits": [{"document": "app.json", "op": "set", "pointer": "/port",
"value": 8080}, ...]}: op is "set" or "remove", pointer is a JSON Pointer within
the named document, and an optional "mode" of "expression" writes a string
verbatim as an entryconf expression instead of as an escaped literal. Only JSON
documents are writable (SPEC §10). Output is the plan — before/after text,
candidate tree, affected effective pointers, the revisions it depends on — plus
"committed" and the new "revision".

A config directory holds exactly one entrypoint file (entrypoint.json, .yaml,
.yml or .toml), any number of *.env variable files, and any files reachable
through "@file:" references. Variables are read from the *.env files, with the
process environment taking precedence.

Exit status:
  0   the command succeeded; its result is on stdout
  1   the load or edit failed; the first line of stderr is the E_* error code
  2   any other fault (a wrong command line, or an internal error); no E_*
      code is printed, so an E_* code on stderr always means a rejected config
      or edit

Examples:
  entryconf dump ./envs/staging
  DB_PASSWORD=hunter2 entryconf dump ./envs/prod | jq .database
`

func main() {
	os.Exit(run(os.Args[1:], os.Stdout, os.Stderr))
}

func run(args []string, stdout, stderr io.Writer) int {
	if len(args) == 0 {
		fmt.Fprint(stderr, usage)
		return 2
	}
	switch args[0] {
	case "dump":
		return runDump(args[1:], stdout, stderr)
	case "inspect":
		return runInspect(args[1:], stdout, stderr)
	case "edit":
		return runEdit(args[1:], stdout, stderr)
	case "help", "-h", "--help":
		fmt.Fprint(stdout, usage)
		return 0
	case "version", "--version":
		fmt.Fprintf(stdout, "entryconf (Go) — implements entryconf spec %s\n", specVersion)
		return 0
	default:
		fmt.Fprintf(stderr, "entryconf: unknown command %q\n\n", args[0])
		fmt.Fprint(stderr, usage)
		return 2
	}
}

func runDump(args []string, stdout, stderr io.Writer) int {
	fs := flag.NewFlagSet("dump", flag.ContinueOnError)
	fs.SetOutput(stderr)
	fs.Usage = func() { fmt.Fprint(stderr, usage) }
	var compact bool
	fs.BoolVar(&compact, "compact", false, "print one line of JSON")
	fs.BoolVar(&compact, "c", false, "print one line of JSON (shorthand)")
	if err := fs.Parse(args); err != nil {
		return 2
	}
	if fs.NArg() != 1 {
		fmt.Fprintf(stderr, "entryconf: dump takes exactly one config directory\n\n")
		fmt.Fprint(stderr, usage)
		return 2
	}

	tree, err := entryconf.Load(fs.Arg(0))
	if err != nil {
		return reportError(err, stderr)
	}
	return writeJSON(tree, compact, stdout, stderr)
}

// reportError is the failure half of the exit convention. A load or edit
// failure, and only such a failure, prints a code: the first line of stderr
// is exactly the E_* code, so other tools can compare it across
// implementations. An uncoded error is an internal fault, not a verdict about
// the config, so it must not be given a code: exit 2.
func reportError(err error, stderr io.Writer) int {
	var ecErr *entryconf.Error
	if !errors.As(err, &ecErr) {
		// Unreachable: every failure the library returns carries a SPEC §7
		// or §10.9 code.
		fmt.Fprintf(stderr, "entryconf: internal error: %s\n", err)
		return 2
	}
	fmt.Fprintf(stderr, "%s\nentryconf: %s\n", ecErr.Code(), err)
	return 1
}

// writeJSON prints v as JSON. It encodes into a buffer first: a value that
// could not be rendered must not leave half a document on stdout for a caller
// to parse. Object keys come out sorted (encoding/json sorts map keys), so the
// output is comparable across implementations.
func writeJSON(v any, compact bool, stdout, stderr io.Writer) int {
	var buf bytes.Buffer
	enc := json.NewEncoder(&buf)
	if !compact {
		enc.SetIndent("", "  ")
	}
	enc.SetEscapeHTML(false)
	if err := enc.Encode(v); err != nil {
		// Unreachable: SPEC §2 makes the tree JSON-equivalent, and the
		// library rejects any value that is not. The operation already
		// succeeded, so this is an internal fault: exit 2 with no E_* code.
		fmt.Fprintf(stderr, "entryconf: internal error: cannot encode result as JSON: %s\n", err)
		return 2
	}
	if _, err := stdout.Write(buf.Bytes()); err != nil {
		fmt.Fprintf(stderr, "entryconf: cannot write output: %s\n", err)
		return 2
	}
	return 0
}

// runInspect prints the origin of one effective value (SPEC §10.3), or with
// no pointer the snapshot's documents — the way a caller finds out which
// source document authors a value before editing it.
func runInspect(args []string, stdout, stderr io.Writer) int {
	fs := flag.NewFlagSet("inspect", flag.ContinueOnError)
	fs.SetOutput(stderr)
	fs.Usage = func() { fmt.Fprint(stderr, usage) }
	if err := fs.Parse(args); err != nil {
		return 2
	}
	if fs.NArg() < 1 || fs.NArg() > 2 {
		fmt.Fprintf(stderr, "entryconf: inspect takes a config directory and an optional pointer\n\n")
		fmt.Fprint(stderr, usage)
		return 2
	}
	snap, err := entryconf.Open(fs.Arg(0))
	if err != nil {
		return reportError(err, stderr)
	}
	if fs.NArg() == 1 {
		return writeJSON(map[string]any{"dir": snap.Dir, "documents": snap.Documents}, false, stdout, stderr)
	}
	origin, err := snap.Inspect(fs.Arg(1))
	if err != nil {
		return reportError(err, stderr)
	}
	return writeJSON(origin, false, stdout, stderr)
}

// editRequest is the on-disk form of an edit request (SPEC §11 request.json).
type editRequest struct {
	Edits []entryconf.Edit `json:"edits"`
}

// editResult is what edit prints: the plan, and whether it was committed.
type editResult struct {
	*entryconf.Plan
	Committed bool    `json:"committed"`
	Revision  *string `json:"revision"`
}

// runEdit plans the edits in a request file against <dir> and, unless -n is
// given, commits them (SPEC §10.6–10.7). The library does all the work; this
// command only parses the request and prints the plan.
func runEdit(args []string, stdout, stderr io.Writer) int {
	fs := flag.NewFlagSet("edit", flag.ContinueOnError)
	fs.SetOutput(stderr)
	fs.Usage = func() { fmt.Fprint(stderr, usage) }
	var dryRun bool
	var lockTimeout time.Duration
	fs.BoolVar(&dryRun, "dry-run", false, "plan only")
	fs.BoolVar(&dryRun, "n", false, "plan only (shorthand)")
	fs.DurationVar(&lockTimeout, "lock-timeout", entryconf.DefaultLockTimeout, "lock wait")
	if err := fs.Parse(args); err != nil {
		return 2
	}
	if fs.NArg() != 2 {
		fmt.Fprintf(stderr, "entryconf: edit takes a config directory and a request file (or -)\n\n")
		fmt.Fprint(stderr, usage)
		return 2
	}
	var data []byte
	var err error
	if fs.Arg(1) == "-" {
		data, err = io.ReadAll(os.Stdin)
	} else {
		data, err = os.ReadFile(fs.Arg(1))
	}
	if err != nil {
		fmt.Fprintf(stderr, "entryconf: cannot read request: %s\n", err)
		return 2
	}
	var req editRequest
	dec := json.NewDecoder(bytes.NewReader(data))
	dec.UseNumber()
	if err := dec.Decode(&req); err != nil {
		fmt.Fprintf(stderr, "entryconf: request is not JSON: %s\n", err)
		return 2
	}
	for i := range req.Edits {
		req.Edits[i].Value = plainNumbers(req.Edits[i].Value)
	}

	snap, err := entryconf.Open(fs.Arg(0))
	if err != nil {
		return reportError(err, stderr)
	}
	plan, err := snap.Plan(req.Edits)
	if err != nil {
		return reportError(err, stderr)
	}
	result := editResult{Plan: plan}
	if !dryRun {
		receipt, err := plan.Commit(&entryconf.CommitOptions{LockTimeout: lockTimeout})
		if err != nil {
			return reportError(err, stderr)
		}
		result.Committed = true
		result.Revision = &receipt.Revision
	}
	return writeJSON(result, false, stdout, stderr)
}

// plainNumbers turns the json.Number values UseNumber produced into the
// library's int64/float64 shapes, keeping integers exact.
func plainNumbers(v any) any {
	switch t := v.(type) {
	case json.Number:
		if i, err := t.Int64(); err == nil {
			return i
		}
		f, _ := t.Float64()
		return f
	case map[string]any:
		for k, val := range t {
			t[k] = plainNumbers(val)
		}
		return t
	case []any:
		for i, val := range t {
			t[i] = plainNumbers(val)
		}
		return t
	}
	return v
}
