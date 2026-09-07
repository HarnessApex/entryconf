"""Command-line entrypoint.

    python -m entryconf <dir>                         dump the tree (0.2.0 form)
    python -m entryconf inspect <dir> [<pointer>]     provenance / document listing
    python -m entryconf edit [-n|--dry-run] <dir> <request.json | ->

The exit convention is shared by every implementation:

* success — JSON on stdout, exit 0;
* an ``EntryconfError`` (a load failure, or an editing failure such as
  ``E_STALE_PLAN``) — the bare ``E_*`` code as the first stderr line and
  nothing on stdout, exit 1;
* any **other** fault (usage, internal) — exit 2, and no ``E_*`` code, so a
  caller can never mistake a broken invocation for a conformance verdict.

With exactly one argument the tool is the 0.2.0 dump CLI, byte for byte. The
single argument is a *directory*, never an option: a dash-led argument is a
mis-invocation (``--help``/``--version`` print and exit 0; anything else
dash-led is a usage fault, exit 2), never handed to :func:`load`, which would
otherwise report a mistyped flag as ``E_NO_ENTRYPOINT``.
"""

from __future__ import annotations

import json
import re
import sys
from typing import Any, Sequence

from . import SPEC_VERSION, __version__
from ._edit import Edit, open as open_snapshot
from ._errors import EntryconfError
from ._loader import load

_USAGE = (
    "usage: python -m entryconf <config-dir>\n"
    "       python -m entryconf inspect <config-dir> [<pointer>]\n"
    "       python -m entryconf edit [-n|--dry-run] <config-dir> <request.json | ->\n"
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


def _emit(value: Any) -> None:
    # Serialize the WHOLE document first: writing incrementally would leak a
    # partial result onto stdout if serialization failed part way through.
    text = json.dumps(value, sort_keys=True, ensure_ascii=False, allow_nan=False)
    sys.stdout.write(text + "\n")


def _run(args: list[str]) -> int:
    if len(args) == 1:
        _emit(load(args[0]))
        return 0
    command = args[0]
    if command == "inspect" and len(args) in (2, 3):
        snap = open_snapshot(args[1])
        if len(args) == 3:
            _emit(snap.inspect(args[2]).to_json())
        else:
            _emit(snap.to_json())
        return 0
    if command == "edit":
        rest = args[1:]
        dry_run = False
        if rest and rest[0] in ("-n", "--dry-run"):
            dry_run = True
            rest = rest[1:]
        if len(rest) != 2 or rest[0].startswith("-"):
            print(_USAGE, file=sys.stderr)
            return 2
        directory, source = rest
        text = sys.stdin.read() if source == "-" else _read(source)
        if text is None:
            return 2
        try:
            request = json.loads(text)
            edits = [Edit.from_json(e) for e in request["edits"]]
        except (ValueError, KeyError, TypeError, AttributeError) as exc:
            print(f"entryconf: bad edit request: {exc}", file=sys.stderr)
            return 2
        plan = open_snapshot(directory).plan(edits)
        out = plan.to_json()
        out["committed"] = False
        out["revision"] = None
        if not dry_run:
            receipt = plan.commit()
            out["committed"] = True
            out["revision"] = receipt.revision
        _emit(out)
        return 0
    print(_USAGE, file=sys.stderr)
    return 2


def _read(path: str) -> str | None:
    try:
        with open(path, encoding="utf-8") as f:
            return f.read()
    except OSError as exc:
        print(f"entryconf: cannot read {path}: {exc}", file=sys.stderr)
        return None


def main(argv: Sequence[str] | None = None) -> int:
    args = list(sys.argv[1:] if argv is None else argv)
    if not args:
        print(_USAGE, file=sys.stderr)
        return 2
    if len(args) == 1:
        target = args[0]
        if target in ("-h", "--help"):
            print(_USAGE)
            return 0
        if target == "--version":
            print(_VERSION)
            return 0
        if target.startswith("-"):
            # Includes the bare "--": the dump form has no options. Exit 2 with no E_* code.
            print(f"entryconf: not a config directory: {target}", file=sys.stderr)
            print(_USAGE, file=sys.stderr)
            return 2
    try:
        return _run(args)
    except EntryconfError as exc:
        # A load or editing failure: the bare code, first line, exit 1.
        print(exc.code, file=sys.stderr)
        return 1
    except Exception as exc:  # noqa: BLE001 - never a traceback over partial output
        # Not a verdict: exit 2, and name no E_* code at all. Any code-shaped
        # text in the message is scrubbed so that holds whatever the
        # underlying exception happens to say.
        detail = _CODE_RE.sub("<code>", f"{type(exc).__name__}: {exc}")
        print(f"entryconf: internal error: {detail}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
