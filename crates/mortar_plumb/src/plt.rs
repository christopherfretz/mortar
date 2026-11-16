//! P-D10's PLT header and the way back to it (`spec/plumb.md` §12.2,
//! WP-082): every PLT entry whose word carries a `JUMP_SLOT` is executed
//! symbolically, first to the jump through its own word, then from that
//! word's initial content (the lazy target) to the jump into the loader's
//! resolver. What the resolver receives is compared with what glibc's
//! trampolines read (`sysdeps/x86_64/dl-trampoline.h`,
//! `sysdeps/aarch64/dl-trampoline.S`):
//!
//! - x86-64: the stack holds `[.got.plt[1], n]` (the link map on top, the
//!   index of the entry's record in `DT_JMPREL` below it) and the jump goes
//!   through `.got.plt[2]`, `.got.plt` being `DT_PLTGOT`'s address. GNU ld
//!   and lld push `n` in the entry (`push $n; jmp PLT0`), mold sets `%r11d`
//!   in the entry and PLT0 pushes `%r11`; the IBT shapes (`.plt.sec`
//!   entries whose lazy word is a `.plt` stub `endbr64; push $n; jmp
//!   PLT0`) are the same path.
//! - AArch64: the header pushes `x16` (the entry's `x16`, from which the
//!   resolver derives the index: record `(x16 - .got.plt[3]) / 8`), loads
//!   `x17` from `.got.plt[2]` and leaves `x16 = &.got.plt[2]` (the
//!   resolver reads the link map at `x16 - 8`).
//!
//! The words the loader fills, `.got.plt[1]` and `[2]`, are zero in the
//! file and no record writes them; `.got.plt[0]` holds `_DYNAMIC` on
//! x86-64 (all three references, D-32) and 0 or `_DYNAMIC` on AArch64
//! (GNU ld and lld 0, mold `_DYNAMIC`; mortar 0, `spec/a64.md` §10.2).
//! The check runs under `-z now` too: the references write the same
//! shapes and words there, and so must mortar (D-49).

use std::collections::{BTreeMap, BTreeSet};

use mortar_elf::consts::{EM_AARCH64, EM_X86_64, PF_X};

use crate::dynout::{DT_AARCH64_BTI_PLT, DT_PLTGOT, DynOut};
use crate::image::Image;
use crate::loader::{Kind, kind};
use crate::map::Format;
use crate::report::{Finding, Rule};

fn fd(msg: impl Into<String>, at: u64) -> Finding {
    Finding::new(Rule::Dyn(10), msg).at(at)
}

/// A value the lazy path pushes, with the address of the instruction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Push {
    /// An immediate or a register holding a known number.
    Imm(u64),
    /// The word at this address (x86-64 `push *disp(%rip)`).
    Word(u64),
}

/// What one instruction of a walk did (the mutation classes find their
/// sites by it).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Step {
    /// x86-64 `push $n`.
    PushImm,
    /// x86-64 `mov $n, %r11d`.
    MovR11,
    /// x86-64 `push *disp(%rip)`.
    PushWord,
    /// x86-64 `jmp rel32`.
    Jmp,
    /// x86-64 `jmp *disp(%rip)`; AArch64 `br x17`.
    JmpInd,
    /// AArch64 `adrp x16`.
    Adrp,
    /// AArch64 `ldr x17, [x16, #lo]`.
    Ldr,
    /// AArch64 `add x16, x16, #lo`.
    Add,
}

/// The end of one walk: the lazy target, what the lazy part pushed and
/// the word its last indirect jump goes through.
#[derive(Debug)]
struct Walk {
    lazy: u64,
    pushes: Vec<(Push, u64)>,
    through: u64,
    /// The address of the jump through `through`.
    jump_at: u64,
    /// AArch64: `x16` when the entry jumps through its word, and when the
    /// header jumps to the resolver.
    x16_entry: Option<u64>,
    x16_end: Option<u64>,
    /// Each instruction that matters: what, where, and whether it ran
    /// after the jump through the entry's word.
    trace: Vec<(Step, u64, bool)>,
}

fn executable<'a>(image: &Image<'a>, addr: u64, len: u64) -> Option<&'a [u8]> {
    let p = image.load_of(addr, len)?;
    if p.p_flags & PF_X == 0 {
        return None;
    }
    image.bytes(addr, len)
}

fn rel32(b: &[u8], i: usize) -> i64 {
    i64::from(i32::from_le_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]]))
}

/// Instructions the walk follows at most (the longest reference path is
/// seven).
const STEPS: usize = 24;

/// Up to 7 executable bytes at `at` (fewer at the end of a segment).
fn fetch<'a>(image: &Image<'a>, at: u64) -> Option<&'a [u8]> {
    (1..=7u64).rev().find_map(|n| {
        let p = image.load_of(at, n)?;
        if p.p_flags & PF_X == 0 {
            return None;
        }
        image.bytes(at, n)
    })
}

/// x86-64: the entry at `entry` run to the jump through `slot`, then from
/// the slot's initial word to the next indirect jump.
fn x64_walk(image: &Image<'_>, entry: u64, slot: u64) -> Result<Walk, (String, u64)> {
    x64_walk_depths(image, entry, slot, &mut Vec::new())
}

/// [`x64_walk`], recording in `depths` each instruction it reaches with
/// the bytes the path has pushed above the caller's stack pointer,
/// counting the return address (the CFA is `%rsp` + that at the
/// instruction): 8 at the entry, 8 more per `push`. Kept on an error too:
/// what was reached before it.
fn x64_walk_depths(
    image: &Image<'_>,
    entry: u64,
    slot: u64,
    depths: &mut Vec<(u64, u64)>,
) -> Result<Walk, (String, u64)> {
    let mut depth = 8u64;
    let mut pc = entry;
    let mut r11: Option<u64> = None;
    let mut pushes = Vec::new();
    let mut lazy: Option<u64> = None;
    let mut trace = Vec::new();
    let early = |at: u64| {
        (format!("the PLT entry at {entry:#x} pushes before it jumps through its word"), at)
    };
    for _ in 0..STEPS {
        let at = pc;
        let b = fetch(image, at).ok_or_else(|| {
            (format!("the lazy path reaches {at:#x}, outside executable bytes"), at)
        })?;
        depths.push((at, depth));
        let unknown =
            || (format!("the lazy path reaches an instruction it does not know at {at:#x}"), at);
        // A `bnd` prefix (MPX) before a jump is ignored.
        let o = usize::from(b[0] == 0xf2);
        let c = &b[o..];
        let next = |len: usize| at + (o + len) as u64;
        match c {
            [0xf3, 0x0f, 0x1e, 0xfa, ..] if o == 0 => pc = next(4),
            [0x41, 0xbb, _, _, _, _, ..] if o == 0 => {
                r11 = Some(rel32(c, 2) as u64 & 0xffff_ffff);
                trace.push((Step::MovR11, at, lazy.is_some()));
                pc = next(6);
            }
            [0x68, _, _, _, _, ..] if o == 0 => {
                if lazy.is_none() {
                    return Err(early(at));
                }
                depth += 8;
                pushes.push((Push::Imm(rel32(c, 1) as u64), at));
                trace.push((Step::PushImm, at, true));
                pc = next(5);
            }
            [0x41, 0x53, ..] if o == 0 => {
                let Some(v) = r11 else {
                    return Err((
                        format!("the lazy path pushes %r11 at {at:#x}, which nothing set"),
                        at,
                    ));
                };
                if lazy.is_none() {
                    return Err(early(at));
                }
                depth += 8;
                pushes.push((Push::Imm(v), at));
                pc = next(2);
            }
            [0xff, 0x35, _, _, _, _, ..] if o == 0 => {
                if lazy.is_none() {
                    return Err(early(at));
                }
                depth += 8;
                pushes.push((Push::Word(next(6).wrapping_add_signed(rel32(c, 2))), at));
                trace.push((Step::PushWord, at + o as u64, true));
                pc = next(6);
            }
            [0xe9, _, _, _, _, ..] => {
                trace.push((Step::Jmp, at + o as u64, lazy.is_some()));
                pc = next(5).wrapping_add_signed(rel32(c, 1));
            }
            [0xeb, d, ..] if o == 0 => pc = next(2).wrapping_add_signed(i64::from(*d as i8)),
            [0xff, 0x25, _, _, _, _, ..] => {
                let word = next(6).wrapping_add_signed(rel32(c, 2));
                trace.push((Step::JmpInd, at + o as u64, lazy.is_some()));
                let Some(l) = lazy else {
                    if word != slot {
                        return Err((
                            format!(
                                "the PLT entry at {entry:#x} jumps through {word:#x}, not its word {slot:#x}"
                            ),
                            at,
                        ));
                    }
                    let l = image.value(slot, 8).ok_or_else(|| {
                        (format!("the JUMP_SLOT word {slot:#x} is not in the file"), slot)
                    })?;
                    lazy = Some(l);
                    pc = l;
                    continue;
                };
                return Ok(Walk {
                    lazy: l,
                    pushes,
                    through: word,
                    jump_at: at,
                    x16_entry: None,
                    x16_end: None,
                    trace,
                });
            }
            _ => return Err(unknown()),
        }
    }
    Err((
        format!("the lazy path of the PLT entry at {entry:#x} does not reach an indirect jump"),
        entry,
    ))
}

/// AArch64: as [`x64_walk`], with `x16` and `x17` followed.
fn a64_walk(image: &Image<'_>, entry: u64, slot: u64) -> Result<Walk, (String, u64)> {
    let mut pc = entry;
    let mut x16: Option<u64> = None;
    let mut x17: Option<u64> = None;
    let mut pushes = Vec::new();
    let mut lazy: Option<u64> = None;
    let mut x16_entry = None;
    let mut trace = Vec::new();
    for _ in 0..STEPS {
        let at = pc;
        let unknown =
            || (format!("the lazy path reaches an instruction it does not know at {at:#x}"), at);
        let insn = executable(image, at, 4)
            .filter(|_| at.is_multiple_of(4))
            .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
            .ok_or_else(|| {
                (format!("the lazy path reaches {at:#x}, outside executable bytes"), at)
            })?;
        pc = at + 4;
        if insn == 0xd503_201f || crate::a64::bti(insn) {
            continue;
        }
        if insn == 0xa9bf_7bf0 {
            // stp x16, x30, [sp, #-16]!
            let Some(v) = x16 else { return Err(unknown()) };
            if lazy.is_none() {
                return Err((
                    format!("the PLT entry at {entry:#x} pushes before it jumps through its word"),
                    at,
                ));
            }
            pushes.push((Push::Imm(v), at));
            continue;
        }
        if let Some((16, page)) = crate::a64::adrp(insn, at) {
            x16 = Some(page);
            trace.push((Step::Adrp, at, lazy.is_some()));
            continue;
        }
        if let Some((17, 16, off)) = crate::a64::ldr64_imm(insn) {
            x17 = Some(x16.ok_or_else(unknown)?.wrapping_add(off));
            trace.push((Step::Ldr, at, lazy.is_some()));
            continue;
        }
        if let Some((16, 16, imm)) = crate::a64::add_imm(insn) {
            x16 = Some(x16.ok_or_else(unknown)?.wrapping_add(imm));
            trace.push((Step::Add, at, lazy.is_some()));
            continue;
        }
        if insn == 0xd61f_0220 {
            // br x17
            let word = x17.ok_or_else(unknown)?;
            trace.push((Step::JmpInd, at, lazy.is_some()));
            if lazy.is_none() {
                if word != slot {
                    return Err((
                        format!(
                            "the PLT entry at {entry:#x} jumps through {word:#x}, not its word {slot:#x}"
                        ),
                        at,
                    ));
                }
                let l = image.value(slot, 8).ok_or_else(|| {
                    (format!("the JUMP_SLOT word {slot:#x} is not in the file"), slot)
                })?;
                lazy = Some(l);
                x16_entry = x16;
                x17 = None;
                pc = l;
                continue;
            }
            return Ok(Walk {
                lazy: lazy.unwrap_or(0),
                pushes,
                through: word,
                jump_at: at,
                x16_entry,
                x16_end: x16,
                trace,
            });
        }
        return Err(unknown());
    }
    Err((
        format!("the lazy path of the PLT entry at {entry:#x} does not reach an indirect jump"),
        entry,
    ))
}

/// P-D10's header part: `.got.plt`'s reserved words and every lazy path.
// One pass over the words, then one per entry; ELF fields read best by
// the psABIs' short names.
#[allow(clippy::too_many_lines, clippy::many_single_char_names)]
pub(crate) fn check(
    image: &Image<'_>,
    out: &DynOut,
    entries: &BTreeMap<u64, u64>,
    format: Format,
    v: &mut Vec<Finding>,
) {
    let machine = image.header.e_machine;
    if machine != EM_X86_64 && machine != EM_AARCH64 {
        return;
    }
    let slots: Vec<(u64, u64)> = out
        .rels
        .iter()
        .filter(|r| r.plt)
        .enumerate()
        .filter(|(_, r)| kind(machine, r.ty) == Kind::JumpSlot)
        .map(|(k, r)| (k as u64, r.offset))
        .collect();
    if slots.is_empty() {
        return;
    }
    let Some(got) = out.get(DT_PLTGOT) else {
        v.push(fd(
            "JUMP_SLOT records and no DT_PLTGOT: the loader cannot fill the resolver's words",
            0,
        ));
        return;
    };
    let mut seen: BTreeSet<(u64, String)> = BTreeSet::new();
    let mut push = |f: Finding, v: &mut Vec<Finding>| {
        if seen.insert((f.addr.unwrap_or(0), f.message.clone())) {
            v.push(f);
        }
    };
    // The reserved words.
    let dynamic = out.dynamic.map(|d| d.0);
    match image.value(got, 8) {
        None => push(fd(format!("DT_PLTGOT {got:#x} is not in the output's memory"), got), v),
        Some(w0) => {
            let ok = if machine == EM_X86_64 {
                Some(w0) == dynamic
            } else if format == Format::Mortar {
                w0 == 0
            } else {
                w0 == 0 || Some(w0) == dynamic
            };
            if !ok {
                let want = if machine == EM_X86_64 {
                    "_DYNAMIC's address (D-32)"
                } else if format == Format::Mortar {
                    "0 (spec/a64.md §10.2)"
                } else {
                    "0 or _DYNAMIC's address"
                };
                let mut f = fd(format!(".got.plt[0] holds {w0:#x}, not {want}"), got);
                f.found = Some(w0);
                push(f, v);
            }
        }
    }
    for i in 1..3u64 {
        let a = got + 8 * i;
        if image.value(a, 8).is_some_and(|w| w != 0) {
            push(fd(format!(".got.plt[{i}] is not zero in the file: the loader fills it"), a), v);
        }
    }
    for r in &out.rels {
        if r.offset < got + 24 && r.offset + 8 > got + 8 {
            push(
                fd(
                    format!(
                        "a dynamic record writes {:#x}, a word of .got.plt[1..2] the loader fills for the resolver",
                        r.offset
                    ),
                    r.offset,
                ),
                v,
            );
        }
    }
    // Every lazy path.
    let bti = machine == EM_AARCH64
        && (out.has(DT_AARCH64_BTI_PLT) || crate::property::aarch64_feature_1_and(image) & 1 != 0);
    let ibt = machine == EM_X86_64 && crate::property::x86_feature_1_and(image) & 1 != 0;
    let by_slot: BTreeMap<u64, u64> = slots.iter().map(|&(k, s)| (s, k)).collect();
    let in_plt = |e: u64| {
        image.sections.iter().any(|s| {
            matches!(s.name.as_str(), ".plt" | ".plt.sec")
                && (s.shdr.sh_addr..s.shdr.sh_addr + s.shdr.sh_size).contains(&e)
        })
    };
    for (&e, &slot) in entries {
        let Some(&k) = by_slot.get(&slot) else { continue };
        // One entry decoded twice (AArch64 `bti c` and the `adrp` after it).
        if e >= 4 && entries.get(&(e - 4)) == Some(&slot) {
            continue;
        }
        if !in_plt(e) {
            continue;
        }
        let walk =
            if machine == EM_X86_64 { x64_walk(image, e, slot) } else { a64_walk(image, e, slot) };
        let w = match walk {
            Ok(w) => w,
            Err((m, at)) => {
                push(fd(format!("{m} (P-D10: the way back to the PLT header)"), at), v);
                continue;
            }
        };
        let want = got + 16;
        if w.through != want {
            push(
                fd(
                    format!(
                        "the PLT header jumps through {:#x}, not .got.plt[2] ({want:#x}, DT_PLTGOT + 16): every lazy call goes astray",
                        w.through
                    ),
                    w.jump_at,
                ),
                v,
            );
        }
        if machine == EM_X86_64 {
            if ibt && executable(image, w.lazy, 4) != Some(&[0xf3, 0x0f, 0x1e, 0xfa][..]) {
                push(
                    fd(
                        format!(
                            "an IBT output's lazy target {:#x} (word {slot:#x}) is not an endbr64",
                            w.lazy
                        ),
                        slot,
                    ),
                    v,
                );
            }
            let link = w.pushes.iter().position(|(p, _)| matches!(p, Push::Word(_)));
            match (w.pushes.as_slice(), link) {
                ([(Push::Imm(n), at), (Push::Word(m), mat)], Some(1)) => {
                    if *n != k {
                        let mut f = fd(
                            format!(
                                "the lazy path of the PLT entry at {e:#x} gives the resolver index {n}; its JUMP_SLOT ({slot:#x}) is record {k} of DT_JMPREL"
                            ),
                            *at,
                        );
                        f.found = Some(*n);
                        f.expected = Some(k);
                        push(f, v);
                    }
                    if *m != got + 8 {
                        push(
                            fd(
                                format!(
                                    "the PLT header pushes the word at {m:#x}, not .got.plt[1] ({:#x}): the resolver gets no link map",
                                    got + 8
                                ),
                                *mat,
                            ),
                            v,
                        );
                    }
                }
                _ => push(
                    fd(
                        format!(
                            "the lazy path of the PLT entry at {e:#x} pushes {} values; the resolver takes the index, then .got.plt[1]",
                            w.pushes.len()
                        ),
                        w.jump_at,
                    ),
                    v,
                ),
            }
        } else {
            if bti && !image.insn(w.lazy).is_some_and(crate::a64::landing_pad) {
                push(
                    fd(
                        format!(
                            "a BTI output's lazy target {:#x} (word {slot:#x}) is not a landing pad for br x17",
                            w.lazy
                        ),
                        slot,
                    ),
                    v,
                );
            }
            if w.x16_end != Some(want) {
                push(
                    fd(
                        format!(
                            "the PLT header leaves x16 = {}; the resolver reads the link map at x16 - 8 and needs x16 = .got.plt[2] ({want:#x})",
                            w.x16_end.map_or("unknown".into(), |x| format!("{x:#x}"))
                        ),
                        w.jump_at,
                    ),
                    v,
                );
            }
            match w.pushes.as_slice() {
                [(Push::Imm(p), at)] => {
                    let expect = got + 24 + 8 * k;
                    if *p != expect || w.x16_entry != Some(expect) {
                        let mut f = fd(
                            format!(
                                "the PLT entry at {e:#x} hands the resolver x16 = {p:#x}, which names record {} of DT_JMPREL; its JUMP_SLOT ({slot:#x}) is record {k} (x16 must be {expect:#x})",
                                (p.wrapping_sub(got + 24) as i64) / 8
                            ),
                            *at,
                        );
                        f.found = Some(*p);
                        f.expected = Some(expect);
                        push(f, v);
                    }
                }
                _ => push(
                    fd(
                        format!(
                            "the lazy path of the PLT entry at {e:#x} pushes {} pairs; the header pushes x16 and x30 once",
                            w.pushes.len()
                        ),
                        w.jump_at,
                    ),
                    v,
                ),
            }
        }
    }
}

/// A walk's steps: what, at which address, whether in the entry (before
/// its own slot's jump) or on the lazy path.
pub(crate) type Trace = Vec<(Step, u64, bool)>;

/// The PLT section of the output `pc` is in: its name, start and end.
pub(crate) fn in_plt<'a>(image: &'a Image<'_>, pc: u64) -> Option<(&'a str, u64, u64)> {
    image.sections.iter().find_map(|s| {
        let (lo, hi) = (s.shdr.sh_addr, s.shdr.sh_addr.saturating_add(s.shdr.sh_size));
        (matches!(s.name.as_str(), ".plt" | ".plt.got" | ".plt.sec" | ".iplt")
            && (lo..hi).contains(&pc))
        .then_some((s.name.as_str(), lo, hi))
    })
}

/// x86-64: the stack depth (CFA - `%rsp`, see [`x64_walk_depths`]) at
/// each instruction of a PLT section that a PLT entry or its lazy path
/// executes, the entries being [`crate::loader::plt_entries`]'s and the
/// lazy target each entry's word holds in the file (E6, `spec/plumb.md`
/// §6.4): `image` is the file's, not a loaded view's. An instruction reached at two depths keeps the first (the PLT
/// is then P-D10's finding). Padding no path reaches is not in the map.
pub(crate) fn x64_depths(image: &Image<'_>) -> BTreeMap<u64, u64> {
    let entries = crate::loader::plt_entries(image);
    let mut out = BTreeMap::new();
    let mut v = Vec::new();
    for (&e, &slot) in &entries {
        v.clear();
        let _ = x64_walk_depths(image, e, slot, &mut v);
        for &(pc, d) in &v {
            if in_plt(image, pc).is_some() {
                out.entry(pc).or_insert(d);
            }
        }
    }
    out
}

/// The traces of every lazy path that walks to an indirect jump: for the
/// mutation classes (`dynmutate.rs`). Entry address, its slot, the trace.
pub(crate) fn traces(image: &Image<'_>, out: &DynOut) -> Vec<(u64, u64, Trace)> {
    let machine = image.header.e_machine;
    let slots: BTreeSet<u64> = out
        .rels
        .iter()
        .filter(|r| r.plt && kind(machine, r.ty) == Kind::JumpSlot)
        .map(|r| r.offset)
        .collect();
    let entries = crate::loader::plt_entries(image);
    let mut v = Vec::new();
    for (&e, &slot) in &entries {
        if !slots.contains(&slot) || (e >= 4 && entries.get(&(e - 4)) == Some(&slot)) {
            continue;
        }
        let w = match machine {
            EM_X86_64 => x64_walk(image, e, slot),
            EM_AARCH64 => a64_walk(image, e, slot),
            _ => continue,
        };
        if let Ok(w) = w {
            v.push((e, slot, w.trace));
        }
    }
    v
}
