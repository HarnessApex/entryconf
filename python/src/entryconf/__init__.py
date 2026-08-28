"""entryconf — load a config directory into a single tree.

Implements the entryconf specification, version 0.2.0.

    >>> import entryconf
    >>> cfg = entryconf.load("envs/deploy")     # doctest: +SKIP
"""

from __future__ import annotations

from ._errors import EntryconfError
from ._loader import load

__all__ = ["load", "EntryconfError"]
__version__ = "0.2.0"
SPEC_VERSION = "0.2.0"
