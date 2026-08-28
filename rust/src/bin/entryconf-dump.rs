//! Cross-implementation check harness: load a config directory and print its
//! tree as one line of JSON.
//!
//!   entryconf-dump [-v] [--] <config-dir>
//!
//! Exit-code convention, shared by every implementation's dump CLI:
//!
//!   * success — the tree on stdout, exit 0;
//!   * a **load** failure — the bare `E_*` code as the first line of stderr and
//!     nothing else, so the output is comparable across implementations, exit 1
//!     (`-v` adds the non-normative detail message on a second line);
//!   * any **other** fault (usage, internal) — exit 2, and never an `E_*` code,
//!     so a broken invocation can't be mistaken for a config verdict.

use std::io::Write;
use std::path::Path;
use std::process::ExitCode;

/// A fault that is not a load failure: exit 2, no `E_*` code.
const EXIT_FAULT: u8 = 2;

fn main() -> ExitCode {
    let argv: Vec<String> = std::env::args().skip(1).collect();

    let Some((dir, verbose)) = parse_args(&argv) else {
        eprintln!("usage: entryconf-dump [-v] [--] <config-dir>");
        return ExitCode::from(EXIT_FAULT);
    };

    match entryconf::load(Path::new(dir)) {
        Ok(tree) => {
            // Writing the tree is the one internal fault this program can hit
            // (a closed or full stdout); it is not a config verdict, so it
            // exits 2 without a code rather than looking like a load failure.
            if let Err(e) = writeln!(std::io::stdout(), "{tree}") {
                eprintln!("cannot write the tree to stdout: {e}");
                return ExitCode::from(EXIT_FAULT);
            }
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("{}", e.code());
            if verbose {
                eprintln!("{}", e.message());
            }
            ExitCode::FAILURE
        }
    }
}

/// Returns the config directory and the verbose flag, or `None` for a usage
/// fault. Exactly one operand is required; `-v` may appear anywhere before
/// `--`, and any other dash-led argument is a usage fault rather than a
/// directory name that would fail as `E_NO_ENTRYPOINT`.
fn parse_args(argv: &[String]) -> Option<(&str, bool)> {
    let mut verbose = false;
    let mut operand: Option<&str> = None;
    let mut rest = argv.iter();

    while let Some(arg) = rest.next() {
        match arg.as_str() {
            "-v" => verbose = true,
            "--" => {
                for tail in rest {
                    if operand.replace(tail).is_some() {
                        return None;
                    }
                }
                break;
            }
            flag if flag.starts_with('-') && flag.len() > 1 => return None,
            operand_arg => {
                if operand.replace(operand_arg).is_some() {
                    return None;
                }
            }
        }
    }

    operand.map(|dir| (dir, verbose))
}

#[cfg(test)]
mod tests {
    use super::parse_args;

    fn args(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| (*s).to_string()).collect()
    }

    #[test]
    fn accepts_a_lone_directory() {
        assert_eq!(parse_args(&args(&["cfg"])), Some(("cfg", false)));
    }

    #[test]
    fn accepts_verbose_on_either_side() {
        assert_eq!(parse_args(&args(&["-v", "cfg"])), Some(("cfg", true)));
        assert_eq!(parse_args(&args(&["cfg", "-v"])), Some(("cfg", true)));
    }

    #[test]
    fn after_dash_dash_a_dash_led_name_is_a_directory() {
        assert_eq!(parse_args(&args(&["--", "-cfg"])), Some(("-cfg", false)));
    }

    #[test]
    fn rejects_missing_extra_and_unknown_operands() {
        assert_eq!(parse_args(&args(&[])), None);
        assert_eq!(parse_args(&args(&["-v"])), None);
        assert_eq!(parse_args(&args(&["a", "b"])), None);
        assert_eq!(parse_args(&args(&["--help"])), None);
        assert_eq!(parse_args(&args(&["-x", "cfg"])), None);
    }

    /// A bare `-` is an operand, not a flag (nothing reads stdin here, so it
    /// simply fails to load — but as a directory, not as a usage fault).
    #[test]
    fn treats_a_bare_dash_as_an_operand() {
        assert_eq!(parse_args(&args(&["-"])), Some(("-", false)));
    }
}
