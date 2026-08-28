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
const specVersion = "0.2.0"

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
  2   any other fault (a wrong command line, or an internal error); no E_*
      code is printed, so an E_* code on stderr always means a load failure

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
		// A load failure, and only a load failure, prints a code: the first
		// line of stderr is exactly the E_* code, so other tools can compare
		// it across implementations.
		var ecErr *entryconf.Error
		if !errors.As(err, &ecErr) {
			// Unreachable: Load's contract is that every failure carries a
			// SPEC §7 code. An uncoded error is an internal fault, not a
			// verdict about the config, so it must not be given a code.
			fmt.Fprintf(stderr, "entryconf: internal error: %s\n", err)
			return 2
		}
		fmt.Fprintf(stderr, "%s\nentryconf: %s\n", ecErr.Code(), err)
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
		// rejects any value that is not. If it ever happens the load already
		// succeeded, so this is an internal fault: exit 2 with no E_* code.
		fmt.Fprintf(stderr, "entryconf: internal error: cannot encode tree as JSON: %s\n", err)
		return 2
	}
	if _, err := stdout.Write(buf.Bytes()); err != nil {
		fmt.Fprintf(stderr, "entryconf: cannot write output: %s\n", err)
		return 2
	}
	return 0
}
