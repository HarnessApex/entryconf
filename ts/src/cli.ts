#!/usr/bin/env node
/**
 * Dump entrypoint for cross-implementation checking:
 *
 *   node src/cli.ts <config-dir>
 *
 * Prints the loaded tree as JSON on stdout and exits 0, or prints the E_* code
 * on stderr and exits 1.
 */
import { EntryconfError, load } from "./index.ts";

const dir = process.argv[2];
if (dir === undefined || process.argv.length > 3) {
  process.stderr.write("usage: node src/cli.ts <config-dir>\n");
  process.exit(2);
}

try {
  process.stdout.write(`${JSON.stringify(load(dir), null, 2)}\n`);
} catch (err) {
  if (err instanceof EntryconfError) {
    process.stderr.write(`${err.code}\n`);
    process.exit(1);
  }
  throw err;
}
