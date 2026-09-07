"""The directory lock and the atomic replacement write (SPEC §10.7)."""

from __future__ import annotations

import os
import secrets
import sys
import tempfile
import time
from datetime import datetime, timezone
from pathlib import Path

from ._errors import E_LOCKED, E_WRITE, EntryconfError

LOCK_FILE_NAME = ".entryconf.lock"

#: A lock older than this is presumed abandoned by a crashed writer.
LOCK_STALE_AFTER = 30.0

#: How long ``Plan.commit`` waits for the lock by default.
DEFAULT_LOCK_TIMEOUT = 5.0


def acquire_lock(directory: Path, timeout: float) -> Path:
    """Exclusive-create ``<dir>/.entryconf.lock``, retrying until ``timeout``;
    a lock older than :data:`LOCK_STALE_AFTER` is broken by renaming it to a
    unique name and deleting that (so at most one waiter breaks it)."""
    path = directory / LOCK_FILE_NAME
    deadline = time.monotonic() + timeout
    wait = 0.005
    while True:
        try:
            fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o644)
        except FileExistsError:
            pass
        except OSError as exc:
            raise EntryconfError(E_LOCKED, f"cannot create lock file {path}: {exc}") from exc
        else:
            with os.fdopen(fd, "w", encoding="utf-8") as f:
                stamp = datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")
                f.write(f'{{"pid": {os.getpid()}, "created": "{stamp}"}}\n')
            return path
        try:
            age = time.time() - path.stat().st_mtime
        except OSError:
            continue  # released between our attempts; try again at once
        if age > LOCK_STALE_AFTER:
            stale = Path(f"{path}.{secrets.token_hex(6)}")
            try:
                os.rename(path, stale)
            except OSError:
                pass
            else:
                try:
                    os.unlink(stale)
                except OSError:
                    pass
            continue
        if time.monotonic() > deadline:
            raise EntryconfError(
                E_LOCKED, f"lock file {path} is held by another writer (waited {timeout:g}s)"
            )
        time.sleep(wait)
        wait = min(wait * 2, 0.1)


def release_lock(path: Path) -> None:
    try:
        os.unlink(path)
    except OSError:
        pass


def atomic_write(target: Path, data: bytes) -> None:
    """Replace ``target``'s content: a temporary file in the target's directory,
    flushed, given the target's permission bits, renamed over it. A symlinked
    target is resolved first so the link survives. On any failure the temporary
    file is removed and the target is untouched."""
    resolved = Path(os.path.realpath(target))
    try:
        mode = resolved.stat().st_mode & 0o777
    except OSError as exc:
        raise EntryconfError(E_WRITE, f"cannot stat {resolved}: {exc}") from exc
    try:
        fd, tmp_name = tempfile.mkstemp(dir=str(resolved.parent), prefix=f".{resolved.name}.entryconf-tmp-")
    except OSError as exc:
        raise EntryconfError(E_WRITE, f"cannot create a temporary file beside {resolved}: {exc}") from exc
    tmp = Path(tmp_name)
    try:
        try:
            os.write(fd, data)
            os.fsync(fd)
            if sys.platform != "win32":
                os.fchmod(fd, mode)
        finally:
            os.close(fd)
        os.replace(tmp, resolved)
    except OSError as exc:
        try:
            os.unlink(tmp)
        except OSError:
            pass
        raise EntryconfError(E_WRITE, f"cannot replace {resolved}: {exc}") from exc
    # Best effort: make the rename itself durable where the platform allows.
    try:
        dfd = os.open(str(resolved.parent), os.O_RDONLY)
        try:
            os.fsync(dfd)
        finally:
            os.close(dfd)
    except OSError:
        pass
