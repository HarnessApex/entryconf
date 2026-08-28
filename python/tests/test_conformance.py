"""The conformance harness (SPEC §8).

One parametrized test walks the shared fixture suite in
``../../testdata/cases`` — there are deliberately no hand-written per-case
tests, which could drift from the fixtures.

The unit tests at the bottom cover only what a fixture *cannot* express: a
config directory that does not exist (git cannot store a missing directory),
the dump CLI's exit-code convention, and the boundary of the YAML expansion
budget.
"""

from __future__ import annotations

import contextlib
import json
import os
import re
import subprocess
import sys
import threading
import time
from pathlib import Path
from typing import Any, Iterator

import pytest

from entryconf import EntryconfError, load
from entryconf._parsers import MAX_EXPANDED_NODES, parse_yaml

CASES_DIR = Path(__file__).resolve().parents[2] / "testdata" / "cases"
CASES = sorted(path for path in CASES_DIR.iterdir() if path.is_dir())

_REFERENCE_RE = re.compile(r"\$\{?([A-Za-z_][A-Za-z0-9_]*)")
_ENV_LINE_RE = re.compile(r"^\s*([A-Za-z_][A-Za-z0-9_]*)\s*=", re.MULTILINE)

# Cases mutate the real process environment, so they must not run concurrently.
_ENV_LOCK = threading.Lock()


def _case_variable_names(case: Path) -> set[str]:
    """Every variable name that appears anywhere in a case's files.

    The harness contract requires these to be otherwise unset in the real
    environment, so the harness scrubs them.
    """
    names: set[str] = set()
    procenv = case / "procenv.json"
    if procenv.is_file():
        names |= set(json.loads(procenv.read_text(encoding="utf-8")))
    for path in sorted((case / "config").rglob("*")):
        if not path.is_file():
            continue
        try:
            text = path.read_text(encoding="utf-8")
        except (OSError, UnicodeDecodeError):
            continue
        names |= set(_REFERENCE_RE.findall(text))
        if path.name.endswith(".env"):
            names |= set(_ENV_LINE_RE.findall(text))
    return names


@contextlib.contextmanager
def _process_env(unset: set[str], overrides: dict[str, str]) -> Iterator[None]:
    with _ENV_LOCK:
        saved = dict(os.environ)
        try:
            for name in unset:
                os.environ.pop(name, None)
            os.environ.update(overrides)
            yield
        finally:
            os.environ.clear()
            os.environ.update(saved)


def _is_number(value: Any) -> bool:
    return isinstance(value, (int, float)) and not isinstance(value, bool)


def _mismatch(actual: Any, expected: Any, path: str = "$") -> str | None:
    """Structural comparison; numbers compare numerically (8080 == 8080.0)."""
    if isinstance(expected, bool) or expected is None:
        if actual is not expected:
            return f"{path}: expected {expected!r}, got {actual!r}"
        return None
    if _is_number(expected):
        if not _is_number(actual) or float(actual) != float(expected):
            return f"{path}: expected {expected!r}, got {actual!r}"
        return None
    if isinstance(expected, str):
        if not isinstance(actual, str) or actual != expected:
            return f"{path}: expected {expected!r}, got {actual!r}"
        return None
    if isinstance(expected, list):
        if not isinstance(actual, list):
            return f"{path}: expected an array, got {actual!r}"
        if len(actual) != len(expected):
            return f"{path}: expected {len(expected)} items, got {len(actual)}"
        for index, (got, want) in enumerate(zip(actual, expected)):
            found = _mismatch(got, want, f"{path}[{index}]")
            if found is not None:
                return found
        return None
    if isinstance(expected, dict):
        if not isinstance(actual, dict):
            return f"{path}: expected an object, got {actual!r}"
        if set(actual) != set(expected):
            missing = sorted(set(expected) - set(actual))
            extra = sorted(set(actual) - set(expected))
            return f"{path}: key mismatch (missing {missing}, unexpected {extra})"
        for key, want in expected.items():
            found = _mismatch(actual[key], want, f"{path}.{key}")
            if found is not None:
                return found
        return None
    raise AssertionError(f"unsupported expected value at {path}: {expected!r}")


@pytest.mark.parametrize("case", CASES, ids=[case.name for case in CASES])
def test_conformance(case: Path) -> None:
    config = case / "config"
    procenv_file = case / "procenv.json"
    overrides: dict[str, str] = (
        json.loads(procenv_file.read_text(encoding="utf-8"))
        if procenv_file.is_file()
        else {}
    )
    expected_error = case / "expected_error.txt"

    with _process_env(_case_variable_names(case), overrides):
        if expected_error.is_file():
            code = expected_error.read_text(encoding="utf-8").strip()
            with pytest.raises(EntryconfError) as excinfo:
                load(config)
            assert excinfo.value.code == code
            return
        tree = load(config)

    expected = json.loads((case / "expected.json").read_text(encoding="utf-8"))
    assert _mismatch(tree, expected) is None, _mismatch(tree, expected)


# --------------------------------------------------------------------------
# Unit tests for what a fixture cannot express.
# --------------------------------------------------------------------------


def test_missing_config_dir_is_no_entrypoint(tmp_path: Path) -> None:
    """SPEC §3: a directory that does not exist is `E_NO_ENTRYPOINT`.

    Unfixturable: git cannot store a missing directory, so the suite can never
    cover this and it has to be asserted here.
    """
    with pytest.raises(EntryconfError) as excinfo:
        load(tmp_path / "does-not-exist")
    assert excinfo.value.code == "E_NO_ENTRYPOINT"

    # Same code for a path that exists but is not a readable directory.
    not_a_dir = tmp_path / "entrypoint.json"
    not_a_dir.write_text("{}", encoding="utf-8")
    with pytest.raises(EntryconfError) as excinfo:
        load(not_a_dir)
    assert excinfo.value.code == "E_NO_ENTRYPOINT"


def test_dump_cli(tmp_path: Path) -> None:
    """The dump-CLI convention: 0 with JSON, 1 with a bare code, 2 otherwise."""
    config = tmp_path / "config"
    config.mkdir()
    (config / "entrypoint.json").write_text('{"port": 8080}', encoding="utf-8")

    ok = subprocess.run(
        [sys.executable, "-m", "entryconf", str(config)],
        capture_output=True,
        text=True,
    )
    assert ok.returncode == 0, ok.stderr
    assert json.loads(ok.stdout) == {"port": 8080}

    # A load failure: exit 1, the bare E_* code as the first stderr line.
    bad = subprocess.run(
        [sys.executable, "-m", "entryconf", str(tmp_path / "empty")],
        capture_output=True,
        text=True,
    )
    assert bad.returncode == 1
    assert bad.stderr.splitlines()[0] == "E_NO_ENTRYPOINT"
    assert bad.stderr.strip() == "E_NO_ENTRYPOINT"
    assert bad.stdout == ""

    # Any other fault: exit 2, and no E_* code anywhere on stderr, so it can
    # never be read as a conformance verdict.
    for argv in ([], [str(config), str(config)]):
        usage = subprocess.run(
            [sys.executable, "-m", "entryconf", *argv],
            capture_output=True,
            text=True,
        )
        assert usage.returncode == 2, usage.stderr
        assert "E_" not in usage.stderr
        assert usage.stdout == ""


def test_dump_cli_dash_led_argument_is_a_usage_fault(tmp_path: Path) -> None:
    """A dash-led argument is a mis-invocation, never a config directory.

    Handing `-x` or `--` to `load` would report a mistyped flag as
    `E_NO_ENTRYPOINT`: a broken invocation dressed as a conformance verdict.
    The rule is the dump-CLI convention's own — exit 2, no `E_*` on stderr.
    """
    for argv in (["--"], ["-x"], ["--nope"], ["-"]):
        proc = subprocess.run(
            [sys.executable, "-m", "entryconf", *argv],
            capture_output=True,
            text=True,
            cwd=tmp_path,
        )
        assert proc.returncode == 2, (argv, proc.stderr)
        assert "E_" not in proc.stderr, (argv, proc.stderr)
        assert proc.stdout == ""


def test_dump_cli_help_and_version_exit_zero(tmp_path: Path) -> None:
    """`--help`/`-h`/`--version` print on stdout and exit 0, naming no code."""
    for argv in (["--help"], ["-h"], ["--version"]):
        proc = subprocess.run(
            [sys.executable, "-m", "entryconf", *argv],
            capture_output=True,
            text=True,
            cwd=tmp_path,
        )
        assert proc.returncode == 0, (argv, proc.stderr)
        assert proc.stdout.strip() != ""
        assert "E_" not in proc.stdout + proc.stderr, (argv, proc.stdout)


def test_dump_cli_dot_slash_addresses_a_dash_led_directory(tmp_path: Path) -> None:
    """The usage line's escape hatch has to actually work.

    Usage promises `./-name` reaches a directory whose name starts with `-`;
    that is only true if the dash test looks at the argument as given, so a
    dotted path stays a path. Asserting it keeps the documented workaround
    from rotting into a lie.
    """
    weird = tmp_path / "-weird-dir"
    weird.mkdir()
    (weird / "entrypoint.json").write_text('{"ok": true}', encoding="utf-8")

    proc = subprocess.run(
        [sys.executable, "-m", "entryconf", "./-weird-dir"],
        capture_output=True,
        text=True,
        cwd=tmp_path,
    )
    assert proc.returncode == 0, proc.stderr
    assert json.loads(proc.stdout) == {"ok": True}


def test_dump_cli_internal_error_exits_two(tmp_path: Path) -> None:
    """An internal fault is exit 2 with no `E_*` code, even a code-shaped one.

    Injecting a non-`EntryconfError` is the only way to reach that branch; the
    planted `E_PARSE` in the message proves the scrubbing, so exit 2 can never
    be mistaken for a load verdict.
    """
    script = (
        "import entryconf.__main__ as m\n"
        "def boom(_):\n"
        "    raise RuntimeError('planted E_PARSE lookalike')\n"
        "m.load = boom\n"
        f"raise SystemExit(m.main([{str(tmp_path)!r}]))\n"
    )
    proc = subprocess.run(
        [sys.executable, "-c", script], capture_output=True, text=True
    )
    assert proc.returncode == 2
    assert "E_" not in proc.stderr
    assert proc.stdout == ""


def test_dump_cli_alias_bomb_fails_fast() -> None:
    """The alias bomb is a load failure, and the budget settles it instantly.

    The point of a node budget (rather than a timeout) is that rejection costs
    a few dozen steps, so a generous ceiling here still fails loudly if the
    expansion is ever materialized instead of counted.
    """
    config = CASES_DIR / "57-yaml-alias-bomb" / "config"
    started = time.monotonic()
    proc = subprocess.run(
        [sys.executable, "-m", "entryconf", str(config)],
        capture_output=True,
        text=True,
        timeout=30,
    )
    elapsed = time.monotonic() - started
    assert proc.returncode == 1
    assert proc.stderr.splitlines()[0] == "E_PARSE"
    assert proc.stdout == ""
    assert elapsed < 10, f"the bomb took {elapsed:.1f}s — is the budget counted?"


def test_yaml_expansion_budget_boundary() -> None:
    """SPEC §2: the budget bounds the *expanded* tree, counted exactly.

    Both documents are a few kilobytes of source and differ only in how many
    times they alias the same 1000-element sequence, so nothing but the
    expanded-node count can separate them. Sizes follow SPEC §2's counting
    rule — a scalar is one node, a sequence is its elements plus their sizes, a
    mapping its entries plus its values' sizes:

        total = 2 top-level entries + 2k (the anchor) + m + 2km (the aliases)
    """
    def document(k: int, m: int) -> str:
        base = "[" + ", ".join("x" for _ in range(k)) + "]"
        return f"a: &a {base}\nb: [" + ", ".join("*a" for _ in range(m)) + "]\n"

    k = 1000
    under, over = 498, 500  # 998,500 and 1,002,502 expanded nodes
    assert 2 + 2 * k + under + 2 * k * under <= MAX_EXPANDED_NODES
    assert 2 + 2 * k + over + 2 * k * over > MAX_EXPANDED_NODES

    tree = parse_yaml(document(k, under), Path("under.yaml"))
    assert len(tree["b"]) == under and len(tree["b"][0]) == k

    with pytest.raises(EntryconfError) as excinfo:
        parse_yaml(document(k, over), Path("over.yaml"))
    assert excinfo.value.code == "E_PARSE"
