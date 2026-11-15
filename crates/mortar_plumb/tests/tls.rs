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
