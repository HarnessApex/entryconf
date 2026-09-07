import { randomBytes } from "node:crypto";
import {
  closeSync,
  fchmodSync,
  fsyncSync,
  openSync,
  realpathSync,
  renameSync,
  statSync,
  unlinkSync,
  writeSync,
} from "node:fs";
import { basename, dirname, join } from "node:path";

import { EntryconfError } from "./errors.ts";

export const LOCK_FILE_NAME = ".entryconf.lock";

/** A lock file older than this is presumed abandoned by a crashed writer (SPEC §10.7). */
export const LOCK_STALE_AFTER_MS = 30_000;

/** How long `commit` waits for the lock when no timeout is given. */
export const DEFAULT_LOCK_TIMEOUT_MS = 5_000;

function sleep(ms: number): void {
  Atomics.wait(new Int32Array(new SharedArrayBuffer(4)), 0, 0, ms);
}

function randomSuffix(): string {
  return randomBytes(6).toString("hex");
}

/**
 * SPEC §10.7 lock protocol: exclusive-create `<dir>/.entryconf.lock`, retry
 * until the timeout, break a lock older than `LOCK_STALE_AFTER_MS` by renaming
 * it away first (so at most one waiter breaks a given stale lock). Returns the
 * release function.
 */
export function acquireLock(dir: string, timeoutMs: number): () => void {
  const path = join(dir, LOCK_FILE_NAME);
  const deadline = Date.now() + timeoutMs;
  let wait = 5;
  for (;;) {
    try {
      const fd = openSync(path, "wx", 0o644);
      try {
        writeSync(
          fd,
          `{"pid": ${process.pid}, "created": ${JSON.stringify(new Date().toISOString())}}\n`,
        );
      } finally {
        closeSync(fd);
      }
      return () => {
        try {
          unlinkSync(path);
        } catch {
          // already gone
        }
      };
    } catch (err) {
      if ((err as NodeJS.ErrnoException).code !== "EEXIST") {
        throw new EntryconfError(
          "E_LOCKED",
          `cannot create lock file ${path}: ${(err as Error).message}`,
        );
      }
    }
    let stale = false;
    try {
      stale = Date.now() - statSync(path).mtimeMs > LOCK_STALE_AFTER_MS;
    } catch {
      // vanished between attempts; retry
    }
    if (stale) {
      const aside = `${path}.${randomSuffix()}`;
      try {
        renameSync(path, aside);
        unlinkSync(aside);
      } catch {
        // another waiter broke it first
      }
      continue;
    }
    if (Date.now() >= deadline) {
      throw new EntryconfError(
        "E_LOCKED",
        `lock file ${path} is held by another writer (waited ${timeoutMs}ms)`,
      );
    }
    sleep(wait);
    if (wait < 100) wait *= 2;
  }
}

/**
 * Replace `target`'s content atomically (SPEC §10.7 step 3): a temporary file
 * beside the resolved target, flushed, given the target's permission bits, and
 * renamed over it. A symlinked target is resolved first so the link survives.
 * On any failure the temporary file is removed and the target is untouched.
 */
export function atomicWrite(target: string, data: Uint8Array): void {
  const fail = (step: string, err: unknown): EntryconfError =>
    new EntryconfError("E_WRITE", `${step} ${target}: ${(err as Error).message}`);

  let resolved: string;
  let mode: number;
  try {
    resolved = realpathSync(target);
    mode = statSync(resolved).mode & 0o7777;
  } catch (err) {
    throw fail("cannot resolve", err);
  }
  const dir = dirname(resolved);
  const tmp = join(dir, `.${basename(resolved)}.entryconf-tmp-${randomSuffix()}`);
  let fd: number | null = null;
  try {
    fd = openSync(tmp, "wx", 0o600);
  } catch (err) {
    throw fail("cannot create a temporary file beside", err);
  }
  try {
    writeSync(fd, data);
    fsyncSync(fd);
    if (process.platform !== "win32") fchmodSync(fd, mode);
    closeSync(fd);
    fd = null;
    renameSync(tmp, resolved);
  } catch (err) {
    if (fd !== null) {
      try {
        closeSync(fd);
      } catch {
        // ignore
      }
    }
    try {
      unlinkSync(tmp);
    } catch {
      // ignore
    }
    throw fail("cannot replace", err);
  }
  // Best effort: make the rename durable where the platform allows.
  try {
    const dfd = openSync(dir, "r");
    try {
      fsyncSync(dfd);
    } finally {
      closeSync(dfd);
    }
  } catch {
    // not supported here
  }
}
