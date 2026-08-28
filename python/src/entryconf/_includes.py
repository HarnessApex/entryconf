"""``@file:`` include grafting (SPEC §5)."""

from __future__ import annotations

import os
from pathlib import Path
from typing import Any

from ._errors import E_INCLUDE, E_INCLUDE_CYCLE, E_PARSE, E_SUBSTITUTION, EntryconfError
from ._parsers import SUFFIXES, parse_document

_PREFIX = "@file:"


def read_document(path: Path, missing_code: str) -> Any:
    """Read and parse one document; read failures use ``missing_code``."""
    try:
        text = path.read_text(encoding="utf-8")
    except OSError as exc:
        raise EntryconfError(missing_code, f"{path}: {exc}") from exc
    except UnicodeDecodeError as exc:
        raise EntryconfError(E_PARSE, f"{path}: {exc}") from exc
    return parse_document(path, text)


def _graft(target: Path, chain: list[Path]) -> Any:
    # SPEC §5: the extension is matched case-sensitively; `.JSON` is not `.json`.
    if target.suffix not in SUFFIXES:
        raise EntryconfError(
            E_INCLUDE, f"{target}: unsupported extension {target.suffix!r}"
        )
    real = Path(os.path.realpath(target))
    if real in chain:
        cycle = [str(p) for p in chain[chain.index(real) :]] + [str(real)]
        raise EntryconfError(E_INCLUDE_CYCLE, " -> ".join(cycle))
    tree = read_document(target, E_INCLUDE)
    return resolve(tree, target.parent, chain + [real])


def resolve(node: Any, base_dir: Path, chain: list[Path]) -> Any:
    """Replace every ``@file:`` string value below ``node`` with its tree.

    ``base_dir`` is the directory of the file the node came from: include
    paths are relative to the *referencing* file, not the entrypoint.
    """
    if isinstance(node, str):
        if node.startswith("@@"):
            # A leading `@@` becomes a literal `@`; the result is inert.
            return "@" + node[2:]
        if node.startswith(_PREFIX):
            return _graft(base_dir / node[len(_PREFIX) :], chain)
        if node.startswith("@"):
            raise EntryconfError(
                E_SUBSTITUTION, f"{node!r}: reserved `@` directive (write `@@` for a literal `@`)"
            )
        return node
    if isinstance(node, dict):
        return {key: resolve(value, base_dir, chain) for key, value in node.items()}
    if isinstance(node, list):
        return [resolve(item, base_dir, chain) for item in node]
    return node
