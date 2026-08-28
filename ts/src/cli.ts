#!/usr/bin/env node
/**
 * Dump entrypoint for cross-implementation checking:
 *
 *   node src/cli.ts <config-dir>
 *
 * Exit codes follow the repo-wide dump-CLI convention, so a harness can tell a
 * conformance result from a broken tool:
 *
 *   0  the tree is on stdout as JSON
 *   1  the load failed — the bare E_* code is the first line on stderr
 *   2  any other fault (usage, internal) — no E_* code is printed
 */
import { EntryconfError, load } from "./index.ts";

const USAGE = "usage: node src/cli.ts <config-dir>\n";

const dir = process.argv[2];
if (dir === undefined || process.argv.length > 3) {
  process.stderr.write(USAGE);
  process.exit(2);
}
// The tool takes exactly one positional argument and knows no options, so a
// dash-led first argument is a usage fault, not a directory name: `--help`,
// `-h`, `--`, and anything else starting with `-` must exit 2 (or 0 for the
// help request) and never print an E_* code that a harness could read as a
// conformance result. Without this, `--help` reaches load() and reports
// E_NO_ENTRYPOINT for a directory the user never named.
if (dir === "--help" || dir === "-h") {
  process.stdout.write(USAGE);
  process.exit(0);
}
if (dir.startsWith("-")) {
  // Echoing the argument is redacted the same way an internal fault's detail
  // is: a harness scans stderr for an E_* token, so `-E_PARSE` must not put
  // one there.
  const shown = dir.replace(/E_[A-Z][A-Z0-9_]*/g, "[code redacted]");
  process.stderr.write(`unknown option: ${shown}\n${USAGE}`);
  process.exit(2);
}

try {
  process.stdout.write(`${JSON.stringify(load(dir), null, 2)}\n`);
} catch (err) {
  if (err instanceof EntryconfError) {
    process.stderr.write(`${err.code}\n`);
    process.exit(1);
  }
  // Not a load failure but a fault in this tool (or an I/O failure writing
  // stdout): exit 2 and print no E_* code, so it can never be read as a
  // conformance result. Any code-shaped token in the detail is redacted for
  // the same reason — harnesses scan stderr for the code, not just line one.
  const detail = err instanceof Error ? (err.stack ?? err.message) : String(err);
  process.stderr.write(
    `internal error: ${detail.replace(/E_[A-Z][A-Z0-9_]*/g, "[code redacted]")}\n`,
  );
  process.exit(2);
}
