"""The watching conformance harness (SPEC §13): open a copy, take its watch
set, apply ``change.json``, refresh, and compare with ``expected.json``."""

from __future__ import annotations

import json
import shutil
from pathlib import Path

import pytest

from entryconf._edit import _open_with_env
from entryconf._refresh import _refresh

from test_conformance import _mismatch

CASES_DIR = Path(__file__).resolve().parents[2] / "testdata" / "watchcases"
CASES = sorted(path for path in CASES_DIR.iterdir() if path.is_dir())


@pytest.mark.parametrize("case", CASES, ids=[case.name for case in CASES])
def test_watch_conformance(case: Path, tmp_path: Path) -> None:
    config = tmp_path / "config"
    shutil.copytree(case / "config", config)
    if (case / "outside").is_dir():
        shutil.copytree(case / "outside", tmp_path, dirs_exist_ok=True)

    procenv_file = case / "procenv.json"
    env: dict[str, str] = (
        json.loads(procenv_file.read_text(encoding="utf-8")) if procenv_file.is_file() else {}
    )
    snap = _open_with_env(config, dict(env), env.get)
    watch = snap.watch_set()

    change = json.loads((case / "change.json").read_text(encoding="utf-8"))
    for key, text in change.get("files", {}).items():
        path = config / key
        if text is None:
            path.unlink()
        else:
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(text.encode("utf-8"))
    for name, value in change.get("procenv", {}).items():
        if value is None:
            env.pop(name, None)
        else:
            env[name] = value

    r = _refresh(snap, dict(env), env.get)
    if r.error is None:
        assert r.snapshot is not None
        if not r.stale:
            assert r.snapshot is snap, "a refresh that is not stale returns the old snapshot"
    else:
        assert r.snapshot is None, "a failed refresh carries no snapshot"
    got = {"watch": watch, **r.to_json()}
    expected = json.loads((case / "expected.json").read_text(encoding="utf-8"))
    found = _mismatch(got, expected)
    assert found is None, found
