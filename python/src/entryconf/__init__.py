"""entryconf — load a config directory into a single tree, and edit its
JSON source documents.

Implements the entryconf specification, version 0.3.0.

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
    E_WRITE,
    EntryconfError,
)
from ._loader import load

__all__ = [
    "load",
    "open",
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
]
__version__ = "0.3.0rc1"
SPEC_VERSION = "0.3.0"
