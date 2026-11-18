//! The refusals and diagnostics of a `-r` link: `spec/diag.md` §4.4
//! (WP-181, RL-1). No new kind and no new class: each is a kind of §3.1
//! or a refusal of a §4.1 class, and this module holds their names, notes
//! and constructors so that the `-r` packages (RL-2..RL-11) only use them.
//! [`ROWS`] is §4.4's table as data; `tests/spec_tables.rs` checks the
//! one against the other.

use crate::{Diagnostic, Kind, RefusalClass};

/// `unsupported feature: relocatable output (-r)`: the line had `-r` (or
/// `-Ur`) until RL-7 joined the stages (WP-187); now RL-9's REL stub
/// (`mortar_image::relrel::write`) only (`spec/resolve.md` §1, §11).
pub const RELOCATABLE_OUTPUT: &str = "relocatable output (-r)";
/// `shared object input with -r` (`spec/cli.md` C-R5), `.at` the file.
pub const SHARED_OBJECT_INPUT: &str = "shared object input with -r";
/// `relocatable output with no input of a known machine` (C-R5).
pub const NO_INPUT_OF_A_KNOWN_MACHINE: &str = "relocatable output with no input of a known machine";
/// The note of [`NO_INPUT_OF_A_KNOWN_MACHINE`].
pub const NO_INPUT_OF_A_KNOWN_MACHINE_NOTE: &str = "give -m EMULATION";
/// `GCC LTO sections with -r` (`spec/resolve.md` RV-R9, a fat LTO
/// object), `.at` the file.
pub const GCC_LTO_SECTIONS: &str = "GCC LTO sections with -r";

/// `section groups with -r` (`spec/layout.md` L-R2, L-R3), `.at` the
/// first kept `SHT_GROUP` section: writing them is RL-8's (the cut of RL-7
/// is C, record `spec/landed/layout/WP-154.md` §F).
pub const SECTION_GROUPS: &str = "section groups with -r";
/// The note of [`SECTION_GROUPS`].
pub const SECTION_GROUPS_NOTE: &str =
    "RL-8 lifts this at -r (spec/layout.md L-R2, L-R3: COMDAT groups, C++)";
/// `{machine} output with -r` (`x86-64`, `i386`): RL-9's (REL targets and
/// x86-64's parts, record WP-154 §F).
pub const TARGET_WITH_R: &str = "{machine} output with -r";
/// The note of [`TARGET_WITH_R`].
pub const TARGET_WITH_R_NOTE: &str = "RL-9 lifts this at -r (AArch64 only until then)";

/// `removed .eh_frame records at -r` (`spec/layout.md` L-R6): an FDE of a
/// dropped section is removed with its records, RL-8's (the image and the
/// driver both refuse a non-empty `eh_removed`).
pub const EH_RECORDS_REMOVED: &str = "removed .eh_frame records at -r";
/// The note of [`EH_RECORDS_REMOVED`].
pub const EH_RECORDS_REMOVED_NOTE: &str = "RL-8 (spec/layout.md L-R6) lifts this at -r";

/// `unsupported feature: removed .eh_frame records at -r` (RL-8).
#[must_use]
pub fn refused_eh_records_removed() -> Diagnostic {
    Diagnostic::refused(RefusalClass::Feature, EH_RECORDS_REMOVED).note(EH_RECORDS_REMOVED_NOTE)
}

/// `unsupported feature: section groups with -r` (RL-8).
#[must_use]
pub fn refused_section_groups() -> Diagnostic {
    Diagnostic::refused(RefusalClass::Feature, SECTION_GROUPS).note(SECTION_GROUPS_NOTE)
}

/// `unsupported feature: {machine} output with -r` (RL-9), `machine` as
/// `Machine::name` spells it.
#[must_use]
pub fn refused_target_with_r(machine: &str) -> Diagnostic {
    Diagnostic::refused(RefusalClass::Feature, TARGET_WITH_R.replace("{machine}", machine))
        .note(TARGET_WITH_R_NOTE)
}

/// The note of `-T with -r` (C-R3, RV-R10).
pub const SCRIPT_NOTE: &str = "kernel modules and scripts at -r are not supported yet";
/// The note of `--gc-sections with -r` (C-R4, `spec/image.md` I-R10).
pub const GC_SECTIONS_NOTE: &str =
    "a partial link cannot know what the final link needs; collect in the final link";
/// The note of `--wrap with -r` (C-R4, RV-R10).
pub const WRAP_NOTE: &str = "wrap in the final link";

/// The options §4.4 refuses at `-r` with a fixed name (`{option} with
/// -r`), by the spelling of the option the name starts with.
pub const OPTIONS_REFUSED_WITH_R: [&str; 9] = [
    "-T",
    "--defsym",
    "--gc-sections",
    "--icf",
    "--unique",
    "--relax",
    "--wrap",
    "--compress-debug-sections",
    "--gdb-index",
];

/// The options of C-R3 refused at `-r` until their package lands, as §4.4
/// spells them (`-d with -r` .. `--force-group-allocation with -r`).
pub const OPTIONS_REFUSED_UNTIL: [&str; 12] = [
    "-d",
    "-dc",
    "-dp",
    "-s",
    "--strip-all",
    "-S",
    "--strip-debug",
    "-x",
    "--discard-all",
    "-b",
    "--format=binary",
    "--force-group-allocation",
];

/// The `-z` keywords §4.4 refuses at `-r` (`-z {keyword} with -r`, the
/// keyword as spelled on the line; `stack-size=` takes its value).
pub const Z_KEYWORDS_REFUSED_WITH_R: [&str; 13] = [
    "force-bti",
    "gcs=always",
    "stack-size=",
    "memory-seal",
    "ibt",
    "shstk",
    "indirect-extern-access",
    "x86-64-v2",
    "x86-64-v3",
    "x86-64-v4",
    "lam-u48",
    "lam-u57",
    "unique-symbol",
];

/// The `{second}` of `-r and {second} cannot be used together`, as §4.4
/// lists them (the spelling on the line is what is reported;
/// `--pic-executable` is `-pie`'s other spelling).
pub const CONFLICTS_WITH_R: [&str; 17] = [
    "-shared",
    "-pie",
    "--pic-executable",
    "-static-pie",
    "-z nosectionheader",
    "-Bsymbolic",
    "-Bsymbolic-functions",
    "--dynamic-list",
    "--export-dynamic",
    "-E",
    "-soname",
    "--enable-new-dtags",
    "--export-dynamic-symbol",
    "-init",
    "-fini",
    "--pack-dyn-relocs",
    "--android-memtag-mode",
];

/// The name `{option} with -r` (`spec/cli.md` C-R2b's last bullet).
#[must_use]
pub fn with_r(option: &str) -> String {
    format!("{option} with -r")
}

/// `unsupported option: {option} with -r`.
#[must_use]
pub fn refused_with_r(option: &str) -> Diagnostic {
    Diagnostic::refused(RefusalClass::Option, with_r(option))
}

/// `unsupported option: -z {keyword} with -r`.
#[must_use]
pub fn refused_z_with_r(keyword: &str) -> Diagnostic {
    refused_with_r(&format!("-z {keyword}"))
}

/// `unsupported feature: relocatable output (-r)`.
#[must_use]
pub fn refused_relocatable_output() -> Diagnostic {
    Diagnostic::refused(RefusalClass::Feature, RELOCATABLE_OUTPUT)
}

/// `-r and {second} cannot be used together` (C-R1, C-R2a, C-R4, C-R2b):
/// kind `conflicting-options` with `first` = `-r`; `second` as spelled on
/// the line.
#[must_use]
pub fn conflicting_with_r(second: impl Into<String>) -> Diagnostic {
    Diagnostic::error(Kind::ConflictingOptions { first: "-r".to_owned(), second: second.into() })
}

/// What a row of §4.4 is in the code.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Entry {
    /// A refusal of `class`, with the names it is built with (empty: the
    /// name is a final link's; a name with `{..}` is a template).
    Refusal { class: RefusalClass, names: &'static [&'static str] },
    /// An error of the §3.1 kind with this id, as §3.1 says.
    Kind(&'static str),
    /// A warning of the §3.1 kind with this id.
    Warning(&'static str),
    /// As in a final link: nothing of its own at `-r`.
    AsFinal,
    /// A diagnostic §4.4 names that the code cannot build as written: its
    /// id is not a kind of §3.1, and §4.4 says §3.1 does not change
    /// (`eh-frame-terminator`, record `spec/landed/core/WP-181.md`).
    NotBuilt(&'static str),
}

/// A row of §4.4's table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Row {
    /// The first cell ("id or name"), without its backticks.
    pub id: &'static str,
    /// What the code builds for it.
    pub entry: Entry,
    /// The last cell ("until"): the package that lifts it, or `—`.
    pub until: &'static str,
}

const fn refusal(
    id: &'static str,
    class: RefusalClass,
    names: &'static [&'static str],
    until: &'static str,
) -> Row {
    Row { id, entry: Entry::Refusal { class, names }, until }
}

const fn row(id: &'static str, entry: Entry, until: &'static str) -> Row {
    Row { id, entry, until }
}

const NONE: &str = "—";

/// §4.4's table, in its order.
pub const ROWS: &[Row] = &[
    refusal(
        RELOCATABLE_OUTPUT,
        RefusalClass::Feature,
        &[RELOCATABLE_OUTPUT],
        "RL-7 for -r itself (lifted, WP-187); RL-9 for the stub",
    ),
    row("conflicting-options, first = -r", Entry::Kind("conflicting-options"), NONE),
    refusal("-T with -r", RefusalClass::Option, &["-T with -r"], "RL-11"),
    refusal("--defsym with -r", RefusalClass::Option, &["--defsym with -r"], "RL-11"),
    refusal("--gc-sections with -r", RefusalClass::Option, &["--gc-sections with -r"], NONE),
    refusal("--icf with -r", RefusalClass::Option, &["--icf with -r"], NONE),
    refusal("--unique with -r", RefusalClass::Option, &["--unique with -r"], "RL-11 (C-R3, L-R11)"),
    refusal("--relax with -r", RefusalClass::Option, &["--relax with -r"], NONE),
    refusal("--wrap with -r", RefusalClass::Option, &["--wrap with -r"], NONE),
    refusal(
        "--compress-debug-sections with -r",
        RefusalClass::Option,
        &["--compress-debug-sections with -r"],
        NONE,
    ),
    refusal("--gdb-index with -r", RefusalClass::Option, &["--gdb-index with -r"], NONE),
    refusal(
        "-z KEYWORD with -r",
        RefusalClass::Option,
        &["-z {keyword} with -r"],
        "a line that needs one",
    ),
    refusal(
        "-d with -r, -dc with -r, -dp with -r; -s with -r, --strip-all with -r, -S with -r, \
         --strip-debug with -r, -x with -r, --discard-all with -r; -b with -r, --format=binary \
         with -r; --force-group-allocation with -r",
        RefusalClass::Option,
        &["{option} with -r"],
        "RL-11 (-d, -s, -S, -x), RL-10 (-b), RL-8 (--force-group-allocation)",
    ),
    refusal(SHARED_OBJECT_INPUT, RefusalClass::Feature, &[SHARED_OBJECT_INPUT], NONE),
    refusal(SECTION_GROUPS, RefusalClass::Feature, &[SECTION_GROUPS], "RL-8"),
    refusal(TARGET_WITH_R, RefusalClass::Feature, &[TARGET_WITH_R], "RL-9"),
    refusal(EH_RECORDS_REMOVED, RefusalClass::Feature, &[EH_RECORDS_REMOVED], "RL-8"),
    refusal(
        NO_INPUT_OF_A_KNOWN_MACHINE,
        RefusalClass::Feature,
        &[NO_INPUT_OF_A_KNOWN_MACHINE],
        NONE,
    ),
    refusal("LLVM bitcode", RefusalClass::FileFormat, &[], NONE),
    row("the slim GCC LTO object", Entry::AsFinal, NONE),
    refusal(GCC_LTO_SECTIONS, RefusalClass::Feature, &[GCC_LTO_SECTIONS], "a line that needs it"),
    refusal("SHT_CREL", RefusalClass::SectionType, &[], NONE),
    refusal(
        "SHT_REL (AArch64, x86-64), SHT_RELA (i386)",
        RefusalClass::SectionType,
        &["SHT_REL", "SHT_RELA"],
        NONE,
    ),
    row("section-conflict", Entry::Kind("section-conflict"), NONE),
    row(".gnu.linkonce", Entry::AsFinal, NONE),
    row("relocation-out-of-range", Entry::Kind("relocation-out-of-range"), NONE),
    refusal("a relocation type the machine does not know", RefusalClass::RelocationType, &[], NONE),
    row("e_flags", Entry::AsFinal, NONE),
    row("duplicate-symbol", Entry::Kind("duplicate-symbol"), NONE),
    row("discarded-section-reference", Entry::Kind("discarded-section-reference"), NONE),
    row("comdat-mismatch", Entry::Kind("comdat-mismatch"), NONE),
    row("--build-id=STYLE", Entry::AsFinal, NONE),
    row(
        "--incremental and every --incremental-* option with -r",
        Entry::Warning("incremental-unsupported"),
        NONE,
    ),
    row("the ODR diagnostics", Entry::AsFinal, NONE),
    row(
        "a -r of the startup files (crt1.o … crtend.o in the middle)",
        Entry::NotBuilt("eh-frame-terminator"),
        "RL-11",
    ),
];
