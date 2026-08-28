#!/usr/bin/env python3
"""Cross-implementation differ for the entryconf conformance suite.

Runs every implementation's documented *dump* invocation against every case
under ``testdata/cases/`` and compares the results — against the fixture's
``expected.json`` and against each other. It is the check the per-language
harnesses cannot make: each of those only proves its own implementation
matches the fixtures, never that the four agree on the same bytes.

For each SUCCESS case (a case with ``expected.json``):

* run all four dumps with the case's ``procenv.json`` applied and every
  case-named variable scrubbed from the real environment (SPEC §8);
* parse each dump's stdout as JSON and normalize it (object keys sorted,
  integral floats folded to int, so ``8080.0`` == ``8080``);
* diff each implementation against ``expected.json``, then every
  implementation against every other.

For each FAILURE case (a case with ``expected_error.txt``): every dump MUST
exit nonzero and name the expected ``E_*`` code on stderr.

Usage::

    python3 tools/crosscheck/crosscheck.py [-root DIR] [-case NAME]... [-v]

Exit status is 0 only if every implementation ran, matched the fixture, and
agreed with the others. Stdlib only, by design: this tool must not depend on
any implementation it is checking.
"""

from __future__ import annotations

import argparse
import json
import os
import re
import shutil
import subprocess
import sys
from pathlib import Path
from typing import Any, Iterable

# Variable names appearing in a case's files; SPEC §8 requires them to be
# otherwise unset in the real environment, so the runner scrubs them.
_REFERENCE_RE = re.compile(r"\$\{?([A-Za-z_][A-Za-z0-9_]*)")
_ENV_LINE_RE = re.compile(r"^\s*([A-Za-z_][A-Za-z0-9_]*)\s*=", re.MULTILINE)

TIMEOUT = 120.0


# --------------------------------------------------------------------------
# implementations


class Impl:
    """One implementation's dump command, as documented in its README."""

    def __init__(self, name: str, argv: list[str], cwd: Path, env: dict[str, str] | None = None):
        self.name = name
        self.argv = argv
        self.cwd = cwd
        self.extra_env = env or {}

    def run(self, config: Path, env: dict[str, str]) -> subprocess.CompletedProcess[str]:
        full_env = dict(env)
        full_env.update(self.extra_env)
        return subprocess.run(
            [*self.argv, str(config)],
            cwd=str(self.cwd),
            env=full_env,
            capture_output=True,
            text=True,
            timeout=TIMEOUT,
        )


def build_impls(root: Path, workdir: Path) -> tuple[list[Impl], list[str]]:
    """Prepare the four dump commands, building binaries where that is cheaper.

    Returns the implementations that are runnable plus notes about any that
    could not be prepared (reported as failures later, never skipped silently).
    """
    impls: list[Impl] = []
    notes: list[str] = []

    # Go — README: `go run ./cmd/entryconf dump <dir>` (or `go install`).
    # Build once so 22 cases do not pay 22 link steps.
    go_dir = root / "go"
    if go_dir.is_dir():
        binary = workdir / "entryconf-go"
        built = subprocess.run(
            ["go", "build", "-o", str(binary), "./cmd/entryconf"],
            cwd=str(go_dir), capture_output=True, text=True,
        )
        if built.returncode == 0:
            impls.append(Impl("go", [str(binary), "dump"], go_dir))
        else:
            notes.append(f"go: build failed: {built.stderr.strip()[:400]}")
    else:
        notes.append("go: directory missing")

    # Python — README: `python -m entryconf <dir>`.
    py_dir = root / "python"
    if py_dir.is_dir():
        venv_py = py_dir / ".venv" / "bin" / "python"
        interpreter = str(venv_py) if venv_py.exists() else sys.executable
        # PYTHONPATH keeps this working even without an editable install.
        src = str(py_dir / "src")
        impls.append(Impl("python", [interpreter, "-m", "entryconf"], py_dir,
                          env={"PYTHONPATH": src}))
    else:
        notes.append("python: directory missing")

    # TypeScript — README: `node src/cli.ts <dir>`.
    ts_dir = root / "ts"
    if ts_dir.is_dir():
        if (ts_dir / "node_modules").is_dir():
            impls.append(Impl("ts", ["node", str(ts_dir / "src" / "cli.ts")], ts_dir))
        else:
            notes.append("ts: node_modules missing (run `npm install` in ts/)")
    else:
        notes.append("ts: directory missing")

    # Rust — README: `cargo build --bin entryconf-dump`, then the binary.
    rs_dir = root / "rust"
    if rs_dir.is_dir():
        built = subprocess.run(
            ["cargo", "build", "--quiet", "--bin", "entryconf-dump"],
            cwd=str(rs_dir), capture_output=True, text=True,
        )
        binary = rs_dir / "target" / "debug" / "entryconf-dump"
        if built.returncode == 0 and binary.exists():
            impls.append(Impl("rust", [str(binary)], rs_dir))
        else:
            notes.append(f"rust: build failed: {built.stderr.strip()[:400]}")
    else:
        notes.append("rust: directory missing")

    return impls, notes


# --------------------------------------------------------------------------
# environment


def case_variable_names(case: Path) -> set[str]:
    names: set[str] = set()
    procenv = case / "procenv.json"
    if procenv.is_file():
        names |= set(json.loads(procenv.read_text(encoding="utf-8")))
    config = case / "config"
    if config.is_dir():
        for path in sorted(config.rglob("*")):
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


def case_env(case: Path) -> dict[str, str]:
    env = dict(os.environ)
    for name in case_variable_names(case):
        env.pop(name, None)
    procenv = case / "procenv.json"
    if procenv.is_file():
        overrides = json.loads(procenv.read_text(encoding="utf-8"))
        env.update({str(k): str(v) for k, v in overrides.items()})
    return env


# --------------------------------------------------------------------------
# normalization and diffing


def normalize(value: Any) -> Any:
    """Canonical form: keys sorted, integral floats folded to int."""
    if isinstance(value, dict):
        return {k: normalize(value[k]) for k in sorted(value)}
    if isinstance(value, list):
        return [normalize(v) for v in value]
    if isinstance(value, bool) or value is None:
        return value
    if isinstance(value, float):
        if value == int(value) and abs(value) < 2**53:
            return int(value)
        return value
    return value


def diff(a: Any, b: Any, path: str = "$") -> str | None:
    """First structural difference between two normalized trees, or None."""
    if isinstance(a, bool) or isinstance(b, bool) or a is None or b is None:
        if a is not b and a != b:
            return f"{path}: {a!r} != {b!r}"
        if isinstance(a, bool) != isinstance(b, bool):
            return f"{path}: {a!r} != {b!r}"
        return None
    if isinstance(a, dict) and isinstance(b, dict):
        if set(a) != set(b):
            only_a = sorted(set(a) - set(b))
            only_b = sorted(set(b) - set(a))
            return f"{path}: key mismatch (left-only {only_a}, right-only {only_b})"
        for key in sorted(a):
            found = diff(a[key], b[key], f"{path}.{key}")
            if found:
                return found
        return None
    if isinstance(a, list) and isinstance(b, list):
        if len(a) != len(b):
            return f"{path}: length {len(a)} != {len(b)}"
        for i, (x, y) in enumerate(zip(a, b)):
            found = diff(x, y, f"{path}[{i}]")
            if found:
                return found
        return None
    if isinstance(a, (int, float)) and isinstance(b, (int, float)):
        if float(a) != float(b):
            return f"{path}: {a!r} != {b!r}"
        return None
    if type(a) is not type(b) or a != b:
        return f"{path}: {a!r} != {b!r}"
    return None


# --------------------------------------------------------------------------
# the run


class Result:
    """One implementation's outcome on one case."""

    def __init__(self, ok: bool, detail: str = ""):
        self.ok = ok
        self.detail = detail


def check_success(impl: Impl, case: Path, expected: Any, env: dict[str, str]) -> tuple[Result, Any]:
    try:
        proc = impl.run(case / "config", env)
    except subprocess.TimeoutExpired:
        return Result(False, f"timed out after {TIMEOUT:g}s"), None
    except OSError as exc:
        return Result(False, f"could not run: {exc}"), None
    if proc.returncode != 0:
        first = (proc.stderr.strip().splitlines() or [""])[0]
        return Result(False, f"exit {proc.returncode}: {first}"), None
    try:
        tree = normalize(json.loads(proc.stdout))
    except json.JSONDecodeError as exc:
        return Result(False, f"stdout is not JSON: {exc}"), None
    found = diff(tree, expected)
    if found:
        return Result(False, f"differs from expected.json at {found}"), tree
    return Result(True), tree


def check_failure(impl: Impl, case: Path, code: str, env: dict[str, str]) -> Result:
    try:
        proc = impl.run(case / "config", env)
    except subprocess.TimeoutExpired:
        return Result(False, f"timed out after {TIMEOUT:g}s")
    except OSError as exc:
        return Result(False, f"could not run: {exc}")
    if proc.returncode == 0:
        return Result(False, f"exited 0; expected failure with {code}")
    if code not in proc.stderr:
        first = (proc.stderr.strip().splitlines() or ["<empty stderr>"])[0]
        return Result(False, f"stderr lacks {code} (got: {first})")
    return Result(True)


def main(argv: list[str]) -> int:
    here = Path(__file__).resolve()
    parser = argparse.ArgumentParser(description="cross-implementation differ for entryconf")
    parser.add_argument("-root", default=str(here.parents[2]),
                        help="repository root (default: inferred from this file)")
    parser.add_argument("-case", action="append", default=[],
                        help="only run these case directories (repeatable)")
    parser.add_argument("-v", "--verbose", action="store_true",
                        help="print every failure detail, not just the first per case")
    args = parser.parse_args(argv)

    root = Path(args.root).resolve()
    cases_dir = root / "testdata" / "cases"
    if not cases_dir.is_dir():
        print(f"crosscheck: no testdata/cases under {root}", file=sys.stderr)
        return 2

    for tool in ("go", "node", "cargo"):
        if shutil.which(tool) is None:
            print(f"crosscheck: {tool} not on PATH", file=sys.stderr)

    workdir = root / "tools" / "crosscheck" / ".build"
    workdir.mkdir(parents=True, exist_ok=True)

    impls, notes = build_impls(root, workdir)
    for note in notes:
        print(f"crosscheck: cannot run {note}", file=sys.stderr)
    if not impls:
        print("crosscheck: no runnable implementations", file=sys.stderr)
        return 2

    cases = sorted(p for p in cases_dir.iterdir() if p.is_dir())
    if args.case:
        wanted = set(args.case)
        cases = [c for c in cases if c.name in wanted]
        missing = sorted(wanted - {c.name for c in cases})
        if missing:
            print(f"crosscheck: no such case(s): {', '.join(missing)}", file=sys.stderr)
            return 2
    if not cases:
        print(f"crosscheck: no cases under {cases_dir}", file=sys.stderr)
        return 2

    names = [impl.name for impl in impls]
    width = max([len(c.name) for c in cases] + [4])
    header = f"{'case'.ljust(width)}  kind     " + "  ".join(n.ljust(7) for n in names) + "  agree"
    print(header)
    print("-" * len(header))

    failures: list[str] = []

    for case in cases:
        env = case_env(case)
        error_file = case / "expected_error.txt"
        expected_file = case / "expected.json"
        row: dict[str, Result] = {}
        agree = "-"

        if error_file.is_file():
            kind = "fail"
            code = error_file.read_text(encoding="utf-8").strip()
            for impl in impls:
                row[impl.name] = check_failure(impl, case, code, env)
            agree = "ok" if all(r.ok for r in row.values()) else "DIFF"
        elif expected_file.is_file():
            kind = "ok"
            expected = normalize(json.loads(expected_file.read_text(encoding="utf-8")))
            trees: dict[str, Any] = {}
            for impl in impls:
                result, tree = check_success(impl, case, expected, env)
                row[impl.name] = result
                if tree is not None:
                    trees[impl.name] = tree
            # pairwise agreement between the implementations themselves
            pair_problems: list[str] = []
            got = [n for n in names if n in trees]
            for i, left in enumerate(got):
                for right in got[i + 1:]:
                    found = diff(trees[left], trees[right])
                    if found:
                        pair_problems.append(f"{left} vs {right}: {found}")
            if pair_problems:
                agree = "DIFF"
                for problem in pair_problems:
                    failures.append(f"{case.name}: {problem}")
            elif len(got) < len(names):
                agree = "n/a"
            else:
                agree = "ok"
        else:
            kind = "????"
            failures.append(f"{case.name}: neither expected.json nor expected_error.txt")
            print(f"{case.name.ljust(width)}  {kind}     " + "  ".join("-".ljust(7) for _ in names) + "  -")
            continue

        cells = []
        for name in names:
            result = row[name]
            cells.append(("PASS" if result.ok else "FAIL").ljust(7))
            if not result.ok:
                failures.append(f"{case.name}: {name}: {result.detail}")
        print(f"{case.name.ljust(width)}  {kind.ljust(7)}  " + "  ".join(cells) + f"  {agree}")

    print()
    if failures:
        print(f"crosscheck: {len(failures)} problem(s):", file=sys.stderr)
        shown: Iterable[str] = failures if args.verbose else failures[:40]
        for line in shown:
            print(f"  {line}", file=sys.stderr)
        if not args.verbose and len(failures) > 40:
            print(f"  ... and {len(failures) - 40} more (-v for all)", file=sys.stderr)
        return 1

    print(f"crosscheck: {len(cases)} case(s) x {len(impls)} implementation(s) "
          f"({', '.join(names)}) — all agree with the fixtures and with each other")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
