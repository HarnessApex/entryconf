"""entryconf — load a config directory into a single tree, and edit its
JSON source documents.

Implements the entryconf specification, version 0.4.0.

    >>> import entryconf
    >>> cfg = entryconf.load("envs/deploy")     # doctest: +SKIP
    >>> snap = entryconf.open("envs/deploy")    # doctest: +SKIP
"""

from __future__ import annotations

from ._edit import (
    Document,
    Edit,
    Graft,
    Origin,
    Plan,
    Receipt,
    Reference,
    Snapshot,
    Variable,
    open,
)
from ._refresh import DocumentChanges, Refreshed, refresh
from ._errors import (
    E_EDIT,
    E_ENV_CONFLICT,
    E_INCLUDE,
    E_INCLUDE_CYCLE,
    E_LOCKED,
    E_MISSING_VAR,
    E_MULTIPLE_ENTRYPOINTS,
    E_NO_ENTRYPOINT,
    E_PARSE,
    E_PATH,
    E_STALE_PLAN,
    E_SUBSTITUTION,
    E_UNSUPPORTED_EDIT,
    E_WATCH,
    E_WRITE,
    EntryconfError,
)
from ._loader import load

__all__ = [
    "load",
    "open",
    "refresh",
    "Refreshed",
    "DocumentChanges",
    "EntryconfError",
    "Snapshot",
    "Document",
    "Graft",
    "Reference",
    "Origin",
    "Variable",
    "Edit",
    "Plan",
    "Receipt",
    "E_NO_ENTRYPOINT",
    "E_MULTIPLE_ENTRYPOINTS",
    "E_PARSE",
    "E_ENV_CONFLICT",
    "E_INCLUDE",
    "E_INCLUDE_CYCLE",
    "E_MISSING_VAR",
    "E_SUBSTITUTION",
    "E_UNSUPPORTED_EDIT",
    "E_EDIT",
    "E_PATH",
    "E_STALE_PLAN",
    "E_LOCKED",
    "E_WRITE",
    "E_WATCH",
]
__version__ = "0.4.0rc1"
SPEC_VERSION = "0.4.0"
