# entryconf (TypeScript)

Implements the **entryconf spec 0.3.0** (`../SPEC.md`). Conformance is defined
by the shared fixture suites in `../testdata/cases/` (reading) and
`../testdata/editcases/` (editing, SPEC §10–11).

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

`load(dir)` is unchanged from 0.2.0: synchronous — startup config is read once,
before anything else runs — reading the real process environment, which
overrides `*.env` values (SPEC §4). Every failure is an `EntryconfError` with a
normative `code`; the `Tree` / `Value` / `ErrorCode` types describe the result.

## Editing (SPEC §10)

Since 0.3.0 the package can also edit the **JSON** source documents of a config
directory — the entrypoint or any `@file:`-included `.json` file — through a
snapshot / plan / commit cycle. YAML, TOML, and `*.env` files are read as
before but are never rewritten (`E_UNSUPPORTED_EDIT`); the process environment
is never a target.

```ts
import { open } from "entryconf";

const snap = open("envs/deploy");          // effective tree + every source document
snap.tree;                                 // === load("envs/deploy")
snap.documents["ephoros.json"];            // { key, path, format, revision, writable, grafts }

// Which file authors an effective value, and does it depend on a variable?
const origin = snap.inspect("/ephoros/sessions/task/permission_mode");
origin.document;                           // "ephoros.json"   (choose this explicitly)
origin.pointer;                            // "/sessions/task/permission_mode"
origin.chain;                              // [{ document: "entrypoint.toml", pointer: "/ephoros" }]
origin.variables;                          // [] — a literal, not "${MODE}"

// Prepare: nothing is written. The candidate is the full load with the new
// text substituted in memory, so validate it (a schema, a policy) first.
const plan = snap.plan([
  { document: origin.document, op: "set", pointer: origin.pointer, value: "auto" },
]);
plan.candidate;                            // the tree a commit will produce
plan.affected;                             // ["/ephoros/sessions/task/permission_mode"]
plan.after;                                // the normalized JSON text to be written

// Commit: lock, recheck every dependency's revision and the variables the
// candidate used (E_STALE_PLAN on any change), write atomically.
const receipt = plan.commit();             // { document, revision, revisions }
```

### API

```ts
function open(dir: string): Snapshot;

class Snapshot {
  readonly dir: string;                     // absolute
  readonly tree: Tree;
  readonly documents: Record<string, Document>;
  inspect(pointer: string): Origin;         // effective JSON Pointer → E_PATH if unresolvable
  plan(edits: Edit[]): Plan;                // one document per plan
}

interface Document  { key: string; path: string; format: "json"|"yaml"|"toml"|"env";
                      revision: string; writable: boolean; grafts: Graft[] }
interface Graft     { effective: string; chain: Reference[] }
interface Reference { document: string; pointer: string }
interface Origin    { effective: string; document: string; pointer: string; authored: Value;
                      chain: Reference[]; variables: Variable[]; writable: boolean }
interface Variable  { name: string; origin: "process"|"file"|"default"; file?: string }
interface Edit      { document: string; op: "set"|"remove"; pointer: string;
                      value?: unknown; mode?: "literal"|"expression" }

class Plan {
  readonly document: string;  readonly before: string;  readonly after: string;
  readonly candidate: Tree;   readonly affected: string[];  readonly grafts: Graft[];
  readonly revisions: Record<string, string>;   // every document the candidate read
  readonly variables: string[];                 // every variable it looked up
  readonly variablesRevision: string;           // JSON wire form: variables_revision
  commit(options?: { lockTimeoutMs?: number }): Receipt;
  toJSON(): Record<string, unknown>;
}
interface Receipt { document: string; revision: string; revisions: Record<string, string> }
```

Semantics, briefly (the normative text is SPEC §10):

- **Pointers** are RFC 6901. `set` creates missing parents as objects, replaces
  or appends (`/list/-` or the length index) in arrays, and `""` replaces the
  whole document. `remove` requires every step to exist; removing a member is
  not the same as setting it to `null`.
- **`mode: "literal"`** (default) escapes strings so they load back as
  themselves — `"$HOME"` is written `"$$HOME"`, `"@file:x"` is written
  `"@@file:x"`, recursively through arrays and objects. **`mode: "expression"`**
  writes a string verbatim as an entryconf expression (`"${MODE:-dev}"`,
  `"@file:other.json"`), interpreted when the candidate is evaluated.
- Untouched values are carried through verbatim: an unrelated `"${VAR}"` is
  never replaced by its resolved value.
- A document included twice is edited once and both graft positions change;
  `plan.affected` lists every one. Nothing chooses between editing a shared
  include and replacing one reference — the edit names its document.
- Revisions are `sha256:<hex>` of the file bytes. `open` captures the process
  environment; `commit` rechecks against the live one.
- The edited file is rewritten in normalized JSON (two-space indent, keys
  sorted by code point); its authored formatting is not preserved. Every other
  file stays byte-for-byte unchanged.
- Cooperating writers serialize on `<dir>/.entryconf.lock` (exclusive create;
  a lock older than 30 s is presumed abandoned). That is advisory: an editor
  that does not take the lock can still write between the recheck and the
  rename. The write itself is a same-directory temporary file, `fsync`,
  original permission bits, then `rename` — atomic for readers; on a symlinked
  document the link survives and its target is replaced.
- Codes: `E_UNSUPPORTED_EDIT`, `E_EDIT`, `E_PATH`, `E_STALE_PLAN`, `E_LOCKED`,
  `E_WRITE`, plus any load code when the candidate fails to load.

## Command line (cross-implementation checking)

```sh
node src/cli.ts <config-dir>                                # dump the tree
node src/cli.ts inspect <config-dir>                        # documents, revisions, grafts
node src/cli.ts inspect <config-dir> <pointer>              # one value's origin
node src/cli.ts edit [-n|--dry-run] <config-dir> <request.json | ->
```

`edit` reads `{"edits": [...]}` (the SPEC §11 request shape), plans, commits
unless `-n`, and prints the plan's wire form plus `committed` and the new
`revision` (`null` on a dry run). All JSON output has keys sorted recursively.

Exit codes follow the repo-wide dump-CLI convention, so a harness can tell a
conformance result from a broken tool:

| Exit | Meaning |
|---|---|
| 0 | the tree is on stdout as JSON |
| 1 | an entryconf failure (load, plan, or commit) — the bare `E_*` code is the first line on stderr |
| 2 | any other fault (usage, internal) — no `E_*` code is printed |

The dump form takes exactly one positional argument and knows no options, so
a dash-led first argument is a usage fault, never a directory name: `--help`
and `-h` print the usage on stdout and exit 0, anything else starting with `-`
exits 2. Neither ever prints an `E_*` code.

Set process-env variables the usual way:
`EC_HOST=prod node src/cli.ts ./config`.

## Test

```sh
npm test
```

`test/conformance.test.ts` is the read suite: it walks `../testdata/cases`,
runs each case as a subtest named after its directory, sets `procenv.json`
variables while scrubbing every other variable the case's files mention, and
compares `expected.json` structurally with numeric equality for numbers
(`8080` equals `8080.0`). Cases run serially because they mutate
`process.env`.

`test/editcases.test.ts` is the editing suite (SPEC §11): each case runs
against a fresh copy of its `config/`, dispatches the `request.json`
(`inspect`, `documents`, or `edits` with an optional `before_commit` step
between plan and commit), compares `expected.json` structurally, and then
checks that every file the case did not write is byte-identical and that no
lock or temporary file was left behind. Beside it are the unit tests for what a
fixture cannot carry: two cooperating writers, lock contention and stale-lock
breaking, permission preservation, a failing write (`E_WRITE`, source intact,
nothing left behind), a symlinked document, and a live environment change
making a plan stale.

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
