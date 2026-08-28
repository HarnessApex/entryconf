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

	entryconf "github.com/HarnessApex/entryconf/go"
)

// specVersion is the version of SPEC.md this build implements.
const specVersion = "0.1.0"

const usage = `entryconf — load a config directory into a single tree (entryconf spec ` + specVersion + `)

Usage:
  entryconf dump [flags] <dir>   load <dir> and print the resulting tree as JSON
  entryconf help                 show this message
  entryconf version              print the implemented spec version

Dump flags:
  -c, --compact                  print one line of JSON instead of indenting

A config directory holds exactly one entrypoint file (entrypoint.json, .yaml,
.yml or .toml), any number of *.env variable files, and any files reachable
through "@file:" references. Variables are read from the *.env files, with the
process environment taking precedence.

Exit status:
  0   the directory loaded; the tree is on stdout
  1   the load failed; the first line of stderr is the E_* error code
  2   the command line was wrong

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
		// The first line of stderr is exactly the E_* code, so other tools can
		// compare it across implementations.
		fmt.Fprintf(stderr, "%s\nentryconf: %s\n", errorCode(err), err)
		return 1
	}

	// Encode into a buffer first: a tree that could not be rendered must not
	// leave half a document on stdout for a caller to parse.
	var buf bytes.Buffer
	enc := json.NewEncoder(&buf)
	if !compact {
		enc.SetIndent("", "  ")
	}
	enc.SetEscapeHTML(false)
	if err := enc.Encode(tree); err != nil {
		// Unreachable: SPEC §2 makes the tree JSON-equivalent, and Load
		// rejects any value that is not. Still coded, so every exit-1 path
		// keeps the "first stderr line is an E_* code" contract.
		fmt.Fprintf(stderr, "%s\nentryconf: cannot encode tree as JSON: %s\n", entryconf.CodeParse, err)
		return 1
	}
	if _, err := stdout.Write(buf.Bytes()); err != nil {
		fmt.Fprintf(stderr, "entryconf: cannot write output: %s\n", err)
		return 1
	}
	return 0
}

// errorCode is the E_* code printed on the first line of stderr. Load always
// returns an *entryconf.Error; E_PARSE is the documented code for unreadable
// or malformed input and stands in if an error ever arrives unwrapped, so the
// contract holds unconditionally.
func errorCode(err error) string {
	var ecErr *entryconf.Error
	if errors.As(err, &ecErr) {
		return ecErr.Code()
	}
	return entryconf.CodeParse
}
