"""Document parsers (SPEC §2).

JSON / YAML / TOML are parsed with stock parsers, constrained to the spec's
data model:

* the loaded tree is JSON-equivalent (null, bool, number, string, array,
  object with string keys),
* YAML uses the **YAML 1.2 core schema** (PyYAML's own resolvers implement
  YAML 1.1, so the resolvers and constructors are rebuilt here),
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


_CoreConstructor.yaml_constructors = {}
_CoreConstructor.yaml_multi_constructors = {}


def _bad_yaml(node: yaml.Node, detail: str) -> EntryconfError:
    mark = node.start_mark
    return EntryconfError(E_PARSE, f"line {mark.line + 1}, column {mark.column + 1}: {detail}")


def _scalar(loader: Any, node: yaml.Node) -> str:
    if not isinstance(node, yaml.ScalarNode):
        raise _bad_yaml(node, f"expected a scalar for tag {node.tag}")
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
    return [loader.construct_object(child, deep=True) for child in node.value]


def _construct_map(loader: Any, node: yaml.Node) -> dict[str, Any]:
    if not isinstance(node, yaml.MappingNode):
        raise _bad_yaml(node, "expected a mapping")
    result: dict[str, Any] = {}
    for key_node, value_node in node.value:
        key = loader.construct_object(key_node, deep=True)
        if not isinstance(key, str):
            raise _bad_yaml(key_node, "object keys must be strings")
        if key in result:
            raise _bad_yaml(key_node, f"duplicate key {key!r}")
        result[key] = loader.construct_object(value_node, deep=True)
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
