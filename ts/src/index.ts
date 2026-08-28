import { readFileSync, readdirSync, realpathSync, statSync } from "node:fs";
import { dirname, join, resolve } from "node:path";

import { buildNamespace } from "./env.ts";
import { EntryconfError, type ErrorCode } from "./errors.ts";
import { resolveIncludes } from "./includes.ts";
import { interpolate } from "./interpolate.ts";
import { decodeUtf8, parseDocument, type Format } from "./parse.ts";
import { isPlainRecord, type Tree, type Value } from "./tree.ts";

export { EntryconfError };
export type { ErrorCode, Tree, Value };

const ENTRYPOINTS: [name: string, format: Format][] = [
  ["entrypoint.json", "json"],
  ["entrypoint.yaml", "yaml"],
  ["entrypoint.yml", "yaml"],
  ["entrypoint.toml", "toml"],
];

/**
 * Load a config directory into a single tree (entryconf spec 0.1.0).
 *
 * Locates the entrypoint (§3), builds the variable namespace from the
 * directory's `*.env` files and the process environment (§4), resolves every
 * `@file:` include (§5), then interpolates `$` references (§6). Every failure
 * is an `EntryconfError` whose `code` is one of the normative `E_*` codes.
 */
export function load(dir: string): Tree {
  const root = resolve(dir);
  const names = listDirectory(root);

  const entrypoint = findEntrypoint(root, names);
  const vars = buildNamespace(readEnvFiles(root, names), process.env);
  const tree = readAndResolve(entrypoint.path, entrypoint.format);
  return interpolate(tree, vars);
}

function listDirectory(root: string): string[] {
  try {
    return readdirSync(root);
  } catch {
    throw new EntryconfError(
      "E_NO_ENTRYPOINT",
      `${root}: not a readable config directory`,
    );
  }
}

function isFile(path: string): boolean {
  try {
    return statSync(path).isFile();
  } catch {
    return false;
  }
}

function findEntrypoint(
  root: string,
  names: string[],
): { path: string; format: Format } {
  const found = ENTRYPOINTS.filter(
    ([name]) => names.includes(name) && isFile(join(root, name)),
  );
  if (found.length === 0) {
    throw new EntryconfError(
      "E_NO_ENTRYPOINT",
      `${root}: expected one of ${ENTRYPOINTS.map(([n]) => n).join(", ")}`,
    );
  }
  if (found.length > 1) {
    throw new EntryconfError(
      "E_MULTIPLE_ENTRYPOINTS",
      `${root}: found ${found.map(([n]) => n).join(", ")}`,
    );
  }
  const [name, format] = found[0];
  return { path: join(root, name), format };
}

function readEnvFiles(
  root: string,
  names: string[],
): { path: string; text: string }[] {
  const files: { path: string; text: string }[] = [];
  for (const name of [...names].sort()) {
    if (!name.endsWith(".env")) continue;
    const path = join(root, name);
    if (!isFile(path)) continue;
    files.push({ path, text: readTextFile(path) });
  }
  return files;
}

/**
 * Read a file the loader requires (entrypoint or `*.env`): unreadable content
 * and content that is not valid UTF-8 are both `E_PARSE` (SPEC §2).
 */
function readTextFile(path: string): string {
  let bytes: Uint8Array;
  try {
    bytes = readFileSync(path);
  } catch {
    throw new EntryconfError("E_PARSE", `${path}: unreadable`);
  }
  return decodeUtf8(bytes, path);
}

function readAndResolve(path: string, format: Format): Value {
  const tree = parseDocument(readTextFile(path), format, path);
  // SPEC §3: the entrypoint's own top-level value must be an object — an
  // array, a scalar, or an empty document is E_PARSE. (Included files, §5,
  // may hold any value.)
  if (!isPlainRecord(tree)) {
    throw new EntryconfError(
      "E_PARSE",
      `${path}: the entrypoint's top-level value must be an object`,
    );
  }
  // Includes identify files by their real path, so seed the cycle chain with
  // the entrypoint's real path too.
  let real: string;
  try {
    real = realpathSync(path);
  } catch {
    real = path;
  }
  return resolveIncludes(tree, dirname(real), [real]);
}
