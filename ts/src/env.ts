import { EntryconfError } from "./errors.ts";
import { stripBom } from "./parse.ts";

const NAME_RE = /^[A-Za-z_][A-Za-z0-9_]*$/;

/** A process-environment lookup; `undefined` means unset. */
export type EnvLookup = (name: string) => string | undefined;

interface Definition {
  value: string;
  file: string;
}

/**
 * Parse one `*.env` file: a strict subset of dotenv (SPEC §4). Every line is
 * blank, a `#` comment, or `NAME=value`; anything else is E_PARSE.
 */
export function parseEnvFile(text: string, path: string): Map<string, string> {
  const defs = new Map<string, string>();
  const lines = stripBom(text).split("\n");
  for (let n = 0; n < lines.length; n++) {
    const raw = lines[n].replace(/\r$/, "");
    const line = raw.trim();
    if (line === "" || line.startsWith("#")) continue;

    const eq = line.indexOf("=");
    const name = eq === -1 ? "" : line.slice(0, eq);
    if (eq === -1 || !NAME_RE.test(name)) {
      throw new EntryconfError(
        "E_PARSE",
        `${path}:${n + 1}: expected a blank line, a "#" comment, or NAME=value`,
      );
    }
    if (defs.has(name)) {
      throw new EntryconfError(
        "E_ENV_CONFLICT",
        `${name} is defined more than once in ${path}`,
      );
    }
    defs.set(name, unquote(line.slice(eq + 1).trim()));
  }
  return defs;
}

function unquote(value: string): string {
  if (value.length >= 2) {
    const first = value[0];
    if ((first === '"' || first === "'") && value.endsWith(first)) {
      return value.slice(1, -1);
    }
  }
  return value;
}

/**
 * The single global variable namespace (SPEC §4): `*.env` definitions with the
 * process environment on top. Every lookup is remembered (SPEC §10.6
 * "variables"), and `where` reports which layer supplies a name.
 */
export class Vars {
  readonly files: Map<string, string>;
  /** name -> path of the `*.env` file defining it */
  readonly origin: Map<string, string>;
  readonly proc: EnvLookup;
  readonly used = new Set<string>();

  constructor(files: Map<string, string>, origin: Map<string, string>, proc: EnvLookup) {
    this.files = files;
    this.origin = origin;
    this.proc = proc;
  }

  get(name: string): string | undefined {
    this.used.add(name);
    const fromProcess = this.proc(name);
    if (fromProcess !== undefined) return fromProcess;
    return this.files.get(name);
  }

  /** "process", "file" (with the `*.env` path), or "" when unset. */
  where(name: string): { origin: "process" | "file" | ""; file: string } {
    if (this.proc(name) !== undefined) return { origin: "process", file: "" };
    const file = this.origin.get(name);
    if (file !== undefined) return { origin: "file", file };
    return { origin: "", file: "" };
  }
}

/**
 * Merge the `*.env` peers: all files are unordered peers, so a name defined
 * twice is a conflict. Returns the values and which file defines each.
 */
export function mergeEnvFiles(
  envFiles: { path: string; text: string }[],
): { files: Map<string, string>; origin: Map<string, string> } {
  const defs = new Map<string, Definition>();
  for (const file of envFiles) {
    for (const [name, value] of parseEnvFile(file.text, file.path)) {
      const existing = defs.get(name);
      if (existing) {
        throw new EntryconfError(
          "E_ENV_CONFLICT",
          `${name} is defined in both ${existing.file} and ${file.path}`,
        );
      }
      defs.set(name, { value, file: file.path });
    }
  }
  const files = new Map<string, string>();
  const origin = new Map<string, string>();
  for (const [name, def] of defs) {
    files.set(name, def.value);
    origin.set(name, def.file);
  }
  return { files, origin };
}

/** Kept for source compatibility with 0.2.0 internals: the merged namespace as a Map. */
export function buildNamespace(
  envFiles: { path: string; text: string }[],
  processEnv: Record<string, string | undefined>,
): Map<string, string> {
  const { files } = mergeEnvFiles(envFiles);
  for (const [name, value] of Object.entries(processEnv)) {
    if (value !== undefined) files.set(name, value);
  }
  return files;
}
