//! The reference linkers are clean (`spec/plumb.md` §9.1): every link of
//! the corpus by GNU ld, lld and mold verifies with no finding but the
//! reference linkers' recorded defects (§10.2), and what is not checked is
//! only what stage 2 checks (§7.2).

mod common;

use std::collections::BTreeSet;
use std::fmt::Write as _;

use common::{Linker, Size, assert_clean, corpus, known_anomaly, types_for};
use mortar_plumb::{Reason, Report, Rewrite};

fn report_text(r: &Report, label: &str) -> String {
    format!("{label}:\n{}{}", r.render(label), r.summary())
}

// spec/plumb.md §9.1, §7.2.
#[test]
fn s9_1_reference_links_verify_clean() {
    let Some(c) = corpus(Size::All) else { return };
    let mut table = String::new();
    for link in &c.links {
        let r = link.verify();
        let label = link.label();
        let unexpected: Vec<_> =
            r.findings.iter().filter(|f| known_anomaly(link, f).is_none()).collect();
        assert!(unexpected.is_empty(), "{}", report_text(&r, &label));
        let cov = &r.coverage;
        assert!(cov.records > 0, "{label}: no relocation records");
        // Only a recorded anomaly (§10.2 A6) fails a record.
        let anomalous =
            r.findings.iter().filter(|f| f.rule == mortar_plumb::Rule::GotSlot).count() as u64;
        assert!(cov.failed <= anomalous, "{label}: {} records failed", cov.failed);
        assert_eq!(
            cov.records,
            cov.verified_total()
                + cov.accepted_total()
                + cov.not_checked_total()
                + cov.none
                + cov.failed,
            "{label}: the coverage counts do not add up"
        );
        // §6.4: the libraries' and the programs' FDEs are matched one by
        // one; a search table, where there is one, lists every live FDE.
        if matches!(link.case, "c-hello" | "cxx-eh" | "cxx-eh-hdr" | "comdat" | "gc-sections") {
            assert!(cov.fdes_checked > 100, "{label}: {} FDEs checked", cov.fdes_checked);
        }
        if matches!(link.case, "comdat" | "gc-sections") {
            assert!(cov.fdes_dropped > 0, "{label}: no FDE dropped with its function");
        }
        if link.case == "cxx-eh-hdr" || (link.linker == Linker::Mold && cov.fdes_checked > 0) {
            assert_eq!(
                cov.eh_frame_hdr_entries,
                cov.fdes_checked + cov.fdes_neutralised,
                "{label}: the search table"
            );
        }
        // §6.5: the symbol table. On the glibc cases every linker-defined
        // symbol GNU ld and lld write is judged; mold's own are named.
        if matches!(link.case, "c-hello" | "cxx-eh" | "fortran" | "comdat") {
            assert!(cov.globals_checked > 1000, "{label}: {} globals", cov.globals_checked);
            assert!(cov.locals_checked > 300, "{label}: {} locals", cov.locals_checked);
            assert!(cov.linker_defined_checked >= 9, "{label}: {}", cov.linker_defined_checked);
            // lld's Cortex-A53 843419 patch symbols are judged at the
            // patch of their site (plumb.md W8, WP-082).
            for n in &cov.linker_defined_not_judged {
                let mold_own = n.contains('$') || n.ends_with("_EHDR") || n.ends_with("_PHDR");
                assert!(link.linker == Linker::Mold && mold_own, "{label}: '{n}' not judged");
            }
        }
        if link.case == "cxx-eh-hdr" {
            assert_eq!(link.run(), Some(0), "{label}: the program fails");
        }
        for (reason, n) in &cov.not_checked {
            assert!(
                reason.is_stage2(),
                "{label}: {n} records not checked for '{}'\n{}",
                reason.id(),
                r.summary()
            );
        }
        let _ = writeln!(
            table,
            "{label:24} records {:6} verified {:6} accepted {:5} not-checked {:5} (tls {}, ifunc {}, eh-frame {}, non-alloc {}) fdes {}/{} hdr {} anomalies {}",
            cov.records,
            cov.verified_total(),
            cov.accepted_total(),
            cov.not_checked_total(),
            cov.not_checked_of(Reason::Tls),
            cov.not_checked_of(Reason::Ifunc),
            cov.not_checked_of(Reason::EhFrame),
            cov.not_checked_of(Reason::NonAlloc),
            cov.fdes_checked,
            cov.fdes_dropped,
            cov.eh_frame_hdr_entries,
            r.findings.len(),
        );
    }
    eprint!("{table}");
}

// spec/plumb.md §4.2, §9.1: the hand-written assembly exercises every type
// each linker accepts, and each of them is checked.
#[test]
fn s4_2_every_type_each_linker_accepts_is_checked() {
    let Some(c) = corpus(Size::Small) else { return };
    for linker in Linker::ALL {
        let link = c.get("types", linker);
        let r = link.verify();
        assert_clean(link, &r);
        let checked: BTreeSet<&str> = r
            .coverage
            .checked_types
            .keys()
            .filter_map(|&t| mortar_plumb::a64::def(t).map(|d| d.name))
            .collect();
        for name in types_for(linker) {
            assert!(
                checked.contains(name),
                "{}: {name} not checked; checked: {checked:?}",
                link.label()
            );
        }
    }
}

// spec/plumb.md §5.3, §5.4, §10.1: the rewritings each linker performs on
// the corpus are the specified ones, and each is accepted as such.
#[test]
fn s5_3_observed_rewritings_are_accepted() {
    let Some(c) = corpus(Size::Small) else { return };
    let expect: &[(&str, Linker, &[Rewrite])] = &[
        ("veneer", Linker::Bfd, &[Rewrite::Veneer]),
        ("veneer", Linker::Lld, &[Rewrite::Veneer]),
        ("veneer", Linker::Mold, &[Rewrite::Veneer]),
        ("got", Linker::Bfd, &[Rewrite::WeakBranchNop]),
        ("got", Linker::Lld, &[Rewrite::GotToAdr, Rewrite::WeakBranchNext]),
        ("got", Linker::Mold, &[Rewrite::GotToAdrpAdd, Rewrite::WeakBranchNop]),
        ("start", Linker::Lld, &[Rewrite::AdrpAddToAdr]),
        ("start", Linker::Mold, &[Rewrite::AdrpAddToAdr]),
        ("erratum", Linker::Bfd, &[Rewrite::Erratum843419Adr]),
        ("erratum", Linker::Lld, &[Rewrite::Erratum843419Patch]),
    ];
    for (case, linker, rewrites) in expect {
        let link = c.get(case, *linker);
        let r = link.verify();
        assert_clean(link, &r);
        // lld names each patch `__CortexA53843419_<site>`: judged (W8).
        assert!(
            !r.coverage
                .linker_defined_not_judged
                .iter()
                .any(|n| n.starts_with("__CortexA53843419_")),
            "{}: an lld 843419 patch symbol not judged",
            link.label()
        );
        for rw in *rewrites {
            assert!(
                r.coverage.accepted_of(*rw) > 0,
                "{}: no {} accepted\n{}",
                link.label(),
                rw.id(),
                r.summary()
            );
        }
    }
    // §10.2 A4: mold accepts --fix-cortex-a53-843419 and leaves the
    // sequence as it is; the verifier checks the fields, which are right.
    let link = c.get("erratum", Linker::Mold);
    let r = link.verify();
    assert_clean(link, &r);
    assert_eq!(r.coverage.accepted_total(), 0, "{}", r.summary());
}

// spec/plumb.md §3.2, §9.3: every reference map written in mortar's format
// reads back as the same records.
#[test]
fn s3_2_reference_maps_round_trip_through_mortar_format() {
    let Some(c) = corpus(Size::Small) else { return };
    for link in &c.links {
        let map = link.parse_map();
        let text = mortar_plumb::map::mortar::write(&with_line(&map));
        let back = mortar_plumb::map::parse(mortar_plumb::Format::Mortar, &text).unwrap();
        let key = |m: &mortar_plumb::Map| {
            let placed: Vec<_> = m
                .placed
                .iter()
                .map(|p| {
                    (
                        m.files[p.file as usize].to_string(),
                        p.section.clone(),
                        p.input_offset,
                        p.addr,
                        p.size,
                    )
                })
                .collect();
            let discarded: Vec<_> = m
                .discarded
                .iter()
                .map(|d| (m.files[d.file as usize].to_string(), d.section.clone(), d.reason))
                .collect();
            let chunks: Vec<_> = m
                .chunks
                .iter()
                .map(|c| (c.kind, c.name.clone(), c.addr, c.size, c.target.clone()))
                .collect();
            let defined: Vec<_> = m.defined.iter().map(|d| (d.name.clone(), d.value)).collect();
            (placed, discarded, chunks, defined)
        };
        assert_eq!(key(&back), key(&map), "{}", link.label());
        assert!(!map.placed.is_empty(), "{}", link.label());
    }
}

// spec/plumb.md §7.1: the report is deterministic.
#[test]
fn s7_1_reports_are_deterministic() {
    let Some(c) = corpus(Size::Small) else { return };
    for link in &c.links {
        assert_eq!(link.verify(), link.verify(), "{}", link.label());
        let _ = report_text;
    }
}

/// `map` with a line (`spec/plumb.md` §3.2a: every mortar map states its
/// link's words, and mortar's reader refuses one without). A reference's
/// map has none: these links gave none of the options §3.2a's rules read,
/// so the map's input paths stand for the words.
fn with_line(map: &mortar_plumb::Map) -> mortar_plumb::Map {
    let mut m = map.clone();
    if m.args.is_empty() {
        m.args = m.paths().iter().map(|p| p.as_bytes().to_vec()).collect();
        if m.args.is_empty() {
            m.args.push(b"a.o".to_vec());
        }
    }
    m
}
