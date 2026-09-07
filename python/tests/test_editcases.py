"""The editing conformance harness (SPEC §11) and the unit tests for what a
fixture cannot express (SPEC §10.9): lock contention, filesystem failure,
permissions, symlinks, and the serializer's byte rules.

One parametrized test walks ``../../testdata/editcases``; every case runs
against a fresh copy of its ``config/`` directory, and afterwards every file the
case did not expect to be written must be byte-identical to the copy with no
file added.
"""

from __future__ import annotations

import json
import os
import shutil
import stat
import sys
import time
from pathlib import Path
from typing import Any

import pytest

import entryconf
from entryconf import Edit, EntryconfError
from entryconf._edit import _open_with_env
from entryconf._jsonfmt import format_json
from entryconf._loader import load_with_env
from entryconf._lock import LOCK_FILE_NAME, LOCK_STALE_AFTER

from test_conformance import _mismatch

CASES_DIR = Path(__file__).resolve().parents[2] / "testdata" / "editcases"
CASES = sorted(path for path in CASES_DIR.iterdir() if path.is_dir())


def _files(root: Path) -> dict[str, bytes]:
    return {
        p.relative_to(root).as_posix(): p.read_bytes()
        for p in sorted(root.rglob("*"))
        if p.is_file()
    }


@pytest.mark.parametrize("case", CASES, ids=[case.name for case in CASES])
def test_edit_conformance(case: Path, tmp_path: Path) -> None:
    work = tmp_path / "config"
    shutil.copytree(case / "config", work)
    original = _files(work)

    procenv_file = case / "procenv.json"
    env: dict[str, str] = (
        json.loads(procenv_file.read_text(encoding="utf-8")) if procenv_file.is_file() else {}
    )
    captured = dict(env)
    live = env.get

    request = json.loads((case / "request.json").read_text(encoding="utf-8"))
    error_file = case / "expected_error.txt"
    want_error = error_file.read_text(encoding="utf-8").strip() if error_file.is_file() else None
    expected = (
        json.loads((case / "expected.json").read_text(encoding="utf-8"))
        if (case / "expected.json").is_file()
        else None
    )
    expect_written: set[str] = set()

    got: Any = None
    error: EntryconfError | None = None
    try:
        snap = _open_with_env(work, captured, live)
        if "inspect" in request:
            got = {"origin": snap.inspect(request["inspect"]).to_json()}
        elif "documents" in request:
            got = {
                "documents": {
                    key: {"format": d.format, "writable": d.writable, "grafts": [g.to_json() for g in d.grafts]}
                    for key, d in snap.documents.items()
                }
            }
        else:
            plan = snap.plan([Edit.from_json(e) for e in request["edits"]])
            before_commit = request.get("before_commit", {})
            for key, text in before_commit.get("files", {}).items():
                (work / key).write_text(text, encoding="utf-8")
                original[key] = text.encode("utf-8")
            env.update(before_commit.get("procenv", {}))
            receipt = plan.commit()
            expect_written.add(plan.document)
            assert receipt.document == plan.document
            reloaded = load_with_env(work, env)
            assert _mismatch(reloaded, plan.candidate) is None, "committed tree differs from the candidate"
            on_disk = (work / plan.document).read_bytes()
            assert on_disk == plan.after.encode("utf-8")
            assert receipt.revision == "sha256:" + __import__("hashlib").sha256(on_disk).hexdigest()
            got = {
                "tree": reloaded,
                "affected": list(plan.affected),
                "documents": {plan.document: json.loads(plan.after)},
            }
    except EntryconfError as exc:
        error = exc

    if want_error is not None:
        assert error is not None, f"expected {want_error}, got {got!r}"
        assert error.code == want_error, str(error)
    else:
        assert error is None, str(error)
        found = _mismatch(got, expected)
        assert found is None, found

    after = _files(work)
    for rel, data in after.items():
        assert rel in original, f"{rel} was created (lock or temporary file left behind?)"
        if rel not in expect_written:
            assert data == original[rel], f"{rel} changed but was not the edited document"
    assert set(original) <= set(after), "a file disappeared"


# --------------------------------------------------------------------------
# What a fixture cannot express (SPEC §10.9).
# --------------------------------------------------------------------------


def _scratch(tmp_path: Path, entrypoint: str) -> Path:
    d = tmp_path / "cfg"
    d.mkdir()
    (d / "entrypoint.json").write_text(entrypoint, encoding="utf-8")
    return d


def _edit(pointer: str, value: Any, document: str = "entrypoint.json") -> Edit:
    return Edit(document=document, op="set", pointer=pointer, value=value)


def test_two_cooperating_writers(tmp_path: Path) -> None:
    d = _scratch(tmp_path, '{"a": 1, "b": 2}')
    snap = entryconf.open(d)
    p1 = snap.plan([_edit("/a", 10)])
    p2 = snap.plan([_edit("/b", 20)])
    p1.commit()
    with pytest.raises(EntryconfError) as exc:
        p2.commit()
    assert exc.value.code == "E_STALE_PLAN"
    # Committing the same plan twice is also stale: its own write moved the revision.
    with pytest.raises(EntryconfError) as exc:
        p1.commit()
    assert exc.value.code == "E_STALE_PLAN"
    assert entryconf.load(d) == {"a": 10, "b": 2}


def test_lock_contention_and_stale_lock(tmp_path: Path) -> None:
    d = _scratch(tmp_path, '{"a": 1}')
    plan = entryconf.open(d).plan([_edit("/a", 2)])
    lock = d / LOCK_FILE_NAME
    lock.write_text("{}")
    started = time.monotonic()
    with pytest.raises(EntryconfError) as exc:
        plan.commit(lock_timeout=0.15)
    assert exc.value.code == "E_LOCKED"
    assert time.monotonic() - started < 2
    assert (d / "entrypoint.json").read_text() == '{"a": 1}'
    assert lock.exists(), "another writer's lock was removed"
    old = time.time() - 2 * LOCK_STALE_AFTER
    os.utime(lock, (old, old))
    plan.commit(lock_timeout=1.0)
    assert not lock.exists()
    assert entryconf.load(d) == {"a": 2}


@pytest.mark.skipif(sys.platform == "win32", reason="no permission bits on windows")
def test_commit_preserves_permissions(tmp_path: Path) -> None:
    d = _scratch(tmp_path, '{"a": 1}')
    target = d / "entrypoint.json"
    target.chmod(0o600)
    entryconf.open(d).plan([_edit("/a", 2)]).commit()
    assert stat.S_IMODE(target.stat().st_mode) == 0o600


@pytest.mark.skipif(
    sys.platform == "win32" or (hasattr(os, "getuid") and os.getuid() == 0),
    reason="needs POSIX permissions and a non-root user",
)
def test_write_failure_leaves_source_and_no_temp(tmp_path: Path) -> None:
    d = _scratch(tmp_path, '{"x": "@file:sub/x.json"}')
    sub = d / "sub"
    sub.mkdir()
    (sub / "x.json").write_text('{"v": 1}')
    plan = entryconf.open(d).plan([_edit("/v", 2, "sub/x.json")])
    sub.chmod(0o555)
    try:
        with pytest.raises(EntryconfError) as exc:
            plan.commit()
        assert exc.value.code == "E_WRITE"
        assert [p.name for p in sub.iterdir()] == ["x.json"], "temporary file left behind"
        assert (sub / "x.json").read_text() == '{"v": 1}'
        assert not (d / LOCK_FILE_NAME).exists()
    finally:
        sub.chmod(0o755)


@pytest.mark.skipif(sys.platform == "win32", reason="symlinks need privilege on windows")
def test_commit_through_symlink(tmp_path: Path) -> None:
    d = _scratch(tmp_path, '{"x": "@file:link.json"}')
    real = tmp_path / "real.json"
    real.write_text('{"v": 1}')
    os.symlink(real, d / "link.json")
    entryconf.open(d).plan([_edit("/v", 2, "link.json")]).commit()
    assert (d / "link.json").is_symlink()
    assert json.loads(real.read_text()) == {"v": 2}


def test_plan_does_not_touch_disk(tmp_path: Path) -> None:
    d = _scratch(tmp_path, '{"a": 1}')
    before = _files(d)
    entryconf.open(d).plan([_edit("/a", 2)])
    assert _files(d) == before


def test_open_uses_process_environment(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    d = _scratch(tmp_path, '{"host": "${EC_EDIT_HOST}"}')
    monkeypatch.setenv("EC_EDIT_HOST", "prod")
    snap = entryconf.open(d)
    assert snap.tree["host"] == "prod"
    origin = snap.inspect("/host")
    assert [v.to_json() for v in origin.variables] == [{"name": "EC_EDIT_HOST", "origin": "process"}]
    plan = snap.plan([_edit("/x", 1)])
    monkeypatch.setenv("EC_EDIT_HOST", "other")
    with pytest.raises(EntryconfError) as exc:
        plan.commit()
    assert exc.value.code == "E_STALE_PLAN"


def test_format_json_normalization() -> None:
    value = {
        "b": [1, 2.0, 1.5, "x"],
        "a": {},
        "c": [],
        "s": 'q"\\\n\t\x01é',
        "big": float(2**53 - 1),
    }
    want = (
        "{\n"
        '  "a": {},\n'
        '  "b": [\n    1,\n    2,\n    1.5,\n    "x"\n  ],\n'
        '  "big": 9007199254740991,\n'
        '  "c": [],\n'
        '  "s": "q\\"\\\\\\n\\t\\u0001é"\n'
        "}\n"
    )
    assert format_json(value) == want
    assert sorted(["é", "z", "Z", "a"]) == ["Z", "a", "z", "é"]
