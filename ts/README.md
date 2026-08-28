# entryconf (TypeScript)

Implements the **entryconf spec 0.2.0** (`../SPEC.md`). Conformance is defined
by the shared fixture suite in `../testdata/cases/`.

Development runs straight from source: Node 22+ executes `.ts` directly via
type stripping, so `npm test` and the dump CLI need no build step. Publishing
does need one — Node does **not** type-strip inside `node_modules`, so the
package ships compiled JS plus declarations (see [Build](#build)).

## Install

```sh
npm install        # yaml + smol-toml, plus typescript for the build
```

Requires Node >= 22 — type stripping runs `.ts` unflagged from 22.18 onward;
on an earlier 22.x, pass `--experimental-strip-types`.

## Use

```ts
import { load, EntryconfError } from "entryconf";

try {
  const config = load("envs/deploy"); // plain JSON-equivalent object
  console.log(config.database.host);
} catch (err) {
  if (err instanceof EntryconfError) {
    console.error(err.code); // "E_INCLUDE_CYCLE", "E_MISSING_VAR", ...
  }
  throw err;
}
```

The public surface is exactly `load(dir)`, the `EntryconfError` class (with its
normative `code`), and the `Tree` / `Value` / `ErrorCode` types. `load()` is
synchronous — startup config is read once, before anything else runs — and
reads the real process environment, which overrides `*.env` values (SPEC §4).

## Dump (cross-implementation checking)

```sh
node src/cli.ts <config-dir>
```

Exit codes follow the repo-wide dump-CLI convention, so a harness can tell a
conformance result from a broken tool:

| Exit | Meaning |
|---|---|
| 0 | the tree is on stdout as JSON |
| 1 | the load failed — the bare `E_*` code is the first line on stderr |
| 2 | any other fault (usage, internal) — no `E_*` code is printed |

The tool takes exactly one positional argument and knows no options, so a
dash-led first argument is a usage fault, never a directory name: `--help` and
`-h` print the usage line on stdout and exit 0, anything else starting with `-`
exits 2. Neither ever prints an `E_*` code.

Set process-env variables the usual way:
`EC_HOST=prod node src/cli.ts ./config`.

## Test

```sh
npm test
```

`test/conformance.test.ts` is the whole suite: it walks `../testdata/cases`,
runs each case as a subtest named after its directory, sets `procenv.json`
variables while scrubbing every other variable the case's files mention, and
compares `expected.json` structurally with numeric equality for numbers
(`8080` equals `8080.0`). Cases run serially because they mutate
`process.env`.

## Build

```sh
npm run build       # tsc -p tsconfig.build.json  ->  dist/*.js + dist/*.d.ts
npm run typecheck   # tsc --noEmit over src/ and test/
```

`npm pack` and `npm publish` run the build automatically via `prepack`, so the
tarball always carries fresh output. Only `dist/` and `README.md` are packed
(the `files` whitelist) — no sources, tests, or configs — and `main`,
`exports`, `types`, and `bin` all point into `dist/`.

Two configs, on purpose: `tsconfig.json` is the dev/CI type-check (`noEmit`,
covering `src/` and `test/`), and `tsconfig.build.json` extends it to emit
`src/` into `dist/`. Sources import each other with explicit `.ts` extensions
because Node's type stripping requires them; the build sets
`rewriteRelativeImportExtensions` so the emitted `.js` imports `./env.js`
instead.

## Implementation notes

- `.json` is parsed by `JSON.parse`, so trees match a stock JSON parse exactly.
  Because `JSON.parse` is silently last-wins on duplicate keys, the accepted
  document is then re-scanned at token level to reject them (`E_PARSE`,
  SPEC §2).
- YAML uses the `yaml` package's YAML 1.2 core schema, so `yes`/`no`/`on`/`off`
  are strings and an unquoted `010` is the decimal integer 10; unresolvable
  (custom) tags, duplicate keys, non-string mapping keys, multi-document
  streams, and non-finite numbers are `E_PARSE`.
- Alias expansion is bounded by a real node budget (`E_PARSE` past 1,000,000
  nodes, SPEC §2), not by the `yaml` package's `maxAliasCount`, whose default
  counts alias *references* — that rejects honest heavy reuse while saying
  nothing about expanded size, so it is disabled (`maxAliasCount: -1`).
  `toJS` then returns a cheap shared graph (every alias to one anchor is the
  same object), and the normalization walk that rebuilds it into a plain tree
  charges SPEC §2's rule as it goes: one per scalar value, and one per
  sequence element or mapping entry *plus* that element value's own count,
  with mapping keys charged nothing. So an
  alias bomb fails after a million nodes rather than materializing tens of
  millions, and the same walk still catches an alias that resolves to a cycle.
- TOML datetimes are converted to their RFC 3339 string form, preserving the
  authored shape (offset date-time, local date-time, local date, local time).
  `smol-toml`'s `TomlDate.toISOString()` keeps the source's own spelling and
  always renders milliseconds, so the result is normalized to the single
  rendering SPEC §2 pins: uppercase `T` separator, a UTC offset (`Z`, `z`, or
  `+00:00`) as `Z`, any other offset in its authored numeric form, and
  fractional seconds with trailing zeros dropped.
- Files are decoded with a `fatal: true` `TextDecoder` rather than
  `readFileSync(path, "utf8")`, which silently substitutes U+FFFD: content
  that is not valid UTF-8 is `E_PARSE` (SPEC §2).
- Whole-value typing only produces a number when the substituted text parses
  to a *finite* double, so `1e400` — which `Number()` turns into `Infinity` —
  stays the string `"1e400"` (SPEC §6).
