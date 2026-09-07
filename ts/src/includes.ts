import { realpathSync } from "node:fs";
import { dirname, extname, resolve } from "node:path";

import type { Reference } from "./edit.ts";
import { EntryconfError } from "./errors.ts";
import type { Loader } from "./loader.ts";
import { decodeUtf8, formatForExtension, parseDocument } from "./parse.ts";
import { escapePointerToken } from "./pointer.ts";
import { emptyObject, setKey, type Value } from "./tree.ts";

export const INCLUDE_PREFIX = "@file:";

/**
 * The include resolver's position: the file whose value is being walked and
 * its directory (paths are relative to the referencing file), the pointer
 * within that file (`src`) and within the effective tree (`eff`), the real
 * paths of the files being resolved (cycle detection, innermost last), and the
 * `@file:` references traversed so far (SPEC §10.3 "chain", absolute paths
 * until `open` keys them).
 */
export interface Walk {
  doc: string;
  dir: string;
  src: string;
  eff: string;
  files: string[];
  chain: Reference[];
}

function step(w: Walk, token: string): Walk {
  const t = `/${escapePointerToken(token)}`;
  return { ...w, src: w.src + t, eff: w.eff + t };
}

/** Whether an authored string is an `@file:` reference. */
export function isInclude(s: string): boolean {
  return s.startsWith(INCLUDE_PREFIX);
}

/** The cleaned absolute path an `@file:` string names, relative to `dir`. */
export function includeTarget(s: string, dir: string): string {
  return resolve(dir, s.slice(INCLUDE_PREFIX.length));
}

/**
 * Replace every `@file:<path>` string with the parsed tree of its target and
 * unescape leading `@@` (SPEC §5).
 */
export function resolveIncludes(l: Loader, value: Value, w: Walk): Value {
  if (typeof value === "string") return resolveString(l, value, w);
  if (Array.isArray(value)) {
    return value.map((item, i) => resolveIncludes(l, item, step(w, String(i))));
  }
  if (typeof value === "object" && value !== null) {
    const out = emptyObject();
    for (const [key, item] of Object.entries(value)) {
      setKey(out, key, resolveIncludes(l, item, step(w, key)));
    }
    return out;
  }
  return value;
}

function resolveString(l: Loader, value: string, w: Walk): Value {
  if (value.startsWith("@@")) {
    // A doubled leading "@" is a literal "@"; the result is never an include.
    return value.slice(1);
  }
  if (isInclude(value)) {
    return includeFile(l, value.slice(INCLUDE_PREFIX.length), w);
  }
  if (value.startsWith("@")) {
    throw new EntryconfError(
      "E_SUBSTITUTION",
      `${JSON.stringify(value)} is not a valid directive; write "@@" for a literal leading "@"`,
    );
  }
  return value;
}

function includeFile(l: Loader, target: string, w: Walk): Value {
  if (target === "") {
    throw new EntryconfError("E_INCLUDE", "empty @file: path");
  }
  const path = resolve(w.dir, target);
  const format = formatForExtension(extname(path));
  if (format === null) {
    throw new EntryconfError(
      "E_INCLUDE",
      `${path}: unsupported extension (expected .json, .yaml, .yml, or .toml)`,
    );
  }

  // Cycles are detected on real paths, so a file reached through two names
  // is still one file; everything else uses the path as referenced.
  let real: string;
  try {
    real = l.src.isFile(path) ? realpathSync(path) : path;
  } catch {
    throw new EntryconfError("E_INCLUDE", `${path}: missing or unreadable`);
  }
  const seen = w.files.indexOf(real);
  if (seen !== -1) {
    throw new EntryconfError(
      "E_INCLUDE_CYCLE",
      `include cycle: ${[...w.files.slice(seen), real].join(" -> ")}`,
    );
  }

  let bytes: Uint8Array;
  try {
    bytes = l.src.readFile(path);
  } catch {
    throw new EntryconfError("E_INCLUDE", `${path}: missing or unreadable`);
  }
  // A missing or unreadable target is E_INCLUDE, but content that is not valid
  // UTF-8 is a parse fault like any other (SPEC §2, §5).
  const tree = parseDocument(decodeUtf8(bytes, path), format, path);
  const chain: Reference[] = [...w.chain, { document: w.doc, pointer: w.src }];
  if (l.rec) {
    l.rec.record(path, bytes, format, tree);
    l.rec.graft(path, { effective: w.eff, chain });
  }
  return resolveIncludes(l, tree, {
    doc: path,
    dir: dirname(path),
    src: "",
    eff: w.eff,
    files: [...w.files, real],
    chain,
  });
}
