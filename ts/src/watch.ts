/**
 * `watch` (SPEC §12.3): hold a current snapshot of a config directory and hand
 * each change on disk to a callback that accepts or refuses it.
 *
 * Built on Node's `fs.watch` (inotify, FSEvents, kqueue,
 * ReadDirectoryChangesW), with no dependency. It never polls: `fs.watch` does
 * not, and `fs.watchFile` (which does) is not used. Like every platform watch
 * it does not see changes made on another machine to a network filesystem
 * (NFS, SMB); on such a directory call `refresh` yourself.
 */
import { watch as fsWatch, statSync, lstatSync, realpathSync, type FSWatcher } from "node:fs";
import { basename, dirname, normalize, resolve } from "node:path";

import { open, type Snapshot } from "./edit.ts";
import { EntryconfError } from "./errors.ts";
import { refresh, type DocumentChanges } from "./refresh.ts";

/** Milliseconds a watcher waits after the last event of a burst before it refreshes. */
export const DEFAULT_SETTLE_MS = 100;

const ENTRYPOINTS = new Set(["entrypoint.json", "entrypoint.yaml", "entrypoint.yml", "entrypoint.toml"]);

/** What a callback is handed (SPEC §12.3). */
export interface Change {
  /** The current snapshot. */
  old: Snapshot;
  /** The refreshed snapshot; absent when `error` is set. */
  new?: Snapshot;
  changed: string[];
  documents: DocumentChanges;
  /**
   * The open's failure, or `E_WATCH` when the platform refused to watch a
   * path; `old` stays current whatever the callback does.
   */
  error?: EntryconfError;
}

/**
 * Returning (or resolving) accepts a change: `new` becomes current. Throwing
 * (or rejecting) refuses it: `old` stays current, and the next change is
 * refreshed from `old`. Callbacks never overlap; `close()` may be called from
 * one, and its promise settles once the callback has returned.
 */
export type Callback = (change: Change) => void | Promise<void>;

export interface WatchOptions {
  /** Milliseconds after the last event of a burst; default `DEFAULT_SETTLE_MS`. */
  settleMs?: number;
}

/** @internal — the seam the tests use to make the platform refuse a directory. */
export const platform = {
  watch(dir: string, listener: (event: string, name: string | Buffer | null) => void): FSWatcher {
    return fsWatch(dir, listener);
  },
};

/** Watches one config directory. Make one with `watch`. */
export class Watcher {
  readonly #callback: Callback;
  readonly #settle: number;
  readonly #config: string;
  readonly #configReal: string;
  #current: Snapshot;
  #watches = new Map<string, FSWatcher>();
  #names = new Map<string, Set<string>>();
  #timer: NodeJS.Timeout | undefined;
  #running: Promise<void> | undefined;
  #pending = false;
  #closed = false;
  #failures: EntryconfError[] = [];

  /** @internal — use `watch()`. */
  constructor(snapshot: Snapshot, callback: Callback, settle: number) {
    this.#callback = callback;
    this.#settle = settle;
    this.#current = snapshot;
    this.#config = snapshot.dir;
    this.#configReal = safeRealpath(snapshot.dir);
    const failed = this.#arm(targets(this.#config, documentPaths(snapshot))).failed;
    if (failed) {
      this.#unwatchAll();
      throw failed;
    }
    // A change made between open and the watches being placed is caught by
    // one refresh at the start; it calls nothing if nothing changed.
    this.#schedule();
  }

  /** The snapshot `watch` opened, or the last one a callback accepted. */
  get current(): Snapshot {
    return this.#current;
  }

  /**
   * Stop the watch. The promise settles once a callback running has returned;
   * none starts after `close` is called.
   */
  async close(): Promise<void> {
    this.#closed = true;
    clearTimeout(this.#timer);
    this.#unwatchAll();
    if (this.#running) await this.#running.catch(() => {});
  }

  #schedule(): void {
    if (this.#closed) return;
    if (this.#running) {
      // Gathered: delivered as one change after the running callback returns.
      this.#pending = true;
      return;
    }
    clearTimeout(this.#timer);
    this.#timer = setTimeout(() => this.#tick(), this.#settle);
  }

  #tick(): void {
    if (this.#closed) return;
    this.#running = this.#refresh().finally(() => {
      this.#running = undefined;
      if (this.#pending) {
        this.#pending = false;
        this.#schedule();
      }
    });
  }

  async #refresh(): Promise<void> {
    for (let error = this.#failures.shift(); error; error = this.#failures.shift()) {
      await this.#deliver({ old: this.#current, changed: [], documents: emptyDocuments(), error });
    }
    const old = this.#current;
    const r = refresh(old);
    if (!r.stale) return;
    const change: Change = { old, changed: r.changed, documents: r.documents };
    if (r.error) change.error = r.error;
    else change.new = r.snapshot;
    const accepted = await this.#deliver(change);
    let paths: string[];
    if (r.error) paths = [...documentPaths(old), ...r.tried];
    else if (accepted) {
      this.#current = r.snapshot!;
      paths = documentPaths(r.snapshot!);
    } else paths = [...documentPaths(old), ...documentPaths(r.snapshot!)];
    if (this.#closed) return;
    const { added, failed } = this.#arm(targets(this.#config, paths));
    if (failed) {
      await this.#deliver({ old: this.#current, changed: [], documents: emptyDocuments(), error: failed });
    } else if (added) {
      this.#pending = true; // a change may have landed before the new watch
    }
  }

  async #deliver(change: Change): Promise<boolean> {
    if (this.#closed) return false;
    try {
      await this.#callback(change);
    } catch {
      return false; // throwing is how a callback refuses
    }
    return change.error === undefined;
  }

  #onEvent(dir: string, name: string | Buffer | null): void {
    // Some platforms give no name: anything may have changed.
    if (name === null || this.#relevant(dir, name.toString())) this.#schedule();
  }

  #relevant(dir: string, name: string): boolean {
    if (this.#names.get(dir)?.has(name)) return true;
    const inConfig = dir === this.#config || dir === this.#configReal;
    return inConfig && (ENTRYPOINTS.has(name) || name.endsWith(".env"));
  }

  #onError(dir: string, err: Error): void {
    this.#watches.get(dir)?.close();
    this.#watches.delete(dir);
    if (this.#closed) return;
    // Delivered in turn, by the next tick, like any change.
    this.#failures.push(new EntryconfError("E_WATCH", `watching ${dir}: ${err.message}`));
    this.#schedule();
  }

  /** Watch exactly the directories in `want`; E_WATCH if the platform refused one. */
  #arm(want: Map<string, Set<string>>): { added: boolean; failed?: EntryconfError } {
    let added = false;
    let failed: EntryconfError | undefined;
    for (const dir of [...want.keys()]) {
      if (this.#watches.has(dir)) continue;
      try {
        const w = platform.watch(dir, (_event, name) => this.#onEvent(dir, name));
        w.on("error", (err) => this.#onError(dir, err));
        this.#watches.set(dir, w);
        added = true;
      } catch (err) {
        failed ??= new EntryconfError("E_WATCH", `cannot watch ${dir}: ${(err as Error).message}`);
        want.delete(dir);
      }
    }
    for (const [dir, w] of this.#watches) {
      if (!want.has(dir)) {
        w.close();
        this.#watches.delete(dir);
      }
    }
    this.#names = want;
    return { added, failed };
  }

  #unwatchAll(): void {
    for (const w of this.#watches.values()) w.close();
    this.#watches.clear();
  }
}

/** The snapshot's watch set (SPEC §12.1) as the paths the load resolved. */
function documentPaths(s: Snapshot): string[] {
  return s.watchSet().documents.map((key) => s.documents[key].path);
}

/**
 * Directories to watch and the entry names in each that matter. A path's own
 * directory is watched (an editor's rename replaces the file); a symbolic link
 * is also watched where it points; a path that does not exist (what a failed
 * open could not read) is watched at its nearest existing ancestor, by the
 * name of the next step down, so its creation is seen.
 */
function targets(config: string, paths: string[]): Map<string, Set<string>> {
  const out = new Map<string, Set<string>>([[config, new Set()]]);
  const add = (dir: string, name: string): void => {
    if (!out.has(dir)) out.set(dir, new Set());
    out.get(dir)!.add(name);
  };
  for (const raw of paths) {
    const p = normalize(raw);
    if (p === config) continue;
    for (let q = p; ; ) {
      const dir = dirname(q);
      if (isDir(dir)) {
        add(dir, basename(q));
        break;
      }
      if (dir === q) break;
      q = dir;
    }
    if (isSymlink(p)) {
      const real = safeRealpath(p);
      add(dirname(real), basename(real));
    }
  }
  return out;
}

function isDir(p: string): boolean {
  try {
    return statSync(p).isDirectory();
  } catch {
    return false;
  }
}

function isSymlink(p: string): boolean {
  try {
    return lstatSync(p).isSymbolicLink();
  } catch {
    return false;
  }
}

function safeRealpath(p: string): string {
  try {
    return realpathSync(p);
  } catch {
    return p;
  }
}

function emptyDocuments(): DocumentChanges {
  return { added: [], removed: [], changed: [] };
}

/**
 * Open `dir` (`open`) and watch it (SPEC §12.3). A failed open throws its
 * code; a watch the platform refuses throws `E_WATCH`. `callback` is called
 * with each stale refresh until `close()`.
 */
export function watch(dir: string, callback: Callback, options: WatchOptions = {}): Watcher {
  const snapshot = open(resolve(dir));
  const settle = options.settleMs && options.settleMs > 0 ? options.settleMs : DEFAULT_SETTLE_MS;
  return new Watcher(snapshot, callback, settle);
}
