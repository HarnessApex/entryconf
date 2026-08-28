# entryconf (Rust)

Implements **entryconf spec 0.2.0** (`../SPEC.md`). Conformance is defined by the
shared fixture suite in `../testdata/cases/`, which this crate's single test
harness walks.

## Install

```toml
[dependencies]
entryconf = "0.2"
```

Or, from inside this repository, as a path dependency:

```toml
[dependencies]
entryconf = { path = "../rust" }
```

Requires a stable Rust toolchain (edition 2021).

## Use

The public surface is one function plus its error type:

```rust
use std::path::Path;

let tree: serde_json::Value = entryconf::load(Path::new("envs/deploy"))?;
println!("{}", tree["database"]["host"]);
# Ok::<(), entryconf::Error>(())
```

`load` returns the loaded tree as a `serde_json::Value` — the natural
JSON-equivalent tree type in Rust — so you can index it, pattern-match it, or
`serde_json::from_value` it into your own structs.

Variables come from the directory's `*.env` files plus this process's
environment, which overrides them (SPEC §4).

### Errors

Every failure is hard and carries one of the eight normative `E_*` codes
(SPEC §7). `Error::code()` gives the code as a string; `Error::kind()` gives it
as an `ErrorCode` enum you can match on exhaustively.

```rust
use std::path::Path;
use entryconf::ErrorCode;

match entryconf::load(Path::new("envs/deploy")) {
    Ok(tree) => println!("{tree}"),
    Err(e) if e.kind() == ErrorCode::MissingVar => eprintln!("unset variable: {}", e.message()),
    Err(e) => eprintln!("{}: {}", e.code(), e.message()),
}
```

## Dump (cross-implementation checking)

`entryconf-dump` loads a config directory and prints the tree as a single line
of JSON on stdout. Object keys are emitted in sorted order, so output is
byte-comparable across runs. It follows the exit-code convention every
implementation's dump CLI shares:

| Outcome | stdout | stderr | Exit |
|---|---|---|---|
| success | the tree, one line of JSON | — | 0 |
| **load** failure | — | the bare `E_*` code as the first line (`-v` adds the non-normative detail on a second) | 1 |
| any other fault (usage, internal) | — | a plain message, never an `E_*` code | 2 |

The split matters for the cross-check: exit 1 with a code is a verdict *about
the config*, and exit 2 means the tool itself was misused, so a broken
invocation can never be mistaken for a conformance result.

```console
$ cargo run --quiet --bin entryconf-dump -- ../testdata/cases/01-basic/config
{"app":"demo","features":["alpha","beta"],"limits":{"burst":null,"rps":100},"port":8080}

$ cargo run --quiet --bin entryconf-dump -- ../testdata/cases/05-env-conflict/config
E_ENV_CONFLICT
$ echo $?
1

$ cargo run --quiet --bin entryconf-dump
usage: entryconf-dump [-v] [--] <config-dir>
$ echo $?
2
```

Or build once and invoke the binary directly:

```console
$ cargo build --release --bin entryconf-dump
$ ./target/release/entryconf-dump <config-dir>
```

The dump binary reads the real process environment, so `EC_HOST=x
entryconf-dump ./config` exercises the override path (SPEC §4).

## Test

```console
$ cargo test
```

`tests/conformance.rs` is the only test: it walks `../testdata/cases/` and turns
each case directory into a named trial (via `libtest-mimic`), so the output
reads `06-include ... ok` per fixture. There are no hand-written per-case tests
that could drift from the shared suite.

Because the harness reads `../testdata`, which lives outside the crate, it is
excluded from the published package (`package.exclude` in `Cargo.toml`) — run
`cargo test` from a checkout of this repository, not from the crates.io tarball.

The harness *injects* the process environment rather than mutating it: each case
sees exactly the variables its `procenv.json` names and nothing else, which
satisfies SPEC §8's "case-named vars are otherwise unset" contract and lets the
trials run in parallel. The seam is `entryconf::load_with_env(dir, &env_map)`
(`#[doc(hidden)]`, not part of the stable surface); the public `load` always
reads `std::env`.

## Implementation notes

- **YAML** is built on `saphyr-parser`'s event stream rather than a document
  loader, so YAML 1.2 **core schema** resolution, custom-tag rejection, and
  duplicate-key detection are all under this crate's control. `on`/`off`/
  `yes`/`no`/`y`/`n` are plain strings; only `true|True|TRUE|false|False|FALSE`
  are booleans. Anchors and aliases resolve to plain values, under a **node
  budget**: the walk charges SPEC §2's counting rule against the
  1,000,000-node bound — a scalar value costs one, a sequence or mapping costs
  one per element or entry *plus* each element value's own count, and a value
  in mapping-key position costs nothing at all, because keys are not counted.
  An alias is charged the whole SPEC §2 count of the anchored
  subtree — read from the anchor table — *before* that subtree is cloned. So a
  layered alias bomb is rejected as `E_PARSE` after work proportional to the
  budget rather than to its 48-million-node expansion (case 57 settles in
  milliseconds); it is an accounted bound, not a timeout.
- **JSON** uses `serde_json`'s parser driven by a custom visitor, because
  `serde_json::Value`'s stock `Deserialize` silently last-wins on duplicate
  keys, which SPEC §2 makes `E_PARSE`.
- **TOML** uses the `toml` crate; duplicate keys are already a hard error in the
  TOML grammar. Datetimes are rendered from `toml::value::Datetime`'s parsed
  fields rather than its `Display`, which diverges from SPEC §2 twice: it writes
  a zero fraction as `.0` instead of dropping it, and it renders an authored
  `+00:00` offset numerically instead of as `Z`. Offsets are preserved, never
  normalized to UTC.
