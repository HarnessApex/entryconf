import { resolve } from "node:path";

import { EntryconfError, type ErrorCode } from "./errors.ts";
import { loadTree, processEnvLookup } from "./loader.ts";
import { osSource } from "./source.ts";
import type { Tree, Value } from "./tree.ts";

export { EntryconfError };
export type { ErrorCode, Tree, Value };
export {
  open,
  Snapshot,
  Plan,
  type Document,
  type Graft,
  type Reference,
  type Origin,
  type Variable,
  type Edit,
  type EditOp,
  type EditMode,
  type Receipt,
  type CommitOptions,
} from "./edit.ts";

/**
 * Load a config directory into a single tree (entryconf spec 0.3.0).
 *
 * Locates the entrypoint (§3), builds the variable namespace from the
 * directory's `*.env` files and the process environment (§4), resolves every
 * `@file:` include (§5), then interpolates `$` references (§6). Every failure
 * is an `EntryconfError` whose `code` is one of the normative `E_*` codes.
 */
export function load(dir: string): Tree {
  return loadTree({ src: osSource, procEnv: processEnvLookup }, resolve(dir));
}
