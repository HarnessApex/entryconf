"""``watch`` (SPEC §12.3): hold a current snapshot of a config directory and
hand each change on disk to a callback that accepts or refuses it.

This module needs ``watchdog`` (``pip install entryconf[watch]``); importing
:mod:`entryconf` alone never does. Changes are reported by the platform
(inotify, FSEvents, kqueue, ReadDirectoryChangesW); nothing is polled. Like
every platform watch, it does not see changes made on another machine to a
network filesystem (NFS, SMB).

    >>> from entryconf.watch import watch            # doctest: +SKIP
    >>> w = watch("envs/prod", on_change)             # doctest: +SKIP
    >>> w.close()                                     # doctest: +SKIP
"""

from __future__ import annotations

import os
import queue
import threading
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any, Callable

from watchdog.events import FileSystemEvent, FileSystemEventHandler
from watchdog.observers import Observer

from ._edit import Snapshot, open as open_snapshot
from ._errors import E_WATCH, EntryconfError
from ._refresh import DocumentChanges, refresh

DEFAULT_SETTLE = 0.1
"""Seconds a watcher waits after the last event of a burst before it refreshes."""

_ENTRYPOINTS = frozenset({"entrypoint.json", "entrypoint.yaml", "entrypoint.yml", "entrypoint.toml"})


@dataclass(frozen=True)
class Change:
    """What a callback is handed (SPEC §12.3).

    ``old`` is the current snapshot, ``new`` the refreshed one (``None`` when
    ``error`` is set). ``error`` is the open's failure, or ``E_WATCH`` when the
    platform refused to watch a path; ``old`` stays current whatever the
    callback does.
    """

    old: Snapshot
    new: Snapshot | None
    changed: tuple[str, ...] = ()
    documents: DocumentChanges = field(default_factory=DocumentChanges)
    error: EntryconfError | None = None


Callback = Callable[[Change], Any]
"""Returning accepts a change (``new`` becomes current); raising refuses it
(``old`` stays current, and the next change is refreshed from ``old``).
Callbacks never overlap. A callback must not call ``close`` on its own
watcher: ``close`` waits for the callback."""


def _schedule(observer: Any, handler: FileSystemEventHandler, directory: str) -> Any:
    """The seam the tests use to make the platform refuse a directory."""
    return observer.schedule(handler, directory, recursive=False)


class _Handler(FileSystemEventHandler):
    def __init__(self, events: "queue.Queue[str | None]") -> None:
        self.events = events

    def on_any_event(self, event: FileSystemEvent) -> None:
        if event.event_type in ("opened", "closed_no_write"):
            return
        for path in (event.src_path, getattr(event, "dest_path", "")):
            if path:
                self.events.put(os.fsdecode(path))


class Watcher:
    """Watches one config directory. Make one with :func:`watch`."""

    def __init__(self, snapshot: Snapshot, callback: Callback, settle: float) -> None:
        self._callback = callback
        self._settle = settle
        self._lock = threading.Lock()
        self._current = snapshot
        self._config = snapshot.dir
        self._config_real = os.path.realpath(snapshot.dir)
        self._events: queue.Queue[str | None] = queue.Queue()
        self._handler = _Handler(self._events)
        self._observer = Observer()
        self._watches: dict[str, Any] = {}
        self._names: dict[str, set[str]] = {}
        self._closing = threading.Event()
        self._closed = False
        self._arm(_targets(self._config, _document_paths(snapshot)))
        self._observer.start()
        self._thread = threading.Thread(target=self._loop, name="entryconf-watch", daemon=True)
        self._thread.start()

    @property
    def current(self) -> Snapshot:
        """The snapshot :func:`watch` opened, or the last one a callback accepted."""
        with self._lock:
            return self._current

    def close(self) -> None:
        """Stop the watch. A callback running is waited for; none starts after
        ``close`` returns."""
        with self._lock:
            if self._closed:
                return
            self._closed = True
        self._closing.set()
        self._events.put(None)
        self._thread.join()
        self._observer.stop()
        self._observer.join()

    def __enter__(self) -> "Watcher":
        return self

    def __exit__(self, *exc: object) -> None:
        self.close()

    # -- the loop -----------------------------------------------------------

    def _loop(self) -> None:
        # A change made between open and the watches being placed is caught by
        # one refresh at the start; it calls nothing if nothing changed.
        due = True
        while not self._closing.is_set():
            try:
                path = self._events.get(timeout=self._settle if due else None)
            except queue.Empty:
                due = self._refresh()
                continue
            if path is None:
                return
            if self._relevant(path):
                due = True

    def _refresh(self) -> bool:
        """Refresh and deliver; True when a refresh is due again (a directory
        newly watched, where a change may have landed before its watch)."""
        old = self.current
        r = refresh(old)
        if not r.stale:
            return False
        accepted = self._deliver(Change(old, r.snapshot if r.error is None else None, r.changed, r.documents, r.error))
        if r.error is not None:
            paths = _document_paths(old) + list(r.tried)
        elif accepted:
            assert r.snapshot is not None
            with self._lock:
                self._current = r.snapshot
            paths = _document_paths(r.snapshot)
        else:
            assert r.snapshot is not None
            paths = _document_paths(old) + _document_paths(r.snapshot)
        try:
            return self._arm(_targets(self._config, paths))
        except EntryconfError as exc:
            self._deliver(Change(self.current, None, error=exc))
            return False

    def _deliver(self, change: Change) -> bool:
        if self._closing.is_set():
            return False
        try:
            self._callback(change)
        except Exception:  # noqa: BLE001 - raising is how a callback refuses
            return False
        return change.error is None

    def _relevant(self, path: str) -> bool:
        p = Path(path)
        directory, name = str(p.parent), p.name
        with self._lock:
            if name in self._names.get(directory, ()):
                return True
        in_config = directory in (self._config, self._config_real)
        return in_config and (name in _ENTRYPOINTS or name.endswith(".env"))

    def _arm(self, want: dict[str, set[str]]) -> bool:
        """Watch exactly the directories in ``want``; True if any was added.
        ``E_WATCH`` if the platform refused one (the others stay watched)."""
        failed: EntryconfError | None = None
        added = False
        for directory in list(want):
            if directory in self._watches:
                continue
            try:
                self._watches[directory] = _schedule(self._observer, self._handler, directory)
                added = True
            except OSError as exc:
                failed = failed or EntryconfError(E_WATCH, f"cannot watch {directory}: {exc}")
                del want[directory]
        for directory in list(self._watches):
            if directory not in want:
                self._observer.unschedule(self._watches.pop(directory))
        # Some platforms (FSEvents) report a directory by its real path.
        names = dict(want)
        for directory, entries in want.items():
            names.setdefault(os.path.realpath(directory), entries)
        with self._lock:
            self._names = names
        if failed is not None:
            raise failed
        return added


def _document_paths(snapshot: Snapshot) -> list[str]:
    """The snapshot's watch set (SPEC §12.1) as the paths the load resolved."""
    return [snapshot.documents[key].path for key in snapshot.watch_set()["documents"]]


def _targets(config: str, paths: list[str]) -> dict[str, set[str]]:
    """Directories to watch and the entry names in each that matter. A path's
    own directory is watched (an editor's rename replaces the file); a symbolic
    link is also watched where it points; a path that does not exist (what a
    failed open could not read) is watched at its nearest existing ancestor,
    by the name of the next step down, so its creation is seen."""
    out: dict[str, set[str]] = {config: set()}
    for raw in paths:
        p = Path(os.path.normpath(raw))
        if str(p) == config:
            continue
        q = p
        while True:
            if q.parent.is_dir():
                out.setdefault(str(q.parent), set()).add(q.name)
                break
            if q.parent == q:
                break
            q = q.parent
        if p.is_symlink():
            real = Path(os.path.realpath(p))
            out.setdefault(str(real.parent), set()).add(real.name)
    return out


def watch(config_dir: str | os.PathLike[str], callback: Callback, *, settle: float = DEFAULT_SETTLE) -> Watcher:
    """Open ``config_dir`` (:func:`entryconf.open`) and watch it (SPEC §12.3).

    A failed open raises its code; a watch the platform refuses raises
    ``E_WATCH``. ``callback`` is called with each stale refresh until
    :meth:`Watcher.close`.
    """
    snapshot = open_snapshot(config_dir)
    try:
        return Watcher(snapshot, callback, settle if settle > 0 else DEFAULT_SETTLE)
    except OSError as exc:
        raise EntryconfError(E_WATCH, f"cannot start watching {config_dir}: {exc}") from exc
