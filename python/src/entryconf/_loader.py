"""The load pipeline (SPEC §1)."""

from __future__ import annotations

import os
from pathlib import Path
from typing import Any

from ._env import ProcEnv, Vars, load_env_files
from ._errors import (
    E_MULTIPLE_ENTRYPOINTS,
    E_NO_ENTRYPOINT,
    E_PARSE,
    EntryconfError,
)
from ._includes import Resolver, Walk
from ._interpolate import interpolate
from ._parsers import SUFFIXES
from ._source import OsSource

_ENTRYPOINTS = tuple(f"entrypoint{suffix}" for suffix in SUFFIXES)


def find_entrypoint(config_dir: Path, src: Any = None) -> Path:
    """The single ``entrypoint.{json,yaml,yml,toml}`` in the directory (SPEC §3)."""
    src = src or OsSource()
    try:
        if not config_dir.is_dir():
            raise EntryconfError(E_NO_ENTRYPOINT, f"{config_dir}: not a directory")
        found = [config_dir / name for name in _ENTRYPOINTS if src.is_file(config_dir / name)]
    except OSError as exc:
        raise EntryconfError(E_NO_ENTRYPOINT, f"{config_dir}: {exc}") from exc
    if not found:
        raise EntryconfError(
            E_NO_ENTRYPOINT, f"{config_dir}: no entrypoint.{{json,yaml,yml,toml}}"
        )
    if len(found) > 1:
        listed = ", ".join(path.name for path in found)
        raise EntryconfError(E_MULTIPLE_ENTRYPOINTS, f"{config_dir}: {listed}")
    return found[0]


class Loader:
    """Runs the five steps of SPEC §1 against a file source.

    When ``rec`` is set, every document read, every graft, and every variable
    lookup is recorded on the way through — what ``open`` (SPEC §10.2) and a
    plan's candidate evaluation (SPEC §10.6) are built on. A plain ``load``
    records nothing.
    """

    def __init__(self, src: Any, proc_env: ProcEnv, rec: Any = None) -> None:
        self.src = src
        self.proc_env = proc_env
        self.rec = rec
        self.vars: Vars | None = None

    def load_tree(self, config_dir: Path) -> Any:
        # 1. Locate the entrypoint.
        entrypoint = find_entrypoint(config_dir, self.src)

        # 2. Build the variable namespace.
        files, origin = load_env_files(config_dir, self.src, self.rec)
        self.vars = Vars(files, origin, self.proc_env)

        # 3. Parse the entrypoint and graft every `@file:` include.
        resolver = Resolver(self.src, self.rec)
        tree = resolver.read_document(entrypoint, E_PARSE)
        # SPEC §3: the entrypoint's top-level value MUST be an object — anything
        # else, an empty document included, is E_PARSE. (Included files, §5, may
        # hold any value.)
        if not isinstance(tree, dict):
            raise EntryconfError(
                E_PARSE,
                f"{entrypoint}: the entrypoint's top-level value must be an object, "
                f"not {type(tree).__name__}",
            )
        if self.rec is not None:
            self.rec.graft(entrypoint, "", [])
        walk = Walk(
            doc=entrypoint,
            base_dir=entrypoint.parent,
            src="",
            eff="",
            files=[Path(os.path.realpath(entrypoint))],
            chain=[],
        )
        tree = resolver.resolve(tree, walk)

        # 4. Interpolate `$` references across the assembled tree.
        return interpolate(tree, self.vars)


def load_with_env(config_dir: str | os.PathLike[str], process_env: dict[str, str]) -> Any:
    """``load`` with an explicit process environment (internal test seam)."""
    return Loader(OsSource(), process_env.get).load_tree(Path(config_dir))


def load(config_dir: str | os.PathLike[str]) -> Any:
    """Load a config directory into a single tree (SPEC §1).

    Raises :class:`EntryconfError` — whose ``code`` is the normative ``E_*``
    code from SPEC §7 — on any failure. No partial result is ever returned.
    """
    return load_with_env(config_dir, dict(os.environ))
