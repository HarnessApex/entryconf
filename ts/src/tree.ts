/**
 * The loaded tree is JSON-equivalent (SPEC §2): null, boolean, number, string,
 * array, object with string keys.
 */
export type Value =
  | null
  | boolean
  | number
  | string
  | Value[]
  | { [key: string]: Value };

export type Tree = Value;

/**
 * Assign an own data property, so that a config key literally named
 * `__proto__` becomes a plain own key instead of mutating the prototype.
 */
export function setKey(
  obj: { [key: string]: Value },
  key: string,
  value: Value,
): void {
  Object.defineProperty(obj, key, {
    value,
    writable: true,
    enumerable: true,
    configurable: true,
  });
}

export function emptyObject(): { [key: string]: Value } {
  return {};
}

export function isPlainRecord(v: Value): v is { [key: string]: Value } {
  return typeof v === "object" && v !== null && !Array.isArray(v);
}
