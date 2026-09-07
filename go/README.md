# entryconf (Go)

Go implementation of the entryconf convention: load a config directory — one
entrypoint file, any number of `*.env` variable files, `@file:` includes and
`$VAR` interpolation — into a single tree.

Implements entryconf spec 0.3.0 (`../SPEC.md`). Conformance is defined by the
shared fixture suites in `../testdata/cases` (reading) and
`../testdata/editcases` (editing).

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

### Editing (spec 0.3.0, SPEC §10)

```go
snap, err := entryconf.Open("envs/staging")            // effective tree + source documents
origin, err := snap.Inspect("/database/pool/max")       // which file/pointer authors it, include chain, variables
plan, err := snap.Plan([]entryconf.Edit{{
    Document: origin.Document,                          // always explicit; only JSON documents are writable
    Op:       entryconf.OpSet,
    Pointer:  origin.Pointer,
    Value:    20,                                       // literal (default): loads back as exactly this value
}})
// plan.Candidate — the tree a commit produces; validate it here
// plan.Affected  — effective pointers that change; plan.Before / plan.After — the document's text
receipt, err := plan.Commit(nil)                        // lock, revision recheck, atomic replace
```

Types: `Snapshot{Dir, Tree, Documents}`, `Document{Key, Path, Format, Revision,
Writable, Grafts}`, `Graft{Effective, Chain}`, `Reference{Document, Pointer}`,
`Origin{Effective, Document, Pointer, Authored, Chain, Variables, Writable}`,
`Variable{Name, Origin, File}`, `Edit{Document, Op, Pointer, Value, Mode}`,
`Plan{Document, Before, After, Candidate, Affected, Grafts, Revisions,
Variables, VariablesRevision}`, `CommitOptions{LockTimeout}`, `Receipt{Document,
Revision, Revisions}`, and the codes `CodeUnsupportedEdit`, `CodeEdit`,
`CodePath`, `CodeStalePlan`, `CodeLocked`, `CodeWrite`. `Edit.Mode` is
`ModeLiteral` (default) or `ModeExpression` (write a `${VAR}` / `@file:` string
verbatim). The full semantics — literal escaping, remove vs null, shared
includes, stale plans, the lock protocol, atomic replacement — are in
`../docs/EDITING.md` and SPEC §10.

`Load` is unchanged from 0.2.0; the editing surface is additive.

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
always means the config or the edit was rejected, never that the tool misfired.
`entryconf help` and `entryconf version` do what they say.

```
entryconf inspect <dir> [<pointer>]      # the SPEC §10.3 origin of a value, or the documents list
entryconf edit [-n] <dir> <request.json> # plan (and, without -n, commit) the edits in the request
```

`edit` prints the plan — `document`, `before`, `after`, `candidate`,
`affected`, `grafts`, `revisions`, `variables`, `variables_revision` — plus
`committed` and the new `revision`. A request is
`{"edits": [{"document": "app.json", "op": "set", "pointer": "/port", "value": 8080}]}`;
`-` reads it from stdin; `--lock-timeout 10s` widens the lock wait. Editing
failures follow the same exit convention: 1 with the bare code (`E_STALE_PLAN`,
`E_UNSUPPORTED_EDIT`, …) first on stderr.

## Tests

```
go test ./...
```

`conformance_test.go` walks `../testdata/cases` and `editcases_test.go` walks
`../testdata/editcases`, each running every case as a subtest named after its
directory. There are no hand-written per-case tests, so the fixtures cannot
drift. Editing cases run against a copy of their `config/` and then assert that
every untouched file is byte-identical and no lock or temporary file remains.

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
  enforced by a timeout. `inspect` and `edit` are driven over a copy of the
  Ephoros-shaped fixture, including a dry run and a rejected YAML edit.
- Editing unit tests (`editcases_test.go`, SPEC §10.9) — two cooperating
  writers (the second commit is `E_STALE_PLAN`), lock contention (`E_LOCKED`
  within the timeout, a 30-second-stale lock broken), permission bits
  preserved, a write failure leaving the source intact with no temporary or
  lock file (`E_WRITE`), a symlinked document whose link survives, a plan that
  touches no file, a live environment change making a plan stale, and the
  SPEC §10.8 serialization bytes.
