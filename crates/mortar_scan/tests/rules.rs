//! `spec/image.md` §4 rule by rule (L1), on objects assembled from
//! AArch64 source and linked `-static -e _start` without libraries.
//! Each test is named for its rule; each diagnostic's text, id and key
//! are checked.

mod support;

use std::fmt::Write as _;
use std::path::Path;

use mortar_core::demands::DemandFlags as F;
use mortar_core::par::{Pool, check_schedules};
use mortar_core::{FileId, SymKey};
use mortar_diag::{Key, Policy};
use mortar_elf::Object;
use mortar_scan::Order;
use mortar_testutil::Scratch;
use support::{Linked, assemble, link_args, render};

/// Assembles each `(name, source)` and links them in order.
fn build(label: &str, objects: &[(&str, &str)], extra: &[&str]) -> Option<(Scratch, Linked)> {
    let dir = Scratch::new(label);
    for (name, src) in objects {
        assemble(dir.path(), name, src)?;
    }
    let mut args = vec!["-static", "-e", "_start", "-o", "out"];
    args.extend_from_slice(extra);
    args.extend(objects.iter().map(|o| o.0));
    let l = link_args(dir.path(), &args, &Pool::new(4)).expect("load");
    Some((dir, l))
}

const START: &str = ".text\n.globl _start\n_start:\n  ret\n";

fn key(file: u32, section: u64, record: u64) -> Key {
    Key::new(70, file, section << 32 | record)
}

fn keys(l: &Linked) -> Vec<Key> {
    let mut v: Vec<Key> = l.scanned.diagnostics.iter().map(|d| d.0).collect();
    v.sort();
    v
}

fn assert_before_clean(l: &Linked) {
    let ids: Vec<&str> = l.before.iter().map(|d| d.1.id()).collect();
    assert!(ids.is_empty(), "phases before the scan: {:?}", l.before);
}

const MANY: &str = ".text\n.globl _start\n_start:\n  bl missing\n  bl missing\n  bl missing\n  bl missing\n  bl missing\n  ret\n";
const TWO: &str = ".text\n.globl fb\nfb:\n  bl missing\n  bl missing\n  ret\n";

#[test]
fn i20_i21_i22_undefined_one_diagnostic_per_key() {
    let Some((_d, l)) = build("scan-i22-undef", &[("a.o", MANY), ("b.o", TWO)], &[]) else {
        return;
    };
    assert_before_clean(&l);
    assert_eq!(l.ids(), ["undefined-symbol"]);
    assert_eq!(keys(&l), [key(1, 1, 0)]);
    assert_eq!(
        l.rendered(),
        "ld: error: undefined symbol: missing (undefined reference to `missing')\n\
         >>> referenced by a.o:(.text+0x0)\n\
         >>> referenced by a.o:(.text+0x4)\n\
         >>> referenced by a.o:(.text+0x8)\n\
         >>> referenced 4 more times\n"
    );
}

#[test]
fn i22_undefined_first_reference_is_least_refid() {
    // b.o first on the line: its references come first.
    let Some((_d, l)) = build("scan-i22-order", &[("b.o", TWO), ("a.o", MANY)], &[]) else {
        return;
    };
    assert_eq!(keys(&l), [key(1, 1, 0)]);
    assert_eq!(
        l.rendered(),
        "ld: error: undefined symbol: missing (undefined reference to `missing')\n\
         >>> referenced by b.o:(.text+0x0)\n\
         >>> referenced by b.o:(.text+0x4)\n\
         >>> referenced by a.o:(.text+0x0)\n\
         >>> referenced 4 more times\n"
    );
}

#[test]
fn i19_undefined_weak_makes_no_event() {
    let src = ".text\n.globl _start\n.weak wk\n_start:\n  bl wk\n  adrp x0, wk\n  ret\n";
    let Some((_d, l)) = build("scan-i19-weak", &[("a.o", src)], &[]) else { return };
    assert_before_clean(&l);
    assert!(l.scanned.diagnostics.is_empty(), "{}", l.rendered());
    assert_eq!(l.scanned.stats.events, 0);
}

#[test]
fn i19_ignore_all_makes_no_event_and_warn_makes_warnings() {
    let Some((_d, l)) =
        build("scan-i19-ignore", &[("a.o", MANY)], &["--unresolved-symbols=ignore-all"])
    else {
        return;
    };
    assert!(l.scanned.diagnostics.is_empty(), "{}", l.rendered());
    let Some((_d, l)) = build("scan-i19-warn", &[("a.o", MANY)], &["--warn-unresolved-symbols"])
    else {
        return;
    };
    assert_eq!(l.ids(), ["undefined-symbol"]);
    assert!(l.rendered().starts_with("ld: warning: undefined symbol: missing"), "{}", l.rendered());
}

#[test]
fn i22_hidden_undefined_is_an_error_with_the_note() {
    let src = ".text\n.globl _start\n.hidden hm\n_start:\n  bl hm\n  ret\n";
    let Some((_d, l)) = build("scan-i22-hidden", &[("a.o", src)], &["--warn-unresolved-symbols"])
    else {
        return;
    };
    assert_eq!(
        l.rendered(),
        "ld: error: undefined symbol: hm (undefined reference to `hm')\n\
         >>> referenced by a.o:(.text+0x0)\n\
         >>> the symbol has hidden visibility\n"
    );
}

#[test]
fn i110_undefined_from_non_alloc_only_is_an_error() {
    let src = format!("{START}.section .debug_info,\"\",@progbits\n  .xword dbgmiss\n");
    let Some((d, l)) = build("scan-i110", &[("a.o", &src)], &[]) else { return };
    assert_eq!(l.ids(), ["undefined-symbol"]);
    let bytes = std::fs::read(d.join("a.o")).unwrap();
    let o = Object::parse(&bytes).unwrap();
    let idx = o.sections().find(|s| s.name().ok() == Some(&b".debug_info"[..])).unwrap().index().0;
    assert_eq!(keys(&l), [key(1, u64::from(idx), 0)]);
    assert_eq!(
        l.rendered(),
        "ld: error: undefined symbol: dbgmiss (undefined reference to `dbgmiss')\n\
         >>> referenced by a.o:(.debug_info+0x0)\n"
    );
}

#[test]
fn i24_error_limit_counts_groups() {
    let mut src = String::from(".text\n.globl _start\n_start:\n");
    for k in 0..25 {
        writeln!(src, "  bl m{k:02}\n  bl m{k:02}").unwrap();
    }
    let Some((_d, l)) = build("scan-i24", &[("a.o", &src)], &[]) else { return };
    assert_eq!(l.scanned.diagnostics.len(), 25);
    assert_eq!(l.scanned.stats.groups, 25);
    let text = render(&l.scanned.diagnostics, Policy::default());
    let heads = text.lines().filter(|t| t.starts_with("ld: error: undefined symbol")).count();
    assert_eq!(heads, 20, "{text}");
    assert!(text.contains("m19") && !text.contains("m20 "), "{text}");
    let all = render(&l.scanned.diagnostics, Policy { error_limit: 0, ..Policy::default() });
    assert_eq!(all.lines().filter(|t| t.starts_with("ld: error:")).count(), 25);
}

const G1: &str = ".section .text.g,\"axG\",@progbits,g,comdat\n.globl g\ng:\n  ret\n";
const G2: &str =
    ".section .text.g,\"axG\",@progbits,g,comdat\n.globl g\n.globl hx\ng:\n  ret\nhx:\n  ret\n";

#[test]
fn i18_comdat_mismatch_one_per_key() {
    let m = ".text\n.globl _start\n_start:\n  bl hx\n  bl hx\n  ret\n";
    let Some((_d, l)) = build("scan-i18-mismatch", &[("m.o", m), ("g1.o", G1), ("g2.o", G2)], &[])
    else {
        return;
    };
    assert_eq!(l.ids(), ["comdat-mismatch"], "{}", l.rendered());
    assert_eq!(keys(&l), [key(1, 1, 0)]);
    assert_eq!(
        l.rendered(),
        "ld: error: symbol hx is defined only in a discarded copy of COMDAT group g\n\
         >>> kept copy: g1.o\n\
         >>> discarded copy defining it: g2.o\n\
         >>> referenced by m.o:(.text+0x0)\n\
         >>> referenced by m.o:(.text+0x4)\n\
         >>> the copies of g differ: a one-definition-rule violation, or objects compiled with different options\n"
    );
}

/// A copy of group `g` with a local `lg` referenced from `.text` and
/// from `{from}`.
fn dropped_ref(from: &str) -> String {
    format!(
        ".section .text.g,\"axG\",@progbits,g,comdat\n.globl g\ng:\n  ret\nlg:\n  ret\n\
         .text\n.globl fb\nfb:\n  bl lg\n  ret\n{from}"
    )
}

#[test]
fn i18_discarded_section_reference_per_symbol_and_section() {
    let b = dropped_ref("");
    let Some((_d, l)) =
        build("scan-i18-discard", &[("a.o", START), ("g1.o", G1), ("b.o", &b)], &[])
    else {
        return;
    };
    assert_eq!(l.ids(), ["discarded-section-reference"], "{}", l.rendered());
    let text = l.rendered();
    assert!(
        text == "ld: error: relocation refers to a symbol in a discarded section: lg\n\
                 >>> defined in b.o:(.text.g)\n\
                 >>> referenced by b.o:(.text+0x0)\n"
            || text
                == "ld: error: relocation refers to a symbol in a discarded section: .text.g\n\
                    >>> defined in b.o:(.text.g)\n\
                    >>> referenced by b.o:(.text+0x0)\n",
        "{text}"
    );
}

#[test]
fn i45_i46_resolve_6_3_referring_section_names() {
    // Accepted: a debug name, `.gcc_except_table`, `.eh_frame`-like
    // dead FDEs. Refused: any other non-alloc name, an alloc debug name.
    let cases: [(&str, bool); 6] = [
        (".section .debug_info,\"\",@progbits\n  .xword lg\n", false),
        (".section .gcc_except_table,\"a\",@progbits\n  .xword lg\n", false),
        (".section .comment.x,\"\",@progbits\n  .xword lg\n", true),
        (".section .debu,\"\",@progbits\n  .xword lg\n", true),
        (".section .debug_x,\"a\",@progbits\n  .xword lg\n", true),
        (".section .zdebug_x,\"\",@progbits\n  .xword lg\n", false),
    ];
    for (k, (from, error)) in cases.iter().enumerate() {
        let b = dropped_ref(from);
        let label = format!("scan-i46-{k}");
        let Some((_d, l)) = build(&label, &[("a.o", START), ("g1.o", G1), ("b.o", &b)], &[]) else {
            return;
        };
        let n = l.ids().iter().filter(|i| **i == "discarded-section-reference").count();
        assert_eq!(n, 1 + usize::from(*error), "{from}: {}", l.rendered());
    }
}

#[test]
fn w1_two_files_with_equal_section_and_symbol_indexes() {
    let b = dropped_ref("");
    let Some((_d, l)) =
        build("scan-w1", &[("a.o", START), ("g1.o", G1), ("b.o", &b), ("c.o", &b)], &[])
    else {
        return;
    };
    assert_eq!(l.ids(), ["discarded-section-reference", "discarded-section-reference"]);
    assert_eq!(keys(&l), [key(3, 2, 0), key(4, 2, 0)], "{}", l.rendered());
}

#[test]
fn i19_references_from_dropped_sections_make_no_event() {
    // The dropped copy of g references `nowhere`; the kept one does not.
    let g2 = ".section .text.g,\"axG\",@progbits,g,comdat\n.globl g\ng:\n  bl nowhere\n  ret\n";
    let Some((_d, l)) =
        build("scan-i19-dropped", &[("a.o", START), ("g1.o", G1), ("g2.o", g2)], &[])
    else {
        return;
    };
    assert!(l.scanned.diagnostics.is_empty(), "{}", l.rendered());
}

#[test]
fn i19_consumed_sequence_members_make_no_event() {
    let src = ".text\n.globl _start\n_start:\n  adrp x0, :tlsgd:tv\n  add x0, x0, :tlsgd_lo12:tv\n  bl __tls_get_addr\n  nop\n  ret\n\
               .section .tbss,\"awT\",@nobits\n.globl tv\ntv:\n  .space 4\n";
    // TLSGD is M2 in a64.md's table (`A64::M1` refuses it): the rule is
    // the driver's, so it is tested with the M2 table.
    let m2 = mortar_a64::A64::new(mortar_arch::Status::M2, false);
    let Some((_d, mut l)) = build("scan-i19-consumed", &[("a.o", src)], &[]) else { return };
    assert_eq!(l.ids(), ["refused", "refused"], "{}", l.rendered());
    l.arch = m2;
    l.scanned = l.rescan(&Pool::new(2));
    assert!(l.scanned.diagnostics.is_empty(), "{}", l.rendered());
    // Without the `nop` the call is no sequence: __tls_get_addr is
    // referenced.
    let src = src.replace("  nop\n", "");
    let Some((_d, mut l)) = build("scan-i19-unconsumed", &[("a.o", &src)], &[]) else { return };
    l.arch = m2;
    l.scanned = l.rescan(&Pool::new(2));
    assert_eq!(l.ids(), ["undefined-symbol"], "{}", l.rendered());
}

#[test]
fn i23_gnu_warning_per_key() {
    let w = ".text\n.globl danger\ndanger:\n  ret\n.section .gnu.warning.danger,\"\",@progbits\n  .string \"danger is dangerous\"\n";
    let a = ".text\n.globl _start\n_start:\n  bl danger\n  bl danger\n  ret\n";
    let Some((_d, l)) = build("scan-gnu-warning", &[("a.o", a), ("w.o", w)], &[]) else {
        return;
    };
    assert_eq!(l.ids(), ["gnu-warning"], "{}", l.rendered());
    assert_eq!(keys(&l), [key(1, 1, 0)]);
    assert_eq!(
        l.rendered(),
        "ld: warning: danger is dangerous\n\
         >>> referenced by a.o:(.text+0x0)\n\
         >>> referenced by a.o:(.text+0x4)\n"
    );
}

/// Rewrites, in `path`, record 0 of `.rela.text`: `r_sym` when given,
/// `r_offset` when given.
fn patch_rela(path: &Path, r_sym: Option<u32>, r_offset: Option<u64>) {
    let mut bytes = std::fs::read(path).unwrap();
    let range = {
        let o = Object::parse(&bytes).unwrap();
        let s = o.sections().find(|s| s.name().ok() == Some(&b".rela.text"[..])).unwrap();
        s.file_range().unwrap().unwrap()
    };
    let at = range.start;
    if let Some(off) = r_offset {
        bytes[at..at + 8].copy_from_slice(&off.to_le_bytes());
    }
    if let Some(sym) = r_sym {
        let info = u64::from_le_bytes(bytes[at + 8..at + 16].try_into().unwrap());
        let info = (u64::from(sym) << 32) | (info & 0xffff_ffff);
        bytes[at + 8..at + 16].copy_from_slice(&info.to_le_bytes());
    }
    std::fs::write(path, bytes).unwrap();
}

#[test]
fn i17_malformed_records_refused_by_name_with_their_keys() {
    let dir = Scratch::new("scan-i17");
    let src = ".text\n.globl _start\n_start:\n  bl _start\n  bl _start\n  ret\n";
    let Some(a) = assemble(dir.path(), "a.o", src) else { return };
    patch_rela(&a, Some(999), None);
    let l = link_args(dir.path(), &["-static", "-e", "_start", "-o", "out", "a.o"], &Pool::new(2))
        .expect("load");
    assert_eq!(l.ids(), ["malformed-input"]);
    assert_eq!(keys(&l), [key(1, 1, 0)]);
    let text = l.rendered();
    assert!(text.starts_with("ld: error: "), "{text}");
    assert!(text.contains("a.o:(.text+0x0)") && text.contains("999"), "{text}");

    let Some(a) = assemble(dir.path(), "b.o", src) else { return };
    patch_rela(&a, None, Some(0x1000));
    let l = link_args(dir.path(), &["-static", "-e", "_start", "-o", "out", "b.o"], &Pool::new(2))
        .expect("load");
    assert_eq!(l.ids(), ["malformed-input"]);
    assert_eq!(keys(&l), [key(1, 1, 0)]);
    assert!(
        l.rendered()
            .contains("relocation R_AARCH64_CALL26 at offset 0x1000 lies outside its section"),
        "{}",
        l.rendered()
    );
}

#[test]
fn i29_record_index_sorted_and_permuted() {
    let src = ".data\n.globl x\nx:\n  .xword 0, 0, 0\n  .reloc x+8, R_AARCH64_ABS64, x\n  .reloc x, R_AARCH64_ABS64, x\n  .reloc x+16, R_AARCH64_ABS64, x\n";
    let Some((_d, l)) = build("scan-i29", &[("s.o", START), ("a.o", src)], &[]) else { return };
    assert!(l.scanned.diagnostics.is_empty(), "{}", l.rendered());
    let unsorted: Vec<_> = l.scanned.order.unsorted().collect();
    assert_eq!(unsorted.len(), 1);
    assert_eq!(l.scanned.order.get(unsorted[0]), Order::Permuted(&[1, 0, 2]));
    let (file, _) = l.outcome.resolution.sh_index(unsorted[0]).unwrap();
    assert_eq!(file, FileId(2));
}

#[test]
fn i16_demands_of_got_and_tls() {
    let src = ".text\n.globl _start\n_start:\n  adrp x0, :got:gv\n  ldr x0, [x0, :got_lo12:gv]\n  \
               adrp x1, :gottprel:tv\n  ldr x1, [x1, :gottprel_lo12:tv]\n  ret\n\
               .data\n.globl gv\ngv:\n  .xword 0\n.section .tbss,\"awT\",@nobits\n.globl tv\ntv:\n  .space 8\n";
    let Some((_d, l)) = build("scan-i16", &[("a.o", src)], &[]) else { return };
    assert!(l.scanned.diagnostics.is_empty(), "{}", l.rendered());
    let id = |n: &[u8]| l.inputs.symbols.iter().find(|(_, k)| k.name == n).unwrap().0;
    let d = &l.scanned.demands;
    assert!(d.of(SymKey::global(id(b"gv"))).contains(F::GOT), "{d:?}");
    assert!(d.of(SymKey::global(id(b"tv"))).contains(F::GOT_TPREL), "{d:?}");
    d.check(l.inputs.symbols.len()).unwrap();
}

/// Many references to one key from many blocks, and from two files:
/// the whole scan, under every schedule, against one thread.
#[test]
fn i21_merge_under_schedules_big_sections() {
    let src =
        ".text\n.globl _start\n_start:\n  ret\n.data\n.rept 140000\n  .xword missing\n.endr\n";
    let b = ".data\n.rept 70000\n  .xword missing\n  .xword other\n.endr\n";
    let Some((_d, l)) = build("scan-i21-big", &[("a.o", src), ("b.o", b)], &[]) else { return };
    assert_eq!(l.scanned.stats.units, 3 + 3, "{:?}", l.scanned.stats);
    let pic = check_schedules(|pool| Linked::picture(&l.rescan(pool)));
    assert_eq!(pic.2.len(), 2);
    assert!(pic.2[0].1.contains(">>> referenced 209997 more times"), "{}", pic.2[0].1);
    assert!(pic.2[1].1.contains("undefined symbol: other") && pic.2[1].1.contains("69997 more"));
}

/// I-29 and §3.4 across blocks: a section of more than one block whose
/// records are not in order; the second block's windows overlap the
/// first's, which only the merge sees.
#[test]
fn i29_overlap_across_blocks_and_permutation() {
    let mut src = String::from(".text\n.globl _start\n_start:\n  ret\n.data\n.globl x\nx:\n");
    let n = 65_536 + 100;
    src.push_str(".rept 65537\n  .xword 0\n.endr\n");
    for i in 0..65_536 {
        writeln!(src, "  .reloc x+{}, R_AARCH64_ABS64, x", 8 * i).unwrap();
    }
    for i in 0..100 {
        writeln!(src, "  .reloc x+{}, R_AARCH64_ABS64, x", 8 * i + 4).unwrap();
    }
    let Some((_d, l)) = build("scan-i29-blocks", &[("a.o", &src)], &[]) else { return };
    assert_eq!(l.scanned.stats.rewalked, 1, "{:?}", l.scanned.stats);
    let ids = l.ids();
    // Records 1..=100 overlap the second block's, and those the first's.
    assert_eq!(ids.len(), 200, "{}", l.rendered());
    assert!(ids.iter().all(|i| *i == "refused"));
    let unsorted: Vec<_> = l.scanned.order.unsorted().collect();
    let Order::Permuted(p) = l.scanned.order.get(unsorted[0]) else { panic!() };
    assert_eq!(p.len(), n);
    assert_eq!(&p[..3], &[0, 65_536, 1]);
    let pic = check_schedules(|pool| Linked::picture(&l.rescan(pool)));
    assert_eq!(pic.2.len(), 200);
    assert!(pic.2[0].1.contains("overlapping relocations"), "{}", pic.2[0].1);
}
