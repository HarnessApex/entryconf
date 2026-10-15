# Watching configuration with entryconf

This is the API tour and integration handoff for watching, added in spec
0.4.0 (`SPEC.md` §12–§13). It is written for a long-running program (a
daemon, a server) that loads a config directory with entryconf and wants to
take a change without a restart: noticing it, knowing what changed, and
deciding whether to accept it.

`Load(dir)`, reading and editing are unchanged. A program that never watches
takes no new dependency: in Go, Python and Rust the file-event library sits in
a separate package or optional feature; TypeScript uses Node's own `fs.watch`.

The first user is emphar's host (HarnessApex/emphar,
`docs/design/settings-live.md`): settings applied without a restart, the
host's own writes applied at once and a hand edit through `Watch`.

## The model in one paragraph

A snapshot's **watch set** is the config directory (for an entrypoint or a
`*.env` file appearing, going or being renamed) and every document the open
read, by key. **`Refresh(snapshot)`** opens the directory again against the
process environment as it is now and reports whether it is **stale** (other
documents, a document whose bytes differ, or a variable whose value differs),
the new snapshot or the open's error, the **affected pointers** between the two
trees (as a plan's, SPEC §10.6), and the document keys **added, removed and
changed**. A reformatted file is stale with nothing changed in the tree.
**`Watch(dir, callback, options)`** holds a **current** snapshot and calls the
callback with each stale refresh: returning success accepts (the new snapshot
becomes current), returning an error refuses (the old one stays current, and
the next refresh is again of it). A failed open is delivered with its error
and keeps the old snapshot whatever the callback says. Callbacks never
overlap; events within `settle` (100 ms by default) are one change, so an
editor's write-to-temp-and-rename is one call; nothing stale, nothing called.

## Go

```go
import (
    entryconf "github.com/HarnessApex/entryconf/go"
    "github.com/HarnessApex/entryconf/go/watch" // fsnotify; only if you watch
)

func (s *Snapshot) WatchSet() WatchSet          // {Directory: ".", Documents: [keys…]}
func Refresh(s *Snapshot) *Refreshed

type Refreshed struct {
    Stale     bool
    Snapshot  *Snapshot      // the new one; the old one when not stale; nil on Err
    Err       error          // an *entryconf.Error; the old snapshot stays valid
    Changed   []string       // affected effective pointers
    Documents DocumentChanges // {Added, Removed, Changed []string}
    Tried     []string       // after a failed open: every path it asked about
}

func watch.Watch(dir string, cb watch.Callback, opts *watch.Options) (*watch.Watcher, error)
type watch.Callback func(watch.Change) error  // nil accepts, an error refuses
type watch.Change struct {
    Old, New  *entryconf.Snapshot             // New is nil when Err is set
    Changed   []string
    Documents entryconf.DocumentChanges
    Err       error                           // the open's code, or E_WATCH
}
func (w *Watcher) Current() *entryconf.Snapshot
func (w *Watcher) Close() error               // waits for a running callback
```

The callback runs on the watcher's goroutine; it must not call `Close` on its
own watcher (Close waits for it).

```go
w, err := watch.Watch(dir, func(c watch.Change) error {
    if c.Err != nil {
        log.Printf("config not applied: %v", c.Err) // the old snapshot stays current
        return nil
    }
    next, err := resolve(c.New.Tree) // your own validation
    if err != nil {
        return err // refused: c.Old stays current
    }
    apply(next, c.Changed)
    return nil
}, nil)
```

## Python

```python
import entryconf
snap = entryconf.open("envs/prod")
snap.watch_set()                    # {"directory": ".", "documents": [...]}
r = entryconf.refresh(snap)         # Refreshed(stale, snapshot, error, changed, documents, tried)

from entryconf.watch import watch   # pip install "entryconf[watch]" (watchdog)
w = watch("envs/prod", on_change, settle=0.1)
w.current                           # the current snapshot
w.close()                           # also a context manager
```

`on_change(change)` returning accepts; raising refuses. It runs on the
watcher's thread.

## TypeScript

```ts
import { open, refresh, watch } from "entryconf";
const snap = open("envs/prod");
snap.watchSet();                                // { directory: ".", documents: [...] }
const r = refresh(snap);                        // { stale, snapshot?, error?, changed, documents, tried }
const w = watch("envs/prod", async (change) => { /* throw or reject to refuse */ }, { settleMs: 100 });
w.current;
await w.close();                                // settles once a running callback has returned
```

`fs.watch` never polls; on a network filesystem (NFS, SMB) it does not see
changes made on another machine, so call `refresh` yourself there.

## Rust

```rust
let snap = entryconf::open(Path::new("envs/prod"))?;
snap.watch_set();                                // WatchSet { directory, documents }
let r = entryconf::refresh(&snap);               // Refreshed { stale, snapshot, error, changed, documents, tried }
// r.snapshot is Some only when stale and the open succeeded: keep `snap` otherwise.

// Cargo feature "watch" (notify):
let w = entryconf::watch::watch(dir, |c: &entryconf::watch::Change| {
    // Ok(()) accepts; Err(refusal) refuses
    Ok(())
}, entryconf::watch::Options::default())?;
w.current();                                     // Arc<Snapshot>
w.close();                                       // or drop it; waits for a running callback
```

## Command line

Every implementation's CLI has `refresh <dir> <change.json>`: open, apply a
SPEC §13 change, refresh, print the watch set and what the refresh found.
It is what `tools/crosscheck` compares; it is not a watcher.

## What the conformance suite pins, and what it does not

`testdata/watchcases/` (SPEC §13) pins the watch set and `Refresh`: an
unchanged directory, a file rewritten with its own bytes, a value changed, an
include grafted twice, a reformatted file, a `*.env` added or removed or
changed, a process variable changed, an include removed, a second entrypoint,
an include no longer referenced, an include outside the directory. `Watch`
itself depends on the platform's events, so each implementation tests §12.3's
rules against a real directory: a burst is one change; a temp-and-rename is
one change; nothing stale calls nothing; a refused change keeps the old
snapshot and the next is refreshed from it; a refused change's new include is
still watched; callbacks never overlap and a change during one comes after
it; a failed open keeps the old snapshot and creating the missing include is
seen; `Close` waits for a running callback and none starts after; `E_WATCH`
when the platform refuses a directory, at the start and later.

## How a watcher watches

Each implementation watches **directories**, not files: the directory of each
document (an editor's rename replaces the file, which would end a watch on
the file itself), the target's directory as well for a symbolic link, and, for
a path that does not exist yet, its nearest existing ancestor. Events are
filtered by entry name, and a refresh decides; the lock file and an editor's
temporary files trigger nothing.
