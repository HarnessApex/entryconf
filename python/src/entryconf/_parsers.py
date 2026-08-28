"""Document parsers (SPEC §2).

JSON / YAML / TOML are parsed with stock parsers, constrained to the spec's
data model:

* the loaded tree is JSON-equivalent (null, bool, number, string, array,
  object with string keys),
* YAML uses the **YAML 1.2 core schema** (PyYAML's own resolvers implement
  YAML 1.1, so the resolvers and constructors are rebuilt here),
* YAML alias expansion is bounded by a node budget (:data:`MAX_EXPANDED_NODES`),
* TOML datetimes become their RFC 3339 string form,
* a duplicate key within one document is ``E_PARSE``.
"""

from __future__ import annotations

import datetime as _dt
import json
import math
import re
import tomllib
from pathlib import Path
from typing import Any

import yaml

from ._errors import E_PARSE, EntryconfError

#: Extensions that select a parser (SPEC §3 and §5).
SUFFIXES = (".json", ".yaml", ".yml", ".toml")

#: SPEC §2: a YAML document whose fully expanded tree would exceed this many
#: nodes is ``E_PARSE``. Each scalar value, sequence element and mapping entry
#: counts as one node.
MAX_EXPANDED_NODES = 1_000_000


def _parse_error(path: Path, detail: object) -> EntryconfError:
    return EntryconfError(E_PARSE, f"{path}: {detail}")


# --------------------------------------------------------------------------
# JSON
# --------------------------------------------------------------------------


def _json_object_pairs(pairs: list[tuple[str, Any]]) -> dict[str, Any]:
    obj: dict[str, Any] = {}
    for key, value in pairs:
        if key in obj:
            raise ValueError(f"duplicate key {key!r}")
        obj[key] = value
    return obj


def _json_constant(name: str) -> Any:
    raise ValueError(f"{name} is not a JSON value")


def parse_json(text: str, path: Path) -> Any:
    try:
        return json.loads(
            text,
            object_pairs_hook=_json_object_pairs,
            parse_constant=_json_constant,
        )
    except (json.JSONDecodeError, ValueError, RecursionError) as exc:
        raise _parse_error(path, exc) from exc


# --------------------------------------------------------------------------
# YAML 1.2 core schema
# --------------------------------------------------------------------------

_CORE_BOOL = re.compile(r"^(?:true|True|TRUE|false|False|FALSE)$")
_CORE_NULL = re.compile(r"^(?:~|null|Null|NULL|)$")
# The core schema's three integer forms. The decimal alternative is plain
# `[0-9]+`, so a leading zero is just a leading zero (SPEC §2): unquoted `010`
# is decimal 10, and `0o10` is the only octal spelling. YAML 1.1 — and PyYAML's
# stock resolver — read `010` as octal 8 instead, which is exactly why the
# resolvers here are rebuilt from `BaseResolver`.
_CORE_INT = re.compile(r"^(?:[-+]?[0-9]+|0o[0-7]+|0x[0-9a-fA-F]+)$")
_CORE_FLOAT = re.compile(
    r"""^(?:
          [-+]?(?:\.[0-9]+|[0-9]+(?:\.[0-9]*)?)(?:[eE][-+]?[0-9]+)?
        | [-+]?\.(?:inf|Inf|INF)
        | \.(?:nan|NaN|NAN)
        )$""",
    re.VERBOSE,
)


class _CoreResolver(yaml.resolver.BaseResolver):
    """Only the YAML 1.2 core schema's implicit resolvers.

    Deriving from ``BaseResolver`` (not ``Resolver``) drops PyYAML's YAML 1.1
    resolvers wholesale: ``yes``/``no``/``on``/``off`` stay strings,
    sexagesimals and ``0``-prefixed octals stay strings or plain decimals.
    """


# Order matters: the first matching resolver wins, so ``int`` precedes
# ``float`` (both match "123").
_CoreResolver.add_implicit_resolver("tag:yaml.org,2002:bool", _CORE_BOOL, list("tTfF"))
_CoreResolver.add_implicit_resolver(
    "tag:yaml.org,2002:int", _CORE_INT, list("-+0123456789")
)
_CoreResolver.add_implicit_resolver(
    "tag:yaml.org,2002:float", _CORE_FLOAT, list("-+0123456789.")
)
_CoreResolver.add_implicit_resolver(
    "tag:yaml.org,2002:null", _CORE_NULL, ["~", "n", "N", ""]
)


class _CoreConstructor(yaml.constructor.SafeConstructor):
    """Constructors for exactly the core schema's tags; anything else fails."""

    def __init__(self) -> None:
        super().__init__()
        #: Expanded node count per composed node, filled in as construction
        #: proceeds. An alias composes to the *same* node object, so a shared
        #: node is constructed once but its expanded size is charged at every
        #: reference site — that is what makes the budget below a bound on the
        #: expanded tree rather than on the source.
        self.expanded_sizes: dict[yaml.Node, int] = {}


_CoreConstructor.yaml_constructors = {}
_CoreConstructor.yaml_multi_constructors = {}


def _bad_yaml(node: yaml.Node, detail: str) -> EntryconfError:
    mark = node.start_mark
    return EntryconfError(E_PARSE, f"line {mark.line + 1}, column {mark.column + 1}: {detail}")


# --------------------------------------------------------------------------
# The expansion budget (SPEC §2)
#
# Aliases turn the composed document into a DAG: `[*big, *big, *big]` is three
# edges to one node, so the *source* says nothing about how large the expanded
# tree is. Sizes are therefore accumulated bottom-up during construction —
# a scalar is 1; a sequence is its element count plus the expanded size of each
# element; a mapping is its entry count plus the expanded size of each value
# (keys are not counted; SPEC §2 counts scalar values, sequence elements and
# mapping entries) — and every partial sum is checked against the budget.
#
# Bottom-up accumulation over the DAG is what makes rejection instant: a nine
# deep, nine wide alias bomb expanding to ~48M nodes is settled in a few dozen
# steps, because the size of a shared node is computed once and then reused,
# never materialized. Checking each partial sum (rather than only a finished
# container) also stops a single wide sequence of huge aliases early, and keeps
# the running totals small.
# --------------------------------------------------------------------------


def _charge(loader: Any, node: yaml.Node, size: int) -> None:
    """Record ``node``'s expanded size, rejecting an over-budget document."""
    if size > MAX_EXPANDED_NODES:
        raise _bad_yaml(
            node,
            f"expanded document exceeds the {MAX_EXPANDED_NODES}-node budget "
            f"(alias expansion is bounded)",
        )
    loader.expanded_sizes[node] = size


def _expanded_size(loader: Any, node: yaml.Node) -> int:
    """The expanded size of an already-constructed node.

    Every constructor in this module records one, so a missing entry would be
    an internal bug rather than bad input; 1 is the conservative fallback.
    """
    return loader.expanded_sizes.get(node, 1)


def _scalar(loader: Any, node: yaml.Node) -> str:
    if not isinstance(node, yaml.ScalarNode):
        raise _bad_yaml(node, f"expected a scalar for tag {node.tag}")
    _charge(loader, node, 1)
    return loader.construct_scalar(node)


def _construct_null(loader: Any, node: yaml.Node) -> None:
    value = _scalar(loader, node)
    if not _CORE_NULL.match(value):
        raise _bad_yaml(node, f"{value!r} is not a core-schema null")
    return None


def _construct_bool(loader: Any, node: yaml.Node) -> bool:
    value = _scalar(loader, node)
    if not _CORE_BOOL.match(value):
        raise _bad_yaml(node, f"{value!r} is not a core-schema boolean")
    return value.lower() == "true"


def _construct_int(loader: Any, node: yaml.Node) -> int:
    value = _scalar(loader, node)
    if not _CORE_INT.match(value):
        raise _bad_yaml(node, f"{value!r} is not a core-schema integer")
    if value.startswith("0x"):
        return int(value[2:], 16)
    if value.startswith("0o"):
        return int(value[2:], 8)
    return int(value, 10)


def _construct_float(loader: Any, node: yaml.Node) -> float:
    value = _scalar(loader, node)
    if not _CORE_FLOAT.match(value):
        raise _bad_yaml(node, f"{value!r} is not a core-schema float")
    lowered = value.lower()
    if lowered.endswith(".inf") or lowered == ".nan":
        # SPEC §2: a value with no JSON-equivalent form is E_PARSE.
        raise _bad_yaml(node, f"{value!r} has no JSON-equivalent form")
    number = float(value)
    if not math.isfinite(number):
        raise _bad_yaml(node, f"{value!r} has no JSON-equivalent form")
    return number


def _construct_str(loader: Any, node: yaml.Node) -> str:
    return _scalar(loader, node)


def _construct_seq(loader: Any, node: yaml.Node) -> list[Any]:
    if not isinstance(node, yaml.SequenceNode):
        raise _bad_yaml(node, "expected a sequence")
    items: list[Any] = []
    size = len(node.value)  # one node per element ...
    _charge(loader, node, size)
    for child in node.value:
        items.append(loader.construct_object(child, deep=True))
        size += _expanded_size(loader, child)  # ... plus what it expands to
        _charge(loader, node, size)
    return items


def _construct_map(loader: Any, node: yaml.Node) -> dict[str, Any]:
    if not isinstance(node, yaml.MappingNode):
        raise _bad_yaml(node, "expected a mapping")
    result: dict[str, Any] = {}
    size = len(node.value)  # one node per entry ...
    _charge(loader, node, size)
    for key_node, value_node in node.value:
        key = loader.construct_object(key_node, deep=True)
        if not isinstance(key, str):
            raise _bad_yaml(key_node, "object keys must be strings")
        if key in result:
            raise _bad_yaml(key_node, f"duplicate key {key!r}")
        result[key] = loader.construct_object(value_node, deep=True)
        size += _expanded_size(loader, value_node)  # ... plus the value's size
        _charge(loader, node, size)
    return result


def _construct_undefined(loader: Any, node: yaml.Node) -> Any:
    raise _bad_yaml(node, f"unsupported tag {node.tag!r}")


_CoreConstructor.add_constructor("tag:yaml.org,2002:null", _construct_null)
_CoreConstructor.add_constructor("tag:yaml.org,2002:bool", _construct_bool)
_CoreConstructor.add_constructor("tag:yaml.org,2002:int", _construct_int)
_CoreConstructor.add_constructor("tag:yaml.org,2002:float", _construct_float)
_CoreConstructor.add_constructor("tag:yaml.org,2002:str", _construct_str)
_CoreConstructor.add_constructor("tag:yaml.org,2002:seq", _construct_seq)
_CoreConstructor.add_constructor("tag:yaml.org,2002:map", _construct_map)
_CoreConstructor.add_constructor(None, _construct_undefined)


class _CoreLoader(  # type: ignore[misc]
    yaml.reader.Reader,
    yaml.scanner.Scanner,
    yaml.parser.Parser,
    yaml.composer.Composer,
    _CoreConstructor,
    _CoreResolver,
):
    def __init__(self, stream: str) -> None:
        yaml.reader.Reader.__init__(self, stream)
        yaml.scanner.Scanner.__init__(self)
        yaml.parser.Parser.__init__(self)
        yaml.composer.Composer.__init__(self)
        _CoreConstructor.__init__(self)
        _CoreResolver.__init__(self)


def parse_yaml(text: str, path: Path) -> Any:
    loader = _CoreLoader(text)
    try:
        data = loader.get_single_data()
    except EntryconfError as exc:
        raise EntryconfError(exc.code, f"{path}: {exc.message}") from exc
    except yaml.YAMLError as exc:
        raise _parse_error(path, exc) from exc
    finally:
        loader.dispose()
    return data


# --------------------------------------------------------------------------
# TOML
# --------------------------------------------------------------------------


def _fraction(microsecond: int) -> str:
    """Fractional seconds with trailing zeros dropped (SPEC §2).

    The ``.`` goes with them when the fraction reaches zero, so an all-zero
    fraction renders as the empty string.
    """
    digits = f"{microsecond:06d}".rstrip("0")
    return f".{digits}" if digits else ""


def _offset(value: _dt.datetime | _dt.time) -> str:
    """The offset fragment: ``Z`` for UTC, the numeric form otherwise.

    A local (offset-less) value keeps its offset-less grammar fragment, so it
    contributes nothing.
    """
    delta = value.utcoffset()
    if delta is None:
        return ""
    total = int(delta.total_seconds())
    if total == 0:
        # Source `Z`, `z`, and `+00:00` all render as `Z`.
        return "Z"
    sign = "-" if total < 0 else "+"
    minutes = abs(total) // 60
    return f"{sign}{minutes // 60:02d}:{minutes % 60:02d}"


def _rfc3339(value: _dt.date | _dt.time) -> str:
    """TOML datetimes become their RFC 3339 string form (SPEC §2).

    The separator is always uppercase ``T``; a UTC offset becomes ``Z`` while
    any other offset keeps its authored numeric form (values are never shifted
    between zones); fractional seconds drop trailing zeros.
    """
    if isinstance(value, _dt.datetime):
        date = f"{value.year:04d}-{value.month:02d}-{value.day:02d}"
        clock = f"{value.hour:02d}:{value.minute:02d}:{value.second:02d}"
        return f"{date}T{clock}{_fraction(value.microsecond)}{_offset(value)}"
    if isinstance(value, _dt.time):
        clock = f"{value.hour:02d}:{value.minute:02d}:{value.second:02d}"
        return f"{clock}{_fraction(value.microsecond)}{_offset(value)}"
    return f"{value.year:04d}-{value.month:02d}-{value.day:02d}"


def _toml_scalars(value: Any, path: Path) -> Any:
    if isinstance(value, dict):
        return {key: _toml_scalars(item, path) for key, item in value.items()}
    if isinstance(value, list):
        return [_toml_scalars(item, path) for item in value]
    # datetime is a date subclass, so it must be tested first (it is, inside
    # `_rfc3339`); bool is an int subclass but needs no special handling here.
    if isinstance(value, (_dt.datetime, _dt.date, _dt.time)):
        return _rfc3339(value)
    if isinstance(value, float) and not math.isfinite(value):
        # SPEC §2: no JSON-equivalent form (TOML `inf`/`nan`).
        raise _parse_error(path, f"{value!r} has no JSON-equivalent form")
    return value


def parse_toml(text: str, path: Path) -> Any:
    try:
        # tomllib rejects duplicate keys itself.
        data = tomllib.loads(text)
    except (tomllib.TOMLDecodeError, ValueError) as exc:
        raise _parse_error(path, exc) from exc
    return _toml_scalars(data, path)


# --------------------------------------------------------------------------
# Dispatch
# --------------------------------------------------------------------------


def parse_document(path: Path, text: str) -> Any:
    """Parse ``text`` using the parser selected by ``path``'s extension.

    Extensions are matched case-sensitively (SPEC §5): ``.JSON`` is not a
    recognized extension.
    """
    suffix = path.suffix
    if suffix == ".json":
        return parse_json(text, path)
    if suffix in (".yaml", ".yml"):
        return parse_yaml(text, path)
    if suffix == ".toml":
        return parse_toml(text, path)
    raise _parse_error(path, f"unsupported extension {suffix!r}")
