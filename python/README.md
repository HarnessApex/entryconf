# entryconf (Python)

Implements the [entryconf spec](../SPEC.md) 0.1.0.

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

Prints the loaded tree as JSON (object keys sorted) on stdout and exits 0; on
failure prints just the `E_*` code on stderr and exits 1. A missing or extra
argument is a usage error on stderr with exit 2.

## Tests

```sh
pytest
```

One parametrized test walks the shared fixtures in `../testdata/cases`, one
subtest per case directory, per the harness contract in SPEC §8. It sets each
case's `procenv.json` variables and unsets every variable name that appears in
the case's files, so no real environment variable can leak in; because that
mutates the process environment, the cases are serialized under a lock.
