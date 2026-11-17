//! The rows of the option table (`spec/cli.md` §4). The generated
//! `docs/status/options.md` and `--help` come from these.
//!
//! One row per option: its spellings (the first is its name), its value,
//! the reference linkers that accept it (GNU ld 2.44 from its manual and
//! `--help`; lld 22 and mold by experiment), the targets it belongs to, its
//! `--help` group, its status, where it lands in the `Plan`, and one line
//! of help. A row's status is `Refused` until the pipeline honors it.

use crate::plan::Target;
use crate::plan::{Bsymbolic, Discard, Magic, ScriptRole, Strip, SymbolRole, VersionStyle};
use crate::table::Status::{Noop, Partial, Refused, Supported};
use crate::table::{
    Arg, AtR, AtRClass, Group, KindSet, Lands, Linkers, Row, Spelling, StartKind, StateOp, Status,
    Until, Values, ZField,
};

const GLM: Linkers = Linkers::GLM;
const GL: Linkers = Linkers::GL;
const GM: Linkers = Linkers::GM;
const G: Linkers = Linkers::G;
const LM: Linkers = Linkers::LM;
const L: Linkers = Linkers::L;
const M: Linkers = Linkers::M;
const MORTAR: Linkers = Linkers::MORTAR;

/// The note of `--hash-style` (`spec/dyn.md` D-20, R-C1).
const HASH_STYLE: &str =
    "dyn.md D-20: an executable gets .gnu.hash and no .hash; mortar writes =gnu";
/// The reason of `--no-copy-dt-needed-entries` (`spec/dyn.md` D-15).
const NO_COPY_DT_NEEDED: &str = "dyn.md D-15: the default; a library reached only through another's DT_NEEDED entries never defines a symbol of the link (D-12), and --copy-dt-needed-entries is refused";
/// The reason of gold's `--detect-odr-violations` (`spec/odr.md` O-41):
/// mortar's checks are on by default, and the option lowers nothing.
const DETECT_ODR: &str = "odr.md O-41: mortar checks the one-definition rule by default; --detect-odr-violations has no further effect";

const A64: &[Target] = &[Target::Aarch64];
const X86: &[Target] = &[Target::X86_64, Target::I386];
const X64: &[Target] = &[Target::X86_64];

#[allow(clippy::too_many_arguments)]
const fn r(
    spellings: &'static [Spelling],
    arg: Arg,
    meta: &'static str,
    linkers: Linkers,
    targets: &'static [Target],
    group: Group,
    status: Status,
    lands: Lands,
    help: &'static str,
) -> Row {
    Row { spellings, arg, meta, linkers, targets, group, status, lands, help, at_r: None }
}

// The classes at `-r` (`spec/cli.md` C-R2b; the column "at `-r`" of
// `docs/status/options.md`).
const fn at_r(class: AtRClass, note: &'static str, rule: &'static str) -> AtR {
    AtR { class, note, rule, until: None }
}
const fn acts(note: &'static str, rule: &'static str) -> AtR {
    at_r(AtRClass::Acts, note, rule)
}
/// Acts once `package` lands; until then refused as `refusal`.
const fn acts_until(
    note: &'static str,
    rule: &'static str,
    package: &'static str,
    refusal: &'static str,
) -> AtR {
    AtR { class: AtRClass::Acts, note, rule, until: Some(Until { package, refusal }) }
}
const fn noop_r(note: &'static str, rule: &'static str) -> AtR {
    at_r(AtRClass::Noop, note, rule)
}
const fn final_r(note: &'static str, rule: &'static str) -> AtR {
    at_r(AtRClass::AsFinal, note, rule)
}
const fn conflicting_r(rule: &'static str) -> AtR {
    at_r(AtRClass::Conflicting, "", rule)
}
const fn refused_r(note: &'static str, rule: &'static str) -> AtR {
    at_r(AtRClass::Refused, note, rule)
}
/// A row C-R2, C-R2a, C-R3 and C-R4 do not name and C-R2b does not class
/// otherwise: its final-link status holds at `-r` (refused by its own
/// name, C-R2a's last paragraph).
const FINAL: AtR = final_r("", "C-R2b");

const fn s(text: &'static str) -> Spelling {
    Spelling::short(text, true)
}
const fn sx(text: &'static str) -> Spelling {
    Spelling::short(text, false)
}
const fn g(text: &'static str) -> Spelling {
    Spelling::long(text, true)
}
const fn g1(text: &'static str) -> Spelling {
    Spelling::single(text, true)
}
const fn g2(text: &'static str) -> Spelling {
    Spelling::two_dashes(text)
}
const fn x(text: &'static str) -> Spelling {
    Spelling::long(text, false)
}
const fn z(text: &'static str) -> Spelling {
    Spelling::z(text, true)
}
const fn zx(text: &'static str) -> Spelling {
    Spelling::z(text, false)
}

/// The reasons some `refused` rows give in their refusal's note
/// (`spec/cli.md` §4.4), by row name.
pub(crate) static REFUSED_NOTES: &[(&str, &str)] = &[
    (
        "--debug-names",
        "gdb 16 ignores a .debug_names it did not write; use --gdb-index (spec/synth.md S-G9)",
    ),
    // `spec/dyn.md` D-112 (`spec/cli.md` §4.2's M4 block): refused by name
    // in every kind, not planned.
    ("--Bsymbolic-non-weak", NON_WEAK),
    ("--Bsymbolic-non-weak-functions", NON_WEAK),
    ("--dynamic-list-data", DYNAMIC_LIST_DATA),
    ("--dynamic-list-cpp-new", DYNAMIC_LIST_CPP),
    ("--dynamic-list-cpp-typeinfo", DYNAMIC_LIST_CPP),
];

/// The `-shared` row's status (`spec/landed/dyn/WP-110.md` §G.4, written by
/// WP-117; `partial` since WP-123 joined the first target): what is joined,
/// what is not caught and what is refused although correct. A `partial`
/// row with no refused value: its refusals are by name, in the link.
pub(crate) const SHARED: &str = "AArch64 and x86-64 (i386 refused, `feature`, until WP-130; local-exec TLS in a shared object refused, dyn.md D-119; `extern \"C++\"` version-script entries refused, dyn.md D-114). Not caught (with every reference): non-PIC AArch64 code that reaches its own exported data only through a form dyn.md D-117's analysis does not follow (a spilled or reloaded base, a computed index, a 32-bit register, a value passed to a callee, code outside a sized function symbol, hand-written assembly; 3 of 306 measured bound definitions, every object refused for another use); an object whose `reloc-model` row (hod's) says `static` or `pie` and that defines an exported preemptible datum is refused whatever that analysis finds (meta.md MT-26, WP-129). Refused although correct: -fPIC code that forms an address before the start of a static object placed after exported data (`tab - 1`, GCC and Clang AArch64 -O0; dyn.md D-117; remedy -fdata-sections). Not judged on x86-64 (x64.md X-62): a section-relative PC-relative field in code naming exported data (none measured in GCC, Clang or kiln output at any -O)";
/// `--Bsymbolic-non-weak` and `--Bsymbolic-non-weak-functions` (D-112).
const NON_WEAK: &str = "dyn.md D-112: GNU ld does not know it (lld and mold do); use -Bsymbolic-functions, or a --dynamic-list";
/// `--dynamic-list-data` (D-112).
const DYNAMIC_LIST_DATA: &str = "dyn.md D-112: the references disagree (GNU ld binds functions and NOTYPE symbols, mold changes nothing, lld does not know it); use -Bsymbolic-functions, or a --dynamic-list";
/// `--dynamic-list-cpp-new` and `--dynamic-list-cpp-typeinfo` (D-112).
const DYNAMIC_LIST_CPP: &str =
    "dyn.md D-112: GNU ld only (lld and mold refuse it); list the symbols in a --dynamic-list";

/// `-z nodelete`, `-z nodlopen`, `-z initfirst` outside a shared object
/// (`spec/dyn.md` D-103, measured by WP-128).
const Z_EXEC_DIFFER: &str = "dyn.md D-103: mortar writes this flag in a shared object (-shared) only; in an executable the references disagree (GNU ld writes nothing, lld and mold write the flag)";
/// `-z interpose`, `-z origin` outside a shared object (`spec/dyn.md`
/// D-103).
const Z_EXEC_SHARED_ONLY: &str =
    "dyn.md D-103: mortar writes this flag in a shared object (-shared) only";

/// `--exclude-libs` (`spec/dyn.md` D-110 item 1, WP-128): what is refused
/// although a reference links it.
const WRAP: &str = "resolve.md §7.1: a versioned SYMBOL (f@V: GNU ld and lld wrap the reference f@V, mold f) and --wrap of another --wrap's __real_ name (GNU ld wraps it, lld takes the last option, mold binds it to the symbol) are refused by name";
const DEFSYM: &str = "resolve.md §7.2: EXPR is a number (0x... hexadecimal, or decimal without a leading 0), a symbol, or a symbol + or - a number; a number with a leading 0 (GNU ld reads it octal, lld and mold decimal: write it in hexadecimal) and every other expression (k/M suffixes, arithmetic, parentheses, shifts, ALIGN, two symbols) are refused by name";
const EXCLUDE_LIBS: &str = "dyn.md D-110 item 1: an entry is an archive's file name (ALL: every archive); an entry without .a that names an archive's file name without its .a (libfoo for libfoo.a) is refused by name (GNU ld matches it, lld and mold do not)";

/// The options whose effect exists only in a shared object (`spec/dyn.md`
/// D-102, WP-123): supported in a link whose output kind is `Shared`;
/// elsewhere each is refused by name (`Refused{Option, spelling}`) with
/// this note (GNU ld writes `DT_SONAME` into an executable, mortar does not
/// write it there).
pub(crate) static SHARED_ONLY: &[(&str, &str)] = &[
    ("-h", "dyn.md D-102: mortar writes DT_SONAME in a shared object (-shared) only"),
    (
        "--version-script",
        "dyn.md D-114: mortar applies a version script in a shared object (-shared) only; for an executable the references disagree (GNU ld exports the node names, lld exports nothing without -E, mold exports the script's global entries)",
    ),
    // `spec/dyn.md` D-103 (WP-128): in an executable GNU ld drops these three
    // and lld and mold write them; D-103 keeps them at their M2 status there.
    ("-z nodelete", Z_EXEC_DIFFER),
    ("-z nodlopen", Z_EXEC_DIFFER),
    ("-z initfirst", Z_EXEC_DIFFER),
    // `-z interpose` and `-z origin`: all three write the flag in every
    // executable kind; D-103 keeps them at their M2 status there too.
    ("-z interpose", Z_EXEC_SHARED_ONLY),
    ("-z origin", Z_EXEC_SHARED_ONLY),
];

/// The options whose effect in a `Shared` link a later package implements
/// (`spec/cli.md` §4.2's M4 block): in a link whose output kind is
/// `Shared` each is refused by name (`Refused{Option, spelling}`) with
/// this note, whatever its status; elsewhere its row's status holds.
/// Empty since WP-128 landed the option tail.
pub(crate) static SHARED_LATER: &[(&str, &str)] = &[];

/// The table.
#[rustfmt::skip]
pub(crate) static ROWS: &[Row] = &[
    r(&[s("o"), g("output")], Arg::Required(Values::Any), "FILE", GLM, &[], Group::Output, Supported, Lands::Output, "Write the output to FILE (default a.out)").at(acts("", "C-R3, I-R1")),
    r(&[g1("shared"), g("Bshareable")], Arg::None, "", GLM, &[], Group::Output, Partial { refused: &[], note: SHARED }, Lands::Kind(KindSet::Shared), "Produce a shared object").at(conflicting_r("C-R1")),
    r(&[g1("pie"), g("pic-executable")], Arg::None, "", GLM, &[], Group::Output, Supported, Lands::Kind(KindSet::Pie), "Produce a position-independent executable").at(conflicting_r("C-R1")),
    r(&[g1("no-pie"), x("no-pic-executable")], Arg::None, "", GLM, &[], Group::Output, Supported, Lands::Kind(KindSet::NoPie), "Produce a position-dependent executable (default)").at(noop_r("", "C-R2")),
    r(&[s("r"), s("i"), g("relocatable")], Arg::None, "", GLM, &[], Group::Output, Refused, Lands::Kind(KindSet::Relocatable), "Produce a relocatable object").at(acts_until("the kind", "C-R1", "RL-7", "relocatable output (-r)")),
    r(&[s("m")], Arg::Required(Values::Any), "EMULATION", GLM, &[], Group::Output, Supported, Lands::Emulation, "Link for EMULATION: aarch64linux, aarch64elf, elf_x86_64, elf_i386").at(noop_r("a wrong emulation still refused", "C-R2")),
    r(&[s("e"), g("entry")], Arg::Required(Values::Any), "SYMBOL", GLM, &[], Group::Output, Supported, Lands::Entry, "Start execution at SYMBOL").at(acts("accepted, e_entry 0, not a root", "C-R3, L-R7, RV-R2")),
    r(&[g("oformat")], Arg::Required(Values::Any), "FORMAT", GLM, &[], Group::Output, Refused, Lands::Other, "Write the output in FORMAT").at(FINAL),
    r(&[g1("Ur")], Arg::None, "", G, &[], Group::Output, Refused, Lands::Ur, "Produce a relocatable object with constructor tables built").at(acts_until("as `-r` (`plan.ur`)", "C-R1", "RL-7", "relocatable output (-r)")),
    r(&[s("d"), g("dc"), g("dp")], Arg::None, "", GLM, &[], Group::Output, Refused, Lands::DefineCommon, "Allocate common symbols even in a relocatable link").at(acts_until("commons allocated", "C-R3, RV-R5", "RL-11", "-d with -r")),
    r(&[g("no-define-common")], Arg::None, "", G, &[], Group::Output, Refused, Lands::Other, "Leave common symbols unallocated").at(FINAL),
    r(&[s("q"), g("emit-relocs")], Arg::None, "", GLM, &[], Group::Output, Refused, Lands::EmitRelocs, "Keep relocations in the output").at(noop_r("the class at -r wins", "C-R2, C-R2b")),
    r(&[s("s"), g("strip-all")], Arg::None, "", GLM, &[], Group::Output, Supported, Lands::Strip(Strip::All), "Omit all symbol information").at(acts_until("", "C-R3, I-R9", "RL-11", "-s with -r")),
    r(&[s("S"), g("strip-debug")], Arg::None, "", GLM, &[], Group::Output, Supported, Lands::Strip(Strip::Debug), "Omit debugging information").at(acts_until("", "C-R3, I-R9", "RL-11", "-S with -r")),
    r(&[g("strip-discarded")], Arg::None, "", G, &[], Group::Output, Noop("synth.md S-76: locals of sections that are not live are always dropped (the default)"), Lands::Other, "Omit symbols of discarded sections").at(noop_r("measured", "C-R2b, rl0-opts")),
    r(&[g("no-strip-discarded")], Arg::None, "", G, &[], Group::Output, Refused, Lands::Other, "Keep symbols of discarded sections").at(FINAL),
    r(&[s("x"), g("discard-all")], Arg::None, "", GLM, &[], Group::Output, Supported, Lands::Discard(Discard::All), "Omit all local symbols").at(acts_until("", "C-R3, I-R9", "RL-11", "-x with -r")),
    r(&[s("X"), g("discard-locals")], Arg::None, "", GLM, &[], Group::Output, Supported, Lands::Discard(Discard::Locals), "Omit temporary local symbols").at(noop_r("", "C-R2")),
    r(&[g("discard-none")], Arg::None, "", GL, &[], Group::Output, Supported, Lands::Discard(Discard::None), "Keep every local symbol").at(noop_r("measured", "C-R2b, rl0-opts")),
    r(&[s("n"), g("nmagic")], Arg::None, "", GLM, &[], Group::Output, Refused, Lands::Magic(Magic::Nmagic), "Do not page-align sections").at(FINAL),
    r(&[s("N"), g("omagic")], Arg::None, "", GLM, &[], Group::Output, Refused, Lands::Magic(Magic::Omagic), "Do not page-align sections; make text writable").at(FINAL),
    r(&[g2("no-omagic")], Arg::None, "", GLM, &[], Group::Output, Refused, Lands::Magic(Magic::Paged), "Page-align sections; text read-only (default)").at(FINAL),
    r(&[x("no-nmagic")], Arg::None, "", LM, &[], Group::Output, Refused, Lands::Magic(Magic::Paged), "Page-align sections (default)").at(FINAL),
    r(&[g("build-id")], Arg::Optional(Values::BuildId), "STYLE", GLM, &[], Group::Output, Partial { refused: &["md5", "uuid"], note: "synth.md S-91: md5 is not written and uuid is M2; mortar writes --build-id, =sha1, =sha256, =fast, =none and =0xHEX" }, Lands::BuildId, "Write a build-ID note: md5, sha1, sha256, fast, uuid, none or 0xHEX").at(acts("the note over the -r output", "C-R3, I-R8")),
    r(&[x("no-build-id")], Arg::None, "", M, &[], Group::Output, Supported, Lands::NoBuildId, "Write no build-ID note").at(noop_r("", "C-R2")),
    r(&[g("compress-debug-sections")], Arg::Required(Values::Choice(&["none", "zlib", "zlib-gnu", "zlib-gabi", "zstd"])), "TYPE", GLM, &[], Group::Output, Partial { refused: &["zlib", "zlib-gnu", "zlib-gabi", "zstd"], note: "image.md: debug sections are written uncompressed; mortar writes =none only" }, Lands::CompressDebugSections, "Compress the debugging sections").at(refused_r("`--compress-debug-sections with -r`; `=none` no-op", "C-R4")),
    r(&[g("eh-frame-hdr")], Arg::None, "", GLM, &[], Group::Output, Supported, Lands::EhFrameHdr(true), "Write .eh_frame_hdr and PT_GNU_EH_FRAME").at(noop_r("", "C-R2")),
    r(&[g("no-eh-frame-hdr")], Arg::None, "", GLM, &[], Group::Output, Supported, Lands::EhFrameHdr(false), "Write no .eh_frame_hdr").at(noop_r("measured", "C-R2b, rl0-opts")),
    r(&[g("package-metadata")], Arg::Optional(Values::Any), "JSON", GM, &[], Group::Output, Refused, Lands::Other, "Write a .note.package note").at(FINAL),
    r(&[g("gc-sections")], Arg::None, "", GLM, &[], Group::Output, Supported, Lands::GcSections(true), "Remove sections nothing refers to").at(refused_r("`--gc-sections with -r`", "C-R4, I-R10")),
    r(&[g("no-gc-sections")], Arg::None, "", GLM, &[], Group::Output, Supported, Lands::GcSections(false), "Keep every section (default)").at(noop_r("measured", "C-R2b, rl0-opts")),
    r(&[g("print-gc-sections")], Arg::None, "", GLM, &[], Group::Output, Supported, Lands::PrintGcSections(true), "List the sections --gc-sections removes").at(noop_r("measured", "C-R2b, rl0-opts")),
    r(&[g("no-print-gc-sections")], Arg::None, "", GLM, &[], Group::Output, Supported, Lands::PrintGcSections(false), "Do not list removed sections (default)").at(noop_r("measured", "C-R2b, rl0-opts")),
    r(&[g("gc-keep-exported")], Arg::None, "", G, &[], Group::Output, Supported, Lands::GcKeepExported, "Keep sections that define exported symbols").at(noop_r("measured", "C-R2b, rl0-opts")),
    r(&[x("icf")], Arg::Required(Values::Choice(&["all", "safe", "none"])), "MODE", LM, &[], Group::Output, Refused, Lands::Icf, "Fold identical code").at(refused_r("`--icf with -r`", "C-R4")),
    r(&[x("no-icf")], Arg::None, "", M, &[], Group::Output, Refused, Lands::NoIcf, "Fold nothing (default)").at(FINAL),
    r(&[x("print-icf-sections")], Arg::None, "", LM, &[], Group::Output, Refused, Lands::Other, "List the folded sections").at(FINAL),
    r(&[x("no-print-icf-sections")], Arg::None, "", LM, &[], Group::Output, Refused, Lands::Other, "Do not list folded sections (default)").at(FINAL),
    r(&[x("keep-unique")], Arg::Required(Values::Any), "SYMBOL", L, &[], Group::Output, Refused, Lands::Other, "Do not fold SYMBOL").at(FINAL),
    r(&[x("ignore-data-address-equality")], Arg::None, "", LM, &[], Group::Output, Refused, Lands::Other, "Allow folding data whose address is compared").at(FINAL),
    r(&[x("ignore-function-address-equality")], Arg::None, "", L, &[], Group::Output, Refused, Lands::Other, "Allow folding functions whose address is compared").at(FINAL),
    r(&[x("gdb-index")], Arg::None, "", LM, &[], Group::Output, Supported, Lands::GdbIndex(true), "Write a .gdb_index section").at(refused_r("`--gdb-index with -r`", "C-R4")),
    r(&[x("no-gdb-index")], Arg::None, "", LM, &[], Group::Output, Noop("the default: no .gdb_index is written; the last of --gdb-index and --no-gdb-index wins"), Lands::GdbIndex(false), "Write no .gdb_index section (default)").at(noop_r("measured", "C-R2b, rl0-opts (lld)")),
    r(&[x("debug-names")], Arg::None, "", L, &[], Group::Output, Refused, Lands::Other, "Write a merged .debug_names section").at(FINAL),
    r(&[x("no-debug-names")], Arg::None, "", L, &[], Group::Output, Noop("the default: no merged .debug_names is written"), Lands::Other, "Write no merged .debug_names (default)").at(noop_r("measured", "C-R2b, rl0-opts (lld)")),
    r(&[g("relax")], Arg::None, "", GLM, &[], Group::Output, Refused, Lands::Other, "Relax instructions where the target allows").at(refused_r("`--relax with -r`", "C-R4")),
    r(&[g("no-relax")], Arg::None, "", GLM, &[], Group::Output, Refused, Lands::Other, "Do not relax instructions").at(noop_r("the class at -r wins", "C-R2, C-R2b")),
    r(&[g("force-group-allocation")], Arg::None, "", GL, &[], Group::Output, Refused, Lands::ForceGroupAllocation, "Place section group members as ordinary sections").at(acts_until("", "C-R3, L-R3", "RL-8", "--force-group-allocation with -r")),
    r(&[g("traditional-format")], Arg::None, "", G, &[], Group::Output, Refused, Lands::Other, "Write what the system's native linker would").at(FINAL),
    r(&[g("embedded-relocs")], Arg::None, "", G, &[], Group::Output, Refused, Lands::Other, "Write embedded relocations (MIPS)").at(FINAL),
    r(&[g("force-exe-suffix")], Arg::None, "", G, &[], Group::Output, Refused, Lands::Other, "Add .exe to the output name").at(FINAL),
    r(&[g("out-implib")], Arg::Required(Values::Any), "FILE", GL, &[], Group::Output, Refused, Lands::Other, "Write an import library").at(FINAL),
    r(&[x("execute-only")], Arg::None, "", LM, &[], Group::Output, Refused, Lands::Other, "Make executable segments unreadable").at(FINAL),
    r(&[x("no-execute-only")], Arg::None, "", L, &[], Group::Output, Refused, Lands::Other, "Make executable segments readable (default)").at(FINAL),
    r(&[x("compress-sections")], Arg::Required(Values::Any), "GLOB=TYPE", L, &[], Group::Output, Refused, Lands::Other, "Compress the non-alloc output sections GLOB names").at(FINAL),
    r(&[x("separate-debug-file")], Arg::Optional(Values::Any), "FILE", M, &[], Group::Output, Refused, Lands::Other, "Write the debugging information to a separate file").at(FINAL),
    r(&[x("no-separate-debug-file")], Arg::None, "", M, &[], Group::Output, Refused, Lands::Other, "Keep the debugging information in the output (default)").at(FINAL),
    r(&[s("l"), g("library")], Arg::Required(Values::Any), "NAME", GLM, &[], Group::Inputs, Supported, Lands::Library, "Link libNAME (or :FILE, by file name)").at(noop_r("picks the inputs, extraction by RV-R2", "C-R2, RV-R2")),
    r(&[s("R"), g("just-symbols")], Arg::Required(Values::Any), "FILE", GLM, &[], Group::Inputs, Refused, Lands::JustSymbols, "Take only the symbols of FILE").at(FINAL),
    r(&[s("b"), g("format")], Arg::Required(Values::Any), "FORMAT", GL, &[], Group::Inputs, Refused, Lands::State(StateOp::Format), "Read the following inputs as FORMAT").at(acts_until("`binary` (L-R9)", "C-R3, L-R9", "RL-10", "-b with -r")),
    r(&[s("("), g("start-group")], Arg::None, "", GLM, &[], Group::Inputs, Supported, Lands::State(StateOp::StartGroup), "Start a group of archives searched repeatedly").at(noop_r("acts on extraction", "C-R2, RV-R2")),
    r(&[s(")"), g("end-group")], Arg::None, "", GLM, &[], Group::Inputs, Supported, Lands::State(StateOp::EndGroup), "End a group").at(noop_r("", "C-R2")),
    r(&[x("start-lib")], Arg::None, "", LM, &[], Group::Inputs, Supported, Lands::State(StateOp::StartLib), "Read the following objects as members of an archive").at(final_r("lazy objects (lld's reading)", "C-R2b, RV-R2")),
    r(&[x("end-lib")], Arg::None, "", LM, &[], Group::Inputs, Supported, Lands::State(StateOp::EndLib), "End --start-lib").at(final_r("with `--start-lib`", "RV-R2")),
    r(&[g("as-needed")], Arg::None, "", GLM, &[], Group::Inputs, Supported, Lands::State(StateOp::AsNeeded(true)), "Record DT_NEEDED for the following shared objects only if used").at(noop_r("", "C-R2")),
    r(&[g("no-as-needed")], Arg::None, "", GLM, &[], Group::Inputs, Supported, Lands::State(StateOp::AsNeeded(false)), "Record DT_NEEDED for every following shared object (default)").at(noop_r("", "C-R2")),
    r(&[g("whole-archive")], Arg::None, "", GLM, &[], Group::Inputs, Supported, Lands::State(StateOp::WholeArchive(true)), "Take every member of the following archives").at(noop_r("acts on extraction", "C-R2, RV-R2")),
    r(&[g("no-whole-archive")], Arg::None, "", GLM, &[], Group::Inputs, Supported, Lands::State(StateOp::WholeArchive(false)), "Take the members of archives that are needed (default)").at(noop_r("", "C-R2")),
    r(&[g1("Bstatic"), g("dn"), g("non_shared"), g("static")], Arg::None, "", GLM, &[], Group::Inputs, Supported, Lands::State(StateOp::Static(true)), "Search -l for archives only").at(noop_r("a `.so` is refused", "C-R2, C-R5")),
    r(&[g1("Bdynamic"), g("dy"), g("call_shared")], Arg::None, "", GLM, &[], Group::Inputs, Supported, Lands::State(StateOp::Static(false)), "Search -l for shared objects too (default)").at(noop_r("a `.so` is refused", "C-R2, C-R5")),
    r(&[g("copy-dt-needed-entries"), g("add-needed")], Arg::None, "", G, &[], Group::Inputs, Refused, Lands::State(StateOp::CopyDtNeeded(true)), "Follow the DT_NEEDED entries of the following shared objects").at(FINAL),
    r(&[g("no-copy-dt-needed-entries"), g("no-add-needed")], Arg::None, "", GLM, &[], Group::Inputs, Noop(NO_COPY_DT_NEEDED), Lands::State(StateOp::CopyDtNeeded(false)), "Do not follow DT_NEEDED entries (default)").at(noop_r("measured", "C-R2b, rl0-opts")),
    r(&[g("push-state")], Arg::None, "", GLM, &[], Group::Inputs, Supported, Lands::State(StateOp::Push), "Save the positional state").at(noop_r("", "C-R2")),
    r(&[g("pop-state")], Arg::None, "", GLM, &[], Group::Inputs, Supported, Lands::State(StateOp::Pop), "Restore the positional state last saved").at(noop_r("", "C-R2")),
    r(&[g("accept-unknown-input-arch")], Arg::None, "", G, &[], Group::Inputs, Refused, Lands::Other, "Accept inputs of an unknown architecture").at(FINAL),
    r(&[g("no-accept-unknown-input-arch")], Arg::None, "", G, &[], Group::Inputs, Refused, Lands::Other, "Reject inputs of an unknown architecture (default)").at(FINAL),
    r(&[g("no-warn-mismatch")], Arg::None, "", GL, &[], Group::Inputs, Refused, Lands::Other, "Link inputs that do not match the output's machine").at(FINAL),
    r(&[g("no-warn-search-mismatch")], Arg::None, "", G, &[], Group::Inputs, Refused, Lands::Other, "Do not warn about skipped incompatible libraries").at(FINAL),
    r(&[g("remap-inputs-file")], Arg::Required(Values::Any), "FILE", GL, &[], Group::Inputs, Refused, Lands::Other, "Remap inputs as FILE says").at(FINAL),
    r(&[g("remap-inputs")], Arg::Required(Values::Any), "PATTERN=FILE", GL, &[], Group::Inputs, Refused, Lands::Other, "Read FILE for the inputs matching PATTERN").at(FINAL),
    r(&[x("dependent-libraries")], Arg::None, "", L, &[], Group::Inputs, Refused, Lands::Other, "Link the libraries inputs ask for (default)").at(FINAL),
    r(&[x("no-dependent-libraries")], Arg::None, "", L, &[], Group::Inputs, Noop("mortar never loads the libraries .deplibs sections name (resolve.md §10)"), Lands::Other, "Ignore the libraries inputs ask for (mortar's behavior, always on)").at(noop_r("measured", "C-R2b, rl0-opts (lld)")),
    r(&[x("fortran-common")], Arg::None, "", L, &[], Group::Inputs, Refused, Lands::Other, "Take archive members that define common symbols (mortar's behavior, always on)").at(FINAL),
    r(&[x("no-fortran-common")], Arg::None, "", L, &[], Group::Inputs, Refused, Lands::Other, "Do not take archive members for common symbols").at(FINAL),
    r(&[x("warn-backrefs")], Arg::None, "", L, &[], Group::Inputs, Supported, Lands::WarnBackrefs(true), "Warn when an archive member is taken for an earlier reference").at(final_r("a diagnostic", "rl0-opts")),
    r(&[x("no-warn-backrefs")], Arg::None, "", L, &[], Group::Inputs, Supported, Lands::WarnBackrefs(false), "Do not warn about backward references (default)").at(final_r("", "rl0-opts")),
    r(&[x("warn-backrefs-exclude")], Arg::Required(Values::Any), "GLOB", L, &[], Group::Inputs, Refused, Lands::Other, "Do not warn about backward references into GLOB").at(FINAL),
    r(&[x("why-extract")], Arg::Required(Values::Any), "FILE", L, &[], Group::Inputs, Refused, Lands::Other, "Write why each archive member is taken to FILE").at(FINAL),
    r(&[x("print-archive-stats")], Arg::Required(Values::Any), "FILE", L, &[], Group::Inputs, Refused, Lands::Other, "Write archive statistics to FILE").at(FINAL),
    r(&[x("fat-lto-objects")], Arg::None, "", L, &[], Group::Inputs, Refused, Lands::Other, "Read the bitcode of fat LTO objects").at(FINAL),
    r(&[x("no-fat-lto-objects")], Arg::None, "", L, &[], Group::Inputs, Refused, Lands::Other, "Read the code of fat LTO objects (default)").at(FINAL),
    r(&[s("L"), g("library-path")], Arg::Required(Values::Any), "DIR", GLM, &[], Group::Search, Supported, Lands::LibraryPath, "Search DIR for -l libraries and scripts").at(noop_r("", "C-R2")),
    r(&[g("sysroot")], Arg::Required(Values::Any), "DIR", GLM, &[], Group::Search, Supported, Lands::Sysroot, "Use DIR as the system root").at(final_r("the search", "rl0-opts")),
    r(&[g1("nostdlib")], Arg::None, "", GLM, &[], Group::Search, Refused, Lands::NoStdlib, "Do not search the SEARCH_DIR directories of linker scripts").at(FINAL),
    r(&[g1("rpath-link")], Arg::Required(Values::Any), "DIR", GLM, &[], Group::Search, Supported, Lands::RpathLink, "Search DIR for the DT_NEEDED entries of shared objects; a :-separated list").at(noop_r("measured", "C-R2b, rl0-opts")),
    r(&[s("Y")], Arg::Required(Values::Any), "PATH", G, &[], Group::Search, Refused, Lands::Other, "Default search path for Solaris compatibility").at(FINAL),
    r(&[sx("C"), x("directory")], Arg::Required(Values::Any), "DIR", M, &[], Group::Search, Refused, Lands::Other, "Change to DIR first").at(FINAL),
    r(&[x("chroot")], Arg::Required(Values::Any), "DIR", LM, &[], Group::Search, Refused, Lands::Other, "Prefix every path with DIR").at(FINAL),
    r(&[s("T"), g("script")], Arg::Required(Values::Any), "FILE", GLM, &[], Group::Scripts, Refused, Lands::Script(ScriptRole::Script), "Read the linker script FILE").at(acts_until("", "C-R3", "RL-11", "-T with -r")),
    r(&[g("default-script"), g("dT")], Arg::Required(Values::Any), "FILE", GL, &[], Group::Scripts, Refused, Lands::Script(ScriptRole::DefaultScript), "Use FILE as the default linker script").at(FINAL),
    r(&[s("c"), g("mri-script")], Arg::Required(Values::Any), "FILE", G, &[], Group::Scripts, Refused, Lands::Other, "Read the MRI script FILE").at(FINAL),
    r(&[g("version-script")], Arg::Required(Values::Any), "FILE", GLM, &[], Group::Scripts, Supported, Lands::Script(ScriptRole::VersionScript), "Read the version script FILE").at(noop_r("", "C-R2")),
    r(&[g("dynamic-list")], Arg::Required(Values::Any), "FILE", GLM, &[], Group::Scripts, Supported, Lands::Script(ScriptRole::DynamicList), "Read the dynamic list FILE").at(conflicting_r("C-R4")),
    r(&[g("export-dynamic-symbol-list")], Arg::Required(Values::Any), "FILE", GLM, &[], Group::Scripts, Refused, Lands::Script(ScriptRole::ExportDynamicSymbolList), "Export the symbols FILE lists").at(conflicting_r("C-R4")),
    r(&[g("retain-symbols-file")], Arg::Required(Values::Any), "FILE", GLM, &[], Group::Scripts, Refused, Lands::Script(ScriptRole::RetainSymbolsFile), "Keep only the symbols FILE lists").at(FINAL),
    r(&[g("section-ordering-file")], Arg::Required(Values::Any), "FILE", G, &[], Group::Scripts, Refused, Lands::Other, "Order sections as FILE says").at(FINAL),
    r(&[x("symbol-ordering-file")], Arg::Required(Values::Any), "FILE", L, &[], Group::Scripts, Refused, Lands::Other, "Order sections by the symbols FILE lists").at(FINAL),
    r(&[x("no-warn-symbol-ordering")], Arg::None, "", L, &[], Group::Scripts, Refused, Lands::Other, "Do not warn about the symbol ordering file").at(FINAL),
    r(&[x("warn-symbol-ordering")], Arg::None, "", L, &[], Group::Scripts, Refused, Lands::Other, "Warn about the symbol ordering file (default)").at(FINAL),
    r(&[x("call-graph-ordering-file")], Arg::Required(Values::Any), "FILE", L, &[], Group::Scripts, Refused, Lands::Other, "Order sections by the call graph FILE gives").at(FINAL),
    r(&[x("call-graph-profile-sort")], Arg::Required(Values::Choice(&["none", "hfsort", "cdsort"])), "ALGORITHM", L, &[], Group::Scripts, Refused, Lands::Other, "Order sections by the call graph profile").at(noop_r("by what the option is", "C-R2b")),
    r(&[x("print-symbol-order")], Arg::Required(Values::Any), "FILE", L, &[], Group::Scripts, Refused, Lands::Other, "Write the symbol order used to FILE").at(FINAL),
    r(&[x("irpgo-profile")], Arg::Required(Values::Any), "FILE", L, &[], Group::Scripts, Refused, Lands::Other, "Read a temporal profile").at(FINAL),
    r(&[x("bp-startup-sort")], Arg::Required(Values::Choice(&["none", "function"])), "MODE", L, &[], Group::Scripts, Refused, Lands::Other, "Order sections for startup").at(noop_r("by what the option is", "C-R2b")),
    r(&[x("bp-compression-sort")], Arg::Required(Values::Choice(&["none", "function", "data", "both"])), "MODE", L, &[], Group::Scripts, Refused, Lands::Other, "Order sections for compression").at(noop_r("by what the option is", "C-R2b")),
    r(&[x("bp-compression-sort-startup-functions")], Arg::None, "", L, &[], Group::Scripts, Refused, Lands::Other, "Order startup functions for compression too").at(noop_r("by what the option is", "C-R2b")),
    r(&[x("verbose-bp-section-orderer")], Arg::None, "", L, &[], Group::Scripts, Refused, Lands::Other, "Report on balanced partitioning").at(FINAL),
    r(&[g("orphan-handling")], Arg::Required(Values::Choice(&["place", "discard", "warn", "error"])), "MODE", GL, &[], Group::Scripts, Refused, Lands::Other, "Handle sections no script statement places").at(noop_r("by what the option is", "C-R2b")),
    r(&[g("sort-common"), g("sort_common")], Arg::Optional(Values::Choice(&["ascending", "descending"])), "ORDER", GLM, &[], Group::Scripts, Supported, Lands::SortCommon, "Sort common symbols by alignment").at(noop_r("", "C-R2")),
    r(&[g("sort-section")], Arg::Required(Values::Choice(&["name", "alignment"])), "KEY", GLM, &[], Group::Scripts, Supported, Lands::SortSection, "Sort sections matched by wildcards").at(noop_r("", "C-R2")),
    r(&[g("unique")], Arg::Optional(Values::Any), "SECTION", GLM, &[], Group::Scripts, Refused, Lands::Unique, "Do not merge orphan sections (or SECTION)").at(acts_until("", "C-R3, C-R4, L-R11", "RL-11", "--unique with -r")),
    r(&[g("enable-non-contiguous-regions")], Arg::None, "", GL, &[], Group::Scripts, Refused, Lands::Other, "Spill sections into later matching memory regions").at(FINAL),
    r(&[g("enable-non-contiguous-regions-warnings")], Arg::None, "", G, &[], Group::Scripts, Refused, Lands::Other, "Warn where non-contiguous regions can surprise").at(FINAL),
    r(&[g("disable-linker-version")], Arg::None, "", G, &[], Group::Scripts, Refused, Lands::Other, "Make the LINKER_VERSION script command write nothing").at(FINAL),
    r(&[g("enable-linker-version")], Arg::None, "", G, &[], Group::Scripts, Refused, Lands::Other, "Make the LINKER_VERSION script command write the version").at(FINAL),
    r(&[g("check-sections")], Arg::None, "", GL, &[], Group::Scripts, Refused, Lands::Other, "Check output sections for overlaps (default)").at(FINAL),
    r(&[g("no-check-sections")], Arg::None, "", GL, &[], Group::Scripts, Refused, Lands::Other, "Do not check output sections for overlaps").at(FINAL),
    r(&[x("shuffle-sections")], Arg::Optional(Values::Any), "GLOB=SEED", LM, &[], Group::Scripts, Refused, Lands::Other, "Shuffle the sections GLOB matches").at(FINAL),
    r(&[x("reverse-sections")], Arg::None, "", M, &[], Group::Scripts, Refused, Lands::Other, "Reverse the order of input sections").at(FINAL),
    r(&[x("randomize-section-padding")], Arg::Required(Values::Any), "SEED", L, &[], Group::Scripts, Refused, Lands::Other, "Insert random padding between sections").at(FINAL),
    r(&[s("u"), g("undefined")], Arg::Required(Values::Any), "SYMBOL", GLM, &[], Group::Symbols, Supported, Lands::Symbol(SymbolRole::Undefined), "Start with an undefined reference to SYMBOL").at(acts("a root, and `UND` written", "C-R3, RV-R2, RV-R3")),
    r(&[g("require-defined")], Arg::Required(Values::Any), "SYMBOL", GM, &[], Group::Symbols, Supported, Lands::Symbol(SymbolRole::RequireDefined), "Require that SYMBOL is defined").at(acts("a root for extraction like `-u`, nothing written; undefined at the end reported as in a final link (RL-2 measures)", "C-R2b, RV-R2")),
    r(&[x("undefined-glob")], Arg::Required(Values::Any), "PATTERN", LM, &[], Group::Symbols, Refused, Lands::Other, "Add an undefined reference for each symbol PATTERN matches").at(FINAL),
    r(&[g("defsym")], Arg::Required(Values::Any), "SYMBOL=EXPR", GLM, &[], Group::Symbols, Partial { refused: &[], note: DEFSYM }, Lands::Symbol(SymbolRole::Defsym), "Define SYMBOL as EXPR").at(acts_until("", "C-R3, RV-R10", "RL-11", "--defsym with -r")),
    r(&[g("wrap")], Arg::Required(Values::Any), "SYMBOL", GLM, &[], Group::Symbols, Partial { refused: &[], note: WRAP }, Lands::Symbol(SymbolRole::Wrap), "Send references to SYMBOL to __wrap_SYMBOL").at(refused_r("`--wrap with -r`", "C-R4")),
    r(&[g("export-dynamic-symbol")], Arg::Required(Values::Any), "SYMBOL", GLM, &[], Group::Symbols, Supported, Lands::Symbol(SymbolRole::ExportDynamicSymbol), "Export SYMBOL (a glob)").at(at_r(AtRClass::Conflicting, "C-R4's `--export-dynamic*`", "C-R4")),
    r(&[g("exclude-libs")], Arg::Required(Values::Any), "LIBS", GLM, &[], Group::Symbols, Partial { refused: &[], note: EXCLUDE_LIBS }, Lands::Symbol(SymbolRole::ExcludeLibs), "Do not export the symbols of the archives LIBS").at(noop_r("", "C-R2")),
    r(&[s("y"), g("trace-symbol")], Arg::Required(Values::Any), "SYMBOL", GLM, &[], Group::Symbols, Refused, Lands::Symbol(SymbolRole::TraceSymbol), "Report each file that mentions SYMBOL").at(FINAL),
    r(&[g("ignore-unresolved-symbol")], Arg::Required(Values::Any), "SYMBOL", G, &[], Group::Symbols, Refused, Lands::Symbol(SymbolRole::IgnoreUnresolved), "Do not report SYMBOL as unresolved").at(FINAL),
    r(&[g1("init")], Arg::Required(Values::Any), "SYMBOL", GLM, &[], Group::Symbols, Supported, Lands::Init, "Call SYMBOL when the object is loaded").at(conflicting_r("C-R2b")),
    r(&[g1("fini")], Arg::Required(Values::Any), "SYMBOL", GLM, &[], Group::Symbols, Supported, Lands::Fini, "Call SYMBOL when the object is unloaded").at(conflicting_r("C-R2b")),
    r(&[g("demangle")], Arg::Optional(Values::Any), "STYLE", GLM, &[], Group::Symbols, Refused, Lands::Other, "Demangle symbol names in messages").at(FINAL),
    r(&[g("no-demangle")], Arg::None, "", GLM, &[], Group::Symbols, Refused, Lands::Other, "Do not demangle symbol names").at(FINAL),
    r(&[g("default-symver")], Arg::None, "", GM, &[], Group::Symbols, Refused, Lands::Other, "Give exported symbols the output's version").at(FINAL),
    r(&[g("default-imported-symver")], Arg::None, "", G, &[], Group::Symbols, Refused, Lands::Other, "Give imported symbols their library's version").at(FINAL),
    r(&[g("undefined-version")], Arg::None, "", GLM, &[], Group::Symbols, Supported, Lands::UndefinedVersion(true), "Accept version scripts that name undefined symbols").at(noop_r("measured", "C-R2b, rl0-opts")),
    r(&[g("no-undefined-version")], Arg::None, "", GLM, &[], Group::Symbols, Supported, Lands::UndefinedVersion(false), "Reject version scripts that name undefined symbols").at(noop_r("measured", "C-R2b, rl0-opts")),
    r(&[g("version-exports-section")], Arg::Required(Values::Any), "SYMBOL", G, &[], Group::Symbols, Refused, Lands::Other, "Take the exported symbols from .exports").at(FINAL),
    r(&[g("disable-multiple-abs-defs")], Arg::None, "", G, &[], Group::Symbols, Refused, Lands::Other, "Reject absolute symbols defined twice through -R").at(FINAL),
    r(&[x("gnu-unique")], Arg::None, "", L, &[], Group::Symbols, Refused, Lands::Other, "Keep the STB_GNU_UNIQUE binding (default)").at(FINAL),
    r(&[x("no-gnu-unique")], Arg::None, "", L, &[], Group::Symbols, Refused, Lands::Other, "Make STB_GNU_UNIQUE symbols global").at(FINAL),
    r(&[g("task-link")], Arg::Required(Values::Any), "SYMBOL", G, &[], Group::Symbols, Refused, Lands::Other, "Link at task level").at(FINAL),
    r(&[s("E"), g("export-dynamic")], Arg::None, "", GLM, &[], Group::Dynamic, Supported, Lands::ExportDynamic(true), "Export every symbol").at(conflicting_r("C-R4")),
    r(&[g("no-export-dynamic")], Arg::None, "", GLM, &[], Group::Dynamic, Supported, Lands::ExportDynamic(false), "Export only the symbols that need it (default)").at(noop_r("by what the option is", "C-R2b")),
    r(&[g1("Bsymbolic")], Arg::None, "", GLM, &[], Group::Dynamic, Supported, Lands::Bsymbolic(Some(Bsymbolic::All)), "Bind a shared object's references to its own definitions (DT_SYMBOLIC); nothing in an executable").at(conflicting_r("C-R4")),
    r(&[g1("Bsymbolic-functions")], Arg::None, "", GLM, &[], Group::Dynamic, Supported, Lands::Bsymbolic(Some(Bsymbolic::Functions)), "Bind references to global functions locally").at(conflicting_r("C-R4")),
    r(&[x("Bsymbolic-non-weak")], Arg::None, "", LM, &[], Group::Dynamic, Refused, Lands::Bsymbolic(Some(Bsymbolic::NonWeak)), "Bind references to non-weak globals locally").at(conflicting_r("C-R4")),
    r(&[x("Bsymbolic-non-weak-functions")], Arg::None, "", LM, &[], Group::Dynamic, Refused, Lands::Bsymbolic(Some(Bsymbolic::NonWeakFunctions)), "Bind references to non-weak global functions locally").at(conflicting_r("C-R4")),
    r(&[g1("Bno-symbolic")], Arg::None, "", GLM, &[], Group::Dynamic, Supported, Lands::Bsymbolic(None), "Undo -Bsymbolic (default)").at(noop_r("by what the option is", "C-R2b")),
    r(&[s("h"), g1("soname")], Arg::Required(Values::Any), "NAME", GLM, &[], Group::Dynamic, Supported, Lands::Soname, "Record NAME as DT_SONAME").at(conflicting_r("C-R4")),
    r(&[g1("rpath")], Arg::Required(Values::Any), "DIR", GLM, &[], Group::Dynamic, Supported, Lands::Rpath, "Add DIR to the run-time search path").at(noop_r("measured", "C-R4")),
    r(&[s("I"), g("dynamic-linker")], Arg::Required(Values::Any), "FILE", GLM, &[], Group::Dynamic, Supported, Lands::DynamicLinker, "Use FILE as the program interpreter").at(noop_r("", "C-R2")),
    r(&[g("no-dynamic-linker")], Arg::None, "", GLM, &[], Group::Dynamic, Supported, Lands::NoDynamicLinker, "Write no program interpreter").at(noop_r("", "C-R2")),
    r(&[g("enable-new-dtags")], Arg::None, "", GLM, &[], Group::Dynamic, Supported, Lands::NewDtags(true), "Write DT_RUNPATH for -rpath").at(conflicting_r("C-R4")),
    r(&[g("disable-new-dtags")], Arg::None, "", GLM, &[], Group::Dynamic, Supported, Lands::NewDtags(false), "Write DT_RPATH for -rpath").at(noop_r("by what the option is", "C-R2b")),
    r(&[g("hash-style")], Arg::Required(Values::Choice(&["sysv", "gnu", "both", "none"])), "STYLE", GLM, &[], Group::Dynamic, Partial { refused: &["sysv", "both", "none"], note: HASH_STYLE }, Lands::HashStyle, "Write DT_HASH (sysv), DT_GNU_HASH (gnu), both or none").at(noop_r("", "C-R2")),
    r(&[x("pack-dyn-relocs")], Arg::Required(Values::Choice(&["none", "relr", "android", "android+relr"])), "FORMAT", LM, &[], Group::Dynamic, Refused, Lands::PackDynRelocs, "Pack dynamic relocations").at(conflicting_r("C-R2b")),
    r(&[s("f"), g("auxiliary")], Arg::Required(Values::Any), "NAME", GLM, &[], Group::Dynamic, Refused, Lands::Other, "Record NAME as DT_AUXILIARY").at(FINAL),
    r(&[s("F"), g("filter")], Arg::Required(Values::Any), "NAME", GLM, &[], Group::Dynamic, Refused, Lands::Other, "Record NAME as DT_FILTER").at(FINAL),
    r(&[g("audit")], Arg::Required(Values::Any), "LIB", G, &[], Group::Dynamic, Refused, Lands::Other, "Record LIB as DT_AUDIT").at(FINAL),
    r(&[s("P"), g("depaudit")], Arg::Required(Values::Any), "LIB", G, &[], Group::Dynamic, Refused, Lands::Other, "Record LIB as DT_DEPAUDIT").at(FINAL),
    r(&[g1("Bgroup")], Arg::None, "", G, &[], Group::Dynamic, Refused, Lands::Other, "Use group lookup for this object's dependencies").at(FINAL),
    r(&[g("dynamic-list-data")], Arg::None, "", GM, &[], Group::Dynamic, Refused, Lands::Other, "Add the data symbols to the dynamic list").at(conflicting_r("C-R4")),
    r(&[g("dynamic-list-cpp-new")], Arg::None, "", G, &[], Group::Dynamic, Refused, Lands::Other, "Add operator new and delete to the dynamic list").at(conflicting_r("C-R4")),
    r(&[g("dynamic-list-cpp-typeinfo")], Arg::None, "", G, &[], Group::Dynamic, Refused, Lands::Other, "Add the C++ type information to the dynamic list").at(conflicting_r("C-R4")),
    r(&[g("allow-shlib-undefined")], Arg::None, "", GLM, &[], Group::Dynamic, Refused, Lands::AllowShlibUndefined(true), "Accept undefined symbols in shared objects").at(FINAL),
    r(&[g("no-allow-shlib-undefined")], Arg::None, "", GLM, &[], Group::Dynamic, Refused, Lands::AllowShlibUndefined(false), "Report undefined symbols in shared objects").at(FINAL),
    r(&[g("spare-dynamic-tags")], Arg::Required(Values::Count), "COUNT", GM, &[], Group::Dynamic, Refused, Lands::Other, "Reserve COUNT entries in .dynamic").at(FINAL),
    r(&[x("spare-program-headers")], Arg::Required(Values::Count), "COUNT", M, &[], Group::Dynamic, Refused, Lands::Other, "Reserve COUNT program headers").at(FINAL),
    r(&[x("apply-dynamic-relocs")], Arg::None, "", LM, &[], Group::Dynamic, Refused, Lands::Other, "Write link-time values where dynamic relocations apply").at(FINAL),
    r(&[x("use-android-relr-tags")], Arg::None, "", L, &[], Group::Dynamic, Refused, Lands::Other, "Use the Android RELR tags").at(FINAL),
    r(&[x("no-use-android-relr-tags")], Arg::None, "", L, &[], Group::Dynamic, Refused, Lands::Other, "Use the SHT_RELR tags (default)").at(FINAL),
    r(&[x("android-memtag-heap")], Arg::None, "", L, &[], Group::Dynamic, Refused, Lands::Other, "Ask the loader for MTE heap protection").at(FINAL),
    r(&[x("android-memtag-stack")], Arg::None, "", L, &[], Group::Dynamic, Refused, Lands::Other, "Ask the loader for MTE stack protection").at(FINAL),
    r(&[x("android-memtag-mode")], Arg::Required(Values::Choice(&["async", "sync", "none"])), "MODE", L, &[], Group::Dynamic, Refused, Lands::Other, "The MTE mode the loader starts in").at(conflicting_r("C-R2b")),
    r(&[g1("Ttext")], Arg::Required(Values::Any), "ADDRESS", GLM, &[], Group::Layout, Refused, Lands::Start(StartKind::Text), "Place .text at ADDRESS").at(FINAL),
    r(&[g1("Tdata")], Arg::Required(Values::Any), "ADDRESS", GLM, &[], Group::Layout, Refused, Lands::Start(StartKind::Data), "Place .data at ADDRESS").at(FINAL),
    r(&[g1("Tbss")], Arg::Required(Values::Any), "ADDRESS", GLM, &[], Group::Layout, Refused, Lands::Start(StartKind::Bss), "Place .bss at ADDRESS").at(FINAL),
    r(&[g1("Ttext-segment")], Arg::Required(Values::Any), "ADDRESS", GL, &[], Group::Layout, Refused, Lands::Start(StartKind::TextSegment), "Place the text segment at ADDRESS").at(FINAL),
    r(&[g1("Trodata-segment")], Arg::Required(Values::Any), "ADDRESS", G, &[], Group::Layout, Refused, Lands::Start(StartKind::RodataSegment), "Place the read-only data segment at ADDRESS").at(FINAL),
    r(&[g1("Tldata-segment")], Arg::Required(Values::Any), "ADDRESS", G, &[], Group::Layout, Refused, Lands::Start(StartKind::LdataSegment), "Place the large data segment at ADDRESS").at(FINAL),
    r(&[g("section-start")], Arg::Required(Values::Any), "SECTION=ADDRESS", GLM, &[], Group::Layout, Refused, Lands::Start(StartKind::Section), "Place SECTION at ADDRESS").at(FINAL),
    r(&[g("image-base")], Arg::Required(Values::Any), "ADDRESS", GLM, &[], Group::Layout, Refused, Lands::ImageBase, "Place the image at ADDRESS").at(FINAL),
    r(&[g("rosegment")], Arg::None, "", GLM, &[], Group::Layout, Noop("layout.md L-65: read-only data always has a PT_LOAD of its own"), Lands::Other, "Put read-only data in its own segment").at(noop_r("measured", "C-R2b, rl0-opts")),
    r(&[g("no-rosegment")], Arg::None, "", GLM, &[], Group::Layout, Refused, Lands::Other, "Do not give read-only data its own segment").at(FINAL),
    r(&[x("xosegment")], Arg::None, "", L, &[], Group::Layout, Refused, Lands::Other, "Put execute-only sections in their own segment").at(FINAL),
    r(&[x("no-xosegment")], Arg::None, "", L, &[], Group::Layout, Refused, Lands::Other, "Do not give execute-only sections their own segment (default)").at(FINAL),
    r(&[g("split-by-file")], Arg::Optional(Values::Count), "SIZE", G, &[], Group::Layout, Refused, Lands::Other, "Split output sections every SIZE bytes, at file boundaries").at(FINAL),
    r(&[g("split-by-reloc")], Arg::Optional(Values::Count), "COUNT", G, &[], Group::Layout, Refused, Lands::Other, "Split output sections every COUNT relocations").at(FINAL),
    r(&[s("G"), g("gpsize")], Arg::Required(Values::Count), "SIZE", GL, &[], Group::Layout, Refused, Lands::Other, "Small data size").at(FINAL),
    r(&[x("optimize-bb-jumps")], Arg::None, "", L, &[], Group::Layout, Refused, Lands::Other, "Remove jumps to the next basic block").at(FINAL),
    r(&[x("no-optimize-bb-jumps")], Arg::None, "", L, &[], Group::Layout, Refused, Lands::Other, "Keep jumps to the next basic block (default)").at(FINAL),
    r(&[x("branch-to-branch")], Arg::None, "", L, &[], Group::Layout, Refused, Lands::Other, "Retarget branches to branches").at(FINAL),
    r(&[x("no-branch-to-branch")], Arg::None, "", L, &[], Group::Layout, Refused, Lands::Other, "Do not retarget branches to branches").at(FINAL),
    r(&[x("error-limit")], Arg::Required(Values::Count), "N", L, &[], Group::Diagnostics, Supported, Lands::ErrorLimit, "Show at most N errors; 0 shows all (default 20)").at(final_r("", "rl0-opts (lld)")),
    r(&[g("fatal-warnings")], Arg::None, "", GLM, &[], Group::Diagnostics, Supported, Lands::FatalWarnings(true), "Treat warnings as errors").at(final_r("", "rl0-opts")),
    r(&[g("no-fatal-warnings")], Arg::None, "", GLM, &[], Group::Diagnostics, Supported, Lands::FatalWarnings(false), "Do not treat warnings as errors (default)").at(final_r("", "rl0-opts")),
    r(&[s("w"), g("no-warnings")], Arg::None, "", GL, &[], Group::Diagnostics, Supported, Lands::NoWarnings, "Report no warnings").at(final_r("never hides a refusal", "rl0-opts, diag §4.3")),
    r(&[x("color-diagnostics")], Arg::Optional(Values::Choice(&["auto", "always", "never"])), "WHEN", LM, &[], Group::Diagnostics, Supported, Lands::Color, "Color diagnostics: always (alone), auto (default) or never").at(final_r("", "rl0-opts (lld)")),
    r(&[x("no-color-diagnostics")], Arg::None, "", L, &[], Group::Diagnostics, Supported, Lands::NoColor, "Do not color diagnostics").at(final_r("", "rl0-opts (lld)")),
    r(&[g("no-undefined")], Arg::None, "", GLM, &[], Group::Diagnostics, Supported, Lands::NoUndefined, "Report undefined symbols even in a shared object").at(noop_r("nothing is undefined at -r", "C-R2")),
    r(&[g("unresolved-symbols")], Arg::Required(Values::Choice(&["report-all", "ignore-all", "ignore-in-object-files", "ignore-in-shared-libs"])), "METHOD", GLM, &[], Group::Diagnostics, Refused, Lands::UnresolvedSymbols, "How to treat unresolved symbols").at(noop_r("by what the option is", "C-R2b")),
    r(&[g("warn-unresolved-symbols")], Arg::None, "", GLM, &[], Group::Diagnostics, Refused, Lands::WarnUnresolved(true), "Report unresolved symbols as warnings").at(FINAL),
    r(&[g("error-unresolved-symbols")], Arg::None, "", GLM, &[], Group::Diagnostics, Refused, Lands::WarnUnresolved(false), "Report unresolved symbols as errors (default)").at(FINAL),
    r(&[g("allow-multiple-definition")], Arg::None, "", GLM, &[], Group::Diagnostics, Supported, Lands::AllowMultipleDefinition(true), "Take the first of several definitions of a symbol").at(final_r("", "RV-R4")),
    r(&[x("no-allow-multiple-definition")], Arg::None, "", L, &[], Group::Diagnostics, Supported, Lands::AllowMultipleDefinition(false), "Report symbols defined more than once (default)").at(final_r("", "RV-R4")),
    r(&[g("noinhibit-exec"), g("noinhibit_exec")], Arg::None, "", GLM, &[], Group::Diagnostics, Refused, Lands::NoinhibitExec, "Write the output even after errors").at(FINAL),
    r(&[g("error-handling-script")], Arg::Required(Values::Any), "SCRIPT", GL, &[], Group::Diagnostics, Refused, Lands::Other, "Run SCRIPT for undefined symbols and missing libraries").at(FINAL),
    r(&[g("warn-common")], Arg::None, "", GLM, &[], Group::Diagnostics, Supported, Lands::WarnCommon(true), "Warn about common symbols").at(noop_r("", "C-R2")),
    r(&[x("no-warn-common")], Arg::None, "", LM, &[], Group::Diagnostics, Supported, Lands::WarnCommon(false), "Do not warn about common symbols (default)").at(final_r("", "rl0-opts (lld)")),
    r(&[g("warn-constructors")], Arg::None, "", GM, &[], Group::Diagnostics, Refused, Lands::Other, "Warn about global constructors").at(FINAL),
    r(&[g("warn-multiple-gp")], Arg::None, "", G, &[], Group::Diagnostics, Refused, Lands::Other, "Warn about several GP values").at(FINAL),
    r(&[g("warn-once")], Arg::None, "", GLM, &[], Group::Diagnostics, Refused, Lands::Other, "Warn about each undefined symbol once").at(FINAL),
    r(&[g("warn-section-align")], Arg::None, "", G, &[], Group::Diagnostics, Refused, Lands::Other, "Warn when alignment moves a section's start").at(FINAL),
    r(&[g("warn-textrel"), g("warn-shared-textrel")], Arg::None, "", GLM, &[], Group::Diagnostics, Refused, Lands::Other, "Warn about text relocations").at(FINAL),
    r(&[g("warn-alternate-em")], Arg::None, "", G, &[], Group::Diagnostics, Refused, Lands::Other, "Warn about objects with an alternate ELF machine code").at(FINAL),
    r(&[g("warn-execstack-objects")], Arg::None, "", G, &[], Group::Diagnostics, Refused, Lands::Other, "Warn about objects that ask for an executable stack").at(FINAL),
    r(&[g("warn-execstack")], Arg::None, "", GLM, &[], Group::Diagnostics, Noop("layout.md L-84: the executable-stack warning is the default"), Lands::Other, "Warn when the stack is made executable").at(final_r("", "rl0-opts")),
    r(&[g("no-warn-execstack")], Arg::None, "", GM, &[], Group::Diagnostics, Refused, Lands::Other, "Do not warn when the stack is made executable").at(FINAL),
    r(&[g("error-execstack")], Arg::None, "", G, &[], Group::Diagnostics, Refused, Lands::Other, "Make executable stack warnings errors").at(FINAL),
    r(&[g("no-error-execstack")], Arg::None, "", G, &[], Group::Diagnostics, Refused, Lands::Other, "Do not make executable stack warnings errors").at(FINAL),
    r(&[g("warn-rwx-segments")], Arg::None, "", G, &[], Group::Diagnostics, Refused, Lands::Other, "Warn about loadable segments that are writable and executable").at(FINAL),
    r(&[g("no-warn-rwx-segments")], Arg::None, "", G, &[], Group::Diagnostics, Refused, Lands::Other, "Do not warn about RWX segments").at(FINAL),
    r(&[g("error-rwx-segments")], Arg::None, "", G, &[], Group::Diagnostics, Refused, Lands::Other, "Make RWX segment warnings errors").at(FINAL),
    r(&[g("no-error-rwx-segments")], Arg::None, "", G, &[], Group::Diagnostics, Refused, Lands::Other, "Do not make RWX segment warnings errors").at(FINAL),
    r(&[x("warn-ifunc-textrel")], Arg::None, "", L, &[], Group::Diagnostics, Refused, Lands::Other, "Warn about ifunc symbols with text relocations").at(FINAL),
    r(&[x("no-warn-ifunc-textrel")], Arg::None, "", L, &[], Group::Diagnostics, Refused, Lands::Other, "Do not warn about ifunc text relocations (default)").at(FINAL),
    r(&[x("vs-diagnostics")], Arg::None, "", L, &[], Group::Diagnostics, Refused, Lands::Other, "Format diagnostics for Visual Studio").at(FINAL),
    r(&[x("why-live")], Arg::Required(Values::Any), "GLOB", L, &[], Group::Diagnostics, Refused, Lands::Other, "Report what keeps the symbols GLOB matches alive").at(FINAL),
    // The one-definition-rule switches (`spec/odr.md` O-41, amendment A-5).
    r(&[x("odr")], Arg::Required(Values::Choice(&["warn", "error", "strict", "off"])), "MODE", MORTAR, &[], Group::Diagnostics, Supported, Lands::Odr, "Check the one-definition rule: warn (default), error, strict or off").at(final_r("over the -r's units", "MT-30a")),
    r(&[x("no-odr-check")], Arg::None, "", MORTAR, &[], Group::Diagnostics, Supported, Lands::NoOdrCheck, "Run no one-definition-rule check (--odr=off)").at(final_r("", "MT-30a")),
    r(&[x("odr-stats")], Arg::None, "", MORTAR, &[], Group::Diagnostics, Supported, Lands::OdrStats, "Print one line of one-definition-rule statistics").at(final_r("", "MT-30a")),
    r(&[x("odr-check-archives")], Arg::None, "", MORTAR, &[], Group::Diagnostics, Supported, Lands::OdrCheckArchives, "Also compare the archive members that are not linked (odr.md O-23)").at(final_r("", "MT-30a")),
    r(&[x("odr-table")], Arg::Required(Values::Choice(&["off", "exported", "full"])), "ROWS", MORTAR, &[], Group::Diagnostics, Supported, Lands::OdrTable, "Write a loadable one-definition-rule table: off (default), exported or full").at(noop_r("by what the option is; the final link writes the table", "C-R2b")),
    r(&[x("keep-mason-meta")], Arg::None, "", MORTAR, &[], Group::Diagnostics, Supported, Lands::KeepMasonMeta, "Keep the inputs' Mason metadata in the output (meta.md MT-32)").at(noop_r("by what the option is; MT-30 always writes the container", "C-R2b, MT-30")),
    r(&[x("detect-odr-violations")], Arg::None, "", MORTAR, &[], Group::Diagnostics, Noop(DETECT_ODR), Lands::Other, "GNU gold's: check the one-definition rule").at(noop_r("gold's spelling, a no-op in a final link", "C-R2b")),
    r(&[g("version")], Arg::None, "", GLM, &[], Group::Bookkeeping, Supported, Lands::Version, "Print the version and exit").at(final_r("", "")),
    r(&[s("v")], Arg::None, "", GLM, &[], Group::Bookkeeping, Supported, Lands::PrintVersion(VersionStyle::Version), "Print the version and go on").at(final_r("", "")),
    r(&[s("V")], Arg::None, "", GLM, &[], Group::Bookkeeping, Supported, Lands::PrintVersion(VersionStyle::Emulations), "Print the version and the emulations and go on").at(final_r("", "")),
    r(&[g("help")], Arg::None, "", GLM, &[], Group::Bookkeeping, Supported, Lands::Help, "Print this help and exit").at(final_r("", "")),
    r(&[g1("Map")], Arg::Required(Values::Any), "FILE", GLM, &[], Group::Bookkeeping, Supported, Lands::Map, "Write a link map to FILE").at(noop_r("written as in any link", "C-R2, I-R12")),
    r(&[s("M"), g("print-map")], Arg::None, "", GLM, &[], Group::Bookkeeping, Supported, Lands::PrintMap, "Write a link map to standard output").at(final_r("", "rl0-opts")),
    r(&[g("print-map-discarded")], Arg::None, "", G, &[], Group::Bookkeeping, Refused, Lands::Other, "Show discarded sections in the map (default)").at(FINAL),
    r(&[g("no-print-map-discarded")], Arg::None, "", G, &[], Group::Bookkeeping, Refused, Lands::Other, "Do not show discarded sections in the map").at(FINAL),
    r(&[g("print-map-locals")], Arg::None, "", G, &[], Group::Bookkeeping, Refused, Lands::Other, "Show local symbols in the map").at(FINAL),
    r(&[g("no-print-map-locals")], Arg::None, "", G, &[], Group::Bookkeeping, Refused, Lands::Other, "Do not show local symbols in the map (default)").at(FINAL),
    r(&[g("cref")], Arg::None, "", GL, &[], Group::Bookkeeping, Refused, Lands::Cref, "Write a cross-reference table").at(FINAL),
    r(&[s("t"), g("trace")], Arg::None, "", GLM, &[], Group::Bookkeeping, Refused, Lands::Trace, "Print the name of each input file").at(FINAL),
    r(&[g("verbose")], Arg::Optional(Values::Count), "LEVEL", GLM, &[], Group::Bookkeeping, Supported, Lands::Verbose, "Print the version and the emulations, as -V does").at(final_r("", "rl0-opts")),
    r(&[g("dependency-file")], Arg::Required(Values::Any), "FILE", GLM, &[], Group::Bookkeeping, Supported, Lands::DependencyFile, "Write make dependencies of the link to FILE").at(final_r("", "rl0-opts")),
    r(&[x("reproduce")], Arg::Required(Values::Any), "FILE", L, &[], Group::Bookkeeping, Refused, Lands::Reproduce, "Write a tar archive that reproduces the link").at(FINAL),
    r(&[x("repro")], Arg::None, "", M, &[], Group::Bookkeeping, Refused, Lands::Other, "Embed the inputs in a .repro section").at(FINAL),
    r(&[x("threads")], Arg::Optional(Values::Count), "N", LM, &[], Group::Bookkeeping, Supported, Lands::Threads, "Use N threads (alone: all processors)").at(noop_r("", "C-R2")),
    r(&[x("thread-count")], Arg::Required(Values::Count), "N", M, &[], Group::Bookkeeping, Refused, Lands::ThreadCount, "Use N threads").at(FINAL),
    r(&[x("no-threads")], Arg::None, "", M, &[], Group::Bookkeeping, Refused, Lands::NoThreads, "Use one thread").at(FINAL),
    r(&[x("mortar-map")], Arg::Required(Values::Any), "FILE", MORTAR, &[], Group::Bookkeeping, Supported, Lands::MortarMap, "Write the machine-readable map (mortar-map 2) to FILE").at(noop_r("written (§14.2 of plumb.md)", "C-R2, I-R12")),
    r(&[x("time-phases")], Arg::None, "", MORTAR, &[], Group::Bookkeeping, Supported, Lands::TimePhases, "Report the time and memory of each phase").at(final_r("", "")),
    r(&[x("verify")], Arg::None, "", MORTAR, &[], Group::Bookkeeping, Refused, Lands::Verify, "Check the output with mortar_plumb (reserved)").at(FINAL),
    r(&[x("incremental")], Arg::Optional(Values::Choice(&["full", "update"])), "MODE", MORTAR, &[], Group::Bookkeeping, Supported, Lands::Incremental, "Link incrementally: update the output from its state if possible (alone), always from scratch (full), or update or fail (update)").at(acts("IN-12's warning, a plain -r", "C-R4, IN-12")),
    r(&[x("no-incremental")], Arg::None, "", MORTAR, &[], Group::Bookkeeping, Supported, Lands::NoIncremental, "Link plainly, never reading or writing a state (default)").at(noop_r("", "C-R2b")),
    r(&[x("incremental-state")], Arg::Required(Values::Any), "PATH", MORTAR, &[], Group::Bookkeeping, Supported, Lands::IncrementalState, "Keep the incremental link state at PATH (default: OUTPUT.mortar-state)").at(acts("IN-12's warning, no effect", "C-R2b, IN-12")),
    r(&[x("incremental-report")], Arg::Optional(Values::Any), "FILE", MORTAR, &[], Group::Bookkeeping, Supported, Lands::IncrementalReport, "Report how each incremental link went, to FILE (default: standard error)").at(acts("IN-12's warning, no effect", "C-R2b, IN-12")),
    r(&[x("incremental-write")], Arg::Required(Values::Choice(&["copy", "in-place"])), "MODE", MORTAR, &[], Group::Bookkeeping, Refused, Lands::Other, "How an update writes the output: copy (default) or in-place (reserved)").at(acts("IN-12's warning, no effect", "C-R2b, IN-12")),
    r(&[x("incremental-sync")], Arg::Required(Values::Choice(&["full", "next-link"])), "MODE", MORTAR, &[], Group::Bookkeeping, Refused, Lands::Other, "Sync an update's writes: full (default) or next-link (reserved)").at(acts("IN-12's warning, no effect", "C-R2b, IN-12")),
    r(&[x("incremental-verify")], Arg::None, "", MORTAR, &[], Group::Bookkeeping, Refused, Lands::Other, "Hash every input of an update regardless of its stat (reserved)").at(acts("IN-12's warning, no effect", "C-R2b, IN-12")),
    r(&[x("incremental-changed")], Arg::Required(Values::Any), "PATH", MORTAR, &[], Group::Bookkeeping, Refused, Lands::Other, "Treat input PATH as changed (reserved)").at(acts("IN-12's warning, no effect", "C-R2b, IN-12")),
    r(&[g("stats")], Arg::None, "", GLM, &[], Group::Bookkeeping, Refused, Lands::Other, "Print statistics").at(FINAL),
    r(&[x("perf")], Arg::None, "", M, &[], Group::Bookkeeping, Refused, Lands::Other, "Print performance statistics").at(FINAL),
    r(&[x("time-trace")], Arg::Optional(Values::Any), "FILE", L, &[], Group::Bookkeeping, Refused, Lands::Other, "Write a time trace").at(FINAL),
    r(&[x("time-trace-granularity")], Arg::Required(Values::Count), "MICROSECONDS", L, &[], Group::Bookkeeping, Refused, Lands::Other, "Granularity of the time trace").at(FINAL),
    r(&[g("print-memory-usage")], Arg::None, "", GL, &[], Group::Bookkeeping, Refused, Lands::Other, "Report the use of memory regions").at(FINAL),
    r(&[g("print-output-format")], Arg::None, "", G, &[], Group::Bookkeeping, Refused, Lands::Other, "Print the default output format").at(FINAL),
    r(&[g("print-sysroot")], Arg::None, "", G, &[], Group::Bookkeeping, Refused, Lands::Other, "Print the system root").at(FINAL),
    r(&[g("target-help")], Arg::None, "", G, &[], Group::Bookkeeping, Refused, Lands::Other, "Print the options of the targets").at(FINAL),
    r(&[x("fork")], Arg::None, "", M, &[], Group::Bookkeeping, Refused, Lands::Other, "Link in a child process").at(FINAL),
    r(&[x("no-fork")], Arg::None, "", M, &[], Group::Bookkeeping, Refused, Lands::Other, "Link in this process").at(FINAL),
    r(&[x("quick-exit")], Arg::None, "", M, &[], Group::Bookkeeping, Refused, Lands::Other, "Exit without cleanup").at(FINAL),
    r(&[x("no-quick-exit")], Arg::None, "", M, &[], Group::Bookkeeping, Refused, Lands::Other, "Exit with cleanup").at(FINAL),
    r(&[x("detach")], Arg::None, "", M, &[], Group::Bookkeeping, Refused, Lands::Other, "Write the separate debug file in the background").at(FINAL),
    r(&[x("no-detach")], Arg::None, "", M, &[], Group::Bookkeeping, Refused, Lands::Other, "Write the separate debug file before exiting").at(FINAL),
    r(&[x("mmap-output-file")], Arg::None, "", L, &[], Group::Bookkeeping, Refused, Lands::Other, "Map the output file into memory (default)").at(FINAL),
    r(&[x("no-mmap-output-file")], Arg::None, "", L, &[], Group::Bookkeeping, Refused, Lands::Other, "Write the output file without mapping it").at(FINAL),
    r(&[x("rsp-quoting")], Arg::Required(Values::Choice(&["posix", "windows"])), "STYLE", L, &[], Group::Bookkeeping, Refused, Lands::Other, "Quoting style of response files").at(FINAL),
    r(&[x("run")], Arg::Rest, "COMMAND", M, &[], Group::Bookkeeping, Refused, Lands::Other, "Run COMMAND with this linker as ld").at(FINAL),
    r(&[g1("plugin")], Arg::Required(Values::Any), "PLUGIN", GLM, &[], Group::Lto, Noop("mortar loads no linker plugin; an LTO input is refused by name when it is read"), Lands::Plugin, "Load a linker plugin").at(noop_r("an LTO object still refused", "C-R2, RV-R9")),
    r(&[g1("plugin-opt")], Arg::Required(Values::Any), "OPTION", GLM, &[], Group::Lto, Noop("only the plugin reads it, and mortar loads none"), Lands::PluginOpt, "Pass OPTION to the plugin").at(noop_r("", "C-R2")),
    r(&[g1("plugin-save-temps")], Arg::None, "", G, &[], Group::Lto, Noop("only the plugin reads it, and mortar loads none"), Lands::Other, "Keep the plugin's temporary files").at(noop_r("measured", "C-R2b, rl0-opts")),
    r(&[g1("flto")], Arg::None, "", G, &[], Group::Lto, Noop("GNU ld ignores it too; GCC passes it on some LTO links"), Lands::Other, "Ignored, for GCC").at(noop_r("measured", "C-R2b, rl0-opts")),
    r(&[g1("flto-partition")], Arg::Required(Values::Any), "MODE", G, &[], Group::Lto, Noop("GNU ld ignores it too; GCC passes it on some LTO links"), Lands::Other, "Ignored, for GCC").at(noop_r("measured", "C-R2b, rl0-opts")),
    r(&[g1("fuse-ld")], Arg::Required(Values::Any), "LINKER", G, &[], Group::Lto, Noop("GNU ld ignores it too; it chooses the linker, which is chosen"), Lands::Other, "Ignored, for GCC").at(noop_r("measured", "C-R2b, rl0-opts")),
    r(&[x("lto")], Arg::Required(Values::Choice(&["full", "thin"])), "BACKEND", L, &[], Group::Lto, Refused, Lands::Other, "LTO back end").at(noop_r("by what the option is", "C-R2b")),
    r(&[x("lto-O")], Arg::Glued(Values::Count), "LEVEL", LM, &[], Group::Lto, Refused, Lands::Other, "LTO optimization level").at(noop_r("by what the option is", "C-R2b")),
    r(&[x("lto-CGO")], Arg::Glued(Values::Count), "LEVEL", L, &[], Group::Lto, Refused, Lands::Other, "LTO code generation level").at(noop_r("by what the option is", "C-R2b")),
    r(&[x("lto-partitions")], Arg::Required(Values::Count), "N", LM, &[], Group::Lto, Refused, Lands::Other, "Number of LTO partitions").at(noop_r("by what the option is", "C-R2b")),
    r(&[x("lto-aa-pipeline")], Arg::Required(Values::Any), "PIPELINE", L, &[], Group::Lto, Refused, Lands::Other, "LTO alias analysis pipeline").at(noop_r("by what the option is", "C-R2b")),
    r(&[x("lto-newpm-passes")], Arg::Required(Values::Any), "PASSES", L, &[], Group::Lto, Refused, Lands::Other, "LTO passes").at(noop_r("by what the option is", "C-R2b")),
    r(&[x("lto-debug-pass-manager")], Arg::None, "", LM, &[], Group::Lto, Refused, Lands::Other, "Debug the LTO pass manager").at(noop_r("by what the option is", "C-R2b")),
    r(&[x("lto-emit-asm")], Arg::None, "", LM, &[], Group::Lto, Refused, Lands::Other, "Write assembly instead of linking").at(noop_r("by what the option is", "C-R2b")),
    r(&[x("lto-emit-llvm")], Arg::None, "", L, &[], Group::Lto, Refused, Lands::Other, "Write bitcode instead of linking").at(noop_r("by what the option is", "C-R2b")),
    r(&[x("lto-sample-profile")], Arg::Required(Values::Any), "FILE", LM, &[], Group::Lto, Refused, Lands::Other, "LTO sample profile").at(noop_r("by what the option is", "C-R2b")),
    r(&[x("lto-cs-profile-file")], Arg::Required(Values::Any), "FILE", LM, &[], Group::Lto, Refused, Lands::Other, "LTO context-sensitive profile").at(noop_r("by what the option is", "C-R2b")),
    r(&[x("lto-cs-profile-generate")], Arg::None, "", LM, &[], Group::Lto, Refused, Lands::Other, "LTO context-sensitive instrumentation").at(noop_r("by what the option is", "C-R2b")),
    r(&[x("lto-whole-program-visibility")], Arg::None, "", L, &[], Group::Lto, Refused, Lands::Other, "LTO: the link sees the whole program").at(noop_r("by what the option is", "C-R2b")),
    r(&[x("no-lto-whole-program-visibility")], Arg::None, "", L, &[], Group::Lto, Refused, Lands::Other, "LTO: the link does not see the whole program").at(FINAL),
    r(&[x("lto-pgo-warn-mismatch")], Arg::None, "", L, &[], Group::Lto, Refused, Lands::Other, "Warn about profile mismatches (default)").at(noop_r("by what the option is", "C-R2b")),
    r(&[x("no-lto-pgo-warn-mismatch")], Arg::None, "", L, &[], Group::Lto, Refused, Lands::Other, "Do not warn about profile mismatches").at(FINAL),
    r(&[x("lto-basic-block-sections")], Arg::Required(Values::Any), "VALUE", L, &[], Group::Lto, Refused, Lands::Other, "LTO basic block sections").at(noop_r("by what the option is", "C-R2b")),
    r(&[x("lto-basic-block-address-map")], Arg::None, "", L, &[], Group::Lto, Refused, Lands::Other, "LTO basic block address map").at(noop_r("by what the option is", "C-R2b")),
    r(&[x("no-lto-basic-block-address-map")], Arg::None, "", L, &[], Group::Lto, Refused, Lands::Other, "No LTO basic block address map (default)").at(FINAL),
    r(&[x("lto-unique-basic-block-section-names")], Arg::None, "", L, &[], Group::Lto, Refused, Lands::Other, "Unique basic block section names").at(noop_r("by what the option is", "C-R2b")),
    r(&[x("no-lto-unique-basic-block-section-names")], Arg::None, "", L, &[], Group::Lto, Refused, Lands::Other, "No unique basic block section names (default)").at(FINAL),
    r(&[x("lto-validate-all-vtables-have-type-infos")], Arg::None, "", L, &[], Group::Lto, Refused, Lands::Other, "Check that vtables have type information").at(noop_r("by what the option is", "C-R2b")),
    r(&[x("no-lto-validate-all-vtables-have-type-infos")], Arg::None, "", L, &[], Group::Lto, Refused, Lands::Other, "Do not check vtables (default)").at(FINAL),
    r(&[x("lto-known-safe-vtables")], Arg::Required(Values::Any), "GLOB", L, &[], Group::Lto, Refused, Lands::Other, "Vtables the check skips").at(noop_r("by what the option is", "C-R2b")),
    r(&[x("load-pass-plugin")], Arg::Required(Values::Any), "FILE", L, &[], Group::Lto, Refused, Lands::Other, "Load LLVM passes from FILE").at(FINAL),
    r(&[x("mllvm")], Arg::Required(Values::Any), "OPTION", L, &[], Group::Lto, Refused, Lands::Other, "Pass OPTION to LLVM").at(FINAL),
    r(&[x("save-temps")], Arg::Optional(Values::Any), "WHAT", LM, &[], Group::Lto, Refused, Lands::Other, "Keep LTO intermediate files").at(FINAL),
    r(&[x("opt-remarks-filename")], Arg::Required(Values::Any), "FILE", LM, &[], Group::Lto, Refused, Lands::Other, "Optimization remarks file").at(FINAL),
    r(&[x("opt-remarks-format")], Arg::Required(Values::Any), "FORMAT", LM, &[], Group::Lto, Refused, Lands::Other, "Optimization remarks format").at(FINAL),
    r(&[x("opt-remarks-passes")], Arg::Required(Values::Any), "REGEX", LM, &[], Group::Lto, Refused, Lands::Other, "Passes whose remarks are kept").at(FINAL),
    r(&[x("opt-remarks-hotness-threshold")], Arg::Required(Values::Any), "N", LM, &[], Group::Lto, Refused, Lands::Other, "Remark hotness threshold").at(FINAL),
    r(&[x("opt-remarks-with-hotness")], Arg::None, "", L, &[], Group::Lto, Refused, Lands::Other, "Remarks with hotness").at(FINAL),
    r(&[x("thinlto-cache-dir")], Arg::Required(Values::Any), "DIR", LM, &[], Group::Lto, Refused, Lands::Other, "ThinLTO cache directory").at(FINAL),
    r(&[x("thinlto-cache-policy")], Arg::Required(Values::Any), "POLICY", LM, &[], Group::Lto, Refused, Lands::Other, "ThinLTO cache policy").at(FINAL),
    r(&[x("thinlto-jobs")], Arg::Required(Values::Any), "N", LM, &[], Group::Lto, Refused, Lands::Other, "ThinLTO jobs").at(FINAL),
    r(&[x("thinlto-single-module")], Arg::Required(Values::Any), "MODULE", L, &[], Group::Lto, Refused, Lands::Other, "ThinLTO single module").at(FINAL),
    r(&[x("thinlto-distributor")], Arg::Required(Values::Any), "FILE", L, &[], Group::Lto, Refused, Lands::Other, "ThinLTO distributor").at(FINAL),
    r(&[x("thinlto-distributor-arg")], Arg::Required(Values::Any), "ARG", L, &[], Group::Lto, Refused, Lands::Other, "ThinLTO distributor argument").at(FINAL),
    r(&[x("thinlto-remote-compiler")], Arg::Required(Values::Any), "FILE", L, &[], Group::Lto, Refused, Lands::Other, "ThinLTO remote compiler").at(FINAL),
    r(&[x("thinlto-remote-compiler-arg")], Arg::Required(Values::Any), "ARG", L, &[], Group::Lto, Refused, Lands::Other, "ThinLTO remote compiler argument").at(FINAL),
    r(&[x("thinlto-remote-compiler-prepend-arg")], Arg::Required(Values::Any), "ARG", L, &[], Group::Lto, Refused, Lands::Other, "ThinLTO remote compiler prepended argument").at(FINAL),
    r(&[g("fix-cortex-a53-835769")], Arg::None, "", GM, A64, Group::TargetSpecific, Refused, Lands::Other, "Work around Cortex-A53 erratum 835769").at(FINAL),
    r(&[g("fix-cortex-a53-843419")], Arg::Optional(Values::Choice(&["full", "adr", "adrp"])), "WORKAROUND", GLM, A64, Group::TargetSpecific, Partial { refused: &["adr", "adrp"], note: "a64.md §11.2: GNU ld's =adr and =adrp restrict the fix, which mortar does not do; use --fix-cortex-a53-843419 or =full" }, Lands::Other, "Work around Cortex-A53 erratum 843419").at(noop_r("", "C-R2")),
    r(&[g("no-apply-dynamic-relocs")], Arg::None, "", GLM, A64, Group::TargetSpecific, Refused, Lands::Other, "Write zero where dynamic relocations apply").at(FINAL),
    r(&[g("pic-veneer")], Arg::None, "", GL, A64, Group::TargetSpecific, Refused, Lands::Other, "Always use position-independent veneers").at(FINAL),
    r(&[g("stub-group-size")], Arg::Required(Values::Any), "N", G, A64, Group::TargetSpecific, Refused, Lands::Other, "Size of a group of sections one stub section serves").at(FINAL),
    r(&[g("no-enum-size-warning")], Arg::None, "", G, A64, Group::TargetSpecific, Refused, Lands::Other, "Do not warn about objects with other enum sizes").at(FINAL),
    r(&[g("no-wchar-size-warning")], Arg::None, "", G, A64, Group::TargetSpecific, Refused, Lands::Other, "Do not warn about objects with another wchar_t size").at(FINAL),
    r(&[g("no-pipeline-knowledge")], Arg::None, "", G, A64, Group::TargetSpecific, Refused, Lands::Other, "(hidden in GNU ld)").at(FINAL),
    r(&[g("ld-generated-unwind-info")], Arg::None, "", G, X86, Group::TargetSpecific, Refused, Lands::Other, "Write unwind information for the PLT").at(FINAL),
    r(&[g("no-ld-generated-unwind-info")], Arg::None, "", G, X86, Group::TargetSpecific, Refused, Lands::Other, "Write no unwind information for the PLT").at(FINAL),
    r(&[z("force-bti")], Arg::None, "", GL, A64, Group::TargetSpecific, Refused, Lands::Other, "Require BTI and write BTI PLTs").at(refused_r("`-z force-bti with -r`", "C-R2a")),
    r(&[z("bti-report")], Arg::Optional(Values::Choice(&["none", "warning", "error"])), "LEVEL", GL, A64, Group::TargetSpecific, Refused, Lands::Other, "Report inputs without BTI markings").at(noop_r("C-R2a's first row; the class at -r wins", "C-R2a, C-R2b")),
    r(&[z("pac-plt")], Arg::None, "", GL, A64, Group::TargetSpecific, Refused, Lands::Other, "Protect PLTs with pointer authentication").at(noop_r("C-R2a's first row; the class at -r wins", "C-R2a, C-R2b")),
    r(&[z("gcs")], Arg::Required(Values::Choice(&["always", "never", "implicit"])), "WHEN", GL, A64, Group::TargetSpecific, Refused, Lands::Other, "Mark the output for the Guarded Control Stack").at(refused_r("`-z gcs=always with -r`; `=never` is a no-op (C-R2a's first row)", "C-R2a")),
    r(&[z("gcs-report")], Arg::Optional(Values::Choice(&["none", "warning", "error"])), "LEVEL", GL, A64, Group::TargetSpecific, Refused, Lands::Other, "Report inputs without GCS markings").at(noop_r("C-R2a's first row; the class at -r wins", "C-R2a, C-R2b")),
    r(&[z("gcs-report-dynamic")], Arg::Required(Values::Choice(&["none", "warning", "error"])), "LEVEL", GL, A64, Group::TargetSpecific, Refused, Lands::Other, "Report shared objects without GCS markings").at(noop_r("C-R2a's first row; the class at -r wins", "C-R2a, C-R2b")),
    r(&[z("noextern-protected-data")], Arg::None, "", G, X86, Group::TargetSpecific, Refused, Lands::Other, "Do not treat protected data as external").at(noop_r("C-R2a's first row; the class at -r wins", "C-R2a, C-R2b")),
    r(&[z("indirect-extern-access")], Arg::None, "", G, X86, Group::TargetSpecific, Refused, Lands::Other, "Access external data indirectly").at(refused_r("`-z indirect-extern-access with -r`", "C-R2a")),
    r(&[z("noindirect-extern-access")], Arg::None, "", G, X86, Group::TargetSpecific, Refused, Lands::Other, "Access external data directly (default)").at(noop_r("C-R2a's first row; the class at -r wins", "C-R2a, C-R2b")),
    r(&[z("dynamic-undefined-weak")], Arg::None, "", GLM, X86, Group::TargetSpecific, Refused, Lands::Other, "Make undefined weak symbols dynamic").at(noop_r("C-R2a's first row; the class at -r wins", "C-R2a, C-R2b")),
    r(&[z("nodynamic-undefined-weak")], Arg::None, "", GLM, X86, Group::TargetSpecific, Refused, Lands::Other, "Do not make undefined weak symbols dynamic").at(noop_r("C-R2a's first row; the class at -r wins", "C-R2a, C-R2b")),
    r(&[z("call-nop")], Arg::Required(Values::Any), "PADDING", G, X86, Group::TargetSpecific, Refused, Lands::Other, "The 1-byte NOP used to pad converted branches").at(noop_r("C-R2a's first row; the class at -r wins", "C-R2a, C-R2b")),
    r(&[z("ibtplt")], Arg::None, "", GM, X86, Group::TargetSpecific, Refused, Lands::Other, "Write IBT PLT entries").at(noop_r("C-R2a's first row; the class at -r wins", "C-R2a, C-R2b")),
    r(&[z("ibt")], Arg::None, "", GM, X86, Group::TargetSpecific, Refused, Lands::Other, "Mark the output for IBT").at(refused_r("`-z ibt with -r`", "C-R2a")),
    r(&[z("shstk")], Arg::None, "", GLM, X86, Group::TargetSpecific, Refused, Lands::Other, "Mark the output for the shadow stack").at(refused_r("`-z shstk with -r`", "C-R2a")),
    r(&[z("cet-report")], Arg::Required(Values::Choice(&["none", "warning", "error"])), "LEVEL", GLM, X86, Group::TargetSpecific, Refused, Lands::Other, "Report inputs without IBT and SHSTK markings").at(final_r("the same check as in a final link; `=none` and `=warning` are no-ops", "C-R2a")),
    r(&[z("report-relative-reloc")], Arg::None, "", G, X86, Group::TargetSpecific, Refused, Lands::Other, "Report relative relocations").at(noop_r("C-R2a's first row; the class at -r wins", "C-R2a, C-R2b")),
    r(&[z("x86-64-baseline")], Arg::None, "", G, X86, Group::TargetSpecific, Refused, Lands::Other, "Mark the x86-64 baseline ISA level as needed").at(FINAL),
    r(&[z("x86-64-v2")], Arg::None, "", GM, X86, Group::TargetSpecific, Refused, Lands::Other, "Mark the x86-64-v2 ISA level as needed").at(refused_r("`-z x86-64-v2 with -r`", "C-R2a")),
    r(&[z("x86-64-v3")], Arg::None, "", GM, X86, Group::TargetSpecific, Refused, Lands::Other, "Mark the x86-64-v3 ISA level as needed").at(refused_r("`-z x86-64-v3 with -r`", "C-R2a")),
    r(&[z("x86-64-v4")], Arg::None, "", GM, X86, Group::TargetSpecific, Refused, Lands::Other, "Mark the x86-64-v4 ISA level as needed").at(refused_r("`-z x86-64-v4 with -r`", "C-R2a")),
    r(&[z("isa-level-report")], Arg::Required(Values::Choice(&["none", "all", "needed", "used"])), "WHICH", G, X86, Group::TargetSpecific, Refused, Lands::Other, "Report the x86-64 ISA levels").at(noop_r("C-R2a's first row; the class at -r wins", "C-R2a, C-R2b")),
    r(&[z("noreloc-overflow")], Arg::None, "", G, X64, Group::TargetSpecific, Refused, Lands::Other, "Do not check relocation overflow").at(noop_r("C-R2a's first row; the class at -r wins", "C-R2a, C-R2b")),
    r(&[z("lam-u48")], Arg::None, "", G, X64, Group::TargetSpecific, Refused, Lands::Other, "Mark the output for LAM_U48").at(refused_r("`-z lam-u48 with -r`", "C-R2a")),
    r(&[z("lam-u48-report")], Arg::Required(Values::Choice(&["none", "warning", "error"])), "LEVEL", G, X64, Group::TargetSpecific, Refused, Lands::Other, "Report inputs without LAM_U48 markings").at(final_r("the same check as in a final link; `=none` and `=warning` are no-ops", "C-R2a")),
    r(&[z("lam-u57")], Arg::None, "", G, X64, Group::TargetSpecific, Refused, Lands::Other, "Mark the output for LAM_U57").at(refused_r("`-z lam-u57 with -r`", "C-R2a")),
    r(&[z("lam-u57-report")], Arg::Required(Values::Choice(&["none", "warning", "error"])), "LEVEL", G, X64, Group::TargetSpecific, Refused, Lands::Other, "Report inputs without LAM_U57 markings").at(final_r("the same check as in a final link; `=none` and `=warning` are no-ops", "C-R2a")),
    r(&[z("lam-report")], Arg::Required(Values::Choice(&["none", "warning", "error"])), "LEVEL", G, X64, Group::TargetSpecific, Refused, Lands::Other, "Report inputs without LAM markings").at(final_r("the same check as in a final link; `=none` and `=warning` are no-ops", "C-R2a")),
    r(&[z("mark-plt")], Arg::None, "", G, X64, Group::TargetSpecific, Refused, Lands::Other, "Mark the PLT with dynamic tags").at(noop_r("C-R2a's first row; the class at -r wins", "C-R2a, C-R2b")),
    r(&[z("nomark-plt")], Arg::None, "", G, X64, Group::TargetSpecific, Refused, Lands::Other, "Do not mark the PLT (default)").at(noop_r("C-R2a's first row; the class at -r wins", "C-R2a, C-R2b")),
    r(&[z("now")], Arg::None, "", GLM, &[], Group::Z, Supported, Lands::Z(ZField::Now(true)), "Resolve every symbol at load time").at(noop_r("C-R2a's first row", "C-R2a")),
    r(&[z("lazy")], Arg::None, "", GLM, &[], Group::Z, Supported, Lands::Z(ZField::Now(false)), "Resolve functions when first called (default)").at(noop_r("C-R2a's first row", "C-R2a")),
    r(&[z("relro")], Arg::None, "", GLM, &[], Group::Z, Supported, Lands::Z(ZField::Relro(true)), "Write PT_GNU_RELRO (default)").at(noop_r("C-R2a's first row", "C-R2a")),
    r(&[z("norelro")], Arg::None, "", GLM, &[], Group::Z, Supported, Lands::Z(ZField::Relro(false)), "Write no PT_GNU_RELRO").at(noop_r("C-R2a's first row", "C-R2a")),
    r(&[z("execstack")], Arg::None, "", GLM, &[], Group::Z, Supported, Lands::Z(ZField::Execstack(true)), "Make the stack executable").at(acts("L-R6a", "C-R3, L-R6a")),
    r(&[z("noexecstack")], Arg::None, "", GLM, &[], Group::Z, Supported, Lands::Z(ZField::Execstack(false)), "Make the stack not executable").at(acts("L-R6a", "C-R3, L-R6a")),
    r(&[z("text")], Arg::None, "", GLM, &[], Group::Z, Supported, Lands::Z(ZField::Text(true)), "Report text relocations as errors").at(noop_r("C-R2a's first row", "C-R2a")),
    r(&[z("notext"), z("textoff")], Arg::None, "", GLM, &[], Group::Z, Refused, Lands::Z(ZField::Text(false)), "Accept text relocations").at(noop_r("C-R2a's first row; the class at -r wins", "C-R2a, C-R2b")),
    r(&[z("separate-code")], Arg::None, "", GLM, &[], Group::Z, Supported, Lands::Z(ZField::SeparateCode(true)), "Put code in segments of its own pages").at(noop_r("C-R2a's first row", "C-R2a")),
    r(&[z("noseparate-code")], Arg::None, "", GLM, &[], Group::Z, Supported, Lands::Z(ZField::SeparateCode(false)), "Let code share pages with other data").at(noop_r("C-R2a's first row", "C-R2a")),
    r(&[z("defs")], Arg::None, "", GLM, &[], Group::Z, Supported, Lands::Z(ZField::Defs(true)), "Report undefined symbols of object files").at(noop_r("C-R2a's first row", "C-R2a")),
    r(&[z("undefs")], Arg::None, "", GLM, &[], Group::Z, Refused, Lands::Z(ZField::Defs(false)), "Accept undefined symbols in a shared object").at(noop_r("C-R2a's first row; the class at -r wins", "C-R2a, C-R2b")),
    r(&[z("max-page-size")], Arg::Required(Values::Any), "SIZE", GLM, &[], Group::Z, Supported, Lands::Z(ZField::MaxPageSize), "The largest page size the output supports").at(noop_r("C-R2a's first row", "C-R2a")),
    r(&[z("common-page-size")], Arg::Required(Values::Any), "SIZE", GLM, &[], Group::Z, Noop("layout.md L-75: the value is checked, and no layout rule reads it"), Lands::Z(ZField::CommonPageSize), "The common page size").at(noop_r("C-R2a's first row", "C-R2a")),
    r(&[z("stack-size")], Arg::Required(Values::Any), "SIZE", GLM, &[], Group::Z, Refused, Lands::Z(ZField::StackSize), "The size of the stack segment").at(refused_r("`-z stack-size=SIZE with -r`, the value as written", "C-R2a")),
    r(&[z("pack-relative-relocs")], Arg::None, "", GLM, &[], Group::Z, Refused, Lands::Z(ZField::PackRelativeRelocs(true)), "Pack relative relocations in DT_RELR").at(noop_r("C-R2a's first row; the class at -r wins", "C-R2a, C-R2b")),
    r(&[z("nopack-relative-relocs")], Arg::None, "", GLM, &[], Group::Z, Refused, Lands::Z(ZField::PackRelativeRelocs(false)), "Do not pack relative relocations (default)").at(noop_r("C-R2a's first row; the class at -r wins", "C-R2a, C-R2b")),
    r(&[zx("copyreloc")], Arg::None, "", L, &[], Group::Z, Refused, Lands::Z(ZField::Copyreloc(true)), "Make copy relocations (default)").at(FINAL),
    r(&[z("nocopyreloc")], Arg::None, "", GLM, &[], Group::Z, Refused, Lands::Z(ZField::Copyreloc(false)), "Make no copy relocations").at(noop_r("C-R2a's first row; the class at -r wins", "C-R2a, C-R2b")),
    r(&[z("combreloc")], Arg::None, "", GLM, &[], Group::Z, Refused, Lands::Z(ZField::Combreloc(true)), "Sort dynamic relocations into one section (default)").at(noop_r("C-R2a's first row; the class at -r wins", "C-R2a, C-R2b")),
    r(&[z("nocombreloc")], Arg::None, "", GLM, &[], Group::Z, Refused, Lands::Z(ZField::Combreloc(false)), "Do not sort dynamic relocations").at(noop_r("C-R2a's first row; the class at -r wins", "C-R2a, C-R2b")),
    r(&[z("sectionheader")], Arg::None, "", GLM, &[], Group::Z, Noop("layout.md L-63: the section header table is always written (the default)"), Lands::Z(ZField::Sectionheader(true)), "Write the section header table (default)").at(noop_r("C-R2a's first row", "C-R2a")),
    r(&[z("nosectionheader")], Arg::None, "", GLM, &[], Group::Z, Refused, Lands::Z(ZField::Sectionheader(false)), "Write no section header table").at(conflicting_r("C-R2a")),
    r(&[z("muldefs")], Arg::None, "", GLM, &[], Group::Z, Supported, Lands::AllowMultipleDefinition(true), "Take the first of several definitions of a symbol").at(final_r("", "RV-R4, C-R2a")),
    r(&[z("unique-symbol")], Arg::None, "", G, &[], Group::Z, Refused, Lands::Other, "Give local symbols unique names").at(refused_r("`-z unique-symbol with -r`", "C-R2a")),
    r(&[z("nounique-symbol")], Arg::None, "", G, &[], Group::Z, Refused, Lands::Other, "Keep duplicate local symbol names (default)").at(noop_r("C-R2a's first row; the class at -r wins", "C-R2a, C-R2b")),
    r(&[z("globalaudit")], Arg::None, "", G, &[], Group::Z, Refused, Lands::Other, "Mark the executable for global auditing").at(noop_r("C-R2a's first row; the class at -r wins", "C-R2a, C-R2b")),
    r(&[z("start-stop-gc")], Arg::None, "", GL, &[], Group::Z, Refused, Lands::Other, "Let __start_/__stop_ references not keep sections").at(noop_r("C-R2a's first row; the class at -r wins", "C-R2a, C-R2b")),
    r(&[z("nostart-stop-gc")], Arg::None, "", GL, &[], Group::Z, Noop("layout.md L-96: the default"), Lands::Other, "Let __start_/__stop_ references keep sections (default)").at(noop_r("C-R2a's first row", "C-R2a")),
    r(&[z("start-stop-visibility")], Arg::Required(Values::Choice(&["default", "internal", "hidden", "protected"])), "VISIBILITY", GL, &[], Group::Z, Refused, Lands::Other, "Visibility of __start_ and __stop_ symbols").at(noop_r("C-R2a's first row; the class at -r wins", "C-R2a, C-R2b")),
    r(&[z("global")], Arg::None, "", GL, &[], Group::Z, Refused, Lands::Other, "Make the symbols of this object available to later loads").at(noop_r("C-R2a's first row; the class at -r wins", "C-R2a, C-R2b")),
    r(&[z("initfirst")], Arg::None, "", GLM, &[], Group::Z, Supported, Lands::Other, "Initialize this object first").at(noop_r("C-R2a's first row", "C-R2a")),
    r(&[z("interpose")], Arg::None, "", GLM, &[], Group::Z, Supported, Lands::Other, "Let this object interpose all but the executable").at(noop_r("C-R2a's first row", "C-R2a")),
    r(&[z("unique")], Arg::None, "", G, &[], Group::Z, Refused, Lands::Other, "Load this object at most once").at(noop_r("C-R2a's first row; the class at -r wins", "C-R2a, C-R2b")),
    r(&[z("nounique")], Arg::None, "", G, &[], Group::Z, Refused, Lands::Other, "Do not mark the object as unique").at(noop_r("C-R2a's first row; the class at -r wins", "C-R2a, C-R2b")),
    r(&[z("loadfltr")], Arg::None, "", G, &[], Group::Z, Refused, Lands::Other, "Process the filtees at load time").at(noop_r("C-R2a's first row; the class at -r wins", "C-R2a, C-R2b")),
    r(&[z("nodefaultlib")], Arg::None, "", GLM, &[], Group::Z, Refused, Lands::Other, "Do not search the default directories for dependencies").at(noop_r("C-R2a's first row; the class at -r wins", "C-R2a, C-R2b")),
    r(&[z("nodelete")], Arg::None, "", GLM, &[], Group::Z, Supported, Lands::Other, "Never unload this object").at(noop_r("C-R2a's first row", "C-R2a")),
    r(&[z("nodlopen")], Arg::None, "", GLM, &[], Group::Z, Supported, Lands::Other, "Do not allow dlopen of this object").at(noop_r("C-R2a's first row", "C-R2a")),
    r(&[z("nodump")], Arg::None, "", GM, &[], Group::Z, Refused, Lands::Other, "Do not allow dldump of this object").at(noop_r("C-R2a's first row; the class at -r wins", "C-R2a, C-R2b")),
    r(&[z("origin")], Arg::None, "", GLM, &[], Group::Z, Supported, Lands::Other, "The object uses $ORIGIN").at(noop_r("C-R2a's first row", "C-R2a")),
    r(&[z("common")], Arg::None, "", G, &[], Group::Z, Refused, Lands::Other, "Give common symbols the type STT_COMMON").at(noop_r("C-R2a's first row; the class at -r wins", "C-R2a, C-R2b")),
    r(&[z("nocommon")], Arg::None, "", G, &[], Group::Z, Refused, Lands::Other, "Give common symbols the type STT_OBJECT").at(noop_r("C-R2a's first row; the class at -r wins", "C-R2a, C-R2b")),
    r(&[z("memory-seal")], Arg::None, "", G, &[], Group::Z, Supported, Lands::Z(ZField::MemorySeal(true)), "Mark the object for memory sealing").at(refused_r("`-z memory-seal with -r`", "C-R2a")),
    r(&[z("nomemory-seal")], Arg::None, "", G, &[], Group::Z, Supported, Lands::Z(ZField::MemorySeal(false)), "Do not mark the object for memory sealing (default)").at(noop_r("C-R2a's first row", "C-R2a")),
    r(&[zx("dead-reloc-in-nonalloc")], Arg::Required(Values::Any), "GLOB=VALUE", L, &[], Group::Z, Refused, Lands::Other, "The value of relocations to discarded sections in non-alloc sections").at(FINAL),
    r(&[zx("execute-only-report")], Arg::Required(Values::Choice(&["none", "warning", "error"])), "LEVEL", L, &[], Group::Z, Refused, Lands::Other, "Report inputs that are not execute-only").at(noop_r("by what the option is", "C-R2b")),
    r(&[zx("force-ibt")], Arg::None, "", L, &[], Group::Z, Refused, Lands::Other, "Write IBT PLT entries and mark the output for IBT").at(FINAL),
    r(&[zx("hazardplt")], Arg::None, "", L, &[], Group::Z, Refused, Lands::Other, "MIPS hazard-barrier PLT entries").at(FINAL),
    r(&[zx("ifunc-noplt")], Arg::None, "", L, &[], Group::Z, Refused, Lands::Other, "Resolve ifuncs without PLT entries").at(FINAL),
    r(&[zx("keep-text-section-prefix")], Arg::None, "", LM, &[], Group::Z, Refused, Lands::Other, "Keep .text.hot and similar output sections apart").at(FINAL),
    r(&[zx("nokeep-text-section-prefix")], Arg::None, "", LM, &[], Group::Z, Refused, Lands::Other, "Merge .text.hot and similar into .text (default)").at(FINAL),
    r(&[zx("nognustack")], Arg::None, "", L, &[], Group::Z, Refused, Lands::Other, "Write no PT_GNU_STACK").at(FINAL),
    r(&[zx("rel")], Arg::None, "", L, &[], Group::Z, Refused, Lands::Other, "Write REL dynamic relocations").at(FINAL),
    r(&[zx("rela")], Arg::None, "", L, &[], Group::Z, Refused, Lands::Other, "Write RELA dynamic relocations").at(FINAL),
    r(&[zx("retpolineplt")], Arg::None, "", L, &[], Group::Z, Refused, Lands::Other, "Write retpoline PLT entries").at(FINAL),
    r(&[zx("rodynamic")], Arg::None, "", LM, &[], Group::Z, Refused, Lands::Other, "Make .dynamic read-only").at(FINAL),
    r(&[zx("separate-loadable-segments")], Arg::None, "", LM, &[], Group::Z, Refused, Lands::Other, "Put every loadable segment on pages of its own").at(FINAL),
    r(&[zx("wxneeded")], Arg::None, "", L, &[], Group::Z, Refused, Lands::Other, "Mark the object as needing writable and executable memory").at(FINAL),
    r(&[zx("lrodata-after-bss")], Arg::None, "", L, &[], Group::Z, Refused, Lands::Other, "Place .lrodata after .bss").at(FINAL),
    r(&[zx("nolrodata-after-bss")], Arg::None, "", L, &[], Group::Z, Refused, Lands::Other, "Place .lrodata before .bss").at(FINAL),
    r(&[zx("zicfilp")], Arg::Required(Values::Any), "MODE", L, &[], Group::Z, Refused, Lands::Other, "RISC-V landing pads").at(FINAL),
    r(&[zx("zicfiss")], Arg::Required(Values::Any), "MODE", L, &[], Group::Z, Refused, Lands::Other, "RISC-V shadow stack").at(FINAL),
    r(&[zx("zicfilp-unlabeled-report")], Arg::Required(Values::Any), "LEVEL", L, &[], Group::Z, Refused, Lands::Other, "RISC-V landing pad report").at(FINAL),
    r(&[zx("zicfilp-func-sig-report")], Arg::Required(Values::Any), "LEVEL", L, &[], Group::Z, Refused, Lands::Other, "RISC-V landing pad report").at(FINAL),
    r(&[zx("zicfiss-report")], Arg::Required(Values::Any), "LEVEL", L, &[], Group::Z, Refused, Lands::Other, "RISC-V shadow stack report").at(FINAL),
    r(&[zx("execstack-if-needed")], Arg::None, "", M, &[], Group::Z, Refused, Lands::Other, "Make the stack executable if an input asks").at(FINAL),
    r(&[zx("rewrite-endbr")], Arg::None, "", M, &[], Group::Z, Refused, Lands::Other, "Rewrite unneeded endbr instructions as NOPs").at(FINAL),
    r(&[zx("norewrite-endbr")], Arg::None, "", M, &[], Group::Z, Refused, Lands::Other, "Keep endbr instructions (default)").at(FINAL),
    r(&[s("a")], Arg::Required(Values::Any), "KEYWORD", G, &[], Group::Compatibility, Refused, Lands::Other, "HP-UX shared library control").at(FINAL),
    r(&[s("A"), g("architecture")], Arg::Required(Values::Any), "ARCH", G, &[], Group::Compatibility, Refused, Lands::Other, "Set the architecture").at(FINAL),
    r(&[g1("EB")], Arg::None, "", GLM, &[], Group::Compatibility, Refused, Lands::Other, "Link big-endian objects").at(FINAL),
    r(&[g1("EL")], Arg::None, "", GLM, &[], Group::Compatibility, Noop("every target mortar links for is little-endian"), Lands::Other, "Link little-endian objects").at(noop_r("", "C-R2")),
    r(&[s("g")], Arg::None, "", GL, &[], Group::Compatibility, Noop("GNU ld ignores it too"), Lands::Other, "Ignored").at(noop_r("measured", "C-R2b, rl0-opts")),
    r(&[s("O")], Arg::Required(Values::Any), "LEVEL", GLM, &[], Group::Compatibility, Noop("synth.md S-32: mortar merges at every level, as GNU ld and mold do"), Lands::Other, "Optimize the output").at(noop_r("", "C-R2")),
    r(&[g1("Qy")], Arg::None, "", G, &[], Group::Compatibility, Noop("GNU ld ignores it too (SVR4)"), Lands::Other, "Ignored").at(noop_r("measured", "C-R2b, rl0-opts")),
    r(&[g1("qmagic")], Arg::None, "", G, &[], Group::Compatibility, Noop("GNU ld ignores it too (Linux a.out)"), Lands::Other, "Ignored").at(noop_r("measured", "C-R2b, rl0-opts")),
    r(&[g1("assert")], Arg::Required(Values::Any), "KEYWORD", G, &[], Group::Compatibility, Noop("GNU ld ignores it too (SunOS)"), Lands::Other, "Ignored").at(noop_r("measured", "C-R2b, rl0-opts")),
    r(&[g("map-whole-files")], Arg::None, "", G, &[], Group::Compatibility, Noop("GNU ld ignores it too (gold)"), Lands::Other, "Ignored").at(noop_r("measured", "C-R2b, rl0-opts")),
    r(&[g("no-map-whole-files")], Arg::None, "", G, &[], Group::Compatibility, Noop("GNU ld ignores it too (gold)"), Lands::Other, "Ignored").at(noop_r("measured", "C-R2b, rl0-opts")),
    r(&[g("no-keep-memory")], Arg::None, "", GLM, &[], Group::Compatibility, Noop("a memory-use hint for BFD; it does not change the output"), Lands::Other, "Use less memory, more I/O").at(noop_r("measured", "C-R2b, rl0-opts")),
    r(&[g("reduce-memory-overheads")], Arg::None, "", G, &[], Group::Compatibility, Noop("a memory-use hint for BFD; it does not change the output"), Lands::Other, "Use less memory, more time").at(noop_r("GNU ld reorders `.symtab`; mortar the same bytes", "rl0-opts")),
    r(&[g("hash-size")], Arg::Required(Values::Count), "NUMBER", G, &[], Group::Compatibility, Noop("a table-size hint for BFD; it does not change the output"), Lands::Other, "Hash table size hint").at(noop_r("as `--reduce-memory-overheads`", "rl0-opts")),
    r(&[g("max-cache-size")], Arg::Required(Values::Any), "SIZE", GM, &[], Group::Compatibility, Noop("a memory-use hint for BFD; it does not change the output"), Lands::Other, "Cache size limit").at(noop_r("measured", "C-R2b, rl0-opts")),
    r(&[g("dll-verbose")], Arg::None, "", G, &[], Group::Compatibility, Refused, Lands::Other, "(hidden in GNU ld)").at(FINAL),
    r(&[g("ctf-variables")], Arg::None, "", G, &[], Group::Compatibility, Refused, Lands::Other, "Write CTF for static variables").at(FINAL),
    r(&[g("no-ctf-variables")], Arg::None, "", G, &[], Group::Compatibility, Refused, Lands::Other, "Write no CTF for static variables").at(FINAL),
    r(&[g("ctf-share-types")], Arg::Required(Values::Choice(&["share-unconflicted", "share-duplicated"])), "METHOD", G, &[], Group::Compatibility, Refused, Lands::Other, "How CTF types are shared").at(noop_r("by what the option is", "C-R2b")),
    r(&[x("be8")], Arg::None, "", L, &[], Group::Compatibility, Refused, Lands::Other, "(lld, Arm) BE8 output").at(FINAL),
    r(&[x("fix-cortex-a8")], Arg::None, "", L, &[], Group::Compatibility, Refused, Lands::Other, "(lld, Arm) Cortex-A8 erratum").at(FINAL),
    r(&[x("merge-exidx-entries")], Arg::None, "", L, &[], Group::Compatibility, Refused, Lands::Other, "(lld, Arm) merge .ARM.exidx").at(FINAL),
    r(&[x("no-merge-exidx-entries")], Arg::None, "", L, &[], Group::Compatibility, Refused, Lands::Other, "(lld, Arm) do not merge .ARM.exidx").at(FINAL),
    r(&[x("target1-abs")], Arg::None, "", L, &[], Group::Compatibility, Refused, Lands::Other, "(lld, Arm) R_ARM_TARGET1 is absolute").at(FINAL),
    r(&[x("target1-rel")], Arg::None, "", L, &[], Group::Compatibility, Refused, Lands::Other, "(lld, Arm) R_ARM_TARGET1 is relative").at(FINAL),
    r(&[x("target2")], Arg::Required(Values::Any), "TYPE", L, &[], Group::Compatibility, Refused, Lands::Other, "(lld, Arm) R_ARM_TARGET2").at(FINAL),
    r(&[x("cmse-implib")], Arg::None, "", L, &[], Group::Compatibility, Refused, Lands::Other, "(lld, Arm) CMSE import library").at(FINAL),
    r(&[x("in-implib")], Arg::Required(Values::Any), "FILE", L, &[], Group::Compatibility, Refused, Lands::Other, "(lld, Arm) CMSE input import library").at(FINAL),
    r(&[x("relax-gp")], Arg::None, "", L, &[], Group::Compatibility, Refused, Lands::Other, "(lld, RISC-V) global pointer relaxation").at(FINAL),
    r(&[x("no-relax-gp")], Arg::None, "", L, &[], Group::Compatibility, Refused, Lands::Other, "(lld, RISC-V) no global pointer relaxation").at(FINAL),
    r(&[x("pcrel-optimize")], Arg::None, "", L, &[], Group::Compatibility, Refused, Lands::Other, "(lld, PowerPC64)").at(FINAL),
    r(&[x("no-pcrel-optimize")], Arg::None, "", L, &[], Group::Compatibility, Refused, Lands::Other, "(lld, PowerPC64)").at(FINAL),
    r(&[x("toc-optimize")], Arg::None, "", L, &[], Group::Compatibility, Refused, Lands::Other, "(lld, PowerPC64)").at(FINAL),
    r(&[x("no-toc-optimize")], Arg::None, "", L, &[], Group::Compatibility, Refused, Lands::Other, "(lld, PowerPC64)").at(FINAL),
    r(&[x("power10-stubs")], Arg::Optional(Values::Any), "MODE", L, &[], Group::Compatibility, Refused, Lands::Other, "(lld, PowerPC64)").at(FINAL),
    r(&[x("no-power10-stubs")], Arg::None, "", L, &[], Group::Compatibility, Refused, Lands::Other, "(lld, PowerPC64)").at(FINAL),
    r(&[x("split-stack-adjust-size")], Arg::Required(Values::Any), "SIZE", L, &[], Group::Compatibility, Refused, Lands::Other, "(lld) split-stack adjustment").at(FINAL),
];
