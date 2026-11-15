//! The linker's own symbols (`spec/resolve.md` §8): which names exist as
//! `Provide` candidates (rank 5), when, with which binding and
//! visibility, and which names an object may not define (§8.2).

use mortar_elf::Machine;
use mortar_elf::consts::{STV_DEFAULT, STV_HIDDEN, STV_PROTECTED};

/// When a name of §8.1's table is defined.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum When {
    /// Always.
    Always,
    /// Executables (every M1 output).
    Executable,
    /// Static executables that are not position-independent (`-static`).
    /// Under `-static-pie` the name stays undefined (zero), as in GNU ld and
    /// lld (mold defines it absolute 0): the `IRELATIVE` relocations are in
    /// `.rela.dyn`, which the startup code applies.
    StaticExecutable,
    /// Dynamic links only (M2).
    Dynamic,
    /// AArch64 only (GNU ld's AArch64 script).
    AArch64,
    /// When an output section or segment exists: a conditional candidate
    /// that the first layout stage confirms or withdraws (§5.1).
    Conditional,
    /// x86-64 only, and conditional as [`When::Conditional`]
    /// (`_TLS_MODULE_BASE_`, `spec/x64.md` X-47: the TLS block exists;
    /// layout L-98 refuses it by name when it does not).
    X86_64Conditional,
    /// A row of a later milestone: not a candidate in M1.
    Later,
}

/// A row of §8.1's table.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct LinkerSymbol {
    pub name: &'static [u8],
    pub when: When,
    /// `true`: `STB_LOCAL` in `.symtab` (with `visibility`); `false`:
    /// `STB_GLOBAL`.
    pub local: bool,
    pub visibility: u8,
    /// §8.2: a live object definition is `linker-symbol-redefined`.
    pub forced: bool,
}

const fn row(name: &'static [u8], when: When, local: bool, forced: bool) -> LinkerSymbol {
    LinkerSymbol {
        name,
        when,
        local,
        visibility: if local { STV_HIDDEN } else { STV_DEFAULT },
        forced,
    }
}

/// §8.1's table, without the `__start_`/`__stop_` rows ([`start_stop`])
/// and the names that are not defined (`__dso_handle`, ...).
pub const TABLE: &[LinkerSymbol] = &[
    row(b"_GLOBAL_OFFSET_TABLE_", When::Always, true, true),
    row(b"_DYNAMIC", When::Dynamic, true, false),
    row(b"__ehdr_start", When::Always, true, false),
    row(b"__executable_start", When::Executable, true, false),
    row(b"_etext", When::Always, false, false),
    row(b"etext", When::Always, false, false),
    row(b"__etext", When::Always, false, false),
    row(b"_edata", When::Always, false, true),
    row(b"edata", When::Always, false, false),
    row(b"__bss_start", When::Always, false, true),
    row(b"_end", When::Always, false, true),
    row(b"end", When::Always, false, false),
    row(b"__data_start", When::AArch64, false, false),
    row(b"__end__", When::AArch64, false, true),
    row(b"__bss_start__", When::AArch64, false, true),
    row(b"__bss_end__", When::AArch64, false, true),
    row(b"_bss_end__", When::AArch64, false, true),
    row(b"__init_array_start", When::Always, true, false),
    row(b"__init_array_end", When::Always, true, false),
    row(b"__preinit_array_start", When::Always, true, false),
    row(b"__preinit_array_end", When::Always, true, false),
    row(b"__fini_array_start", When::Always, true, false),
    row(b"__fini_array_end", When::Always, true, false),
    row(b"__rela_iplt_start", When::StaticExecutable, true, false),
    row(b"__rela_iplt_end", When::StaticExecutable, true, false),
    row(b"__rel_iplt_start", When::StaticExecutable, true, false),
    row(b"__rel_iplt_end", When::StaticExecutable, true, false),
    row(b"_TLS_MODULE_BASE_", When::X86_64Conditional, true, false),
    row(b"__GNU_EH_FRAME_HDR", When::Conditional, true, false),
];

/// Which linker symbol a `Provide` definition is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Provided {
    /// Row `n` of [`TABLE`].
    Table(u8),
    /// `__start_<sec>` (the key's name says which section).
    Start,
    /// `__stop_<sec>`.
    Stop,
}

impl Provided {
    /// `.symtab` binding (local or global) and visibility (§8.1).
    #[must_use]
    pub fn binding(self) -> (bool, u8) {
        match self {
            Self::Table(n) => {
                let row = TABLE[n as usize];
                (row.local, row.visibility)
            }
            Self::Start | Self::Stop => (false, STV_PROTECTED),
        }
    }
}

/// Whether `name` is a C identifier (§8.1, `__start_<sec>`).
fn is_c_identifier(name: &[u8]) -> bool {
    !name.is_empty()
        && !name[0].is_ascii_digit()
        && name.iter().all(|&b| b.is_ascii_alphanumeric() || b == b'_')
}

/// `__start_<sec>` / `__stop_<sec>` with `<sec>` a C identifier.
#[must_use]
pub fn start_stop(name: &[u8]) -> Option<Provided> {
    if let Some(sec) = name.strip_prefix(b"__start_") {
        return is_c_identifier(sec).then_some(Provided::Start);
    }
    if let Some(sec) = name.strip_prefix(b"__stop_") {
        return is_c_identifier(sec).then_some(Provided::Stop);
    }
    None
}

/// The `Provide` candidate for a referenced key named `name` (§8.1), and
/// whether it is conditional; `None` when the name is not one or its
/// condition is false for this output.
#[must_use]
pub fn candidate(
    name: &[u8],
    machine: Machine,
    output: crate::options::Output,
) -> Option<(Provided, bool)> {
    if let Some(p) = start_stop(name) {
        return Some((p, true));
    }
    let n = TABLE.iter().position(|row| row.name == name)?;
    let row = TABLE[n];
    let iplt_rel = name.starts_with(b"__rel_iplt");
    let iplt_rela = name.starts_with(b"__rela_iplt");
    let on = match row.when {
        When::Always | When::Executable => true,
        When::StaticExecutable => {
            !output.dynamic
                && !output.pie
                && if machine == Machine::I386 { iplt_rel } else { iplt_rela || !iplt_rel }
        }
        When::Dynamic => output.dynamic,
        When::AArch64 => machine == Machine::AArch64,
        When::Conditional => return Some((Provided::Table(n as u8), true)),
        When::X86_64Conditional if machine == Machine::X86_64 => {
            return Some((Provided::Table(n as u8), true));
        }
        When::X86_64Conditional => false,
        When::Later => false,
    };
    on.then_some((Provided::Table(n as u8), false))
}

/// The names an object may not define on `machine` (§8.2).
pub fn forced(machine: Machine) -> impl Iterator<Item = &'static [u8]> {
    TABLE
        .iter()
        .filter(move |row| row.forced && (row.when != When::AArch64 || machine == Machine::AArch64))
        .map(|row| row.name)
}
