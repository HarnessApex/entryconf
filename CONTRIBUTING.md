# Contributing to entryconf

The product here is the **spec plus the conformance suite**. The four
implementations (`go/`, `python/`, `ts/`, `rust/`) are thin clients of both.
Contributions are welcome; the rules below exist so the implementations can
never disagree about what `Load(dir)` means.

## The spec-first rule

`SPEC.md` is normative. `testdata/cases/` operationally defines conformance: an
implementation is correct iff it passes every case.

**Any behavior change ships in one PR:**

1. the wording in `SPEC.md`,
2. a fixture under `testdata/cases/` that pins the new behavior,
3. **all four implementations** updated to match.

Do not land a behavior change in one implementation and "catch the others up
later" — a drifting implementation is a bug even when its own tests pass. If a
change is too large for one PR, discuss it in an issue first
(`.github/ISSUE_TEMPLATE/spec-change.md`) rather than splitting it across
implementations.

Cross-language divergences are found by `tools/crosscheck` (below), so a PR
that only fixes some implementations will fail CI.

## Error codes are public contract

The `E_*` codes in SPEC §7 are part of the API. Do **not** rename an existing
code, reuse it for a different condition, or change which condition it covers
without a spec change. New codes go into the SPEC §7 table *and*
`testdata/errors.json`, with at least one failure fixture — `tools/lintcases`
enforces that coupling.

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

Trim `config/` to the smallest thing that still shows the problem. If the four
implementations disagree, say which produced which result — the per-language
dump commands (below) print a comparable tree or a bare `E_*` code, and
`tools/crosscheck` diffs them for you.

Use `.github/ISSUE_TEMPLATE/conformance-failure.md` for "an implementation is
wrong" and `.github/ISSUE_TEMPLATE/spec-change.md` for "the spec is wrong or
silent".

## Fixture conventions

- Case directories are numbered and kebab-cased: `12-some-behavior/`. The
  numeric prefix must be unique across the suite.
- Exactly one of `expected.json` or `expected_error.txt` per case, plus a
  `config/` directory. `expected_error.txt` holds one code, listed in
  `testdata/errors.json`.
- Variable names use the `EC_` prefix (or another unlikely name) so real
  environment variables cannot leak in. Per the harness contract (SPEC §8,
  `testdata/README.md`), every variable a case mentions must be otherwise unset.
- `expected.json` is compared structurally; numbers compare numerically
  (`8080` equals `8080.0`).
- Every normative MUST in the spec should have at least one fixture, and every
  error code at least one failure case.

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
every case and diffs the results against the fixture *and* against each other
(stdlib Python only; needs all four toolchains available):

```sh
python3 tools/crosscheck/crosscheck.py            # whole suite
python3 tools/crosscheck/crosscheck.py -case 06-include -v
```

Dump one config directory with a single implementation (stdout: the tree as
JSON, exit 0; stderr: the bare `E_*` code, exit 1):

```sh
cd go     && go run ./cmd/entryconf dump <dir>
             python -m entryconf <dir>
cd ts     && node src/cli.ts <dir>
cd rust   && cargo run --quiet --bin entryconf-dump -- <dir>
```

CI (`.github/workflows/ci.yml`) runs the linter, all four suites, the
crosscheck, and a packaging dry run on every push and pull request.

## Adding a language implementation

- Public API: a single `Load(dir)` in local idiomatic casing, returning the
  tree as the language's natural map type (SPEC §9), plus an error type
  exposing the `E_*` code. Keep the surface minimal.
- The test suite must be a harness that walks `../testdata/cases/` — never
  hand-written per-case tests.
- Use stock parsers per SPEC §2: YAML 1.2 core schema, TOML datetimes rendered
  as RFC 3339-style strings.
- Add the implementation to `tools/crosscheck` and to CI.

## Style

Match the surrounding code and prose. Keep implementations small and boring:
this is a convention, not a framework.

## License

Contributions are accepted under the repository's Apache-2.0 license
(`LICENSE`).
