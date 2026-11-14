//! The `mortar` command-line tool. All behavior is in the library.

use std::io::Write;
use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match mortar::link(&args) {
        mortar::Outcome::Done(text) => {
            // A closed pipe is not an error worth a second message.
            let _ = std::io::stdout().write_all(text.as_bytes());
            ExitCode::SUCCESS
        }
        mortar::Outcome::Failed(lines) => {
            let mut err = std::io::stderr().lock();
            for line in lines {
                let _ = writeln!(err, "{line}");
            }
            ExitCode::FAILURE
        }
    }
}
