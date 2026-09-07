"""RFC 6901 JSON Pointers and the two edit operations (SPEC §10.3, §10.5)."""

from __future__ import annotations

import copy
from typing import Any

from ._errors import E_PATH, EntryconfError


def parse_pointer(pointer: str) -> list[str]:
    """Split a pointer into tokens; ``""`` is the root. Malformed is ``E_PATH``."""
    if pointer == "":
        return []
    if not pointer.startswith("/"):
        raise EntryconfError(E_PATH, f"pointer {pointer!r} must be empty or start with '/'")
    tokens: list[str] = []
    for raw in pointer[1:].split("/"):
        out: list[str] = []
        i = 0
        while i < len(raw):
            c = raw[i]
            if c != "~":
                out.append(c)
                i += 1
                continue
            if i + 1 >= len(raw) or raw[i + 1] not in "01":
                raise EntryconfError(E_PATH, f"pointer {pointer!r}: '~' must be followed by 0 or 1")
            out.append("~" if raw[i + 1] == "0" else "/")
            i += 2
        tokens.append("".join(out))
    return tokens


def escape_token(token: str) -> str:
    return token.replace("~", "~0").replace("/", "~1")


def join_pointer(tokens: list[str]) -> str:
    return "".join("/" + escape_token(t) for t in tokens)


def array_index(token: str) -> int | None:
    """A decimal index with no leading zeros, or ``None`` (including for ``-``)."""
    if not token or (len(token) > 1 and token[0] == "0") or not token.isascii() or not token.isdigit():
        return None
    return int(token)


def deep_copy(value: Any) -> Any:
    return copy.deepcopy(value)


def _descend(parent: Any, tok: str, pointer: str, create: bool) -> tuple[Any, Any]:
    """Step into ``parent`` at ``tok``; returns (child, holder-for-writeback)."""
    if isinstance(parent, dict):
        if tok not in parent:
            if not create:
                raise EntryconfError(E_PATH, f"pointer {pointer!r}: no member {tok!r}")
            parent[tok] = {}
        return parent[tok], None
    if isinstance(parent, list):
        i = array_index(tok)
        if i is None or i >= len(parent):
            raise EntryconfError(E_PATH, f"pointer {pointer!r}: array index {tok!r} does not exist")
        return parent[i], None
    raise EntryconfError(E_PATH, f"pointer {pointer!r}: cannot descend into a scalar at {tok!r}")


def set_at(root: Any, pointer: str, value: Any) -> Any:
    """Apply a ``set`` edit (SPEC §10.5) and return the new root."""
    tokens = parse_pointer(pointer)
    if not tokens:
        return value
    parent = root
    for tok in tokens[:-1]:
        parent, _ = _descend(parent, tok, pointer, create=True)
    last = tokens[-1]
    if isinstance(parent, dict):
        parent[last] = value
    elif isinstance(parent, list):
        if last == "-":
            parent.append(value)
        else:
            i = array_index(last)
            if i is None or i > len(parent):
                raise EntryconfError(
                    E_PATH, f"pointer {pointer!r}: array index {last!r} is out of range (0..{len(parent)})"
                )
            if i == len(parent):
                parent.append(value)
            else:
                parent[i] = value
    else:
        raise EntryconfError(E_PATH, f"pointer {pointer!r}: cannot set a member of a scalar")
    return root


def remove_at(root: Any, pointer: str) -> Any:
    """Apply a ``remove`` edit (SPEC §10.5) and return the new root."""
    tokens = parse_pointer(pointer)
    if not tokens:
        raise EntryconfError(E_PATH, "the root value cannot be removed")
    parent = root
    for tok in tokens[:-1]:
        parent, _ = _descend(parent, tok, pointer, create=False)
    last = tokens[-1]
    if isinstance(parent, dict):
        if last not in parent:
            raise EntryconfError(E_PATH, f"pointer {pointer!r}: no member {last!r} to remove")
        del parent[last]
    elif isinstance(parent, list):
        i = array_index(last)
        if i is None or i >= len(parent):
            raise EntryconfError(E_PATH, f"pointer {pointer!r}: array index {last!r} does not exist")
        del parent[i]
    else:
        raise EntryconfError(E_PATH, f"pointer {pointer!r}: cannot remove a member of a scalar")
    return root


def resolve_pointer(value: Any, pointer: str) -> Any:
    cur = value
    for tok in parse_pointer(pointer):
        cur, _ = _descend(cur, tok, pointer, create=False)
    return cur
