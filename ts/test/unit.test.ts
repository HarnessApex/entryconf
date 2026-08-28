/**
 * The few behaviors the shared fixture suite cannot express.
 *
 * Everything that *can* be a fixture belongs in `testdata/cases/` — this file
 * is only for contracts git or the harness cannot represent: a config
 * directory that does not exist (git stores no empty or absent directory), the
 * expansion budget's constant, and the dump CLI's exit-code convention, which
 * lives outside `load()` altogether.
 */
import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { after, test } from "node:test";
import { fileURLToPath } from "node:url";

import { EntryconfError, load } from "../src/index.ts";
import { MAX_EXPANDED_NODES } from "../src/parse.ts";

const here = dirname(fileURLToPath(import.meta.url));
const cli = resolve(here, "..", "src", "cli.ts");
const casesDir = resolve(here, "..", "..", "testdata", "cases");

const scratch = mkdtempSync(join(tmpdir(), "entryconf-ts-unit-"));
after(() => {
  rmSync(scratch, { recursive: true, force: true });
});

function assertCode(code: string, run: () => unknown): EntryconfError {
  let thrown: unknown;
  try {
    run();
  } catch (err) {
    thrown = err;
  }
  assert.ok(
    thrown instanceof EntryconfError,
    `expected an EntryconfError, got: ${String(thrown)}`,
  );
  assert.strictEqual(thrown.code, code, thrown.message);
  return thrown;
}

test("a config directory that does not exist is E_NO_ENTRYPOINT", () => {
  // SPEC §3: "A config directory that does not exist or cannot be read is
  // E_NO_ENTRYPOINT." Unfixturable: the case would need a config/ directory
  // that is absent, which git cannot store.
  assertCode("E_NO_ENTRYPOINT", () => load(join(scratch, "no-such-directory")));
});

test("an empty config directory is E_NO_ENTRYPOINT", () => {
  // Also unfixturable — git stores no empty directory.
  const empty = mkdtempSync(join(scratch, "empty-"));
  assertCode("E_NO_ENTRYPOINT", () => load(empty));
});

test("a config path that is a file, not a directory, is E_NO_ENTRYPOINT", () => {
  const file = join(scratch, "not-a-directory.json");
  writeFileSync(file, "{}\n");
  assertCode("E_NO_ENTRYPOINT", () => load(file));
});

test("the alias-expansion budget is the value SPEC §2 fixes", () => {
  assert.strictEqual(MAX_EXPANDED_NODES, 1_000_000);
});

test("a duplicate key is E_PARSE before any alias expansion is charged", () => {
  // Cases 14/35 fixture the plain duplicate-key rule; what they cannot show is
  // the *ordering*: duplicate detection walks the AST, so a document that is
  // both duplicate-keyed and an alias bomb is rejected on the key rather than
  // after (or instead of) a million nodes of expansion.
  const dir = mkdtempSync(join(scratch, "dup-bomb-"));
  const rows = Array.from({ length: 400 }, () => "*row").join(", ");
  writeFileSync(
    join(dir, "entrypoint.yaml"),
    `row: &row [${Array.from({ length: 2000 }, (_, i) => i).join(", ")}]\n` +
      `bomb: [${rows}]\n` +
      "app: one\napp: two\n",
  );
  const err = assertCode("E_PARSE", () => load(dir));
  // The code is what the spec fixes; the detail is checked only because it is
  // the one observable that says *which* rule fired.
  assert.match(err.message, /duplicate key/);
});

test("a large alias-free mapping loads in time linear in its size", () => {
  // Regression guard for the duplicate-key check: comparing each key against
  // every key already in the mapping is quadratic, and a mapping this size
  // took minutes. Linear detection puts it around a second, so the bound below
  // is generous for the fix and unreachable without it.
  const dir = mkdtempSync(join(scratch, "wide-map-"));
  const entries = Array.from(
    { length: 100_000 },
    (_, i) => `  k${i}: ${i}\n`,
  ).join("");
  writeFileSync(join(dir, "entrypoint.yaml"), `big:\n${entries}`);
  const started = process.hrtime.bigint();
  const tree = load(dir) as { big: Record<string, number> };
  const elapsedMs = Number(process.hrtime.bigint() - started) / 1e6;
  assert.strictEqual(Object.keys(tree.big).length, 100_000);
  assert.ok(elapsedMs < 15_000, `took ${elapsedMs.toFixed(0)}ms`);
});

// --- dump CLI convention ---------------------------------------------------

interface Run {
  status: number;
  stdout: string;
  stderr: string;
}

function runCli(args: string[]): Run {
  try {
    const stdout = execFileSync("node", [cli, ...args], {
      encoding: "utf8",
      stdio: ["ignore", "pipe", "pipe"],
    });
    return { status: 0, stdout, stderr: "" };
  } catch (err) {
    const failure = err as {
      status: number | null;
      stdout: string;
      stderr: string;
    };
    assert.ok(
      failure.status !== null,
      `cli did not exit normally: ${String(err)}`,
    );
    return {
      status: failure.status,
      stdout: failure.stdout,
      stderr: failure.stderr,
    };
  }
}

const CODE_RE = /E_[A-Z][A-Z0-9_]*/;

test("dump CLI: a successful load exits 0 with the tree on stdout", () => {
  const run = runCli([join(casesDir, "01-basic", "config")]);
  assert.strictEqual(run.status, 0);
  assert.ok(typeof JSON.parse(run.stdout) === "object");
});

test("dump CLI: a load failure prints the bare code and exits 1", () => {
  const run = runCli([join(casesDir, "07-include-cycle", "config")]);
  assert.strictEqual(run.status, 1);
  assert.strictEqual(run.stderr.split("\n")[0], "E_INCLUDE_CYCLE");
});

test("dump CLI: the alias bomb is E_PARSE, exit 1, and fast", () => {
  // The budget must be a counted budget, not a timeout: case 57 expands to
  // ~48M nodes and has to fail after a million of them.
  const started = process.hrtime.bigint();
  const run = runCli([join(casesDir, "57-yaml-alias-bomb", "config")]);
  const elapsedMs = Number(process.hrtime.bigint() - started) / 1e6;
  assert.strictEqual(run.status, 1);
  assert.strictEqual(run.stderr.split("\n")[0], "E_PARSE");
  // Generous next to a real budget (milliseconds) and still far under the
  // seconds an unbounded expansion would take.
  assert.ok(elapsedMs < 5000, `took ${elapsedMs.toFixed(0)}ms`);
});

test("dump CLI: usage errors exit 2 and print no E_* code", () => {
  const argSets = [
    [],
    [".", "extra-argument"],
    // A dash-led first argument names no directory: it is a usage fault, not a
    // load failure, so it must never reach load() and report E_NO_ENTRYPOINT.
    ["--version"],
    ["--"],
    ["-anything"],
    ["-E_PARSE"],
  ];
  for (const args of argSets) {
    const run = runCli(args);
    assert.strictEqual(run.status, 2, `args: ${JSON.stringify(args)}`);
    assert.doesNotMatch(run.stderr, CODE_RE, `args: ${JSON.stringify(args)}`);
    assert.doesNotMatch(run.stdout, CODE_RE, `args: ${JSON.stringify(args)}`);
  }
});

test("dump CLI: --help and -h print usage on stdout and exit 0", () => {
  for (const flag of ["--help", "-h"]) {
    const run = runCli([flag]);
    assert.strictEqual(run.status, 0, `flag: ${flag}`);
    assert.match(run.stdout, /usage:/);
    assert.doesNotMatch(run.stdout, CODE_RE);
    assert.doesNotMatch(run.stderr, CODE_RE);
  }
});

test("dump CLI: an internal fault exits 2 and prints no E_* code", () => {
  // Force a non-EntryconfError out of load() by making the tree unserializable
  // for JSON.stringify (a BigInt), which is exactly the shape of fault the
  // convention reserves exit 2 for.
  const script = `
    import { load } from ${JSON.stringify(cli.replace(/cli\.ts$/, "index.ts"))};
    globalThis.JSON.stringify = () => { throw new TypeError("boom E_PARSE"); };
    await import(${JSON.stringify(cli)});
  `;
  const file = join(scratch, "internal-fault.mjs");
  writeFileSync(file, script);
  let status: number | null = 0;
  let stderr = "";
  try {
    execFileSync("node", [file, join(casesDir, "01-basic", "config")], {
      encoding: "utf8",
      stdio: ["ignore", "pipe", "pipe"],
    });
  } catch (err) {
    const failure = err as { status: number | null; stderr: string };
    status = failure.status;
    stderr = failure.stderr;
  }
  assert.strictEqual(status, 2, stderr);
  assert.doesNotMatch(stderr, CODE_RE);
  assert.match(stderr, /internal error:/);
});
