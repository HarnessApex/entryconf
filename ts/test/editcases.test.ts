/**
 * The editing conformance harness (SPEC §11) plus the unit tests for what a
 * fixture cannot express (SPEC §10.9: lock contention, filesystem failure,
 * permissions, symlinks, the live environment).
 *
 * Every case runs against a fresh copy of its config/ directory; afterwards
 * every file the case did not expect to be written must be byte-identical to
 * the copy and no file may have been added.
 */
import assert from "node:assert/strict";
import {
  chmodSync,
  cpSync,
  existsSync,
  lstatSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  readdirSync,
  rmSync,
  statSync,
  symlinkSync,
  utimesSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, relative, resolve, sep } from "node:path";
import { after, test } from "node:test";
import { fileURLToPath } from "node:url";

import { openWith, revisionOf, type Edit } from "../src/edit.ts";
import { EntryconfError, load, open, type Value } from "../src/index.ts";
import { formatJSON } from "../src/jsonfmt.ts";
import { loadWith } from "../src/loader.ts";
import { LOCK_FILE_NAME, LOCK_STALE_AFTER_MS } from "../src/lock.ts";

const here = dirname(fileURLToPath(import.meta.url));
const editCasesDir = resolve(here, "..", "..", "testdata", "editcases");

const scratch = mkdtempSync(join(tmpdir(), "entryconf-ts-edit-"));
after(() => {
  rmSync(scratch, { recursive: true, force: true });
});

function readIfPresent(path: string): string | null {
  try {
    return readFileSync(path, "utf8");
  } catch {
    return null;
  }
}

function fileBytes(root: string): Map<string, Buffer> {
  const out = new Map<string, Buffer>();
  const walk = (dir: string): void => {
    for (const entry of readdirSync(dir, { withFileTypes: true })) {
      const path = join(dir, entry.name);
      if (entry.isDirectory()) walk(path);
      else out.set(relative(root, path).split(sep).join("/"), readFileSync(path));
    }
  };
  walk(root);
  return out;
}

function isNumber(v: unknown): v is number {
  return typeof v === "number";
}

/** Structural comparison; numbers numerically. Returns the first mismatch. */
function mismatch(actual: unknown, expected: unknown, path: string): string | null {
  if (expected === null || typeof expected === "boolean" || typeof expected === "string") {
    return actual === expected ? null : `${path}: expected ${JSON.stringify(expected)}, got ${JSON.stringify(actual)}`;
  }
  if (isNumber(expected)) {
    return isNumber(actual) && actual === expected ? null : `${path}: expected ${expected}, got ${JSON.stringify(actual)}`;
  }
  if (Array.isArray(expected)) {
    if (!Array.isArray(actual)) return `${path}: expected an array, got ${JSON.stringify(actual)}`;
    if (actual.length !== expected.length) return `${path}: expected ${expected.length} items, got ${actual.length}`;
    for (let i = 0; i < expected.length; i++) {
      const found = mismatch(actual[i], expected[i], `${path}[${i}]`);
      if (found) return found;
    }
    return null;
  }
  if (typeof actual !== "object" || actual === null || Array.isArray(actual)) {
    return `${path}: expected an object, got ${JSON.stringify(actual)}`;
  }
  const a = actual as Record<string, unknown>;
  const e = expected as Record<string, unknown>;
  const ak = Object.keys(a).sort();
  const ek = Object.keys(e).sort();
  if (JSON.stringify(ak) !== JSON.stringify(ek)) {
    return `${path}: key mismatch (expected ${JSON.stringify(ek)}, got ${JSON.stringify(ak)})`;
  }
  for (const k of ek) {
    const found = mismatch(a[k], e[k], `${path}.${k}`);
    if (found) return found;
  }
  return null;
}

/** Round-trip through JSON so class instances compare as their wire form. */
function wire(v: unknown): unknown {
  return JSON.parse(JSON.stringify(v));
}

function runEditCase(caseDir: string): void {
  const work = mkdtempSync(join(scratch, "case-"));
  cpSync(join(caseDir, "config"), work, { recursive: true });
  const original = fileBytes(work);

  const procenvText = readIfPresent(join(caseDir, "procenv.json"));
  const env: Record<string, string> = procenvText ? JSON.parse(procenvText) : {};
  const captured = { ...env };
  const live = (name: string): string | undefined => (Object.hasOwn(env, name) ? env[name] : undefined);

  const req = JSON.parse(readFileSync(join(caseDir, "request.json"), "utf8")) as Record<string, unknown>;
  const wantErr = (readIfPresent(join(caseDir, "expected_error.txt")) ?? "").trim();
  const expectedText = readIfPresent(join(caseDir, "expected.json"));
  const expected = expectedText ? JSON.parse(expectedText) : null;

  const expectWritten = new Set<string>();
  let got: unknown;
  let opErr: unknown = null;

  try {
    const snap = openWith(work, captured, live);
    if (typeof req.inspect === "string") {
      got = { origin: wire(snap.inspect(req.inspect)) };
    } else if (req.documents === true) {
      const docs: Record<string, unknown> = {};
      for (const [key, d] of Object.entries(snap.documents)) {
        docs[key] = { format: d.format, writable: d.writable, grafts: wire(d.grafts) };
      }
      got = { documents: docs };
    } else {
      const plan = snap.plan(req.edits as Edit[]);
      const bc = req.before_commit as { files?: Record<string, string>; procenv?: Record<string, string> } | undefined;
      if (bc?.files) {
        for (const [key, text] of Object.entries(bc.files)) {
          writeFileSync(join(work, ...key.split("/")), text);
          original.set(key, Buffer.from(text));
        }
      }
      if (bc?.procenv) Object.assign(env, bc.procenv);
      const receipt = plan.commit();
      expectWritten.add(plan.document);
      assert.equal(receipt.document, plan.document);
      const reloaded = loadWith(work, live);
      assert.equal(mismatch(reloaded, plan.candidate, "$"), null, "committed tree differs from the plan's candidate");
      const onDisk = readFileSync(join(work, ...plan.document.split("/")));
      assert.ok(onDisk.equals(Buffer.from(plan.after)), "file on disk is not plan.after");
      assert.equal(receipt.revision, revisionOf(onDisk));
      got = {
        tree: reloaded,
        affected: plan.affected,
        documents: { [plan.document]: JSON.parse(plan.after) },
      };
    }
  } catch (err) {
    opErr = err;
  }

  if (wantErr !== "") {
    assert.ok(opErr !== null, `expected ${wantErr}, got success: ${JSON.stringify(got)}`);
    assert.ok(opErr instanceof EntryconfError, `expected an EntryconfError, got ${String(opErr)}`);
    assert.equal(opErr.code, wantErr, opErr.message);
  } else {
    if (opErr !== null) throw opErr;
    const found = mismatch(got, expected, "$");
    assert.equal(found, null, found ?? "");
  }

  // Filesystem discipline (SPEC §11).
  const afterFiles = fileBytes(work);
  for (const [rel, data] of afterFiles) {
    const orig = original.get(rel);
    assert.ok(orig !== undefined, `file ${rel} was created (lock or temporary file left behind?)`);
    if (!expectWritten.has(rel)) assert.ok(orig.equals(data), `file ${rel} changed but was not the edited document`);
  }
  for (const rel of original.keys()) assert.ok(afterFiles.has(rel), `file ${rel} disappeared`);
}

test("editing conformance suite", async (t) => {
  const cases = readdirSync(editCasesDir)
    .filter((name) => statSync(join(editCasesDir, name)).isDirectory())
    .sort();
  assert.ok(cases.length > 0, `no cases found in ${editCasesDir}`);
  for (const name of cases) {
    await t.test(name, () => runEditCase(join(editCasesDir, name)));
  }
});

// --- what fixtures cannot express (SPEC §10.9) -----------------------------

function scratchConfig(entrypoint: string): string {
  const dir = mkdtempSync(join(scratch, "unit-"));
  writeFileSync(join(dir, "entrypoint.json"), entrypoint);
  return dir;
}

function assertCode(code: string, run: () => unknown): EntryconfError {
  let thrown: unknown;
  try {
    run();
  } catch (err) {
    thrown = err;
  }
  assert.ok(thrown instanceof EntryconfError, `expected an EntryconfError, got: ${String(thrown)}`);
  assert.equal(thrown.code, code, thrown.message);
  return thrown;
}

const edit = (document: string, pointer: string, value: unknown): Edit => ({ document, op: "set", pointer, value });

test("two cooperating writers: the second commit is E_STALE_PLAN", () => {
  const dir = scratchConfig('{"a": 1, "b": 2}');
  const snap = open(dir);
  const p1 = snap.plan([edit("entrypoint.json", "/a", 10)]);
  const p2 = snap.plan([edit("entrypoint.json", "/b", 20)]);
  p1.commit();
  assertCode("E_STALE_PLAN", () => p2.commit());
  // The same plan twice is also stale: its own write moved the revision.
  assertCode("E_STALE_PLAN", () => p1.commit());
  const tree = load(dir) as { a: number; b: number };
  assert.equal(tree.a, 10);
  assert.equal(tree.b, 2);
});

test("a held lock is E_LOCKED within the timeout; a stale lock is broken", () => {
  const dir = scratchConfig('{"a": 1}');
  const plan = open(dir).plan([edit("entrypoint.json", "/a", 2)]);
  const lock = join(dir, LOCK_FILE_NAME);
  writeFileSync(lock, "{}");
  const started = Date.now();
  assertCode("E_LOCKED", () => plan.commit({ lockTimeoutMs: 150 }));
  assert.ok(Date.now() - started < 2000, "lock wait did not respect the timeout");
  assert.equal(readFileSync(join(dir, "entrypoint.json"), "utf8"), '{"a": 1}', "source changed under a held lock");
  assert.ok(existsSync(lock), "another writer's lock was removed");

  const old = new Date(Date.now() - 2 * LOCK_STALE_AFTER_MS);
  utimesSync(lock, old, old);
  plan.commit({ lockTimeoutMs: 1000 });
  assert.ok(!existsSync(lock), "lock file left behind after commit");
});

test("commit preserves the file's permission bits", { skip: process.platform === "win32" }, () => {
  const dir = scratchConfig('{"a": 1}');
  const target = join(dir, "entrypoint.json");
  chmodSync(target, 0o600);
  open(dir).plan([edit("entrypoint.json", "/a", 2)]).commit();
  assert.equal(statSync(target).mode & 0o777, 0o600);
});

test(
  "a failed write is E_WRITE with the source intact and no temp or lock file",
  { skip: process.platform === "win32" || process.getuid?.() === 0 },
  () => {
    const dir = scratchConfig('{"x": "@file:sub/x.json"}');
    const sub = join(dir, "sub");
    mkdirSync(sub);
    writeFileSync(join(sub, "x.json"), '{"v": 1}');
    const plan = open(dir).plan([edit("sub/x.json", "/v", 2)]);
    chmodSync(sub, 0o555);
    try {
      assertCode("E_WRITE", () => plan.commit());
      assert.deepEqual(readdirSync(sub), ["x.json"], "temporary file left behind");
      assert.equal(readFileSync(join(sub, "x.json"), "utf8"), '{"v": 1}');
      assert.ok(!existsSync(join(dir, LOCK_FILE_NAME)), "lock file left behind after a failed commit");
    } finally {
      chmodSync(sub, 0o755);
    }
  },
);

test("committing through a symlink replaces the target and keeps the link", { skip: process.platform === "win32" }, () => {
  const dir = mkdtempSync(join(scratch, "link-"));
  const realDir = mkdtempSync(join(scratch, "real-"));
  const real = join(realDir, "real.json");
  writeFileSync(real, '{"v": 1}');
  writeFileSync(join(dir, "entrypoint.json"), '{"x": "@file:link.json"}');
  symlinkSync(real, join(dir, "link.json"));
  open(dir).plan([edit("link.json", "/v", 2)]).commit();
  assert.ok(lstatSync(join(dir, "link.json")).isSymbolicLink(), "the symlink was replaced by a regular file");
  assert.match(readFileSync(real, "utf8"), /"v": 2/);
});

test("plan does not touch disk", () => {
  const dir = scratchConfig('{"a": 1}');
  const before = fileBytes(dir);
  open(dir).plan([edit("entrypoint.json", "/a", 2)]);
  const afterFiles = fileBytes(dir);
  assert.equal(afterFiles.size, before.size);
  assert.ok(before.get("entrypoint.json")!.equals(afterFiles.get("entrypoint.json")!));
});

test("open captures the real environment; a live change makes the plan stale", () => {
  const dir = scratchConfig('{"host": "${EC_EDIT_HOST}"}');
  const saved = process.env.EC_EDIT_HOST;
  process.env.EC_EDIT_HOST = "prod";
  try {
    const snap = open(dir);
    assert.equal((snap.tree as { host: string }).host, "prod");
    const origin = snap.inspect("/host");
    assert.deepEqual(origin.variables, [{ name: "EC_EDIT_HOST", origin: "process" }]);
    const plan = snap.plan([edit("entrypoint.json", "/x", 1)]);
    process.env.EC_EDIT_HOST = "other";
    assertCode("E_STALE_PLAN", () => plan.commit());
  } finally {
    if (saved === undefined) delete process.env.EC_EDIT_HOST;
    else process.env.EC_EDIT_HOST = saved;
  }
});

test("the normalized JSON form follows SPEC §10.8 byte for byte", () => {
  const v: Value = {
    b: [1, 2, 1.5, "x"],
    a: {},
    c: [],
    s: 'q"\\\n\té',
    big: 2 ** 53 - 1,
    "é": 1,
    Z: 1,
  };
  const want =
    "{\n" +
    '  "Z": 1,\n' +
    '  "a": {},\n' +
    '  "b": [\n    1,\n    2,\n    1.5,\n    "x"\n  ],\n' +
    '  "big": 9007199254740991,\n' +
    '  "c": [],\n' +
    '  "s": "q\\"\\\\\\n\\t\\u0001é",\n' +
    '  "é": 1\n' +
    "}\n";
  assert.equal(formatJSON(v), want);
});
