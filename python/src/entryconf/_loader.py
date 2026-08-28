"""The load pipeline (SPEC §1)."""

from __future__ import annotations

import os
from pathlib import Path
from typing import Any

from ._env import build_namespace
from ._errors import (
    E_MULTIPLE_ENTRYPOINTS,
    E_NO_ENTRYPOINT,
    E_PARSE,
    EntryconfError,
)
from ._includes import read_document, resolve
from ._interpolate import interpolate
from ._parsers import SUFFIXES

_ENTRYPOINTS = tuple(f"entrypoint{suffix}" for suffix in SUFFIXES)


def find_entrypoint(config_dir: Path) -> Path:
    """The single ``entrypoint.{json,yaml,yml,toml}`` in the directory (SPEC §3)."""
    try:
        names = {entry.name for entry in config_dir.iterdir() if entry.is_file()}
    except OSError as exc:
        raise EntryconfError(E_NO_ENTRYPOINT, f"{config_dir}: {exc}") from exc
    found = [config_dir / name for name in _ENTRYPOINTS if name in names]
    if not found:
        raise EntryconfError(
            E_NO_ENTRYPOINT, f"{config_dir}: no entrypoint.{{json,yaml,yml,toml}}"
        )
    if len(found) > 1:
        listed = ", ".join(path.name for path in found)
        raise EntryconfError(E_MULTIPLE_ENTRYPOINTS, f"{config_dir}: {listed}")
    return found[0]


def load_with_env(config_dir: str | os.PathLike[str], process_env: dict[str, str]) -> Any:
    """``load`` with an explicit process environment (internal test seam)."""
    directory = Path(config_dir)

    # 1. Locate the entrypoint.
    entrypoint = find_entrypoint(directory)

    # 2. Build the variable namespace.
    env = build_namespace(directory, process_env)

    # 3. Parse the entrypoint and graft every `@file:` include.
    tree = read_document(entrypoint, E_PARSE)
    # SPEC §3: the entrypoint's top-level value MUST be an object — anything
    # else, an empty document included, is E_PARSE. (Included files, §5, may
    # hold any value.)
    if not isinstance(tree, dict):
        raise EntryconfError(
            E_PARSE,
            f"{entrypoint}: the entrypoint's top-level value must be an object, "
            f"not {type(tree).__name__}",
        )
    tree = resolve(tree, entrypoint.parent, [Path(os.path.realpath(entrypoint))])

    # 4. Interpolate `$` references across the assembled tree.
    return interpolate(tree, env)


def load(config_dir: str | os.PathLike[str]) -> Any:
    """Load a config directory into a single tree (SPEC §1).

    Raises :class:`EntryconfError` — whose ``code`` is the normative ``E_*``
    code from SPEC §7 — on any failure. No partial result is ever returned.
    """
    return load_with_env(config_dir, dict(os.environ))
