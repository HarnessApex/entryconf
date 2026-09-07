# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).
While the spec is `0.x`, any release may change normative behavior.

## [0.3.0-rc.1] - 2026-09-08

**Pre-release.** The editing surface below is complete and verified on Linux
across all four implementations; this release candidate exists to exercise
it on other platforms and in the Ephoros integration before 0.3.0 is final.
Package versions are `0.3.0rc1` (Python) and `0.3.0-rc.1` (npm, crate, Go
tag); the spec text is 0.3.0.

Adds a source-aware **editing** surface — `Open`, `Inspect`, `Plan`, `Commit` —
to the spec and all four implementations. `Load(dir)` and every read fixture
are unchanged; a program that never edits is unaffected.

### Added

- **Editing** (`SPEC.md` §10) — `Open(dir)` returns a snapshot: the effective
  tree, every source document the load read (key, format, `sha256:` revision,
  `writable`, and its grafts into the effective tree), and the process
  environment as captured. `Inspect(pointer)` reports an effective value's
  provenance: authoring document and source pointer, the authored value, the
  `@file:` chain, and each `$` variable with the layer that supplies it
  (`process`, `file`, `default`). `Plan(edits)` applies a batch of `set` /
  `remove` edits (RFC 6901 pointers) to **one explicitly named JSON document**
  in memory, evaluates the candidate with the full load semantics and the new
  text substituted, and returns before/after text, the candidate tree, the
  affected effective pointers, the grafts the change lands in, and the
  revisions and variable fingerprint it depends on. `Commit(plan)` takes the
  shared advisory lock (`.entryconf.lock`, exclusive-create, 30 s stale
  breaking), rechecks every revision and the live variables, and atomically
  replaces the one file (same-directory temp, fsync, permissions preserved,
  rename; symlinks resolved so the link survives).
- **Literal vs. expression writes** — literal values are escaped (`$`→`$$`,
  leading `@`→`@@`, recursively, never keys) so they load back as themselves;
  `expression` mode writes a string verbatim. Untouched expressions are carried
  through; a resolved value is never written over a `${VAR}` reference.
- **Editing scope made explicit** — only JSON is writable. YAML, TOML and
  `*.env` are read exactly as before and an edit naming one is
  `E_UNSUPPORTED_EDIT` with no file touched. One plan edits one document;
  multi-document requests are rejected before writing. Normalized JSON output
  (§10.8) is documented; unedited files stay byte-identical.
- **Error codes** (§10.9) — `E_UNSUPPORTED_EDIT`, `E_EDIT`, `E_PATH`,
  `E_STALE_PLAN`, `E_LOCKED`, `E_WRITE`. A candidate that fails to load reports
  the load code, so conflicts, parse failures, and unsupported operations are
  distinguishable. `testdata/errors.json` entries now carry a `"suite"` field
  saying which suite covers the code (`editcases`, or `unit` for the two that
  need concurrency or filesystem failure).
- **Editing conformance suite** (`SPEC.md` §11, `testdata/editcases/`, 58
  cases) — scalar/subtree set, remove vs null, invalid paths, literal `$`/`@`
  round trips including nested containers and keys, expression writes and
  preservation of unrelated expressions, editing an included JSON document
  under JSON/YAML/TOML entrypoints (the TOML case is the Ephoros shape:
  `ephoros = "@file:ephoros.json"`), shared-include impact at both grafts,
  unrelated files unchanged, process-env and `.env` provenance, unsupported
  YAML/TOML/env edits, candidate validation failures (`E_MISSING_VAR`,
  `E_SUBSTITUTION`, `E_INCLUDE`, `E_PARSE`), stale document / other document /
  variable / new-include revisions, and multi-document rejection. Cases run
  against a copy of `config/`; harnesses assert untouched files are
  byte-identical and nothing was added.
- **Per-implementation unit tests** for what fixtures cannot carry: two
  cooperating writers, lock contention and stale-lock breaking, permission
  preservation, write failure cleanup, symlinked documents, plan purity, live
  environment changes, serialization bytes.
- **CLIs** — Go `entryconf inspect <dir> [ptr]` and `entryconf edit [-n]
  <dir> <request>`; Python `python -m entryconf inspect|edit …`; TypeScript
  `node src/cli.ts inspect|edit …`; Rust `entryconf-edit inspect|edit …`. All
  share one output shape and the dump exit convention, so `tools/crosscheck`
  diffs origins, candidates, affected pointers, and written bytes across
  implementations.
- **Tooling** — `tools/lintcases` validates `testdata/editcases/` (request
  shapes, expectation files) and per-suite error-code coverage;
  `tools/crosscheck` runs the editing suite through every implementation's
  editing CLI in a scratch copy.
- **Docs** — `docs/EDITING.md`: API tour, Ephoros-shaped walkthrough, exact Go
  signatures, semantics, guarantee boundary, error handling.

### Changed

- Spec version bumped to **0.3.0**, along with every implementation's version
  and README conformance line. `SPEC.md` §7 now points to §10.9; §9 notes the
  additive editing API.
- Rust: `ErrorCode` gained six variants; an exhaustive `match` over it needs
  the new arms (the string form `Error::code()` is unaffected).

## [0.2.0] - 2026-08-29

Bounds YAML alias expansion, pins down leading-zero integers, and unifies the
dump CLIs' exit codes. No change to any tree that loaded under 0.1.0.

### Added

- **Bounded YAML alias expansion** (`SPEC.md` §2) — a document whose fully
  expanded tree would exceed **1,000,000 nodes** is `E_PARSE`. Nodes are
  counted recursively: a scalar value counts one; a sequence or mapping
  contributes one per element or entry **plus** each element value's own
  count; mapping keys are not counted. So a sequence of 1,000 scalars counts
  2,000 — each element once as an element slot and once as the scalar in it —
  and a flat mapping of 1,000 scalar entries likewise counts 2,000, not 3,000.
  Aliases still
  resolve to plain values, so a layered anchor graph can expand exponentially
  in the source size; the budget closes that denial-of-service without
  touching any plausible config, which is orders of magnitude smaller. All
  four implementations charge the budget *as the tree is produced* (Rust and
  Python size a shared subtree once and charge it at every reference), so an
  over-budget document is rejected after bounded work instead of being
  materialized first — the 48-million-node bomb of case 57 is rejected in
  well under a tenth of a second in every implementation.
- **Fixtures 57-62** — `57-yaml-alias-bomb` (the layered bomb, `E_PARSE`);
  `58-yaml-alias-heavy-ok` (109 alias references expanding to under a
  thousand nodes, which MUST load — an implementation capping alias
  *references* rather than expanded nodes fails here);
  `59-yaml-leading-zero-int`; `60-toml-lowercase-t-separator`;
  `61-env-unbalanced-quote`; and `62-yaml-budget-counting`, which pins the
  counting rule itself: at 1,202,602 nodes it is 1.20x over the budget under
  the rule above and 0.60x under it (601,603 nodes) under a naive
  "count every value once" reading, so an implementation using the naive rule
  loads it and fails the case. Cases 57 and 58 are deliberately
  verdict-stable — 48x over and three orders of magnitude under — so neither
  turns on how the rule is read; 62 is the case that does.
- **Per-implementation unit tests for the unfixturable case** — a config
  directory that does not exist, or that is a regular file, is
  `E_NO_ENTRYPOINT` (SPEC §3). Git cannot store a missing directory, so the
  shared suite cannot express this; each of `go/`, `python/`, `ts/`, and
  `rust/` now asserts it beside the fixture harness, along with the dump-CLI
  exit convention and the expansion budget.

### Changed

- **Leading-zero integers** (`SPEC.md` §2) — an explicit core-schema note that
  an unquoted `010` is the decimal integer 10: YAML 1.2 has no leading-zero
  octal form, and octal is spelled `0o10`. This is a clarification, not a
  behavior change — the core schema already implied it — but it is where
  stock YAML 1.1 resolvers (PyYAML's included) diverge by reading `010` as 8,
  so it is now stated and fixtured.
- **Unified dump-CLI exit convention** — every implementation's dump command
  now exits **1** for a load failure, with the bare `E_*` code as the first
  line of stderr and nothing on stdout, and **2** for anything else (a wrong
  command line, an internal fault), printing no `E_*` code at all. An `E_*`
  code on stderr therefore always means the config was rejected, never that
  the tool misfired, so a cross-checking harness can never read a broken
  invocation as a conformance verdict.
- Spec version bumped to **0.2.0**, along with `python/pyproject.toml`,
  `ts/package.json`, `rust/Cargo.toml`, and every implementation's README
  conformance line.

## [0.1.0] - 2026-08-28

First version: the specification, the language-neutral conformance suite,
and four implementations tracking them.

### Added

- **Specification** (`SPEC.md`, v0.1.0) — the normative definition of
  `Load(dir)`: a single `entrypoint.{json,yaml,yml,toml}` per config directory
  (§3); unordered `*.env` peer files with duplicate definitions as errors and
  the process environment overriding them (§4); `@file:` includes resolved
  relative to the referencing file, recursively, with `@@` escaping and cycle
  detection (§5); `$NAME` / `${NAME}` / `${NAME:-default}` / `$$`
  interpolation with whole-value typing and inert substitution output (§6);
  and the normative error-code table `E_NO_ENTRYPOINT`,
  `E_MULTIPLE_ENTRYPOINTS`, `E_PARSE`, `E_ENV_CONFLICT`, `E_INCLUDE`,
  `E_INCLUDE_CYCLE`, `E_MISSING_VAR`, `E_SUBSTITUTION` (§7).
- **Conformance suite** (`testdata/cases/`) — language-neutral fixtures, each
  a `config/` directory plus either `expected.json` (structural comparison,
  numbers compared numerically) or `expected_error.txt` (expected error code),
  with an optional `procenv.json` for process-environment cases. Covers the
  basic load, substitution, defaults, missing variables, `.env` conflicts,
  includes and include cycles, a TOML entrypoint, `@@`/`$$` escapes, process
  env override, and multiple entrypoints. The harness contract is in SPEC §8
  and `testdata/README.md`; `testdata/errors.json` lists the error codes the
  suite must cover.
- **Implementations** — `go/` (including the `cmd/entryconf` dump CLI),
  `python/`, `ts/`, and `rust/`, each exposing a single `Load(dir)` in local
  idiomatic casing and testing itself with a harness that walks the shared
  fixtures rather than hand-written per-case tests.
- **Tooling** — `tools/lintcases/`, a stdlib-only Go module that validates the
  fixture suite's structure (case naming, unique numbering, exactly one
  expectation file, parseable JSON, and error-code coverage against
  `testdata/errors.json`); `tools/crosscheck/`, which compares implementation
  output across languages; and a GitHub Actions workflow running the linter
  and all four implementations' test suites on push and pull request.
- **Project chrome** — Apache-2.0 `LICENSE`, this changelog, and contributor
  guidance in `CLAUDE.md`.

[0.3.0-rc.1]: https://github.com/HarnessApex/entryconf/releases/tag/spec/v0.3.0-rc.1
[0.2.0]: https://github.com/HarnessApex/entryconf/releases/tag/spec/v0.2.0
[0.1.0]: https://github.com/HarnessApex/entryconf/releases/tag/spec/v0.1.0
