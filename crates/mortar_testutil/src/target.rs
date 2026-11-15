//! The architecture a test is about, on whatever host it runs
//! (`spec/harness.md` "Hosted CI", "A test's subject decides its tools").
//!
//! A test about AArch64 linking compiles with the AArch64 toolchain and
//! runs its programs natively on an AArch64 host and under `qemu-aarch64`
//! elsewhere; never with the host's default toolchain by accident. The
//! names follow the link-test harness (`xtask::linktest::target`): the
//! host's own architecture's tools by their plain names (`gcc`,
//! `ld.bfd`), another's as `<triple>-<tool>`.
//!
//! `MORTAR_TEST_CROSS=1` makes every architecture foreign, the host's
//! too: prefixed tools, programs under qemu. An AArch64 machine then runs
//! its AArch64 tests the way an x86-64 host does, which is how the
//! off-host path is tried without one.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Command;

/// An architecture tests link for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Arch {
    Aarch64,
    X86_64,
    I386,
}

impl Arch {
    /// The architecture of this machine, if it is one of the three.
    pub fn host() -> Option<Self> {
        match std::env::consts::ARCH {
            "aarch64" => Some(Self::Aarch64),
            "x86_64" => Some(Self::X86_64),
            "x86" => Some(Self::I386),
            _ => None,
        }
    }

    /// `aarch64`, `x86_64` or `i386`.
    pub fn name(self) -> &'static str {
        match self {
            Self::Aarch64 => "aarch64",
            Self::X86_64 => "x86_64",
            Self::I386 => "i386",
        }
    }

    /// The GNU triple: the cross tools' prefix, the cross system root's
    /// name and Clang's `--target`.
    pub fn triple(self) -> &'static str {
        match self {
            Self::Aarch64 => "aarch64-linux-gnu",
            Self::X86_64 => "x86_64-linux-gnu",
            Self::I386 => "i686-linux-gnu",
        }
    }

    /// Whether programs of this architecture run natively here and its
    /// tools have their plain names (not under `MORTAR_TEST_CROSS=1`).
    pub fn is_host(self) -> bool {
        Self::host() == Some(self) && std::env::var_os("MORTAR_TEST_CROSS").is_none_or(|v| v != "1")
    }

    /// The name of GNU tool `tool` (`gcc`, `g++`, `gfortran`, `ld.bfd`,
    /// `as`, `ar`, `objdump`) for this architecture: plain on its own
    /// host, `<triple>-<tool>` elsewhere.
    pub fn gnu(self, tool: &str) -> String {
        if self.is_host() { tool.to_owned() } else { format!("{}-{tool}", self.triple()) }
    }

    /// [`crate::tool`] of [`Self::gnu`]`(tool)`.
    pub fn tool(self, tool: &str) -> Option<PathBuf> {
        crate::tool(&self.gnu(tool))
    }

    /// What Clang is given to compile or link for this architecture:
    /// nothing on its own host, `--target=<triple>` elsewhere.
    pub fn clang_args(self) -> Vec<String> {
        if self.is_host() { Vec::new() } else { vec![format!("--target={}", self.triple())] }
    }

    /// The user-mode emulator of this architecture's programs.
    pub fn qemu(self) -> &'static str {
        match self {
            Self::Aarch64 => "qemu-aarch64",
            Self::X86_64 => "qemu-x86_64",
            Self::I386 => "qemu-i386",
        }
    }

    /// The system root of the cross packages, `/usr/<triple>`: the C
    /// library and dynamic loader qemu is pointed at.
    pub fn sysroot(self) -> PathBuf {
        Path::new("/usr").join(self.triple())
    }

    /// The debugger that reads and runs this architecture's programs:
    /// `gdb` on its own host, `gdb-multiarch` elsewhere.
    pub fn gdb(self) -> &'static str {
        if self.is_host() { "gdb" } else { "gdb-multiarch" }
    }

    /// A command that runs `program` with `args`: the program itself on
    /// its own host, else `qemu-<arch> -L <sysroot> [-0 <argv0>] program
    /// args...`. `argv0` replaces the program's `argv[0]` when given.
    ///
    /// # Panics
    /// Panics when the emulator is needed and not found (a failure under
    /// `MORTAR_REQUIRE_TOOLS=1` in [`crate::tool`]; without it too, since
    /// the caller has nothing to skip to).
    pub fn command(self, program: &Path, argv0: Option<&str>) -> Command {
        if self.is_host() {
            let mut c = Command::new(program);
            if let Some(argv0) = argv0 {
                std::os::unix::process::CommandExt::arg0(&mut c, argv0);
            }
            return c;
        }
        let qemu = crate::tool(self.qemu()).unwrap_or_else(|| {
            panic!("{} runs {} programs off their host", self.qemu(), self.name())
        });
        let mut c = Command::new(qemu);
        c.arg("-L").arg(self.sysroot());
        if let Some(argv0) = argv0 {
            c.arg("-0").arg(argv0);
        }
        c.arg(program);
        c
    }

    /// [`Self::command`]'s argument list for a caller that builds its own
    /// command line (a shell, a timeout wrapper): the program to start
    /// and its leading arguments.
    pub fn runner(self, program: &Path) -> (PathBuf, Vec<OsString>) {
        if self.is_host() {
            return (program.to_path_buf(), Vec::new());
        }
        let qemu = crate::tool(self.qemu()).unwrap_or_else(|| {
            panic!("{} runs {} programs off their host", self.qemu(), self.name())
        });
        (qemu, vec!["-L".into(), self.sysroot().into_os_string(), program.as_os_str().to_owned()])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // "A test's subject decides its tools": the names on the host and off
    // it, as the link-test harness names them.
    #[test]
    fn tool_names_follow_the_host() {
        for arch in [Arch::Aarch64, Arch::X86_64, Arch::I386] {
            let gcc = arch.gnu("gcc");
            if arch.is_host() {
                assert_eq!(gcc, "gcc");
                assert!(arch.clang_args().is_empty());
                assert_eq!(arch.gdb(), "gdb");
            } else {
                assert_eq!(gcc, format!("{}-gcc", arch.triple()));
                assert_eq!(arch.clang_args(), [format!("--target={}", arch.triple())]);
                assert_eq!(arch.gdb(), "gdb-multiarch");
            }
            assert!(crate::ci_tools::covers(&gcc) && crate::ci_tools::covers(arch.qemu()));
        }
        assert_eq!(Arch::I386.sysroot(), Path::new("/usr/i686-linux-gnu"));
    }
}
