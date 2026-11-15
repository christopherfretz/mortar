//! The link plan: what the command line asks for, as plain data
//! (`spec/cli.md` §3). Nothing here reads a file or decides how to link;
//! later phases read the plan.
//!
//! The types live in `mortar_core` so that every phase and the link state
//! name them without depending on the parser (`spec/model.md` §3.1);
//! `mortar_cli` writes a [`Plan`] and re-exports them.

use std::ffi::OsString;

/// What the command line asks for. Built by `mortar_cli::parse`; every field
/// is filled whatever the option's status (`spec/cli.md` §3.1).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Plan {
    /// `-o`, `--output`: the output path. `None`: the default, `a.out`.
    pub output: Option<OsString>,
    /// What kind of file the link produces (§3.2).
    pub kind: OutputKind,
    /// `-m`: the emulation, which names the target. `None`: not given; the
    /// target is inferred from the first input by a later phase.
    pub emulation: Option<Emulation>,
    /// `-e`, `--entry`: the entry symbol.
    pub entry: Option<OsString>,
    /// The inputs, in command-line order, each with the positional state
    /// in force where it appeared (§3.3).
    pub inputs: Vec<Input>,
    /// `-L`, `--library-path`, in order.
    pub library_paths: Vec<SearchPath>,
    /// `--sysroot`.
    pub sysroot: Option<OsString>,
    /// `-nostdlib`: the `SEARCH_DIR` directories of linker scripts are not
    /// searched (`spec/input.md` §4.2).
    pub nostdlib: bool,
    /// Scripts and lists named by options, in order (§3.4).
    pub scripts: Vec<ScriptArg>,
    /// Options that name symbols, in order (§3.5).
    pub symbols: Vec<SymbolArg>,
    /// Dynamic linking (§3.6).
    pub dynamic: Dynamic,
    /// The typed `-z` keywords (§3.7).
    pub z: ZOptions,
    /// Output shaping (§3.8).
    pub shaping: Shaping,
    /// Diagnostic switches (§3.9).
    pub diagnostics: Diagnostics,
    /// Bookkeeping (§3.10).
    pub bookkeeping: Bookkeeping,
    /// `-plugin`, `-plugin-opt` (§3.11).
    pub plugin: Plugin,
    /// Options that belong to some targets only, for the check that runs
    /// when the target is known (§2.9).
    pub target_options: Vec<TargetOption>,
    /// Every other option used, in order: those without a typed field
    /// (§3.12).
    pub other: Vec<OtherOption>,
    /// `--version` or `--help`: answer and stop (§5).
    pub exit: Option<Exit>,
    /// `-v` or `-V`: print the version, then go on (§5).
    pub print_version: Option<VersionStyle>,
}

/// The kind of the output (`spec/cli.md` §3.2).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum OutputKind {
    /// A position-dependent executable with a program interpreter
    /// (`-dynamic-linker`), not `-pie`: GNU ld's and lld's default.
    #[default]
    Executable,
    /// A position-independent executable with a program interpreter
    /// (`-pie -dynamic-linker`).
    PieExecutable,
    /// A position-dependent executable for which the command line names no
    /// program interpreter (no `-dynamic-linker`, or
    /// `--no-dynamic-linker`). GNU ld gives such an executable its default
    /// interpreter when a shared object is linked; the phase that reads
    /// the inputs makes that decision (`spec/cli.md` §3.2).
    StaticExecutable,
    /// `-pie` with no program interpreter named (`gcc -static-pie` passes
    /// `-static -pie --no-dynamic-linker`).
    StaticPie,
    /// A shared object (`-shared`).
    Shared,
    /// A relocatable object (`-r`).
    Relocatable,
}

/// An emulation mortar links for (`-m`; `spec/cli.md` §2.8).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Emulation {
    /// `aarch64linux`: AArch64 Linux.
    Aarch64Linux,
    /// `aarch64elf`: AArch64, no operating system.
    Aarch64Elf,
    /// `elf_x86_64`: x86-64 Linux.
    ElfX86_64,
    /// `elf_i386`: 32-bit x86 Linux.
    ElfI386,
}

impl Emulation {
    /// Every emulation, in the order `-V` lists them.
    pub const ALL: [Self; 4] =
        [Self::Aarch64Linux, Self::Aarch64Elf, Self::ElfX86_64, Self::ElfI386];

    /// The name `-m` takes.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Aarch64Linux => "aarch64linux",
            Self::Aarch64Elf => "aarch64elf",
            Self::ElfX86_64 => "elf_x86_64",
            Self::ElfI386 => "elf_i386",
        }
    }

    /// The emulation named `name`, if mortar links for it.
    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|e| e.name() == name)
    }

    /// The target the emulation links for.
    #[must_use]
    pub const fn target(self) -> Target {
        match self {
            Self::Aarch64Linux | Self::Aarch64Elf => Target::Aarch64,
            Self::ElfX86_64 => Target::X86_64,
            Self::ElfI386 => Target::I386,
        }
    }
}

/// A target architecture.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Target {
    /// AArch64 (ELF64, little-endian).
    Aarch64,
    /// x86-64 (ELF64).
    X86_64,
    /// 32-bit x86 (ELF32).
    I386,
}

impl Target {
    /// Every target.
    pub const ALL: [Self; 3] = [Self::Aarch64, Self::X86_64, Self::I386];

    /// The target's name in the option table and the messages.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Aarch64 => "AArch64",
            Self::X86_64 => "x86-64",
            Self::I386 => "i386",
        }
    }

    /// The emulations of the target, the Linux one first.
    #[must_use]
    pub const fn emulations(self) -> &'static [Emulation] {
        match self {
            Self::Aarch64 => &[Emulation::Aarch64Linux, Emulation::Aarch64Elf],
            Self::X86_64 => &[Emulation::ElfX86_64],
            Self::I386 => &[Emulation::ElfI386],
        }
    }
}

/// One input of the link (`spec/cli.md` §3.3).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Input {
    /// The index of the argument that named it, after response files are
    /// expanded (§2.6). Diagnostics about the input key on it.
    pub arg: u32,
    /// How it was named.
    pub spec: InputSpec,
    /// The positional state in force where it appeared.
    pub state: InputState,
}

/// How an input was named.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum InputSpec {
    /// A path (an argument that is not an option, `-` included).
    Path(OsString),
    /// `-l name`: the text after `-l`.
    Library(OsString),
    /// `-l :file`: the file name after the colon, searched as is.
    LibraryFile(OsString),
    /// `-R file`, `--just-symbols=file`: only the symbols of the file. (GNU
    /// ld reads a directory given to `-R` as `-rpath`; that is decided where
    /// files are opened, `spec/cli.md` §3.3.)
    JustSymbols(OsString),
}

/// The positional state an input is read under (`spec/cli.md` §3.3).
#[derive(Debug, Clone, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[allow(clippy::struct_excessive_bools, reason = "one field per switch of the command line")]
pub struct InputState {
    /// `--as-needed` (until `--no-as-needed`).
    pub as_needed: bool,
    /// `--whole-archive` (until `--no-whole-archive`).
    pub whole_archive: bool,
    /// `-Bstatic`, `-static`, `-dn`, `-non_shared` (until `-Bdynamic`):
    /// `-l` finds archives only. It does not make the output static
    /// (§3.2).
    pub link_static: bool,
    /// `--copy-dt-needed-entries` (until `--no-copy-dt-needed-entries`).
    pub copy_dt_needed: bool,
    /// The ordinal of the outermost `--start-group` the input is in.
    pub group: Option<u32>,
    /// The ordinal of the `--start-lib` region the input is in.
    pub lib: Option<u32>,
    /// `-b`, `--format`: the format the input is read as.
    pub format: InputFormat,
}

/// The format an input is read as (`-b`, `--format`).
#[derive(Debug, Clone, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum InputFormat {
    /// Found from the file's contents (`default`; also lld's `elf`).
    #[default]
    Detect,
    /// `binary`: the bytes become a data section.
    Binary,
    /// Any other BFD target name.
    Named(OsString),
}

/// A library search directory (`-L`; `spec/cli.md` §3.3).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SearchPath {
    /// The argument index.
    pub arg: u32,
    /// The directory, without the sysroot prefix.
    pub path: OsString,
    /// The directory started with `=` or `$SYSROOT`: it is inside the
    /// sysroot.
    pub in_sysroot: bool,
}

/// A script or list file named by an option (`spec/cli.md` §3.4).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ScriptArg {
    /// The argument index.
    pub arg: u32,
    /// What the file is.
    pub role: ScriptRole,
    /// The path as given.
    pub path: OsString,
}

/// What a script file named by an option is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ScriptRole {
    /// `-T`, `--script`.
    Script,
    /// `-dT`, `--default-script`.
    DefaultScript,
    /// `--version-script`.
    VersionScript,
    /// `--dynamic-list`.
    DynamicList,
    /// `--export-dynamic-symbol-list`.
    ExportDynamicSymbolList,
    /// `--retain-symbols-file`.
    RetainSymbolsFile,
}

/// An option that names a symbol or a pattern (`spec/cli.md` §3.5).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SymbolArg {
    /// The argument index.
    pub arg: u32,
    /// What the option does with it.
    pub role: SymbolRole,
    /// The value as given (for `--defsym`, the unparsed `symbol=expression`).
    pub value: OsString,
}

/// What an option does with the symbol it names.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum SymbolRole {
    /// `-u`, `--undefined`.
    Undefined,
    /// `--require-defined`.
    RequireDefined,
    /// `--defsym`: `symbol=expression`, unparsed.
    Defsym,
    /// `--wrap`.
    Wrap,
    /// `--export-dynamic-symbol`.
    ExportDynamicSymbol,
    /// `--exclude-libs`: a comma-separated list of archive names, unsplit.
    ExcludeLibs,
    /// `-y`, `--trace-symbol`.
    TraceSymbol,
    /// `--ignore-unresolved-symbol`.
    IgnoreUnresolved,
}

/// Dynamic linking options (`spec/cli.md` §3.6).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Dynamic {
    /// `-h`, `-soname`.
    pub soname: Option<OsString>,
    /// `-rpath`, in order (GNU ld joins them with `:`).
    pub rpath: Vec<OsString>,
    /// `-rpath-link`, in order, each as written: `mortar_input` splits a
    /// `:`-separated list (`spec/input.md` §8.6).
    pub rpath_link: Vec<OsString>,
    /// `-I`, `--dynamic-linker`, `--no-dynamic-linker`; the last wins.
    pub dynamic_linker: Option<DynamicLinker>,
    /// `--enable-new-dtags` (`true`), `--disable-new-dtags` (`false`).
    pub new_dtags: Option<bool>,
    /// `--hash-style`.
    pub hash_style: Option<HashStyle>,
    /// `--pack-dyn-relocs`.
    pub pack_dyn_relocs: Option<PackDynRelocs>,
    /// `-E`, `--export-dynamic` (until `--no-export-dynamic`).
    pub export_dynamic: bool,
    /// `-Bsymbolic` and its variants; `-Bno-symbolic` clears it.
    pub bsymbolic: Option<Bsymbolic>,
    /// `--allow-shlib-undefined` (`true`), `--no-allow-shlib-undefined`
    /// (`false`); `None`: the default for the output kind
    /// (`spec/input.md` §8.6, `spec/resolve.md` §5.2).
    pub allow_shlib_undefined: Option<bool>,
}

/// The program interpreter.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum DynamicLinker {
    /// `-I path`, `--dynamic-linker path`.
    Path(OsString),
    /// `--no-dynamic-linker`: no `PT_INTERP`.
    None,
}

/// `--hash-style`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum HashStyle {
    /// `sysv`: `DT_HASH`.
    Sysv,
    /// `gnu`: `DT_GNU_HASH`.
    Gnu,
    /// `both`.
    Both,
    /// `none` (mold): neither.
    None,
}

/// `--pack-dyn-relocs`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum PackDynRelocs {
    /// `none`.
    None,
    /// `relr`: `DT_RELR`.
    Relr,
    /// `android`.
    Android,
    /// `android+relr`.
    AndroidRelr,
}

/// `-Bsymbolic` and its variants.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Bsymbolic {
    /// `-Bsymbolic`.
    All,
    /// `-Bsymbolic-functions`.
    Functions,
    /// `-Bsymbolic-non-weak` (lld, mold).
    NonWeak,
    /// `-Bsymbolic-non-weak-functions` (lld, mold).
    NonWeakFunctions,
}

/// The `-z` keywords with a typed meaning (`spec/cli.md` §3.7). A keyword
/// and its opposite set one field; the last wins. The other keywords land
/// in [`Plan::other`] as `-z <keyword>`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ZOptions {
    /// `-z now` (`true`), `-z lazy` (`false`).
    pub now: Option<bool>,
    /// `-z relro`, `-z norelro`.
    pub relro: Option<bool>,
    /// `-z execstack`, `-z noexecstack`.
    pub execstack: Option<bool>,
    /// `-z text` (`true`), `-z notext` and `-z textoff` (`false`).
    pub text: Option<bool>,
    /// `-z separate-code`, `-z noseparate-code`.
    pub separate_code: Option<bool>,
    /// `-z defs`, `--no-undefined` (`true`); `-z undefs` (`false`).
    pub defs: Option<bool>,
    /// `-z max-page-size=`: the value as given.
    pub max_page_size: Option<OsString>,
    /// `-z common-page-size=`: the value as given.
    pub common_page_size: Option<OsString>,
    /// `-z stack-size=`: the value as given.
    pub stack_size: Option<OsString>,
    /// `-z pack-relative-relocs`, `-z nopack-relative-relocs`.
    pub pack_relative_relocs: Option<bool>,
    /// `-z copyreloc` (lld), `-z nocopyreloc`.
    pub copyreloc: Option<bool>,
    /// `-z combreloc`, `-z nocombreloc`.
    pub combreloc: Option<bool>,
    /// `-z sectionheader`, `-z nosectionheader`.
    pub sectionheader: Option<bool>,
    /// `-z memory-seal`, `-z nomemory-seal` (GNU ld 2.44; `spec/synth.md`
    /// S-95's `MEMORY_SEAL` property, R-C3).
    pub memory_seal: Option<bool>,
}

/// Output shaping (`spec/cli.md` §3.8).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Shaping {
    /// `--gc-sections` (until `--no-gc-sections`).
    pub gc_sections: bool,
    /// `--icf=`; `--no-icf` is `none`.
    pub icf: Option<Icf>,
    /// `--build-id[=style]`; `--no-build-id` and `--build-id=none` clear it.
    pub build_id: Option<BuildId>,
    /// `-s`, `-S`, and the last of them wins (GNU ld: `-s` implies `-S`).
    pub strip: Option<Strip>,
    /// `-x`, `-X`, `--discard-none`.
    pub discard: Option<Discard>,
    /// `--compress-debug-sections=`.
    pub compress_debug_sections: Option<Compression>,
    /// `--eh-frame-hdr` (`true`), `--no-eh-frame-hdr` (`false`).
    pub eh_frame_hdr: Option<bool>,
    /// `-N`, `-n`, `--no-omagic`, `--no-nmagic`.
    pub magic: Option<Magic>,
    /// `--image-base`: the value as given.
    pub image_base: Option<OsString>,
    /// `-Ttext`, `-Tdata`, `-Tbss`, `--section-start`, `-T*-segment`, in
    /// order; values as given.
    pub starts: Vec<Start>,
    /// `-q`, `--emit-relocs`.
    pub emit_relocs: bool,
    /// `--gdb-index` (`true`), `--no-gdb-index` (`false`).
    pub gdb_index: Option<bool>,
}

/// `--icf`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Icf {
    /// `none`.
    None,
    /// `safe`.
    Safe,
    /// `all`.
    All,
}

/// `--build-id`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum BuildId {
    /// `--build-id` alone: GNU ld's default style, `sha1`.
    Default,
    /// `md5`.
    Md5,
    /// `sha1`.
    Sha1,
    /// `sha256` (mold).
    Sha256,
    /// `fast` (lld, mold).
    Fast,
    /// `uuid`.
    Uuid,
    /// `0x<hex>`: the given bytes, as the option's text (`0x` included;
    /// `spec/image.md` I-70 is checked by the command line and again by
    /// the note's plan).
    Hex(String),
}

/// What `-s` and `-S` strip.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Strip {
    /// `-S`, `--strip-debug`.
    Debug,
    /// `-s`, `--strip-all`.
    All,
}

/// Which local symbols are discarded.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Discard {
    /// `--discard-none`.
    None,
    /// `-X`, `--discard-locals`: temporary locals.
    Locals,
    /// `-x`, `--discard-all`.
    All,
}

/// `--compress-debug-sections`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Compression {
    /// `none`.
    None,
    /// `zlib`, `zlib-gabi`.
    Zlib,
    /// `zlib-gnu`: the obsolete `.zdebug` form.
    ZlibGnu,
    /// `zstd`.
    Zstd,
}

/// `-N`, `-n`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Magic {
    /// Page-aligned, read-only text: `--no-omagic`, `--no-nmagic`.
    Paged,
    /// `-n`, `--nmagic`.
    Nmagic,
    /// `-N`, `--omagic`.
    Omagic,
}

/// An address given for a section or a segment.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Start {
    /// The argument index.
    pub arg: u32,
    /// What starts there.
    pub what: StartOf,
    /// The address as given (GNU ld reads it as hexadecimal).
    pub address: OsString,
}

/// What an address option places.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum StartOf {
    /// A section: `-Ttext` is `.text`, `-Tdata` `.data`, `-Tbss` `.bss`,
    /// `--section-start=name=address` names it.
    Section(OsString),
    /// `-Ttext-segment`.
    TextSegment,
    /// `-Trodata-segment`.
    RodataSegment,
    /// `-Tldata-segment`.
    LdataSegment,
}

/// Diagnostic switches (`spec/cli.md` §3.9).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[allow(clippy::struct_excessive_bools, reason = "one field per switch of the command line")]
pub struct Diagnostics {
    /// `--error-limit=N`; `None`: the default (`mortar_diag`'s 20).
    pub error_limit: Option<usize>,
    /// `--fatal-warnings` (until `--no-fatal-warnings`).
    pub fatal_warnings: bool,
    /// `-w`, `--no-warnings`.
    pub no_warnings: bool,
    /// `--color-diagnostics[=when]`, `--no-color-diagnostics`.
    pub color: Color,
    /// `--unresolved-symbols=`.
    pub unresolved_symbols: Option<UnresolvedSymbols>,
    /// `--warn-unresolved-symbols` (`true`), `--error-unresolved-symbols`
    /// (`false`).
    pub warn_unresolved_symbols: Option<bool>,
    /// `--allow-multiple-definition`, `-z muldefs` (until
    /// `--no-allow-multiple-definition`).
    pub allow_multiple_definition: bool,
    /// `--noinhibit-exec`.
    pub noinhibit_exec: bool,
    /// `--warn-backrefs` (until `--no-warn-backrefs`): report
    /// `archive-backref` (`spec/resolve.md` §4.3).
    pub warn_backrefs: bool,
    /// `--warn-common` (until `--no-warn-common`): report the common-symbol
    /// kinds (`spec/resolve.md` §3.3).
    pub warn_common: bool,
}

/// When diagnostics are colored.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Color {
    /// When standard error is a terminal (the default).
    #[default]
    Auto,
    /// Always (`--color-diagnostics`, `=always`).
    Always,
    /// Never (`=never`, `--no-color-diagnostics`).
    Never,
}

/// `--unresolved-symbols=`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum UnresolvedSymbols {
    /// `report-all`.
    ReportAll,
    /// `ignore-all`.
    IgnoreAll,
    /// `ignore-in-object-files`.
    IgnoreInObjectFiles,
    /// `ignore-in-shared-libs`.
    IgnoreInSharedLibs,
}

/// Bookkeeping (`spec/cli.md` §3.10).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[allow(clippy::struct_excessive_bools, reason = "one field per switch of the command line")]
pub struct Bookkeeping {
    /// `-Map file`.
    pub map: Option<OsString>,
    /// `-M`, `--print-map`.
    pub print_map: bool,
    /// `--cref`.
    pub cref: bool,
    /// `--reproduce=file`.
    pub reproduce: Option<OsString>,
    /// `--threads[=N]`, `--thread-count N`, `--no-threads`.
    pub threads: Option<Threads>,
    /// `--time-phases` (mortar's own).
    pub time_phases: bool,
    /// `--verify` (mortar's own).
    pub verify: bool,
    /// `--verbose[=level]`.
    pub verbose: bool,
    /// `-t`, `--trace`.
    pub trace: bool,
    /// `--dependency-file=file`.
    pub dependency_file: Option<OsString>,
}

/// How many threads a link may use.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Threads {
    /// `--threads` alone: as many as there are processors.
    All,
    /// `--threads=N`, `--thread-count N`; `--no-threads` is `1`.
    Count(u32),
}

/// The linker-plugin options GCC passes on every link (`spec/cli.md`
/// §3.11).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Plugin {
    /// `-plugin path`: the last one given.
    pub path: Option<OsString>,
    /// `-plugin-opt value`, in order.
    pub options: Vec<OsString>,
}

/// An option that belongs to some targets only (`spec/cli.md` §2.9),
/// recorded so that it is checked once the target is known.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TargetOption {
    /// The argument index.
    pub arg: u32,
    /// The option as the user wrote it (`--fix-cortex-a53-843419`,
    /// `-z lam-u48`).
    pub option: String,
    /// The targets it belongs to.
    pub targets: Vec<Target>,
}

/// An option without a typed field (`spec/cli.md` §3.12).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct OtherOption {
    /// The argument index.
    pub arg: u32,
    /// The table row's name (`--warn-common`, `-z nodelete`).
    pub name: String,
    /// The value, if the option took one.
    pub value: Option<OsString>,
}

/// A request that is answered instead of linking (`spec/cli.md` §5).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Exit {
    /// `--version`.
    Version,
    /// `--help`.
    Help,
}

/// What `-v` and `-V` print.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum VersionStyle {
    /// `-v`: the version line.
    Version,
    /// `-V`: the version line and the emulations.
    Emulations,
}
