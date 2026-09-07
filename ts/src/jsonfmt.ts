import { isRecord } from "./pointer.ts";
import type { Value } from "./tree.ts";

/** Code-point order — UTF-8 byte order — not UTF-16 code-unit order. */
export function compareCodePoints(a: string, b: string): number {
  return Buffer.compare(Buffer.from(a, "utf8"), Buffer.from(b, "utf8"));
}

/**
 * Normalized JSON (SPEC §10.8): two-space indent, one member or element per
 * line, keys sorted by code point, minimal escaping, integers as integers, one
 * trailing newline.
 */
export function formatJSON(value: Value): string {
  return `${write(value, 0)}\n`;
}

function write(v: Value, depth: number): string {
  if (v === null) return "null";
  switch (typeof v) {
    case "boolean":
      return v ? "true" : "false";
    case "string":
      return quote(v);
    case "number":
      return Number.isInteger(v) && Math.abs(v) < 2 ** 53 ? String(v) : String(v);
  }
  const pad = "  ".repeat(depth + 1);
  const close = "  ".repeat(depth);
  if (Array.isArray(v)) {
    if (v.length === 0) return "[]";
    return `[\n${v.map((item) => pad + write(item, depth + 1)).join(",\n")}\n${close}]`;
  }
  if (isRecord(v)) {
    const keys = Object.keys(v).sort(compareCodePoints);
    if (keys.length === 0) return "{}";
    return `{\n${keys
      .map((k) => `${pad}${quote(k)}: ${write(v[k], depth + 1)}`)
      .join(",\n")}\n${close}}`;
  }
  return "null";
}

function quote(s: string): string {
  let out = '"';
  for (const c of s) {
    switch (c) {
      case '"':
        out += '\\"';
        break;
      case "\\":
        out += "\\\\";
        break;
      case "\b":
        out += "\\b";
        break;
      case "\f":
        out += "\\f";
        break;
      case "\n":
        out += "\\n";
        break;
      case "\r":
        out += "\\r";
        break;
      case "\t":
        out += "\\t";
        break;
      default: {
        const code = c.codePointAt(0)!;
        if (code < 0x20) out += `\\u00${code.toString(16).padStart(2, "0")}`;
        else out += c;
      }
    }
  }
  return `${out}"`;
}
