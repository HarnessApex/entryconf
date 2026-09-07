# Contributing to entryconf

The product here is the **spec plus the conformance suite**. The four
implementations (`go/`, `python/`, `ts/`, `rust/`) are thin clients of both.
Contributions are welcome; the rules below exist so the implementations can
never disagree about what `Load(dir)` means.

## The spec-first rule

`SPEC.md` is normative. `testdata/cases/` (reading, SPEC §8) and
`testdata/editcases/` (editing, SPEC §11) operationally define conformance: an
implementation is correct iff it passes every case in both.

**Any behavior change ships in one PR:**

1. the wording in `SPEC.md`,
2. a fixture under `testdata/cases/` or `testdata/editcases/` that pins the
   new behavior,
3. **all four implementations** updated to match.

Do not land a behavior change in one implementation and "catch the others up
later" — a drifting implementation is a bug even when its own tests pass. If a
change is too large for one PR, discuss it in an issue first
(`.github/ISSUE_TEMPLATE/spec-change.md`) rather than splitting it across
implementations.

Cross-language divergences are found by `tools/crosscheck` (below), so a PR
that only fixes some implementations will fail CI.

## Error codes are public contract

The `E_*` codes in SPEC §7 and §10.9 are part of the API. Do **not** rename an
existing code, reuse it for a different condition, or change which condition
it covers without a spec change. New codes go into the spec table *and*
`testdata/errors.json`, with at least one failure fixture in the suite the
entry's `"suite"` names (`cases` by default, or `editcases`) — `tools/lintcases`
enforces that coupling. The one exemption is `"suite": "unit"`, for a code
whose condition a fixture cannot carry (`E_LOCKED`, `E_WRITE`); each
implementation must then cover it with a unit test.

## Bug reports: send a fixture

The most useful bug report is a **case directory**, not a prose description.
A minimal reproduction is:

```
cases/<NN>-<kebab-name>/
  config/                 the config directory to load
  procenv.json            optional: process env vars the case needs
  expected.json           success cases: the tree you expect
  expected_error.txt      failure cases: the single expected E_* code
```

An editing bug is reported the same way with the SPEC §11 shape — `config/`,
`request.json` (`{"inspect": …}`, `{"documents": true}`, or `{"edits": […]}`
with an optional `before_commit`), and `expected.json` or `expected_error.txt`
— under `testdata/editcases/`.

Trim `config/` to the smallest thing that still shows the problem. If the four
implementations disagree, say which produced which result — the per-language
dump and editing commands (below) print a comparable tree or a bare `E_*`
code, and `tools/crosscheck` diffs them for you.

Use `.github/ISSUE_TEMPLATE/conformance-failure.md` for "an implementation is
wrong" and `.github/ISSUE_TEMPLATE/spec-change.md` for "the spec is wrong or
silent".

## Fixture conventions

- Case directories are numbered and kebab-cased: `12-some-behavior/`. The
  numeric prefix must be unique across the suite.
- Exactly one of `expected.json` or `expected_error.txt` per case, plus a
  `config/` directory (and, for an editing case, a `request.json`).
  `expected_error.txt` holds one code, listed in `testdata/errors.json`.
- Editing cases are run against a fresh copy of `config/`. Their
  `expected.json` names the written document's parsed value; every other
  file must come out byte-identical, so put comments and odd formatting in
  the YAML/TOML/env files of an editing case on purpose.
- Variable names use the `EC_` prefix (or another unlikely name) so real
  environment variables cannot leak in. Per the harness contract (SPEC §8,
  `testdata/README.md`), every variable a case mentions must be otherwise unset.
- `expected.json` is compared structurally; numbers compare numerically
  (`8080` equals `8080.0`).
- Every normative MUST in the spec should have at least one fixture, and every
  error code at least one failure case.
- Some rules cannot be fixtured at all: git stores neither a missing directory
  nor file permissions, so "the config directory does not exist"
  (`E_NO_ENTRYPOINT`), an unreadable entrypoint or `*.env` file (`E_PARSE`),
  and an unreadable `@file:` target (`E_INCLUDE`) have no case form. Each
  implementation covers those with unit tests beside its fixture harness, and
  a new implementation is expected to do the same — those tests are the only
  thing pinning the behavior.

## Running the suites

Fixture linter (stdlib-only Go module; run it before anything else):

```sh
cd tools/lintcases && go run . -root ../..
```

Per-implementation conformance suites — each is a harness that walks
`testdata/cases/`, so there are no per-case tests to drift:

```sh
cd go     && go test ./...
cd python && pip install -e . pytest && pytest
cd ts     && npm ci && npm test
cd rust   && cargo test
```

Cross-implementation differ — runs every implementation's dump command over
every read case, and its `inspect`/`edit` command over every editing case (in
a scratch copy), and diffs the results against the fixture *and* against each
other — including the bytes each implementation wrote (stdlib Python only;
needs all four toolchains available):

```sh
python3 tools/crosscheck/crosscheck.py            # whole suite
python3 tools/crosscheck/crosscheck.py -case 06-include -v
```

Dump one config directory with a single implementation. Every dump CLI shares
one exit convention, so the crosscheck can tell a verdict from a broken tool:
**0** the tree as JSON on stdout; **1** a load failure, with the bare `E_*`
code as the first line of stderr and nothing on stdout; **2** anything else (a
wrong command line, an internal fault), printing no `E_*` code at all. An
`E_*` code therefore always means the config was rejected.

```sh
cd go     && go run ./cmd/entryconf dump <dir>
             python -m entryconf <dir>
cd ts     && node src/cli.ts <dir>
cd rust   && cargo run --quiet --bin entryconf-dump -- <dir>
```

The editing CLIs share one command shape and one output shape, so the
crosscheck can diff them too. `inspect <dir> <pointer>` prints the SPEC §10.3
origin object; `inspect <dir>` prints `{"dir", "documents"}`; `edit [-n] <dir>
<request.json|->` prints the plan (`document`, `before`, `after`, `candidate`,
`affected`, `grafts`, `revisions`, `variables`, `variables_revision`) plus
`committed` and the new `revision` (`null` after `-n`). Exit codes are the
dump convention's: 1 with the bare code for any `E_*` failure — load, edit,
stale plan, lock — and 2 for anything else.

```sh
cd go     && go run ./cmd/entryconf inspect <dir> [<pointer>]
             go run ./cmd/entryconf edit [-n] <dir> <request.json>
             python -m entryconf inspect <dir> [<pointer>]
             python -m entryconf edit [-n] <dir> <request.json>
cd ts     && node src/cli.ts inspect <dir> [<pointer>]
             node src/cli.ts edit [-n] <dir> <request.json>
cd rust   && cargo run --quiet --bin entryconf-edit -- inspect <dir> [<pointer>]
             cargo run --quiet --bin entryconf-edit -- edit [-n] <dir> <request.json>
```

CI (`.github/workflows/ci.yml`) runs the linter, all four suites, the
crosscheck, and a packaging dry run on every push and pull request.

## Adding a language implementation

- Public API: a single `Load(dir)` in local idiomatic casing, returning the
  tree as the language's natural map type (SPEC §9), plus an error type
  exposing the `E_*` code; and the editing surface of SPEC §10 — `Open`
  returning a snapshot with `Inspect`, `Plan`, and `Commit`. Keep the surface
  minimal.
- The test suite must be a harness that walks `../testdata/cases/` and one
  that walks `../testdata/editcases/` — never hand-written per-case tests. Add
  unit tests only for what a fixture cannot express (see *Fixture
  conventions*; for editing: lock contention, write failure, permissions,
  symlinks, two cooperating writers) and for the CLI exit convention.
- Use stock parsers per SPEC §2: YAML 1.2 core schema, TOML datetimes rendered
  as RFC 3339-style strings.
- Add the implementation to `tools/crosscheck` and to CI.

## Style

Match the surrounding code and prose. Keep implementations small and boring:
this is a convention, not a framework.

## License

Contributions are accepted under the repository's Apache-2.0 license
(`LICENSE`).
