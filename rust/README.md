# entryconf (Rust)

Implements **entryconf spec 0.3.0** (`../SPEC.md`): loading a config directory
into a tree, and — since 0.3.0 — inspecting where each value comes from and
editing JSON source documents (SPEC §10). Conformance is defined by the shared
fixture suites in `../testdata/cases/` (reading) and `../testdata/editcases/`
(editing), which this crate's two test harnesses walk.

## Install

```toml
[dependencies]
entryconf = "0.3"
```

Or, from inside this repository, as a path dependency:

```toml
[dependencies]
entryconf = { path = "../rust" }
```

Requires a stable Rust toolchain (edition 2021).

## Use

### Loading

The read surface is one function plus its error type, unchanged from 0.2.0:

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

### Editing (SPEC §10)

```rust
use std::path::Path;
use entryconf::{CommitOptions, Edit};
use serde_json::json;

// 1. Open: the effective tree, every source document, and the process
//    environment as captured now.
let snap = entryconf::open(Path::new("envs/deploy"))?;
for doc in snap.documents.values() {
    println!("{} ({}, writable: {})", doc.key, doc.format.as_str(), doc.writable);
}

// 2. Inspect: which document authors an effective value, through which
//    includes, and which variables it depends on.
let origin = snap.inspect("/ephoros/sessions/task/permission_mode")?;
assert!(origin.writable);                 // its document is JSON
let target = origin.document.clone();    // e.g. "ephoros.json"

// 3. Plan: a batch of edits to ONE document. Nothing is written. The plan
//    carries the candidate tree — validate it with your own schema first.
let plan = snap.plan(&[
    Edit::set(&target, "/sessions/task/permission_mode", json!("auto")),
    Edit::set(&target, "/notes", json!("costs $5")),          // literal: loads back as "costs $5"
    Edit::set_expression(&target, "/mode", "${MODE:-dev}"),    // expression: interpreted on load
    Edit::remove(&target, "/legacy"),
])?;
println!("{}", plan.candidate["ephoros"]["sessions"]["task"]["permission_mode"]);
println!("{:?}", plan.affected);          // effective pointers that change

// 4. Commit: lock, recheck every dependency's revision and the variables
//    fingerprint, atomic replace. E_STALE_PLAN if anything moved.
let receipt = plan.commit(&CommitOptions::default())?;
println!("{} is now {}", receipt.document, receipt.revision);
# Ok::<(), entryconf::Error>(())
```

Public types: `Snapshot { dir, tree, documents }` with `inspect(&str)` and
`plan(&[Edit])`; `Document { key, path, format: Format, revision, writable,
grafts: Vec<Graft> }`; `Graft { effective, chain: Vec<Reference> }`;
`Reference { document, pointer }`; `Origin { effective, document, pointer,
authored, chain, variables: Vec<Variable>, writable }`; `Variable { name,
origin: VariableOrigin, file }`; `Edit { document, op: Op, pointer, value:
Option<Value>, mode: Mode }` (constructors `Edit::set`, `Edit::set_expression`,
`Edit::remove`; `parse_edits(&Value)` decodes a SPEC §11 request array, reporting
a malformed entry as `E_EDIT`); `Plan { document, before, after, candidate,
affected, grafts, revisions, variables, variables_revision }` with
`commit(&CommitOptions)`; `CommitOptions { lock_timeout }` (default 5 s);
`Receipt { document, revision, revisions }`. All of them implement
`serde::Serialize` in the wire form the CLIs print.

Only JSON documents are writable: naming a YAML, TOML, or `*.env` document is
`E_UNSUPPORTED_EDIT` and nothing is written. The edited document is rewritten in
the normalized form of SPEC §10.8 (two-space indent, keys sorted); every other
file stays byte-for-byte unchanged. Plans resolve variables against the
environment captured by `open`; a commit rechecks the live environment.

### Errors

Every failure is hard and carries a normative `E_*` code: the eight load codes
of SPEC §7 and, since 0.3.0, the six editing codes of SPEC §10.9
(`E_UNSUPPORTED_EDIT`, `E_EDIT`, `E_PATH`, `E_STALE_PLAN`, `E_LOCKED`,
`E_WRITE`). `Error::code()` gives the code as a string; `Error::kind()` gives it
as an `ErrorCode` enum. **Source-compatibility note:** the six new variants
mean an exhaustive `match` on `ErrorCode` written against 0.2.0 no longer
compiles without a wildcard arm — that is the one breaking change in the
crate's surface.

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

## Edit (CLI)

`entryconf-edit` is the editing surface as a command, sharing the library's
behavior rather than implementing a second editor. It prints the same JSON
shapes as the other implementations' editing CLIs, keys sorted, so
`tools/crosscheck` can diff them.

```console
$ entryconf-edit inspect <dir>                     # {"dir", "documents": {key: {...}}}
$ entryconf-edit inspect <dir> /ephoros/mode       # the origin object (SPEC §10.3)
$ entryconf-edit edit -n <dir> request.json        # plan only: prints the plan, "committed": false
$ entryconf-edit edit <dir> -  < request.json      # plan and commit; "revision" is the new one
```

`request.json` is `{"edits": [{"document", "op", "pointer", "value"?, "mode"?}, …]}`
(SPEC §10.4). Exit codes follow the dump convention: 0 success; 1 for any
entryconf error with the bare `E_*` code as the first stderr line and nothing on
stdout (`-v` adds the message); 2 for usage or internal faults, never printing
a code.

## Test

```console
$ cargo test
```

`tests/conformance.rs` walks `../testdata/cases/` and `tests/editcases.rs`
walks `../testdata/editcases/`; each turns a case directory into a named trial
(via `libtest-mimic`), so the output reads `06-include ... ok` per fixture.
There are no hand-written per-case tests that could drift from the shared
suites. The editing harness runs every case against a fresh copy of its
`config/` under `target/` and then checks that no other file changed and no
lock or temporary file was left behind (SPEC §11).

`tests/edit_unit.rs` covers what a fixture cannot carry (SPEC §10.9): two
cooperating writers (the second commit is `E_STALE_PLAN`), a held lock
(`E_LOCKED` within the timeout) and a stale one (broken), permission bits
preserved across a commit, a write that fails in a read-only directory
(`E_WRITE`, source intact, no temporary file), a symlinked document (the link
survives, its target changes), and a live environment change making a plan
stale.

Because the harness reads `../testdata`, which lives outside the crate, it is
excluded from the published package (`package.exclude` in `Cargo.toml`) — run
`cargo test` from a checkout of this repository, not from the crates.io tarball.

The harnesses *inject* the process environment rather than mutating it: each
case sees exactly the variables its `procenv.json` names and nothing else, which
satisfies SPEC §8's "case-named vars are otherwise unset" contract and lets the
trials run in parallel. The seams are `entryconf::load_with_env(dir, &env_map)`
and `entryconf::open_with_env(dir, &captured, live)` (`#[doc(hidden)]`, not
part of the stable surface); the public `load` and `open` always read
`std::env`.

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
- **Editing** keeps every document's bytes and parsed value in the snapshot;
  a plan evaluates its candidate with the edited text substituted in memory
  through the same loader, reading disk only for a file the snapshot never saw
  (a newly written `@file:` reference). Document keys are the paths *as
  referenced*, lexically cleaned relative to the config directory; canonical
  paths are used only for include-cycle detection. The lock is
  `.entryconf.lock`, taken with `create_new`; the replacement is a
  `.<name>.entryconf-tmp-*` file beside the target, `sync_all`'d, given the
  target's mode (unix), and renamed over it, with a best-effort directory
  fsync. Integral floats below 2^53 serialize as integers; other floats use
  Rust's shortest round-trip `Display`, whose exponent spelling SPEC §10.8
  allows to differ between implementations.
