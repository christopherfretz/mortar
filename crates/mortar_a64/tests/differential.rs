//! `apply` against GNU ld, lld and mold (a64.md §6.1, §7, §8.1; WP-023
//! stage A test (i)).
//!
//! A corpus of C, C++, Fortran and hand-written assembly is compiled by
//! GCC, clang and gfortran and linked statically by each reference linker
//! with `--emit-relocs` and `--no-relax`. Every relocation the linker kept
//! is then applied again by mortar to the linked image, with `S`, `P`, `A`,
//! `G` and the TLS template read from that image: mortar agrees with the
//! reference exactly when the field it writes is the field already there
//! (insertion writes the field's bits only).
//!
//! - **ok**: mortar writes the same bytes.
//! - **mismatch**: mortar writes other bytes, or refuses a value the
//!   reference wrote. Each is printed; there must be none.
//! - **relaxed**: the word no longer has the instruction the type
//!   requires: the reference relaxed it. The comparison runs with R3
//!   and R5 inhibited (the fixed point needs the unrelaxed form);
//!   R1 (TLSDESC → LE) is compared on canonical input words instead.
//!   For IE → LE, R3's bytes are compared on the input rebuilt from the
//!   reference's register (`relaxed_eq`); R3's registers are checked on
//!   the input objects by `s8_3_r3_from_the_inputs`.
//! - **ifunc**: the target is an `STT_GNU_IFUNC`; the reference points the
//!   field at its IPLT entry, which this harness does not locate (the
//!   IPLT path is tested in `a64.rs` §10.4).
//! - **refused**: the type is not enabled at M2, or mortar refuses the
//!   construct by name (never a thread-local `weak0`, a64.md §7).
//!
//! The table printed lists every M1 type, including those no input covers.

#![allow(clippy::many_single_char_names, clippy::too_many_lines)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use mortar_a64::{A64, table};
use mortar_arch::{Arch, DemandFlags, FieldError, Reloc, SlotKind, Status, TargetKind};
use mortar_testutil::{LLVM_MAJORS, Scratch, run, tool};

mod support;
use support::{Case, M2, Tables, key};

mod elf;
use elf::{
    Image, SHF_ALLOC, SHF_TLS, SHN_ABS, SHT_RELA, STT_GNU_IFUNC, STT_SECTION, STT_TLS, Sym, u32_at,
    u64_at,
};

// ---------------------------------------------------------------- comparing

#[derive(Default, Clone, Copy)]
struct Tally {
    ok: u32,
    mismatch: u32,
    relaxed: u32,
    ifunc: u32,
    refused: u32,
    /// A difference the spec records (a64.md §7: GNU ld's branches to
    /// `weak0`; §8.1: mold's R1 order).
    documented: u32,
    /// The emitted record does not describe the field: lld's records
    /// against section symbols and in `.eh_frame` (which it rewrites), and
    /// lld's and mold's records whose symbol is gone (index 0). Not
    /// compared.
    emit: u32,
    /// Of all the above, the records against a thread-local `weak0`
    /// (§7; glibc's `_nl_current_LC_*`), which must never be refused.
    weak_tls: u32,
    /// Of `relaxed`, the IE → LE records whose bytes mortar's R3 writes
    /// too (§8.3).
    relaxed_eq: u32,
}

type Tallies = BTreeMap<u32, Tally>;

/// Canonical words of the TLSDESC sequence, for R1: `adrp x0`, `ldr x1,
/// [x0]`, `add x0, x0`, `blr x1`.
fn tlsdesc_input(r_type: u32) -> Option<u32> {
    match r_type {
        562 => Some(0x9000_0000),
        563 => Some(0xF940_0001),
        564 => Some(0x9100_0000),
        569 => Some(0xD63F_0020),
        _ => None,
    }
}

fn compare(
    label: &str,
    img: &Image,
    tallies: &mut Tallies,
    mismatches: &mut Vec<String>,
    by_place: &mut BTreeMap<String, u32>,
) {
    let syms = img.symbols();
    let got = img.got_words();
    let linker = label.split(' ').next().unwrap_or("");
    let got_base = syms.iter().find(|s| s.name == "_GLOBAL_OFFSET_TABLE_").map(|s| s.value);
    for rs in img
        .sections
        .iter()
        .filter(|s| s.kind == SHT_RELA && s.flags & SHF_ALLOC == 0 && s.info != 0)
    {
        let target = img.sections[rs.info as usize].clone();
        let tdata = img.data(&target);
        for e in img.data(rs).chunks_exact(24) {
            let r_offset = u64_at(e, 0);
            let info = u64_at(e, 8);
            let r_type = (info & 0xFFFF_FFFF) as u32;
            let sym = syms.get((info >> 32) as usize).cloned().unwrap_or(Sym {
                name: String::new(),
                info: 0,
                shndx: 0,
                value: 0,
            });
            let addend = u64_at(e, 16) as i64;
            let entry = tallies.entry(r_type).or_default();
            let Some(h) = table::howto(r_type).filter(|h| M2.enables(h.status)) else {
                entry.refused += 1;
                continue;
            };
            if r_type == 0 {
                entry.ok += 1;
                continue;
            }
            if sym.info & 0xF == STT_GNU_IFUNC {
                entry.ifunc += 1;
                continue;
            }
            // The field's bytes, at the place's address.
            let off = if target.flags & SHF_ALLOC != 0 { r_offset - target.addr } else { r_offset };
            let off = usize::try_from(off).unwrap();
            let end = (off + 8).min(tdata.len());
            let before = &tdata[off..end];
            let tdesc = tlsdesc_input(r_type);
            let input: Vec<u8> = match tdesc {
                Some(w) => w.to_le_bytes().to_vec(),
                None => before.to_vec(),
            };
            // The symbol's facts in the image.
            let tls_section = usize::from(sym.shndx) < img.sections.len()
                && img.sections[usize::from(sym.shndx)].flags & SHF_TLS != 0;
            let tls = sym.info & 0xF == STT_TLS;
            // A static image that linked has no undefined symbol but weak
            // ones (lld writes them `STB_LOCAL`).
            let kind = match sym.shndx {
                0 if info >> 32 != 0 => TargetKind::Weak0,
                SHN_ABS => TargetKind::Abs,
                _ => TargetKind::Local,
            };
            let s = match (tls, img.tls) {
                _ if kind == TargetKind::Weak0 => 0,
                (true, Some(t)) => t.start + sym.value,
                _ => sym.value,
            };
            // §7, §14: GNU ld and mold fill a `weak0` IE slot with the
            // literal `TPREL(0)`, mortar with 0. The slot is located by the
            // reference's value; the content difference is documented.
            let weak_ie = kind == TargetKind::Weak0 && h.class == mortar_arch::DemandClass::TlsIe;
            let lost = info >> 32 == 0 && linker != "bfd";
            let lld_rewrites =
                linker == "lld" && (sym.info & 0xF == STT_SECTION || target.name == ".eh_frame");
            if lost || lld_rewrites {
                entry.emit += 1;
                continue;
            }
            let mut t = Tables::default();
            if tls || tls_section {
                t.tls_sym(key(1), s, kind);
            } else {
                t.sym(key(1), s, kind);
            }
            // The slot: a `.got` word holding the value the class wants.
            let (slot_kind, want) = match (h.class, img.tls) {
                (mortar_arch::DemandClass::TlsIe, Some(t)) => {
                    (SlotKind::TpOff { negated: false }, A64::M1.tls().tp_rel(s, t) as u64)
                }
                (mortar_arch::DemandClass::TlsIe, None) if weak_ie => {
                    (SlotKind::TpOff { negated: false }, 0)
                }
                _ => (SlotKind::Addr, if kind == TargetKind::Weak0 { 0 } else { s }),
            };
            let slots: Vec<Option<u64>> = match h.class {
                mortar_arch::DemandClass::Got | mortar_arch::DemandClass::TlsIe => {
                    got.get(&want).map_or(vec![None], |v| v.iter().map(|&a| Some(a)).collect())
                }
                _ => vec![None],
            };
            // The fixed point re-applies a record to the output word, so
            // only the unrelaxed forms can be compared: R3 and R5 are
            // switched off by their inhibitors (their forms are tested in
            // `relax.rs`). A reference that relaxed shows as `relaxed`.
            t.flags.insert(key(1), DemandFlags::TLSIE_NORELAX | DemandFlags::GOT_NORELAX);
            let mut outcome = None;
            for slot in slots {
                if let Some(a) = slot {
                    t.slots.insert((key(1), slot_kind), a);
                }
                let mut c =
                    Case::new(&t, &input, if target.flags & SHF_ALLOC != 0 { r_offset } else { 0 });
                c.arch = M2;
                c.alloc = target.flags & SHF_ALLOC != 0;
                c.name = target.name.as_bytes();
                c.tls = img.tls;
                c.got_base = got_base;
                let r = Reloc { r_offset: 0, key: key(1), r_type, addend };
                let res = c.apply(&[r]);
                let good = matches!(&res, Ok(after) if after[..] == before[..input.len().min(before.len())]);
                outcome = Some(res);
                if good {
                    break;
                }
            }
            let word = before.get(..4).map(|b| u32::from_le_bytes(b.try_into().unwrap()));
            let branch_form = match (h.class, h.field) {
                (mortar_arch::DemandClass::Branch, mortar_arch::Field::Insn { form }) => Some(form),
                _ => None,
            };
            let outcome = outcome.unwrap();
            // GNU ld: `nop` for CALL26 and JUMP26, a branch to itself for
            // CONDBR19 and TSTBR14 (§7).
            let gnu_weak = linker == "bfd"
                && kind == TargetKind::Weak0
                && branch_form.is_some_and(|f| {
                    word.is_some_and(|w| w == 0xD503_201F || mortar_a64::forms::extract(f, w) == 0)
                });
            // mold's R1: `nop; nop; movz; movk`, the same words in another order.
            let mold_r1 = linker == "mold"
                && tdesc.is_some()
                && word.is_some_and(|w| r1_words(img, s).contains(&w));
            // mold: ADRP against `weak0` addresses page 0 (§7 takes
            // `S = P`, with GNU ld and lld).
            let mold_adrp = linker == "mold"
                && kind == TargetKind::Weak0
                && h.field == mortar_arch::Field::Insn { form: mortar_a64::forms::ADRP }
                && word.is_some_and(|w| {
                    mortar_a64::forms::extract(mortar_a64::forms::ADRP, w)
                        == (addend as u64 & !0xFFF).wrapping_sub(r_offset & !0xFFF) as i64
                });
            let weak_tls_slot = weak_ie
                && matches!(&outcome, Ok(a) if a[..] == before[..a.len().min(before.len())]);
            if kind == TargetKind::Weak0 && tls {
                entry.weak_tls += 1;
            }
            // R3 where the reference relaxed IE → LE (§8.3): the input pair
            // is rebuilt from the reference's `movz`/`movk` register (the
            // matching-register form, `adrp xR; ldr xR, [xR]`), R3 applied
            // to it, and the bytes compared. The registers themselves are
            // checked on the inputs (`s8_3_r3_from_the_inputs`).
            if matches!(outcome, Err(FieldError::Instruction { .. }))
                && matches!(r_type, 541 | 542)
                && let Some(w) = word
            {
                let rd = w & 0x1F;
                let (shape, rebuilt) = if r_type == 541 {
                    (0xD2A0_0000, 0x9000_0000 | rd)
                } else {
                    (0xF280_0000, 0xF940_0000 | rd << 5 | rd)
                };
                let same = w & 0xFFE0_0000 == shape && {
                    t.flags.insert(key(1), DemandFlags::GOT_NORELAX);
                    let input = rebuilt.to_le_bytes();
                    let mut c = Case::new(&t, &input, r_offset);
                    c.arch = M2;
                    c.tls = img.tls;
                    let r = Reloc { r_offset: 0, key: key(1), r_type, addend };
                    matches!(c.apply(&[r]), Ok(a) if a[..] == before[..4])
                };
                if same {
                    entry.relaxed_eq += 1;
                } else {
                    entry.mismatch += 1;
                    mismatches.push(format!(
                        "{label}: R3 {r_type} at {r_offset:#x} in {}: {} S={s:#x}: reference {w:#010x}",
                        target.name, sym.name
                    ));
                }
            }
            match outcome {
                _ if gnu_weak || mold_r1 || mold_adrp || weak_tls_slot => entry.documented += 1,
                Ok(after) if after[..] == before[..after.len().min(before.len())] => entry.ok += 1,
                Err(FieldError::Instruction { .. }) if tdesc.is_none() => entry.relaxed += 1,
                // A thread-local weak0 is never refused (§7): a mismatch.
                Err(FieldError::Refused(_)) if !(kind == TargetKind::Weak0 && tls) => {
                    entry.refused += 1;
                }
                res => {
                    entry.mismatch += 1;
                    let linker = label.split(' ').next().unwrap_or("");
                    let n = by_place
                        .entry(format!("{linker} {} in {}", h.name, target.name))
                        .or_default();
                    *n += 1;
                    if *n <= 3 {
                        mismatches.push(format!(
                            "{label}: {} {} at {r_offset:#x} in {}: {:?} (info {:#x}, shndx {}) S={s:#x} kind={kind:?} A={addend:#x}: reference {:02x?}, mortar {res:?}",
                            r_type,
                            h.name,
                            target.name,
                            sym.name,
                            sym.info,
                            sym.shndx,
                            &before[..before.len().min(8)]
                        ));
                    }
                }
            }
        }
    }
}

/// The four words of R1 for a target at `s` (a64.md §8.1).
fn r1_words(img: &Image, s: u64) -> [u32; 4] {
    let t = img.tls.map_or(0, |t| A64::M1.tls().tp_rel(s, t)) as u32;
    [0xD2A0_0000 | (t >> 16) << 5, 0xF280_0000 | (t & 0xFFFF) << 5, 0xD503_201F, 0xD503_201F]
}

// ---------------------------------------------------------------- the corpus

const C_GENERAL: &str = r#"
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
__thread int tv = 3;
static __thread long tl[4];
extern __thread int te;
__thread int te = 9;
extern void weak_fn(void) __attribute__((weak));
extern int weak_var __attribute__((weak));
int g[100];
static short sh[8];
long long big = 5;
const char *names[] = { "a", "b", "c" };
static int (*fps[])(const char *) = { puts, atoi };
__int128 wide;
static int sw(int x) {
    switch (x) { case 0: return 3; case 1: return 7; case 2: return 11; case 3: return 2;
                 case 4: return 9; case 5: return 1; case 6: return 4; default: return -1; }
}
int main(int c, char **v) {
    tl[c & 3] += c; te++;
    if (weak_fn) weak_fn();
    if (&weak_var) g[1] = weak_var;
    g[c] = tv + (int)tl[0] + sw(c) + sh[c & 7];
    wide += c;
    fps[c & 1](names[c % 3]);
    printf("%d %lld %s\n", g[c], big, strchr(v[0], '/'));
    return (int)wide;
}
"#;

/// Linked with `-nostdlib`: the tiny code model needs the image to be
/// small.
const C_TINY: &str = r"
static int arr[16];
int counter;
long words[4] = { 1, 2, 3, 4 };
__thread int tv;
extern __thread int tie;
__thread int tie = 4;
extern int ext_weak __attribute__((weak));
extern void weak_fn(void) __attribute__((weak));
static long sum(int n) { long s = 0; for (int i = 0; i < n; i++) s += arr[i] + words[i & 3]; return s; }
void entry(void) {
    for (;;) {
        counter += (int)sum(counter & 15) + tv + tie;
        if (&ext_weak) counter += ext_weak;
        if (weak_fn) weak_fn();
        tv++;
    }
}
";

const CXX: &str = r#"
#include <iostream>
#include <map>
#include <memory>
#include <stdexcept>
#include <string>
#include <vector>
struct Base { virtual ~Base() = default; virtual int f() const = 0; };
struct D1 : Base { int f() const override { return 1; } };
struct D2 : Base { int v; explicit D2(int x) : v(x) {} int f() const override { return v; } };
thread_local std::string tls_name = "t";
template <typename T> T twice(T x) { return x + x; }
int main(int argc, char **) {
    std::vector<std::unique_ptr<Base>> v;
    v.push_back(std::make_unique<D1>());
    v.push_back(std::make_unique<D2>(argc));
    std::map<std::string, int> m;
    for (auto &p : v) m[std::to_string(p->f())] += twice(p->f());
    try {
        if (argc > 5) throw std::runtime_error("many");
    } catch (const std::exception &e) { std::cerr << e.what() << '\n'; }
    tls_name += "x";
    std::cout << m.size() << tls_name << twice(2.5) << std::endl;
    return 0;
}
"#;

const F90: &str = r"
module shapes
  implicit none
  real(8), allocatable :: grid(:,:)
  integer :: calls = 0
contains
  subroutine fill(n)
    integer, intent(in) :: n
    integer :: i, j
    allocate(grid(n, n))
    do j = 1, n
      do i = 1, n
        grid(i, j) = real(i * j, 8) / 3.0d0
      end do
    end do
    calls = calls + 1
  end subroutine
end module
program main
  use shapes
  implicit none
  common /blk/ a, b
  real :: a, b
  a = 1.5; b = 2.5
  call fill(4)
  write (*, '(F8.3, I4, 2F6.2)') sum(grid), calls, a, b
end program
";

/// Every M1 type, written by hand, in units small enough that a reference
/// linker that refuses one type refuses only its unit. Each is assembled
/// after [`ASM_DEFS`] by GNU as and llvm-mc. `TLSLE_LDST128_TPREL_LO12`
/// and its `_NC` (570, 571) are not here: no reference linker takes them
/// (GNU as has no operator for them either), so they are reported with
/// no coverage.
#[rustfmt::skip]
const ASM_UNITS: &[(&str, &str)] = &[
    ("asm-addr", r"
        adrp    x0, var
        add     x0, x0, :lo12:var
        adrp    x0, :pg_hi21_nc:var
        ldrb    w1, [x0, :lo12:var]
        ldrh    w1, [x0, :lo12:var2]
        ldr     w1, [x0, :lo12:var4]
        ldr     x1, [x0, :lo12:var8]
        ldr     q1, [x0, :lo12:var16]
"),
    ("asm-uabs", r"
        movz    x0, #:abs_g3:var
        movk    x0, #:abs_g2_nc:var
        movk    x0, #:abs_g1_nc:var
        movk    x0, #:abs_g0_nc:var
        movz    x0, #:abs_g2:var
        movz    x0, #:abs_g1:var
        movz    x0, #:abs_g0:small
"),
    ("asm-sabs", r"
        movz    x0, #:abs_g0_s:neg
        movz    x0, #:abs_g1_s:neg
        movz    x0, #:abs_g2_s:neg
        movz    x0, #:abs_g0_s:small
"),
    ("asm-lit", r"
        ldr     x0, lit_target
        ldr     w0, lit_target
        adr     x0, lit_target
"),
    ("asm-branch", r"
        b.eq    far_label
        cbz     x0, far_label
        bl      far_func
        b       far_func
"),
    ("asm-tbz", r"
        tbz     x0, #3, far_label
        tbnz    x0, #63, far_label
"),
    ("asm-got", r"
        adrp    x0, :got:var
        ldr     x0, [x0, :got_lo12:var]
        adrp    x0, :got:wk
        ldr     x0, [x0, :got_lo12:wk]
"),
    ("asm-got19", r"
        ldr     x0, :got:var
        ldr     x0, :got:wk
"),
    ("asm-ie", r"
        adrp    x0, :gottprel:tv
        ldr     x0, [x0, #:gottprel_lo12:tv]
"),
    // R3's register condition (§8.3): `tv2` has a pair with Rt ≠ Rn, so
    // every use of it keeps IE; `tv` relaxes; `wtv` is a thread-local
    // weak0 (§7).
    ("asm-ie-regs", r"
        adrp    x2, :gottprel:tv2
        ldr     x3, [x2, #:gottprel_lo12:tv2]
        adrp    x4, :gottprel:tv2
        ldr     x4, [x4, #:gottprel_lo12:tv2]
        adrp    x5, :gottprel:tv
        ldr     x5, [x5, #:gottprel_lo12:tv]
        adrp    x6, :gottprel:wtv
        ldr     x6, [x6, #:gottprel_lo12:wtv]
"),
    ("asm-ie19", r"
        ldr     x0, :gottprel:tv
"),
    ("asm-le-g", r"
        movz    x0, #:tprel_g2:tv
        movz    x0, #:tprel_g1:tv
        movz    x0, #:tprel_g0:tv
"),
    ("asm-le-nc", r"
        movk    x0, #:tprel_g1_nc:tv
        movk    x0, #:tprel_g0_nc:tv
        add     x0, x0, #:tprel_hi12:tv, lsl #12
        add     x0, x0, #:tprel_lo12_nc:tv
        ldrb    w1, [x0, #:tprel_lo12_nc:tv]
        ldrh    w1, [x0, #:tprel_lo12_nc:tv]
        ldr     w1, [x0, #:tprel_lo12_nc:tv]
        ldr     x1, [x0, #:tprel_lo12_nc:tv]
"),
    ("asm-le-checked", r"
        add     x0, x0, #:tprel_lo12:tv
        ldrb    w1, [x0, #:tprel_lo12:tv]
        ldrh    w1, [x0, #:tprel_lo12:tv]
        ldr     w1, [x0, #:tprel_lo12:tv]
        ldr     x1, [x0, #:tprel_lo12:tv]
"),
    ("asm-desc", r"
        adrp    x0, :tlsdesc:tv2
        ldr     x1, [x0, #:tlsdesc_lo12:tv2]
        add     x0, x0, #:tlsdesc_lo12:tv2
        .tlsdesccall tv2
        blr     x1
"),
    ("asm-weak-branch", r"
        bl      wk
        b       wk
        b.ne    wk
        cbnz    x1, wk
"),
    ("asm-weak-tbz", r"
        tbnz    x1, #0, wk
"),
    ("asm-weak", r"
        adrp    x0, wk
        add     x0, x0, :lo12:wk
        ldr     x1, [x0, :lo12:wk]
        movz    x0, #:abs_g0_nc:wk
"),
    ("asm-weak-lit", r"
        adr     x0, wk
        ldr     x0, wk
"),
    ("asm-data", r"
        .data
        .xword var
        .xword var + 0x123
        .word var
        .hword small
        .xword var - .
        .word var - .
        .hword near - .
"),
    ("asm-weak-data", r"
        .data
        .xword wk
        .word wk
        .word wk - .
        .xword wk - .
"),
];

/// The targets the units use, and `_start`. `small` and `neg` are given
/// by `--defsym` (absolute symbols defined in the object would be folded by
/// the assembler).
const ASM_DEFS: &str = r#"
        .text
        .globl _start
        .type _start, %function
_start:
        ret

        .section .text.far, "ax", %progbits
        .globl far_label, far_func, lit_target
far_label:
        nop
far_func:
        ret
        .p2align 3
lit_target:
        .xword 42

        .data
        .p2align 4
        .globl var, var2, var4, var8, var16, near
var:    .byte 1
        .p2align 1
var2:   .hword 2
        .p2align 2
var4:   .word 4
        .p2align 3
var8:   .xword 8
        .p2align 4
var16:  .zero 16
        .section .data.near, "aw", %progbits
near:   .xword 0

        .section .tbss, "awT", %nobits
        .p2align 4
        .globl tv, tv2
tv:     .zero 16
tv2:    .zero 8

        .weak wk, wtv

        .text
"#;

#[derive(Clone, Copy, PartialEq)]
enum Lang {
    C,
    Cxx,
    Fortran,
    Asm,
}

#[derive(Clone, Copy)]
struct Build {
    name: &'static str,
    lang: Lang,
    src: &'static str,
    compile: &'static [&'static str],
    /// Linked with `-nostdlib -e <entry>`.
    entry: Option<&'static str>,
}

const BUILDS: &[Build] = &[
    Build { name: "c-O2-g", lang: Lang::C, src: C_GENERAL, compile: &["-O2", "-g"], entry: None },
    Build { name: "c-O0", lang: Lang::C, src: C_GENERAL, compile: &["-O0"], entry: None },
    Build { name: "c-pic", lang: Lang::C, src: C_GENERAL, compile: &["-O2", "-fPIC"], entry: None },
    Build {
        name: "c-large",
        lang: Lang::C,
        src: C_GENERAL,
        compile: &["-O2", "-mcmodel=large", "-fno-pic", "-fno-pie"],
        entry: None,
    },
    Build {
        name: "c-tiny",
        lang: Lang::C,
        src: C_TINY,
        compile: &["-O2", "-mcmodel=tiny", "-fno-pic"],
        entry: Some("entry"),
    },
    Build {
        name: "c-tiny-pic",
        lang: Lang::C,
        src: C_TINY,
        compile: &["-O2", "-mcmodel=tiny", "-fPIC"],
        entry: Some("entry"),
    },
    Build {
        name: "c-small-pic",
        lang: Lang::C,
        src: C_TINY,
        compile: &["-O1", "-fpic"],
        entry: Some("entry"),
    },
    Build { name: "cxx-O2", lang: Lang::Cxx, src: CXX, compile: &["-O2", "-g"], entry: None },
    Build { name: "cxx-pic", lang: Lang::Cxx, src: CXX, compile: &["-O1", "-fPIC"], entry: None },
    Build { name: "f90-O2", lang: Lang::Fortran, src: F90, compile: &["-O2", "-g"], entry: None },
];

struct Tools {
    gcc: PathBuf,
    gxx: PathBuf,
    gfortran: PathBuf,
    clang: PathBuf,
    clangxx: PathBuf,
    llvm_mc: PathBuf,
    /// A directory holding `ld.lld`, for `-B`.
    lld_dir: PathBuf,
    mold: bool,
}

fn tools() -> Option<Tools> {
    let lld_dir = LLVM_MAJORS
        .iter()
        .map(|m| PathBuf::from(format!("/usr/lib/llvm-{m}/bin")))
        .find(|d| d.join("ld.lld").is_file());
    let Some(lld_dir) = lld_dir else {
        tool("ld.lld")?;
        return None;
    };
    Some(Tools {
        gcc: tool("gcc")?,
        gxx: tool("g++")?,
        gfortran: tool("gfortran")?,
        clang: tool("clang")?,
        clangxx: tool("clang++")?,
        llvm_mc: tool("llvm-mc")?,
        lld_dir,
        mold: tool("mold").is_some(),
    })
}

fn must(what: &str, o: &mortar_testutil::Outcome) {
    assert!(o.success(), "{what} failed: {}", o.stderr_text());
}

/// The objects of one build: one per compiler that reads the language.
fn objects(t: &Tools, dir: &Scratch, b: &Build) -> Vec<(String, PathBuf)> {
    let ext = match b.lang {
        Lang::C => "c",
        Lang::Cxx => "cc",
        Lang::Fortran => "f90",
        Lang::Asm => "s",
    };
    let text = if b.lang == Lang::Asm { format!("{ASM_DEFS}{}", b.src) } else { b.src.to_string() };
    let src = dir.write(&format!("{}.{ext}", b.name), text);
    let compilers: Vec<(&str, &Path)> = match b.lang {
        Lang::C => vec![("gcc", &t.gcc), ("clang", &t.clang)],
        Lang::Cxx => vec![("g++", &t.gxx), ("clang++", &t.clangxx)],
        Lang::Fortran => vec![("gfortran", &t.gfortran)],
        Lang::Asm => vec![("as", &t.gcc), ("llvm-mc", &t.llvm_mc)],
    };
    let mut out = Vec::new();
    for (cc, path) in compilers {
        let obj = dir.join(format!("{}-{cc}.o", b.name));
        let mut args: Vec<String> = Vec::new();
        if cc == "llvm-mc" {
            args.extend(["-triple=aarch64-linux-gnu", "-filetype=obj"].map(String::from));
        } else {
            args.push("-c".into());
            if b.lang == Lang::Fortran {
                // The module file goes to the scratch directory, not the
                // working directory.
                args.push(format!("-J{}", dir.path().display()));
            }
            args.extend(b.compile.iter().map(|s| (*s).to_string()));
            if b.entry.is_some() {
                args.extend(["-ffreestanding", "-fno-stack-protector"].map(String::from));
            }
        }
        args.extend([src.display().to_string(), "-o".into(), obj.display().to_string()]);
        must(&format!("{cc} {}", b.name), &run(path, &args));
        out.push((format!("{}/{cc}", b.name), obj));
    }
    out
}

/// The linked image, or `None` when the reference linker refuses the input
/// (mold does not take every type), which is reported.
fn link(
    t: &Tools,
    dir: &Scratch,
    b: &Build,
    label: &str,
    obj: &Path,
    linker: &str,
) -> Option<PathBuf> {
    let driver = match b.lang {
        Lang::Cxx => &t.gxx,
        Lang::Fortran => &t.gfortran,
        _ => &t.gcc,
    };
    let exe = dir.join(format!("{}.{linker}", label.replace('/', "-")));
    let mut args: Vec<String> = vec!["-static".into(), "-no-pie".into()];
    match linker {
        "bfd" => args.push("-fuse-ld=bfd".into()),
        "lld" => args.extend(["-fuse-ld=lld".into(), format!("-B{}/", t.lld_dir.display())]),
        _ => args.push("-fuse-ld=mold".into()),
    }
    args.extend(["-Wl,--emit-relocs", "-Wl,--no-relax", "-Wl,--build-id=none"].map(String::from));
    if let Some(e) = b.entry {
        args.extend(["-nostdlib".into(), format!("-Wl,-e,{e}")]);
    }
    if b.lang == Lang::Asm {
        args.extend(
            ["-Wl,--defsym=small=0x1234", "-Wl,--defsym=neg=0xffffffffffffedcc"].map(String::from),
        );
    }
    args.extend([obj.display().to_string(), "-o".into(), exe.display().to_string()]);
    let o = run(driver, &args);
    assert!(!o.timed_out(), "{linker} {label} timed out");
    if !o.success() {
        let why = o.stderr_text();
        let why = why
            .lines()
            .find(|l| l.contains("relocation"))
            .unwrap_or_else(|| why.lines().next().unwrap_or(""));
        let why = why.rsplit_once(": ").map_or(why, |(_, w)| w);
        eprintln!("note: {linker} refuses {label}: {why}");
        return None;
    }
    Some(exe)
}

#[test]
fn s6_1_apply_against_reference_linkers() {
    let Some(t) = tools() else { return };
    let dir = Scratch::new("a64-diff");
    let mut linkers = vec!["bfd", "lld"];
    if t.mold {
        linkers.push("mold");
    }
    let mut all: BTreeMap<&str, Tallies> = BTreeMap::new();
    let mut mismatches = Vec::new();
    let mut by_place = BTreeMap::new();
    let units = ASM_UNITS.iter().map(|&(name, src)| Build {
        name,
        lang: Lang::Asm,
        src,
        compile: &[],
        entry: Some("_start"),
    });
    let builds: Vec<Build> = BUILDS.iter().map(|b| Build { ..*b }).chain(units).collect();
    for b in &builds {
        for (label, obj) in objects(&t, &dir, b) {
            let mut linked = 0;
            for &linker in &linkers {
                let Some(exe) = link(&t, &dir, b, &label, &obj, linker) else { continue };
                linked += 1;
                let img = Image::read(&exe);
                compare(
                    &format!("{linker} {label}"),
                    &img,
                    all.entry(linker).or_default(),
                    &mut mismatches,
                    &mut by_place,
                );
                std::fs::remove_file(&exe).unwrap();
            }
            assert!(linked > 0, "no reference linker links {label}");
        }
    }
    // The table: every M1 type, then every other type seen.
    let mut types: Vec<u32> =
        (0..=0xFFFF).filter(|&r| table::howto(r).is_some_and(|h| h.status == Status::M1)).collect();
    for tallies in all.values() {
        for &r in tallies.keys() {
            if !types.contains(&r) {
                types.push(r);
            }
        }
    }
    eprintln!(
        "{:<5} {:<34} {:>4} | {}",
        "type",
        "name",
        "st",
        linkers.iter().map(|l| format!("{l:^48}")).collect::<Vec<_>>().join(" | ")
    );
    eprintln!(
        "{:<45} | {}",
        "",
        linkers
            .iter()
            .map(|_| format!(
                "{:>6} {:>6} {:>6} {:>6} {:>6} {:>6} {:>6}",
                "ok", "mism", "relax", "ifunc", "refus", "doc", "emit"
            ))
            .collect::<Vec<_>>()
            .join(" | ")
    );
    let mut zero = Vec::new();
    for r in types {
        let name = mortar_elf::reloc::aarch64::name(r).unwrap_or("?");
        let st = table::howto(r).map_or("-".to_string(), |h| format!("{:?}", h.status));
        let cells: Vec<String> = linkers
            .iter()
            .map(|l| {
                let x = all.get(l).and_then(|m| m.get(&r)).copied().unwrap_or_default();
                format!(
                    "{:>6} {:>6} {:>6} {:>6} {:>6} {:>6} {:>6}",
                    x.ok, x.mismatch, x.relaxed, x.ifunc, x.refused, x.documented, x.emit
                )
            })
            .collect();
        let seen: u32 = all.values().filter_map(|m| m.get(&r)).map(|x| x.ok + x.mismatch).sum();
        if seen == 0 && table::howto(r).is_some_and(|h| h.status == Status::M1) {
            zero.push(name);
        }
        eprintln!("{r:<5} {name:<34} {st:>4} | {}", cells.join(" | "));
    }
    eprintln!("M1 types with no compared relocation: {zero:?}");
    for (k, n) in &by_place {
        eprintln!("mismatches: {n:>6} {k}");
    }
    for m in &mismatches {
        eprintln!("MISMATCH {m}");
    }
    for l in &linkers {
        let (n, eq) = [541, 542]
            .iter()
            .filter_map(|r| all[l].get(r))
            .fold((0, 0), |a, x| (a.0 + x.relaxed, a.1 + x.relaxed_eq));
        eprintln!("{l}: IE → LE relaxed {n}, of which R3 writes the same bytes {eq}");
    }
    // §7: static glibc's `_nl_current_LC_*` references (`uselocale.o`,
    // `setlocale.o`) reach every reference linker's output.
    for l in &linkers {
        let n: u32 = all[l].values().map(|x| x.weak_tls).sum();
        eprintln!("{l}: {n} records against a thread-local weak0");
        assert!(n > 0, "{l}: no thread-local weak0 record: the §7 differential is vacuous");
    }
    assert!(mismatches.is_empty(), "{} mismatches", mismatches.len());
    let _ = A64::M1;
}

// ---------------------------------------------------------------- R3 from the inputs

/// What R3 did to the IE records of one link's inputs (a64.md §8.3).
#[derive(Default, Debug)]
struct R3Tally {
    /// 542 records whose pair mortar relaxed and whose decoded result is
    /// `TPREL(S)` in the register the original `ldr` wrote.
    relaxed: u32,
    /// 542 records kept IE because the symbol has `TLSIE_NORELAX`; of
    /// them, those whose own `Rt ≠ Rn`.
    kept: u32,
    kept_rt_ne_rn: u32,
    /// 541 records checked (relaxed or kept, each with its own form).
    pages: u32,
    /// Sequences whose 542 has no 541 of the same symbol and register in
    /// its section (checked alone).
    unpaired: u32,
}

/// `TPREL(x)` as §12.2 writes it, derived here independently of
/// `mortar_arch::TlsAbi`.
fn tprel_12_2(x: u64, tls: mortar_arch::TlsBlock) -> u64 {
    let tcb = 16u64.next_multiple_of(tls.align.max(1));
    x - tls.start + tcb
}

/// R3 over every IE sequence of `obj` (a relocatable object): scan raises
/// the inhibitors, apply relaxes each record on its own, and the result is
/// decoded: `movz xA, #hi, lsl #16` at the `adrp`, `movk xT, #lo` at the
/// `ldr`, `A` the `adrp`'s `Rd` and `T` the `ldr`'s `Rt`; the pair must
/// leave `TPREL(S)` in `xT`, which holds only when `A = T` (the `ldr`'s
/// `Rn`). A symbol with `TLSIE_NORELAX` must keep both instructions
/// unrelaxed.
fn r3_object(label: &str, obj: &Image, tally: &mut R3Tally, bad: &mut Vec<String>) {
    use support::{Sink, raised, scan_all};
    const TLS: mortar_arch::TlsBlock =
        mortar_arch::TlsBlock { start: 0x42_0000, size: 0x8_0000, align: 64 };
    let syms = obj.symbols();
    // Each symbol's `S`: spread over the template, some near its end.
    let s_of = |i: u32| (TLS.start + u64::from(i.wrapping_mul(0x9E37) % 0x7_FF00)) & !7;
    let mut t = Tables::default();
    let mut sections = Vec::new();
    for rs in obj.sections.iter().filter(|s| s.kind == SHT_RELA && s.info != 0) {
        let target = &obj.sections[rs.info as usize];
        if target.flags & SHF_ALLOC == 0 {
            continue;
        }
        let relocs: Vec<Reloc> = obj
            .data(rs)
            .chunks_exact(24)
            .filter_map(|e| {
                let info = u64_at(e, 8);
                let r_type = (info & 0xFFFF_FFFF) as u32;
                let sym = (info >> 32) as u32;
                matches!(r_type, 541 | 542).then(|| Reloc {
                    r_offset: u64_at(e, 0),
                    key: key(sym + 1),
                    r_type,
                    addend: u64_at(e, 16) as i64,
                })
            })
            .collect();
        if relocs.is_empty() {
            continue;
        }
        for e in obj.data(rs).chunks_exact(24) {
            let info = u64_at(e, 8);
            if !matches!(info & 0xFFFF_FFFF, 541 | 542) {
                continue;
            }
            let i = (info >> 32) as u32;
            let k = key(i + 1);
            let sym = &syms[i as usize];
            let kind = if sym.shndx == 0 && sym.info >> 4 == 2 {
                TargetKind::Weak0
            } else {
                TargetKind::Local
            };
            let s = if kind == TargetKind::Weak0 { 0 } else { s_of(i) };
            t.tls_sym(k, s, kind);
            t.slots.insert((k, SlotKind::TpOff { negated: false }), 0x50_0000 + 8 * u64::from(i));
        }
        sections.push((target.clone(), relocs));
    }
    // Scan: the flags of every key, over all sections of the object.
    for (target, relocs) in &sections {
        let mut sink = Sink::default();
        let place = mortar_arch::PlaceFacts { alloc: true, writable: false };
        scan_all(
            M2,
            &t,
            mortar_arch::OutputKind::Static,
            place,
            obj.data(target),
            relocs,
            &mut sink,
        )
        .unwrap();
        assert!(sink.errors.is_empty(), "{label} {}: {:?}", target.name, sink.errors);
        let keys: Vec<_> = sink.raised.iter().map(|(k, _)| *k).collect();
        for k in keys {
            *t.flags.entry(k).or_default() |= raised(&sink, k);
        }
    }
    let addr = 0x40_0000;
    for (target, relocs) in &sections {
        let input = obj.data(target);
        let mut c = Case::new(&t, input, addr);
        c.arch = M2;
        c.tls = Some(TLS);
        let word_at = |b: &[u8], o: u64| u32_at(b, usize::try_from(o).unwrap());
        for (i, r) in relocs.iter().enumerate() {
            let sym = t.syms[&r.key];
            let tp = if sym.kind == TargetKind::Weak0 { 0 } else { tprel_12_2(sym.s, TLS) };
            let keep = t.flags.get(&r.key).is_some_and(|f| f.contains(DemandFlags::TLSIE_NORELAX));
            let w = word_at(input, r.r_offset);
            let out = match c.apply_at(relocs, i) {
                Ok(out) => word_at(&out, r.r_offset),
                Err(e) => {
                    bad.push(format!("{label} {}+{:#x}: {e:?}", target.name, r.r_offset));
                    continue;
                }
            };
            let fail = |what: &str| {
                format!(
                    "{label} {}+{:#x} type {}: {what}: input {w:#010x} output {out:#010x} T={tp:#x}",
                    target.name, r.r_offset, r.r_type
                )
            };
            if r.r_type == 541 {
                tally.pages += 1;
                let want = if keep {
                    // ADRP to the slot's page.
                    out & 0x9F00_001F == w & 0x9F00_001F
                } else {
                    out == 0xD2A0_0000 | ((tp >> 16) as u32 & 0xFFFF) << 5 | (w & 0x1F)
                };
                if !want {
                    bad.push(fail(if keep { "kept ADRP changed" } else { "not movz xRd" }));
                }
                continue;
            }
            let (rt, rn) = (w & 0x1F, (w >> 5) & 0x1F);
            if keep {
                tally.kept += 1;
                tally.kept_rt_ne_rn += u32::from(rt != rn);
                if out & !(0xFFF << 10) != w & !(0xFFF << 10) {
                    bad.push(fail("kept LDR changed"));
                }
                continue;
            }
            // Relaxed: the register the `ldr` wrote must get TPREL(S).
            if rt != rn {
                bad.push(fail("relaxed with Rt != Rn"));
                continue;
            }
            // The pair's `adrp`: same symbol, `Rd = Rn`.
            let page = relocs.iter().enumerate().find(|(_, p)| {
                p.r_type == 541 && p.key == r.key && word_at(input, p.r_offset) & 0x1F == rn
            });
            let hi = if let Some((j, p)) = page {
                let o = c.apply_at(relocs, j).unwrap();
                let m = word_at(&o, p.r_offset);
                // movz x<rn>, #imm, lsl #16: x<rn> = imm << 16.
                if m & 0xFFE0_001F != 0xD2A0_0000 | rn {
                    bad.push(fail("pair's adrp not movz into Rn"));
                    continue;
                }
                u64::from((m >> 5) & 0xFFFF) << 16
            } else {
                tally.unpaired += 1;
                tp & 0xFFFF_0000
            };
            // movk x<rt>, #imm: replaces bits [15:0] of x<rt> = x<rn>.
            if out & 0xFFE0_001F != 0xF280_0000 | rt {
                bad.push(fail("not movk xRt"));
                continue;
            }
            let x = hi & !0xFFFF | u64::from((out >> 5) & 0xFFFF);
            if x != tp {
                bad.push(fail(&format!("computes {x:#x}")));
                continue;
            }
            tally.relaxed += 1;
        }
    }
}

/// R4's window (§8.4) over one object: each GD sequence matching the
/// pattern (`adrp; add; bl __tls_get_addr; nop` with records 513, 514,
/// `CALL26` in order), and every other record whose window meets
/// `[o, o + 16)`, which the overlap check refuses.
fn r4_object(label: &str, obj: &Image, found: &mut [u32; 2], bad: &mut Vec<String>) {
    let syms = obj.symbols();
    for rs in obj.sections.iter().filter(|s| s.kind == SHT_RELA && s.info != 0) {
        let target = &obj.sections[rs.info as usize];
        let bytes = obj.data(target);
        let recs: Vec<(u64, u32, u32)> = obj
            .data(rs)
            .chunks_exact(24)
            .map(|e| {
                let info = u64_at(e, 8);
                (u64_at(e, 0), (info & 0xFFFF_FFFF) as u32, (info >> 32) as u32)
            })
            .collect();
        for (i, &(o, r_type, _)) in recs.iter().enumerate() {
            if r_type != 513 {
                continue;
            }
            let at = |j: usize, t: u32, off: u64| {
                recs.get(j).is_some_and(|&(x, y, _)| y == t && x == off)
            };
            let o12 = usize::try_from(o + 12).unwrap();
            let pattern = at(i + 1, 514, o + 4)
                && at(i + 2, 283, o + 8)
                && syms[recs[i + 2].2 as usize].name == "__tls_get_addr"
                && o12 + 4 <= bytes.len()
                && u32_at(bytes, o12) == 0xD503_201F;
            if !pattern {
                continue;
            }
            found[0] += 1;
            for (j, &(x, y, _)) in recs.iter().enumerate() {
                if (i..i + 3).contains(&j) {
                    continue;
                }
                let w = table::howto(y).map_or(4, |h| match h.field.width() {
                    0 => 4,
                    n => u64::from(n),
                });
                if x < o + 16 && x + w > o {
                    found[1] += 1;
                    bad.push(format!(
                        "{label} {}+{o:#x}: record {j} type {y} at {x:#x}",
                        target.name
                    ));
                }
            }
        }
    }
}

/// a64.md §8.3, R3 on the INPUT objects: every corpus object and every
/// member of the static libraries the corpus links (a superset of what the
/// reference links pulled in), checked by decoding; the byte equality with
/// the references' relaxed records is in `compare` (`relaxed_eq`). Also
/// §8.4: no ordinary input has a record in an R4 window.
#[test]
fn s8_3_r3_from_the_inputs() {
    let Some(t) = tools() else { return };
    let dir = Scratch::new("a64-r3");
    let mut tally = R3Tally::default();
    // R4 patterns, and records overlapping their windows.
    let mut r4 = [0u32; 2];
    let mut bad = Vec::new();
    let units = ASM_UNITS.iter().map(|&(name, src)| Build {
        name,
        lang: Lang::Asm,
        src,
        compile: &[],
        entry: Some("_start"),
    });
    let builds: Vec<Build> = BUILDS.iter().map(|b| Build { ..*b }).chain(units).collect();
    for b in &builds {
        for (label, obj) in objects(&t, &dir, b) {
            let img = Image::read(&obj);
            r3_object(&label, &img, &mut tally, &mut bad);
            r4_object(&label, &img, &mut r4, &mut bad);
        }
    }
    // GCC's traditional GD, which no default build emits: R4's pattern.
    let src = dir.write("trad.c", C_GENERAL);
    let obj = dir.join("trad.o");
    let args = ["-c", "-O2", "-fPIC", "-mtls-dialect=trad"].map(String::from);
    let args =
        args.into_iter().chain([src.display().to_string(), "-o".into(), obj.display().to_string()]);
    must("gcc -mtls-dialect=trad", &run(&t.gcc, args));
    let img = Image::read(&obj);
    r3_object("trad/gcc", &img, &mut tally, &mut bad);
    r4_object("trad/gcc", &img, &mut r4, &mut bad);
    let mut members = 0;
    for lib in ["libc.a", "libm.a", "libstdc++.a", "libgfortran.a", "libgcc.a", "libgcc_eh.a"] {
        let o = run(&t.gcc, [format!("-print-file-name={lib}")]);
        must("gcc -print-file-name", &o);
        let path = PathBuf::from(o.stdout_text().trim());
        if !path.is_absolute() {
            eprintln!("note: {lib} not found");
            continue;
        }
        let bytes = std::fs::read(&path).unwrap();
        for (name, m) in elf::ar_members(&bytes) {
            members += 1;
            let img = Image::parse(m);
            r3_object(&format!("{lib}({name})"), &img, &mut tally, &mut bad);
            r4_object(&format!("{lib}({name})"), &img, &mut r4, &mut bad);
        }
    }
    eprintln!("R3 over the inputs ({members} archive members): {tally:?}");
    eprintln!("R4 patterns {}, records overlapping their windows {}", r4[0], r4[1]);
    for b in bad.iter().take(20) {
        eprintln!("R3 BAD {b}");
    }
    assert!(bad.is_empty(), "{} R3/R4 failures", bad.len());
    assert!(tally.relaxed > 1000 && tally.kept_rt_ne_rn > 0, "{tally:?}");
    assert!(r4[0] > 0, "no R4 pattern in the inputs");
}
