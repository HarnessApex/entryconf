# entryconf Specification

**Version 0.2.0**

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
