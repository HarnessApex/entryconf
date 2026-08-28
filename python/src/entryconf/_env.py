"""The variable namespace (SPEC §4).

All ``*.env`` files directly in the config directory are unordered peers: a
name defined twice — in one file or across two — is ``E_ENV_CONFLICT``. The
process environment overrides them.
"""

from __future__ import annotations

import re
from pathlib import Path

from ._errors import E_ENV_CONFLICT, E_PARSE, EntryconfError

NAME_RE = re.compile(r"[A-Za-z_][A-Za-z0-9_]*")
_LINE_RE = re.compile(r"([A-Za-z_][A-Za-z0-9_]*)=(.*)", re.DOTALL)


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


def build_namespace(config_dir: Path, process_env: dict[str, str]) -> dict[str, str]:
    """Merge the ``*.env`` peers, then let the process environment override."""
    values: dict[str, str] = {}
    origin: dict[str, Path] = {}
    for path in env_files(config_dir):
        try:
            text = path.read_text(encoding="utf-8")
        except OSError as exc:
            raise EntryconfError(E_PARSE, f"{path}: {exc}") from exc
        except UnicodeDecodeError as exc:
            raise EntryconfError(E_PARSE, f"{path}: {exc}") from exc
        for name, value in parse_env_file(text, path).items():
            if name in values:
                raise EntryconfError(
                    E_ENV_CONFLICT,
                    f"{name} is defined in both {origin[name]} and {path}",
                )
            values[name] = value
            origin[name] = path
    values.update(process_env)
    return values
