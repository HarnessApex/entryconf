# entryconf (Python)

Implements the [entryconf spec](../SPEC.md) 0.3.0.

Loads a config directory — one `entrypoint.{json,yaml,yml,toml}`, its `*.env`
peers, `@file:` includes and `$VAR` interpolation — into a single tree, and
edits the JSON source documents in it (SPEC §10).

## Install

Requires Python 3.11+ (`tomllib` is stdlib); the only dependency is PyYAML.

```sh
pip install -e .            # from this directory
pip install -e . pytest     # to run the conformance suite
```

## Use

```python
import entryconf

cfg = entryconf.load("envs/deploy")   # -> dict (JSON-equivalent values inside)
```

Every failure raises `entryconf.EntryconfError`, whose `.code` is the
normative `E_*` code from SPEC §7:

```python
try:
    cfg = entryconf.load("envs/deploy")
except entryconf.EntryconfError as exc:
    if exc.code == "E_MISSING_VAR":
        ...
```

`entryconf.load` and `entryconf.EntryconfError` are unchanged from 0.2.0; a
program that only reads is unaffected by the editing surface below.

## Edit (SPEC §10)

Editing goes through a **snapshot** of the directory: open it, inspect where a
value comes from, plan a batch of edits to **one explicitly named JSON
document**, validate the candidate tree, then commit.

```python
import entryconf

snap = entryconf.open("envs/deploy")          # Snapshot: tree + documents + env capture
snap.tree                                     # exactly what load() returns
snap.documents["ephoros.json"].writable       # True only for .json documents

origin = snap.inspect("/ephoros/sessions/task/permission_mode")
origin.document, origin.pointer, origin.authored   # 'ephoros.json', '/sessions/task/permission_mode', 'ask'
origin.chain          # (Reference(document='entrypoint.toml', pointer='/ephoros'),)
origin.variables      # () — or which $ references supply the value, and from where

plan = snap.plan([
    entryconf.Edit("ephoros.json", "set", "/sessions/task/permission_mode", "auto"),
])
plan.candidate        # the full effective tree the commit will produce — validate it here
plan.affected         # ['/ephoros/sessions/task/permission_mode']
plan.after            # the normalized JSON text that will be written

receipt = plan.commit()                       # or plan.commit(lock_timeout=2.0)
receipt.revision                              # 'sha256:…' of the new file
```

Types (all dataclasses, each with `to_json()` producing the SPEC §11 wire form):

| Type | Fields |
|---|---|
| `Snapshot` | `dir`, `tree`, `documents: dict[str, Document]`; methods `inspect(pointer) -> Origin`, `plan(edits) -> Plan` |
| `Document` | `key`, `path`, `format` (`json`/`yaml`/`toml`/`env`), `revision`, `writable`, `grafts` |
| `Graft` / `Reference` | `effective`, `chain` / `document`, `pointer` |
| `Origin` | `effective`, `document`, `pointer`, `authored`, `chain`, `variables`, `writable` |
| `Variable` | `name`, `origin` (`process`/`file`/`default`), `file` |
| `Edit` | `document`, `op` (`set`/`remove`), `pointer`, `value=None`, `mode="literal"` (or `"expression"`) |
| `Plan` | `document`, `before`, `after`, `candidate`, `affected`, `grafts`, `revisions`, `variables`, `variables_revision`; method `commit(lock_timeout=5.0) -> Receipt` |
| `Receipt` | `document`, `revision`, `revisions` |

Semantics worth knowing (the spec is normative):

- Pointers are RFC 6901 JSON Pointers; `""` is the root.
- Only `.json` documents are writable. Naming a YAML/TOML/`*.env` document is
  `E_UNSUPPORTED_EDIT` and nothing is written; a plan edits exactly one file.
- **Literal mode** (default) escapes strings so they load back as themselves:
  setting `"$HOME"` writes `"$$HOME"`. **Expression mode** writes a string
  verbatim as an entryconf expression (`"${MODE:-dev}"`, `"@file:x.json"`);
  the candidate load then validates it, so a bad expression fails the plan with
  the load's code (`E_MISSING_VAR`, `E_SUBSTITUTION`, `E_INCLUDE`, …).
- `plan()` writes nothing. `commit()` takes the directory lock
  (`.entryconf.lock`), rechecks every document's revision and the variables'
  fingerprint against the live environment (`E_STALE_PLAN` on any change), and
  replaces the file atomically (temp file + `os.replace`, mode preserved).
  `E_LOCKED` if the lock is held past `lock_timeout`; `E_WRITE` if the write
  fails, leaving the target untouched.
- The edited file is rewritten in normalized JSON (sorted keys, two-space
  indent). Every other file is byte-for-byte unchanged.

The new codes are exported as constants: `E_UNSUPPORTED_EDIT`, `E_EDIT`,
`E_PATH`, `E_STALE_PLAN`, `E_LOCKED`, `E_WRITE` (SPEC §10.9).

## Command line

```sh
python -m entryconf <config-dir>                          # dump the tree
python -m entryconf inspect <config-dir> [<pointer>]      # provenance, or the document listing
python -m entryconf edit [-n|--dry-run] <config-dir> <request.json | ->
```

The one-argument dump form is unchanged from 0.2.0. `inspect` prints the origin
of one effective pointer, or without a pointer `{"dir", "documents"}` with every
document's key, path, format, revision, writability and grafts. `edit` reads
`{"edits": [...]}` (the SPEC §11 edit shape) from a file or stdin, plans, and
commits unless `-n`; it prints the plan (`document`, `before`, `after`,
`candidate`, `affected`, `grafts`, `revisions`, `variables`,
`variables_revision`) plus `committed` and the new `revision` (or `null`).

Every form follows the CLI convention shared by every implementation:

| Outcome | stdout | stderr | exit |
|---|---|---|---|
| success | JSON, object keys sorted | — | 0 |
| load or editing failure (`E_STALE_PLAN`, `E_LOCKED`, …) | — | the bare `E_*` code, first line | 1 |
| usage or internal fault | — | a message with no `E_*` code | 2 |

So exit 1 always means "here is the verdict, and here is the code"; exit 2
always means "the tool was misused or broke", and can never be mistaken for a
conformance verdict.

The dump form's argument is a directory, never an option: `--help`/`-h` and `--version`
print and exit 0, and any other dash-led argument is a usage fault (exit 2)
rather than a directory named `-x`. To load a directory whose name does start
with `-`, prefix it with `./` — `python -m entryconf ./-weird-dir`.

## Tests

```sh
pytest
```

`test_conformance.py` walks the read fixtures in `../testdata/cases` and
`test_editcases.py` walks the editing fixtures in `../testdata/editcases`
(SPEC §11) — each case against a fresh copy of its `config/`, asserting that
the committed tree equals the plan's candidate, that untouched files are
byte-identical, and that no lock or temporary file remains. Both are one
parametrized test per suite, one subtest per case directory, per the harness
contract in SPEC §8. It sets each
case's `procenv.json` variables and unsets every variable name that appears in
the case's files, so no real environment variable can leak in; because that
mutates the process environment, the cases are serialized under a lock.

Unit tests cover what a fixture cannot: a config directory that does not
exist (`E_NO_ENTRYPOINT` — git cannot store a missing directory), the CLI's
exit-code convention, the YAML expansion budget's boundary, and the editing
contracts of SPEC §10.7/§10.9 — two cooperating writers (`E_STALE_PLAN`), lock
contention and stale-lock breaking (`E_LOCKED`), permission preservation, a
failing write leaving the source intact with no temporary file (`E_WRITE`),
symlinked documents, and the serializer's byte rules.
