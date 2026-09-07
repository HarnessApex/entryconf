//! Editing CLI (SPEC §10), sharing the library's behavior rather than
//! implementing a second editor:
//!
//!   entryconf-edit inspect [-v] <dir>               list the snapshot's documents
//!   entryconf-edit inspect [-v] <dir> <pointer>     provenance of one effective value
//!   entryconf-edit edit [-n|--dry-run] [-v] <dir> <request.json | ->
//!
//! `edit` reads `{"edits": [...]}` (SPEC §10.4) from the file, or stdin for
//! `-`, plans, and commits unless `-n`. Output is JSON on stdout with keys
//! sorted, the same shapes every implementation prints, so
//! `tools/crosscheck` can diff them.
//!
//! Exit-code convention, as `entryconf-dump`: 0 success; 1 for any entryconf
//! error, with the bare `E_*` code as the first line of stderr and nothing on
//! stdout (`-v` adds the message); 2 for usage or internal faults, never
//! printing an `E_*` code.

use std::io::{Read, Write};
use std::path::Path;
use std::process::ExitCode;

use serde_json::{json, Value};

use entryconf::{CommitOptions, Edit, Error};

const USAGE: &str = "usage: entryconf-edit inspect [-v] <dir> [<pointer>]\n       entryconf-edit edit [-n|--dry-run] [-v] <dir> <request.json | ->";

const EXIT_FAULT: u8 = 2;

fn main() -> ExitCode {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let Some(cmd) = parse_args(&argv) else {
        eprintln!("{USAGE}");
        return ExitCode::from(EXIT_FAULT);
    };
    let verbose = cmd.verbose;
    match run(cmd) {
        Ok(Some(value)) => {
            let text = serde_json::to_string_pretty(&sorted(value)).expect("serializable");
            if let Err(e) = writeln!(std::io::stdout(), "{text}") {
                eprintln!("cannot write to stdout: {e}");
                return ExitCode::from(EXIT_FAULT);
            }
            ExitCode::SUCCESS
        }
        Ok(None) => ExitCode::from(EXIT_FAULT),
        Err(e) => {
            eprintln!("{}", e.code());
            if verbose {
                eprintln!("{}", e.message());
            }
            ExitCode::FAILURE
        }
    }
}

struct Command {
    verbose: bool,
    dry_run: bool,
    kind: Kind,
}

enum Kind {
    Inspect { dir: String, pointer: Option<String> },
    Edit { dir: String, request: String },
}

fn parse_args(argv: &[String]) -> Option<Command> {
    let (name, rest) = argv.split_first()?;
    let mut verbose = false;
    let mut dry_run = false;
    let mut operands: Vec<&str> = Vec::new();
    let mut only_operands = false;
    for arg in rest {
        match arg.as_str() {
            _ if only_operands => operands.push(arg),
            "--" => only_operands = true,
            "-v" => verbose = true,
            "-n" | "--dry-run" if name == "edit" => dry_run = true,
            // `-` alone is an operand (stdin); anything else dash-led is a
            // usage fault, never a directory name.
            flag if flag.starts_with('-') && flag.len() > 1 => return None,
            operand => operands.push(operand),
        }
    }
    let kind = match (name.as_str(), operands.as_slice()) {
        ("inspect", [dir]) => Kind::Inspect { dir: dir.to_string(), pointer: None },
        ("inspect", [dir, pointer]) => Kind::Inspect {
            dir: dir.to_string(),
            pointer: Some(pointer.to_string()),
        },
        ("edit", [dir, request]) => Kind::Edit {
            dir: dir.to_string(),
            request: request.to_string(),
        },
        _ => return None,
    };
    Some(Command { verbose, dry_run, kind })
}

/// `Ok(None)` is a usage or internal fault already reported on stderr.
fn run(cmd: Command) -> Result<Option<Value>, Error> {
    match cmd.kind {
        Kind::Inspect { dir, pointer } => {
            let snap = entryconf::open(Path::new(&dir))?;
            match pointer {
                Some(ptr) => Ok(Some(serde_json::to_value(snap.inspect(&ptr)?).expect("serializable"))),
                None => Ok(Some(json!({
                    "dir": snap.dir,
                    "documents": snap.documents,
                }))),
            }
        }
        Kind::Edit { dir, request } => {
            let text = if request == "-" {
                let mut buf = String::new();
                if let Err(e) = std::io::stdin().read_to_string(&mut buf) {
                    eprintln!("cannot read stdin: {e}");
                    return Ok(None);
                }
                buf
            } else {
                match std::fs::read_to_string(&request) {
                    Ok(t) => t,
                    Err(e) => {
                        eprintln!("cannot read {request}: {e}");
                        return Ok(None);
                    }
                }
            };
            let parsed: Value = match serde_json::from_str(&text) {
                Ok(v) => v,
                Err(e) => {
                    eprintln!("request is not JSON: {e}");
                    return Ok(None);
                }
            };
            let Some(edits) = parsed.get("edits") else {
                eprintln!("request has no \"edits\" array");
                return Ok(None);
            };
            // A malformed edit is the library's verdict (E_EDIT), not a tool fault.
            let edits: Vec<Edit> = entryconf::parse_edits(edits)?;
            let snap = entryconf::open(Path::new(&dir))?;
            let plan = snap.plan(&edits)?;
            let mut out = serde_json::to_value(&plan).expect("serializable");
            let (committed, revision) = if cmd.dry_run {
                (false, Value::Null)
            } else {
                let receipt = plan.commit(&CommitOptions::default())?;
                (true, Value::String(receipt.revision))
            };
            let obj = out.as_object_mut().expect("plan is an object");
            obj.insert("committed".to_string(), Value::Bool(committed));
            obj.insert("revision".to_string(), revision);
            Ok(Some(out))
        }
    }
}

/// serde_json's default `Map` already iterates in key order; this makes that
/// explicit for nested values built with `json!`.
fn sorted(value: Value) -> Value {
    match value {
        Value::Object(map) => {
            Value::Object(map.into_iter().map(|(k, v)| (k, sorted(v))).collect())
        }
        Value::Array(items) => Value::Array(items.into_iter().map(sorted).collect()),
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::{parse_args, Kind};

    fn args(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| (*s).to_string()).collect()
    }

    #[test]
    fn parses_inspect_and_edit() {
        let c = parse_args(&args(&["inspect", "cfg"])).unwrap();
        assert!(matches!(c.kind, Kind::Inspect { pointer: None, .. }));
        let c = parse_args(&args(&["inspect", "-v", "cfg", "/a"])).unwrap();
        assert!(c.verbose);
        assert!(matches!(c.kind, Kind::Inspect { pointer: Some(_), .. }));
        let c = parse_args(&args(&["edit", "-n", "cfg", "-"])).unwrap();
        assert!(c.dry_run);
        assert!(matches!(c.kind, Kind::Edit { ref request, .. } if request == "-"));
    }

    #[test]
    fn rejects_bad_invocations() {
        assert!(parse_args(&args(&[])).is_none());
        assert!(parse_args(&args(&["inspect"])).is_none());
        assert!(parse_args(&args(&["inspect", "a", "b", "c"])).is_none());
        assert!(parse_args(&args(&["edit", "cfg"])).is_none());
        assert!(parse_args(&args(&["edit", "--nope", "cfg", "r.json"])).is_none());
        assert!(parse_args(&args(&["inspect", "-n", "cfg"])).is_none());
        assert!(parse_args(&args(&["frobnicate", "cfg"])).is_none());
    }
}
