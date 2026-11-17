//! `spec/dyn.md` D-117, "Bound use through a register" (items 1-9): the
//! AArch64 analysis of `mortar_scan::regflow`, on objects assembled or
//! compiled by the test (`support::ARCH`'s GNU tools, Clang), with the
//! preemptible definitions of the default line (`regflow::default_line`:
//! every `STV_DEFAULT` global definition, as the oracle
//! `tests/shared-design/exp3/s1d.py` assumes).
//!
//! The off-gate check (record `spec/landed/dyn/WP-119.md` §4) is
//! [`corpus`], ignored by default: it prints, for every object of a
//! list, the analysis's uses in `s1d.py --hits`'s format.

mod support;

use std::fmt::Write as _;
use std::path::Path;

use mortar_diag::{FileRef, Style};
use mortar_elf::Object;
use mortar_elf::archive::Archive;
use mortar_scan::regflow::{Analysis, Reach, analyse, default_line, resident};
use mortar_testutil::Scratch;
use support::{assemble, compile, need};

/// The analysis of an object file's bytes with the default line.
fn analyse_bytes(bytes: &[u8]) -> Analysis {
    let obj = Object::parse(bytes).expect("an object");
    let defs = default_line(&obj).expect("symbols");
    analyse(&obj, &defs, &mut resident(&obj)).expect("analysis")
}

fn analyse_file(path: &Path) -> Analysis {
    analyse_bytes(&std::fs::read(path).unwrap())
}

/// The uses as `s1d.py --hits` prints them, each once:
/// `ANA <function>+<offset> <form> <section>+<a> <definition> [boundary]`.
fn hits(a: &Analysis) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for u in &a.uses {
        let form = match u.reach {
            Reach::Computed => "add",
            Reach::Access => "ldst",
            Reach::WriteBack => "wb",
        };
        let line = format!(
            "ANA {}+{:#x} {form} {}+{:#x} {} {}",
            u.function_name,
            u.function_offset,
            u.address_section_name,
            u.address.offset,
            u.definition_name,
            if u.boundary { "boundary" } else { "" }
        );
        if !out.contains(&line) {
            out.push(line);
        }
    }
    out
}

/// The data of the assembled cases: in `.data`, the local `l` (0..8),
/// the exported `g` (8..0x18) and `h` (0x18..0x28), and a literal word
/// (`.Lpool`, 0x28) naming `.data + 0xc`, inside `g`.
const DATA: &str = "
    .data
    .balign 16
.Lbase:
    .type l, %object
    .size l, 8
l:  .xword 0
    .globl g
    .type g, %object
    .size g, 16
g:  .zero 16
    .globl h
    .type h, %object
    .size h, 16
h:  .zero 16
.Lpool:
    .xword .Lbase+12
";

/// The defining form most cases start with: `x0 = .data + 0` at `f+0x4`
/// (`R_AARCH64_ADD_ABS_LO12_NC` against `.data`); the body starts at
/// `f+0x8`.
const PRO: &str = "    adrp x0, .Lbase\n    add x0, x0, :lo12:.Lbase\n";

/// `body` as the one function `f` beside [`DATA`], assembled.
fn object(s: &Scratch, name: &str, body: &str) -> Option<Vec<u8>> {
    let src = format!(
        "    .arch armv8.1-a\n{DATA}
    .text
    .globl f
    .type f, %function
    .p2align 2
f:
{body}
    .size f, .-f
"
    );
    let o = assemble(s.path(), name, &src)?;
    Some(std::fs::read(o).unwrap())
}

/// The uses of `body` (after [`PRO`] when `pro`), each as
/// `f+<off> <form> <section>+<a> <definition>[ boundary]`.
fn uses_of(s: &Scratch, name: &str, pro: bool, body: &str) -> Option<Vec<String>> {
    let body = if pro { format!("{PRO}{body}") } else { body.to_owned() };
    let a = analyse_bytes(&object(s, name, &body)?);
    assert!(a.ran, "{name}: the pre-filter passes");
    Some(hits(&a).iter().map(|h| h.trim_start_matches("ANA ").trim_end().to_owned()).collect())
}

/// Checks each `(name, body, uses)` case after [`PRO`].
fn cases(label: &str, pro: bool, cases: &[(&str, &str, &[&str])]) {
    let s = Scratch::new(label);
    for (name, body, want) in cases {
        let Some(got) = uses_of(&s, name, pro, body) else { return };
        assert_eq!(got, *want, "{name}:\n{body}");
    }
}

/// Item 5's table, row by row (the branch rows with item 8's control
/// flow, `BL`/`BLR` with item 7).
#[test]
fn decoding_table() {
    cases(
        "regflow-table",
        true,
        &[
            // B: the target is followed.
            ("b", "b 1f\n ret\n1: ldr w1, [x0, #12]", &["f+0x10 ldst .data+0xc g"]),
            // A word no branch reaches starts empty, and meets.
            ("b-dead", "b 1f\n nop\n1: ldr w1, [x0, #12]", &[]),
            // BL kills x0-x18 and x30, keeps x19-x29.
            (
                "bl",
                "mov x19, x0\n mov x18, x0\n mov x30, x0\n bl ext\n ldr w1, [x19, #12]\n \
                 ldr w2, [x18, #12]\n ldr w3, [x30, #12]\n ldr w4, [x0, #12]",
                &["f+0x18 ldst .data+0xc g"],
            ),
            (
                "blr",
                "mov x29, x0\n blr x9\n ldr w1, [x29, #12]\n ldr w2, [x0, #12]",
                &["f+0x10 ldst .data+0xc g"],
            ),
            // B.cond: both successors; the meet of different values kills.
            ("bcond", "cmp x1, #0\n b.eq 1f\n add x0, x0, #4\n1: ldr w1, [x0, #12]", &[]),
            // The meet of equal values keeps.
            (
                "bcond-same",
                "cmp x1, #0\n b.eq 1f\n mov x5, x6\n1: ldr w1, [x0, #12]",
                &["f+0x14 ldst .data+0xc g"],
            ),
            ("cbz", "cbz x1, 1f\n add x0, x0, #4\n1: ldr w1, [x0, #12]", &[]),
            ("cbz-fall", "cbz x1, 1f\n ldr w1, [x0, #12]\n1: ret", &["f+0xc ldst .data+0xc g"]),
            ("tbz", "tbz w1, #3, 1f\n add x0, x0, #4\n1: ldr w2, [x0, #12]", &[]),
            // BR: every leader starts empty (the target of the CBZ here).
            ("br", "cbz x1, 1f\n br x9\n1: ldr w2, [x0, #12]", &[]),
            ("no-br", "cbz x1, 1f\n nop\n1: ldr w2, [x0, #12]", &["f+0x10 ldst .data+0xc g"]),
            // BR: a word that is not a leader keeps its state.
            ("br-not-leader", "ldr w2, [x0, #12]\n br x9", &["f+0x8 ldst .data+0xc g"]),
            ("ret", "ret\n ldr w1, [x0, #12]", &[]),
            // System: kills Rd and x30.
            (
                "sys",
                "mov x30, x0\n mov x1, x0\n msr nzcv, x1\n ldr w1, [x30, #12]\n \
                 ldr w2, [x1, #12]\n ldr w3, [x0, #12]",
                &["f+0x1c ldst .data+0xc g"],
            ),
            ("mrs", "mrs x0, nzcv\n ldr w1, [x0, #12]", &[]),
            // ADD/SUB (immediate), 64-bit: computed; the boundary (g's start
            // is l's end, no self-GOT).
            (
                "addsub",
                "add x1, x0, #12\n sub x2, x1, #4\n add x3, x2, #1, lsl #12",
                &["f+0x8 add .data+0xc g", "f+0xc add .data+0x8 g boundary"],
            ),
            // 32-bit: kills.
            ("add32", "add w1, w0, #12\n ldr w2, [x1]", &[]),
            ("adrp", "adrp x0, h\n ldr w1, [x0, #12]", &[]),
            ("adr", "adr x0, f\n ldr w1, [x0, #12]", &[]),
            // MOV (register), 64-bit propagates; 32-bit kills.
            (
                "mov",
                "mov x1, x0\n ldr w2, [x1, #12]\n mov w1, w0\n ldr w3, [x1, #12]",
                &["f+0xc ldst .data+0xc g"],
            ),
            ("other", "eor x0, x0, x1\n ldr w2, [x0, #12]", &[]),
            ("unallocated", ".inst 0\n ldr w2, [x0, #12]", &[]),
        ],
    );
}

/// Item 5's five load/store forms and the rest of the group; item 6's
/// write-back.
#[test]
fn load_store_forms() {
    cases(
        "regflow-ldst",
        true,
        &[
            (
                "unsigned",
                "ldr w1, [x0, #12]\n str x2, [x0, #16]\n ldr q1, [x0, #16]\n ldrb w3, [x0, #13]",
                &[
                    "f+0x8 ldst .data+0xc g",
                    "f+0xc ldst .data+0x10 g",
                    "f+0x10 ldst .data+0x10 g",
                    "f+0x14 ldst .data+0xd g",
                ],
            ),
            // A load into its base kills it.
            ("load-base", "ldr x0, [x0, #12]\n ldr w1, [x0, #12]", &["f+0x8 ldst .data+0xc g"]),
            ("unscaled", "ldur w1, [x0, #13]\n stur x2, [x0, #-1]", &["f+0x8 ldst .data+0xd g"]),
            // Post-index: the access at the base (inside l), the write-back.
            (
                "post",
                "ldr w1, [x0], #12\n ldr w2, [x0]",
                &["f+0x8 wb .data+0xc g", "f+0xc ldst .data+0xc g"],
            ),
            ("pre", "ldr w1, [x0, #12]!", &["f+0x8 ldst .data+0xc g", "f+0x8 wb .data+0xc g"]),
            ("unprivileged", "ldtr w1, [x0, #12]", &["f+0x8 ldst .data+0xc g"]),
            (
                "pair",
                "ldp w1, w2, [x0, #12]\n stp q0, q1, [x0, #16]\n ldp x1, x2, [x0, #16]!\n \
                 stp x1, x2, [x0], #-16\n ldp x0, x1, [x0, #16]\n ldr w3, [x0, #16]",
                &[
                    "f+0x8 ldst .data+0xc g",
                    "f+0xc ldst .data+0x10 g",
                    "f+0x10 ldst .data+0x10 g",
                    "f+0x10 wb .data+0x10 g",
                    "f+0x14 ldst .data+0x10 g",
                    "f+0x18 ldst .data+0x10 g",
                ],
            ),
            // Exclusive: offset 0; kills Rt, Rs.
            (
                "exclusive",
                "add x3, x0, #12\n add x5, x0, #12\n stxr w5, x1, [x3]\n ldr w6, [x5]\n \
                 ldxr x3, [x3]\n ldr w7, [x3]",
                &[
                    "f+0x8 add .data+0xc g",
                    "f+0xc add .data+0xc g",
                    "f+0x10 ldst .data+0xc g",
                    "f+0x18 ldst .data+0xc g",
                ],
            ),
            // CASP kills Rs and Rs+1.
            (
                "casp",
                "add x5, x0, #12\n casp x4, x5, x6, x7, [x9]\n ldr w1, [x5]",
                &["f+0x8 add .data+0xc g"],
            ),
            (
                "atomic",
                "add x3, x0, #12\n ldadd w1, w3, [x3]\n ldr w2, [x3]",
                &["f+0x8 add .data+0xc g", "f+0xc ldst .data+0xc g"],
            ),
            // LDR (literal) that is no defining form: kills Rt.
            (
                "literal",
                "add x1, x0, #12\n ldr x1, 1f\n ldr w2, [x1]\n ret\n .balign 8\n1: .xword 0",
                &["f+0x8 add .data+0xc g"],
            ),
            // Any other word of the group (LD1): kills every register.
            ("other-ldst", "ld1 {v0.16b}, [x1]\n ldr w2, [x0, #12]", &[]),
            ("prfm", "prfm pldl1keep, [x0, #16]\n prfum pldl1keep, [x0, #13]", &[]),
        ],
    );
}

/// Item 4's defining forms (without [`PRO`]).
#[test]
fn defining_forms() {
    cases(
        "regflow-defs",
        false,
        &[
            // 274.
            ("adr", "adr x0, .Lbase+12\n ldr w1, [x0]", &["f+0x4 ldst .data+0xc g"]),
            // 286, through the literal word at .data+0x28.
            (
                "ldst64",
                "adrp x1, .Lpool\n ldr x1, [x1, :lo12:.Lpool]\n ldr w2, [x1]",
                &["f+0x8 ldst .data+0xc g"],
            ),
            // 273.
            ("ld-prel", "ldr x1, .Lpool\n ldr w2, [x1]", &["f+0x4 ldst .data+0xc g"]),
            // An LDR (literal) without a relocation, its word in the code.
            (
                "literal",
                "ldr x1, 1f\n ldr w2, [x1]\n ret\n .balign 8\n1: .xword .Lbase+12",
                &["f+0x4 ldst .data+0xc g"],
            ),
            // A 32-bit LDR (literal) is none.
            ("literal32", "ldr w1, 1f\n ldr w2, [x1]\n ret\n .balign 8\n1: .xword .Lbase+12", &[]),
        ],
    );
}

/// Item 2: a data word (`$d`) is no instruction and kills every register.
#[test]
fn data_words() {
    cases(
        "regflow-data",
        true,
        &[
            // 0x91003001 is `add x1, x0, #12`.
            ("word", ".word 0x91003001\n ldr w2, [x0, #12]", &[]),
            (
                "inst",
                ".inst 0x91003001\n ldr w2, [x0, #12]",
                &["f+0x8 add .data+0xc g", "f+0xc ldst .data+0xc g"],
            ),
        ],
    );
}

/// Item 9: a boundary address is a use when accessed, or when the object
/// has no self-GOT; an address that is not one past the end of a local is
/// none.
#[test]
fn boundary_rule() {
    cases(
        "regflow-boundary",
        true,
        &[
            (
                "no-self-got",
                "add x1, x0, #8\n ldr w2, [x0, #8]",
                &["f+0x8 add .data+0x8 g boundary", "f+0xc ldst .data+0x8 g boundary"],
            ),
            (
                "self-got",
                "add x1, x0, #8\n ldr w2, [x0, #8]\n adrp x9, :got:g\n \
                 ldr x9, [x9, :got_lo12:g]\n add x3, x0, #24",
                &["f+0xc ldst .data+0x8 g boundary", "f+0x18 add .data+0x18 h"],
            ),
        ],
    );
}

/// Item 1 and the analysis's input: nothing runs without a relocation
/// against a local of some T, nor for another machine, nor without
/// preemptible data definitions.
#[test]
fn not_run() {
    let s = Scratch::new("regflow-notrun");
    let Some(globals) =
        object(&s, "globals", "adrp x0, g\n add x0, x0, :lo12:g\n ldr w1, [x0, #4]")
    else {
        return;
    };
    assert_eq!(analyse_bytes(&globals), Analysis::default());
    let Some(mut bytes) = object(&s, "local", &format!("{PRO} ldr w1, [x0, #12]")) else { return };
    assert!(analyse_bytes(&bytes).refused());
    let obj = Object::parse(&bytes).unwrap();
    assert_eq!(analyse(&obj, &[], &mut resident(&obj)).unwrap(), Analysis::default());
    // e_machine EM_X86_64.
    bytes[18..20].copy_from_slice(&62u16.to_le_bytes());
    assert_eq!(analyse_bytes(&bytes), Analysis::default());
}

/// The diagnostic (`spec/diag.md` §3.2) of a bound use.
#[test]
fn diagnostic() {
    let s = Scratch::new("regflow-diag");
    let Some(bytes) = object(&s, "d", &format!("{PRO} ldr w1, [x0, #12]\n add x1, x0, #8")) else {
        return;
    };
    let a = analyse_bytes(&bytes);
    assert_eq!(a.definitions().len(), 1);
    let text: String =
        a.diagnostics(&FileRef::new("d.o")).iter().map(|d| d.render(&Style::plain("ld"))).collect();
    assert_eq!(
        text,
        "ld: error: d.o:(.text+0x8): relocation R_AARCH64_ADD_ABS_LO12_NC against symbol `g' \
         which may bind externally can not be used when making a shared object; recompile with \
         -fPIC\n\
         >>> the address .data+0xc is accessed by the instruction at f+0x8, in a register \
         formed by R_AARCH64_ADD_ABS_LO12_NC against .data at .text+0x4; it lies inside g\n\
         ld: error: d.o:(.text+0xc): relocation R_AARCH64_ADD_ABS_LO12_NC against symbol `g' \
         which may bind externally can not be used when making a shared object; recompile with \
         -fPIC\n\
         >>> the address .data+0x8 is computed by the instruction at f+0xc, in a register \
         formed by R_AARCH64_ADD_ABS_LO12_NC against .data at .text+0x4; it lies inside g\n\
         >>> compile with -fdata-sections (each object in its own section)\n"
    );
}

/// A tiny LCG (the tests' only randomness; fixed seeds).
struct Lcg(u64);

impl Lcg {
    fn next(&mut self) -> u32 {
        self.0 =
            self.0.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
        (self.0 >> 32) as u32
    }
}

/// No input panics: random words after a defining form, and random bytes
/// anywhere in the file (the analysis may fault, never panic).
#[test]
fn never_panics() {
    let s = Scratch::new("regflow-random");
    let nops = " nop\n".repeat(64);
    let Some(base) = object(&s, "r", &format!("{PRO}{nops} ret")) else { return };
    let nop = 0xd503_201fu32.to_le_bytes().repeat(64);
    let at = base.windows(nop.len()).position(|w| w == nop.as_slice()).expect("the nops");
    let mut rng = Lcg(0x5eed);
    let mut runs = 0u32;
    for _ in 0..400 {
        let mut b = base.clone();
        for k in 0..64 {
            b[at + 4 * k..at + 4 * k + 4].copy_from_slice(&rng.next().to_le_bytes());
        }
        let a = analyse_bytes(&b);
        assert!(a.ran);
        assert_eq!(a, analyse_bytes(&b), "deterministic");
        runs += 1;
    }
    for _ in 0..3000 {
        let mut b = base.clone();
        for _ in 0..=(rng.next() % 8) {
            let i = rng.next() as usize % b.len();
            b[i] = rng.next() as u8;
        }
        if rng.next().is_multiple_of(16) {
            b.truncate(rng.next() as usize % b.len());
        }
        let Ok(obj) = Object::parse(&b) else { continue };
        let defs = default_line(&obj).unwrap_or_default();
        let _ = analyse(&obj, &defs, &mut resident(&obj));
        runs += 1;
    }
    assert!(runs > 400);
}

/// What the oracle `s1d.py --hits` prints for W44's object.
const GAP2_HITS: [&str; 2] =
    ["ANA all+0xc ldst .data+0x4 g boundary", "ANA set+0xc ldst .data+0x4 g boundary"];

/// W44 (`tests/shared-design/exp2/gapdemo/gap2.c`, GCC `-O2`, the
/// default `-fPIE`): `g`, between `h2` and `h3` in `.data`, is reached
/// from the anchor `h1` by an immediate.
#[test]
fn w44_gap2() {
    let s = Scratch::new("regflow-gap2");
    let src = include_str!("../../../tests/shared-design/exp2/gapdemo/gap2.c");
    let Some(o) = compile(s.path(), "gcc", "gap2.o", "c", src, &["-O2"]) else { return };
    let a = analyse_file(&o);
    assert_eq!(hits(&a), GAP2_HITS, "{:#?}", a.uses);
    assert!(a.uses.iter().all(|u| u.definition_name == "g"));
}

/// The Clang `-O0` case of `shared-pic-before-start` (dyn.md D-117,
/// "before the start"; `tests/shared-design/sup/c.c`, `-fPIC`): `tab - 1`
/// folds to `.data + 0x10`, the start of `gcount`, and is refused with
/// both notes.
#[test]
fn clang_before_start() {
    let s = Scratch::new("regflow-before");
    let Some(clang) = need("clang") else { return };
    std::fs::write(s.path().join("c.c"), include_str!("../../../tests/shared-design/sup/c.c"))
        .unwrap();
    let args: Vec<std::ffi::OsString> =
        ["--target=aarch64-linux-gnu", "-O0", "-fPIC", "-c", "c.c", "-o", "c.o"]
            .iter()
            .map(Into::into)
            .collect();
    let r = support::run_in(&clang, &args, s.path());
    assert!(r.success(), "clang: {}", r.stderr_text());
    let a = analyse_file(&s.path().join("c.o"));
    let uses: Vec<_> = a.uses.iter().filter(|u| u.before_start()).collect();
    assert!(!uses.is_empty(), "{:#?}", a.uses);
    for u in &a.uses {
        assert_eq!((u.address_section_name.as_str(), u.address.offset), (".data", 0x10));
        assert_eq!(u.definition_name, "gcount");
        assert!(u.before_start() && !u.boundary, "{u:#?}");
    }
    let text = uses[0].diagnostic(&FileRef::new("c.o")).render(&Style::plain("ld"));
    assert!(
        text.contains(
            ">>> compile with -fdata-sections (each object in its own section), and do not \
             form an address before an object"
        ),
        "{text}"
    );
    // `-O2`: the address is formed exactly.
    let args: Vec<std::ffi::OsString> =
        ["--target=aarch64-linux-gnu", "-O2", "-fPIC", "-c", "c.c", "-o", "c2.o"]
            .iter()
            .map(Into::into)
            .collect();
    let r = support::run_in(&clang, &args, s.path());
    assert!(r.success(), "clang: {}", r.stderr_text());
    assert!(!analyse_file(&s.path().join("c2.o")).refused());
}

/// abseil's `raw_logging.cc` (GCC 14 `-O2`), reduced: the hooks
/// `RegisterLogFilterAndPrefixHook` and `RegisterInternalLogFunction`
/// over the anchor `.LANCHOR0` of `.data.rel.local` (the local
/// `log_filter_and_prefix_hook`, then the exported
/// `internal_log_function`). `{0}` is how the second reaches its datum.
const RAW_LOGGING: &str = "
    .text
    .align 2
    .global _ZN4absl12lts_2024072216raw_log_internal30RegisterLogFilterAndPrefixHookEPFbNS0_11LogSeverityEPKciPPcPiE
    .type _ZN4absl12lts_2024072216raw_log_internal30RegisterLogFilterAndPrefixHookEPFbNS0_11LogSeverityEPKciPPcPiE, %function
_ZN4absl12lts_2024072216raw_log_internal30RegisterLogFilterAndPrefixHookEPFbNS0_11LogSeverityEPKciPPcPiE:
    stp x29, x30, [sp, -32]!
    mov x29, sp
    stp x19, x20, [sp, 16]
    cbz x0, .L43
    adrp x2, .LANCHOR0
    add x2, x2, :lo12:.LANCHOR0
    mov x1, x0
    mov x19, x0
    ldr x20, [x2, 8]
    mov x0, x20
    bl __aarch64_cas8_acq_rel
    ldp x19, x20, [sp, 16]
    ldp x29, x30, [sp], 32
    ret
.L43:
    bl abort
    .size _ZN4absl12lts_2024072216raw_log_internal30RegisterLogFilterAndPrefixHookEPFbNS0_11LogSeverityEPKciPPcPiE, .-_ZN4absl12lts_2024072216raw_log_internal30RegisterLogFilterAndPrefixHookEPFbNS0_11LogSeverityEPKciPPcPiE
    .align 2
    .global _ZN4absl12lts_2024072216raw_log_internal27RegisterInternalLogFunctionEPFvNS0_11LogSeverityEPKciRKNSt7__cxx1112basic_stringIcSt11char_traitsIcESaIcEEEE
    .type _ZN4absl12lts_2024072216raw_log_internal27RegisterInternalLogFunctionEPFvNS0_11LogSeverityEPKciRKNSt7__cxx1112basic_stringIcSt11char_traitsIcESaIcEEEE, %function
_ZN4absl12lts_2024072216raw_log_internal27RegisterInternalLogFunctionEPFvNS0_11LogSeverityEPKciRKNSt7__cxx1112basic_stringIcSt11char_traitsIcESaIcEEEE:
    stp x29, x30, [sp, -32]!
    mov x29, sp
    stp x19, x20, [sp, 16]
    cbz x0, .L57
{0}
    mov x0, x20
    bl __aarch64_cas8_acq_rel
    ldp x19, x20, [sp, 16]
    ldp x29, x30, [sp], 32
    ret
.L57:
    bl abort
    .size _ZN4absl12lts_2024072216raw_log_internal27RegisterInternalLogFunctionEPFvNS0_11LogSeverityEPKciRKNSt7__cxx1112basic_stringIcSt11char_traitsIcESaIcEEEE, .-_ZN4absl12lts_2024072216raw_log_internal27RegisterInternalLogFunctionEPFvNS0_11LogSeverityEPKciRKNSt7__cxx1112basic_stringIcSt11char_traitsIcESaIcEEEE
    .global _ZN4absl12lts_2024072216raw_log_internal21internal_log_functionB5cxx11E
    .section .data.rel.local,\"aw\"
    .align 3
    .set .LANCHOR0,. + 0
    .type _ZN4absl12lts_2024072216raw_log_internal12_GLOBAL__N_126log_filter_and_prefix_hookE, %object
    .size _ZN4absl12lts_2024072216raw_log_internal12_GLOBAL__N_126log_filter_and_prefix_hookE, 32
_ZN4absl12lts_2024072216raw_log_internal12_GLOBAL__N_126log_filter_and_prefix_hookE:
    .zero 32
    .type _ZN4absl12lts_2024072216raw_log_internal21internal_log_functionB5cxx11E, %object
    .size _ZN4absl12lts_2024072216raw_log_internal21internal_log_functionB5cxx11E, 16
_ZN4absl12lts_2024072216raw_log_internal21internal_log_functionB5cxx11E:
    .zero 16
";

/// abseil's `format.cc` (GCC 14 `-O2`), reduced: `.LANCHOR0` of
/// `.rodata` holds the locals `kInfiniteFutureStr`, `kInfinitePastStr`
/// (with two bytes of padding after it: no boundary), then the exported
/// `RFC3339_full`. `{0}` is how `FormatTime` names it.
const FORMAT: &str = "
    .text
    .align 2
    .type _ZN4absl12lts_2024072212_GLOBAL__N_19Infinite, %function
_ZN4absl12lts_2024072212_GLOBAL__N_19Infinite:
    adrp x1, .LANCHOR0
    add x1, x1, :lo12:.LANCHOR0
    mov x0, x8
    add x2, x1, 15
    b _ZNSt7__cxx1112basic_stringIcSt11char_traitsIcESaIcEE12_M_construct
    .size _ZN4absl12lts_2024072212_GLOBAL__N_19Infinite, .-_ZN4absl12lts_2024072212_GLOBAL__N_19Infinite
    .align 2
    .global _ZN4absl12lts_2024072210FormatTimeB5cxx11ENS0_4TimeENS0_8TimeZoneE
    .type _ZN4absl12lts_2024072210FormatTimeB5cxx11ENS0_4TimeENS0_8TimeZoneE, %function
_ZN4absl12lts_2024072210FormatTimeB5cxx11ENS0_4TimeENS0_8TimeZoneE:
    stp x29, x30, [sp, -48]!
{0}
    mov x29, sp
    uxtw x3, w1
    mov x4, x2
    mov x2, x0
    mov x0, 24
    str x19, [sp, 16]
    mov x19, x8
    bl _ZN4absl12lts_2024072210FormatTimeB5cxx11ESt17basic_string_viewIcSt11char_traitsIcEENS0_4TimeENS0_8TimeZoneE
    mov x0, x19
    ldr x19, [sp, 16]
    ldp x29, x30, [sp], 48
    ret
    .size _ZN4absl12lts_2024072210FormatTimeB5cxx11ENS0_4TimeENS0_8TimeZoneE, .-_ZN4absl12lts_2024072210FormatTimeB5cxx11ENS0_4TimeENS0_8TimeZoneE
    .global _ZN4absl12lts_2024072212RFC3339_fullE
    .section .rodata
    .align 4
    .set .LANCHOR0,. + 0
    .type _ZN4absl12lts_2024072212_GLOBAL__N_1L18kInfiniteFutureStrE, %object
    .size _ZN4absl12lts_2024072212_GLOBAL__N_1L18kInfiniteFutureStrE, 16
_ZN4absl12lts_2024072212_GLOBAL__N_1L18kInfiniteFutureStrE:
    .string \"infinite-future\"
    .type _ZN4absl12lts_2024072212_GLOBAL__N_1L16kInfinitePastStrE, %object
    .size _ZN4absl12lts_2024072212_GLOBAL__N_1L16kInfinitePastStrE, 14
_ZN4absl12lts_2024072212_GLOBAL__N_1L16kInfinitePastStrE:
    .string \"infinite-past\"
    .zero 2
    .type _ZN4absl12lts_2024072212RFC3339_fullE, %object
    .size _ZN4absl12lts_2024072212RFC3339_fullE, 25
_ZN4absl12lts_2024072212RFC3339_fullE:
    .string \"%Y-%m-%d%ET%H:%M:%E*S%Ez\"
";

/// The twins (record `spec/landed/dyn/WP-110.md` §G.1): `-fPIE` objects
/// are refused by the analysis alone (no relocation names the exported
/// datum), their `-fPIC` twins, which reach it through the GOT, are not.
#[test]
fn abseil_twins() {
    let s = Scratch::new("regflow-abseil");
    let raw_anchor = "    adrp x2, .LANCHOR0\n    add x2, x2, :lo12:.LANCHOR0\n    mov x1, x0\n    \
                   mov x19, x0\n    add x2, x2, 32\n    ldr x20, [x2, 8]";
    let raw_got = "    adrp x2, :got:_ZN4absl12lts_2024072216raw_log_internal21internal_log_functionB5cxx11E\n    \
                   ldr x2, [x2, :got_lo12:_ZN4absl12lts_2024072216raw_log_internal21internal_log_functionB5cxx11E]\n    \
                   mov x1, x0\n    mov x19, x0\n    ldr x20, [x2, 8]";
    let fmt_anchor = "    adrp x5, .LANCHOR0\n    add x5, x5, :lo12:.LANCHOR0\n    add x1, x5, 32";
    let fmt_got = "    adrp x1, :got:_ZN4absl12lts_2024072212RFC3339_fullE\n    \
                   ldr x1, [x1, :got_lo12:_ZN4absl12lts_2024072212RFC3339_fullE]";
    let fn_raw = "_ZN4absl12lts_2024072216raw_log_internal27RegisterInternalLogFunctionEPFvNS0_11LogSeverityEPKciRKNSt7__cxx1112basic_stringIcSt11char_traitsIcESaIcEEEE";
    let def_raw = "_ZN4absl12lts_2024072216raw_log_internal21internal_log_functionB5cxx11E";
    let fn_fmt = "_ZN4absl12lts_2024072210FormatTimeB5cxx11ENS0_4TimeENS0_8TimeZoneE";
    let def_fmt = "_ZN4absl12lts_2024072212RFC3339_fullE";
    let want_raw = [
        format!("ANA {fn_raw}+0x20 add .data.rel.local+0x20 {def_raw} boundary"),
        format!("ANA {fn_raw}+0x24 ldst .data.rel.local+0x28 {def_raw} "),
    ];
    let want_fmt = [format!("ANA {fn_fmt}+0xc add .rodata+0x20 {def_fmt} ")];
    for (name, template, body, want) in [
        ("raw_logging.pie", RAW_LOGGING, raw_anchor, &want_raw[..]),
        ("raw_logging.pic", RAW_LOGGING, raw_got, &[][..]),
        ("format.pie", FORMAT, fmt_anchor, &want_fmt[..]),
        ("format.pic", FORMAT, fmt_got, &[][..]),
    ] {
        let Some(o) = assemble(s.path(), name, &template.replace("{0}", body)) else { return };
        let a = analyse_file(&o);
        assert!(a.ran, "{name}: the pre-filter passes");
        assert_eq!(hits(&a), want, "{name}");
    }
}

/// The off-gate check: `REGFLOW_LIST` names a file of object and archive
/// paths, one per line; `REGFLOW_OUT` the report, written in the format
/// of `s1d.py --hits` (an object line for every object the analysis ran
/// on, then its uses; a `TOTAL` line).
#[test]
#[ignore = "off-gate: the corpora of record WP-119 §4"]
fn corpus() {
    let (Ok(list), Ok(report)) = (std::env::var("REGFLOW_LIST"), std::env::var("REGFLOW_OUT"))
    else {
        eprintln!("set REGFLOW_LIST and REGFLOW_OUT");
        return;
    };
    let mut text = String::new();
    let (mut objs, mut pre, mut ana_objs, mut words, mut undec) = (0u64, 0u64, 0u64, 0u64, 0u64);
    let started = std::time::Instant::now();
    let mut one = |name: &str, bytes: &[u8], text: &mut String| {
        let Ok(obj) = Object::parse(bytes) else { return };
        if obj.header().e_machine != mortar_elf::consts::EM_AARCH64 {
            return;
        }
        objs += 1;
        let defs = default_line(&obj).unwrap_or_default();
        let a = match analyse(&obj, &defs, &mut resident(&obj)) {
            Ok(a) => a,
            Err(e) => {
                writeln!(text, "{name} FAULT {e:?}").unwrap();
                return;
            }
        };
        pre += u64::from(a.ran);
        words += a.words;
        undec += a.undecoded;
        ana_objs += u64::from(a.refused());
        if a.ran {
            writeln!(
                text,
                "{name} pre=1 ana={} words={} funcs={}",
                a.uses.len(),
                a.words,
                a.functions
            )
            .unwrap();
            for h in hits(&a) {
                writeln!(text, "    {h}").unwrap();
            }
        }
    };
    for path in std::fs::read_to_string(&list).unwrap().lines().filter(|l| !l.is_empty()) {
        let Ok(bytes) = std::fs::read(path) else { continue };
        if Archive::sniff(&bytes).is_some() {
            let Ok(ar) = Archive::parse(&bytes) else { continue };
            for m in ar.members().flatten() {
                let Some(data) = m.data() else { continue };
                let name = format!("{path}({})", String::from_utf8_lossy(m.name()));
                one(&name, data, &mut text);
            }
        } else {
            one(path, &bytes, &mut text);
        }
    }
    writeln!(
        text,
        "TOTAL objs={objs} pre={pre} ana_objs={ana_objs} words={words} undec={undec} ms={}",
        started.elapsed().as_millis()
    )
    .unwrap();
    std::fs::write(report, text).unwrap();
}
