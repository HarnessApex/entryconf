# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).
While the spec is `0.x`, any release may change normative behavior.

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

[0.1.0]: https://github.com/entryconf/entryconf/releases/tag/v0.1.0
