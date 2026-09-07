"""Where the loader gets its bytes, and what one load touched (SPEC §10.2).

``load`` reads the real filesystem. ``open`` reads it while *recording* every
document, graft and variable lookup; a plan's candidate evaluation (SPEC §10.6)
then reads the snapshot's recorded bytes with the edited document replaced in
memory, falling back to disk only for a file the snapshot never saw (a newly
written ``@file:`` reference).
"""

from __future__ import annotations

import os
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any


class OsSource:
    """The real filesystem."""

    def read_file(self, path: Path) -> bytes:
        return path.read_bytes()

    def is_file(self, path: Path) -> bool:
        return path.is_file()

    def env_file_names(self, directory: Path) -> list[str]:
        return sorted(
            entry.name
            for entry in directory.iterdir()
            if entry.name.endswith(".env") and entry.is_file()
        )


class OverlaySource:
    """Recorded bytes first, disk second; the ``*.env`` listing the snapshot saw."""

    def __init__(self, files: dict[str, bytes], env_names: list[str], fallback: OsSource) -> None:
        self.files = files
        self.env_names = env_names
        self.fallback = fallback

    def read_file(self, path: Path) -> bytes:
        data = self.files.get(str(path))
        if data is not None:
            return data
        return self.fallback.read_file(path)

    def is_file(self, path: Path) -> bool:
        return str(path) in self.files or self.fallback.is_file(path)

    def env_file_names(self, directory: Path) -> list[str]:
        return list(self.env_names)


@dataclass
class DocRecord:
    path: Path
    data: bytes = b""
    format: str = ""
    parsed: Any = None
    grafts: list[dict[str, Any]] = field(default_factory=list)


class Recorder:
    """Captures each document's bytes, format and raw parsed value, plus every
    graft of its root into the effective tree (SPEC §10.2)."""

    def __init__(self) -> None:
        self.docs: dict[str, DocRecord] = {}
        self.order: list[str] = []

    def record(self, path: Path, data: bytes, fmt: str, parsed: Any) -> None:
        key = str(path)
        existing = self.docs.get(key)
        if existing is not None and existing.format:
            return  # a shared include is parsed once per reference; recorded once
        if existing is None:
            existing = DocRecord(path=path)
            self.docs[key] = existing
            self.order.append(key)
        existing.data, existing.format, existing.parsed = data, fmt, parsed

    def graft(self, path: Path, effective: str, chain: list[dict[str, str]]) -> None:
        key = str(path)
        if key not in self.docs:
            self.docs[key] = DocRecord(path=path)
            self.order.append(key)
        self.docs[key].grafts.append({"effective": effective, "chain": chain})


def document_key(directory: Path, path: Path) -> str:
    """SPEC §10.2's key: the path relative to the config directory, slash
    separated, or the absolute path when no relative form exists."""
    try:
        rel = os.path.relpath(str(path), str(directory))
    except ValueError:
        return str(path).replace(os.sep, "/")
    return rel.replace(os.sep, "/")


def clean_join(base: Path, target: str) -> Path:
    """Resolve an include target against the referencing file's directory and
    lexically normalize it (no symlink resolution), like Go's filepath.Clean."""
    joined = target if os.path.isabs(target) else os.path.join(str(base), target)
    return Path(os.path.normpath(joined))


def format_of(path: Path) -> str:
    return {".json": "json", ".yaml": "yaml", ".yml": "yaml", ".toml": "toml", ".env": "env"}.get(
        path.suffix, ""
    )
