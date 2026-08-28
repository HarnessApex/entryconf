# entryconf (Python)

Implements the [entryconf spec](../SPEC.md) 0.2.0.

Loads a config directory — one `entrypoint.{json,yaml,yml,toml}`, its `*.env`
peers, `@file:` includes and `$VAR` interpolation — into a single tree.

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

The public surface is exactly `entryconf.load` and `entryconf.EntryconfError`.

## Dump (cross-implementation checking)

```sh
python -m entryconf <config-dir>
```

Follows the dump-CLI convention shared by every implementation:

| Outcome | stdout | stderr | exit |
|---|---|---|---|
| success | the tree as JSON, object keys sorted | — | 0 |
| load failure | — | the bare `E_*` code, first line | 1 |
| usage or internal fault | — | a message with no `E_*` code | 2 |

So exit 1 always means "the config was rejected, and here is the code"; exit 2
always means "the tool was misused or broke", and can never be mistaken for a
conformance verdict.

The argument is a directory, never an option: `--help`/`-h` and `--version`
print and exit 0, and any other dash-led argument is a usage fault (exit 2)
rather than a directory named `-x`. To load a directory whose name does start
with `-`, prefix it with `./` — `python -m entryconf ./-weird-dir`.

## Tests

```sh
pytest
```

One parametrized test walks the shared fixtures in `../testdata/cases`, one
subtest per case directory, per the harness contract in SPEC §8. It sets each
case's `procenv.json` variables and unsets every variable name that appears in
the case's files, so no real environment variable can leak in; because that
mutates the process environment, the cases are serialized under a lock.

A handful of unit tests cover what a fixture cannot: a config directory that
does not exist (`E_NO_ENTRYPOINT` — git cannot store a missing directory), the
CLI's exit-code convention, and the YAML expansion budget's boundary.
