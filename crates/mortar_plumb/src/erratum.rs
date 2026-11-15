//! Erratum 843419 by site (`spec/plumb.md` §5.10, from `spec/a64.md`
//! §11.3): the fixes are undone, the sites found on the pre-fix words
//! with a64.md §11.1's predicate, and each site must carry exactly one
//! fix whose meaning is the original instruction's.
//!
//! The meaning of each fix is judged where the words are judged: a patch's
//! first word is the relocated final word (R1 on its record, or S12 when
//! it has none), it returns to `F + 4` (otherwise it is not followed and
//! the branch at `F` is R1/S12); an ADR over an ADRP computes the page the
//! relocated ADRP computes (W7 in `reloc.rs`; S12 for an ADRP without a
//! record, `content.rs`). This module judges that the fixes are where the
//! sites are: C1-C6.

use std::collections::{BTreeMap, BTreeSet};

use mortar_elf::SymbolSection;
use mortar_elf::consts::{EM_AARCH64, SHF_ALLOC, SHF_EXECINSTR, SHT_NOBITS, STT_NOTYPE};

use crate::a64;
use crate::locate::Place;
use crate::map::{ChunkKind, Format, Map};
use crate::reloc::Engine;
use crate::report::{Finding, Rule};

/// The end of a C1 message on a site with a patched site before it: what
/// GNU ld's A18 looks like (§10.2).
pub(crate) const AFTER_PATCH: &str = "; a site before it is patched";

/// What a64.md §11.1 condition 5 makes of an input word.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Class {
    /// In a `$x` range.
    Code,
    /// In a `$d` range.
    Data,
    /// Before the first mapping symbol of a section that has none at 0.
    Unproved,
}

/// An input word in an erratum window.
#[derive(Debug, Clone, Copy)]
struct Word {
    input: u32,
    class: Class,
    obj: usize,
    sec: usize,
    offset: u64,
}

/// The mapping ranges of an input section: `(offset, is code)`, sorted.
fn mapping(e: &Engine<'_, '_>, oi: usize, si: usize) -> Vec<(u64, bool)> {
    let o = &e.objs[oi];
    let Some(t) = o.symtab else { return Vec::new() };
    let mut v: Vec<(u64, bool)> = t
        .iter()
        .filter(|(_, s)| s.sym_type() == STT_NOTYPE)
        .filter_map(|(i, s)| {
            let Ok(SymbolSection::Section(k)) = t.section_of(i, &s) else { return None };
            if k.0 as usize != si {
                return None;
            }
            let name = t.name(&s).ok()?;
            let code = name == b"$x" || name.starts_with(b"$x.");
            let data = name == b"$d" || name.starts_with(b"$d.");
            (code || data).then_some((s.st_value, code))
        })
        .collect();
    v.sort_unstable();
    v
}

fn class_at(ranges: &[(u64, bool)], off: u64) -> Class {
    match ranges.partition_point(|r| r.0 <= off) {
        0 => Class::Unproved,
        i if ranges[i - 1].1 => Class::Code,
        _ => Class::Data,
    }
}

/// a64.md §11.1 condition 2: `W2` is in the "Loads and Stores" group.
fn load_store(w: u32) -> bool {
    w & 0x0A00_0000 == 0x0800_0000
}

/// For a reference linker's output: whether GNU ld 2.44 and lld 22 both
/// fix a site with these words (a64.md §11.4: the predicate is a superset
/// of their union, so a reference is judged only where both agree). `W2`
/// is a single-register integer or SIMD&FP load or store (unsigned,
/// unscaled, pre/post-index or register offset) or a store pair, that does
/// not load `Xn`; `W3`, in the 4-word form, is not a branch.
fn both_fix(w2: u32, w3: Option<u32>, n: u32) -> bool {
    let single = w2 & 0x3B00_0000 == 0x3900_0000 || w2 & 0x3B00_0000 == 0x3800_0000;
    let store_pair = w2 & 0x3A00_0000 == 0x2800_0000 && w2 & (1 << 22) == 0;
    // A single-register load (opc != 00) writes Rt: GNU ld alone patches
    // a `W2` that writes `Xn` (§11.4).
    let loads_n = single && (w2 >> 22) & 3 != 0 && w2 & 31 == n;
    let branch = |w: u32| {
        w & 0x7C00_0000 == 0x1400_0000 // B, BL
            || w & 0xFF00_0010 == 0x5400_0000 // B.cond
            || w & 0x7E00_0000 == 0x3400_0000 // CBZ, CBNZ
            || w & 0x7E00_0000 == 0x3600_0000 // TBZ, TBNZ
            || w & 0xFE1F_FC1F == 0xD61F_0000 // BR, BLR, RET (unpacked)
    };
    (single || store_pair) && !loads_n && !w3.is_some_and(branch)
}

/// The link's `--fix-cortex-a53-843419`, as the caller knows it, or
/// inferred from the fixes found (§5.10).
#[allow(clippy::too_many_lines)] // §11.3's steps in order
pub(crate) fn check(e: &mut Engine<'_, '_>, map: &Map) {
    if e.image.header.e_machine != EM_AARCH64 {
        return;
    }
    let mortar = map.format == Format::Mortar;
    // The input words in [P + 0xFF8, P + 0x100C) of every page: the only
    // words the predicate reads (a64.md §11.1). A window may start in the
    // section before; each word is taken from the section that holds it.
    let mut words: BTreeMap<u64, Word> = BTreeMap::new();
    for (oi, o) in e.objs.iter().enumerate() {
        for (si, s) in o.secs.iter().enumerate() {
            let Place::Whole(addr) = s.place else { continue };
            if s.shdr.sh_flags & (SHF_ALLOC | SHF_EXECINSTR) != SHF_ALLOC | SHF_EXECINSTR
                || s.shdr.sh_type == SHT_NOBITS
                || s.data.is_empty()
            {
                continue;
            }
            let len = s.data.len() as u64;
            let mut ranges: Option<Vec<(u64, bool)>> = None;
            let mut page = (addr & !0xFFF).saturating_sub(0x1000);
            while page.saturating_add(0xFF8) < addr.saturating_add(len) {
                let lo = page + 0xFF8;
                let hi = page + 0x100C;
                let mut a = lo.max(addr);
                // Word-aligned in the output: words the processor reads.
                a = (a + 3) & !3;
                while a + 4 <= hi.min(addr + len) {
                    let off = a - addr;
                    let r = ranges.get_or_insert_with(|| mapping(e, oi, si));
                    let b = &s.data[off as usize..off as usize + 4];
                    let input = u32::from_le_bytes([b[0], b[1], b[2], b[3]]);
                    words.insert(
                        a,
                        Word { input, class: class_at(r, off), obj: oi, sec: si, offset: off },
                    );
                    a += 4;
                }
                page += 0x1000;
            }
        }
    }
    // 1. Undo the fixes (a64.md §11.3 step 1).
    let mut pre: BTreeMap<u64, u32> = BTreeMap::new();
    let mut redirects: BTreeSet<u64> = BTreeSet::new();
    let mut adr_fixes: BTreeSet<u64> = BTreeSet::new();
    for (&a, w) in &words {
        let Some(out) = e.image.insn(a) else { continue };
        let mut word = out;
        if w.class == Class::Code {
            if a64::erratum_843419_site(a)
                && a64::is_ldst_unsigned_imm(w.input)
                && let Some(moved) = e.erratum_patch(a, out)
            {
                word = moved;
                redirects.insert(a);
            } else if a64::erratum_843419_adrp(a)
                && let (Some((d, _)), Some((rd, _))) = (a64::adr(out, a), a64::adrp(w.input, a))
                && d == rd
            {
                word = w.input;
                adr_fixes.insert(a);
            }
        }
        pre.insert(a, word);
    }
    let fix = map.fix_843419.or((!redirects.is_empty() || !adr_fixes.is_empty()).then_some(true));
    // 2. The sites, on the pre-fix words (§11.1).
    let mut site_starts: BTreeSet<u64> = BTreeSet::new();
    let mut site_ends: BTreeSet<u64> = BTreeSet::new();
    let mut findings = Vec::new();
    for (&q, &w1) in &pre {
        let Some((n, _)) = a64::erratum_843419_adrp(q).then(|| a64::adrp(w1, q)).flatten() else {
            continue;
        };
        let Some(&w2) = pre.get(&(q + 4)) else { continue };
        if !load_store(w2) {
            continue;
        }
        for f in [q + 8, q + 12] {
            let Some(&wf) = pre.get(&f) else { continue };
            if !a64::is_ldst_unsigned_imm(wf) || (wf >> 5) & 31 != n {
                continue;
            }
            // Condition 5: every word is input code (a word outside every
            // input section is padding or a synthetic chunk: §11.1 1-3).
            let span: Vec<Option<&Word>> =
                (0..=(f - q) / 4).map(|i| words.get(&(q + 4 * i))).collect();
            if span.iter().any(|w| w.is_none_or(|w| w.class == Class::Data)) {
                continue;
            }
            let w = words[&q];
            if span.iter().any(|w| w.is_some_and(|w| w.class == Class::Unproved)) {
                if mortar {
                    findings.push(
                        Finding::new(
                            Rule::Erratum(4),
                            "an erratum 843419 candidate in code without a mapping symbol \
                             (mortar must refuse it, spec/a64.md §11.1 condition 5)",
                        )
                        .at(q)
                        .site(e.objs[w.obj].site(w.sec, w.offset)),
                    );
                } else {
                    e.cov.erratum_not_judged += 1;
                }
                continue;
            }
            site_starts.insert(q);
            site_ends.insert(f);
            let (patched, adr) = (redirects.contains(&f), adr_fixes.contains(&q));
            if patched
                && let Some(out) = e.image.insn(f)
                && let Some((false, t)) = a64::branch(out, f)
            {
                e.patch_stubs.insert(t, f);
            }
            let site = e.objs[w.obj].site(w.sec, w.offset);
            if patched && adr {
                findings.push(
                    Finding::new(
                        Rule::Erratum(2),
                        "an erratum 843419 site has both fixes (a64.md §11.3 item 5)",
                    )
                    .at(q)
                    .site(site),
                );
            } else if patched || adr {
                e.cov.erratum_sites += 1;
            } else if fix != Some(true) {
                e.cov.erratum_not_judged += 1;
            } else if !mortar && !both_fix(w2, (f == q + 12).then(|| pre[&(q + 8)]), n) {
                // Beyond what both references fix (a64.md §11.4).
                e.cov.erratum_not_judged += 1;
            } else {
                let after = if redirects.range(..f).next().is_some() { AFTER_PATCH } else { "" };
                let mut x = Finding::new(
                    Rule::Erratum(1),
                    format!(
                        "an erratum 843419 site (ADRP at {q:#x}, final word at {f:#x}) is not fixed \
                         (spec/a64.md §11.2){after}"
                    ),
                )
                .at(f)
                .site(site);
                x.found = Some(u64::from(wf));
                findings.push(x);
            }
        }
    }
    if mortar {
        // 4. Every redirect names a site; 5. no ADR fix outside a site.
        for &f in redirects.difference(&site_ends) {
            let w = words[&f];
            findings.push(
                Finding::new(
                    Rule::Erratum(3),
                    "a branch to an erratum 843419 patch at a word that is not a site's final word \
                     (a64.md §11.3 item 4)",
                )
                .at(f)
                .site(e.objs[w.obj].site(w.sec, w.offset)),
            );
        }
        for &q in adr_fixes.difference(&site_starts) {
            let w = words[&q];
            findings.push(
                Finding::new(
                    Rule::Erratum(2),
                    "an ADRP made an ADR at a word that starts no erratum 843419 site \
                     (a64.md §11.3 item 5)",
                )
                .at(q)
                .site(e.objs[w.obj].site(w.sec, w.offset)),
            );
        }
        if map.fix_843419 == Some(false) {
            for &a in redirects.union(&adr_fixes) {
                findings.push(
                    Finding::new(
                        Rule::Erratum(6),
                        "an erratum 843419 fix in a link without --fix-cortex-a53-843419 (a64.md §11)",
                    )
                    .at(a),
                );
            }
        }
        // 8. No synthetic code chunk starts at 0xFFC, 0x000, 0x004 or 0x008
        // of a page (a64.md §11.1, layout.md L-118).
        if fix == Some(true) {
            for c in &map.chunks {
                let Some(a) = c.addr else { continue };
                if c.size == 0 || !matches!(a & 0xFFF, 0xFFC | 0x000 | 0x004 | 0x008) {
                    continue;
                }
                let exec = e
                    .image
                    .alloc_section_of(a, 1)
                    .is_some_and(|(_, s)| s.shdr.sh_flags & SHF_EXECINSTR != 0);
                if exec && c.kind != ChunkKind::Common {
                    findings.push(
                        Finding::new(
                            Rule::Erratum(5),
                            format!(
                                "the code chunk '{}' starts at page offset {:#x} \
                                 (spec/layout.md L-118, a64.md §11.1)",
                                c.name,
                                a & 0xFFF
                            ),
                        )
                        .at(a),
                    );
                }
            }
        }
    }
    e.findings.extend(findings);
}

#[cfg(test)]
mod tests {
    use super::*;

    // spec: a64.md §11.1 condition 2 (the "Loads and Stores" group).
    #[test]
    fn s5_10_load_store_group() {
        assert!(load_store(0xf940_0442)); // ldr x2, [x2, #8]
        assert!(load_store(0xa9bf_7bfd)); // stp x29, x30, [sp, #-16]!
        assert!(!load_store(0xd503_201f)); // nop
        assert!(!load_store(0x9100_0400)); // add x0, x0, #1
    }

    // spec: a64.md §11.4 (what both references fix).
    #[test]
    fn s5_10_both_references_fix() {
        let n = 0;
        assert!(both_fix(0xf940_03e2, None, n)); // ldr x2, [sp]
        assert!(!both_fix(0xf940_0400, None, n)); // ldr x0, [x0, #8]: writes Xn
        assert!(both_fix(0xf900_0400, None, n)); // str x0, [x0, #8]
        assert!(!both_fix(0xa940_07e0, None, n)); // ldp: neither
        assert!(both_fix(0xa900_07e0, None, n)); // stp
        assert!(!both_fix(0xf940_03e2, Some(0x1400_0002), n)); // W3 a branch
        assert!(both_fix(0xf940_03e2, Some(0xd503_201f), n)); // W3 a nop
    }

    // spec: a64.md §11.1 condition 5.
    #[test]
    fn s5_10_mapping_classes() {
        let r = [(4, true), (16, false), (24, true)];
        assert_eq!(class_at(&r, 0), Class::Unproved);
        assert_eq!(class_at(&r, 4), Class::Code);
        assert_eq!(class_at(&r, 20), Class::Data);
        assert_eq!(class_at(&r, 24), Class::Code);
        assert_eq!(class_at(&[], 0), Class::Unproved);
    }
}
