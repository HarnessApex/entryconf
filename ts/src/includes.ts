import { readFileSync, realpathSync } from "node:fs";
import { dirname, extname, resolve } from "node:path";

import { EntryconfError } from "./errors.ts";
import { decodeUtf8, formatForExtension, parseDocument } from "./parse.ts";
import { emptyObject, setKey, type Value } from "./tree.ts";

const INCLUDE_PREFIX = "@file:";

/**
 * Replace every `@file:<path>` string with the parsed tree of its target and
 * unescape leading `@@` (SPEC §5). `chain` is the stack of files currently
 * being resolved, used for cycle detection.
 */
export function resolveIncludes(
  value: Value,
  fileDir: string,
  chain: string[],
): Value {
  if (typeof value === "string") return resolveString(value, fileDir, chain);
  if (Array.isArray(value)) {
    return value.map((item) => resolveIncludes(item, fileDir, chain));
  }
  if (typeof value === "object" && value !== null) {
    const out = emptyObject();
    for (const [key, item] of Object.entries(value)) {
      setKey(out, key, resolveIncludes(item, fileDir, chain));
    }
    return out;
  }
  return value;
}

function resolveString(value: string, fileDir: string, chain: string[]): Value {
  if (value.startsWith("@@")) {
    // A doubled leading "@" is a literal "@"; the result is never an include.
    return value.slice(1);
  }
  if (value.startsWith(INCLUDE_PREFIX)) {
    return includeFile(value.slice(INCLUDE_PREFIX.length), fileDir, chain);
  }
  if (value.startsWith("@")) {
    throw new EntryconfError(
      "E_SUBSTITUTION",
      `${JSON.stringify(value)} is not a valid directive; write "@@" for a literal leading "@"`,
    );
  }
  return value;
}

function includeFile(target: string, fileDir: string, chain: string[]): Value {
  if (target === "") {
    throw new EntryconfError("E_INCLUDE", "empty @file: path");
  }
  const path = resolve(fileDir, target);
  const format = formatForExtension(extname(path));
  if (format === null) {
    throw new EntryconfError(
      "E_INCLUDE",
      `${path}: unsupported extension (expected .json, .yaml, .yml, or .toml)`,
    );
  }

  let real: string;
  try {
    real = realpathSync(path);
  } catch {
    throw new EntryconfError("E_INCLUDE", `${path}: missing or unreadable`);
  }
  const seen = chain.indexOf(real);
  if (seen !== -1) {
    throw new EntryconfError(
      "E_INCLUDE_CYCLE",
      `include cycle: ${[...chain.slice(seen), real].join(" -> ")}`,
    );
  }

  let bytes: Uint8Array;
  try {
    bytes = readFileSync(real);
  } catch {
    throw new EntryconfError("E_INCLUDE", `${path}: missing or unreadable`);
  }
  // A missing or unreadable target is E_INCLUDE, but content that is not valid
  // UTF-8 is a parse fault like any other (SPEC §2, §5).
  const tree = parseDocument(decodeUtf8(bytes, real), format, real);
  return resolveIncludes(tree, dirname(real), [...chain, real]);
}
