import { EntryconfError } from "./errors.ts";
import { emptyObject, setKey, type Value } from "./tree.ts";

/**
 * Split an RFC 6901 JSON Pointer into tokens (SPEC §10.3). `""` is the root.
 * A pointer not starting with `/`, or with `~` not followed by `0`/`1`, is
 * `E_PATH`.
 */
export function parsePointer(pointer: string): string[] {
  if (pointer === "") return [];
  if (!pointer.startsWith("/")) {
    throw new EntryconfError(
      "E_PATH",
      `pointer ${JSON.stringify(pointer)} must be empty or start with "/"`,
    );
  }
  return pointer
    .slice(1)
    .split("/")
    .map((raw) => {
      let out = "";
      for (let i = 0; i < raw.length; i++) {
        const c = raw[i];
        if (c !== "~") {
          out += c;
          continue;
        }
        const next = raw[i + 1];
        if (next === "0") out += "~";
        else if (next === "1") out += "/";
        else {
          throw new EntryconfError(
            "E_PATH",
            `pointer ${JSON.stringify(pointer)}: "~" must be followed by 0 or 1`,
          );
        }
        i++;
      }
      return out;
    });
}

export function escapePointerToken(token: string): string {
  return token.replaceAll("~", "~0").replaceAll("/", "~1");
}

export function joinPointer(tokens: string[]): string {
  return tokens.map((t) => `/${escapePointerToken(t)}`).join("");
}

/** An array index token: decimal digits, no leading zeros; `-` is not one. */
export function arrayIndex(token: string): number | null {
  if (!/^(0|[1-9][0-9]*)$/.test(token)) return null;
  const n = Number(token);
  return Number.isSafeInteger(n) ? n : null;
}

export function isRecord(v: Value): v is { [key: string]: Value } {
  return typeof v === "object" && v !== null && !Array.isArray(v);
}

export function deepCopy(v: Value): Value {
  if (Array.isArray(v)) return v.map(deepCopy);
  if (isRecord(v)) {
    const out = emptyObject();
    for (const [k, item] of Object.entries(v)) setKey(out, k, deepCopy(item));
    return out;
  }
  return v;
}

function pathError(pointer: string, detail: string): EntryconfError {
  return new EntryconfError("E_PATH", `pointer ${JSON.stringify(pointer)}: ${detail}`);
}

/**
 * Walk to the parent of the last token. With `create`, missing object members
 * become empty objects (SPEC §10.5 "set"); array steps must always exist.
 * Returns the containers on the path so an array replacement can be written
 * back into its holder.
 */
function walkToParent(
  root: Value,
  tokens: string[],
  pointer: string,
  create: boolean,
): { parent: Value; holders: { container: Value; token: string }[] } {
  let parent = root;
  const holders: { container: Value; token: string }[] = [];
  for (const tok of tokens.slice(0, -1)) {
    if (isRecord(parent)) {
      let child = Object.hasOwn(parent, tok) ? parent[tok] : undefined;
      if (child === undefined) {
        if (!create) throw pathError(pointer, `no member ${JSON.stringify(tok)}`);
        child = emptyObject();
        setKey(parent, tok, child);
      }
      holders.push({ container: parent, token: tok });
      parent = child;
    } else if (Array.isArray(parent)) {
      const i = arrayIndex(tok);
      if (i === null || i >= parent.length) {
        throw pathError(pointer, `array index ${JSON.stringify(tok)} does not exist`);
      }
      holders.push({ container: parent, token: tok });
      parent = parent[i];
    } else {
      throw pathError(pointer, `cannot descend into a scalar at ${JSON.stringify(tok)}`);
    }
  }
  return { parent, holders };
}

function writeBack(
  root: Value,
  holders: { container: Value; token: string }[],
  replaced: Value,
): Value {
  if (holders.length === 0) return replaced;
  const { container, token } = holders[holders.length - 1];
  if (isRecord(container)) setKey(container, token, replaced);
  else if (Array.isArray(container)) container[arrayIndex(token)!] = replaced;
  return root;
}

/** Apply a "set" (SPEC §10.5) and return the (possibly new) root. */
export function setAt(root: Value, pointer: string, value: Value): Value {
  const tokens = parsePointer(pointer);
  if (tokens.length === 0) return value;
  const { parent, holders } = walkToParent(root, tokens, pointer, true);
  const last = tokens[tokens.length - 1];
  if (isRecord(parent)) {
    setKey(parent, last, value);
    return root;
  }
  if (Array.isArray(parent)) {
    if (last === "-") return writeBack(root, holders, [...parent, value]);
    const i = arrayIndex(last);
    if (i === null || i > parent.length) {
      throw pathError(pointer, `array index ${JSON.stringify(last)} is out of range (0..${parent.length})`);
    }
    if (i === parent.length) return writeBack(root, holders, [...parent, value]);
    parent[i] = value;
    return root;
  }
  throw pathError(pointer, "cannot set a member of a scalar");
}

/** Apply a "remove" (SPEC §10.5): every step must exist; the root cannot go. */
export function removeAt(root: Value, pointer: string): Value {
  const tokens = parsePointer(pointer);
  if (tokens.length === 0) {
    throw new EntryconfError("E_PATH", "the root value cannot be removed");
  }
  const { parent, holders } = walkToParent(root, tokens, pointer, false);
  const last = tokens[tokens.length - 1];
  if (isRecord(parent)) {
    if (!Object.hasOwn(parent, last)) {
      throw pathError(pointer, `no member ${JSON.stringify(last)} to remove`);
    }
    delete parent[last];
    return root;
  }
  if (Array.isArray(parent)) {
    const i = arrayIndex(last);
    if (i === null || i >= parent.length) {
      throw pathError(pointer, `array index ${JSON.stringify(last)} does not exist`);
    }
    return writeBack(root, holders, [...parent.slice(0, i), ...parent.slice(i + 1)]);
  }
  throw pathError(pointer, "cannot remove a member of a scalar");
}
