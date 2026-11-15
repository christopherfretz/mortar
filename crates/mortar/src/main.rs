//! The `mortar` command-line tool. All behavior is in the library.

use std::ffi::OsString;
use std::io::{IsTerminal, Write};
use std::process::ExitCode;

use mortar::{Color, Outcome};
use mortar_diag::Style;

/// `--color-diagnostics=auto` (the default): color when standard error is
/// a terminal that is not `TERM=dumb` (`spec/cli.md` §3.9).
fn colored(choice: Color) -> bool {
    match choice {
        Color::Always => true,
        Color::Never => false,
        Color::Auto => {
            std::io::stderr().is_terminal() && std::env::var_os("TERM").is_none_or(|t| t != "dumb")
        }
    }
}

fn main() -> ExitCode {
    let mut command_line = std::env::args_os();
    let prog = mortar::program_name(command_line.next().as_deref());
    let args: Vec<OsString> = command_line.collect();
    let invocation = mortar::invoke(&prog, &args);
    match invocation.outcome {
        Outcome::Done(text) => {
            // A closed pipe is not an error worth a second message.
            let _ = std::io::stdout().write_all(text.as_bytes());
            ExitCode::SUCCESS
        }
        Outcome::Failed(report) => {
            // Demangling comes with the phases that name symbols.
            let style = Style { color: colored(invocation.color), ..Style::plain(&prog) };
            let text = report.render(&style);
            let _ = std::io::stderr().lock().write_all(text.as_bytes());
            ExitCode::FAILURE
        }
    }
}
