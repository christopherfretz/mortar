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

/// `MORTAR_HUGEPAGES=1` (opt-in, WP-032e): run the link with glibc's
/// allocator asking for transparent huge pages (`glibc.malloc.hugetlb=1`,
/// read only at process start), by executing this same binary again
/// (`/proc/self/exe`, with the same `argv[0]`, so the program name and
/// `-B` lookups are unchanged) with that tunable appended to any
/// `GLIBC_TUNABLES` already set. Not when the variable already names the
/// tunable, nor in the executed copy (`MORTAR_HUGEPAGES_EXEC`); if the
/// exec fails the link runs here without it. Nothing in the output depends
/// on it: the allocator's page size changes faults, not values.
#[cfg(all(target_os = "linux", target_env = "gnu"))]
fn hugepages() {
    use std::os::unix::process::CommandExt as _;
    const TUNABLE: &str = "glibc.malloc.hugetlb";
    const GUARD: &str = "MORTAR_HUGEPAGES_EXEC";
    if std::env::var_os("MORTAR_HUGEPAGES").is_none_or(|v| v != "1")
        || std::env::var_os(GUARD).is_some()
    {
        return;
    }
    let mut tunables = std::env::var_os("GLIBC_TUNABLES").unwrap_or_default();
    if tunables.to_string_lossy().split(':').any(|t| t.split('=').next() == Some(TUNABLE)) {
        return;
    }
    if !tunables.is_empty() {
        tunables.push(":");
    }
    tunables.push(TUNABLE);
    tunables.push("=1");
    let mut command_line = std::env::args_os();
    let name = command_line.next().unwrap_or_default();
    // Returns only on failure, and then the link goes on in this process.
    let _ = std::process::Command::new("/proc/self/exe")
        .arg0(name)
        .args(command_line)
        .env("GLIBC_TUNABLES", tunables)
        .env(GUARD, "1")
        .exec();
}

#[cfg(not(all(target_os = "linux", target_env = "gnu")))]
const fn hugepages() {}

fn main() -> ExitCode {
    hugepages();
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
        Outcome::Linked { report, stdout } => {
            let style = Style { color: colored(invocation.color), ..Style::plain(&prog) };
            let mut err = std::io::stderr().lock();
            let _ = err.write_all(invocation.removed.as_bytes());
            let _ = err.write_all(report.render(&style).as_bytes());
            let _ = err.write_all(invocation.incremental.as_bytes());
            let _ = err.write_all(invocation.phases.as_bytes());
            let _ = std::io::stdout().write_all(stdout.as_bytes());
            ExitCode::SUCCESS
        }
        Outcome::Failed(report) => {
            // Demangling comes with the phases that name symbols.
            let style = Style { color: colored(invocation.color), ..Style::plain(&prog) };
            let text = report.render(&style);
            let mut err = std::io::stderr().lock();
            let _ = err.write_all(invocation.removed.as_bytes());
            let _ = err.write_all(text.as_bytes());
            let _ = err.write_all(invocation.incremental.as_bytes());
            let _ = err.write_all(invocation.phases.as_bytes());
            ExitCode::FAILURE
        }
    }
}
