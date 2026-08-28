package entryconf

import "fmt"

// Error codes defined by SPEC §7. The codes are part of the public contract:
// they are stable, are never reused, and are what callers should branch on.
const (
	CodeNoEntrypoint        = "E_NO_ENTRYPOINT"
	CodeMultipleEntrypoints = "E_MULTIPLE_ENTRYPOINTS"
	CodeParse               = "E_PARSE"
	CodeEnvConflict         = "E_ENV_CONFLICT"
	CodeInclude             = "E_INCLUDE"
	CodeIncludeCycle        = "E_INCLUDE_CYCLE"
	CodeMissingVar          = "E_MISSING_VAR"
	CodeSubstitution        = "E_SUBSTITUTION"
)

// Error is the only error type returned by Load. Use errors.As to recover it
// and Code to branch on the normative error code:
//
//	var ecErr *entryconf.Error
//	if errors.As(err, &ecErr) && ecErr.Code() == entryconf.CodeMissingVar { ... }
type Error struct {
	code string
	msg  string
	err  error // optional underlying cause (parser errors, I/O errors)
}

// Code returns the SPEC §7 error code, e.g. "E_PARSE".
func (e *Error) Code() string { return e.code }

// Error implements the error interface. The message is informative only; the
// code is the normative part.
func (e *Error) Error() string { return e.code + ": " + e.msg }

// Unwrap exposes the underlying cause, if any.
func (e *Error) Unwrap() error { return e.err }

func errf(code, format string, args ...any) *Error {
	return &Error{code: code, msg: fmt.Sprintf(format, args...)}
}

func wrapf(code string, cause error, format string, args ...any) *Error {
	return &Error{code: code, msg: fmt.Sprintf(format, args...) + ": " + cause.Error(), err: cause}
}
