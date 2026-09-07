"""Normalized JSON output (SPEC §10.8)."""

from __future__ import annotations

import math
from typing import Any

_ESCAPES = {'"': '\\"', "\\": "\\\\", "\b": "\\b", "\f": "\\f", "\n": "\\n", "\r": "\\r", "\t": "\\t"}


def format_json(value: Any) -> str:
    """Two-space indent, one member/element per line, keys sorted by code
    point, minimal escaping, integers as integers, one trailing newline."""
    out: list[str] = []
    _write(out, value, 0)
    out.append("\n")
    return "".join(out)


def _write(out: list[str], value: Any, depth: int) -> None:
    if value is None:
        out.append("null")
    elif value is True:
        out.append("true")
    elif value is False:
        out.append("false")
    elif isinstance(value, str):
        out.append(_string(value))
    elif isinstance(value, int):
        out.append(str(value))
    elif isinstance(value, float):
        out.append(_number(value))
    elif isinstance(value, dict):
        if not value:
            out.append("{}")
            return
        out.append("{\n")
        keys = sorted(value)  # str comparison is by code point
        for i, key in enumerate(keys):
            out.append("  " * (depth + 1))
            out.append(_string(key))
            out.append(": ")
            _write(out, value[key], depth + 1)
            out.append(",\n" if i + 1 < len(keys) else "\n")
        out.append("  " * depth + "}")
    elif isinstance(value, list):
        if not value:
            out.append("[]")
            return
        out.append("[\n")
        for i, item in enumerate(value):
            out.append("  " * (depth + 1))
            _write(out, item, depth + 1)
            out.append(",\n" if i + 1 < len(value) else "\n")
        out.append("  " * depth + "]")
    else:  # unreachable: values are normalized before serialization
        out.append("null")


def _number(f: float) -> str:
    if f == math.trunc(f) and abs(f) < 2**53:
        return str(int(f))
    return repr(f)


def _string(s: str) -> str:
    out = ['"']
    for ch in s:
        esc = _ESCAPES.get(ch)
        if esc is not None:
            out.append(esc)
        elif ord(ch) < 0x20:
            out.append(f"\\u{ord(ch):04x}")
        else:
            out.append(ch)
    out.append('"')
    return "".join(out)
