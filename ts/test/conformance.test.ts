/**
 * The conformance harness (SPEC §8, testdata/README.md).
 *
 * One test walks the shared fixture suite and runs every case as a subtest
 * named after its directory. There are deliberately no hand-written per-case
 * tests: the fixtures are the definition of conformance.
 */
import assert from "node:assert/strict";
import { readFileSync, readdirSync, statSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { test } from "node:test";
import { fileURLToPath } from "node:url";

import { EntryconfError, load, type Value } from "../src/index.ts";

const here = dirname(fileURLToPath(import.meta.url));
const casesDir = resolve(here, "..", "..", "testdata", "cases");

const VAR_RE = /\$\{?([A-Za-z_][A-Za-z0-9_]*)/g;
const ENV_LINE_RE = /^\s*([A-Za-z_][A-Za-z0-9_]*)\s*=/;

function readIfPresent(path: string): string | null {
  try {
    return readFileSync(path, "utf8");
  } catch {
    return null;
  }
}

function walkFiles(dir: string, out: string[] = []): string[] {
  for (const entry of readdirSync(dir, { withFileTypes: true })) {
    const path = join(dir, entry.name);
    if (entry.isDirectory()) walkFiles(path, out);
    else if (entry.isFile()) out.push(path);
  }
  return out;
}

/**
 * Every variable name a case's files mention. The harness contract requires
 * these to be unset unless the case's procenv.json sets them, so the real
 * environment can never leak into a case.
 */
function variableNames(configDir: string): Set<string> {
  const names = new Set<string>();
  for (const file of walkFiles(configDir)) {
    const text = readIfPresent(file);
    if (text === null) continue;
    for (const match of text.matchAll(VAR_RE)) names.add(match[1]);
    if (file.endsWith(".env")) {
      for (const line of text.split("\n")) {
        const match = ENV_LINE_RE.exec(line);
        if (match) names.add(match[1]);
      }
    }
  }
  return names;
}

/** Structural equality; numbers compare numerically (8080 equals 8080.0). */
function assertSameTree(actual: Value, expected: Value, path: string): void {
  if (expected === null || typeof expected !== "object") {
    assert.strictEqual(actual, expected, `at ${path}`);
    return;
  }
  if (Array.isArray(expected)) {
    assert.ok(Array.isArray(actual), `at ${path}: expected an array`);
    const items = actual as Value[];
    assert.strictEqual(items.length, expected.length, `at ${path}: length`);
    expected.forEach((item, i) => {
      assertSameTree(items[i], item, `${path}[${i}]`);
    });
    return;
  }
  assert.ok(
    typeof actual === "object" && actual !== null && !Array.isArray(actual),
    `at ${path}: expected an object`,
  );
  const record = actual as { [key: string]: Value };
  assert.deepStrictEqual(
    Object.keys(record).sort(),
    Object.keys(expected).sort(),
    `at ${path}: keys`,
  );
  for (const [key, item] of Object.entries(expected)) {
    assertSameTree(record[key], item, `${path}.${key}`);
  }
}

function runCase(caseDir: string): void {
  const configDir = join(caseDir, "config");
  const procenvText = readIfPresent(join(caseDir, "procenv.json"));
  const procenv: Record<string, string> = procenvText
    ? JSON.parse(procenvText)
    : {};
  const expectedText = readIfPresent(join(caseDir, "expected.json"));
  const expectedError = readIfPresent(join(caseDir, "expected_error.txt"));
  assert.ok(
    (expectedText === null) !== (expectedError === null),
    "case must have exactly one of expected.json / expected_error.txt",
  );

  const names = variableNames(configDir);
  for (const name of Object.keys(procenv)) names.add(name);
  const saved = new Map<string, string | undefined>();
  for (const name of names) {
    saved.set(name, process.env[name]);
    delete process.env[name];
  }
  for (const [name, value] of Object.entries(procenv)) {
    process.env[name] = value;
  }

  try {
    if (expectedError !== null) {
      const code = expectedError.trim();
      let thrown: unknown;
      let threw = false;
      try {
        load(configDir);
      } catch (err) {
        thrown = err;
        threw = true;
      }
      assert.ok(threw, `expected load() to fail with ${code}`);
      assert.ok(
        thrown instanceof EntryconfError,
        `expected an EntryconfError, got: ${String(thrown)}`,
      );
      assert.strictEqual(thrown.code, code, `error code (${thrown.message})`);
    } else {
      assertSameTree(load(configDir), JSON.parse(expectedText!), "$");
    }
  } finally {
    for (const [name, value] of saved) {
      if (value === undefined) delete process.env[name];
      else process.env[name] = value;
    }
  }
}

test("conformance suite", async (t) => {
  const cases = readdirSync(casesDir)
    .filter((name) => statSync(join(casesDir, name)).isDirectory())
    .sort();
  assert.ok(cases.length > 0, `no cases found in ${casesDir}`);

  // Cases mutate the process environment, so they must not interleave.
  for (const name of cases) {
    await t.test(name, () => runCase(join(casesDir, name)));
  }
});
