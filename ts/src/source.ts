import { readFileSync, readdirSync, statSync } from "node:fs";
import { join, relative, sep } from "node:path";

import type { Graft } from "./edit.ts";
import type { Value } from "./tree.ts";

/**
 * Where the loader gets its bytes. `load()` reads the real filesystem; `open()`
 * reads it while recording; a plan's candidate evaluation (SPEC §10.6) reads
 * the snapshot's recorded bytes with the edited document replaced in memory,
 * falling back to disk only for a file the snapshot never saw (a newly written
 * `@file:` reference).
 */
export interface FileSource {
  readFile(path: string): Uint8Array;
  isFile(path: string): boolean;
  /** The `*.env` files directly in `dir` (SPEC §4); throws if unreadable. */
  envFileNames(dir: string): string[];
}

export const osSource: FileSource = {
  readFile(path) {
    return readFileSync(path);
  },
  isFile(path) {
    try {
      return statSync(path).isFile();
    } catch {
      return false;
    }
  },
  envFileNames(dir) {
    return readdirSync(dir).filter(
      (name) => name.endsWith(".env") && osSource.isFile(join(dir, name)),
    );
  },
};

/** Recorded bytes first, disk second; the `*.env` listing the snapshot saw. */
export class OverlaySource implements FileSource {
  readonly files: Map<string, Uint8Array>;
  readonly envNames: string[];
  readonly fallback: FileSource;

  constructor(files: Map<string, Uint8Array>, envNames: string[], fallback: FileSource) {
    this.files = files;
    this.envNames = envNames;
    this.fallback = fallback;
  }

  readFile(path: string): Uint8Array {
    const data = this.files.get(path);
    return data !== undefined ? data : this.fallback.readFile(path);
  }

  isFile(path: string): boolean {
    return this.files.has(path) || this.fallback.isFile(path);
  }

  envFileNames(): string[] {
    return [...this.envNames];
  }
}

/** One document a load touched (SPEC §10.2). */
export interface DocRecord {
  path: string;
  data: Uint8Array;
  format: "json" | "yaml" | "toml" | "env" | "";
  parsed: Value | null;
  /** Grafts as recorded: chain documents are absolute paths until `open` keys them. */
  grafts: Graft[];
}

/** Captures what one load touched: documents, their raw parsed values, grafts. */
export class Recorder {
  readonly docs = new Map<string, DocRecord>();
  readonly order: string[] = [];

  record(path: string, data: Uint8Array, format: DocRecord["format"], parsed: Value | null): void {
    const existing = this.docs.get(path);
    if (existing && existing.format !== "") return; // shared include: recorded once
    if (existing) {
      existing.data = data;
      existing.format = format;
      existing.parsed = parsed;
      return;
    }
    this.docs.set(path, { path, data, format, parsed, grafts: [] });
    this.order.push(path);
  }

  graft(path: string, g: Graft): void {
    let rec = this.docs.get(path);
    if (!rec) {
      // The entrypoint is grafted before it is recorded; make the slot.
      rec = { path, data: new Uint8Array(), format: "", parsed: null, grafts: [] };
      this.docs.set(path, rec);
      this.order.push(path);
    }
    rec.grafts.push(g);
  }
}

/**
 * SPEC §10.2's document key: the path relative to the config directory,
 * slash-separated, or the absolute path when no relative form exists.
 */
export function documentKey(dir: string, path: string): string {
  let rel: string;
  try {
    rel = relative(dir, path);
  } catch {
    rel = path;
  }
  if (rel === "" ) return ".";
  // A different drive on Windows yields an absolute path; keep it.
  return rel.split(sep).join("/");
}
