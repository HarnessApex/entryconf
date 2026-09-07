import { realpathSync } from "node:fs";
import { dirname, join, resolve } from "node:path";

import { mergeEnvFiles, Vars, type EnvLookup } from "./env.ts";
import { EntryconfError } from "./errors.ts";
import { resolveIncludes } from "./includes.ts";
import { interpolate } from "./interpolate.ts";
import { decodeUtf8, parseDocument, type Format } from "./parse.ts";
import { osSource, type FileSource, type Recorder } from "./source.ts";
import { isPlainRecord, type Tree } from "./tree.ts";

const ENTRYPOINTS: [name: string, format: Format][] = [
  ["entrypoint.json", "json"],
  ["entrypoint.yaml", "yaml"],
  ["entrypoint.yml", "yaml"],
  ["entrypoint.toml", "toml"],
];

/**
 * Per-load state: where files come from, the process environment, the
 * variable namespace once built, and (for `open` and plans) the recorder that
 * captures documents, grafts, and variable lookups (SPEC §10.2, §10.6).
 */
export interface Loader {
  src: FileSource;
  procEnv: EnvLookup;
  vars?: Vars;
  rec?: Recorder;
}

export function processEnvLookup(name: string): string | undefined {
  return Object.hasOwn(process.env, name) ? process.env[name] : undefined;
}

export function mapLookup(env: Record<string, string>): EnvLookup {
  return (name) => (Object.hasOwn(env, name) ? env[name] : undefined);
}

/** `load()` with the process environment supplied explicitly (test seam). */
export function loadWith(dir: string, procEnv: EnvLookup): Tree {
  return loadTree({ src: osSource, procEnv }, resolve(dir));
}

/** The five steps of SPEC §1 against `l.src`. */
export function loadTree(l: Loader, root: string): Tree {
  let envNames: string[];
  try {
    envNames = l.src.envFileNames(root);
  } catch {
    throw new EntryconfError(
      "E_NO_ENTRYPOINT",
      `${root}: not a readable config directory`,
    );
  }

  const entrypoint = findEntrypoint(l, root);
  const { files, origin } = mergeEnvFiles(readEnvFiles(l, root, envNames));
  l.vars = new Vars(files, origin, l.procEnv);
  const tree = readAndResolve(l, entrypoint.path, entrypoint.format);
  return interpolate(tree, l.vars);
}

export function findEntrypoint(
  l: Loader,
  root: string,
): { path: string; format: Format } {
  const found = ENTRYPOINTS.filter(([name]) => l.src.isFile(join(root, name)));
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
  l: Loader,
  root: string,
  names: string[],
): { path: string; text: string }[] {
  const files: { path: string; text: string }[] = [];
  for (const name of [...names].sort()) {
    const path = join(root, name);
    const bytes = readBytes(l, path);
    const text = decodeUtf8(bytes, path);
    if (l.rec) l.rec.record(path, bytes, "env", null);
    files.push({ path, text });
  }
  return files;
}

/**
 * Read a file the loader requires (entrypoint or `*.env`): unreadable content
 * is `E_PARSE` (SPEC §2), as is content that is not valid UTF-8.
 */
function readBytes(l: Loader, path: string): Uint8Array {
  try {
    return l.src.readFile(path);
  } catch {
    throw new EntryconfError("E_PARSE", `${path}: unreadable`);
  }
}

function readAndResolve(l: Loader, path: string, format: Format): Tree {
  const bytes = readBytes(l, path);
  const tree = parseDocument(decodeUtf8(bytes, path), format, path);
  // SPEC §3: the entrypoint's own top-level value must be an object — an
  // array, a scalar, or an empty document is E_PARSE. (Included files, §5,
  // may hold any value.)
  if (!isPlainRecord(tree)) {
    throw new EntryconfError(
      "E_PARSE",
      `${path}: the entrypoint's top-level value must be an object`,
    );
  }
  if (l.rec) {
    l.rec.record(path, bytes, format, tree);
    l.rec.graft(path, { effective: "", chain: [] });
  }
  // Includes identify files by their real path for cycle detection, so seed
  // the chain with the entrypoint's real path too.
  let real: string;
  try {
    real = realpathSync(path);
  } catch {
    real = path;
  }
  return resolveIncludes(l, tree, {
    doc: path,
    dir: dirname(path),
    src: "",
    eff: "",
    files: [real],
    chain: [],
  });
}
