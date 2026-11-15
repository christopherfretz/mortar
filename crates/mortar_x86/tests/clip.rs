//! Clipped output (arch.md §3.6, `spec/image.md` I-33, I-35, I-111):
//! `apply` called on any clip writes exactly the bytes of an unclipped
//! call inside it, for every accepted type and every relaxation form of
//! x86.md §8 and §9, at every pair of clip boundaries; no record writes outside
//! `Facts.max_reach` around its field, nor outside its unit's window;
//! `RelocAt.leads = false` changes no byte of a section without a lead.
#![allow(clippy::many_single_char_names)]

mod support;

use mortar_arch::{
    Arch, FieldError, FillCx, OutputKind, Reloc, RelocAt, SectionOut, Site, SlotKind, TargetKind,
    TlsBlock, Window, max_window,
};
use mortar_core::{FileId, SectionId};
use mortar_x86::relax::MEMBER_WINDOWS;
use mortar_x86::{MAX_REACH, X86, table};
use support::{Case, Tables, key};

const TLS: TlsBlock = TlsBlock { start: 0x0806_0000, size: 0x2_0000, align: 16 };
const ADDR: u64 = 0x0804_8ff0;
const GOT: u64 = 0x0808_0000;

/// A REL record: its addend is in the input bytes (X86-4).
fn r(r_offset: u64, k: u32, r_type: u32) -> Reloc {
    Reloc { r_offset, key: key(k), r_type, addend: 0 }
}

/// Per byte of the clip, `Some(v)` if some record wrote `v`, else `None`;
/// or the first error. Writes are detected with two fills (0x00, 0xFF):
/// `apply` never reads the output.
type Written = Result<Vec<Option<u8>>, FieldError>;

fn at<'a>(
    c: &Case<'a>,
    relocs: &'a [Reloc],
    h: &'a mortar_arch::Howto,
    i: usize,
    leads: bool,
) -> RelocAt<'a> {
    let rec = relocs[i];
    RelocAt {
        file: FileId(1),
        section: SectionId(3),
        index: u32::try_from(i).unwrap(),
        r_offset: rec.r_offset,
        key: rec.key,
        r_type: rec.r_type,
        addend: None,
        howto: h,
        site: Site { bytes: c.input, offset: rec.r_offset },
        records: relocs_dyn(relocs),
        proof: &[],
        leads,
    }
}

fn relocs_dyn(relocs: &[Reloc]) -> &dyn mortar_arch::Records {
    Box::leak(Box::new(relocs))
}

fn out<'b>(c: &Case<'b>, fill: &'b mut [u8], start: u64) -> SectionOut<'b> {
    let o = SectionOut::new(fill, start, c.addr, c.alloc, c.name);
    match c.exec {
        Some(e) => o.with_exec(e),
        None => o,
    }
}

fn run(c: &Case<'_>, relocs: &[Reloc], leads: bool, clip: (u64, u64)) -> Written {
    let (a, b) = clip;
    let len = usize::try_from(b - a).unwrap();
    let mut fills = [vec![0u8; len], vec![0xFFu8; len]];
    let cx = FillCx { tables: c.tables, output: c.output, got_base: c.got_base, tls: c.tls };
    let howtos: Vec<_> = relocs.iter().map(|x| c.howto(x.r_type, x.r_offset)).collect();
    for fill in &mut fills {
        for (i, h) in howtos.iter().enumerate() {
            let at = at(c, relocs, h, i, leads);
            let mut o = out(c, fill, a);
            assert_eq!(o.clip(), clip);
            c.arch.apply(&cx, &at, &mut o)?;
        }
    }
    let [zero, ones] = fills;
    Ok(zero
        .iter()
        .zip(&ones)
        .map(|(&z, &o)| {
            assert!(z == o || (z, o) == (0, 0xFF));
            (z == o).then_some(z)
        })
        .collect())
}

/// Every clip pair `a ≤ b` gives the unclipped result's restriction (and
/// the same error). Returns the unclipped result.
fn check_clips(c: &Case<'_>, relocs: &[Reloc], what: &str) -> Written {
    let len = c.input.len() as u64;
    let whole = run(c, relocs, true, (0, len));
    for a in 0..=len {
        for b in a..=len {
            let got = run(c, relocs, true, (a, b));
            let want = whole.as_ref().map(|w| w[a as usize..b as usize].to_vec());
            assert_eq!(got, want.map_err(|e| *e), "{what}: clip [{a:#x}, {b:#x})");
        }
    }
    whole
}

fn image(input: &[u8], w: &[Option<u8>]) -> Vec<u8> {
    input.iter().zip(w).map(|(&i, &o)| o.unwrap_or(i)).collect()
}

/// Record `i`'s own writes lie inside its `max_reach` window and its
/// unit's window.
fn check_reach(c: &Case<'_>, relocs: &[Reloc], i: usize, what: &str) {
    let rec = relocs[i];
    let h = c.howto(rec.r_type, rec.r_offset);
    let a = at(c, relocs, &h, i, true);
    let cx = FillCx { tables: c.tables, output: c.output, got_base: c.got_base, tls: c.tls };
    let mut written = vec![false; c.input.len()];
    for fill in [0u8, 0xFF] {
        let mut buf = vec![fill; c.input.len()];
        let mut o = out(c, &mut buf, 0);
        if c.arch.apply(&cx, &a, &mut o).is_err() {
            return;
        }
        for (w, &v) in written.iter_mut().zip(&buf) {
            *w |= v != fill;
        }
    }
    let reach = c.arch.facts().max_reach;
    let lo = rec.r_offset - u64::from(reach.before);
    let hi = rec.r_offset + u64::from(h.field.width()) + u64::from(reach.after);
    let unit = c.arch.unit_window(c.output, &a, c.tables).unwrap();
    // The form's own window, tighter than `max_reach`.
    let (flo, fhi) = mortar_arch::seq::field_window(rec.r_offset, &h).unwrap();
    for (off, _) in written.iter().enumerate().filter(|(_, w)| **w) {
        let off = off as u64;
        assert!(lo <= off && off < hi, "{what}: record {i} wrote {off:#x} outside max_reach");
        assert!(flo <= off && off < fhi, "{what}: record {i} wrote {off:#x} outside its window");
        assert!(
            unit.start <= off && off < unit.end,
            "{what}: record {i} wrote {off:#x} outside its unit {unit:?}"
        );
    }
}

fn tables() -> Tables {
    let mut t = Tables::default();
    // 1: a local near the section; 2: a thread-local; 3: a small absolute;
    // 4: ifunc; 5: weak0; 6: thread-local weak0; 7 (below); 9: `___tls_get_addr`.
    t.sym(key(1), ADDR + 0x100, TargetKind::Local);
    t.tls_sym(key(2), 0x0806_0040, TargetKind::Local);
    t.sym(key(3), 0x100, TargetKind::Abs);
    t.sym(key(4), 0x0804_8400, TargetKind::Ifunc);
    t.sym(key(5), 0, TargetKind::Weak0);
    t.tls_sym(key(6), 0, TargetKind::Weak0);
    // 7: a local next to the field (`PC8` reaches it).
    t.sym(key(7), ADDR + 12, TargetKind::Local);
    t.sym(key(9), 0, TargetKind::Undefined);
    t.names.insert(key(9), b"___tls_get_addr".to_vec());
    for k in 1..=7 {
        let base = GOT + 0x100 * u64::from(k);
        for (n, kind) in
            [SlotKind::Addr, SlotKind::TpOff { negated: false }].into_iter().enumerate()
        {
            t.slots.insert((key(k), kind), base + 4 * n as u64);
        }
        t.plts.insert(key(k), 0x0804_8100 + 16 * u64::from(k));
    }
    t
}

fn case<'a>(t: &'a Tables, input: &'a [u8], output: OutputKind) -> Case<'a> {
    let mut c = Case::new(t, input, ADDR);
    c.output = output;
    c.tls = Some(TLS);
    c.got_base = Some(GOT);
    c
}

/// The bytes before the field each single-record form needs (§8's and
/// §9's tables), an X86-13 refusal, and none.
const PREFIXES: [&[u8]; 11] = [
    &[],
    &[0x8b, 0x05],
    &[0x8b, 0x83],
    &[0x03, 0x8b],
    &[0x85, 0x05],
    &[0xff, 0x93],
    &[0xff, 0xa3],
    &[0xa1],
    &[0x8d, 0x83],
    &[0x8b, 0x84, 0x0d],
    &[0xff, 0x15],
];

/// A 24-byte section: `cc`, the prefix ending at offset 8, the field at 8
/// holding the addend `a` (X86-4), `ff 10` after it (T2's marker at 12).
fn single(prefix: &[u8], a: i32) -> Vec<u8> {
    let mut b = vec![0xcc; 24];
    b[8 - prefix.len()..8].copy_from_slice(prefix);
    b[8..12].copy_from_slice(&a.to_le_bytes());
    b[12..14].copy_from_slice(&[0xff, 0x10]);
    b
}

#[test]
fn s3_6_clip_every_type() {
    let t = tables();
    let x = X86::M1;
    let mut checked = 0usize;
    let mut none = Vec::new();
    let mut accepted = 0;
    for r_type in 0..=table::MAX_TYPE {
        if x.classify(r_type, Site { bytes: &[0; 24], offset: 8 }).is_err() {
            continue;
        }
        accepted += 1;
        let mut ok = false;
        for output in [OutputKind::Static, OutputKind::Pie] {
            for prefix in PREFIXES {
                for a in [-4, 0] {
                    let input = single(prefix, a);
                    let mut c = case(&t, &input, output);
                    for (name, exec, alloc) in [
                        (&b".text"[..], Some(true), true),
                        (b".data", Some(false), true),
                        (b".debug_info", None, false),
                    ] {
                        c.name = name;
                        c.exec = exec;
                        c.alloc = alloc;
                        for k in 1..=7 {
                            // The marker of `TLS_DESC_CALL` is at 12.
                            let off = if r_type == 40 { 12 } else { 8 };
                            let relocs = [r(off, k, r_type)];
                            let what = format!(
                                "type {r_type} after {prefix:x?}, key {k}, A {a}, {output:?}, {}",
                                String::from_utf8_lossy(name)
                            );
                            if let Ok(whole) = check_clips(&c, &relocs, &what) {
                                ok = true;
                                checked += 1;
                                check_reach(&c, &relocs, 0, &what);
                                if !matches!(r_type, 18 | 19) {
                                    let len = input.len() as u64;
                                    assert_eq!(
                                        run(&c, &relocs, false, (0, len)),
                                        Ok(whole),
                                        "{what}: leads = false"
                                    );
                                }
                            }
                        }
                    }
                }
            }
        }
        if !ok && !matches!(r_type, 18 | 19) {
            none.push(r_type);
        }
    }
    eprintln!("{accepted} accepted types, {checked} cases");
    assert_eq!(accepted, 21);
    assert!(none.is_empty(), "types applied on no candidate: {none:?}");
}

const E8: [u8; 5] = [0xe8, 0xfc, 0xff, 0xff, 0xff];

fn d1() -> Vec<u8> {
    [&[0x8d, 0x04, 0x1d, 0, 0, 0, 0][..], &E8].concat()
}

fn d2() -> Vec<u8> {
    vec![0x8d, 0x83, 0, 0, 0, 0, 0xff, 0x93, 0, 0, 0, 0]
}

/// Every relaxation form of §8 and §9, two bytes of padding each side.
#[test]
fn s3_6_clip_every_form() {
    let t = tables();
    let d3 = [&[0x8d, 0x83, 0, 0, 0, 0][..], &E8, &[0x90]].concat();
    let l1 = [&[0x8d, 0x86, 0, 0, 0, 0][..], &E8].concat();
    let desc = vec![0x8d, 0x8b, 0, 0, 0, 0, 0xff, 0x10];
    let two = [d1(), d2()].concat();
    let s = OutputKind::Static;
    let p = OutputKind::Pie;
    #[allow(clippy::type_complexity)]
    let forms: Vec<(&str, OutputKind, Vec<u8>, Vec<Reloc>)> = vec![
        ("G1", s, vec![0x8b, 0x83, 0, 0, 0, 0], vec![r(2, 1, 43)]),
        ("G1 no base", s, vec![0x8b, 0x0d, 0, 0, 0, 0], vec![r(2, 3, 43)]),
        ("G1 weak0", s, vec![0x8b, 0x83, 0, 0, 0, 0], vec![r(2, 5, 43)]),
        ("G2", s, vec![0x33, 0x8b, 0, 0, 0, 0], vec![r(2, 1, 43)]),
        ("G3", s, vec![0x85, 0x05, 0, 0, 0, 0], vec![r(2, 1, 43)]),
        ("G4", s, vec![0xff, 0x93, 0, 0, 0, 0], vec![r(2, 1, 43)]),
        ("G5", s, vec![0xff, 0x25, 0, 0, 0, 0], vec![r(2, 7, 43)]),
        ("I1", s, vec![0xa1, 0, 0, 0, 0], vec![r(1, 2, 15)]),
        ("I2", p, vec![0x8b, 0x0d, 0, 0, 0, 0], vec![r(2, 2, 15)]),
        ("I3", s, vec![0x03, 0x3d, 0, 0, 0, 0], vec![r(2, 2, 15)]),
        ("I4", p, vec![0x8b, 0x8b, 0, 0, 0, 0], vec![r(2, 2, 16)]),
        ("I5", s, vec![0x03, 0xbe, 0, 0, 0, 0], vec![r(2, 6, 16)]),
        ("T1 T2", s, desc.clone(), vec![r(2, 2, 39), r(6, 2, 40)]),
        ("T1 T2 weak0", p, desc, vec![r(2, 6, 39), r(6, 6, 40)]),
        ("D1", s, d1(), vec![r(3, 2, 18), r(8, 9, 4)]),
        ("D1 PC32", p, d1(), vec![r(3, 2, 18), r(8, 9, 2)]),
        ("D2", s, d2(), vec![r(2, 2, 18), r(8, 9, 43)]),
        ("D3", p, d3, vec![r(2, 6, 18), r(7, 9, 4)]),
        ("L1", s, l1, vec![r(2, 2, 19), r(7, 9, 4)]),
        ("L2", p, d2(), vec![r(2, 2, 19), r(8, 9, 3)]),
        ("D1 D2", s, two, vec![r(3, 2, 18), r(8, 9, 4), r(14, 6, 18), r(20, 9, 43)]),
    ];
    for (what, output, form, relocs) in forms {
        let mut input = vec![0xcc, 0xcc];
        input.extend(&form);
        input.extend([0xcc, 0xcc]);
        let relocs: Vec<Reloc> =
            relocs.iter().map(|x| Reloc { r_offset: x.r_offset + 2, ..*x }).collect();
        let c = case(&t, &input, output);
        let whole = check_clips(&c, &relocs, what).unwrap_or_else(|e| panic!("{what}: {e:?}"));
        let img = image(&input, &whole);
        assert_ne!(img, input, "{what}: nothing written");
        // Only the form's bytes change (X86-9).
        assert_eq!(img[..2], [0xcc, 0xcc], "{what}");
        assert_eq!(img[img.len() - 2..], [0xcc, 0xcc], "{what}");
        // The instruction was rewritten: a relaxation, not the GOT load.
        assert_ne!(img[2..4], input[2..4], "{what}: not relaxed");
        for i in 0..relocs.len() {
            check_reach(&c, &relocs, i, what);
        }
        if !relocs.iter().any(|x| matches!(x.r_type, 18 | 19)) {
            let len = input.len() as u64;
            assert_eq!(run(&c, &relocs, false, (0, len)), Ok(whole), "{what}: leads");
        }
    }
}

/// The extent covers each sequence's consumed member (arch.md §3.5).
#[test]
fn s3_5_extent_covers_the_unit() {
    let t = tables();
    let input = d1();
    let relocs = [r(3, 2, 18), r(8, 9, 4)];
    let c = case(&t, &input, OutputKind::Static);
    let x = X86::M1;
    for i in 0..2 {
        let h = c.howto(relocs[i].r_type, relocs[i].r_offset);
        let a = at(&c, &relocs, &h, i, true);
        let unit = x.unit_window(c.output, &a, &t).unwrap();
        assert!(unit.unit + x.sequence_extent(&relocs.as_slice(), unit.unit) > i as u32);
    }
    for ty in 0..=table::MAX_TYPE {
        assert_eq!(x.sequence_extent(&[r(0, 1, ty)].as_slice(), 0), 1, "{ty}");
    }
}

/// `Facts.max_reach` is derived (I-111) from the windows `classify` gives
/// over the whole type range and every form.
#[test]
fn s3_1_max_reach_derived() {
    let x = X86::M1;
    let mut w = Window { before: 0, after: 0 };
    let mut inputs = vec![d1(), d2(), vec![0x8d, 0x83, 0, 0, 0, 0, 0xe8, 0, 0, 0, 0, 0x90]];
    for p in PREFIXES {
        inputs.push(single(p, 0));
    }
    for input in &inputs {
        for off in 0..input.len() as u64 {
            for t in 0..=0xFFFF {
                if let Ok(h) = x.classify(t, Site { bytes: input, offset: off }) {
                    assert!(t <= table::MAX_TYPE, "type {t} above MAX_TYPE");
                    w = max_window(w, h.window);
                }
            }
        }
    }
    assert_eq!(w, MAX_REACH);
    assert_eq!(max_window(w, MEMBER_WINDOWS), MAX_REACH);
    assert_eq!((MAX_REACH.before, MAX_REACH.after), (3, 6));
}
