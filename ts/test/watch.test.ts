/**
 * The watching conformance harness (SPEC §13) and the unit tests for `watch`'s
 * rules (SPEC §12.3), against real directories.
 */
import assert from "node:assert/strict";
import { cpSync, existsSync, mkdirSync, mkdtempSync, readdirSync, readFileSync, renameSync, rmSync, unlinkSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { after, test } from "node:test";
import { fileURLToPath } from "node:url";

import { openWith } from "../src/edit.ts";
import { EntryconfError } from "../src/index.ts";
import { refreshedJSON, refreshWith } from "../src/refresh.ts";
import { platform, watch, type Change } from "../src/watch.ts";

const here = dirname(fileURLToPath(import.meta.url));
const watchCasesDir = resolve(here, "..", "..", "testdata", "watchcases");

const scratch = mkdtempSync(join(tmpdir(), "entryconf-ts-watch-"));
after(() => {
  rmSync(scratch, { recursive: true, force: true });
});
let serial = 0;
function freshDir(): string {
  const dir = join(scratch, `w${serial++}`);
  mkdirSync(dir, { recursive: true });
  return dir;
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

for (const name of readdirSync(watchCasesDir).sort()) {
  const caseDir = join(watchCasesDir, name);
  test(`watchcase ${name}`, () => {
    const work = freshDir();
    const config = join(work, "config");
    cpSync(join(caseDir, "config"), config, { recursive: true });
    if (existsSync(join(caseDir, "outside"))) cpSync(join(caseDir, "outside"), work, { recursive: true });
    const procenvPath = join(caseDir, "procenv.json");
    const env: Record<string, string> = existsSync(procenvPath) ? JSON.parse(readFileSync(procenvPath, "utf8")) : {};
    const live = (n: string): string | undefined => (Object.hasOwn(env, n) ? env[n] : undefined);
    const snap = openWith(config, { ...env }, live);
    const watchSet = snap.watchSet();

    const change = JSON.parse(readFileSync(join(caseDir, "change.json"), "utf8")) as {
      files?: Record<string, string | null>;
      procenv?: Record<string, string | null>;
    };
    for (const [key, text] of Object.entries(change.files ?? {})) {
      const path = join(config, key);
      if (text === null) unlinkSync(path);
      else {
        mkdirSync(dirname(path), { recursive: true });
        writeFileSync(path, text);
      }
    }
    for (const [k, v] of Object.entries(change.procenv ?? {})) {
      if (v === null) delete env[k];
      else env[k] = v;
    }

    const r = refreshWith(snap, { ...env }, live);
    if (r.error) assert.equal(r.snapshot, undefined, "a failed refresh carries no snapshot");
    else if (!r.stale) assert.equal(r.snapshot, snap, "a refresh that is not stale returns the old snapshot");
    const got = JSON.parse(JSON.stringify({ watch: watchSet, ...refreshedJSON(r) }));
    const expected = JSON.parse(readFileSync(join(caseDir, "expected.json"), "utf8"));
    const found = mismatch(got, expected, "$");
    assert.equal(found, null, found ?? "");
  });
}

// -- watch (SPEC §12.3) ------------------------------------------------------

const SETTLE = 50;
const QUIET = 400;
const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

function put(path: string, text: string): void {
  mkdirSync(dirname(path), { recursive: true });
  writeFileSync(path, text);
}

/** A watcher on a fresh directory; changes queue up for `next` and `none`. */
function start(files: Record<string, string>, answer?: (c: Change) => void | Promise<void>) {
  const dir = freshDir();
  for (const [name, text] of Object.entries(files)) put(join(dir, name), text);
  const queue: Change[] = [];
  const waiters: ((c: Change) => void)[] = [];
  const w = watch(
    dir,
    async (c) => {
      const waiter = waiters.shift();
      if (waiter) waiter(c);
      else queue.push(c);
      if (answer) await answer(c);
    },
    { settleMs: SETTLE },
  );
  const next = (): Promise<Change> =>
    new Promise((res, rej) => {
      const c = queue.shift();
      if (c) return res(c);
      const t = setTimeout(() => rej(new Error("no change delivered")), 5000);
      waiters.push((c) => {
        clearTimeout(t);
        res(c);
      });
    });
  const none = async (): Promise<void> => {
    await sleep(QUIET);
    assert.equal(queue.length, 0, `unexpected change: ${JSON.stringify(queue[0]?.changed)}`);
  };
  return { dir, w, next, none };
}

const tree = (c: Change) => c.new!.tree as Record<string, any>;

test("watch: a burst is one change", async () => {
  const { dir, w, next, none } = start({ "entrypoint.json": '{"n": 0}' });
  try {
    for (let i = 1; i <= 5; i++) {
      put(join(dir, "entrypoint.json"), `{"n": ${i}}`);
      await sleep(SETTLE / 5);
    }
    const c = await next();
    assert.equal(tree(c).n, 5);
    assert.deepEqual(c.changed, ["/n"]);
    await none();
    assert.equal(w.current, c.new);
  } finally {
    await w.close();
  }
});

test("watch: write to a temporary file and rename is one change", async () => {
  const { dir, w, next, none } = start({ "entrypoint.json": '{"db": "@file:db.json"}', "db.json": '{"port": 1}' });
  try {
    put(join(dir, ".db.json.swp"), '{"port": 2}');
    renameSync(join(dir, ".db.json.swp"), join(dir, "db.json"));
    assert.deepEqual((await next()).changed, ["/db/port"]);
    await none();
    put(join(dir, "db.json"), '{"port": 3}');
    assert.equal(tree(await next()).db.port, 3);
  } finally {
    await w.close();
  }
});

test("watch: nothing stale calls nothing", async () => {
  const { dir, w, none } = start({ "entrypoint.json": '{"n": 1}' });
  try {
    put(join(dir, "entrypoint.json"), '{"n": 1}');
    put(join(dir, "notes.txt"), "not config");
    await none();
  } finally {
    await w.close();
  }
});

test("watch: a refused change keeps the old snapshot", async () => {
  const { dir, w, next } = start({ "entrypoint.json": '{"port": 80}' }, (c) => {
    if (c.new && (c.new.tree as any).port === 0) throw new Error("port out of range");
  });
  try {
    const first = w.current;
    put(join(dir, "entrypoint.json"), '{"port": 0}');
    assert.equal((await next()).old, first);
    await sleep(10);
    assert.equal(w.current, first);
    put(join(dir, "entrypoint.json"), '{"port": 81}');
    const c = await next();
    assert.equal(c.old, first);
    assert.deepEqual(c.changed, ["/port"]);
    await sleep(10);
    assert.equal(w.current, c.new);
  } finally {
    await w.close();
  }
});

test("watch: a refused new include is watched", async () => {
  const { dir, w, next } = start({ "entrypoint.json": '{"port": 80}' }, (c) => {
    if (c.new && (c.new.tree as any).db?.port === 0) throw new Error("refused");
  });
  try {
    put(join(dir, "sub", "db.json"), '{"port": 0}');
    put(join(dir, "entrypoint.json"), '{"port": 80, "db": "@file:sub/db.json"}');
    await next();
    put(join(dir, "sub", "db.json"), '{"port": 5432}');
    const c = await next();
    assert.equal(c.error, undefined);
    assert.equal(tree(c).db.port, 5432);
  } finally {
    await w.close();
  }
});

test("watch: callbacks never overlap; a change during one comes after it", async () => {
  let running = false;
  let overlapped = false;
  let release!: () => void;
  const released = new Promise<void>((r) => (release = r));
  const { dir, w, next, none } = start({ "entrypoint.json": '{"n": 0}' }, async (c) => {
    if (running) overlapped = true;
    running = true;
    try {
      if ((c.new?.tree as any)?.n === 1) await released;
    } finally {
      running = false;
    }
  });
  try {
    put(join(dir, "entrypoint.json"), '{"n": 1}');
    const first = await next();
    put(join(dir, "entrypoint.json"), '{"n": 2}');
    await sleep(QUIET);
    put(join(dir, "entrypoint.json"), '{"n": 3}');
    await none();
    release();
    const second = await next();
    assert.equal(second.old, first.new);
    assert.equal(tree(second).n, 3);
    await none();
    assert.equal(overlapped, false);
  } finally {
    await w.close();
  }
});

test("watch: a failed open keeps the old snapshot; creating the include is seen", async () => {
  const { dir, w, next } = start({ "entrypoint.json": '{"n": 1}' });
  try {
    const first = w.current;
    put(join(dir, "entrypoint.json"), '{"n": 1, "db": "@file:conf/db.json"}');
    const failed = await next();
    assert.equal(failed.error?.code, "E_INCLUDE");
    assert.equal(failed.new, undefined);
    assert.equal(w.current, first);
    put(join(dir, "conf", "db.json"), '{"port": 5432}');
    const c = await next();
    assert.equal(c.error, undefined);
    assert.equal(c.old, first);
    assert.equal(tree(c).db.port, 5432);
  } finally {
    await w.close();
  }
});

test("watch: a new env file and a second entrypoint", async () => {
  const { dir, w, next } = start({ "entrypoint.json": '{"host": "${EC_WT_HOST:-a}"}' });
  try {
    put(join(dir, "host.env"), "EC_WT_HOST=b\n");
    const c = await next();
    assert.equal(tree(c).host, "b");
    assert.deepEqual(c.documents.added, ["host.env"]);
    put(join(dir, "entrypoint.yaml"), "host: c\n");
    assert.equal((await next()).error?.code, "E_MULTIPLE_ENTRYPOINTS");
  } finally {
    await w.close();
  }
});

test("watch: close waits for a running callback, and none starts after", async () => {
  let entered!: () => void;
  const isEntered = new Promise<void>((r) => (entered = r));
  let finished = false;
  let closed = false;
  let late = false;
  const { dir, w } = start({ "entrypoint.json": '{"n": 0}' }, async (c) => {
    if (closed) late = true;
    if ((c.new?.tree as any)?.n === 1) {
      entered();
      await sleep(QUIET);
      finished = true;
    }
  });
  put(join(dir, "entrypoint.json"), '{"n": 1}');
  await isEntered;
  put(join(dir, "entrypoint.json"), '{"n": 2}');
  await w.close();
  closed = true;
  assert.equal(finished, true, "close settled while a callback ran");
  put(join(dir, "entrypoint.json"), '{"n": 3}');
  await sleep(QUIET);
  assert.equal(late, false, "a callback started after close");
  await w.close();
});

test("watch: E_WATCH when the platform refuses a directory", async () => {
  const saved = platform.watch;
  const refused = new Set<string>();
  platform.watch = (dir, listener) => {
    if (refused.has(dir)) throw Object.assign(new Error("ENOSPC: inotify watch limit reached"), { code: "ENOSPC" });
    return saved(dir, listener);
  };
  try {
    const first = freshDir();
    put(join(first, "entrypoint.json"), '{"n": 1}');
    refused.add(first);
    assert.throws(() => watch(first, () => {}), (err: unknown) => err instanceof EntryconfError && err.code === "E_WATCH");

    const { dir, w, next } = start({ "entrypoint.json": '{"n": 1}', "sub/x.json": "{}" });
    try {
      refused.add(join(dir, "sub"));
      put(join(dir, "entrypoint.json"), '{"n": 1, "x": "@file:sub/x.json"}');
      const accepted = await next();
      assert.equal(accepted.error, undefined);
      const c = await next();
      assert.equal(c.error?.code, "E_WATCH");
      assert.equal(c.old, accepted.new);
      assert.equal(w.current, accepted.new);
    } finally {
      await w.close();
    }
  } finally {
    platform.watch = saved;
  }
});
