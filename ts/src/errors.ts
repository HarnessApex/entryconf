/**
 * Error codes are normative (SPEC §7, §10.9); messages are not.
 */
export type ErrorCode =
  | "E_NO_ENTRYPOINT"
  | "E_MULTIPLE_ENTRYPOINTS"
  | "E_PARSE"
  | "E_ENV_CONFLICT"
  | "E_INCLUDE"
  | "E_INCLUDE_CYCLE"
  | "E_MISSING_VAR"
  | "E_SUBSTITUTION"
  // Editing codes (SPEC §10.9), added in 0.3.0.
  | "E_UNSUPPORTED_EDIT"
  | "E_EDIT"
  | "E_PATH"
  | "E_STALE_PLAN"
  | "E_LOCKED"
  | "E_WRITE";

/**
 * Every failure raised by `load()`, `open()`, and the editing operations is an
 * `EntryconfError` carrying a `code`.
 */
export class EntryconfError extends Error {
  readonly code: ErrorCode;

  constructor(code: ErrorCode, detail: string) {
    super(`${code}: ${detail}`);
    this.name = "EntryconfError";
    this.code = code;
  }
}
