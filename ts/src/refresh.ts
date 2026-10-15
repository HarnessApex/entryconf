/**
 * Refreshing a snapshot (SPEC §12.2): open again and say what changed.
 */
import { affectedPointers, openTracked, type Snapshot } from "./edit.ts";
import { type EnvLookup } from "./env.ts";
import { EntryconfError } from "./errors.ts";
import { compareCodePoints } from "./jsonfmt.ts";
import { processEnvLookup } from "./loader.ts";
import { osSource, TrackingSource } from "./source.ts";

/** Document keys between two snapshots, each sorted by code point. */
export interface DocumentChanges {
  added: string[];
  removed: string[];
  changed: string[];
}

/** What `refresh` reports (SPEC §12.2). */
export interface Refreshed {
  /**
   * True iff the open failed, read other documents, read a document whose
   * revision differs, or looked up a variable whose value differs.
   */
  stale: boolean;
  /** The new snapshot (the old one when not stale); absent when the open failed. */
  snapshot?: Snapshot;
  /** The open's failure; the old snapshot stays valid. */
  error?: EntryconfError;
  /** The affected pointers between the old and the new tree. */
  changed: string[];
  documents: DocumentChanges;
  /**
   * After a failed open, every path it asked about, including those it could
   * not read: a watcher watches them so creating a missing include is seen.
   */
  tried: string[];
}

/**
 * Open the snapshot's directory again, against the process environment as it
 * is now, and report whether and what changed (SPEC §12.2). Never writes a
 * file.
 */
export function refresh(snapshot: Snapshot): Refreshed {
  const captured: Record<string, string> = {};
  for (const [k, v] of Object.entries(process.env)) if (v !== undefined) captured[k] = v;
  return refreshWith(snapshot, captured, processEnvLookup);
}

/** `refresh` with the environments supplied (test seam). */
export function refreshWith(snapshot: Snapshot, captured: Record<string, string>, live: EnvLookup): Refreshed {
  const empty = (): DocumentChanges => ({ added: [], removed: [], changed: [] });
  const src = new TrackingSource(osSource);
  let next: Snapshot;
  try {
    next = openTracked(snapshot.dir, captured, live, src);
  } catch (err) {
    if (!(err instanceof EntryconfError)) throw err;
    return { stale: true, error: err, changed: [], documents: empty(), tried: src.paths() };
  }
  const docs = empty();
  for (const [key, d] of Object.entries(next.documents)) {
    const old = snapshot.documents[key];
    if (!old) docs.added.push(key);
    else if (old.revision !== d.revision) docs.changed.push(key);
  }
  for (const key of Object.keys(snapshot.documents)) {
    if (!(key in next.documents)) docs.removed.push(key);
  }
  docs.added.sort(compareCodePoints);
  docs.removed.sort(compareCodePoints);
  docs.changed.sort(compareCodePoints);
  const stale =
    docs.added.length + docs.removed.length + docs.changed.length > 0 ||
    next.variablesRevision !== snapshot.variablesRevision;
  if (!stale) return { stale, snapshot, changed: [], documents: docs, tried: [] };
  return { stale, snapshot: next, changed: affectedPointers(snapshot.tree, next.tree), documents: docs, tried: [] };
}

/** A refresh as SPEC §13's expected fields: the tree, or the error's code. */
export function refreshedJSON(r: Refreshed): Record<string, unknown> {
  const out: Record<string, unknown> = { stale: r.stale, changed: r.changed, documents: r.documents };
  if (r.error) out.error = r.error.code;
  else if (r.snapshot) out.tree = r.snapshot.tree;
  return out;
}
