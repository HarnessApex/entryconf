# entryconf Specification

**Version 0.3.0**

entryconf defines how a *config directory* is loaded into a single tree.
Implementations in any language MUST produce identical results for identical
inputs. Conformance is defined by the fixture suite (§8): an implementation is
conformant iff it passes every case in `testdata/cases/`.

The key words MUST, MUST NOT, SHOULD, and MAY are to be interpreted as in
RFC 2119.

## 1. Overview

A config directory contains:

- exactly one **entrypoint** file (§3),
- zero or more `*.env` **variable files** (§4),
- any other files, which are ignored unless referenced via `@file:` (§5).

`Load(dir)` proceeds in this order:

1. Locate the entrypoint (§3).
2. Build the variable namespace from `*.env` files and the process environment (§4).
3. Parse the entrypoint and resolve every `@file:` include, recursively (§5).
4. Interpolate `$` variable references across the assembled tree (§6).
5. Return the tree.

Every failure is a hard error at load time (§7). Implementations MUST NOT skip
an unreadable or unparseable file and MUST NOT return a partial result.

## 2. Data model

The loaded tree is JSON-equivalent: `null`, boolean, number, string, array,
object (string keys). Format-specific rules:

- **YAML** MUST be parsed with the YAML 1.2 core schema. Custom tags are
  `E_PARSE`. Anchors and aliases are resolved at parse time and produce plain
  values; aliases that form a cycle are `E_PARSE`. Alias expansion is bounded:
  a document whose fully-expanded tree would exceed 1,000,000 nodes is
  `E_PARSE` — this stops alias bombs while leaving orders of magnitude of
  headroom over any plausible config. Nodes are counted recursively: a scalar
  value counts one; a sequence or mapping contributes one per element or entry
  **plus** each element value's own count; mapping keys are not counted (so a
  sequence of 1,000 scalars counts 2,000: each element once as an entry slot
  and once as a scalar). Core-schema resolution note: unquoted
  `010` is the decimal integer 10 (YAML 1.2 has no leading-zero octal form;
  octal is spelled `0o10`). A file MUST contain at most one document — a
  multi-document stream is `E_PARSE`. An empty document parses as `null`.
- **TOML** datetime values MUST be converted to RFC 3339-style strings: the
  date/time separator is uppercase `T`; a UTC offset (`Z`, `z`, or `+00:00` in
  the source) is written as `Z`; any other offset keeps its authored numeric
  form; fractional seconds drop trailing zeros (and the `.` when the fraction
  reaches zero); local date-times, local dates, and local times keep their
  offset-less grammar fragment unchanged. Input MUST be valid TOML 1.0: in
  particular, a time carrying an offset without a full date is not a TOML
  value and is `E_PARSE` even where a lenient parser accepts it.
- A duplicate key within one document is `E_PARSE`.
- Mapping keys MUST be strings; a non-string key is `E_PARSE`. A value with no
  JSON-equivalent form (YAML `.inf`/`.nan`) is `E_PARSE`.
- Numbers MUST be handled with at least IEEE-754 double range and precision;
  implementations SHOULD keep integers exact where the host type allows.
  Integers of magnitude above 2^53 are outside the portable guarantee and
  never appear in fixtures.
- An entrypoint or `*.env` file that cannot be read, or file content anywhere
  that is not valid UTF-8, is `E_PARSE`. (A missing or unreadable `@file:`
  target remains `E_INCLUDE`, §5.)

## 3. Entrypoint

The entrypoint is the file named `entrypoint.json`, `entrypoint.yaml`,
`entrypoint.yml`, or `entrypoint.toml`, directly in the config directory.

- None present → `E_NO_ENTRYPOINT`.
- More than one present → `E_MULTIPLE_ENTRYPOINTS`.

The entrypoint document's top-level value MUST be an object; anything else —
including an empty document — is `E_PARSE`. (Included files may hold any
value, §5.) A config directory that does not exist or cannot be read is
`E_NO_ENTRYPOINT`.

## 4. Variables

All files matching `*.env` directly in the config directory (non-recursive)
are loaded. Variable files are **unordered peers**: the same name defined in
two files, or twice within one file, is `E_ENV_CONFLICT`, and the error SHOULD
name the variable and the file(s).

**File format** (a strict subset of dotenv): each line is either blank, a
comment starting with `#`, or `NAME=value` where `NAME` matches
`[A-Za-z_][A-Za-z0-9_]*`. The value is the text after the first `=`, trimmed
of surrounding whitespace, then unquoted if wrapped in matching single or
double quotes. Any other line is `E_PARSE`. All values are strings.

Lines are trimmed of surrounding whitespace before classification, so
indentation is allowed; whitespace between `NAME` and `=` is not (`FOO = bar`
is `E_PARSE`). The `*.env` pattern matches every file name ending in `.env`,
including the bare name `.env`. A variable defined with an empty value is
**set** — `${NAME:-default}` (§6) never applies to it.

**Process environment**: a variable set in the process environment overrides
any `*.env` definition of the same name. (This is the deployment escape hatch:
`DB_HOST=x ./app` must always win.)

There is a **single global namespace** for the entire include tree: included
files (§5) see exactly the same variables as the entrypoint. Only the config
directory's own `*.env` files participate, regardless of where included files
live.

## 5. Includes: `@file:`

A string value that is exactly `@file:<path>` is replaced by the parsed tree
of the referenced file. Only string **values** are examined: object keys are
never treated as includes, escapes, or reserved directives — a key beginning
with `@` stays literal, just as keys are exempt from `$` interpolation (§6).

- `<path>` is resolved **relative to the directory of the file containing the
  reference** (not the entrypoint, not the working directory).
- The file extension selects the parser: `.json`, `.yaml`, `.yml`, `.toml`,
  matched case-sensitively (`.JSON` is not recognized). Any other extension is
  `E_INCLUDE`.
- Includes work at any value position: object field, array element, nested
  arbitrarily deep.
- Included files may themselves contain `@file:` references. The same file MAY
  be included more than once, but a file that transitively includes itself is
  `E_INCLUDE_CYCLE`; the error SHOULD report the full chain.
- A missing or unreadable target is `E_INCLUDE`; an unparseable target is
  `E_PARSE`.
- Included files are plain documents: they have no entrypoint semantics, and
  `*.env` files cannot be included.

**Escaping**: a string whose first character is `@` and that is not an include
must be written with a doubled `@`: a leading `@@` is replaced by a literal
`@` and the string is never treated as an include (`"@@file:x"` →
`"@file:x"`). A string starting with `@` that is neither `@file:<path>` nor
`@@…` is `E_SUBSTITUTION` (reserved for future directives). The unescaped
string is never re-examined for `@file:`, but like any other string it is
still subject to `$` interpolation (§6).

## 6. Interpolation: `$`

After all includes are resolved, every **string value** in the tree is
scanned. Object keys are never interpolated.

| Form | Meaning |
|---|---|
| `${NAME}` | value of `NAME`; `E_MISSING_VAR` if unset |
| `${NAME:-default}` | value of `NAME`, or the literal text `default` if unset |
| `$NAME` | shorthand; `NAME` is the longest run of `[A-Za-z0-9_]` starting with a letter or `_` |
| `$$` | a literal `$` |

Any other use of `$` (e.g. a trailing `$`, `${}` with an empty name, or any
`${NAME:…}` form other than `:-`) is `E_SUBSTITUTION`. The `${NAME:…}`
namespace is reserved for future extensions. Strictness is deliberate: it
catches typos and keeps implementations from diverging on ambiguous input.

The `default` text is literal — no nested substitution.

**Whole-value typing**: if a string consists of exactly one reference (with or
without a default) and nothing else, and the substituted result is exactly
`true`, `false`, `null`, or a valid JSON number that parses to a finite
IEEE-754 double, the value becomes that typed scalar; otherwise it remains a
string (an overflowing number such as `1e400` stays a string). A reference
embedded in a longer string always yields a string.

**Substitution results are inert**: text produced by substitution is never
re-scanned for `@file:` or `$` forms.

## 7. Errors

Error **codes** are normative; messages are not. Implementations SHOULD expose
the code programmatically.

| Code | Condition |
|---|---|
| `E_NO_ENTRYPOINT` | no entrypoint file in the directory |
| `E_MULTIPLE_ENTRYPOINTS` | two or more entrypoint files |
| `E_PARSE` | malformed config, env, or included file |
| `E_ENV_CONFLICT` | a variable defined more than once across/within `*.env` files |
| `E_INCLUDE` | `@file:` target missing, unreadable, or unsupported extension |
| `E_INCLUDE_CYCLE` | a file transitively includes itself |
| `E_MISSING_VAR` | reference to an unset variable with no default |
| `E_SUBSTITUTION` | malformed `$` or `@` form |

These are the *load* codes. Editing (§10) adds six more in §10.9; a candidate
configuration that fails to load during a plan reports one of the codes above.

## 8. Conformance suite

Each directory under `testdata/cases/` is one case:

```
cases/<name>/
  config/               the config directory to load
  procenv.json          optional: process env vars the harness MUST set
  expected.json         success cases: the expected tree
  expected_error.txt    failure cases: the expected error code
```

Harness contract:

- If `procenv.json` exists, set exactly those variables for the case.
- Ensure every variable name appearing in a case's files is otherwise unset in
  the real environment.
- Compare `expected.json` by structural equality; numbers compare numerically
  (`8080` equals `8080.0`).
- For failure cases, the load MUST fail with the code in `expected_error.txt`.

## 9. API guidance (non-normative)

Expose a single `Load(dir)` in language-idiomatic casing (`entryconf.Load`,
`entryconf.load`), returning the tree as the language's natural map type
and/or unmarshaling into user-defined structures per local idiom.

The editing surface of §10 is exposed alongside it as `Open(dir)` returning a
snapshot with `Inspect`, `Plan`, and `Commit` operations in local idiom.
`Load(dir)` MUST keep its 0.2.0 behavior exactly; the editing surface is
additive and a program that never calls it is unaffected by §10.

## 10. Editing

Sections 1–8 define how a config directory is *read*. This section defines
how a source document in it is *edited* so that the four implementations
agree on what every edit means, what it may touch, and when it must refuse.

### 10.1 Scope and guarantee boundary

- **Writable formats.** In this version the only writable source document
  format is **JSON** (`.json`). YAML and TOML documents and `*.env` files are
  read exactly as before but are not writable: an edit naming one is
  `E_UNSUPPORTED_EDIT` and no file changes. The process environment is never a
  writable target. Implementations MUST NOT rewrite a YAML or TOML file (which
  would lose comments and formatting) as a fallback.
- **One document per commit.** A plan edits exactly one source document. A
  batch of edits within that document is applied atomically — all of them
  land or none does — but there is no transaction across several files. A
  request naming more than one document is `E_UNSUPPORTED_EDIT`, detected
  before anything is written.
- **Unedited files are untouched.** Every file other than the edited document
  MUST remain byte-for-byte unchanged, including shared includes, `*.env`
  files, and the entrypoint when it is not the edited document.
- **Advisory locking.** Cooperating writers (implementations of this section)
  serialize through the lock protocol of §10.7 and re-verify revisions before
  writing. That is *not* a compare-and-swap against arbitrary writers: an
  editor that does not take the lock can still write between the revision
  check and the rename. Implementations MUST NOT describe the revision check
  as protection against non-cooperating writers.

### 10.2 Snapshot and documents

`Open(dir)` performs the load of §1 and returns a **snapshot** holding:

- the **effective tree** — identical to what `Load(dir)` returns;
- the **documents**: one entry per file the load read — the entrypoint, every
  `@file:` target, and every `*.env` file. A document has:
  - a **key**: its path relative to the config directory, `/`-separated,
    lexically normalized (`.` and `..` segments resolved; a file outside the
    directory keeps leading `..` segments; if no relative form exists, the
    absolute path). Keys identify documents in every other operation.
  - a **format**: `json`, `yaml`, `toml`, or `env`.
  - a **revision**: the string `sha256:` followed by the lowercase hex
    SHA-256 of the file's bytes. Revisions are comparable across
    implementations.
  - **writable**: true iff the format is `json`.
  - **grafts**: every position of the effective tree the document's root
    value occupies (§10.3). The entrypoint has exactly one graft at the root
    pointer `""`; a document included *n* times has *n* grafts; a `*.env` file
    has none.
- the **variable capture**: the values the process environment held at open
  time. Plans built from the snapshot resolve variables against this capture,
  never against the live environment (§10.6).

A snapshot is immutable and reads nothing from disk after `Open` returns.

### 10.3 Pointers, grafts, and provenance

**Pointers.** Every structural path in this section is an RFC 6901 JSON
Pointer: `""` is the root, `/a/0/b~1c` addresses key `a`, index `0`, key
`b/c`. A pointer over the **effective tree** is an *effective pointer*; a
pointer over a source document's parsed value is a *source pointer*. A pointer
that is malformed (does not start with `/`, or contains `~` not followed by
`0` or `1`) is `E_PATH`. Array index tokens are decimal digits without leading
zeros; `-` is accepted only where §10.5 says so.

**Grafts.** A graft is `{effective, chain}`: the effective pointer at which a
document's root value sits, and the **chain** of `@file:` references
traversed from the entrypoint to reach it, outermost first. Each chain
element is a **reference** `{document, pointer}`: the key of the document that
holds the `@file:` string and that string's source pointer.

**Provenance.** `Inspect(snapshot, effectivePointer)` returns the **origin**
of an effective value:

| Field | Meaning |
|---|---|
| `effective` | the pointer asked about |
| `document` | key of the document whose *authored* value produces this effective value |
| `pointer` | source pointer of that authored value within `document` |
| `authored` | the authored value itself, as parsed, before include or interpolation |
| `chain` | the references traversed from the entrypoint to `document` (empty for the entrypoint) |
| `variables` | the variables the effective value depends on (below), empty for a non-string |
| `writable` | whether `document` is writable |

The origin is found by walking the effective pointer token by token from the
entrypoint's parsed value while tracking the current document and source
pointer. Before consuming a token, and again after the last one, a current
value that is a `@file:` include string is followed: the reference is appended
to the chain and the walk continues at the target document's root. A token
that names a missing key, an out-of-range index, or descends into a scalar is
`E_PATH`.

Each entry of `variables` is `{name, origin, file}` for each `$` reference in
the authored string, in order of appearance: `origin` is `process` when the
process environment supplies the value, `file` when a `*.env` file does (and
`file` is that document's key), or `default` when the `${NAME:-default}`
default applies. An effective value whose authored form is a string with no
`$` reference has an empty `variables` list. Because interpolation never
alters structure, a value with variables is always a scalar and the
substituted text is never re-scanned, provenance is exact: there is no
guessing a reverse mapping from an effective value to a variable.

Two effective pointers that reach the same document through different chains
are reported as such, each with its own chain; the document's `grafts` list
is the complete set. Inspecting is how a caller *discovers* which document
authors a value; it never edits, and no operation in this section chooses a
document on the caller's behalf.

### 10.4 Edit requests

An **edit** is `{document, op, pointer, value, mode}`:

- `document` — the key of the document to edit. Required; there is no
  default. A key not present in the snapshot is `E_EDIT`. A key whose format
  is not writable is `E_UNSUPPORTED_EDIT`.
- `op` — `set` or `remove`. Anything else is `E_EDIT`.
- `pointer` — a source pointer within `document`.
- `value` — for `set`, any JSON-equivalent value; absent for `remove`.
- `mode` — for `set`: `literal` (the default) or `expression`.

A **plan request** is a non-empty list of edits. An empty list is `E_EDIT`.
All edits in one request MUST name the same document; otherwise the request
is `E_UNSUPPORTED_EDIT` (§10.1). Edits apply in order to the document's
parsed value.

### 10.5 Edit semantics

**`set`.** The pointer's parent path is walked from the root:

- an object step whose key is missing creates an empty object at that key and
  continues (missing parents are created as objects, never as arrays);
- an array step must name an existing index;
- a step into a scalar (including an include reference string, which is a
  scalar in its own document) is `E_PATH`.

At the final token: on an object, the member is added or replaced; on an
array, an index in `0..len-1` replaces that element and `len` or `-` appends;
anything else is `E_PATH`. The root pointer `""` replaces the document's whole
value. Setting a value that already equals the authored value is not an error;
the document is still rewritten in normalized form (§10.8).

**`remove`.** Every step of the pointer must exist (nothing is created). On an
object the member is deleted; on an array the element is deleted and later
elements shift down. A missing member or index, `-`, or the root pointer is
`E_PATH`. Removing a member is distinct from setting it to `null`: the first
deletes the key, the second keeps the key with a null value.

**Literal mode** writes the value so that it *loads back as itself*. Every
string in the value — recursively through arrays and objects, but never object
keys — is escaped: each `$` becomes `$$`, then a leading `@` gains a second
`@`. So the literal string `$HOME` is authored as `$$HOME`, `@file:x` as
`@@file:x`, and `@$x` as `@@$$x`; loading yields the original strings. Numbers,
booleans, and null are written unchanged.

**Expression mode** writes the value verbatim as an authored entryconf
expression. The value MUST be a string; any other value is `E_EDIT`. The
string is interpreted by the ordinary load (§5, §6) when the candidate is
validated, so a malformed expression fails the plan with that load's code
(`E_SUBSTITUTION`, `E_MISSING_VAR`, `E_INCLUDE`, …), and no file changes.

Edits never rewrite values they do not name: an authored `${VAR}` elsewhere in
the document is carried through untouched, and its resolved value is never
written back over it as a side effect. A value that *is* named — for example
a `${MODE}` reference replaced with a literal or a different expression — is
replaced exactly as requested.

### 10.6 Plans

`Plan(snapshot, edits)` validates the request (§10.4), applies the edits to a
copy of the document's parsed value (§10.5), serializes the result (§10.8),
and evaluates the **candidate**: the full load of §1 with the edited
document's new text substituted in memory and every other document taken from
the snapshot. A document the candidate references that the snapshot does not
hold (a newly written `@file:` reference) is read from disk and becomes part
of the plan's dependencies. Any failure of that load — a non-object entrypoint
root (`E_PARSE`), a missing include (`E_INCLUDE`), an unset variable
(`E_MISSING_VAR`), a malformed expression (`E_SUBSTITUTION`) — fails the plan
with that code. Planning MUST NOT change any file.

A plan carries:

| Field | Meaning |
|---|---|
| `document` | the edited document's key |
| `before` | the document's text as held by the snapshot |
| `after` | the document's new text (§10.8) |
| `candidate` | the candidate effective tree |
| `affected` | the effective pointers where `candidate` differs from the snapshot's tree (below) |
| `grafts` | the edited document's grafts — every effective position the change lands in |
| `revisions` | `{key: revision}` for every document the candidate load read, `*.env` files included |
| `variables` | the names of every variable the candidate load looked up |
| `variables_revision` | a fingerprint of those variables' values (below) |

**Affected pointers** are the shallowest effective pointers at which the two
trees differ: two objects are compared member by member (a member present in
only one is affected at its own pointer); two arrays of equal length element
by element; two arrays of different lengths, two values of different kinds, or
two unequal scalars are affected at their own pointer. Numbers compare
numerically. The list is sorted by code point. An edit to a document grafted
*n* times therefore surfaces at *n* effective locations; an edit that leaves
the effective tree unchanged has an empty list.

**Variables fingerprint.** For the names in `variables` sorted by code point,
concatenate `=NAME=VALUE\n` for a name that resolved to a value (whether from
the process capture or a `*.env` file) and `-NAME\n` for one that was unset
(its default applied); `variables_revision` is `sha256:` plus the lowercase
hex SHA-256 of that UTF-8 text.

The candidate is evaluated against the snapshot's variable capture, so a
caller inspects and validates exactly the tree that a successful commit will
produce. A caller MAY run its own validation (a schema, a policy) on
`candidate` before committing; the library imposes none.

### 10.7 Commit

`Commit(plan)` writes `after` to the plan's document, or fails without
changing any file. In order:

1. **Lock.** Acquire the directory lock (below). Failure to acquire it within
   the caller's timeout is `E_LOCKED`.
2. **Recheck.** Re-read every document in `revisions` from disk and compare
   revisions; a mismatch, or a document that has disappeared, is
   `E_STALE_PLAN`. Rebuild the variable namespace (§4) from the re-read
   `*.env` files and the *live* process environment, resolve the plan's
   `variables`, and compare the fingerprint; a mismatch is `E_STALE_PLAN`.
   Because every input the candidate depended on is thereby verified
   unchanged, the committed text is guaranteed to load to exactly the
   validated candidate, and the candidate is not re-evaluated.
3. **Write.** Write `after` to a temporary file in the same directory as the
   target, flush it to stable storage, give it the target's permission bits,
   and atomically rename it over the target. On any failure remove the
   temporary file and report `E_WRITE`; the target is unchanged.
4. **Unlock** and return the document's new revision.

**Lock protocol.** The lock is the file `.entryconf.lock` directly in the
config directory, created with an exclusive-create operation (`O_EXCL`;
`CREATE_NEW` on Windows), which every mainstream platform and language can
perform without native extensions. Its content is informational (an
implementation SHOULD write its process id and an RFC 3339 timestamp). A
writer that finds the file present retries at short intervals until its
timeout (the default SHOULD be a few seconds). A lock file whose modification
time is more than 30 seconds old is presumed abandoned by a crashed writer: a
waiter MAY break it by renaming it to a unique name and deleting that, then
retrying — the rename ensures at most one waiter breaks a given stale lock.
The lock is released by deleting the file. The lock file is not an
entrypoint, not a `*.env` file, and never part of the tree; `Load` ignores it.
Exclusive-create is not reliable on some network filesystems (notably NFSv3);
that is a documented limitation, not something an implementation can detect.

**Symlinks.** If the target document is a symbolic link, the write replaces
the file the link *resolves to* (the temporary file is created in that file's
directory and renamed over it), so the link itself survives. Because rename
replaces the inode, other hard links to the old file keep the old content.
Whether a document lives outside the config directory (a `../shared.json`
include) does not change any of this, but the lock protects only writers of
the *same* config directory; two directories sharing an include are not
serialized against each other.

**Durability.** The temporary file is flushed (`fsync`) before the rename; the
directory is flushed after it where the platform allows (POSIX). On Windows
the rename uses `MoveFileEx` with `MOVEFILE_REPLACE_EXISTING`, which is atomic
with respect to readers on NTFS but the directory is not separately flushed;
permission bits are not a Windows concept and are not preserved there. File
ownership is never preserved (it would require privilege). A crash before the
rename leaves the target unchanged and possibly a stray `.*.entryconf-tmp-*`
file; a crash after it leaves the new content.

### 10.8 Serialization

The edited document is written as normalized JSON. Authored formatting and
member order are **not** preserved; this is a documented consequence of
editing through a parsed value rather than a lossless representation.

- UTF-8 without a byte-order mark; LF line endings; one trailing newline.
- Two-space indentation; every object member on its own line as
  `"key": value`; every array element on its own line; the empty object and
  array are `{}` and `[]`.
- Object members sorted by key, by Unicode code point.
- Strings escape `"` as `\"`, `\` as `\\`, and the control characters U+0000
  through U+001F as `\b`, `\f`, `\n`, `\r`, `\t` where those exist and
  `\u00XX` (lowercase hex) otherwise; every other character, including
  non-ASCII, is written literally.
- Integral numbers of magnitude below 2^53 are written as plain integers with
  no fraction or exponent. Other finite numbers are written in the shortest
  decimal form that round-trips; the exact spelling of an exponent MAY differ
  between implementations.

Two implementations given the same parsed value SHOULD produce identical
bytes, and MUST when the document holds only strings, booleans, null, and
integers.

### 10.9 Editing errors

| Code | Condition |
|---|---|
| `E_UNSUPPORTED_EDIT` | the named document is not a writable format (YAML, TOML, `*.env`), or one request names more than one document |
| `E_EDIT` | an invalid edit request: unknown document key, unknown `op`, a value that is not JSON-equivalent, `expression` mode with a non-string value, empty request |
| `E_PATH` | a malformed pointer, or one that cannot be resolved or applied (§10.3, §10.5) |
| `E_STALE_PLAN` | at commit, a dependency's revision or the variables fingerprint differs from the plan's |
| `E_LOCKED` | the directory lock could not be acquired within the timeout |
| `E_WRITE` | the temporary file could not be created or written, or the rename failed; the target is unchanged |

A candidate that fails to load reports the *load* code (§7), never one of
these, so a caller can tell a rejected configuration (`E_PARSE`,
`E_MISSING_VAR`, …) from a conflict (`E_STALE_PLAN`) from an unsupported
operation (`E_UNSUPPORTED_EDIT`). `E_LOCKED` and `E_WRITE` depend on
concurrency and filesystem state that a fixture cannot carry; each
implementation covers them with unit tests (§11).

## 11. Editing conformance suite

Each directory under `testdata/editcases/` is one editing case:

```
editcases/<name>/
  config/               the initial config directory
  procenv.json          optional: process env vars the harness MUST set
  request.json          the operation to perform (below)
  expected.json         success cases: the expected outcome
  expected_error.txt    failure cases: the expected error code
```

`request.json` is one of:

- `{"inspect": "<effective pointer>"}` — `Inspect`; `expected.json` is
  `{"origin": <origin as in §10.3>}`.
- `{"documents": true}` — list the snapshot's documents; `expected.json` is
  `{"documents": {"<key>": {"format", "writable", "grafts"}}}` (revisions are
  omitted: they depend on checkout line endings).
- `{"edits": [<edit>…], "before_commit": {…}}` — `Plan` then `Commit`. The
  optional `before_commit` is applied by the harness *between* the two:
  `"files": {"<key>": "<text>"}` overwrites files in the config directory
  and `"procenv": {"NAME": "value"}` changes the process environment.
  `expected.json` is `{"tree", "affected", "documents": {"<key>": <value>}}`:
  the effective tree after the commit (which MUST equal the plan's
  candidate), the plan's affected pointers, and the parsed value of every
  document the commit wrote (exactly one).

Harness contract, in addition to §8's:

- Run every case against a fresh copy of `config/`, never the checked-in
  fixture.
- After the operation, every file the case did not expect to be written MUST
  be byte-for-byte identical to the copy — or, for a file `before_commit`
  overwrote, to the text `before_commit` gave it — and no file may have been
  added (no lock file, no temporary file). For a failure case that means every
  file.
- Compare `documents` values structurally (numbers numerically), not
  byte-for-byte: §10.8 allows implementations to differ in the spelling of
  non-integral numbers.
