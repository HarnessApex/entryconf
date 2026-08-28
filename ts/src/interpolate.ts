import { EntryconfError } from "./errors.ts";
import { emptyObject, setKey, type Value } from "./tree.ts";

const NAME_RE = /^[A-Za-z_][A-Za-z0-9_]*$/;
const NAME_START_RE = /[A-Za-z_]/;
const NAME_CHAR_RE = /[A-Za-z0-9_]/;
const JSON_NUMBER_RE = /^-?(?:0|[1-9][0-9]*)(?:\.[0-9]+)?(?:[eE][+-]?[0-9]+)?$/;

/**
 * Interpolate `$` references across the assembled tree (SPEC §6). Object keys
 * are never interpolated.
 */
export function interpolate(value: Value, vars: Map<string, string>): Value {
  if (typeof value === "string") return interpolateString(value, vars);
  if (Array.isArray(value)) {
    return value.map((item) => interpolate(item, vars));
  }
  if (typeof value === "object" && value !== null) {
    const out = emptyObject();
    for (const [key, item] of Object.entries(value)) {
      setKey(out, key, interpolate(item, vars));
    }
    return out;
  }
  return value;
}

function badForm(source: string, detail: string): EntryconfError {
  return new EntryconfError(
    "E_SUBSTITUTION",
    `${JSON.stringify(source)}: ${detail}`,
  );
}

function interpolateString(source: string, vars: Map<string, string>): Value {
  let out = "";
  let references = 0;
  let wholeValue = false;
  let i = 0;

  while (i < source.length) {
    const c = source[i];
    if (c !== "$") {
      out += c;
      i++;
      continue;
    }

    const next = i + 1 < source.length ? source[i + 1] : "";
    if (next === "") {
      throw badForm(source, 'trailing "$" (write "$$" for a literal "$")');
    }
    if (next === "$") {
      out += "$";
      i += 2;
      continue;
    }

    const start = i;
    let name: string;
    let fallback: string | null = null;

    if (next === "{") {
      const close = source.indexOf("}", i + 2);
      if (close === -1) throw badForm(source, 'unterminated "${"');
      const inner = source.slice(i + 2, close);
      const colon = inner.indexOf(":");
      if (colon === -1) {
        if (!NAME_RE.test(inner)) {
          throw badForm(source, `"\${${inner}}" is not a valid reference`);
        }
        name = inner;
      } else {
        name = inner.slice(0, colon);
        const rest = inner.slice(colon + 1);
        if (!NAME_RE.test(name) || !rest.startsWith("-")) {
          // "${NAME:...}" other than ":-" is reserved for future extensions.
          throw badForm(source, `"\${${inner}}" is not a valid reference`);
        }
        fallback = rest.slice(1);
      }
      i = close + 1;
    } else if (NAME_START_RE.test(next)) {
      let j = i + 1;
      while (j < source.length && NAME_CHAR_RE.test(source[j])) j++;
      name = source.slice(i + 1, j);
      i = j;
    } else {
      throw badForm(source, `"$${next}" is not a valid reference`);
    }

    const defined = vars.get(name);
    let resolved: string;
    if (defined !== undefined) {
      resolved = defined;
    } else if (fallback !== null) {
      // The default text is literal: it is never re-scanned.
      resolved = fallback;
    } else {
      throw new EntryconfError(
        "E_MISSING_VAR",
        `${name} is not set and has no default (in ${JSON.stringify(source)})`,
      );
    }

    out += resolved;
    references++;
    wholeValue = start === 0 && i === source.length;
  }

  // Whole-value typing: exactly one reference and nothing else.
  if (references === 1 && wholeValue) {
    if (out === "true") return true;
    if (out === "false") return false;
    if (out === "null") return null;
    if (JSON_NUMBER_RE.test(out)) {
      // A number only becomes a number if it parses to a *finite* double;
      // an overflowing literal such as "1e400" stays a string (SPEC §6).
      const number = Number(out);
      if (Number.isFinite(number)) return number;
    }
  }
  return out;
}
