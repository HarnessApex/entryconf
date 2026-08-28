//! Cross-implementation check harness: load a config directory and print its
//! tree as one line of JSON.
//!
//!   entryconf-dump <config-dir>
//!
//! Success: the tree on stdout, exit 0.
//! Failure: the `E_*` code on stderr — and nothing else, so the output is
//! comparable across implementations — exit 1. `-v` adds the detail message.

use std::path::Path;
use std::process::ExitCode;

fn main() -> ExitCode {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let verbose = args.iter().any(|a| a == "-v");
    args.retain(|a| a != "-v");

    let [dir] = args.as_slice() else {
        eprintln!("usage: entryconf-dump [-v] <config-dir>");
        return ExitCode::from(2);
    };

    match entryconf::load(Path::new(dir)) {
        Ok(tree) => {
            println!("{tree}");
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
