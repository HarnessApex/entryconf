# entryconf (Go)

Go implementation of the entryconf convention: load a config directory — one
entrypoint file, any number of `*.env` variable files, `@file:` includes and
`$VAR` interpolation — into a single tree.

Implements entryconf spec 0.2.0 (`../SPEC.md`). Conformance is defined by the
shared fixture suite in `../testdata/cases`.

## Install

```
go get github.com/HarnessApex/entryconf/go
```

Requires Go 1.22 or newer. Dependencies: `gopkg.in/yaml.v3` and
`github.com/pelletier/go-toml/v2`.

## Use

```go
import entryconf "github.com/HarnessApex/entryconf/go"

tree, err := entryconf.Load("envs/staging")
```

`Load` returns `map[string]any` holding Go's natural JSON shapes: nested
`map[string]any`, `[]any`, `string`, `bool`, `int64` or `float64` for numbers,
and `nil` for null.

Every failure is an `*entryconf.Error` carrying the normative SPEC §7 code:

```go
var ecErr *entryconf.Error
if errors.As(err, &ecErr) && ecErr.Code() == entryconf.CodeMissingVar {
    // a "${VAR}" reference had no value and no default
}
```

The codes are also available as constants: `CodeNoEntrypoint`,
`CodeMultipleEntrypoints`, `CodeParse`, `CodeEnvConflict`, `CodeInclude`,
`CodeIncludeCycle`, `CodeMissingVar`, `CodeSubstitution`.

That — `Load`, `Error`, and the code constants — is the entire public API.

## Command line

```
go run ./cmd/entryconf dump <dir>          # or: go install ./cmd/entryconf
```

`dump` loads `<dir>` and prints the tree as indented JSON on stdout, exiting 0.
On failure it exits 1 and writes the `E_*` code as the **first line of stderr**
(a human-readable message follows on the next line), which is what makes it
usable for cross-implementation comparison:

```
$ entryconf dump ../testdata/cases/06-include/config
{
  "cache": { "ttl": 60 },
  ...
}

$ entryconf dump ../testdata/cases/07-include-cycle/config
E_INCLUDE_CYCLE                                        # stderr, exit 1
entryconf: E_INCLUDE_CYCLE: include cycle: ...         # stderr
```

`dump -c` (or `--compact`) prints a single line. Every other fault — a
malformed command line, or an internal error such as output that cannot be
written — exits **2 and prints no `E_*` code**, so an `E_*` code on stderr
always means the config was rejected, never that the tool misfired.
`entryconf help` and `entryconf version` do what they say.

## Tests

```
go test ./...
```

`conformance_test.go` is the whole suite: it walks `../testdata/cases` and runs
every case as a subtest named after its directory. There are no hand-written
per-case tests, so the fixtures cannot drift.

Per SPEC §8 a case's variables must be set exactly as `procenv.json` says and
otherwise unset. The harness therefore injects the case's environment through
an internal seam (`load(dir, envSource)`) instead of mutating the real process
environment: fixtures cannot see stray variables, and the subtests need no
serialization.

Three things the fixtures cannot express are covered by unit tests beside the
harness:

- `TestLoadUsesProcessEnvironment` — the public `Load` reads the *real* process
  environment and it overrides a `*.env` value (`t.Setenv`).
- `TestLoadMissingDirectory` — a config directory that does not exist (or is a
  file) is `E_NO_ENTRYPOINT`; git cannot carry a case with no `config/`.
- `cmd/entryconf` tests — the CLI convention: a load failure exits 1 with the
  bare `E_*` code as the first line of stderr; any other fault exits 2 and
  prints no code; and the alias bomb of case 57 is rejected in milliseconds,
  because the SPEC §2 budget is counted as nodes are produced rather than
  enforced by a timeout.
