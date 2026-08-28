/**
 * Error codes are normative (SPEC §7); messages are not.
 */
export type ErrorCode =
  | "E_NO_ENTRYPOINT"
  | "E_MULTIPLE_ENTRYPOINTS"
  | "E_PARSE"
  | "E_ENV_CONFLICT"
  | "E_INCLUDE"
  | "E_INCLUDE_CYCLE"
  | "E_MISSING_VAR"
  | "E_SUBSTITUTION";

/** Every failure raised by `load()` is an `EntryconfError` carrying a `code`. */
export class EntryconfError extends Error {
  readonly code: ErrorCode;

  constructor(code: ErrorCode, detail: string) {
    super(`${code}: ${detail}`);
    this.name = "EntryconfError";
    this.code = code;
  }
}
