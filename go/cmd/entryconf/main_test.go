package main

import (
	"bytes"
	"path/filepath"
	"regexp"
	"strings"
	"testing"
	"time"
)

const casesDir = "../../../testdata/cases"

// anyCode matches an E_* code anywhere in a line. Only a load failure may
// print one, so it is also how the usage and internal-fault paths are checked.
var anyCode = regexp.MustCompile(`E_[A-Z_]+`)

func dump(t *testing.T, args ...string) (code int, stdout, stderr string) {
	t.Helper()
	var out, errBuf bytes.Buffer
	code = run(append([]string{"dump"}, args...), &out, &errBuf)
	return code, out.String(), errBuf.String()
}

func firstLine(s string) string {
	line, _, _ := strings.Cut(s, "\n")
	return line
}

// TestDumpLoadFailurePrintsBareCode pins the cross-implementation contract: a
// load failure exits 1 with the bare E_* code as the first line of stderr.
func TestDumpLoadFailurePrintsBareCode(t *testing.T) {
	for _, tc := range []struct{ caseName, want string }{
		{"07-include-cycle", "E_INCLUDE_CYCLE"},
		{"04-var-missing", "E_MISSING_VAR"},
		{"12-no-entrypoint", "E_NO_ENTRYPOINT"},
		{"13-parse-malformed-entrypoint", "E_PARSE"},
	} {
		code, stdout, stderr := dump(t, filepath.Join(casesDir, tc.caseName, "config"))
		if code != 1 {
			t.Errorf("%s: exit %d, want 1 (stderr: %s)", tc.caseName, code, stderr)
		}
		if got := firstLine(stderr); got != tc.want {
			t.Errorf("%s: first stderr line %q, want bare %q", tc.caseName, got, tc.want)
		}
		if stdout != "" {
			t.Errorf("%s: wrote %q to stdout on failure", tc.caseName, stdout)
		}
	}
}

// TestDumpAliasBombFailsFast is the SPEC §2 expansion budget seen from the
// CLI: E_PARSE, exit 1, and fast — the budget is counted during expansion, so
// the document is rejected without being materialized.
func TestDumpAliasBombFailsFast(t *testing.T) {
	start := time.Now()
	code, _, stderr := dump(t, filepath.Join(casesDir, "57-yaml-alias-bomb", "config"))
	elapsed := time.Since(start)

	if code != 1 {
		t.Errorf("exit %d, want 1 (stderr: %s)", code, stderr)
	}
	if got := firstLine(stderr); got != "E_PARSE" {
		t.Errorf("first stderr line %q, want bare %q", got, "E_PARSE")
	}
	if elapsed > 2*time.Second {
		t.Errorf("took %s: the node budget must bound the work, not a timeout", elapsed)
	}
}

// TestDumpSucceeds covers the happy path, including the heavy-but-legal alias
// document that the budget must not reject.
func TestDumpSucceeds(t *testing.T) {
	for _, caseName := range []string{"01-basic", "58-yaml-alias-heavy-ok"} {
		code, stdout, stderr := dump(t, filepath.Join(casesDir, caseName, "config"))
		if code != 0 {
			t.Errorf("%s: exit %d, want 0 (stderr: %s)", caseName, code, stderr)
		}
		if !strings.HasPrefix(stdout, "{") {
			t.Errorf("%s: stdout is not a JSON object: %q", caseName, stdout)
		}
		if stderr != "" {
			t.Errorf("%s: unexpected stderr: %q", caseName, stderr)
		}
	}
}

// TestUsageFaultsExitTwoWithoutCode is the other half of the convention: a
// fault that is not a load failure exits 2 and prints no E_* code, so a code
// on stderr unambiguously means the config was rejected.
func TestUsageFaultsExitTwoWithoutCode(t *testing.T) {
	basic := filepath.Join(casesDir, "01-basic", "config")
	for _, args := range [][]string{
		{},                            // no command
		{"frobnicate"},                // unknown command
		{"dump"},                      // dump without a directory
		{"dump", basic, basic},        // two directories
		{"dump", "--nonesuch", basic}, // unknown flag
	} {
		var out, errBuf bytes.Buffer
		code := run(args, &out, &errBuf)
		if code != 2 {
			t.Errorf("run(%q): exit %d, want 2", args, code)
		}
		if loc := anyCode.FindString(errBuf.String()); loc != "" {
			t.Errorf("run(%q): stderr names %s; only load failures may print a code", args, loc)
		}
		if out.Len() != 0 {
			t.Errorf("run(%q): wrote %q to stdout", args, out.String())
		}
	}
}

// TestHelpAndVersion keeps the two informational commands on exit 0, and the
// version string on the spec version this build implements.
func TestHelpAndVersion(t *testing.T) {
	for _, args := range [][]string{{"help"}, {"--help"}, {"version"}, {"--version"}} {
		var out, errBuf bytes.Buffer
		if code := run(args, &out, &errBuf); code != 0 {
			t.Errorf("run(%q): exit %d, want 0", args, code)
		}
		if out.Len() == 0 {
			t.Errorf("run(%q): no output", args)
		}
	}
	var out, errBuf bytes.Buffer
	run([]string{"version"}, &out, &errBuf)
	if !strings.Contains(out.String(), specVersion) {
		t.Errorf("version output %q does not name spec %s", out.String(), specVersion)
	}
	if specVersion != "0.2.0" {
		t.Errorf("specVersion is %q; this build implements entryconf spec 0.2.0", specVersion)
	}
}
