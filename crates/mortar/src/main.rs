//! The `mortar` command-line tool. All behavior is in the library.

use std::io::Write;
use std::process::ExitCode;

use mortar_diag::Style;

fn main() -> ExitCode {
    let prog = mortar::program_name(std::env::args_os().next().as_deref());
    let args: Vec<String> = std::env::args().skip(1).collect();
    match mortar::link(&args) {
        mortar::Outcome::Done(text) => {
            // A closed pipe is not an error worth a second message.
            let _ = std::io::stdout().write_all(text.as_bytes());
            ExitCode::SUCCESS
        }
        mortar::Outcome::Failed(report) => {
            // Color and demangling come with the command line
            // (`spec/cli.md`); until then: neither.
            let text = report.render(&Style::plain(&prog));
            let _ = std::io::stderr().lock().write_all(text.as_bytes());
            ExitCode::FAILURE
        }
    }
}
