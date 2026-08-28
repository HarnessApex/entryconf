import * as YAML from "yaml";
import { parse as parseTomlText } from "smol-toml";

import { EntryconfError } from "./errors.ts";
import { emptyObject, setKey, type Value } from "./tree.ts";

export type Format = "json" | "yaml" | "toml";

const EXTENSIONS: Record<string, Format> = {
  ".json": "json",
  ".yaml": "yaml",
  ".yml": "yaml",
  ".toml": "toml",
};

/**
 * Parser selected by file extension (SPEC §5); null for unsupported ones.
 * The match is case-sensitive: `.JSON` is "any other extension".
 */
export function formatForExtension(ext: string): Format | null {
  return Object.hasOwn(EXTENSIONS, ext) ? EXTENSIONS[ext] : null;
}

export function stripBom(text: string): string {
  return text.charCodeAt(0) === 0xfeff ? text.slice(1) : text;
}

/**
 * Decode file bytes as UTF-8, strictly. `readFileSync(path, "utf8")` silently
 * substitutes U+FFFD for invalid bytes; SPEC §2 makes content that is not
 * valid UTF-8 an `E_PARSE` anywhere it appears.
 */
export function decodeUtf8(bytes: Uint8Array, path: string): string {
  try {
    return new TextDecoder("utf-8", { fatal: true }).decode(bytes);
  } catch {
    throw parseError(path, "content is not valid UTF-8");
  }
}

function parseError(path: string, detail: string): EntryconfError {
  return new EntryconfError("E_PARSE", `${path}: ${detail}`);
}

export function parseDocument(
  text: string,
  format: Format,
  path: string,
): Value {
  const source = stripBom(text);
  switch (format) {
    case "json":
      return normalize(parseJson(source, path), path, false);
    case "yaml":
      // Only YAML has aliases, so only YAML carries the expansion budget
      // (SPEC §2); a legitimately huge JSON or TOML document is not bounded.
      return normalize(parseYaml(source, path), path, true);
    case "toml":
      return normalize(parseToml(source, path), path, false);
  }
}

// --- JSON ------------------------------------------------------------------

function parseJson(text: string, path: string): unknown {
  let value: unknown;
  try {
    value = JSON.parse(text);
  } catch (err) {
    throw parseError(path, (err as Error).message);
  }
  // JSON.parse is last-wins on duplicate keys; SPEC §2 makes them E_PARSE, so
  // scan the (already validated) token stream for repeated keys per object.
  assertNoDuplicateJsonKeys(text, path);
  return value;
}

function assertNoDuplicateJsonKeys(text: string, path: string): void {
  // `text` has already been accepted by JSON.parse, so the scan below can rely
  // on the document being well-formed: a string token followed by `:` while an
  // object is open is a key, and nothing else is.
  const stack: (Set<string> | null)[] = [];
  let i = 0;
  while (i < text.length) {
    const c = text[i];
    if (c === "{") {
      stack.push(new Set<string>());
      i++;
    } else if (c === "[") {
      stack.push(null);
      i++;
    } else if (c === "}" || c === "]") {
      stack.pop();
      i++;
    } else if (c === '"') {
      const end = scanJsonString(text, i);
      const raw = text.slice(i, end);
      i = end;
      let j = i;
      while (j < text.length && isJsonWhitespace(text[j])) j++;
      const keys = stack.length > 0 ? stack[stack.length - 1] : null;
      if (text[j] === ":" && keys) {
        const key = JSON.parse(raw) as string;
        if (keys.has(key)) {
          throw parseError(path, `duplicate key ${JSON.stringify(key)}`);
        }
        keys.add(key);
      }
    } else {
      i++;
    }
  }
}

function isJsonWhitespace(c: string): boolean {
  return c === " " || c === "\t" || c === "\n" || c === "\r";
}

/** Index just past the closing quote of the string token starting at `start`. */
function scanJsonString(text: string, start: number): number {
  let i = start + 1;
  while (i < text.length) {
    const c = text[i];
    if (c === "\\") {
      i += 2;
      continue;
    }
    if (c === '"') return i + 1;
    i++;
  }
  return text.length;
}

// --- YAML ------------------------------------------------------------------

function parseYaml(text: string, path: string): unknown {
  const docs = YAML.parseAllDocuments(text, {
    logLevel: "silent",
    schema: "core",
    version: "1.2",
    // Duplicate keys are E_PARSE (SPEC §2), but the `yaml` package's own
    // `uniqueKeys` check compares each new key against every key already in
    // the mapping — quadratic, and ruinous on a large alias-free mapping
    // (hundreds of seconds for a few hundred thousand entries). Detect them
    // linearly in `assertYamlKeys` instead, which already visits every Pair.
    uniqueKeys: false,
    merge: false,
    intAsBigInt: false,
  });
  if (docs.length > 1) {
    throw parseError(path, "multi-document YAML streams are not supported");
  }
  if (docs.length === 0) return null;
  const doc = docs[0];
  if (doc.errors.length > 0) {
    const first = doc.errors[0];
    throw parseError(path, first.message);
  }
  // Unresolvable (custom) tags are warnings in the `yaml` package; SPEC §2
  // makes them E_PARSE.
  for (const warning of doc.warnings) {
    if (warning.code === "TAG_RESOLVE_FAILED") {
      throw parseError(path, warning.message);
    }
  }
  // Runs on the AST, before `toJS` expands aliases and before the node budget
  // is charged, so a document that is both duplicate-keyed and oversized is
  // rejected on the key — a dup-key bomb cannot slip past the check by being
  // expensive to expand.
  assertYamlKeys(doc, path);
  try {
    // `maxAliasCount` is a *toJS* option, and its default (100) counts alias
    // references rather than expanded nodes: it rejects honest heavy reuse
    // (case 58 makes 109 references expanding to well under a thousand nodes)
    // while saying nothing about the actual expanded size. Disable it and let
    // `normalize` enforce the real budget SPEC §2 specifies. With aliases
    // unresolved into copies, `toJS` returns a cheap shared graph — repeated
    // aliases to one anchor yield the *same* object — so the expansion (and
    // the blowup an alias bomb is after) happens in `normalize`, which is
    // where the node count has to be charged.
    return doc.toJS({ maxAliasCount: -1 });
  } catch (err) {
    throw parseError(path, (err as Error).message);
  }
}

/**
 * Enforce the two key rules of SPEC §2 in one AST walk: mapping keys MUST be
 * strings, and a duplicate key within one document is `E_PARSE`.
 *
 * Both checks run on the parsed AST, where every anchor appears exactly once
 * and an alias is a single node, so the walk costs the size of the *source*
 * document rather than the size of its expansion — and it is linear in that
 * size: each mapping's keys go into a `Set` of strings, one lookup per entry,
 * in place of the `yaml` package's pairwise `uniqueKeys` comparison.
 */
function assertYamlKeys(doc: YAML.Document.Parsed, path: string): void {
  YAML.visit(doc, {
    Map(_key, node) {
      const seen = new Set<string>();
      for (const pair of node.items) {
        const k = pair.key;
        // Non-string keys are rejected by the `Pair` visitor below, which this
        // pre-order walk reaches right after this mapping; skip them here so
        // the two checks cannot disagree about what a key's text is.
        if (!YAML.isScalar(k) || typeof k.value !== "string") continue;
        if (seen.has(k.value)) {
          throw parseError(path, `duplicate key ${JSON.stringify(k.value)}`);
        }
        seen.add(k.value);
      }
    },
    Pair(_key, pair) {
      const k = pair.key;
      if (!YAML.isScalar(k) || typeof k.value !== "string") {
        throw parseError(
          path,
          "mapping keys must be strings for a JSON-equivalent tree",
        );
      }
    },
  });
}

// --- TOML ------------------------------------------------------------------

function parseToml(text: string, path: string): unknown {
  assertNoTomlOffsetTime(text, path);
  try {
    return parseTomlText(text);
  } catch (err) {
    throw parseError(path, (err as Error).message);
  }
}

/**
 * TOML 1.0 has no offset-time type — an offset is only valid after a full
 * date-time — but smol-toml accepts `07:32:00+05:00`, silently shifts the
 * clock, and returns a local-time `TomlDate` that keeps no trace of the
 * offset (SPEC §2 makes such input `E_PARSE`). The `TomlDate` is
 * indistinguishable from a genuine local time, so the check must run on the
 * source text: blank out strings and comments, then flag any offset-time
 * token that is not preceded by a date and separator.
 */
function assertNoTomlOffsetTime(text: string, path: string): void {
  const scannable = blankTomlStringsAndComments(text);
  const offsetTime = /\d{2}:\d{2}:\d{2}(?:\.\d+)?(?:[Zz]|[+-]\d{2}:\d{2})/g;
  for (const match of scannable.matchAll(offsetTime)) {
    const start = match.index;
    const inDateTime =
      start >= 2 &&
      /[Tt ]/.test(scannable[start - 1]) &&
      /\d/.test(scannable[start - 2]);
    if (!inDateTime) {
      throw parseError(
        path,
        `${match[0]} is not a TOML value (a time offset requires a full date-time)`,
      );
    }
  }
}

/**
 * Replace the contents of TOML strings and comments with spaces (newlines
 * kept), so token-level scans cannot be fooled by string values. Handles
 * basic/literal strings, their multiline forms (including up to two closing
 * quotes owned by the content), and `\` escapes in basic strings. On
 * malformed input this errs toward blanking too much — smol-toml still
 * governs the actual parse, so the worst case is a missed pre-check, never a
 * false E_PARSE.
 */
function blankTomlStringsAndComments(text: string): string {
  let out = "";
  let i = 0;
  while (i < text.length) {
    const c = text[i];
    if (c === "#") {
      while (i < text.length && text[i] !== "\n") {
        out += " ";
        i++;
      }
      continue;
    }
    if (c === '"' || c === "'") {
      const quote = c;
      const triple = text.startsWith(quote.repeat(3), i);
      const delim = triple ? quote.repeat(3) : quote;
      out += " ".repeat(delim.length);
      i += delim.length;
      while (i < text.length) {
        if (quote === '"' && text[i] === "\\") {
          out += "  ";
          i += 2;
          continue;
        }
        if (text.startsWith(delim, i)) {
          if (triple) {
            // A multiline delimiter may be preceded by 1-2 quotes that belong
            // to the content ("""" ends a string whose content ends in `"`).
            let extra = 0;
            while (extra < 2 && text[i + delim.length + extra] === quote) {
              extra++;
            }
            out += " ".repeat(extra);
            i += extra;
          }
          out += " ".repeat(delim.length);
          i += delim.length;
          break;
        }
        out += text[i] === "\n" ? "\n" : " ";
        i++;
      }
      continue;
    }
    out += c;
    i++;
  }
  return out;
}

/**
 * smol-toml returns a `TomlDate` (a `Date` subclass) whose `toISOString()`
 * preserves the authored shape — local date, local time, local date-time, or
 * offset date-time — but keeps the source's own spelling of the separator and
 * the offset, and always renders milliseconds. SPEC §2 pins one rendering:
 * uppercase `T` separator, a UTC offset (`Z`, `z`, or `+00:00`) written as
 * `Z`, any other offset in its authored numeric form, and fractional seconds
 * with trailing zeros dropped (the `.` going with them at zero).
 */
function tomlDateToString(date: Date): string {
  let text = date.toISOString();

  // TOML permits "t"/" " as the date/time separator; the spec pins "T".
  text = text.replace(/^(\d{4}-\d{2}-\d{2})[tT ](?=\d{2}:)/, "$1T");

  // Split the offset off so it cannot be confused with a time fragment.
  let offset = "";
  const found = /(?:[zZ]|[+-]\d{2}:\d{2})$/.exec(text);
  if (found) {
    const authored = found[0];
    text = text.slice(0, text.length - authored.length);
    offset =
      authored === "z" || authored === "Z" || authored === "+00:00"
        ? "Z"
        : authored;
  }

  // Fractional seconds: drop trailing zeros, and the "." at zero.
  text = text.replace(
    /(\d{2}:\d{2}:\d{2})\.(\d+)$/,
    (_all, time: string, fraction: string) => {
      const trimmed = fraction.replace(/0+$/, "");
      return trimmed === "" ? time : `${time}.${trimmed}`;
    },
  );

  return text + offset;
}

// --- normalization ---------------------------------------------------------

/**
 * SPEC §2: a YAML document whose fully expanded tree would exceed this many
 * nodes is `E_PARSE`. Each scalar value, sequence element, and mapping entry
 * counts as one.
 */
export const MAX_EXPANDED_NODES = 1_000_000;

/** State threaded through the normalization walk. */
interface NormalizeContext {
  readonly path: string;
  /**
   * The chain of collections currently being walked, so an alias pointing at
   * one of its own ancestors — a cyclic structure rather than a plain value —
   * is caught. Entries are removed on the way back out, so a node reachable
   * many times over (the normal alias case) is not mistaken for a cycle.
   */
  readonly ancestors: Set<object>;
  /** Nodes still affordable, or null when no budget applies (JSON/TOML). */
  budget: number | null;
}

/**
 * Convert a parser's output into the JSON-equivalent data model, rejecting
 * anything that is not representable (SPEC §2).
 *
 * When `bounded` (YAML), the walk also charges the alias-expansion budget.
 * The walk *is* the expansion: `toJS` hands back a graph in which every alias
 * to one anchor is the same object, and rebuilding it here into a plain tree
 * duplicates those shared structures. So counting nodes as they are produced
 * measures exactly what SPEC §2 bounds — the size of the expanded tree — and
 * an alias bomb is stopped after a million nodes instead of materializing all
 * 48 million of them.
 */
function normalize(value: unknown, path: string, bounded: boolean): Value {
  return normalizeValue(value, {
    path,
    ancestors: new Set(),
    budget: bounded ? MAX_EXPANDED_NODES : null,
  });
}

/** Charge one node against the budget, failing once it is spent. */
function charge(ctx: NormalizeContext): void {
  if (ctx.budget === null) return;
  if (ctx.budget === 0) {
    throw parseError(
      ctx.path,
      `alias expansion exceeds the ${MAX_EXPANDED_NODES}-node budget`,
    );
  }
  ctx.budget -= 1;
}

function normalizeValue(value: unknown, ctx: NormalizeContext): Value {
  const path = ctx.path;
  if (value === null || value === undefined) {
    charge(ctx);
    return null;
  }
  switch (typeof value) {
    case "boolean":
    case "string":
      charge(ctx);
      return value;
    case "number":
      if (!Number.isFinite(value)) {
        throw parseError(
          path,
          `${String(value)} is not representable in the JSON-equivalent data model`,
        );
      }
      charge(ctx);
      return value;
    case "bigint":
      throw parseError(path, "integer is out of range for a JSON number");
    case "object":
      break;
    default:
      throw parseError(path, `unsupported value of type ${typeof value}`);
  }
  if (value instanceof Date) {
    charge(ctx);
    return tomlDateToString(value);
  }

  const node = value as object;
  if (ctx.ancestors.has(node)) {
    throw parseError(path, "recursive alias does not resolve to a plain value");
  }
  ctx.ancestors.add(node);
  try {
    if (Array.isArray(value)) {
      const items: Value[] = [];
      for (const item of value) {
        charge(ctx); // the sequence element
        items.push(normalizeValue(item, ctx));
      }
      return items;
    }
    const out = emptyObject();
    for (const [key, item] of Object.entries(node)) {
      charge(ctx); // the mapping entry
      setKey(out, key, normalizeValue(item, ctx));
    }
    return out;
  } finally {
    ctx.ancestors.delete(node);
  }
}
