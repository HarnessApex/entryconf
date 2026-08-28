"""Dump entrypoint: ``python -m entryconf <dir>``.

Prints the loaded tree as JSON on stdout and exits 0, or prints the ``E_*``
code on stderr and exits 1.
"""

from __future__ import annotations

import json
import sys
from typing import Sequence

from ._errors import EntryconfError
from ._loader import load

_USAGE = "usage: python -m entryconf <config-dir>"


def main(argv: Sequence[str] | None = None) -> int:
    args = list(sys.argv[1:] if argv is None else argv)
    if len(args) != 1:
        print(_USAGE, file=sys.stderr)
        return 2
    try:
        tree = load(args[0])
        # Serialize the WHOLE document first: writing incrementally would leak
        # a partial tree onto stdout if serialization failed part way through,
        # and SPEC §1 forbids partial results.
        # Keys are sorted so output is comparable across implementations.
        text = json.dumps(tree, sort_keys=True, ensure_ascii=False, allow_nan=False)
    except EntryconfError as exc:
        print(exc.code, file=sys.stderr)
        return 1
    except Exception as exc:  # noqa: BLE001 - never a traceback over partial output
        print(f"entryconf: internal error: {exc}", file=sys.stderr)
        return 1
    sys.stdout.write(text + "\n")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
