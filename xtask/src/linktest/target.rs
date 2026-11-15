//! The matrix of a link-test run: the architectures, the linkers, and the
//! tools of each (`spec/harness.md` "Link tests", "The matrix" and
//! "Tools").
//!
//! This is the one place that knows how an architecture's tools are named,
//! where its system root is and what its substitutions are. The naming of
//! the cross toolchains and of the emulator follows hod's
//! `hod_testutil::target`.

use std::path::{Path, PathBuf};

/// An architecture programs are linked and run for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Arch {
    Aarch64,
    X86_64,
    I386,
}

impl Arch {
    /// Every architecture, in the order of a report.
    pub const ALL: [Self; 3] = [Self::Aarch64, Self::X86_64, Self::I386];

    /// `aarch64`, `x86_64` or `i386`: the name in a test, on the command
    /// line and in a report.
    pub fn name(self) -> &'static str {
        match self {
            Self::Aarch64 => "aarch64",
            Self::X86_64 => "x86_64",
            Self::I386 => "i386",
        }
    }

    /// The architecture of that name.
    pub fn parse(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|arch| arch.name() == name)
    }

    /// The architecture of this machine, if it is a Linux machine of one
    /// of the three.
    pub fn host() -> Option<Self> {
        if !cfg!(target_os = "linux") {
            return None;
        }
        match std::env::consts::ARCH {
            "aarch64" => Some(Self::Aarch64),
            "x86_64" => Some(Self::X86_64),
            "x86" => Some(Self::I386),
            _ => None,
        }
    }

    /// The GNU triple: the prefix of the cross tools, the name of the
    /// cross system root, and clang's `--target`.
    pub fn triple(self) -> &'static str {
        match self {
            Self::Aarch64 => "aarch64-linux-gnu",
            Self::X86_64 => "x86_64-linux-gnu",
            Self::I386 => "i686-linux-gnu",
        }
    }

    /// GNU ld's emulation name (`-m`).
    pub fn emulation(self) -> &'static str {
        match self {
            Self::Aarch64 => "aarch64linux",
            Self::X86_64 => "elf_x86_64",
            Self::I386 => "elf_i386",
        }
    }

    /// The dynamic loader's path as the psABI and glibc fix it.
    pub fn dynamic_linker(self) -> &'static str {
        match self {
            Self::Aarch64 => "/lib/ld-linux-aarch64.so.1",
            Self::X86_64 => "/lib64/ld-linux-x86-64.so.2",
            Self::I386 => "/lib/ld-linux.so.2",
        }
    }

    /// `ELF64` or `ELF32`, as `readelf -h` prints the class.
    pub fn elf_class(self) -> &'static str {
        match self {
            Self::Aarch64 | Self::X86_64 => "ELF64",
            Self::I386 => "ELF32",
        }
    }

    /// The machine, as `readelf -h` prints it.
    pub fn machine(self) -> &'static str {
        match self {
            Self::Aarch64 => "AArch64",
            Self::X86_64 => "Advanced Micro Devices X86-64",
            Self::I386 => "Intel 80386",
        }
    }

    /// The name of a GNU tool (`gcc`, `ld.bfd`, `nm`) for this
    /// architecture on a `host` machine: plain for the host's own, else
    /// `<triple>-<tool>`.
    pub fn gnu_tool(self, host: Option<Self>, tool: &str) -> String {
        if host == Some(self) { tool.to_owned() } else { format!("{}-{tool}", self.triple()) }
    }

    /// The system root: `/` for the host's architecture, else the cross
    /// packages' `/usr/<triple>`, which holds the target's C library and
    /// dynamic loader.
    pub fn sysroot(self, host: Option<Self>) -> PathBuf {
        if host == Some(self) { PathBuf::from("/") } else { Path::new("/usr").join(self.triple()) }
    }

    /// The dynamic loader's file on this machine: [`Arch::dynamic_linker`]
    /// under the system root (what `ldd` runs, as `<loader> --list`).
    pub fn loader(self, host: Option<Self>) -> PathBuf {
        self.sysroot(host).join(self.dynamic_linker().trim_start_matches('/'))
    }

    /// The directory of the C library and its start files: Debian's
    /// multiarch `/usr/lib/<triple>` for the host's architecture, else
    /// `/usr/<triple>/lib`.
    pub fn libdir(self, host: Option<Self>) -> PathBuf {
        if host == Some(self) {
            Path::new("/usr/lib").join(self.triple())
        } else {
            self.sysroot(host).join("lib")
        }
    }

    /// The Debian package that provides `what` (a tool's plain name, or
    /// `sysroot`, or `emulator`) for this architecture as a cross target.
    pub fn cross_package(self, what: &str) -> String {
        let (dashed, debian) = match self {
            Self::Aarch64 => ("aarch64-linux-gnu", "arm64"),
            Self::X86_64 => ("x86-64-linux-gnu", "amd64"),
            Self::I386 => ("i686-linux-gnu", "i386"),
        };
        match what {
            "gcc" | "g++" | "gfortran" => format!("{what}-{dashed}"),
            "sysroot" => format!("libc6-dev-{debian}-cross"),
            "emulator" => "qemu-user".to_owned(),
            _ => format!("binutils-{dashed}"),
        }
    }
}

/// A linker of the matrix.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Linker {
    Bfd,
    Lld,
    Mold,
    Mortar,
}

impl Linker {
    /// Every linker, in the order of a run and of a report: the
    /// references, then mortar.
    pub const ALL: [Self; 4] = [Self::Bfd, Self::Lld, Self::Mold, Self::Mortar];

    /// The reference linkers.
    pub const REFERENCES: [Self; 3] = [Self::Bfd, Self::Lld, Self::Mold];

    /// `bfd`, `lld`, `mold` or `mortar`.
    pub fn name(self) -> &'static str {
        match self {
            Self::Bfd => "bfd",
            Self::Lld => "lld",
            Self::Mold => "mold",
            Self::Mortar => "mortar",
        }
    }

    /// The linker of that name.
    pub fn parse(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|linker| linker.name() == name)
    }

    /// Whether this is a linker tests are proven with, not the one under
    /// test.
    pub fn is_reference(self) -> bool {
        self != Self::Mortar
    }
}

/// A compiler family of a compile step or a link driver.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Family {
    Gcc,
    Clang,
}

impl Family {
    /// `gcc` or `clang`.
    pub fn name(self) -> &'static str {
        match self {
            Self::Gcc => "gcc",
            Self::Clang => "clang",
        }
    }

    /// The family of that name.
    pub fn parse(name: &str) -> Option<Self> {
        [Self::Gcc, Self::Clang].into_iter().find(|family| family.name() == name)
    }
}

/// A tool a test must ask for with `requires` before a step may use it:
/// every tool but the C compiler `gcc`, the binutils and the linkers.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Feature {
    Gxx,
    Gfortran,
    Clang,
    Yaml2obj,
    LlvmReadelf,
    /// `check gdb`: GNU's debugger.
    Gdb,
    /// `check lldb`: LLVM's debugger, of a pinned major.
    Lldb,
    /// `check elflint`: elfutils' `eu-elflint`.
    Elflint,
}

impl Feature {
    /// Every feature.
    pub const ALL: [Self; 8] = [
        Self::Gxx,
        Self::Gfortran,
        Self::Clang,
        Self::Yaml2obj,
        Self::LlvmReadelf,
        Self::Gdb,
        Self::Lldb,
        Self::Elflint,
    ];

    /// The word of a `requires` directive.
    pub fn name(self) -> &'static str {
        match self {
            Self::Gxx => "g++",
            Self::Gfortran => "gfortran",
            Self::Clang => "clang",
            Self::Yaml2obj => "yaml2obj",
            Self::LlvmReadelf => "llvm-readelf",
            Self::Gdb => "gdb",
            Self::Lldb => "lldb",
            Self::Elflint => "eu-elflint",
        }
    }

    /// The feature of that name.
    pub fn parse(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|feature| feature.name() == name)
    }
}

/// How a tool is looked up by name: [`mortar_testutil::find_tool`], or a
/// test's stand-in for it.
pub type Find = fn(&str) -> Option<PathBuf>;

/// How programs of an architecture are run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Runner {
    /// Directly: the architecture is the host's.
    Native,
    /// Under the user-mode emulator, `<qemu> -L <sysroot> <program>`.
    Emulated { qemu: PathBuf, sysroot: PathBuf },
}

/// The tools of one architecture. An architecture is in a run only when
/// all of these were found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ArchTools {
    pub arch: Arch,
    /// The C compiler driver, `gcc` or `<triple>-gcc`.
    pub gcc: PathBuf,
    pub ar: PathBuf,
    pub nm: PathBuf,
    pub objdump: PathBuf,
    pub readelf: PathBuf,
    /// The `meta` step's tool.
    pub objcopy: PathBuf,
    pub runner: Runner,
    /// The `requires: g++` tool.
    pub gxx: Option<PathBuf>,
    /// The `requires: gfortran` tool.
    pub gfortran: Option<PathBuf>,
    /// GNU ld for this architecture; `None` leaves `bfd` out of this
    /// architecture's matrix.
    pub bfd: Option<PathBuf>,
}

/// What could not be found for an architecture, with the Debian package
/// that has it when the architecture is not the host's.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Missing {
    pub what: String,
}

impl ArchTools {
    /// Finds the tools of `arch` on a `host` machine.
    ///
    /// # Errors
    /// The first tool every test needs that was not found: the
    /// architecture cannot be run.
    pub fn find(arch: Arch, host: Option<Arch>, find: Find) -> Result<Self, Missing> {
        let cross = host != Some(arch);
        let need = |tool: &str| {
            let name = arch.gnu_tool(host, tool);
            find(&name).ok_or_else(|| Missing { what: with_package(&name, arch, tool, cross) })
        };
        let runner = if cross {
            let name = format!("qemu-{}", arch.name());
            let qemu = find(&name)
                .or_else(|| find(&format!("{name}-static")))
                .ok_or_else(|| Missing { what: with_package(&name, arch, "emulator", true) })?;
            let sysroot = arch.sysroot(host);
            if !sysroot.is_dir() {
                let what = format!("the system root {}", sysroot.display());
                return Err(Missing { what: with_package(&what, arch, "sysroot", true) });
            }
            Runner::Emulated { qemu, sysroot }
        } else {
            Runner::Native
        };
        Ok(Self {
            arch,
            gcc: need("gcc")?,
            ar: need("ar")?,
            nm: need("nm")?,
            objdump: need("objdump")?,
            readelf: need("readelf")?,
            objcopy: need("objcopy")?,
            runner,
            gxx: find(&arch.gnu_tool(host, "g++")),
            gfortran: find(&arch.gnu_tool(host, "gfortran")),
            bfd: find(&arch.gnu_tool(host, "ld.bfd")),
        })
    }
}

/// `name`, with the Debian package that provides it for a cross target.
fn with_package(name: &str, arch: Arch, what: &str, cross: bool) -> String {
    if cross {
        format!("{name} (Debian package {})", arch.cross_package(what))
    } else {
        name.to_owned()
    }
}

/// The tools that are the same program for every architecture.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CommonTools {
    /// The `requires: clang` tools: both must be found.
    pub clang: Option<PathBuf>,
    pub clangxx: Option<PathBuf>,
    pub yaml2obj: Option<PathBuf>,
    pub llvm_readelf: Option<PathBuf>,
    pub lld: Option<PathBuf>,
    pub mold: Option<PathBuf>,
    /// The `requires: gdb` tool (the host's gdb).
    pub gdb: Option<PathBuf>,
    /// The `requires: lldb` tool, of a pinned LLVM major.
    pub lldb: Option<PathBuf>,
    /// The `requires: eu-elflint` tool.
    pub elflint: Option<PathBuf>,
}

impl CommonTools {
    /// Finds them. The LLVM tools are the newest pinned major
    /// ([`mortar_testutil::LLVM_MAJORS`]), never a plain `clang` or
    /// `ld.lld` of an unknown one.
    pub fn find(find: Find) -> Self {
        Self {
            clang: find("clang"),
            clangxx: find("clang++"),
            yaml2obj: find("yaml2obj"),
            llvm_readelf: find("llvm-readelf"),
            lld: find("ld.lld"),
            mold: find("mold").or_else(|| find("ld.mold")),
            gdb: find("gdb"),
            lldb: find("lldb"),
            elflint: find("eu-elflint"),
        }
    }
}

/// The name a missing [`Feature`] is reported by, for `arch` on `host`.
pub fn feature_tool_name(feature: Feature, arch: Arch, host: Option<Arch>) -> String {
    match feature {
        Feature::Gxx => with_package(&arch.gnu_tool(host, "g++"), arch, "g++", host != Some(arch)),
        Feature::Gfortran => {
            with_package(&arch.gnu_tool(host, "gfortran"), arch, "gfortran", host != Some(arch))
        }
        Feature::Clang => "clang and clang++ of a pinned LLVM major".to_owned(),
        Feature::Yaml2obj => "yaml2obj of a pinned LLVM major".to_owned(),
        Feature::LlvmReadelf => "llvm-readelf of a pinned LLVM major".to_owned(),
        Feature::Gdb => "gdb".to_owned(),
        Feature::Lldb => "lldb of a pinned LLVM major".to_owned(),
        Feature::Elflint => "eu-elflint (Debian package elfutils)".to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // spec/harness.md "Link tests", "Tools": the names of an
    // architecture's tools, its system root and its library directory.
    #[test]
    fn names_of_an_architectures_tools() {
        let host = Some(Arch::Aarch64);
        assert_eq!(Arch::Aarch64.gnu_tool(host, "gcc"), "gcc");
        assert_eq!(Arch::Aarch64.gnu_tool(host, "ld.bfd"), "ld.bfd");
        assert_eq!(Arch::X86_64.gnu_tool(host, "gcc"), "x86_64-linux-gnu-gcc");
        assert_eq!(Arch::I386.gnu_tool(host, "ld.bfd"), "i686-linux-gnu-ld.bfd");
        assert_eq!(Arch::I386.gnu_tool(host, "nm"), "i686-linux-gnu-nm");
        // On no known host, every architecture is a cross target.
        assert_eq!(Arch::Aarch64.gnu_tool(None, "g++"), "aarch64-linux-gnu-g++");
        assert_eq!(Arch::Aarch64.sysroot(host), Path::new("/"));
        assert_eq!(Arch::X86_64.sysroot(host), Path::new("/usr/x86_64-linux-gnu"));
        assert_eq!(Arch::I386.sysroot(host), Path::new("/usr/i686-linux-gnu"));
        assert_eq!(Arch::Aarch64.libdir(host), Path::new("/usr/lib/aarch64-linux-gnu"));
        assert_eq!(Arch::I386.libdir(host), Path::new("/usr/i686-linux-gnu/lib"));
        assert_eq!(Arch::Aarch64.loader(host), Path::new("/lib/ld-linux-aarch64.so.1"));
        assert_eq!(
            Arch::X86_64.loader(host),
            Path::new("/usr/x86_64-linux-gnu/lib64/ld-linux-x86-64.so.2")
        );
        assert_eq!(Arch::X86_64.cross_package("gcc"), "gcc-x86-64-linux-gnu");
        assert_eq!(Arch::I386.cross_package("ld.bfd"), "binutils-i686-linux-gnu");
        assert_eq!(Arch::I386.cross_package("sysroot"), "libc6-dev-i386-cross");
        assert_eq!(Arch::X86_64.cross_package("emulator"), "qemu-user");
    }

    // "The matrix": the names, and the order of a report.
    #[test]
    fn the_matrix_has_three_architectures_and_four_linkers() {
        let arches: Vec<&str> = Arch::ALL.iter().map(|arch| arch.name()).collect();
        assert_eq!(arches, ["aarch64", "x86_64", "i386"]);
        let linkers: Vec<&str> = Linker::ALL.iter().map(|linker| linker.name()).collect();
        assert_eq!(linkers, ["bfd", "lld", "mold", "mortar"]);
        assert_eq!(Linker::REFERENCES.map(Linker::is_reference), [true; 3]);
        assert!(!Linker::Mortar.is_reference());
        for arch in Arch::ALL {
            assert_eq!(Arch::parse(arch.name()), Some(arch));
        }
        for linker in Linker::ALL {
            assert_eq!(Linker::parse(linker.name()), Some(linker));
        }
        assert_eq!(Arch::parse("arm64"), None);
        assert_eq!(Linker::parse("gold"), None);
        assert_eq!(Family::parse("clang"), Some(Family::Clang));
        assert_eq!(Feature::parse("g++"), Some(Feature::Gxx));
        assert_eq!(Feature::parse("flang"), None);
    }

    // "Substitutions": the values that depend on the architecture alone.
    #[test]
    fn what_an_architecture_substitutes() {
        let rows = [
            (Arch::Aarch64, "aarch64linux", "/lib/ld-linux-aarch64.so.1", "ELF64", "AArch64"),
            (
                Arch::X86_64,
                "elf_x86_64",
                "/lib64/ld-linux-x86-64.so.2",
                "ELF64",
                "Advanced Micro Devices X86-64",
            ),
            (Arch::I386, "elf_i386", "/lib/ld-linux.so.2", "ELF32", "Intel 80386"),
        ];
        for (arch, emulation, loader, class, machine) in rows {
            assert_eq!(arch.emulation(), emulation);
            assert_eq!(arch.dynamic_linker(), loader);
            assert_eq!(arch.elf_class(), class);
            assert_eq!(arch.machine(), machine);
        }
        assert_eq!(Arch::I386.triple(), "i686-linux-gnu");
    }

    #[allow(clippy::unnecessary_wraps)] // A `Find`.
    fn nothing(_: &str) -> Option<PathBuf> {
        None
    }

    #[allow(clippy::unnecessary_wraps)] // A `Find`.
    fn everything(name: &str) -> Option<PathBuf> {
        Some(Path::new("/tools").join(name))
    }

    fn no_fortran(name: &str) -> Option<PathBuf> {
        (!name.contains("gfortran") && !name.contains("ld.bfd")).then(|| everything(name))?
    }

    // "Tools": an architecture needs its C compiler, binutils and, when
    // it is not the host's, its emulator and system root; the message
    // names the Debian package of a cross tool. `g++`, `gfortran` and
    // GNU ld are found or not.
    #[test]
    fn finding_an_architectures_tools() {
        let host = Some(Arch::Aarch64);
        let missing = ArchTools::find(Arch::Aarch64, host, nothing).unwrap_err();
        assert_eq!(missing.what, "gcc");
        let missing = ArchTools::find(Arch::X86_64, host, nothing).unwrap_err();
        assert_eq!(missing.what, "qemu-x86_64 (Debian package qemu-user)");

        let native = ArchTools::find(Arch::Aarch64, host, everything).unwrap();
        assert_eq!(native.runner, Runner::Native);
        assert_eq!(native.gcc, Path::new("/tools/gcc"));
        assert_eq!(native.bfd.as_deref(), Some(Path::new("/tools/ld.bfd")));
        assert_eq!(native.gfortran.as_deref(), Some(Path::new("/tools/gfortran")));

        let partial = ArchTools::find(Arch::Aarch64, host, no_fortran).unwrap();
        assert_eq!((partial.gfortran, partial.bfd), (None, None));
        assert_eq!(partial.gxx.as_deref(), Some(Path::new("/tools/g++")));

        // A cross architecture whose system root is not installed.
        let unknown = Some(Arch::I386);
        if !Arch::Aarch64.sysroot(unknown).is_dir() {
            let missing = ArchTools::find(Arch::Aarch64, unknown, everything).unwrap_err();
            assert_eq!(
                missing.what,
                "the system root /usr/aarch64-linux-gnu (Debian package libc6-dev-arm64-cross)"
            );
        }
        let common = CommonTools::find(everything);
        assert_eq!(common.lld.as_deref(), Some(Path::new("/tools/ld.lld")));
        assert_eq!(common.mold.as_deref(), Some(Path::new("/tools/mold")));
        assert_eq!(CommonTools::find(nothing), CommonTools::default());
        assert_eq!(
            feature_tool_name(Feature::Gfortran, Arch::I386, host),
            "i686-linux-gnu-gfortran (Debian package gfortran-i686-linux-gnu)"
        );
        assert_eq!(feature_tool_name(Feature::Gxx, Arch::Aarch64, host), "g++");
    }

    thread_local! {
        static ASKED: std::cell::RefCell<Vec<String>> = const { std::cell::RefCell::new(Vec::new()) };
    }

    fn recording(name: &str) -> Option<PathBuf> {
        ASKED.with(|asked| asked.borrow_mut().push(name.to_owned()));
        None
    }

    fn recording_all(name: &str) -> Option<PathBuf> {
        let _ = recording(name);
        everything(name)
    }

    // spec/harness.md "Hosted CI", "The tool guard": every name the run
    // looks a tool up by, for every architecture on either host and
    // whether or not the first name is found, is one scripts/ci-tools.sh
    // covers (the run itself looks them up with
    // `mortar_testutil::find_listed_tool`).
    #[test]
    fn the_tool_tables_are_in_ci_tools() {
        for host in [Some(Arch::Aarch64), Some(Arch::X86_64)] {
            for arch in Arch::ALL {
                for find in [recording as Find, recording_all] {
                    let _ = ArchTools::find(arch, host, find);
                }
                // What a run that finds them asks for (the cross ones stop
                // early on a machine without their system root).
                for name in ["gcc", "ar", "nm", "objdump", "readelf", "g++", "gfortran", "ld.bfd"] {
                    ASKED.with(|asked| asked.borrow_mut().push(arch.gnu_tool(host, name)));
                }
            }
        }
        let _ = CommonTools::find(recording);
        let _ = CommonTools::find(recording_all);
        let asked = ASKED.with(std::cell::RefCell::take);
        assert!(asked.iter().any(|name| name == "qemu-x86_64-static"), "{asked:?}");
        let unlisted: Vec<&String> =
            asked.iter().filter(|name| !mortar_testutil::ci_tools::covers(name)).collect();
        assert!(unlisted.is_empty(), "not in scripts/ci-tools.sh: {unlisted:?}");
    }
}
