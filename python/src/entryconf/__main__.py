"""Dump entrypoint: ``python -m entryconf <dir>``.

The dump-CLI convention, shared by every implementation:

* success — the loaded tree as JSON on stdout, exit 0;
* a **load** failure — the bare ``E_*`` code as the first stderr line, exit 1;
* any **other** fault (usage, internal) — exit 2, and no ``E_*`` code, so a
  caller can never mistake a broken invocation for a conformance verdict.

The single argument is a *directory*, never an option. A dash-led argument is
therefore a mis-invocation, not a config directory: it is answered from the
usage table above (``--help``/``--version`` print and exit 0; anything else
dash-led is a usage fault, exit 2) rather than handed to :func:`load`, which
would otherwise report a mistyped flag as ``E_NO_ENTRYPOINT`` — a broken run
wearing a conformance verdict's clothes.
"""

from __future__ import annotations

import json
import re
import sys
from typing import Sequence

from . import SPEC_VERSION, __version__
from ._errors import EntryconfError
from ._loader import load

_USAGE = (
    "usage: python -m entryconf <config-dir>\n"
    "       python -m entryconf --help | --version\n"
    "\n"
    "<config-dir> is a directory, never an option: a dash-led argument is a\n"
    "usage fault. To load a directory whose name starts with '-', prefix it\n"
    "with './' (e.g. './-weird-dir')."
)

_VERSION = f"entryconf (Python) {__version__} — implements entryconf spec {SPEC_VERSION}"

#: Anything that looks like a normative code, for scrubbing out of an internal
#: error's text: only a real load verdict may name an `E_*` code.
_CODE_RE = re.compile(r"\bE_[A-Z][A-Z_]*")


def main(argv: Sequence[str] | None = None) -> int:
    args = list(sys.argv[1:] if argv is None else argv)
    if len(args) != 1:
        print(_USAGE, file=sys.stderr)
        return 2
    target = args[0]
    if target in ("-h", "--help"):
        print(_USAGE)
        return 0
    if target == "--version":
        print(_VERSION)
        return 0
    if target.startswith("-"):
        # Includes the bare "--": this CLI has no options, so there is nothing
        # for an end-of-options marker to separate. Exit 2 with no E_* code.
        print(f"entryconf: not a config directory: {target}", file=sys.stderr)
        print(_USAGE, file=sys.stderr)
        return 2
    try:
        tree = load(target)
        # Serialize the WHOLE document first: writing incrementally would leak
        # a partial tree onto stdout if serialization failed part way through,
        # and SPEC §1 forbids partial results.
        # Keys are sorted so output is comparable across implementations.
        text = json.dumps(tree, sort_keys=True, ensure_ascii=False, allow_nan=False)
    except EntryconfError as exc:
        # A load failure: the bare code, first line, exit 1.
        print(exc.code, file=sys.stderr)
        return 1
    except Exception as exc:  # noqa: BLE001 - never a traceback over partial output
        # Not a load verdict: exit 2, and name no E_* code at all — a caller
        # must never be able to read a broken run as a conformance answer. Any
        # code-shaped text in the message is scrubbed so that holds whatever
        # the underlying exception happens to say.
        detail = _CODE_RE.sub("<code>", f"{type(exc).__name__}: {exc}")
        print(f"entryconf: internal error: {detail}", file=sys.stderr)
        return 2
    sys.stdout.write(text + "\n")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
