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
#[allow(clippy::struct_excessive_bools)] // independent options of the line
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
    /// Incremental linking (§3.13; `spec/incr.md` IN-10).
    pub incremental: Incremental,
    /// `-Ur` (§3.16 C-R1) [M4-r]: GNU ld's "`-r`, and resolve C++
    /// constructors", which writes the same sections as `-r`; recorded for
    /// the map.
    pub ur: bool,
    /// `--force-group-allocation` (§3.16 C-R3; `spec/resolve.md` RV-R7,
    /// `spec/layout.md` L-R2, L-R3) [M4-r]: every COMDAT group dissolved
    /// after the election, no `SHT_GROUP` written.
    pub force_group_allocation: bool,
    /// `-d`, `-dc`, `-dp` (§3.16 C-R3; `spec/resolve.md` RV-R5,
    /// `spec/layout.md` L-R4) [M4-r]: commons allocated at `-r`.
    pub define_common: bool,
    /// `--unique` and `--unique=PATTERN`, in command-line order (§3.16
    /// C-R3; `spec/layout.md` L-R11) [M4-r]: `None` is the bare form.
    pub unique: Vec<Option<OsString>>,
}

impl Plan {
    /// The line's preemption options as `spec/dyn.md` D-112 resolves them
    /// (`spec/cli.md` §3.6) [M4]: the last `-Bsymbolic` form
    /// ([`Dynamic::bsymbolic`], none being `-Bno-symbolic`) and whether
    /// some `--dynamic-list` names a file. `None` when the last form is one
    /// D-112 refuses (`-Bsymbolic-non-weak`,
    /// `-Bsymbolic-non-weak-functions`): a `Shared` link refuses such a
    /// line before it asks (`spec/cli.md` §4.2). Read in a `Shared` link
    /// only; the other kinds ignore the forms (`spec/dyn.md` D-1).
    #[must_use]
    pub fn symbolic(&self) -> Option<SymbolicLine> {
        let form = match self.dynamic.bsymbolic {
            None => Symbolic::None,
            Some(Bsymbolic::All) => Symbolic::All,
            Some(Bsymbolic::Functions) => Symbolic::Functions,
            Some(Bsymbolic::NonWeak | Bsymbolic::NonWeakFunctions) => return None,
        };
        let has_dynamic_list = self.dynamic_lists().next().is_some();
        Some(SymbolicLine { form, has_dynamic_list })
    }

    /// The `--version-script` files, in command-line order: one script
    /// (`spec/script.md` §9.1) [M4].
    pub fn version_scripts(&self) -> impl Iterator<Item = &ScriptArg> {
        self.scripts.iter().filter(|s| s.role == ScriptRole::VersionScript)
    }

    /// The `--dynamic-list` files, in command-line order: one list
    /// (`spec/script.md` §9.1) [M4]. `--export-dynamic-symbol-list` is not
    /// one of them: it exports and does not decide preemption.
    pub fn dynamic_lists(&self) -> impl Iterator<Item = &ScriptArg> {
        self.scripts.iter().filter(|s| s.role == ScriptRole::DynamicList)
    }

    /// The `--wrap` symbols, in command-line order, each once
    /// (`spec/resolve.md` §7.1: several `--wrap` of one symbol are one).
    #[must_use]
    pub fn wraps(&self) -> Vec<&[u8]> {
        let mut out: Vec<&[u8]> = Vec::new();
        for s in self.symbols.iter().filter(|s| s.role == SymbolRole::Wrap) {
            let name = s.value.as_encoded_bytes();
            if !out.contains(&name) {
                out.push(name);
            }
        }
        out
    }

    /// The names options give that need a key row whether or not a file
    /// mentions them (`spec/model.md` §2.3), in option order: for each
    /// `--wrap=SYM`, `SYM`, `__wrap_SYM` and `__real_SYM`
    /// (`spec/resolve.md` §7.1). Unversioned names: the input phase adds
    /// a row for each one no file mentions, after the mentioned keys.
    #[must_use]
    pub fn option_keys(&self) -> Vec<Vec<u8>> {
        let mut out = Vec::new();
        for sym in self.wraps() {
            out.push(sym.to_vec());
            out.push([b"__wrap_", sym].concat());
            out.push([b"__real_", sym].concat());
        }
        // `--defsym=SYM=EXPR` (`spec/resolve.md` §7.2): `SYM`, and the
        // symbol `EXPR` names.
        for d in self.defsyms().into_iter().flatten() {
            out.push(d.sym.to_vec());
            if let Some(b) = d.expr.symbol() {
                out.push(b.to_vec());
            }
        }
        out
    }

    /// The `--defsym` options in command-line order, each parsed
    /// (`spec/resolve.md` §7.2); a value outside the accepted grammar is
    /// its refusal text.
    #[must_use]
    pub fn defsyms(&self) -> Vec<Result<Defsym<'_>, DefsymRefusal>> {
        self.symbols
            .iter()
            .filter(|s| s.role == SymbolRole::Defsym)
            .map(|s| Defsym::parse(s.value.as_encoded_bytes()))
            .collect()
    }
}

/// One `--defsym=SYM=EXPR` (`spec/resolve.md` §7.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Defsym<'a> {
    pub sym: &'a [u8],
    pub expr: DefsymExpr<'a>,
}

/// The `EXPR` forms the references agree on (§7.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DefsymExpr<'a> {
    /// `0x...` hexadecimal, or decimal without a leading `0`.
    Number(u64),
    /// A lone symbol: `SYM` is an alias of it.
    Symbol(&'a [u8]),
    /// `B + N` or `B - N`: `B`'s address plus the signed `N`, in `B`'s
    /// section.
    Offset(&'a [u8], i64),
}

impl<'a> DefsymExpr<'a> {
    /// The symbol the expression names, if any.
    #[must_use]
    pub fn symbol(self) -> Option<&'a [u8]> {
        match self {
            Self::Number(_) => None,
            Self::Symbol(b) | Self::Offset(b, _) => Some(b),
        }
    }
}

/// Why a `--defsym` value is refused (§7.2): its note.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DefsymRefusal {
    /// No `=`, or an empty `SYM`.
    Shape,
    /// A number with a leading `0` and more digits: GNU ld reads it
    /// octal, lld and mold decimal.
    LeadingZero,
    /// An expression beyond the agreed forms.
    Expression,
}

impl DefsymRefusal {
    /// The note that goes with the refusal.
    #[must_use]
    pub fn note(self) -> &'static str {
        match self {
            Self::Shape => "--defsym takes SYMBOL=EXPRESSION",
            Self::LeadingZero => {
                "GNU ld reads a number with a leading 0 as octal, lld and mold as decimal: \
                 write it in hexadecimal"
            }
            Self::Expression => {
                "mortar accepts a number (0x... or decimal), a symbol, or a symbol + or - a \
                 number (resolve.md §7.2)"
            }
        }
    }
}

fn defsym_number(t: &[u8]) -> Result<Option<u64>, DefsymRefusal> {
    let t = std::str::from_utf8(t).map_err(|_| DefsymRefusal::Expression)?;
    if let Some(hex) = t.strip_prefix("0x").or_else(|| t.strip_prefix("0X")) {
        if hex.is_empty() || !hex.bytes().all(|c| c.is_ascii_hexdigit()) {
            return Err(DefsymRefusal::Expression);
        }
        return u64::from_str_radix(hex, 16).map(Some).map_err(|_| DefsymRefusal::Expression);
    }
    if t.is_empty() || !t.bytes().all(|c| c.is_ascii_digit()) {
        return Ok(None);
    }
    if t.len() > 1 && t.starts_with('0') {
        return Err(DefsymRefusal::LeadingZero);
    }
    t.parse().map(Some).map_err(|_| DefsymRefusal::Expression)
}

fn defsym_symbol(t: &[u8]) -> bool {
    // The names a linker script expression reads as one symbol without
    // quotes, and that do not start with a digit.
    // `.` alone is a script's location counter.
    !t.is_empty()
        && t != b"."
        && !t[0].is_ascii_digit()
        && t.iter().all(|&c| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'.' | b'$'))
}

impl<'a> Defsym<'a> {
    /// Parses `SYM=EXPR`; blanks around the operator are allowed.
    ///
    /// # Errors
    ///
    /// The [`DefsymRefusal`] of a value outside the grammar.
    pub fn parse(value: &'a [u8]) -> Result<Self, DefsymRefusal> {
        let eq = value.iter().position(|&c| c == b'=').ok_or(DefsymRefusal::Shape)?;
        let sym = value[..eq].trim_ascii();
        let expr = value[eq + 1..].trim_ascii();
        if sym.is_empty() || expr.is_empty() {
            return Err(DefsymRefusal::Shape);
        }
        if let Some(n) = defsym_number(expr)? {
            return Ok(Self { sym, expr: DefsymExpr::Number(n) });
        }
        if defsym_symbol(expr) {
            return Ok(Self { sym, expr: DefsymExpr::Symbol(expr) });
        }
        let op =
            expr.iter().position(|&c| c == b'+' || c == b'-').ok_or(DefsymRefusal::Expression)?;
        let base = expr[..op].trim_ascii();
        let n = expr[op + 1..].trim_ascii();
        if !defsym_symbol(base) {
            return Err(DefsymRefusal::Expression);
        }
        let n = defsym_number(n)?.ok_or(DefsymRefusal::Expression)?;
        let n = i64::try_from(n).map_err(|_| DefsymRefusal::Expression)?;
        let n = if expr[op] == b'-' { -n } else { n };
        Ok(Self { sym, expr: DefsymExpr::Offset(base, n) })
    }
}

/// Incremental linking (`spec/cli.md` §3.13; `spec/incr.md` IN-10). The
/// default is a plain link, which never looks at a state (IN-11).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Incremental {
    /// `--incremental[=MODE]`; `None`: `--no-incremental`, the default.
    pub mode: Option<IncrementalMode>,
    /// `--incremental-state=PATH`; `None`: `<output>.mortar-state`.
    pub state: Option<OsString>,
    /// `--incremental-report[=FILE]`: `Some(None)` reports to standard
    /// error.
    pub report: Option<Option<OsString>>,
}

/// What `--incremental` asks for (`spec/incr.md` IN-10).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum IncrementalMode {
    /// `--incremental`: update if possible, else a full link.
    Auto,
    /// `--incremental=full`: a full link in incremental mode.
    Full,
    /// `--incremental=update`: update or fail.
    Update,
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
    /// `-E`, `--export-dynamic` (until `--no-export-dynamic`). Read in
    /// every dynamic kind, `Shared` included (`spec/resolve.md` §9.7 rule
    /// 12: it keeps a `.symver` definition exported) [M4].
    pub export_dynamic: bool,
    /// `-Bsymbolic` and its variants, the last wins; `-Bno-symbolic` clears
    /// it. What it means for a shared object is [`Plan::symbolic`]
    /// (`spec/dyn.md` D-112).
    pub bsymbolic: Option<Bsymbolic>,
    /// `--allow-shlib-undefined` (`true`), `--no-allow-shlib-undefined`
    /// (`false`); `None`: the default for the output kind
    /// (`spec/input.md` §8.6, `spec/resolve.md` §5.2).
    pub allow_shlib_undefined: Option<bool>,
    /// `--undefined-version` (`true`), `--no-undefined-version` (`false`);
    /// the last wins. `None`: accepted, as `true` (`spec/dyn.md` D-114:
    /// an exact `global:` entry naming no definition warns
    /// `version-script-undefined-name`; with `false` it is an error).
    pub undefined_version: Option<bool>,
    /// `-init SYMBOL` (`-init=SYMBOL`); the last wins. `None`: `DT_INIT`
    /// names `_init` when it is defined (`spec/dyn.md` D-30 row 3).
    pub init: Option<OsString>,
    /// `-fini SYMBOL` (`-fini=SYMBOL`); the last wins. `None`: `DT_FINI`
    /// names `_fini` when it is defined (`spec/dyn.md` D-30 row 4).
    pub fini: Option<OsString>,
}

impl Dynamic {
    /// The symbol `DT_INIT` names: `-init`'s, or `_init` (`spec/dyn.md`
    /// D-30 row 3; the tag is written only when the symbol is defined).
    #[must_use]
    pub fn init_symbol(&self) -> &std::ffi::OsStr {
        self.init.as_deref().unwrap_or("_init".as_ref())
    }

    /// The symbol `DT_FINI` names: `-fini`'s, or `_fini` (`spec/dyn.md`
    /// D-30 row 4).
    #[must_use]
    pub fn fini_symbol(&self) -> &std::ffi::OsStr {
        self.fini.as_deref().unwrap_or("_fini".as_ref())
    }

    /// Whether `-rpath` writes `DT_RUNPATH` (`true`, the default:
    /// `--enable-new-dtags`) or `DT_RPATH` (`--disable-new-dtags`; the
    /// last of the two wins; `spec/dyn.md` D-30 row 2, §2.3).
    #[must_use]
    pub fn runpath(&self) -> bool {
        self.new_dtags.unwrap_or(true)
    }
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

/// The line's preemption options as `spec/dyn.md` D-112 resolves them
/// (`spec/cli.md` §3.6) [M4]: the last of `-Bsymbolic`,
/// `-Bsymbolic-functions` and `-Bno-symbolic`, and whether the line has a
/// `--dynamic-list` (which then decides alone, whatever form comes before
/// or after it). Read by [`Plan::symbolic`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct SymbolicLine {
    /// The last form.
    pub form: Symbolic,
    /// Some `--dynamic-list` is on the line.
    pub has_dynamic_list: bool,
}

impl SymbolicLine {
    /// Whether `.dynamic` carries `DT_SYMBOLIC`/`DF_SYMBOLIC`: the last
    /// form is `-Bsymbolic` and the line has no `--dynamic-list`
    /// (`spec/dyn.md` D-112, D-102).
    #[must_use]
    pub const fn writes_symbolic_tags(self) -> bool {
        matches!(self.form, Symbolic::All) && !self.has_dynamic_list
    }
}

/// D-112's form of the line (`spec/dyn.md` D-112's table).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Symbolic {
    /// `-Bno-symbolic`, or no form: every exported definition preemptible
    /// (but those D-112 binds on every line).
    #[default]
    None,
    /// `-Bsymbolic`: every exported definition bound.
    All,
    /// `-Bsymbolic-functions`: `STT_FUNC` definitions bound.
    Functions,
}

/// `-Bsymbolic` and its variants, as the line spells them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Bsymbolic {
    /// `-Bsymbolic`.
    All,
    /// `-Bsymbolic-functions`.
    Functions,
    /// `-Bsymbolic-non-weak` (lld, mold). Refused in a shared object
    /// (`spec/dyn.md` D-112): no [`Symbolic`] form.
    NonWeak,
    /// `-Bsymbolic-non-weak-functions` (lld, mold). Refused like
    /// [`Bsymbolic::NonWeak`].
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
#[allow(clippy::struct_excessive_bools, reason = "one field per independent option")]
pub struct Shaping {
    /// `--gc-sections` (until `--no-gc-sections`).
    pub gc_sections: bool,
    /// `--print-gc-sections` (until `--no-print-gc-sections`).
    pub print_gc_sections: bool,
    /// `--gc-keep-exported`.
    pub gc_keep_exported: bool,
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
    /// `--sort-common[=ascending|descending]` (`spec/layout.md` L-88a);
    /// the last use wins.
    pub sort_common: Option<SortCommon>,
    /// `--sort-section=name|alignment` (`spec/layout.md` L-53a); the last
    /// use wins.
    pub sort_section: Option<SortSection>,
}

/// `--sort-common`: the direction of the commons' order by alignment.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum SortCommon {
    /// `--sort-common` alone, or `=descending`: the most aligned first.
    Descending,
    /// `=ascending`: the least aligned first.
    Ascending,
}

/// `--sort-section`: the key of the input sections' order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum SortSection {
    /// `name`: by section name, byte order.
    Name,
    /// `alignment`: by `sh_addralign`, the largest first.
    Alignment,
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
    /// The one-definition-rule checks (`spec/odr.md` O-40, O-41, O-44).
    pub odr: Odr,
}

/// The one-definition-rule switches (`spec/odr.md` §9.2).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Odr {
    /// `--odr=`, `--no-odr-check`; the last one given wins.
    pub mode: OdrMode,
    /// `--odr-check-archives` (O-23): also compare archive members that
    /// are not linked (WP-076).
    pub check_archives: bool,
    /// `--odr-table=` (O-50): a dynamic executable's `.note.mason`;
    /// `exported` since WP-075, `full` (a second note) since WP-076.
    pub table: OdrTable,
    /// `--odr-stats` (O-44): one line to standard error.
    pub stats: bool,
    /// `--keep-mason-meta` (`spec/meta.md` MT-32): keep the inputs' Mason
    /// metadata in the output, as a non-allocated `.mason.meta` (WP-076).
    pub keep_meta: bool,
}

/// `--odr-table=` (`spec/odr.md` O-50, `spec/cli.md` §3.9).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum OdrTable {
    /// `off`, the default: no table is written.
    #[default]
    Off,
    /// `exported`.
    Exported,
    /// `full`.
    Full,
}

/// `--odr=` (`spec/odr.md` O-40, O-41).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum OdrMode {
    /// `--odr=warn`, the default: the checks O-47 lists as on run, and
    /// their findings are warnings.
    #[default]
    Warn,
    /// `--odr=error`: as `Warn`, with the record-based findings errors.
    Error,
    /// `--odr=strict`: every check runs.
    Strict,
    /// `--odr=off`, `--no-odr-check`: no check runs and no container byte
    /// is read (O-42).
    Off,
}

impl OdrMode {
    /// The word of `--odr=`.
    #[must_use]
    pub const fn word(self) -> &'static str {
        match self {
            Self::Warn => "warn",
            Self::Error => "error",
            Self::Strict => "strict",
            Self::Off => "off",
        }
    }
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
    /// `--mortar-map file` (mortar's own): the machine map, `mortar-map 2`.
    pub mortar_map: Option<OsString>,
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
    /// Its value, if it has one (`full` of `--fix-cortex-a53-843419=full`).
    pub value: Option<OsString>,
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

#[cfg(test)]
mod tests {
    use super::*;

    fn script(arg: u32, role: ScriptRole) -> ScriptArg {
        ScriptArg { arg, role, path: format!("f{arg}").into() }
    }

    // spec: dyn.md D-112 (the last of -Bsymbolic, -Bsymbolic-functions and
    // -Bno-symbolic wins; a --dynamic-list decides alone; DT_SYMBOLIC only
    // for -Bsymbolic without a list), cli.md §3.6
    #[test]
    fn d112_symbolic_form_of_the_line() {
        let mut plan = Plan::default();
        let line = |p: &Plan| p.symbolic().map(|l| (l.form, l.has_dynamic_list));
        assert_eq!(line(&plan), Some((Symbolic::None, false)));
        plan.dynamic.bsymbolic = Some(Bsymbolic::Functions);
        assert_eq!(line(&plan), Some((Symbolic::Functions, false)));
        plan.dynamic.bsymbolic = Some(Bsymbolic::All);
        assert!(plan.symbolic().unwrap().writes_symbolic_tags());
        plan.scripts = vec![
            script(1, ScriptRole::VersionScript),
            script(2, ScriptRole::ExportDynamicSymbolList),
            script(3, ScriptRole::DynamicList),
            script(4, ScriptRole::VersionScript),
            script(5, ScriptRole::DynamicList),
        ];
        assert_eq!(line(&plan), Some((Symbolic::All, true)));
        assert!(!plan.symbolic().unwrap().writes_symbolic_tags());
        let args = |it: &mut dyn Iterator<Item = &ScriptArg>| it.map(|s| s.arg).collect::<Vec<_>>();
        assert_eq!(args(&mut plan.version_scripts()), [1, 4]);
        assert_eq!(args(&mut plan.dynamic_lists()), [3, 5]);
        for refused in [Bsymbolic::NonWeak, Bsymbolic::NonWeakFunctions] {
            plan.dynamic.bsymbolic = Some(refused);
            assert_eq!(plan.symbolic(), None);
        }
    }
}
