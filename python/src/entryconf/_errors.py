"""Error type for entryconf (SPEC §7).

Error *codes* are normative; messages are not.
"""

from __future__ import annotations

E_NO_ENTRYPOINT = "E_NO_ENTRYPOINT"
E_MULTIPLE_ENTRYPOINTS = "E_MULTIPLE_ENTRYPOINTS"
E_PARSE = "E_PARSE"
E_ENV_CONFLICT = "E_ENV_CONFLICT"
E_INCLUDE = "E_INCLUDE"
E_INCLUDE_CYCLE = "E_INCLUDE_CYCLE"
E_MISSING_VAR = "E_MISSING_VAR"
E_SUBSTITUTION = "E_SUBSTITUTION"


class EntryconfError(Exception):
    """A load-time failure. ``code`` is one of the ``E_*`` codes in SPEC §7."""

    def __init__(self, code: str, message: str) -> None:
        super().__init__(f"{code}: {message}")
        self._code = code
        self._message = message

    @property
    def code(self) -> str:
        """The normative ``E_*`` error code."""
        return self._code

    @property
    def message(self) -> str:
        """The human-readable detail (non-normative)."""
        return self._message
