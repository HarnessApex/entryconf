import { createHash } from "node:crypto";
import { readFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";

import { mergeEnvFiles, parseEnvFile, Vars, type EnvLookup } from "./env.ts";
import { EntryconfError } from "./errors.ts";
import { includeTarget, isInclude } from "./includes.ts";
import { compareCodePoints, formatJSON } from "./jsonfmt.ts";
import {
  acquireLock,
  atomicWrite,
  DEFAULT_LOCK_TIMEOUT_MS,
} from "./lock.ts";
import {
  loadTree,
  mapLookup,
  processEnvLookup,
  type Loader,
} from "./loader.ts";
import { decodeUtf8, parseDocument } from "./parse.ts";
import {
  arrayIndex,
  deepCopy,
  escapePointerToken,
  isRecord,
  parsePointer,
  removeAt,
  setAt,
} from "./pointer.ts";
import { scanReferences } from "./refs.ts";
import {
  documentKey,
  osSource,
  OverlaySource,
  Recorder,
  type DocRecord,
} from "./source.ts";
import { emptyObject, setKey, type Tree, type Value } from "./tree.ts";

/** One source file of a snapshot (SPEC §10.2). */
export interface Document {
  key: string;
  path: string;
  format: "json" | "yaml" | "toml" | "env";
  revision: string;
  /** True iff `format` is `"json"`. */
  writable: boolean;
  grafts: Graft[];
}

/**
 * One position the document's root value occupies in the effective tree, with
 * the `@file:` references traversed to reach it, outermost first (SPEC §10.3).
 */
export interface Graft {
  effective: string;
  chain: Reference[];
}

/** An `@file:` string: the document holding it and its pointer there. */
export interface Reference {
  document: string;
  pointer: string;
}

/** The provenance of one effective value (SPEC §10.3). */
export interface Origin {
  effective: string;
  document: string;
  pointer: string;
  authored: Value;
  chain: Reference[];
  variables: Variable[];
  writable: boolean;
}

/**
 * One `$` reference an authored string makes and which layer of SPEC §4
 * supplies it. `file` is present only when `origin` is `"file"`.
 */
export interface Variable {
  name: string;
  origin: "process" | "file" | "default";
  file?: string;
}

export type EditOp = "set" | "remove";
export type EditMode = "literal" | "expression";

/** One operation on one source document (SPEC §10.4). */
export interface Edit {
  /** Key of the document to edit. Required; there is no default. */
  document: string;
  op: EditOp;
  /** RFC 6901 pointer within `document`; `""` is the root. */
  pointer: string;
  /** For `set`: any JSON-equivalent value. Ignored for `remove`. */
  value?: unknown;
  /**
   * For `set`: `"literal"` (default — strings are escaped so they load back as
   * themselves) or `"expression"` (a string written verbatim as an entryconf
   * expression).
   */
  mode?: EditMode;
}

export interface Receipt {
  document: string;
  revision: string;
  revisions: Record<string, string>;
}

export interface CommitOptions {
  /** Bounds the wait for the directory lock; default 5000. */
  lockTimeoutMs?: number;
}

/** SPEC §10.2's revision: `sha256:` + hex SHA-256 of the bytes. */
export function revisionOf(data: Uint8Array | string): string {
  return `sha256:${createHash("sha256").update(data).digest("hex")}`;
}

function sortedRecord<T>(entries: Iterable<[string, T]>): Record<string, T> {
  const out: Record<string, T> = Object.create(null);
  for (const [k, v] of [...entries].sort(([a], [b]) => compareCodePoints(a, b))) {
    Object.defineProperty(out, k, {
      value: v,
      writable: true,
      enumerable: true,
      configurable: true,
    });
  }
  return out;
}

/**
 * What `open()` returns (SPEC §10.2): the effective tree, every source
 * document the load read, and the process environment as captured at open
 * time. Immutable; reads nothing from disk after `open` returns.
 */
export class Snapshot {
  /** Absolute path of the config directory. */
  readonly dir: string;
  /** The effective tree — exactly what `load(dir)` returns. */
  readonly tree: Tree;
  /** Every document the load read, by document key. */
  readonly documents: Record<string, Document>;

  readonly #byPath: Map<string, DocRecord>;
  readonly #envNames: string[];
  readonly #captured: Record<string, string>;
  readonly #live: EnvLookup;

  /** @internal — use `open()`. */
  constructor(
    dir: string,
    tree: Tree,
    documents: Record<string, Document>,
    byPath: Map<string, DocRecord>,
    envNames: string[],
    captured: Record<string, string>,
    live: EnvLookup,
  ) {
    this.dir = dir;
    this.tree = tree;
    this.documents = documents;
    this.#byPath = byPath;
    this.#envNames = envNames;
    this.#captured = captured;
    this.#live = live;
  }

  #entrypoint(): DocRecord {
    for (const name of ["entrypoint.json", "entrypoint.yaml", "entrypoint.yml", "entrypoint.toml"]) {
      const rec = this.#byPath.get(join(this.dir, name));
      if (rec && rec.parsed !== null) return rec;
    }
    throw new EntryconfError("E_NO_ENTRYPOINT", `snapshot of ${this.dir} has no entrypoint`);
  }

  /**
   * Where the effective value at `pointer` comes from (SPEC §10.3): the
   * document and source pointer that author it, the authored value, the
   * include chain that reaches it, and the variables it depends on. A pointer
   * that does not resolve in the effective tree is `E_PATH`.
   */
  inspect(pointer: string): Origin {
    const tokens = parsePointer(pointer);
    let doc = this.#entrypoint();
    let node: Value = doc.parsed as Value;
    let src = "";
    const chain: Reference[] = [];

    const follow = (): void => {
      while (typeof node === "string" && isInclude(node)) {
        const target = includeTarget(node, dirname(doc.path));
        const next = this.#byPath.get(target);
        if (!next) {
          throw new EntryconfError("E_PATH", `include ${target} was not loaded by this snapshot`);
        }
        chain.push({ document: documentKey(this.dir, doc.path), pointer: src });
        doc = next;
        node = next.parsed as Value;
        src = "";
      }
    };

    for (const tok of tokens) {
      follow();
      if (isRecord(node)) {
        if (!Object.hasOwn(node, tok)) {
          throw new EntryconfError("E_PATH", `effective pointer ${JSON.stringify(pointer)}: no member ${JSON.stringify(tok)}`);
        }
        node = node[tok];
      } else if (Array.isArray(node)) {
        const i = arrayIndex(tok);
        if (i === null || i >= node.length) {
          throw new EntryconfError("E_PATH", `effective pointer ${JSON.stringify(pointer)}: array index ${JSON.stringify(tok)} does not exist`);
        }
        node = node[i];
      } else {
        throw new EntryconfError("E_PATH", `effective pointer ${JSON.stringify(pointer)}: cannot descend into a scalar at ${JSON.stringify(tok)}`);
      }
      src += `/${escapePointerToken(tok)}`;
    }
    follow();

    const key = documentKey(this.dir, doc.path);
    return {
      effective: pointer,
      document: key,
      pointer: src,
      authored: deepCopy(node),
      chain,
      variables: typeof node === "string" ? this.#variablesOf(node) : [],
      writable: this.documents[key].writable,
    };
  }

  #vars(): Vars {
    const envFiles = this.#envNames.map((name) => {
      const path = join(this.dir, name);
      const rec = this.#byPath.get(path);
      const bytes = rec ? rec.data : osSource.readFile(path);
      return { path, text: decodeUtf8(bytes, path) };
    });
    const { files, origin } = mergeEnvFiles(envFiles);
    return new Vars(files, origin, mapLookup(this.#captured));
  }

  #variablesOf(authored: string): Variable[] {
    const vars = this.#vars();
    const out: Variable[] = [];
    for (const ref of scanReferences(authored)) {
      const where = vars.where(ref.name);
      if (where.origin === "process") out.push({ name: ref.name, origin: "process" });
      else if (where.origin === "file") {
        out.push({ name: ref.name, origin: "file", file: documentKey(this.dir, where.file) });
      } else if (ref.hasDefault) out.push({ name: ref.name, origin: "default" });
      // otherwise unreachable: the snapshot loaded, so the variable resolved
    }
    return out;
  }

  /** The overlay a candidate is evaluated against. */
  #source(overrides: Map<string, Uint8Array>): OverlaySource {
    const files = new Map<string, Uint8Array>();
    for (const [path, rec] of this.#byPath) files.set(path, rec.data);
    for (const [path, data] of overrides) files.set(path, data);
    return new OverlaySource(files, this.#envNames, osSource);
  }

  /**
   * Validate `edits` (SPEC §10.4), apply them to a copy of the selected
   * document (§10.5), and evaluate the candidate configuration with the new
   * text substituted in memory (§10.6). No file is changed. A candidate that
   * fails to load fails the plan with that load's code.
   */
  plan(edits: Edit[]): Plan {
    if (!Array.isArray(edits) || edits.length === 0) {
      throw new EntryconfError("E_EDIT", "a plan needs at least one edit");
    }
    const key = edits[0].document;
    for (const e of edits) {
      if (e.document !== key) {
        throw new EntryconfError(
          "E_UNSUPPORTED_EDIT",
          `edits name both ${JSON.stringify(key)} and ${JSON.stringify(e.document)}; a plan edits exactly one document`,
        );
      }
    }
    if (typeof key !== "string" || !Object.hasOwn(this.documents, key)) {
      throw new EntryconfError("E_EDIT", `no document ${JSON.stringify(key)} in the snapshot of ${this.dir}`);
    }
    const doc = this.documents[key];
    if (!doc.writable) {
      throw new EntryconfError(
        "E_UNSUPPORTED_EDIT",
        `document ${JSON.stringify(key)} is ${doc.format}; only JSON documents are writable`,
      );
    }
    const rec = this.#byPath.get(doc.path)!;

    let value = deepCopy(rec.parsed as Value);
    edits.forEach((e, i) => {
      if (e.op === "set") {
        let v = normalizeValue(e.value, i);
        const mode = e.mode ?? "literal";
        if (mode === "literal") v = escapeLiteral(v);
        else if (mode === "expression") {
          if (typeof v !== "string") {
            throw new EntryconfError("E_EDIT", `edit ${i}: expression mode requires a string value`);
          }
        } else {
          throw new EntryconfError("E_EDIT", `edit ${i}: unknown mode ${JSON.stringify(mode)}`);
        }
        value = setAt(value, e.pointer, v);
      } else if (e.op === "remove") {
        value = removeAt(value, e.pointer);
      } else {
        throw new EntryconfError("E_EDIT", `edit ${i}: unknown op ${JSON.stringify(e.op)} (want "set" or "remove")`);
      }
    });
    const after = formatJSON(value);

    const rec2 = new Recorder();
    const l: Loader = {
      src: this.#source(new Map([[doc.path, Buffer.from(after, "utf8")]])),
      procEnv: mapLookup(this.#captured),
      rec: rec2,
    };
    const candidate = loadTree(l, this.dir);

    const revisions = new Map<string, string>();
    const paths = new Map<string, string>();
    for (const path of rec2.order) {
      const d = rec2.docs.get(path)!;
      const k = documentKey(this.dir, path);
      revisions.set(k, revisionOf(d.data));
      paths.set(k, path);
    }
    // The edited document's own revision is what is on disk now, not the new
    // text: commit must find the file as the snapshot saw it.
    revisions.set(key, doc.revision);
    const variables = [...l.vars!.used].sort(compareCodePoints);

    return new Plan(
      {
        document: key,
        before: decodeUtf8(rec.data, doc.path),
        after,
        candidate,
        affected: affectedPointers(this.tree, candidate),
        grafts: doc.grafts,
        revisions: sortedRecord(revisions),
        variables,
        variablesRevision: variablesRevision(variables, l.vars!),
      },
      this.dir,
      doc.path,
      paths,
      this.#live,
    );
  }
}

/** A prepared, validated change to one document (SPEC §10.6). Nothing has been written. */
export class Plan {
  readonly document: string;
  readonly before: string;
  readonly after: string;
  readonly candidate: Tree;
  readonly affected: string[];
  readonly grafts: Graft[];
  readonly revisions: Record<string, string>;
  readonly variables: string[];
  readonly variablesRevision: string;

  readonly #dir: string;
  readonly #path: string;
  readonly #paths: Map<string, string>;
  readonly #live: EnvLookup;

  /** @internal — use `Snapshot.plan()`. */
  constructor(
    fields: {
      document: string;
      before: string;
      after: string;
      candidate: Tree;
      affected: string[];
      grafts: Graft[];
      revisions: Record<string, string>;
      variables: string[];
      variablesRevision: string;
    },
    dir: string,
    path: string,
    paths: Map<string, string>,
    live: EnvLookup,
  ) {
    this.document = fields.document;
    this.before = fields.before;
    this.after = fields.after;
    this.candidate = fields.candidate;
    this.affected = fields.affected;
    this.grafts = fields.grafts;
    this.revisions = fields.revisions;
    this.variables = fields.variables;
    this.variablesRevision = fields.variablesRevision;
    this.#dir = dir;
    this.#path = path;
    this.#paths = paths;
    this.#live = live;
  }

  /** The SPEC §10.6 wire form (snake_case `variables_revision`). */
  toJSON(): Record<string, unknown> {
    return {
      document: this.document,
      before: this.before,
      after: this.after,
      candidate: this.candidate,
      affected: this.affected,
      grafts: this.grafts,
      revisions: this.revisions,
      variables: this.variables,
      variables_revision: this.variablesRevision,
    };
  }

  /**
   * Write `after` to the plan's document (SPEC §10.7): lock, recheck every
   * dependency's revision and the variables fingerprint (`E_STALE_PLAN` on any
   * change), write atomically (`E_WRITE` on failure, target untouched), unlock.
   */
  commit(options?: CommitOptions): Receipt {
    const timeout =
      options?.lockTimeoutMs !== undefined && options.lockTimeoutMs > 0
        ? options.lockTimeoutMs
        : DEFAULT_LOCK_TIMEOUT_MS;
    const release = acquireLock(this.#dir, timeout);
    try {
      const files = new Map<string, string>();
      const origin = new Map<string, string>();
      for (const [key, want] of Object.entries(this.revisions)) {
        const path = this.#paths.get(key) ?? resolve(this.#dir, key);
        let data: Uint8Array;
        try {
          data = readFileSync(path);
        } catch (err) {
          throw new EntryconfError("E_STALE_PLAN", `document ${JSON.stringify(key)} can no longer be read: ${(err as Error).message}`);
        }
        const got = revisionOf(data);
        if (got !== want) {
          throw new EntryconfError(
            "E_STALE_PLAN",
            `document ${JSON.stringify(key)} changed since the plan was made (${got}, plan expected ${want})`,
          );
        }
        if (key.endsWith(".env")) {
          for (const [name, value] of parseEnvFile(decodeUtf8(data, path), path)) {
            files.set(name, value);
            origin.set(name, path);
          }
        }
      }
      const vars = new Vars(files, origin, this.#live);
      if (variablesRevision(this.variables, vars) !== this.variablesRevision) {
        throw new EntryconfError(
          "E_STALE_PLAN",
          `a variable the candidate depends on changed since the plan was made (${this.variables.join(", ")})`,
        );
      }

      atomicWrite(this.#path, Buffer.from(this.after, "utf8"));
      const revision = revisionOf(this.after);
      const revisions = sortedRecord([
        ...Object.entries(this.revisions),
        [this.document, revision],
      ]);
      return { document: this.document, revision, revisions };
    } finally {
      release();
    }
  }
}

/** SPEC §10.6's fingerprint over the named variables. */
function variablesRevision(names: string[], vars: Vars): string {
  let text = "";
  for (const name of names) {
    const v = vars.get(name);
    text += v !== undefined ? `=${name}=${v}\n` : `-${name}\n`;
  }
  return revisionOf(text);
}

/**
 * Literal mode (SPEC §10.5): every string, recursively but never an object
 * key, has each `$` doubled and then a leading `@` doubled.
 */
function escapeLiteral(v: Value): Value {
  if (typeof v === "string") {
    const s = v.replaceAll("$", "$$$$");
    return s.startsWith("@") ? `@${s}` : s;
  }
  if (Array.isArray(v)) return v.map(escapeLiteral);
  if (isRecord(v)) {
    const out = emptyObject();
    for (const [k, item] of Object.entries(v)) setKey(out, k, escapeLiteral(item));
    return out;
  }
  return v;
}

/** A caller-supplied value into the loader's JSON shapes, or `E_EDIT`. */
function normalizeValue(v: unknown, index: number): Value {
  if (v === null || v === undefined) return null;
  switch (typeof v) {
    case "boolean":
    case "string":
      return v;
    case "number":
      if (!Number.isFinite(v)) {
        throw new EntryconfError("E_EDIT", `edit ${index}: number ${String(v)} has no JSON-equivalent form`);
      }
      return v;
    case "bigint":
      if (v > BigInt(Number.MAX_SAFE_INTEGER) || v < -BigInt(Number.MAX_SAFE_INTEGER)) {
        throw new EntryconfError("E_EDIT", `edit ${index}: integer ${v} is outside the portable range`);
      }
      return Number(v);
    case "object":
      break;
    default:
      throw new EntryconfError("E_EDIT", `edit ${index}: value of type ${typeof v} has no JSON-equivalent form`);
  }
  if (Array.isArray(v)) return v.map((item) => normalizeValue(item, index));
  if (v instanceof Map) {
    const out = emptyObject();
    for (const [k, item] of v) {
      if (typeof k !== "string") throw new EntryconfError("E_EDIT", `edit ${index}: map keys must be strings`);
      setKey(out, k, normalizeValue(item, index));
    }
    return out;
  }
  const out = emptyObject();
  for (const [k, item] of Object.entries(v as object)) setKey(out, k, normalizeValue(item, index));
  return out;
}

/** SPEC §10.6's diff: the shallowest pointers where the trees differ, sorted. */
export function affectedPointers(before: Value, after: Value): string[] {
  const out: string[] = [];
  diffInto(before, after, "", out);
  return out.sort(compareCodePoints);
}

function diffInto(a: Value, b: Value, ptr: string, out: string[]): void {
  if (isRecord(a)) {
    if (!isRecord(b)) {
      out.push(ptr);
      return;
    }
    const keys = new Set([...Object.keys(a), ...Object.keys(b)]);
    for (const k of keys) {
      const child = `${ptr}/${escapePointerToken(k)}`;
      if (!Object.hasOwn(a, k) || !Object.hasOwn(b, k)) out.push(child);
      else diffInto(a[k], b[k], child, out);
    }
    return;
  }
  if (Array.isArray(a)) {
    if (!Array.isArray(b) || a.length !== b.length) {
      out.push(ptr);
      return;
    }
    a.forEach((item, i) => diffInto(item, b[i], `${ptr}/${i}`, out));
    return;
  }
  if (isRecord(b) || Array.isArray(b) || a !== b) out.push(ptr);
}

/**
 * Load `dir` like `load()` and return a `Snapshot` for inspecting provenance
 * and preparing edits (SPEC §10.2). The process environment is captured now;
 * plans resolve variables against that capture, and `commit` fails with
 * `E_STALE_PLAN` if a variable the candidate depends on has since changed.
 */
export function open(dir: string): Snapshot {
  const captured: Record<string, string> = {};
  for (const [k, v] of Object.entries(process.env)) if (v !== undefined) captured[k] = v;
  return openWith(dir, captured, processEnvLookup);
}

/** `open` with the captured and live environments supplied (test seam). */
export function openWith(dir: string, captured: Record<string, string>, live: EnvLookup): Snapshot {
  const abs = resolve(dir);
  let envNames: string[] = [];
  try {
    envNames = osSource.envFileNames(abs).sort();
  } catch {
    // the load itself reports an unreadable directory
  }
  const rec = new Recorder();
  const l: Loader = { src: osSource, procEnv: mapLookup(captured), rec };
  const tree = loadTree(l, abs);

  const entries: [string, Document][] = rec.order.map((path) => {
    const d = rec.docs.get(path)!;
    const key = documentKey(abs, path);
    const grafts = [...d.grafts]
      .sort((x, y) => compareCodePoints(x.effective, y.effective))
      .map((g) => ({
        effective: g.effective,
        chain: g.chain.map((r) => ({ document: documentKey(abs, r.document), pointer: r.pointer })),
      }));
    return [
      key,
      {
        key,
        path,
        format: d.format as Document["format"],
        revision: revisionOf(d.data),
        writable: d.format === "json",
        grafts,
      },
    ];
  });
  return new Snapshot(abs, tree, sortedRecord(entries), rec.docs, envNames, captured, live);
}

// parseDocument is re-exported for the CLI's request parsing.
export { parseDocument };
