//! Thread-local storage in static executables (`spec/plumb.md` §4.7,
//! W9 to W13) and the review's miscompile cases (§10.2): programs built
//! with gcc and clang-22, linked by GNU ld, lld 22 and mold, run, and
//! verified.

mod common;

use std::collections::BTreeSet;

use common::{Linker, Size, assert_clean, corpus, tls_types_for, unexpected};
use mortar_plumb::{Report, Rewrite, Via};

fn ids(r: &Report) -> Vec<String> {
    r.findings.iter().map(|f| f.rule.id()).collect()
}

// spec/plumb.md §4.7, §5.3: every program runs right, verifies clean, and
// each linker's TLS rewritings are the specified ones.
#[test]
fn s4_7_tls_programs_run_and_verify_clean() {
    let Some(c) = corpus(Size::Tls) else { return };
    let expect: &[(Linker, &[Rewrite], bool)] = &[
        (Linker::Bfd, &[Rewrite::TlsIeToLe, Rewrite::TlsDescToLe, Rewrite::TlsGdToLe], false),
        (Linker::Lld, &[Rewrite::TlsIeToLe, Rewrite::TlsDescToLe], false),
        // mold keeps initial-exec and the traditional sequence: GOT entries.
        (Linker::Mold, &[Rewrite::TlsDescToLe, Rewrite::TlsDescToIe], true),
    ];
    for (linker, rewrites, slots) in expect {
        let link = c.get("tls-c", *linker);
        assert_eq!(link.run(), Some(0), "{}: the program fails", link.label());
        let r = link.verify();
        assert_clean(link, &r);
        for rw in *rewrites {
            assert!(
                r.coverage.accepted_of(*rw) > 0,
                "{}: no {}\n{}",
                link.label(),
                rw.id(),
                r.summary()
            );
        }
        if *slots {
            // Six initial-exec records and two traditional GD sequences'.
            assert!(
                r.coverage.verified_of(Via::GotSlot) >= 10,
                "{}\n{}",
                link.label(),
                r.summary()
            );
        }
    }
}

// spec/plumb.md §4.7, §9.1: every TLS type each linker accepts is checked.
#[test]
fn s4_7_every_tls_type_each_linker_accepts_is_checked() {
    let Some(c) = corpus(Size::Tls) else { return };
    for linker in Linker::ALL {
        let link = c.get("tls-types", linker);
        let r = link.verify();
        assert_clean(link, &r);
        assert_eq!(r.coverage.not_checked_total(), 0, "{}\n{}", link.label(), r.summary());
        let checked: BTreeSet<&str> = r
            .coverage
            .checked_types
            .keys()
            .filter_map(|&t| mortar_plumb::a64::def(t).map(|d| d.name))
            .collect();
        for name in tls_types_for(linker) {
            assert!(
                checked.contains(name),
                "{}: {name} not checked; checked: {checked:?}",
                link.label()
            );
        }
    }
    let r = c.get("tls-types", Linker::Bfd).verify();
    for rw in [Rewrite::TlsGdToLe, Rewrite::TlsLdToLe] {
        assert!(r.coverage.accepted_of(rw) > 0, "bfd: no {}\n{}", rw.id(), r.summary());
    }
}

// spec/plumb.md §10.2 A7: GNU ld relaxes local-dynamic to local-exec with
// a TCB gap of 16 whatever the block's alignment. With PT_TLS aligned to
// 64 the program reads the wrong variable, and the verifier says so; with
// alignment 8 both are right.
#[test]
fn s10_2_a7_gnu_ld_local_dynamic_ignores_the_tls_alignment() {
    let Some(c) = corpus(Size::Tls) else { return };
    let bad = c.get("tls-ld-64", Linker::Bfd);
    assert_ne!(bad.run(), Some(42), "{}: A7 no longer reproduces", bad.label());
    let r = bad.verify();
    let r1: Vec<_> = r.with_rule("R1").collect();
    assert_eq!(r1.len(), 3, "{}", r.render(&bad.label()));
    assert!(r1.iter().all(|f| f.expected == Some(0x40)), "{}", r.render(&bad.label()));
    // Nothing else, but the recorded anomalies of every glibc link (A10).
    assert!(unexpected(bad, &r).iter().all(|f| f.rule.id() == "R1"), "{:?}", ids(&r));
    let good = c.get("tls-ld-8", Linker::Bfd);
    assert_eq!(good.run(), Some(42), "{}", good.label());
    let r = good.verify();
    assert_clean(good, &r);
    assert_eq!(r.coverage.accepted_of(Rewrite::TlsLdToLe), 3, "{}", r.summary());
}

// spec/plumb.md §10.2 A8 (review finding 1): GNU ld and lld relax
// `ADRP x4; LDR x5, [x4]` to `MOVZ x4; MOVK x5`, which leaves x5 wrong;
// mold keeps the GOT entry and is right.
#[test]
fn s10_2_a8_initial_exec_relaxation_ignores_the_registers() {
    let Some(c) = corpus(Size::Tls) else { return };
    for linker in [Linker::Bfd, Linker::Lld] {
        let link = c.get("tls-ie-regs", linker);
        assert_ne!(link.run(), Some(42), "{}: A8 no longer reproduces", link.label());
        let r = link.verify();
        let r1: Vec<_> = r.with_rule("R1").collect();
        assert_eq!(r1.len(), 2, "{}", r.render(&link.label()));
        assert!(
            r1.iter().all(|f| f.symbol.as_deref() == Some("iv")),
            "{}",
            r.render(&link.label())
        );
    }
    let link = c.get("tls-ie-regs", Linker::Mold);
    assert_eq!(link.run(), Some(42), "{}", link.label());
    assert_clean(link, &link.verify());
}

// spec/plumb.md §4.3 R8 (review finding 4): a 64-bit GOT load type on a
// 32-bit load is refused by name, whatever the linker made of it.
#[test]
fn s4_3_r8_the_instruction_must_fit_the_type() {
    let Some(c) = corpus(Size::Tls) else { return };
    for linker in Linker::ALL {
        let link = c.get("got-ldr-w", linker);
        let r = link.verify();
        let r8: Vec<_> = r.with_rule("R8").collect();
        assert_eq!(r8.len(), 1, "{}", r.render(&link.label()));
        assert_eq!(r8[0].reloc.as_deref(), Some("R_AARCH64_LD64_GOT_LO12_NC"));
    }
}

// spec/plumb.md §4.4 (review finding 10): a PC-relative reference to an
// undefined weak symbol resolves to the place (AAELF64), so X = A.
#[test]
fn s4_4_undefined_weak_pc_relative_is_the_place() {
    let Some(c) = corpus(Size::Tls) else { return };
    for linker in [Linker::Bfd, Linker::Lld] {
        let link = c.get("weak-prel", linker);
        let r = link.verify();
        assert_clean(link, &r);
        assert_eq!(r.coverage.verified_of(Via::Direct), 5, "{}\n{}", link.label(), r.summary());
    }
}

// spec/plumb.md §4.7 (WP-092b): an initial-exec load of an undefined
// weak TLS symbol is judged per linker: lld's LE with T = A, GNU ld's and
// mold's GOT entry holding TPREL with S = 0. A mutant of the code word
// and (where there is one) of the GOT entry is a finding.
#[test]
#[allow(clippy::many_single_char_names)] // offsets and words of one output
fn s4_7_undefined_weak_tls_initial_exec_is_judged() {
    let Some(c) = corpus(Size::Tls) else { return };
    for linker in Linker::ALL {
        let link = c.get("weak-tls-ie", linker);
        assert_eq!(link.run(), Some(0), "{}", link.label());
        let r = link.verify();
        assert_clean(link, &r);
        let label = link.label();
        assert_eq!(r.coverage.not_checked_total(), 0, "{label}\n{}", r.summary());
        let judged = if linker == Linker::Lld {
            r.coverage.accepted_of(Rewrite::TlsIeToLe)
        } else {
            r.coverage.verified_of(Via::GotSlot)
        };
        assert_eq!(judged, 2, "{label}\n{}", r.summary());
        let map = link.parse_map();
        let files = link.inputs(&map);
        let out = link.output_bytes();
        let h = mortar_elf::FileHeader::parse(&out).unwrap();
        let secs: Vec<(String, u64, u64, u64)> = mortar_elf::SectionTable::parse(&out, &h)
            .unwrap()
            .iter()
            .map(|s| {
                let n = String::from_utf8_lossy(s.name().unwrap_or(b"")).into_owned();
                (n, s.header().sh_addr, s.header().sh_size, s.header().sh_offset)
            })
            .collect();
        let entry = h.e_entry;
        let off = |va: u64| {
            secs.iter()
                .find(|s| s.1 != 0 && s.1 <= va && va < s.1 + s.2)
                .map(|s| (s.3 + va - s.1) as usize)
                .unwrap()
        };
        // The LDR's or MOVK's immediate, one bit up.
        let mut m = out.clone();
        let o = off(entry + 4);
        m[o] ^= 1 << 5;
        let r = common::verify_with(&m, &files, &map);
        assert!(!r.findings.is_empty(), "{label}: a code mutant passes\n{}", r.summary());
        if linker != Linker::Lld {
            // The GOT entry: TPREL(0) on these, made TPREL(8).
            let (_, ga, gs, _) = secs.iter().find(|s| s.0 == ".got").unwrap().clone();
            let words = (0..gs / 8).map(|k| ga + 8 * k);
            let tp = words
                .filter(|&g| {
                    let o = off(g);
                    u64::from_le_bytes(out[o..o + 8].try_into().unwrap()) >> 63 == 1
                })
                .collect::<Vec<_>>();
            assert_eq!(tp.len(), 1, "{label}: one negative GOT word");
            let mut m = out.clone();
            let o = off(tp[0]);
            let v = u64::from_le_bytes(m[o..o + 8].try_into().unwrap()).wrapping_add(8);
            m[o..o + 8].copy_from_slice(&v.to_le_bytes());
            let r = common::verify_with(&m, &files, &map);
            let ids = ids(&r);
            assert!(ids.iter().any(|i| i == "R4"), "{label}: {ids:?}\n{}", r.summary());
        }
    }
}

// spec/plumb.md §4.8, §9.1: the IFUNC programs run, and every reference
// to an IFUNC is judged through an IPLT entry.
#[test]
fn s4_8_ifunc_programs_run_and_verify_clean() {
    let Some(c) = corpus(Size::Tls) else { return };
    for case in ["ifunc", "ifunc-c"] {
        for linker in Linker::ALL {
            let link = c.get(case, linker);
            assert_eq!(link.run(), Some(0), "{}: the program fails", link.label());
            let r = link.verify();
            assert_clean(link, &r);
            let cov = &r.coverage;
            let s = r.summary();
            assert_eq!(cov.not_checked_of(mortar_plumb::Reason::Ifunc), 0, "{}\n{s}", link.label());
            assert!(cov.irelative_checked >= 2 && cov.iplt_entries >= 2, "{}\n{s}", link.label());
            let iplt = cov.verified_of(Via::Iplt)
                + cov.accepted_of(Rewrite::AdrpAddToAdr)
                + cov.accepted_of(Rewrite::GotToAdr);
            assert!(iplt >= 8, "{}: too few IFUNC references judged\n{s}", link.label());
        }
    }
}

// spec/plumb.md §5.8, §9.1: debug information is checked, with each
// linker's tombstone for a section not in the output; compressed debug
// sections are named as not judged.
#[test]
fn s5_8_debug_references_verify_clean() {
    use mortar_plumb::Reason;
    let Some(c) = corpus(Size::Tls) else { return };
    for case in ["debug-comdat", "debug-comdat-clang", "debug-gc", "debug-zlib"] {
        for linker in Linker::ALL {
            let link = c.get(case, linker);
            let r = link.verify();
            assert_clean(link, &r);
            let cov = &r.coverage;
            let s = r.summary();
            assert_eq!(cov.not_checked_of(Reason::NonAlloc), 0, "{}\n{s}", link.label());
            if case == "debug-zlib" {
                assert!(cov.not_checked_of(Reason::Compressed) > 0, "{}\n{s}", link.label());
            } else {
                assert_eq!(cov.not_checked_of(Reason::Compressed), 0, "{}\n{s}", link.label());
                // GNU ld's redirect counts as a kept copy (§5.8, WP-250).
                let dead = cov.verified_of(Via::Tombstone) + cov.verified_of(Via::KeptCopy);
                assert!(dead > 0, "{}: no tombstone or kept copy\n{s}", link.label());
                if case == "debug-comdat" && link.label().ends_with("/bfd") {
                    assert!(
                        cov.verified_of(Via::KeptCopy) > 0,
                        "{}: no kept copy\n{s}",
                        link.label()
                    );
                }
            }
        }
    }
}

// spec/plumb.md §6.6, §9.1: program properties, merged by each linker's
// rule: GNU ld and lld AND BTI and PAC, OR the notes of one object
// (`prop-two`), and write none when one object has none; mold writes none
// on AArch64.
#[test]
fn s6_6_properties_verify_clean() {
    let Some(c) = corpus(Size::Tls) else { return };
    for (case, want) in [("prop-and", 3), ("prop-bti", 1), ("prop-none", 0), ("prop-two", 3)] {
        for linker in Linker::ALL {
            let link = c.get(case, linker);
            assert_eq!(link.run(), Some(0), "{}: the program fails", link.label());
            let r = link.verify();
            assert_clean(link, &r);
            let out = link.output_bytes();
            let image = mortar_elf::FileHeader::parse(&out).unwrap();
            let table = mortar_elf::SectionTable::parse(&out, &image).unwrap();
            let note = table.iter().find(|s| s.name().is_ok_and(|n| n == b".note.gnu.property"));
            let got = note.map_or(0, |s| {
                let o = s.header().sh_offset as usize + 24;
                u32::from_le_bytes(out[o..o + 4].try_into().unwrap())
            });
            let want = if linker == Linker::Mold { 0 } else { want };
            assert_eq!(got, want, "{}: the AArch64 feature property", link.label());
            assert!(r.coverage.properties_judged >= 1, "{}\n{}", link.label(), r.summary());
        }
    }
}
