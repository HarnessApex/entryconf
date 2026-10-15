"""Unit tests for ``entryconf.watch`` (SPEC §12.3), against real directories."""

from __future__ import annotations

import os
import queue
import threading
import time
from pathlib import Path
from typing import Any, Callable

import pytest

from entryconf import E_INCLUDE, E_MULTIPLE_ENTRYPOINTS, E_WATCH, EntryconfError
from entryconf import watch as watch_module
from entryconf.watch import Change, Watcher, watch

SETTLE = 0.05
QUIET = 0.4


def write(path: Path, text: str) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(text, encoding="utf-8")


class Run:
    def __init__(self, tmp: Path, files: dict[str, str], answer: Callable[[Change], Any] | None = None) -> None:
        self.dir = tmp
        for name, text in files.items():
            write(tmp / name, text)
        self.changes: queue.Queue[Change] = queue.Queue()

        def callback(c: Change) -> None:
            self.changes.put(c)
            if answer is not None:
                answer(c)

        self.w: Watcher = watch(tmp, callback, settle=SETTLE)

    def next(self) -> Change:
        try:
            return self.changes.get(timeout=5)
        except queue.Empty:
            pytest.fail("no change delivered")

    def none(self) -> None:
        try:
            c = self.changes.get(timeout=QUIET)
        except queue.Empty:
            return
        pytest.fail(f"unexpected change: {c.changed} {c.documents} {c.error}")


@pytest.fixture
def run(tmp_path: Path):
    runs: list[Run] = []

    def make(files: dict[str, str], answer: Callable[[Change], Any] | None = None) -> Run:
        r = Run(tmp_path, files, answer)
        runs.append(r)
        return r

    yield make
    for r in runs:
        r.w.close()


def test_burst_is_one_change(run) -> None:
    r = run({"entrypoint.json": '{"n": 0}'})
    for i in range(1, 6):
        write(r.dir / "entrypoint.json", f'{{"n": {i}}}')
        time.sleep(SETTLE / 5)
    c = r.next()
    assert c.error is None and c.new is not None and c.new.tree["n"] == 5
    assert c.changed == ("/n",)
    r.none()
    assert r.w.current is c.new


def test_rename_over_is_one_change(run) -> None:
    r = run({"entrypoint.json": '{"db": "@file:db.json"}', "db.json": '{"port": 1}'})
    tmp = r.dir / ".db.json.swp"
    write(tmp, '{"port": 2}')
    os.replace(tmp, r.dir / "db.json")
    assert r.next().changed == ("/db/port",)
    r.none()
    write(r.dir / "db.json", '{"port": 3}')
    assert r.next().new.tree["db"]["port"] == 3


def test_no_empty_calls(run) -> None:
    r = run({"entrypoint.json": '{"n": 1}'})
    write(r.dir / "entrypoint.json", '{"n": 1}')
    write(r.dir / "notes.txt", "not config")
    r.none()


def test_refused_keeps_old(run) -> None:
    def answer(c: Change) -> None:
        if c.new is not None and c.new.tree["port"] == 0:
            raise ValueError("port out of range")

    r = run({"entrypoint.json": '{"port": 80}'}, answer)
    first = r.w.current
    write(r.dir / "entrypoint.json", '{"port": 0}')
    assert r.next().old is first
    assert r.w.current is first
    write(r.dir / "entrypoint.json", '{"port": 81}')
    c = r.next()
    assert c.old is first and c.changed == ("/port",) and r.w.current is c.new


def test_refused_new_include_is_watched(run) -> None:
    def answer(c: Change) -> None:
        if c.new is not None and c.new.tree.get("db", {}).get("port") == 0:
            raise ValueError("refused")

    r = run({"entrypoint.json": '{"port": 80}'}, answer)
    write(r.dir / "sub" / "db.json", '{"port": 0}')
    write(r.dir / "entrypoint.json", '{"port": 80, "db": "@file:sub/db.json"}')
    r.next()
    write(r.dir / "sub" / "db.json", '{"port": 5432}')
    c = r.next()
    assert c.error is None and r.w.current is c.new and c.new.tree["db"]["port"] == 5432


def test_one_at_a_time(run) -> None:
    running = threading.Lock()
    overlapped = []
    release = threading.Event()

    def answer(c: Change) -> None:
        if not running.acquire(blocking=False):
            overlapped.append(c)
            return
        try:
            if c.new is not None and c.new.tree["n"] == 1:
                release.wait(10)
        finally:
            running.release()

    r = run({"entrypoint.json": '{"n": 0}'}, answer)
    write(r.dir / "entrypoint.json", '{"n": 1}')
    first = r.next()
    write(r.dir / "entrypoint.json", '{"n": 2}')
    time.sleep(QUIET)
    write(r.dir / "entrypoint.json", '{"n": 3}')
    time.sleep(QUIET)
    r.none()
    release.set()
    second = r.next()
    assert second.old is first.new and second.new.tree["n"] == 3
    r.none()
    assert not overlapped


def test_failed_open_then_created_include(run) -> None:
    r = run({"entrypoint.json": '{"n": 1}'})
    first = r.w.current
    write(r.dir / "entrypoint.json", '{"n": 1, "db": "@file:conf/db.json"}')
    c = r.next()
    assert c.error is not None and c.error.code == E_INCLUDE and c.new is None
    assert r.w.current is first
    write(r.dir / "conf" / "db.json", '{"port": 5432}')
    c = r.next()
    assert c.error is None and c.old is first and r.w.current is c.new
    assert c.new.tree["db"]["port"] == 5432


def test_second_entrypoint_and_env_file(run) -> None:
    r = run({"entrypoint.json": '{"host": "${EC_WT_HOST:-a}"}'})
    write(r.dir / "host.env", "EC_WT_HOST=b\n")
    c = r.next()
    assert c.new.tree["host"] == "b" and c.documents.added == ("host.env",)
    write(r.dir / "entrypoint.yaml", "host: c\n")
    assert r.next().error.code == E_MULTIPLE_ENTRYPOINTS


def test_close_waits_for_callback(tmp_path: Path) -> None:
    entered = threading.Event()
    finished = threading.Event()
    closed = threading.Event()
    late: list[Change] = []

    def callback(c: Change) -> None:
        if closed.is_set():
            late.append(c)
        if c.new is not None and c.new.tree["n"] == 1:
            entered.set()
            time.sleep(QUIET)
            finished.set()

    write(tmp_path / "entrypoint.json", '{"n": 0}')
    w = watch(tmp_path, callback, settle=SETTLE)
    write(tmp_path / "entrypoint.json", '{"n": 1}')
    assert entered.wait(5)
    write(tmp_path / "entrypoint.json", '{"n": 2}')
    w.close()
    closed.set()
    assert finished.is_set(), "close returned while a callback ran"
    write(tmp_path / "entrypoint.json", '{"n": 3}')
    time.sleep(QUIET)
    assert not late, "a callback started after close"
    w.close()  # a second close is a no-op


def test_watch_refused(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    saved = watch_module._schedule
    refused: list[str] = []

    def schedule(observer: Any, handler: Any, directory: str) -> Any:
        if directory in refused:
            raise OSError(28, "inotify watch limit reached")
        return saved(observer, handler, directory)

    monkeypatch.setattr(watch_module, "_schedule", schedule)

    first = tmp_path / "first"
    write(first / "entrypoint.json", '{"n": 1}')
    refused.append(str(first))
    with pytest.raises(EntryconfError) as info:
        watch(first, lambda c: None)
    assert info.value.code == E_WATCH

    second = tmp_path / "second"
    write(second / "entrypoint.json", '{"n": 1}')
    write(second / "sub" / "x.json", "{}")
    changes: queue.Queue[Change] = queue.Queue()
    w = watch(second, changes.put, settle=SETTLE)
    try:
        refused.append(str(second / "sub"))
        write(second / "entrypoint.json", '{"n": 1, "x": "@file:sub/x.json"}')
        accepted = changes.get(timeout=5)
        assert accepted.error is None
        c = changes.get(timeout=5)
        assert c.error is not None and c.error.code == E_WATCH
        assert c.old is accepted.new and w.current is accepted.new
    finally:
        w.close()
