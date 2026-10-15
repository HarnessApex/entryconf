"""Refreshing a snapshot (SPEC §12.2): open again and say what changed."""

from __future__ import annotations

import os
from dataclasses import dataclass, field
from typing import Any

from ._edit import Snapshot, _affected_pointers, _open_tracked
from ._env import ProcEnv
from ._errors import EntryconfError
from ._source import OsSource, TrackingSource


@dataclass(frozen=True)
class DocumentChanges:
    """Document keys between two snapshots, each sorted by code point."""

    added: tuple[str, ...] = ()
    removed: tuple[str, ...] = ()
    changed: tuple[str, ...] = ()

    def to_json(self) -> dict[str, Any]:
        return {"added": list(self.added), "removed": list(self.removed), "changed": list(self.changed)}


@dataclass(frozen=True)
class Refreshed:
    """What :func:`refresh` reports (SPEC §12.2).

    ``snapshot`` is the new snapshot (the old one when not stale), ``None``
    when the open failed; ``error`` is then the open's failure and the old
    snapshot stays valid. ``tried``, after a failed open, is every path it
    asked about, including those it could not read: a watcher watches them so
    that creating a missing include is seen.
    """

    stale: bool
    snapshot: Snapshot | None
    error: EntryconfError | None = None
    changed: tuple[str, ...] = ()
    documents: DocumentChanges = field(default_factory=DocumentChanges)
    tried: tuple[str, ...] = ()

    def to_json(self) -> dict[str, Any]:
        out: dict[str, Any] = {
            "stale": self.stale,
            "changed": list(self.changed),
            "documents": self.documents.to_json(),
        }
        if self.error is not None:
            out["error"] = self.error.code
        elif self.snapshot is not None:
            out["tree"] = self.snapshot.tree
        return out


def refresh(snapshot: Snapshot) -> Refreshed:
    """Open ``snapshot``'s directory again, against the process environment as
    it is now, and report whether and what changed (SPEC §12.2). Never writes
    a file."""
    return _refresh(snapshot, dict(os.environ), os.environ.get)


def _refresh(snapshot: Snapshot, captured: dict[str, str], live: ProcEnv) -> Refreshed:
    src = TrackingSource(OsSource())
    try:
        new = _open_tracked(snapshot.dir, captured, live, src)
    except EntryconfError as exc:
        return Refreshed(stale=True, snapshot=None, error=exc, tried=tuple(src.paths()))
    old_docs, new_docs = snapshot.documents, new.documents
    added = sorted(k for k in new_docs if k not in old_docs)
    removed = sorted(k for k in old_docs if k not in new_docs)
    changed = sorted(k for k in new_docs if k in old_docs and old_docs[k].revision != new_docs[k].revision)
    stale = bool(added or removed or changed) or new._vars_revision != snapshot._vars_revision
    if not stale:
        return Refreshed(stale=False, snapshot=snapshot)
    return Refreshed(
        stale=True,
        snapshot=new,
        changed=tuple(_affected_pointers(snapshot.tree, new.tree)),
        documents=DocumentChanges(tuple(added), tuple(removed), tuple(changed)),
    )
