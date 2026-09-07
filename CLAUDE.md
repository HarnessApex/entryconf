# entryconf

A cross-language convention for loading a config directory (one entrypoint +
`*.env` files + `@file:` includes + `$VAR` interpolation) into a single tree.
The product is the **spec and conformance suite**; language implementations
are thin clients of both.

## Source of truth

- `SPEC.md` is normative. `testdata/cases/` (reading) and `testdata/editcases/`
  (editing, SPEC §10–11) operationally define conformance: an implementation
  is correct iff it passes every case in both.
- Any behavior change MUST be made spec-first: update `SPEC.md`, add or update
  fixtures, then change implementations. Never let an implementation's
  behavior drift from the spec "temporarily".
- Error **codes** (`E_*`, SPEC §7) are part of the public contract. Don't
  rename or reuse them; add new ones to the table in SPEC §7.

## Layout

```
SPEC.md               normative spec (v0.3.0)
testdata/cases/       read fixtures: config/ + expected.json
                      or expected_error.txt (+ optional procenv.json)
testdata/editcases/   editing fixtures: config/ + request.json + expected.json
                      or expected_error.txt (+ optional procenv.json)
testdata/errors.json  the error codes (SPEC §7, §10.9) and which suite covers each
docs/EDITING.md       editing API tour + integration handoff
go/                   Go implementation (+ cmd/entryconf: dump, inspect, edit)
python/               Python implementation
ts/                   TypeScript implementation
rust/                 Rust implementation
tools/lintcases/      stdlib-only Go module: validates the fixture suite
tools/crosscheck/     compares implementation output across languages
```

## Fixture conventions

- Case dirs are numbered and kebab-cased: `12-some-behavior/`.
- Variable names in fixtures use the `EC_` prefix (or another unlikely name)
  so real environment variables can't leak in; the harness contract
  (SPEC §8, `testdata/README.md`) requires case-named vars to be otherwise
  unset.
- Every normative MUST in the spec should have at least one fixture; every
  error code should have at least one failure case.
- `expected.json` compares structurally; numbers compare numerically.

## Design invariants (don't relitigate casually)

- Conventions over flexibility: no manifest, no merge-order rules. Composition
  is `@file:` grafting; variance across environments comes from `.env` values.
- `*.env` files are unordered peers → duplicate definitions are errors, never
  last-wins. Process env overrides `.env`.
- Fail loudly at load time: no silently skipped files, no partial results.
- Substitution output is inert (never re-scanned for `$` or `@file:`).
- Strict `$` handling: malformed forms are `E_SUBSTITUTION`, not literals.
  `${NAME:…}` other than `:-` is reserved for future extensions.
- Editing (SPEC §10) is JSON-only and explicit: an edit names its document;
  nothing chooses between a shared include and a reference for the caller;
  YAML/TOML/env are never rewritten; a commit writes one file atomically under
  the shared `.entryconf.lock` protocol and is `E_STALE_PLAN` if any dependency
  changed. Literal strings are escaped so they load back as themselves.
- Editing fixtures run against a *copy* of `config/`; untouched files must
  stay byte-identical and no lock/temp file may remain. `E_LOCKED`/`E_WRITE`
  are unit-tested per implementation (`"suite": "unit"` in `errors.json`).

## When adding a language implementation

- Public API: a single `Load(dir)` in local idiomatic casing, returning the
  tree as the language's natural map type (SPEC §9). Keep the surface minimal.
- The test suite must be a harness that walks `../testdata/cases/` and one
  that walks `../testdata/editcases/` — do not hand-write per-case tests that
  could drift from the shared fixtures.
- The editing CLI (`inspect <dir> [ptr]`, `edit [-n] <dir> <request>`) must
  print the same JSON shapes as the others so `tools/crosscheck` can diff it.
- Use stock parsers (JSON/YAML/TOML) per SPEC §2 restrictions: YAML 1.2 core
  schema, TOML datetimes → RFC 3339 strings.
