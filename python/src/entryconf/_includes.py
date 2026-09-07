"""``@file:`` include grafting (SPEC §5)."""

from __future__ import annotations

import os
from dataclasses import dataclass, replace
from pathlib import Path
from typing import Any

from ._errors import E_INCLUDE, E_INCLUDE_CYCLE, E_PARSE, E_SUBSTITUTION, EntryconfError
from ._parsers import SUFFIXES, parse_document
from ._pointer import escape_token
from ._source import clean_join, format_of

_PREFIX = "@file:"


def is_include(value: str) -> bool:
    return value.startswith(_PREFIX)


def include_target(value: str, base_dir: Path) -> Path:
    """The cleaned path an ``@file:`` string names, relative to its file's directory."""
    return clean_join(base_dir, value[len(_PREFIX) :])


@dataclass
class Walk:
    """Where the resolver is: the file being walked and its directory, the
    pointer within that file (``src``) and within the effective tree
    (``eff``), the realpath chain for cycle detection, and the ``@file:``
    references traversed so far (SPEC §10.3 "chain")."""

    doc: Path
    base_dir: Path
    src: str
    eff: str
    files: list[Path]
    chain: list[dict[str, str]]

    def step(self, token: str) -> "Walk":
        escaped = "/" + escape_token(token)
        return replace(self, src=self.src + escaped, eff=self.eff + escaped)


class Resolver:
    def __init__(self, src: Any, rec: Any = None) -> None:
        self.src = src
        self.rec = rec

    def read_document(self, path: Path, missing_code: str) -> Any:
        """Read and parse one document; read failures use ``missing_code``."""
        try:
            data = self.src.read_file(path)
            text = data.decode("utf-8")
        except OSError as exc:
            raise EntryconfError(missing_code, f"{path}: {exc}") from exc
        except UnicodeDecodeError as exc:
            raise EntryconfError(E_PARSE, f"{path}: {exc}") from exc
        tree = parse_document(path, text)
        if self.rec is not None:
            self.rec.record(path, data, format_of(path), tree)
        return tree

    def _graft(self, value: str, w: Walk) -> Any:
        target = include_target(value, w.base_dir)
        # SPEC §5: the extension is matched case-sensitively; `.JSON` is not `.json`.
        if target.suffix not in SUFFIXES:
            raise EntryconfError(
                E_INCLUDE, f"{target}: unsupported extension {target.suffix!r}"
            )
        real = Path(os.path.realpath(target))
        if real in w.files:
            cycle = [str(p) for p in w.files[w.files.index(real) :]] + [str(real)]
            raise EntryconfError(E_INCLUDE_CYCLE, " -> ".join(cycle))
        tree = self.read_document(target, E_INCLUDE)
        chain = w.chain + [{"document": str(w.doc), "pointer": w.src}]
        if self.rec is not None:
            self.rec.graft(target, w.eff, chain)
        return self.resolve(
            tree,
            Walk(doc=target, base_dir=target.parent, src="", eff=w.eff, files=w.files + [real], chain=chain),
        )

    def resolve(self, node: Any, w: Walk) -> Any:
        """Replace every ``@file:`` string value below ``node`` with its tree."""
        if isinstance(node, str):
            if node.startswith("@@"):
                # A leading `@@` becomes a literal `@`; the result is inert.
                return "@" + node[2:]
            if node.startswith(_PREFIX):
                return self._graft(node, w)
            if node.startswith("@"):
                raise EntryconfError(
                    E_SUBSTITUTION, f"{node!r}: reserved `@` directive (write `@@` for a literal `@`)"
                )
            return node
        if isinstance(node, dict):
            return {key: self.resolve(value, w.step(key)) for key, value in node.items()}
        if isinstance(node, list):
            return [self.resolve(item, w.step(str(i))) for i, item in enumerate(node)]
        return node
