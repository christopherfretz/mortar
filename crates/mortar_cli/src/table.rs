//! The option table: one row per option, with its grammar and its status
//! (`spec/cli.md` §4). The rows are in `rows.rs`.

use crate::plan::{Bsymbolic, Discard, Magic, ScriptRole, Strip, SymbolRole, Target, VersionStyle};

/// One option of the command line (`spec/cli.md` §4.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Row {
    /// The spellings. The first is the row's name in messages, the
    /// generated table and `--help`.
    pub spellings: &'static [Spelling],
    /// Whether it takes a value, and which.
    pub arg: Arg,
    /// What `--help` calls the value (`FILE`, `SYMBOL`).
    pub meta: &'static str,
    /// The reference linkers that accept it.
    pub linkers: Linkers,
    /// The targets it belongs to; empty: every target (§2.9).
    pub targets: &'static [Target],
    /// The `--help` group.
    pub group: Group,
    /// Whether mortar honors it (§4.2).
    pub status: Status,
    /// Where it lands in the `Plan`.
    pub lands: Lands,
    /// One line for `--help` and the generated table.
    pub help: &'static str,
}

/// One spelling of an option (`spec/cli.md` §2.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Spelling {
    /// The name without dashes: `o`, `output`, `Bstatic`; for a `-z`
    /// keyword the keyword (`now`).
    pub text: &'static str,
    /// How it is written.
    pub form: Form,
    /// GNU ld accepts this spelling: it can be abbreviated (§2.4).
    pub gnu: bool,
}

/// How a spelling is written (`spec/cli.md` §2.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Form {
    /// One character after one dash: `-o`, `-(`.
    Short,
    /// A name after one or two dashes, shown with one: `-shared`.
    Single,
    /// A name after one or two dashes, shown with two: `--gc-sections`.
    Long,
    /// A name after exactly two dashes: `--output`, `--omagic` (§2.2).
    LongTwo,
    /// A keyword after `-z`: `-z now`, `-znow`.
    Z,
}

impl Spelling {
    /// A short option (`-o`); `gnu`: GNU ld has it.
    #[must_use]
    pub const fn short(text: &'static str, gnu: bool) -> Self {
        Self { text, form: Form::Short, gnu }
    }

    /// A long option shown with two dashes. A name starting with `l`, `m`
    /// or `o` takes exactly two (§2.2); any other one or two.
    #[must_use]
    pub const fn long(text: &'static str, gnu: bool) -> Self {
        let form = match text.as_bytes()[0] {
            b'l' | b'm' | b'o' => Form::LongTwo,
            _ => Form::Long,
        };
        Self { text, form, gnu }
    }

    /// A long option GNU ld shows with one dash (`-shared`, `-rpath`).
    #[must_use]
    pub const fn single(text: &'static str, gnu: bool) -> Self {
        Self { text, form: Form::Single, gnu }
    }

    /// A GNU long option that takes exactly two dashes although its name
    /// does not start with `l`, `m` or `o` (none today; kept for the rule).
    #[must_use]
    pub const fn two_dashes(text: &'static str) -> Self {
        Self { text, form: Form::LongTwo, gnu: true }
    }

    /// A `-z` keyword.
    #[must_use]
    pub const fn z(text: &'static str, gnu: bool) -> Self {
        Self { text, form: Form::Z, gnu }
    }

    /// Whether the spelling is a long name, matched against `-name` or
    /// `--name`.
    #[must_use]
    pub const fn is_long(&self) -> bool {
        matches!(self.form, Form::Single | Form::Long | Form::LongTwo)
    }

    /// Whether the spelling can be written after `dashes` dashes.
    #[must_use]
    pub const fn takes_dashes(&self, dashes: usize) -> bool {
        match self.form {
            Form::Single | Form::Long => dashes == 1 || dashes == 2,
            Form::LongTwo => dashes == 2,
            Form::Short | Form::Z => dashes == 1,
        }
    }

    /// The spelling as `--help` shows it: `-o`, `-shared`, `--output`,
    /// `-z now`.
    #[must_use]
    pub fn display(&self) -> String {
        match self.form {
            Form::Short | Form::Single => format!("-{}", self.text),
            Form::Long | Form::LongTwo => format!("--{}", self.text),
            Form::Z => format!("-z {}", self.text),
        }
    }
}

/// Whether an option takes a value (`spec/cli.md` §2.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Arg {
    /// No value; `--opt=v` is an unknown option.
    None,
    /// A value: joined (`-ofile`, `--output=file`, `-z k=v`) or the next
    /// argument (`-o file`, `--output file`).
    Required(Values),
    /// A value only when joined with `=`: `--build-id=sha1`; `--build-id
    /// sha1` is the option and an input.
    Optional(Values),
    /// A value glued to the name, two dashes: lld's `--lto-O2`.
    Glued(Values),
    /// Every remaining argument (mold's `--run`).
    Rest,
}

/// The values an option accepts (`spec/cli.md` §2.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Values {
    /// Any text.
    Any,
    /// One of these words.
    Choice(&'static [&'static str]),
    /// An unsigned decimal number.
    Count,
    /// `--build-id`'s: a style word, or `0x` and hexadecimal digits.
    BuildId,
}

/// A set of reference linkers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Linkers(u8);

impl Linkers {
    /// None of them: an option of mortar's own.
    pub const MORTAR: Self = Self(0);
    /// GNU ld.
    pub const G: Self = Self(1);
    /// lld.
    pub const L: Self = Self(2);
    /// mold.
    pub const M: Self = Self(4);
    /// GNU ld and lld.
    pub const GL: Self = Self(3);
    /// GNU ld and mold.
    pub const GM: Self = Self(5);
    /// lld and mold.
    pub const LM: Self = Self(6);
    /// All three.
    pub const GLM: Self = Self(7);

    /// Whether GNU ld accepts the option.
    #[must_use]
    pub const fn gnu(self) -> bool {
        self.0 & 1 != 0
    }

    /// Whether lld accepts the option.
    #[must_use]
    pub const fn lld(self) -> bool {
        self.0 & 2 != 0
    }

    /// Whether mold accepts the option.
    #[must_use]
    pub const fn mold(self) -> bool {
        self.0 & 4 != 0
    }

    /// The names, as the generated table shows them.
    #[must_use]
    pub fn names(self) -> String {
        let mut names = Vec::new();
        if self.gnu() {
            names.push("GNU");
        }
        if self.lld() {
            names.push("lld");
        }
        if self.mold() {
            names.push("mold");
        }
        if names.is_empty() {
            names.push("mortar");
        }
        names.join(", ")
    }
}

/// Whether mortar honors an option (`spec/cli.md` §4.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    /// The pipeline honors it; a test proves it.
    Supported,
    /// Accepted, and correctly without effect, for the reason given.
    Noop(&'static str),
    /// Parsed, but using it is `Refused { class: Option, name }`.
    Refused,
}

impl Status {
    /// The word the generated table and `--help` use.
    #[must_use]
    pub const fn word(self) -> &'static str {
        match self {
            Self::Supported => "supported",
            Self::Noop(_) => "noop",
            Self::Refused => "refused",
        }
    }
}

/// The `--help` groups, in the order `--help` prints them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Group {
    /// What is produced.
    Output,
    /// Inputs and their positional state.
    Inputs,
    /// Library search.
    Search,
    /// Scripts and lists.
    Scripts,
    /// Symbols.
    Symbols,
    /// Dynamic linking.
    Dynamic,
    /// Sections, segments, addresses.
    Layout,
    /// Diagnostics.
    Diagnostics,
    /// Maps, traces, threads, information.
    Bookkeeping,
    /// Linker plugins and LTO.
    Lto,
    /// Options of one target.
    TargetSpecific,
    /// `-z` keywords.
    Z,
    /// Options GNU ld keeps for other systems or other tools.
    Compatibility,
}

impl Group {
    /// Every group, in order.
    pub const ALL: [Self; 13] = [
        Self::Output,
        Self::Inputs,
        Self::Search,
        Self::Scripts,
        Self::Symbols,
        Self::Dynamic,
        Self::Layout,
        Self::Diagnostics,
        Self::Bookkeeping,
        Self::Lto,
        Self::TargetSpecific,
        Self::Z,
        Self::Compatibility,
    ];

    /// The heading.
    #[must_use]
    pub const fn title(self) -> &'static str {
        match self {
            Self::Output => "Output",
            Self::Inputs => "Inputs and their positional state",
            Self::Search => "Library search",
            Self::Scripts => "Scripts and lists",
            Self::Symbols => "Symbols",
            Self::Dynamic => "Dynamic linking",
            Self::Layout => "Sections, segments and addresses",
            Self::Diagnostics => "Diagnostics",
            Self::Bookkeeping => "Maps, traces, threads and information",
            Self::Lto => "Linker plugins and LTO",
            Self::TargetSpecific => "Options of one target",
            Self::Z => "-z keywords",
            Self::Compatibility => "Kept for other systems and tools",
        }
    }
}

/// What a kind option sets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KindSet {
    /// `-shared`.
    Shared,
    /// `-pie`.
    Pie,
    /// `-no-pie`.
    NoPie,
    /// `-r`.
    Relocatable,
}

/// A change of the positional state (`spec/cli.md` §3.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StateOp {
    /// `--as-needed`, `--no-as-needed`.
    AsNeeded(bool),
    /// `--whole-archive`, `--no-whole-archive`.
    WholeArchive(bool),
    /// `-Bstatic` (`true`), `-Bdynamic` (`false`).
    Static(bool),
    /// `--copy-dt-needed-entries`, `--no-copy-dt-needed-entries`.
    CopyDtNeeded(bool),
    /// `--push-state`.
    Push,
    /// `--pop-state`.
    Pop,
    /// `--start-group`.
    StartGroup,
    /// `--end-group`.
    EndGroup,
    /// `--start-lib`.
    StartLib,
    /// `--end-lib`.
    EndLib,
    /// `-b`, `--format`.
    Format,
}

/// A typed `-z` field (`spec/cli.md` §3.7).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ZField {
    /// `now`, `lazy`.
    Now(bool),
    /// `relro`, `norelro`.
    Relro(bool),
    /// `execstack`, `noexecstack`.
    Execstack(bool),
    /// `text`, `notext`, `textoff`.
    Text(bool),
    /// `separate-code`, `noseparate-code`.
    SeparateCode(bool),
    /// `defs`, `undefs`.
    Defs(bool),
    /// `max-page-size=`.
    MaxPageSize,
    /// `common-page-size=`.
    CommonPageSize,
    /// `stack-size=`.
    StackSize,
    /// `pack-relative-relocs`, `nopack-relative-relocs`.
    PackRelativeRelocs(bool),
    /// `copyreloc`, `nocopyreloc`.
    Copyreloc(bool),
    /// `combreloc`, `nocombreloc`.
    Combreloc(bool),
    /// `sectionheader`, `nosectionheader`.
    Sectionheader(bool),
    /// `memory-seal`, `nomemory-seal`.
    MemorySeal(bool),
}

/// What an address option places.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StartKind {
    /// `-Ttext`.
    Text,
    /// `-Tdata`.
    Data,
    /// `-Tbss`.
    Bss,
    /// `--section-start=name=address`.
    Section,
    /// `-Ttext-segment`.
    TextSegment,
    /// `-Trodata-segment`.
    RodataSegment,
    /// `-Tldata-segment`.
    LdataSegment,
}

/// Where an option lands in the `Plan` (`spec/cli.md` §3). The generated
/// table's "lands in" column is [`Lands::field`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lands {
    /// `output`.
    Output,
    /// `kind`.
    Kind(KindSet),
    /// `emulation`.
    Emulation,
    /// `entry`.
    Entry,
    /// `inputs`, as `-l`.
    Library,
    /// `inputs`, as just-symbols.
    JustSymbols,
    /// The positional state of the following inputs.
    State(StateOp),
    /// `library_paths`.
    LibraryPath,
    /// `sysroot`.
    Sysroot,
    /// `nostdlib`.
    NoStdlib,
    /// `scripts`.
    Script(ScriptRole),
    /// `symbols`.
    Symbol(SymbolRole),
    /// `dynamic.export_dynamic`.
    ExportDynamic(bool),
    /// `dynamic.bsymbolic`.
    Bsymbolic(Option<Bsymbolic>),
    /// `dynamic.soname`.
    Soname,
    /// `dynamic.rpath`.
    Rpath,
    /// `dynamic.rpath_link`.
    RpathLink,
    /// `dynamic.dynamic_linker`.
    DynamicLinker,
    /// `dynamic.dynamic_linker`, `None`.
    NoDynamicLinker,
    /// `dynamic.new_dtags`.
    NewDtags(bool),
    /// `dynamic.hash_style`.
    HashStyle,
    /// `dynamic.pack_dyn_relocs`.
    PackDynRelocs,
    /// `z.*`.
    Z(ZField),
    /// `shaping.gc_sections`.
    GcSections(bool),
    /// `shaping.icf`.
    Icf,
    /// `shaping.icf`, `none`.
    NoIcf,
    /// `shaping.build_id`.
    BuildId,
    /// `shaping.build_id`, cleared.
    NoBuildId,
    /// `shaping.strip`.
    Strip(Strip),
    /// `shaping.discard`.
    Discard(Discard),
    /// `shaping.compress_debug_sections`.
    CompressDebugSections,
    /// `shaping.eh_frame_hdr`.
    EhFrameHdr(bool),
    /// `shaping.magic`.
    Magic(Magic),
    /// `shaping.image_base`.
    ImageBase,
    /// `shaping.starts`.
    Start(StartKind),
    /// `shaping.emit_relocs`.
    EmitRelocs,
    /// `shaping.gdb_index`.
    GdbIndex(bool),
    /// `diagnostics.error_limit`.
    ErrorLimit,
    /// `diagnostics.fatal_warnings`.
    FatalWarnings(bool),
    /// `diagnostics.no_warnings`.
    NoWarnings,
    /// `diagnostics.color`.
    Color,
    /// `diagnostics.color`, `never`.
    NoColor,
    /// `z.defs` (`--no-undefined`).
    NoUndefined,
    /// `diagnostics.unresolved_symbols`.
    UnresolvedSymbols,
    /// `diagnostics.warn_unresolved_symbols`.
    WarnUnresolved(bool),
    /// `diagnostics.allow_multiple_definition`.
    AllowMultipleDefinition(bool),
    /// `diagnostics.noinhibit_exec`.
    NoinhibitExec,
    /// `dynamic.allow_shlib_undefined`.
    AllowShlibUndefined(bool),
    /// `diagnostics.warn_backrefs`.
    WarnBackrefs(bool),
    /// `diagnostics.warn_common`.
    WarnCommon(bool),
    /// `bookkeeping.map`.
    Map,
    /// `bookkeeping.print_map`.
    PrintMap,
    /// `bookkeeping.cref`.
    Cref,
    /// `bookkeeping.reproduce`.
    Reproduce,
    /// `bookkeeping.threads` (`--threads[=N]`).
    Threads,
    /// `bookkeeping.threads` (`--thread-count N`).
    ThreadCount,
    /// `bookkeeping.threads`, one.
    NoThreads,
    /// `bookkeeping.time_phases`.
    TimePhases,
    /// `bookkeeping.verify`.
    Verify,
    /// `exit`, `--version`.
    Version,
    /// `print_version`.
    PrintVersion(VersionStyle),
    /// `exit`, `--help`.
    Help,
    /// `bookkeeping.verbose`.
    Verbose,
    /// `bookkeeping.trace`.
    Trace,
    /// `bookkeeping.dependency_file`.
    DependencyFile,
    /// `plugin.path`.
    Plugin,
    /// `plugin.options`.
    PluginOpt,
    /// `other`.
    Other,
}

impl Lands {
    /// The `Plan` field the option lands in, for the generated table.
    #[must_use]
    pub const fn field(self) -> &'static str {
        match self {
            Self::Output => "output",
            Self::Kind(_) => "kind",
            Self::Emulation => "emulation",
            Self::Entry => "entry",
            Self::Library | Self::JustSymbols => "inputs",
            Self::State(StateOp::Push | StateOp::Pop) => "inputs[].state (saved, restored)",
            Self::State(StateOp::StartGroup | StateOp::EndGroup) => "inputs[].state.group",
            Self::State(StateOp::StartLib | StateOp::EndLib) => "inputs[].state.lib",
            Self::State(StateOp::AsNeeded(_)) => "inputs[].state.as_needed",
            Self::State(StateOp::WholeArchive(_)) => "inputs[].state.whole_archive",
            Self::State(StateOp::Static(_)) => "inputs[].state.link_static",
            Self::State(StateOp::CopyDtNeeded(_)) => "inputs[].state.copy_dt_needed",
            Self::State(StateOp::Format) => "inputs[].state.format",
            Self::LibraryPath => "library_paths",
            Self::Sysroot => "sysroot",
            Self::NoStdlib => "nostdlib",
            Self::Script(_) => "scripts",
            Self::Symbol(_) => "symbols",
            Self::ExportDynamic(_) => "dynamic.export_dynamic",
            Self::Bsymbolic(_) => "dynamic.bsymbolic",
            Self::Soname => "dynamic.soname",
            Self::Rpath => "dynamic.rpath",
            Self::RpathLink => "dynamic.rpath_link",
            Self::DynamicLinker | Self::NoDynamicLinker => "dynamic.dynamic_linker",
            Self::NewDtags(_) => "dynamic.new_dtags",
            Self::HashStyle => "dynamic.hash_style",
            Self::PackDynRelocs => "dynamic.pack_dyn_relocs",
            Self::Z(field) => match field {
                ZField::Now(_) => "z.now",
                ZField::Relro(_) => "z.relro",
                ZField::Execstack(_) => "z.execstack",
                ZField::Text(_) => "z.text",
                ZField::SeparateCode(_) => "z.separate_code",
                ZField::Defs(_) => "z.defs",
                ZField::MaxPageSize => "z.max_page_size",
                ZField::CommonPageSize => "z.common_page_size",
                ZField::StackSize => "z.stack_size",
                ZField::PackRelativeRelocs(_) => "z.pack_relative_relocs",
                ZField::Copyreloc(_) => "z.copyreloc",
                ZField::Combreloc(_) => "z.combreloc",
                ZField::Sectionheader(_) => "z.sectionheader",
                ZField::MemorySeal(_) => "z.memory_seal",
            },
            Self::GcSections(_) => "shaping.gc_sections",
            Self::Icf | Self::NoIcf => "shaping.icf",
            Self::BuildId | Self::NoBuildId => "shaping.build_id",
            Self::Strip(_) => "shaping.strip",
            Self::Discard(_) => "shaping.discard",
            Self::CompressDebugSections => "shaping.compress_debug_sections",
            Self::EhFrameHdr(_) => "shaping.eh_frame_hdr",
            Self::Magic(_) => "shaping.magic",
            Self::ImageBase => "shaping.image_base",
            Self::Start(_) => "shaping.starts",
            Self::EmitRelocs => "shaping.emit_relocs",
            Self::GdbIndex(_) => "shaping.gdb_index",
            Self::ErrorLimit => "diagnostics.error_limit",
            Self::FatalWarnings(_) => "diagnostics.fatal_warnings",
            Self::NoWarnings => "diagnostics.no_warnings",
            Self::Color | Self::NoColor => "diagnostics.color",
            Self::NoUndefined => "z.defs",
            Self::UnresolvedSymbols => "diagnostics.unresolved_symbols",
            Self::WarnUnresolved(_) => "diagnostics.warn_unresolved_symbols",
            Self::AllowMultipleDefinition(_) => "diagnostics.allow_multiple_definition",
            Self::NoinhibitExec => "diagnostics.noinhibit_exec",
            Self::AllowShlibUndefined(_) => "dynamic.allow_shlib_undefined",
            Self::WarnBackrefs(_) => "diagnostics.warn_backrefs",
            Self::WarnCommon(_) => "diagnostics.warn_common",
            Self::Map => "bookkeeping.map",
            Self::PrintMap => "bookkeeping.print_map",
            Self::Cref => "bookkeeping.cref",
            Self::Reproduce => "bookkeeping.reproduce",
            Self::Threads | Self::ThreadCount | Self::NoThreads => "bookkeeping.threads",
            Self::TimePhases => "bookkeeping.time_phases",
            Self::Verify => "bookkeeping.verify",
            Self::Version | Self::Help => "exit",
            Self::PrintVersion(_) => "print_version",
            Self::Verbose => "bookkeeping.verbose",
            Self::Trace => "bookkeeping.trace",
            Self::DependencyFile => "bookkeeping.dependency_file",
            Self::Plugin => "plugin.path",
            Self::PluginOpt => "plugin.options",
            Self::Other => "other",
        }
    }
}

impl Row {
    /// The row's name: its first spelling as `--help` shows it.
    #[must_use]
    pub fn name(&self) -> String {
        self.spellings[0].display()
    }

    /// Whether the row is a `-z` keyword.
    #[must_use]
    pub fn is_z(&self) -> bool {
        self.spellings[0].form == Form::Z
    }
}

/// The table (`spec/cli.md` §4).
#[must_use]
pub fn rows() -> &'static [Row] {
    crate::rows::ROWS
}
