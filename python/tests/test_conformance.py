"""The conformance harness (SPEC §8).

One parametrized test walks the shared fixture suite in
``../../testdata/cases`` — there are deliberately no hand-written per-case
tests, which could drift from the fixtures.
"""

from __future__ import annotations

import contextlib
import json
import os
import re
import subprocess
import sys
import threading
from pathlib import Path
from typing import Any, Iterator

import pytest

from entryconf import EntryconfError, load

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


def test_dump_cli(tmp_path: Path) -> None:
    """`python -m entryconf <dir>`: JSON on stdout, or the E_* code on stderr."""
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

    bad = subprocess.run(
        [sys.executable, "-m", "entryconf", str(tmp_path / "empty")],
        capture_output=True,
        text=True,
    )
    assert bad.returncode == 1
    assert bad.stderr.strip() == "E_NO_ENTRYPOINT"
    assert bad.stdout == ""
