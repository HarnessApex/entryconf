"""Source-aware editing (SPEC §10): ``open`` → ``Snapshot`` → ``Plan`` → commit."""

from __future__ import annotations

import hashlib
import math
import os
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any

from ._env import ProcEnv, Vars, load_env_files, parse_env_file
from ._errors import E_EDIT, E_NO_ENTRYPOINT, E_PATH, E_STALE_PLAN, E_UNSUPPORTED_EDIT, EntryconfError
from ._includes import include_target, is_include
from ._interpolate import scan_references
from ._jsonfmt import format_json
from ._loader import Loader
from ._lock import DEFAULT_LOCK_TIMEOUT, acquire_lock, atomic_write, release_lock
from ._pointer import array_index, deep_copy, escape_token, parse_pointer, remove_at, set_at
from ._source import DocRecord, OsSource, OverlaySource, Recorder, document_key

OP_SET = "set"
OP_REMOVE = "remove"
MODE_LITERAL = "literal"
MODE_EXPRESSION = "expression"


@dataclass(frozen=True)
class Reference:
    """An ``@file:`` string: the document holding it and its pointer there."""

    document: str
    pointer: str

    def to_json(self) -> dict[str, Any]:
        return {"document": self.document, "pointer": self.pointer}


@dataclass(frozen=True)
class Graft:
    """One position a document's root occupies in the effective tree, with the
    ``@file:`` references traversed to reach it (SPEC §10.3)."""

    effective: str
    chain: tuple[Reference, ...]

    def to_json(self) -> dict[str, Any]:
        return {"effective": self.effective, "chain": [r.to_json() for r in self.chain]}


@dataclass(frozen=True)
class Document:
    """One source file of a snapshot (SPEC §10.2)."""

    key: str
    path: str
    format: str  # "json", "yaml", "toml", or "env"
    revision: str
    writable: bool  # True iff format == "json"
    grafts: tuple[Graft, ...]

    def to_json(self) -> dict[str, Any]:
        return {
            "key": self.key,
            "path": self.path,
            "format": self.format,
            "revision": self.revision,
            "writable": self.writable,
            "grafts": [g.to_json() for g in self.grafts],
        }


@dataclass(frozen=True)
class Variable:
    """One ``$`` reference and the SPEC §4 layer supplying it: ``"process"``,
    ``"file"`` (``file`` is the ``*.env`` document key), or ``"default"``."""

    name: str
    origin: str
    file: str | None = None

    def to_json(self) -> dict[str, Any]:
        out: dict[str, Any] = {"name": self.name, "origin": self.origin}
        if self.origin == "file":
            out["file"] = self.file
        return out


@dataclass(frozen=True)
class Origin:
    """The provenance of one effective value (SPEC §10.3)."""

    effective: str
    document: str
    pointer: str
    authored: Any
    chain: tuple[Reference, ...]
    variables: tuple[Variable, ...]
    writable: bool

    def to_json(self) -> dict[str, Any]:
        return {
            "effective": self.effective,
            "document": self.document,
            "pointer": self.pointer,
            "authored": self.authored,
            "chain": [r.to_json() for r in self.chain],
            "variables": [v.to_json() for v in self.variables],
            "writable": self.writable,
        }


@dataclass(frozen=True)
class Edit:
    """One operation on one source document (SPEC §10.4).

    ``value`` is used by ``set`` only: ``None``, bool, int, float, str, list,
    or a str-keyed dict. ``mode`` is ``"literal"`` (strings are escaped so they
    load back as themselves) or ``"expression"`` (a string written verbatim as
    an entryconf expression).
    """

    document: str
    op: str
    pointer: str
    value: Any = None
    mode: str = MODE_LITERAL

    @classmethod
    def from_json(cls, obj: dict[str, Any]) -> "Edit":
        return cls(
            document=obj.get("document", ""),
            op=obj.get("op", ""),
            pointer=obj.get("pointer", ""),
            value=obj.get("value"),
            mode=obj.get("mode") or MODE_LITERAL,
        )


@dataclass(frozen=True)
class Receipt:
    """What a successful commit returns."""

    document: str
    revision: str
    revisions: dict[str, str]

    def to_json(self) -> dict[str, Any]:
        return {"document": self.document, "revision": self.revision, "revisions": dict(self.revisions)}


def _revision(data: bytes) -> str:
    return "sha256:" + hashlib.sha256(data).hexdigest()


def _variables_revision(names: list[str], env: Vars) -> str:
    text = []
    for name in names:
        value = env.lookup(name)
        text.append(f"={name}={value}\n" if value is not None else f"-{name}\n")
    return _revision("".join(text).encode("utf-8"))


@dataclass
class Plan:
    """A prepared, validated change to one document (SPEC §10.6). Nothing has
    been written; :meth:`commit` writes ``after`` if every dependency is
    unchanged."""

    document: str
    before: str
    after: str
    candidate: dict[str, Any]
    affected: list[str]
    grafts: tuple[Graft, ...]
    revisions: dict[str, str]
    variables: list[str]
    variables_revision: str

    _dir: Path = field(repr=False, compare=False)
    _path: Path = field(repr=False, compare=False)
    _paths: dict[str, Path] = field(repr=False, compare=False)
    _live: ProcEnv = field(repr=False, compare=False)

    def to_json(self) -> dict[str, Any]:
        return {
            "document": self.document,
            "before": self.before,
            "after": self.after,
            "candidate": self.candidate,
            "affected": list(self.affected),
            "grafts": [g.to_json() for g in self.grafts],
            "revisions": dict(self.revisions),
            "variables": list(self.variables),
            "variables_revision": self.variables_revision,
        }

    def commit(self, lock_timeout: float = DEFAULT_LOCK_TIMEOUT) -> Receipt:
        """Write ``after`` (SPEC §10.7): lock, recheck every dependency's
        revision and the variables fingerprint (``E_STALE_PLAN`` on any
        change), write atomically (``E_WRITE`` on failure, target untouched),
        unlock."""
        lock = acquire_lock(self._dir, lock_timeout)
        try:
            files: dict[str, str] = {}
            origin: dict[str, Path] = {}
            for key, want in self.revisions.items():
                path = self._paths[key]
                try:
                    data = path.read_bytes()
                except OSError as exc:
                    raise EntryconfError(E_STALE_PLAN, f"document {key!r} can no longer be read: {exc}") from exc
                got = _revision(data)
                if got != want:
                    raise EntryconfError(
                        E_STALE_PLAN,
                        f"document {key!r} changed since the plan was made ({got}, plan expected {want})",
                    )
                if key.endswith(".env"):
                    try:
                        parsed = parse_env_file(data.decode("utf-8"), path)
                    except (EntryconfError, UnicodeDecodeError) as exc:
                        raise EntryconfError(E_STALE_PLAN, f"document {key!r}: {exc}") from exc
                    for name, value in parsed.items():
                        files[name] = value
                        origin[name] = path
            env = Vars(files, origin, self._live)
            if _variables_revision(self.variables, env) != self.variables_revision:
                raise EntryconfError(
                    E_STALE_PLAN,
                    f"a variable the candidate depends on changed since the plan was made ({self.variables})",
                )
            data = self.after.encode("utf-8")
            atomic_write(self._path, data)
        finally:
            release_lock(lock)
        revisions = dict(self.revisions)
        revisions[self.document] = _revision(data)
        return Receipt(document=self.document, revision=revisions[self.document], revisions=revisions)


@dataclass
class Snapshot:
    """What :func:`open` returns (SPEC §10.2): the effective tree, every source
    document the load read, and the process environment as captured at open
    time. Immutable; reads nothing from disk after ``open`` returns."""

    dir: str
    tree: dict[str, Any]
    documents: dict[str, Document]

    _by_path: dict[str, DocRecord] = field(repr=False, compare=False)
    _env_names: list[str] = field(repr=False, compare=False)
    _captured: dict[str, str] = field(repr=False, compare=False)
    _live: ProcEnv = field(repr=False, compare=False)

    def to_json(self) -> dict[str, Any]:
        return {
            "dir": self.dir,
            "documents": {key: doc.to_json() for key, doc in self.documents.items()},
        }

    # -- provenance ---------------------------------------------------------

    def _entrypoint(self) -> DocRecord:
        for name in ("entrypoint.json", "entrypoint.yaml", "entrypoint.yml", "entrypoint.toml"):
            rec = self._by_path.get(str(Path(self.dir) / name))
            if rec is not None and rec.format:
                return rec
        raise EntryconfError(E_NO_ENTRYPOINT, f"snapshot of {self.dir} has no entrypoint")

    def inspect(self, pointer: str) -> Origin:
        """Where the effective value at ``pointer`` comes from (SPEC §10.3)."""
        tokens = parse_pointer(pointer)
        doc = self._entrypoint()
        node: Any = doc.parsed
        src = ""
        chain: list[Reference] = []
        directory = Path(self.dir)

        def follow() -> None:
            nonlocal doc, node, src
            while isinstance(node, str) and is_include(node):
                target = include_target(node, doc.path.parent)
                nxt = self._by_path.get(str(target))
                if nxt is None:
                    raise EntryconfError(E_PATH, f"include {target} was not loaded by this snapshot")
                chain.append(Reference(document_key(directory, doc.path), src))
                doc, node, src = nxt, nxt.parsed, ""

        for tok in tokens:
            follow()
            if isinstance(node, dict):
                if tok not in node:
                    raise EntryconfError(E_PATH, f"effective pointer {pointer!r}: no member {tok!r}")
                node = node[tok]
            elif isinstance(node, list):
                i = array_index(tok)
                if i is None or i >= len(node):
                    raise EntryconfError(E_PATH, f"effective pointer {pointer!r}: array index {tok!r} does not exist")
                node = node[i]
            else:
                raise EntryconfError(E_PATH, f"effective pointer {pointer!r}: cannot descend into a scalar at {tok!r}")
            src += "/" + escape_token(tok)
        follow()

        key = document_key(directory, doc.path)
        variables: tuple[Variable, ...] = ()
        if isinstance(node, str):
            variables = self._variables_of(node)
        return Origin(
            effective=pointer,
            document=key,
            pointer=src,
            authored=deep_copy(node),
            chain=tuple(chain),
            variables=variables,
            writable=self.documents[key].writable,
        )

    def _variables_of(self, authored: str) -> tuple[Variable, ...]:
        directory = Path(self.dir)
        files, origin = load_env_files(directory, self._source({}))
        env = Vars(files, origin, self._captured.get)
        out: list[Variable] = []
        for name, has_default in scan_references(authored):
            where, path = env.where(name)
            if where == "process":
                out.append(Variable(name, "process"))
            elif where == "file":
                out.append(Variable(name, "file", document_key(directory, path)))  # type: ignore[arg-type]
            elif has_default:
                out.append(Variable(name, "default"))
            else:  # unreachable: the load would have failed
                out.append(Variable(name, "unset"))
        return tuple(out)

    # -- planning -----------------------------------------------------------

    def _source(self, overrides: dict[str, bytes]) -> OverlaySource:
        files = {path: rec.data for path, rec in self._by_path.items()}
        files.update(overrides)
        return OverlaySource(files, self._env_names, OsSource())

    def plan(self, edits: list[Edit]) -> Plan:
        """Validate ``edits`` (SPEC §10.4), apply them to a copy of the selected
        document (§10.5), and evaluate the candidate with the new text
        substituted in memory (§10.6). No file is changed."""
        if not edits:
            raise EntryconfError(E_EDIT, "a plan needs at least one edit")
        key = edits[0].document
        for e in edits:
            if e.document != key:
                raise EntryconfError(
                    E_UNSUPPORTED_EDIT,
                    f"edits name both {key!r} and {e.document!r}; a plan edits exactly one document",
                )
        doc = self.documents.get(key)
        if doc is None:
            raise EntryconfError(E_EDIT, f"no document {key!r} in the snapshot of {self.dir}")
        if not doc.writable:
            raise EntryconfError(
                E_UNSUPPORTED_EDIT, f"document {key!r} is {doc.format}; only JSON documents are writable"
            )
        rec = self._by_path[doc.path]

        value = deep_copy(rec.parsed)
        for i, e in enumerate(edits):
            if e.op == OP_SET:
                v = _normalize_value(e.value)
                if e.mode in ("", MODE_LITERAL):
                    v = _escape_literal(v)
                elif e.mode == MODE_EXPRESSION:
                    if not isinstance(v, str):
                        raise EntryconfError(E_EDIT, f"edit {i}: expression mode requires a string value")
                else:
                    raise EntryconfError(E_EDIT, f"edit {i}: unknown mode {e.mode!r}")
                value = set_at(value, e.pointer, v)
            elif e.op == OP_REMOVE:
                value = remove_at(value, e.pointer)
            else:
                raise EntryconfError(E_EDIT, f"edit {i}: unknown op {e.op!r} (want 'set' or 'remove')")
        after = format_json(value)

        directory = Path(self.dir)
        cand_rec = Recorder()
        loader = Loader(self._source({doc.path: after.encode("utf-8")}), self._captured.get, cand_rec)
        candidate = loader.load_tree(directory)
        assert loader.vars is not None

        revisions: dict[str, str] = {}
        paths: dict[str, Path] = {}
        for path in cand_rec.order:
            d = cand_rec.docs[path]
            k = document_key(directory, d.path)
            revisions[k] = _revision(d.data)
            paths[k] = d.path
        # The edited document's own revision is what is on disk now, not the
        # new text: commit must find the file as the snapshot saw it.
        revisions[key] = doc.revision
        names = sorted(loader.vars.used)
        return Plan(
            document=key,
            before=rec.data.decode("utf-8"),
            after=after,
            candidate=candidate,
            affected=_affected_pointers(self.tree, candidate),
            grafts=doc.grafts,
            revisions=revisions,
            variables=names,
            variables_revision=_variables_revision(names, loader.vars),
            _dir=directory,
            _path=Path(doc.path),
            _paths=paths,
            _live=self._live,
        )


def _open_with_env(config_dir: str | os.PathLike[str], captured: dict[str, str], live: ProcEnv) -> Snapshot:
    """``open`` with the captured environment and the live lookup injected."""
    directory = Path(os.path.abspath(config_dir))
    try:
        env_names = OsSource().env_file_names(directory)
    except OSError:
        env_names = []  # the load itself reports the failure
    rec = Recorder()
    tree = Loader(OsSource(), captured.get, rec).load_tree(directory)
    documents: dict[str, Document] = {}
    for path in rec.order:
        d = rec.docs[path]
        key = document_key(directory, d.path)
        grafts = sorted(d.grafts, key=lambda g: g["effective"])
        documents[key] = Document(
            key=key,
            path=str(d.path),
            format=d.format,
            revision=_revision(d.data),
            writable=d.format == "json",
            grafts=tuple(
                Graft(
                    g["effective"],
                    tuple(
                        Reference(document_key(directory, Path(r["document"])), r["pointer"]) for r in g["chain"]
                    ),
                )
                for g in grafts
            ),
        )
    return Snapshot(
        dir=str(directory),
        tree=tree,
        documents=documents,
        _by_path=rec.docs,
        _env_names=env_names,
        _captured=captured,
        _live=live,
    )


def open(config_dir: str | os.PathLike[str]) -> Snapshot:  # noqa: A001 - mirrors gzip.open
    """Load ``config_dir`` like :func:`load` and return a :class:`Snapshot` for
    inspecting provenance and preparing edits (SPEC §10.2). The process
    environment is captured now; plans resolve variables against that
    capture, and ``commit`` fails with ``E_STALE_PLAN`` if a variable the
    candidate depends on has since changed."""
    return _open_with_env(config_dir, dict(os.environ), os.environ.get)


# -- helpers ---------------------------------------------------------------


def _escape_literal(value: Any) -> Any:
    """Make a value load back as itself (SPEC §10.5 literal mode)."""
    if isinstance(value, str):
        s = value.replace("$", "$$")
        return "@" + s if s.startswith("@") else s
    if isinstance(value, dict):
        return {k: _escape_literal(v) for k, v in value.items()}
    if isinstance(value, list):
        return [_escape_literal(v) for v in value]
    return value


def _normalize_value(value: Any) -> Any:
    """The loader's JSON shapes, or ``E_EDIT`` for a value with no JSON form."""
    if value is None or isinstance(value, (bool, str)):
        return value
    if isinstance(value, int):
        return value
    if isinstance(value, float):
        if math.isinf(value) or math.isnan(value):
            raise EntryconfError(E_EDIT, f"number {value!r} has no JSON-equivalent form")
        return value
    if isinstance(value, dict):
        out: dict[str, Any] = {}
        for k, v in value.items():
            if not isinstance(k, str):
                raise EntryconfError(E_EDIT, f"map keys must be strings, not {type(k).__name__}")
            out[k] = _normalize_value(v)
        return out
    if isinstance(value, (list, tuple)):
        return [_normalize_value(v) for v in value]
    raise EntryconfError(E_EDIT, f"value of type {type(value).__name__} has no JSON-equivalent form")


def _is_number(v: Any) -> bool:
    return isinstance(v, (int, float)) and not isinstance(v, bool)


def _affected_pointers(before: Any, after: Any) -> list[str]:
    """SPEC §10.6's diff: the shallowest pointers where the trees differ."""
    out: list[str] = []
    _diff_into(before, after, "", out)
    return sorted(out)


def _diff_into(a: Any, b: Any, ptr: str, out: list[str]) -> None:
    if isinstance(a, dict):
        if not isinstance(b, dict):
            out.append(ptr)
            return
        for k in set(a) | set(b):
            child = ptr + "/" + escape_token(k)
            if k not in a or k not in b:
                out.append(child)
            else:
                _diff_into(a[k], b[k], child, out)
    elif isinstance(a, list):
        if not isinstance(b, list) or len(a) != len(b):
            out.append(ptr)
            return
        for i, (x, y) in enumerate(zip(a, b)):
            _diff_into(x, y, f"{ptr}/{i}", out)
    else:
        if not _scalar_equal(a, b):
            out.append(ptr)


def _scalar_equal(a: Any, b: Any) -> bool:
    if _is_number(a):
        return _is_number(b) and float(a) == float(b)
    if isinstance(b, (dict, list)) or _is_number(b):
        return False
    return type(a) is type(b) and a == b


