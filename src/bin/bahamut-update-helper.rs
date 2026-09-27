use std::process::ExitCode;

use bahamut_launcher::update_helper::run_cli;

fn main() -> ExitCode {
    match run_cli(std::env::args_os().skip(1)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::FAILURE
        }
    }
}
