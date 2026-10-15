# Watching: build hand-off

SPEC §12–13 (draft for 0.4.0) define watching: a snapshot's watch set,
`Refresh`, and `Watch` with an accept-or-refuse callback. This file is
what the pull request that carries them still owes, in the order
CONTRIBUTING.md asks: the spec, fixtures pinning it, all four
implementations, in one PR.

The first user is emphar's host (HarnessApex/emphar,
docs/design/settings-live.md): settings applied without a restart, the
host's own writes applied at once and a hand edit through `Watch`.

## Owed

1. **Fixtures**, `testdata/watchcases/` (SPEC §13), at least:
   - `01-unchanged`: `change.json` `{}`; not stale.
   - `02-rewritten-same-bytes`: a document written with its own text; not
     stale.
   - `03-value-changed`: one value in the entrypoint; stale, `changed` that
     pointer, `documents.changed` the entrypoint.
   - `04-include-changed-twice-grafted`: an include grafted at two places;
     `changed` both pointers.
   - `05-same-values-new-bytes`: a document reformatted; stale, `changed`
     empty, `documents.changed` it.
   - `06-env-file-added`: a new `*.env` nothing reads; stale,
     `documents.added`, `changed` empty.
   - `07-env-value-changed`: a variable in a `*.env` used by the tree;
     `changed` where it lands.
   - `08-procenv-changed`: `change.json` `procenv`; stale by the variables
     fingerprint.
   - `09-include-removed`: stale, `error` `E_INCLUDE`, no `tree`.
   - `10-second-entrypoint`: stale, `error` `E_MULTIPLE_ENTRYPOINTS`.
   - `11-include-now-unreferenced`: the reference removed; `documents.removed`
     the include.
   - `12-watch-set`: an include outside the directory (`../shared.json`);
     the watch set names it by its key.
2. **`testdata/errors.json`**: `E_WATCH`, `"suite": "unit"`.
3. **`tools/lintcases`**: validate `watchcases/` (its shape, the required
   files, keys inside the case).
4. **`tools/crosscheck`**: compare `Refresh` across the four.
5. **Implementations**, each with `Refresh`, the watch set, and `Watch`:
   - Go: `Refresh` and the watch set in the module; `Watch` in a
     subpackage `go/watch` on `github.com/fsnotify/fsnotify`, so a program
     that only loads takes no new dependency.
   - Python: `refresh` and `watch_set` in the package; `watch` behind an
     extra (`entryconf[watch]`, on `watchdog`).
   - TypeScript: Node's `fs.watch`, no dependency; document where it polls
     (some network filesystems).
   - Rust: `watch` behind a cargo feature `watch`, on `notify`.
6. **Unit tests per implementation for §12.3's rules**, against a real
   temporary directory: a burst within `settle` is one change; a
   write-to-temp-and-rename is one change; a refused change keeps the old
   snapshot current and the next change is refreshed from it; callbacks
   never overlap and a change during one is delivered after it; a failed
   open keeps the old snapshot and creating the missing include is seen;
   `Close` waits for a running callback and none starts after; `E_WATCH`.
7. **Versions**: SPEC header 0.4.0; `CHANGELOG.md` `[0.4.0-rc.1]`; tags
   `spec/`, `go/`, `python/` as for 0.3.0-rc.1, npm and crate versions.

## Acceptance

- Every implementation passes `testdata/cases/`, `editcases/` and
  `watchcases/`; `tools/crosscheck` finds no difference.
- `Load(dir)` and every read and edit fixture are unchanged.
- A program that never watches takes no new dependency (Go, Python, Rust).
