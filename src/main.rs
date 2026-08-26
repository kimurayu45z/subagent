use std::ffi::OsString;
use std::process::ExitCode;

mod cli;

fn main() -> ExitCode {
    let args: Vec<OsString> = std::env::args_os().skip(1).collect();
    let mut stdout = std::io::stdout();
    let mut stderr = std::io::stderr();
    cli::dispatch(&args, &mut stdout, &mut stderr)
}
