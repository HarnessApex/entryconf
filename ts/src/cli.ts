#!/usr/bin/env node
/**
 * Command line for cross-implementation checking:
 *
 *   node src/cli.ts <config-dir>                          dump the tree
 *   node src/cli.ts inspect <config-dir> [<pointer>]      documents, or one origin
 *   node src/cli.ts edit [-n|--dry-run] <config-dir> <request.json | ->
 *
 * Exit codes follow the repo-wide convention, so a harness can tell a verdict
 * from a broken tool:
 *
 *   0  success — JSON on stdout
 *   1  an entryconf failure — the bare E_* code is the first line on stderr
 *   2  any other fault (usage, internal) — no E_* code is printed
 */
import { readFileSync } from "node:fs";

import { EntryconfError, load, open, type Edit } from "./index.ts";
import { compareCodePoints } from "./jsonfmt.ts";

const USAGE =
  "usage: node src/cli.ts <config-dir>\n" +
  "       node src/cli.ts inspect <config-dir> [<pointer>]\n" +
  "       node src/cli.ts edit [-n|--dry-run] <config-dir> <request.json | ->\n";

function redact(s: string): string {
  return s.replace(/E_[A-Z][A-Z0-9_]*/g, "[code redacted]");
}

/** Object keys sorted recursively by code point so output is comparable. */
function sorted(v: unknown): unknown {
  if (Array.isArray(v)) return v.map(sorted);
  if (v !== null && typeof v === "object") {
    const plain = typeof (v as { toJSON?: unknown }).toJSON === "function"
      ? (v as { toJSON: () => unknown }).toJSON()
      : v;
    if (plain !== v) return sorted(plain);
    const out: Record<string, unknown> = {};
    for (const k of Object.keys(plain as object).sort(compareCodePoints)) {
      Object.defineProperty(out, k, {
        value: sorted((plain as Record<string, unknown>)[k]),
        enumerable: true,
        writable: true,
        configurable: true,
      });
    }
    return out;
  }
  return v;
}

function usage(code: number): never {
  (code === 0 ? process.stdout : process.stderr).write(USAGE);
  process.exit(code);
}

function run(args: string[]): unknown {
  if (args.length === 1) {
    const dir = args[0];
    // The dump form takes exactly one positional argument and knows no
    // options, so a dash-led argument is a usage fault, not a directory name.
    if (dir === "--help" || dir === "-h") usage(0);
    if (dir.startsWith("-")) {
      process.stderr.write(`unknown option: ${redact(dir)}\n${USAGE}`);
      process.exit(2);
    }
    return load(dir);
  }
  if (args[0] === "inspect" && (args.length === 2 || args.length === 3)) {
    const snap = open(args[1]);
    if (args.length === 3) return snap.inspect(args[2]);
    return { dir: snap.dir, documents: snap.documents };
  }
  if (args[0] === "edit") {
    let dryRun = false;
    const rest: string[] = [];
    for (const a of args.slice(1)) {
      if (a === "-n" || a === "--dry-run") dryRun = true;
      else rest.push(a);
    }
    if (rest.length !== 2) usage(2);
    const [dir, source] = rest;
    let text: string;
    try {
      text = readFileSync(source === "-" ? 0 : source, "utf8");
    } catch (err) {
      process.stderr.write(`cannot read request: ${redact((err as Error).message)}\n`);
      process.exit(2);
    }
    let req: { edits?: Edit[] };
    try {
      req = JSON.parse(text);
    } catch (err) {
      process.stderr.write(`request is not JSON: ${redact((err as Error).message)}\n`);
      process.exit(2);
    }
    const plan = open(dir).plan(req.edits ?? []);
    const receipt = dryRun ? null : plan.commit();
    return {
      ...plan.toJSON(),
      committed: receipt !== null,
      revision: receipt ? receipt.revision : null,
    };
  }
  return usage(2);
}

const argv = process.argv.slice(2);
if (argv.length === 0) usage(2);
try {
  const result = run(argv);
  process.stdout.write(`${JSON.stringify(sorted(result), null, 2)}\n`);
} catch (err) {
  if (err instanceof EntryconfError) {
    process.stderr.write(`${err.code}\n`);
    process.exit(1);
  }
  // A fault in this tool, not a verdict: exit 2 and print no E_* code, so it
  // can never be read as a conformance result.
  const detail = err instanceof Error ? (err.stack ?? err.message) : String(err);
  process.stderr.write(`internal error: ${redact(detail)}\n`);
  process.exit(2);
}
