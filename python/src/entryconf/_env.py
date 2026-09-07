"""The variable namespace (SPEC §4).

All ``*.env`` files directly in the config directory are unordered peers: a
name defined twice — in one file or across two — is ``E_ENV_CONFLICT``. The
process environment overrides them.
"""

from __future__ import annotations

import re
from pathlib import Path
from typing import Any, Callable

from ._errors import E_ENV_CONFLICT, E_PARSE, EntryconfError

NAME_RE = re.compile(r"[A-Za-z_][A-Za-z0-9_]*")
_LINE_RE = re.compile(r"([A-Za-z_][A-Za-z0-9_]*)=(.*)", re.DOTALL)

#: A process-environment lookup: the value, or ``None`` when unset.
ProcEnv = Callable[[str], "str | None"]


def _unquote(value: str) -> str:
    value = value.strip()
    if len(value) >= 2 and value[0] == value[-1] and value[0] in ("'", '"'):
        return value[1:-1]
    return value


def parse_env_file(text: str, path: Path) -> dict[str, str]:
    """Parse one ``*.env`` file (a strict subset of dotenv)."""
    values: dict[str, str] = {}
    for lineno, raw in enumerate(text.splitlines(), start=1):
        line = raw.strip()
        if not line or line.startswith("#"):
            continue
        match = _LINE_RE.fullmatch(line)
        if match is None:
            raise EntryconfError(
                E_PARSE, f"{path}:{lineno}: not a blank line, comment, or NAME=value"
            )
        name = match.group(1)
        if name in values:
            raise EntryconfError(
                E_ENV_CONFLICT, f"{name} is defined twice in {path} (line {lineno})"
            )
        values[name] = _unquote(match.group(2))
    return values


def env_files(config_dir: Path) -> list[Path]:
    """Every ``*.env`` file directly in the config directory (non-recursive)."""
    found = [
        entry
        for entry in config_dir.iterdir()
        if entry.name.endswith(".env") and entry.is_file()
    ]
    return sorted(found, key=lambda p: p.name)


class Vars:
    """The single global namespace: ``*.env`` values, process environment on top.

    Every name looked up is remembered in ``used`` (SPEC §10.6 "variables"),
    and ``where`` reports which SPEC §4 layer supplies a name.
    """

    def __init__(self, files: dict[str, str], origin: dict[str, Path], proc: ProcEnv) -> None:
        self.files = files
        self.origin = origin
        self.proc = proc
        self.used: set[str] = set()

    def lookup(self, name: str) -> str | None:
        self.used.add(name)
        value = self.proc(name)
        if value is not None:
            return value
        return self.files.get(name)

    def where(self, name: str) -> tuple[str, Path | None]:
        if self.proc(name) is not None:
            return "process", None
        if name in self.origin:
            return "file", self.origin[name]
        return "", None


def load_env_files(
    config_dir: Path, src: Any, rec: Any = None
) -> tuple[dict[str, str], dict[str, Path]]:
    """Merge the ``*.env`` peers; also report which file defines each name."""
    values: dict[str, str] = {}
    origin: dict[str, Path] = {}
    try:
        names = src.env_file_names(config_dir)
    except OSError as exc:
        raise EntryconfError(E_PARSE, f"{config_dir}: {exc}") from exc
    for name in sorted(names):
        path = config_dir / name
        try:
            data = src.read_file(path)
            text = data.decode("utf-8")
        except OSError as exc:
            raise EntryconfError(E_PARSE, f"{path}: {exc}") from exc
        except UnicodeDecodeError as exc:
            raise EntryconfError(E_PARSE, f"{path}: {exc}") from exc
        parsed = parse_env_file(text, path)
        if rec is not None:
            rec.record(path, data, "env", None)
        for var, value in parsed.items():
            if var in values:
                raise EntryconfError(
                    E_ENV_CONFLICT,
                    f"{var} is defined in both {origin[var]} and {path}",
                )
            values[var] = value
            origin[var] = path
    return values, origin


def build_namespace(config_dir: Path, process_env: dict[str, str]) -> dict[str, str]:
    """Merge the ``*.env`` peers, then let the process environment override.

    Kept for callers of the 0.2.0 internals; the loader uses :class:`Vars`.
    """
    from ._source import OsSource

    values, _ = load_env_files(config_dir, OsSource())
    values.update(process_env)
    return values
