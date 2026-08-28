"""``$`` interpolation (SPEC §6).

Every string *value* in the assembled tree is scanned; object keys never are.
Only four forms are legal — ``${NAME}``, ``${NAME:-default}``, ``$NAME`` and
``$$`` — and any other use of ``$`` is ``E_SUBSTITUTION``. Text produced by a
substitution is inert: it is never re-scanned.
"""

from __future__ import annotations

import json
import math
import re
from typing import Any

from ._errors import E_MISSING_VAR, E_SUBSTITUTION, EntryconfError

_NAME = r"[A-Za-z_][A-Za-z0-9_]*"
_BRACED_PLAIN = re.compile(rf"({_NAME})\Z")
_BRACED_DEFAULT = re.compile(rf"({_NAME}):-(.*)\Z", re.DOTALL)
_SHORTHAND = re.compile(_NAME)

#: JSON number grammar, for whole-value typing.
_NUMBER = re.compile(r"-?(?:0|[1-9][0-9]*)(?:\.[0-9]+)?(?:[eE][-+]?[0-9]+)?\Z")


def _lookup(name: str, default: str | None, env: dict[str, str]) -> str:
    if name in env:
        return env[name]
    if default is not None:
        return default
    raise EntryconfError(E_MISSING_VAR, f"${{{name}}} is not set and has no default")


def _typed(text: str) -> Any:
    """Whole-value typing: a lone reference may yield a non-string scalar.

    The number case requires the text to parse to a *finite* IEEE-754 double
    (SPEC §6), so an overflowing literal such as ``1e400`` stays a string.
    """
    if text == "true":
        return True
    if text == "false":
        return False
    if text == "null":
        return None
    if _NUMBER.fullmatch(text):
        number = json.loads(text)
        if isinstance(number, int) or math.isfinite(number):
            return number
    return text


def interpolate_string(value: str, env: dict[str, str]) -> Any:
    parts: list[tuple[str, str]] = []  # ("lit" | "ref", text)
    literal: list[str] = []
    i = 0
    length = len(value)

    def flush() -> None:
        if literal:
            parts.append(("lit", "".join(literal)))
            literal.clear()

    while i < length:
        char = value[i]
        if char != "$":
            literal.append(char)
            i += 1
            continue
        if i + 1 >= length:
            raise EntryconfError(E_SUBSTITUTION, f"{value!r}: trailing `$`")
        nxt = value[i + 1]
        if nxt == "$":
            literal.append("$")
            i += 2
            continue
        if nxt == "{":
            end = value.find("}", i + 2)
            if end < 0:
                raise EntryconfError(E_SUBSTITUTION, f"{value!r}: unterminated `${{`")
            body = value[i + 2 : end]
            plain = _BRACED_PLAIN.fullmatch(body)
            if plain is not None:
                resolved = _lookup(plain.group(1), None, env)
            else:
                with_default = _BRACED_DEFAULT.fullmatch(body)
                if with_default is None:
                    raise EntryconfError(
                        E_SUBSTITUTION,
                        f"{value!r}: `${{{body}}}` is not `${{NAME}}` or `${{NAME:-default}}`",
                    )
                # The default text is literal: no nested substitution.
                resolved = _lookup(
                    with_default.group(1), with_default.group(2), env
                )
            flush()
            parts.append(("ref", resolved))
            i = end + 1
            continue
        match = _SHORTHAND.match(value, i + 1)
        if match is None:
            raise EntryconfError(
                E_SUBSTITUTION, f"{value!r}: `$` must be followed by a name, `{{`, or `$`"
            )
        flush()
        parts.append(("ref", _lookup(match.group(0), None, env)))
        i = match.end()

    flush()
    if len(parts) == 1 and parts[0][0] == "ref":
        return _typed(parts[0][1])
    return "".join(text for _, text in parts)


def interpolate(node: Any, env: dict[str, str]) -> Any:
    if isinstance(node, str):
        return interpolate_string(node, env)
    if isinstance(node, dict):
        return {key: interpolate(value, env) for key, value in node.items()}
    if isinstance(node, list):
        return [interpolate(item, env) for item in node]
    return node
