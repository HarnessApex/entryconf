import { EntryconfError } from "./errors.ts";
import { stripBom } from "./parse.ts";

const NAME_RE = /^[A-Za-z_][A-Za-z0-9_]*$/;

interface Definition {
  value: string;
  file: string;
}

/**
 * Parse one `*.env` file: a strict subset of dotenv (SPEC §4). Every line is
 * blank, a `#` comment, or `NAME=value`; anything else is E_PARSE.
 */
function parseEnvFile(text: string, path: string): Map<string, string> {
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
 * Build the single global variable namespace (SPEC §4): all `*.env` files in
 * the config directory are unordered peers, so a name defined twice is a
 * conflict; the process environment then overrides whatever they define.
 */
export function buildNamespace(
  envFiles: { path: string; text: string }[],
  processEnv: Record<string, string | undefined>,
): Map<string, string> {
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

  const namespace = new Map<string, string>();
  for (const [name, def] of defs) namespace.set(name, def.value);
  for (const [name, value] of Object.entries(processEnv)) {
    if (value !== undefined) namespace.set(name, value);
  }
  return namespace;
}
