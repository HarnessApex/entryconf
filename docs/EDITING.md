# Editing configuration with entryconf

This is the API tour and integration handoff for the editing surface added in
spec 0.3.0 (`SPEC.md` §10–§11). It is written for a program — a CLI or a
settings UI — that reads a config directory with entryconf and needs to change
values in it without losing track of which file authors what.

`Load(dir)` is unchanged. Everything here is additive; a program that never
calls `Open` is unaffected.

## The model in one paragraph

`Open(dir)` performs the ordinary load and returns a **snapshot**: the
effective tree, plus one **document** record per file the load read (the
entrypoint, every `@file:` target, every `*.env` file), each with a key
(path relative to the directory), a format, a content **revision**
(`sha256:…`), a `writable` flag (true only for JSON), and its **grafts**
(where its root value sits in the effective tree and through which `@file:`
references). `Inspect(pointer)` tells you which document and source pointer
author an effective value, the authored expression, the include chain, and
which variables it depends on and where each is supplied from. `Plan(edits)`
applies a batch of edits to one explicitly named JSON document in memory,
re-runs the full load with the new text substituted, and returns the
**candidate** tree plus the diff and every revision it depends on — nothing is
written. `Commit(plan)` takes the shared directory lock, re-verifies every
revision and the variable fingerprint, and atomically replaces the one file.

## Go

Import path: `github.com/HarnessApex/entryconf/go`.

```go
package entryconf

func Load(dir string) (map[string]any, error)                 // unchanged since 0.1.0
func Open(dir string) (*Snapshot, error)

type Snapshot struct {
    Dir       string
    Tree      map[string]any        // == Load(Dir)
    Documents map[string]*Document  // by key: "entrypoint.toml", "ephoros.json", "sub/db.json", "vars.env"
}
func (s *Snapshot) Inspect(pointer string) (*Origin, error)
func (s *Snapshot) Plan(edits []Edit) (*Plan, error)

type Document struct {
    Key, Path, Format, Revision string   // Format: "json" | "yaml" | "toml" | "env"
    Writable bool                        // Format == "json"
    Grafts   []Graft
}
type Graft     struct { Effective string; Chain []Reference }
type Reference struct { Document, Pointer string }

type Origin struct {
    Effective, Document, Pointer string
    Authored  any                 // the parsed authored value (a string like "${MODE}" for a reference)
    Chain     []Reference         // @file: references traversed from the entrypoint, outermost first
    Variables []Variable          // in order of appearance within Authored
    Writable  bool
}
type Variable struct { Name, Origin, File string }   // Origin: "process" | "file" (File = env doc key) | "default"

type Edit struct {
    Document string   // required: the document key to edit
    Op       string   // OpSet | OpRemove
    Pointer  string   // RFC 6901 within Document; "" is the root
    Value    any      // for set: nil, bool, string, int*, float*, []any, map[string]any (normalized)
    Mode     string   // "" / ModeLiteral (default) | ModeExpression
}
const OpSet, OpRemove, ModeLiteral, ModeExpression

type Plan struct {
    Document          string
    Before, After     string            // the document's text, before and after
    Candidate         map[string]any    // the tree a commit produces — validate this
    Affected          []string          // effective pointers where Candidate differs from the snapshot
    Grafts            []Graft           // every effective position the edit lands in
    Revisions         map[string]string // key -> revision of every document the candidate depends on
    Variables         []string
    VariablesRevision string
}
func (p *Plan) Commit(opts *CommitOptions) (*Receipt, error)

type CommitOptions struct { LockTimeout time.Duration }      // zero -> DefaultLockTimeout (5s)
type Receipt       struct { Document, Revision string; Revisions map[string]string }

// Error codes, in addition to the SPEC §7 load codes:
const CodeUnsupportedEdit, CodeEdit, CodePath, CodeStalePlan, CodeLocked, CodeWrite
```

Every failure is still an `*entryconf.Error`; branch on `Code()`.

### The Ephoros shape, end to end

The workspace file is a TOML entrypoint that grafts a user-editable JSON
document. The UI edits the JSON; the TOML is never rewritten.

```toml
# entrypoint.toml
ephoros = "@file:ephoros.json"

[workspace]
name = "demo"
```

```json
{ "sessions": { "task": { "permission_mode": "ask", "max_turns": 20 } } }
```

```go
snap, err := entryconf.Open(dir)
if err != nil { /* a load failure: E_PARSE, E_MISSING_VAR, ... — the directory is unusable as is */ }

// 1. Find out what authors the value. This is how the UI learns which file to
//    edit; the library never chooses a document for you.
origin, err := snap.Inspect("/ephoros/sessions/task/permission_mode")
// origin.Document == "ephoros.json", origin.Pointer == "/sessions/task/permission_mode"
// origin.Authored == "ask", origin.Writable == true
// origin.Chain == [{Document: "entrypoint.toml", Pointer: "/ephoros"}]
if !origin.Writable { /* tell the user this value lives in a TOML/YAML/env file */ }
if len(origin.Variables) > 0 { /* the value comes from ${VAR}: editing it replaces the reference */ }

// 2. Prepare the change against the document Inspect named.
plan, err := snap.Plan([]entryconf.Edit{{
    Document: origin.Document,
    Op:       entryconf.OpSet,
    Pointer:  origin.Pointer,
    Value:    "auto",          // literal: written so it loads back as exactly "auto"
}})
// plan.Candidate is the full effective tree after the change.
// plan.Affected == ["/ephoros/sessions/task/permission_mode"]
// plan.Before / plan.After are the document's text before and after.

// 3. Validate the candidate with your own schema / policy before writing.
if err := mySchema.Validate(plan.Candidate); err != nil { return err }

// 4. Commit. The exact candidate you validated is what lands: every file and
//    variable the candidate depended on is revision-checked under the lock.
receipt, err := plan.Commit(nil)
var ecErr *entryconf.Error
switch {
case err == nil:
    // receipt.Revision is ephoros.json's new revision; reload with Open to continue editing.
case errors.As(err, &ecErr) && ecErr.Code() == entryconf.CodeStalePlan:
    // someone else changed a dependency (or the environment); Open again and re-plan.
case errors.As(err, &ecErr) && ecErr.Code() == entryconf.CodeLocked:
    // another cooperating writer held the lock past the timeout; retry.
case errors.As(err, &ecErr) && ecErr.Code() == entryconf.CodeWrite:
    // filesystem failure; the source file is unchanged.
}
```

Equivalent CLI (the same library, no second editor):

```sh
entryconf inspect ./ws /ephoros/sessions/task/permission_mode
entryconf inspect ./ws                       # list documents, formats, revisions, grafts
entryconf edit -n ./ws request.json          # plan only: prints before/after/candidate/affected
entryconf edit ./ws request.json             # plan + commit; prints the same plus "committed" and "revision"
# request.json:
# {"edits":[{"document":"ephoros.json","op":"set","pointer":"/sessions/task/permission_mode","value":"auto"}]}
```

## Semantics you must not guess about

- **Literal vs. expression.** The default mode is literal: every string in the
  value (recursively, never object keys) has `$` doubled and a leading `@`
  doubled, so `"$HOME"` loads back as `"$HOME"` and `"@file:x"` as
  `"@file:x"`. `Mode: "expression"` writes a string verbatim and the ordinary
  load interprets it: `"${MODE:-dev}"` becomes a variable reference,
  `"@file:other.json"` an include. A malformed expression fails the plan with
  the load's code and nothing is written.
- **Untouched expressions stay.** Editing `/port` in a document whose `/host`
  is `"${DB_HOST}"` leaves `"${DB_HOST}"` in the file. The resolved value is
  never written back.
- **A referenced value is replaced, not reverse-mapped.** If
  `permission_mode` were `"${MODE}"`, setting it to `"auto"` writes `"auto"`
  over the reference in that document. Editing the `.env` definition instead
  is `E_UNSUPPORTED_EDIT` in this version; the process environment is never
  a target.
- **Remove ≠ null.** `remove` deletes the member; `set` with `nil` keeps the
  key with a null value.
- **Paths.** RFC 6901 pointers. `set` creates missing parents as objects,
  requires array indices to exist, and appends with the index `len` or `-`.
  `remove` requires every step to exist. Descending into a scalar — including
  an `@file:` string, which is a scalar in its own document — is `E_PATH`;
  edit the included document instead.
- **Shared includes.** A document included twice has two grafts; editing it
  changes both places and `plan.Affected` lists both. Replacing one reference
  with an inline value is a different edit (set the reference's pointer in
  the referencing document). Nothing clones or flattens on your behalf.
- **One file per commit.** Edits in one plan must name one document; a
  request naming two is `E_UNSUPPORTED_EDIT` before anything is written.
- **Normalized JSON.** The written document is re-serialized: two-space
  indent, keys sorted by code point, one member per line, minimal escaping,
  trailing newline. Authored formatting and member order are not preserved.
  Unedited files are byte-for-byte untouched.
- **Snapshot vs. live environment.** The candidate is evaluated against the
  environment captured at `Open`. Commit re-resolves the plan's variables
  against the live environment and re-read `*.env` files; any change is
  `E_STALE_PLAN`. A plan is bound to the revisions in `plan.Revisions`; a
  document that changed or disappeared is `E_STALE_PLAN`. Committing the
  same plan twice is stale the second time.
- **Locking is advisory.** Cooperating writers (any entryconf implementation)
  serialize through `<dir>/.entryconf.lock` (exclusive-create, 30-second stale
  breaking) and recheck revisions under it. An editor that does not take the
  lock can still race the rename; the revision check is not a compare-and-swap
  against arbitrary writers. `Load` ignores the lock file.
- **Atomicity and durability.** Same-directory temp file, fsync, permission
  bits copied, rename over the target (through symlinks: the link survives,
  its target is replaced), directory fsync where POSIX allows. Windows renames
  with `MOVEFILE_REPLACE_EXISTING` and preserves no permission bits.
  Ownership is never preserved. A crash before the rename leaves the old file
  and possibly a stray `.<name>.entryconf-tmp-*`; after it, the new file.

## Error codes

| Code | When | What to do |
|---|---|---|
| `E_UNSUPPORTED_EDIT` | the document is YAML/TOML/env, or a request names two documents | tell the user; nothing was written |
| `E_EDIT` | bad request: unknown document key, unknown op/mode, expression with a non-string, empty request | fix the request |
| `E_PATH` | malformed or unresolvable pointer (inspect, set, remove) | fix the pointer |
| load codes (`E_PARSE`, `E_INCLUDE`, `E_MISSING_VAR`, `E_SUBSTITUTION`, …) | the candidate does not load | the edit is rejected; nothing was written |
| `E_STALE_PLAN` | at commit, a dependency's revision or a variable changed | `Open` again, re-plan, re-validate |
| `E_LOCKED` | the lock was held past the timeout | retry |
| `E_WRITE` | temp file or rename failed | source unchanged; report the cause |

## Other languages

The same four operations with the same semantics and wire shapes:

| | Python | TypeScript | Rust |
|---|---|---|---|
| open | `entryconf.open(dir) -> Snapshot` | `open(dir): Snapshot` | `entryconf::open(&Path) -> Result<Snapshot>` |
| inspect | `snap.inspect(ptr) -> Origin` | `snap.inspect(ptr): Origin` | `snap.inspect(&str) -> Result<Origin>` |
| plan | `snap.plan([Edit(...)]) -> Plan` | `snap.plan(edits): Plan` | `snap.plan(&[Edit]) -> Result<Plan>` |
| commit | `plan.commit(lock_timeout=5.0) -> Receipt` | `plan.commit({lockTimeoutMs}): Receipt` | `plan.commit(&CommitOptions) -> Result<Receipt>` |

See each implementation's README for its exact types.

## Conformance

`testdata/editcases/` (58 cases) pins every rule above across the four
implementations, and `tools/crosscheck` runs each implementation's `inspect` /
`edit` CLI over every case in a scratch copy, diffing origins, candidates,
affected pointers, and the written bytes against the fixture and each other.
Lock contention, write failure, permission preservation, symlinks, and two
cooperating writers are unit-tested in every implementation.
