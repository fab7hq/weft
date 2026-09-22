//! The `ringframe` binary. The skills in `fab7` call it by name, and that
//! string is the contract.

use std::io::{Read, Write};

fn main() -> std::process::ExitCode {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    // Read stdin only when a command asks for it: a hook pipes a payload, and
    // everything else would block waiting for one.
    let mut read_stdin = || {
        let mut buf = String::new();
        let _ = std::io::stdin().read_to_string(&mut buf);
        buf
    };
    let run = ringframe::cli::run(&argv, &mut read_stdin);
    let _ = std::io::stdout().write_all(run.out.as_bytes());
    let _ = std::io::stderr().write_all(run.err.as_bytes());
    std::process::ExitCode::from(run.code as u8)
}
