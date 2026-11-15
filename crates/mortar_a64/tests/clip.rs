//! Clipped output (arch.md §3.6, `spec/image.md` I-33, I-35, I-111;
//! WP-023b): `apply` called on any clip writes exactly the bytes of an
//! unclipped call inside it, for every accepted type and every
//! relaxation sequence, at every pair of clip boundaries; no record
//! writes outside `Facts.max_reach` around its field, nor outside its
//! unit's window; `RelocAt.leads = false` changes no byte of a section
//! without a lead; `sequence_extent` covers each recognized sequence.
#![allow(clippy::many_single_char_names)]

mod support;

use mortar_a64::relax::MEMBER_WINDOWS;
use mortar_a64::{MAX_REACH, MAX_TYPE, erratum_adr, erratum_branch};
use mortar_arch::{
    Arch, FieldError, FillCx, OutputKind, Reloc, RelocAt, SectionOut, Site, SlotKind, TargetKind,
    TlsBlock, Window, max_window,
};
use mortar_core::{FileId, SectionId};
use support::{Case, M2, Tables, key, words};

const NOP: u32 = 0xD503_201F;
const TLS: TlsBlock = TlsBlock { start: 0x42_0000, size: 0x2_0000, align: 16 };
const ADDR: u64 = 0x40_0ff0;

fn r(r_offset: u64, k: u32, r_type: u32) -> Reloc {
    Reloc { r_offset, key: key(k), r_type, addend: 0 }
}

/// The outcome of applying a section's records into one clip: per byte
/// of the clip, `Some(v)` if some record wrote `v`, else `None`; or the
/// first error. Writes are detected with two fills (0x00 and 0xFF), since
/// `apply` never reads the output.
type Written = Result<Vec<Option<u8>>, FieldError>;

/// Writes done after the relocations (the erratum fix), as `(offset,
/// bytes)`.
type After<'a> = &'a [(u64, Vec<u8>)];

fn run(c: &Case<'_>, relocs: &[Reloc], leads: bool, clip: (u64, u64), after: After<'_>) -> Written {
    let (a, b) = clip;
    let len = usize::try_from(b - a).unwrap();
    let mut fills = [vec![0u8; len], vec![0xFFu8; len]];
    let cx = FillCx { tables: c.tables, output: c.output, got_base: c.got_base, tls: c.tls };
    for fill in &mut fills {
        for (i, rec) in relocs.iter().enumerate() {
            let h = c.howto(rec.r_type, rec.r_offset);
            let at = RelocAt {
                file: FileId(1),
                section: SectionId(3),
                index: u32::try_from(i).unwrap(),
                r_offset: rec.r_offset,
                key: rec.key,
                r_type: rec.r_type,
                addend: rec.addend,
                howto: &h,
                site: Site { bytes: c.input, offset: rec.r_offset },
                records: &relocs,
                proof: &c.proof,
                leads,
            };
            let mut out = SectionOut::new(fill, a, c.addr, c.alloc, c.name);
            assert_eq!(out.clip(), clip);
            c.arch.apply(&cx, &at, &mut out)?;
        }
        let mut out = SectionOut::new(fill, a, c.addr, c.alloc, c.name);
        for (off, data) in after {
            out.write(*off, data);
        }
    }
    let [zero, ones] = fills;
    Ok(zero
        .iter()
        .zip(&ones)
        .map(|(&z, &o)| {
            // A written byte holds its value in both fills.
            assert!(z == o || (z, o) == (0, 0xFF));
            (z == o).then_some(z)
        })
        .collect())
}

/// The R-2 property for one section: every clip pair `a ≤ b` gives the
/// unclipped result's restriction (and the same error); the three clips
/// `[0, a)`, `[a, b)`, `[b, len)` compose to it. Returns the unclipped
/// result.
fn check_clips(c: &Case<'_>, relocs: &[Reloc], after: After<'_>, what: &str) -> Written {
    let len = c.input.len() as u64;
    let whole = run(c, relocs, true, (0, len), after);
    for a in 0..=len {
        for b in a..=len {
            let got = run(c, relocs, true, (a, b), after);
            let want = whole.as_ref().map(|w| w[a as usize..b as usize].to_vec());
            assert_eq!(got, want.map_err(|e| *e), "{what}: clip [{a:#x}, {b:#x})");
        }
    }
    whole
}

/// The bytes of a result over the input.
fn image(input: &[u8], w: &[Option<u8>]) -> Vec<u8> {
    input.iter().zip(w).map(|(&i, &o)| o.unwrap_or(i)).collect()
}

/// Asserts that record `i`'s own writes lie inside its `max_reach`
/// window and its unit's window.
fn check_reach(c: &Case<'_>, relocs: &[Reloc], i: usize, what: &str) {
    // Applied alone, with its section's records (a member sees its
    // sequence).
    let rec = relocs[i];
    let h = c.howto(rec.r_type, rec.r_offset);
    let at = RelocAt {
        file: FileId(1),
        section: SectionId(3),
        index: u32::try_from(i).unwrap(),
        r_offset: rec.r_offset,
        key: rec.key,
        r_type: rec.r_type,
        addend: rec.addend,
        howto: &h,
        site: Site { bytes: c.input, offset: rec.r_offset },
        records: &relocs,
        proof: &c.proof,
        leads: true,
    };
    let cx = FillCx { tables: c.tables, output: c.output, got_base: c.got_base, tls: c.tls };
    let mut written = vec![false; c.input.len()];
    for fill in [0u8, 0xFF] {
        let mut buf = vec![fill; c.input.len()];
        let mut out = SectionOut::whole(&mut buf, c.addr, c.alloc, c.name);
        if c.arch.apply(&cx, &at, &mut out).is_err() {
            return;
        }
        for (w, &v) in written.iter_mut().zip(&buf) {
            *w |= v != fill;
        }
    }
    let reach = c.arch.facts().max_reach;
    let lo = rec.r_offset - u64::from(reach.before);
    let hi = rec.r_offset + u64::from(h.field.width()) + u64::from(reach.after);
    let unit = c.arch.unit_window(c.output, &at, c.tables).unwrap();
    for (off, _) in written.iter().enumerate().filter(|(_, w)| **w) {
        let off = off as u64;
        assert!(lo <= off && off < hi, "{what}: record {i} wrote {off:#x} outside max_reach");
        assert!(
            unit.start <= off && off < unit.end,
            "{what}: record {i} wrote {off:#x} outside its unit {unit:?}"
        );
    }
}

fn tables() -> Tables {
    let mut t = Tables::default();
    // key 1: an ordinary local; key 2: a thread-local; key 9: the
    // undefined `__tls_get_addr`.
    t.sym(key(1), 0x41_2340, TargetKind::Local);
    t.tls_sym(key(2), 0x42_0040, TargetKind::Local);
    // key 3: a small absolute value; key 4: near the section.
    t.sym(key(3), 0x100, TargetKind::Abs);
    t.sym(key(4), ADDR + 0x100, TargetKind::Local);
    t.sym(key(9), 0, TargetKind::Undefined);
    t.names.insert(key(9), b"__tls_get_addr".to_vec());
    for (k, base) in [(1, 0x43_0000), (2, 0x43_0100)] {
        for (n, kind) in [
            SlotKind::Addr,
            SlotKind::TpOff { negated: false },
            SlotKind::TpOff { negated: true },
            SlotKind::PltGot,
            SlotKind::TlsGd,
            SlotKind::TlsLd,
            SlotKind::TlsDesc,
        ]
        .into_iter()
        .enumerate()
        {
            t.slots.insert((key(k), kind), base + 16 * n as u64);
        }
    }
    t.plts.insert(key(1), 0x40_0100);
    t
}

/// Input words a relocation of some type accepts.
const CANDIDATES: [u32; 28] = [
    0x0000_0000,
    0x9000_0000, // adrp x0
    0x1000_0000, // adr x0
    0x9100_0000, // add x0, x0, #0
    0x9140_0000, // add x0, x0, #0, lsl #12
    0x3940_0000, // ldrb w0, [x0]
    0x7940_0000, // ldrh w0, [x0]
    0xB940_0000, // ldr w0, [x0]
    0xF940_0000, // ldr x0, [x0]
    0x3DC0_0000, // ldr q0, [x0]
    0x1400_0000, // b
    0x9400_0000, // bl
    0x5400_0000, // b.eq
    0xB400_0000, // cbz x0
    0x3600_0000, // tbz w0, #0
    0x5800_0000, // ldr x0, literal
    0xD280_0000, // movz x0, lsl #0, 16, 32, 48
    0xD2A0_0000,
    0xD2C0_0000,
    0xD2E0_0000,
    0x9280_0000, // movn x0
    0x92A0_0000,
    0x92C0_0000,
    0x92E0_0000,
    0xF280_0000, // movk x0
    0xF2A0_0000,
    0xF2C0_0000,
    0xF2E0_0000,
];

fn case<'a>(t: &'a Tables, input: &'a [u8], output: OutputKind) -> Case<'a> {
    let mut c = Case::new(t, input, ADDR);
    c.arch = M2;
    c.output = output;
    c.tls = Some(TLS);
    c.got_base = Some(0x43_0000);
    c
}

/// Every accepted type, on every candidate word, against an ordinary and
/// a thread-local target, in each output kind: the clip property, the
/// reach, and `leads = false`.
#[test]
fn s3_6_clip_every_type() {
    let t = tables();
    let mut checked = 0usize;
    let mut accepted = 0usize;
    let mut none = Vec::new();
    for r_type in 0..=MAX_TYPE {
        let probe = words(&[0; 6]);
        if M2.classify(r_type, Site { bytes: &probe, offset: 8 }).is_err() {
            continue;
        }
        accepted += 1;
        let mut ok = false;
        for output in [OutputKind::Static, OutputKind::Pie, OutputKind::Shared] {
            for w in CANDIDATES {
                // The field at offset 8 of six words: before and after
                // it there are words to (not) write.
                let input = words(&[w; 6]);
                let c = case(&t, &input, output);
                if c.arch.classify(r_type, Site { bytes: &input, offset: 8 }).is_err() {
                    continue;
                }
                for k in [1, 2, 3, 4] {
                    let relocs = [r(8, k, r_type)];
                    let what = format!("type {r_type} on {w:#010x}, key {k}, {output:?}");
                    let whole = check_clips(&c, &relocs, &[], &what);
                    if let Ok(whole) = whole {
                        ok = true;
                        checked += 1;
                        check_reach(&c, &relocs, 0, &what);
                        if r_type != 513 {
                            let len = input.len() as u64;
                            assert_eq!(
                                run(&c, &relocs, false, (0, len), &[]),
                                Ok(whole),
                                "{what}: leads = false"
                            );
                        }
                    }
                }
            }
        }
        if !ok {
            none.push(r_type);
        }
    }
    eprintln!("{accepted} accepted types, {checked} cases");
    assert!(none.is_empty(), "types applied on no candidate: {none:?}");
    assert!(accepted > 100, "{accepted} accepted types");
    assert!(checked > accepted, "{checked} cases");
}

/// A named section: its tables, output kind, words and records.
type Seq<'a> = (&'a str, &'a Tables, OutputKind, &'a [u32], &'a [Reloc]);

/// The relaxation sequences of §8 and the erratum's in-place fix.
#[test]
fn s3_6_clip_sequences_and_erratum() {
    let t = tables();
    let blr_x1 = 0xD63F_0020;
    let desc = [0x9000_0000, 0xF940_0001, 0x9100_0000, blr_x1];
    let desc_r = [r(0, 2, 562), r(4, 2, 563), r(8, 2, 564), r(12, 2, 569)];
    let tiny = [0x5800_0001, 0x1000_0000, blr_x1];
    let tiny_r = [r(0, 2, 560), r(4, 2, 561), r(8, 2, 569)];
    let ie = [0x9000_0005, 0xF940_00A5];
    let ie_r = [r(0, 2, 541), r(4, 2, 542)];
    let gd = [0x9000_0000, 0x9100_0000, 0x9400_0000, NOP];
    let gd_r = [r(0, 2, 513), r(4, 2, 514), r(8, 9, 283)];
    let got = [0x9000_0003, 0xF940_0063];
    let got_r = [r(0, 1, 311), r(4, 1, 312)];
    let mut shlib = tables();
    shlib.tls_sym(key(2), 0x42_0040, TargetKind::Shlib { func: false });
    let seqs: [Seq<'_>; 9] = [
        ("R1 TLSDESC to LE", &t, OutputKind::Static, &desc, &desc_r),
        ("R1 tiny TLSDESC to LE", &t, OutputKind::Static, &tiny, &tiny_r),
        ("R2 TLSDESC to IE", &shlib, OutputKind::Exec, &desc, &desc_r),
        ("R2 tiny TLSDESC to IE", &shlib, OutputKind::Pie, &tiny, &tiny_r),
        ("R3 IE to LE", &t, OutputKind::Static, &ie, &ie_r),
        ("R4 GD to LE", &t, OutputKind::Static, &gd, &gd_r),
        ("R4 GD to IE", &shlib, OutputKind::Exec, &gd, &gd_r),
        ("R5 GOT to address", &t, OutputKind::Static, &got, &got_r),
        ("TLSDESC kept (shared)", &t, OutputKind::Shared, &desc, &desc_r),
    ];
    for (what, tables, output, ws, relocs) in seqs {
        // A word of padding on each side.
        let mut padded = vec![NOP];
        padded.extend_from_slice(ws);
        padded.push(NOP);
        let input = words(&padded);
        let relocs: Vec<Reloc> =
            relocs.iter().map(|x| Reloc { r_offset: x.r_offset + 4, ..*x }).collect();
        let c = case(tables, &input, output);
        let whole = check_clips(&c, &relocs, &[], what).unwrap_or_else(|e| panic!("{what}: {e:?}"));
        assert_ne!(image(&input, &whole), input, "{what}: nothing written");
        for i in 0..relocs.len() {
            check_reach(&c, &relocs, i, what);
        }
        if !relocs.iter().any(|x| x.r_type == 513) {
            let len = input.len() as u64;
            assert_eq!(run(&c, &relocs, false, (0, len), &[]), Ok(whole), "{what}: leads");
        }
    }

    // Erratum 843419 (a64.md §11.2): adrp at Q = 0xff8 (mod 4 KiB), a
    // store, a load through the ADRP's register at F. The fix words are
    // the image's writes after the relocations: the ADR over W1, or the
    // branch to a patch at F.
    let site = [NOP, 0x9000_0000, 0xF900_0041, NOP, 0xF940_0003, NOP];
    let input = words(&site);
    let relocs = [r(4, 1, 275), r(16, 1, 286)];
    let c = case(&t, &input, OutputKind::Static);
    let len = input.len() as u64;
    let applied = image(&input, &run(&c, &relocs, true, (0, len), &[]).unwrap());
    let q = 4u64;
    let w1 = u32::from_le_bytes(applied[4..8].try_into().unwrap());
    let adr = erratum_adr(c.addr + q, w1).expect("in ADR range");
    let b = erratum_branch(c.addr + 16, c.addr + 0x1000).unwrap();
    for (what, after) in [
        ("erratum ADR fix", vec![(q, adr.to_le_bytes().to_vec())]),
        ("erratum patch branch", vec![(16, b.to_le_bytes().to_vec())]),
    ] {
        let whole = check_clips(&c, &relocs, &after, what).unwrap();
        let (off, data) = &after[0];
        let o = usize::try_from(*off).unwrap();
        assert_eq!(&image(&input, &whole)[o..o + 4], &data[..], "{what}");
    }
}

#[test]
fn s3_5_sequence_extent() {
    let a: &dyn Arch = &M2;
    let ext = |rs: &[Reloc], i| a.sequence_extent(&rs, i);
    let gd = [r(0, 2, 513), r(4, 2, 514), r(8, 9, 283)];
    assert_eq!((ext(&gd, 0), ext(&gd, 1), ext(&gd, 2), ext(&gd, 3)), (3, 1, 1, 1));
    // A broken GD: no follower, or the wrong one.
    assert_eq!(ext(&gd[..2], 0), 1);
    assert_eq!(ext(&[r(0, 2, 513), r(4, 2, 514), r(8, 9, 282)], 0), 1);
    let got = [r(0, 1, 311), r(4, 1, 312)];
    assert_eq!((ext(&got, 0), ext(&got, 1)), (2, 1));
    assert_eq!(ext(&got[..1], 0), 1);
    // Every other single record: 1.
    for t in 0..=MAX_TYPE {
        assert_eq!(ext(&[r(0, 1, t)], 0), 1, "{t}");
    }
    // The extent covers each recognized sequence's consumed members.
    let input = words(&[0x9000_0000, 0x9100_0000, 0x9400_0000, NOP]);
    let tbl = tables();
    let c = case(&tbl, &input, OutputKind::Static);
    for i in 0..3u32 {
        let rec = gd[i as usize];
        let h = c.howto(rec.r_type, rec.r_offset);
        let at = RelocAt {
            file: FileId(1),
            section: SectionId(3),
            index: i,
            r_offset: rec.r_offset,
            key: rec.key,
            r_type: rec.r_type,
            addend: 0,
            howto: &h,
            site: Site { bytes: &input, offset: rec.r_offset },
            records: &&gd[..],
            proof: &c.proof,
            leads: true,
        };
        let unit = a.unit_window(c.output, &at, &tbl).unwrap();
        assert!(unit.unit + ext(&gd, unit.unit) > i, "record {i} outside its lead's extent");
    }
}

/// `Facts.max_reach` is derived (I-111): the maximum over every window
/// `classify` gives, over the whole type range, and the member windows.
#[test]
fn s3_1_max_reach_derived() {
    let input = words(&[0; 4]);
    let mut w = Window { before: 0, after: 0 };
    for t in 0..=0xFFFF {
        if let Ok(h) = M2.classify(t, Site { bytes: &input, offset: 4 }) {
            assert!(t <= MAX_TYPE, "type {t} above MAX_TYPE");
            w = max_window(w, h.window);
        }
    }
    for m in MEMBER_WINDOWS {
        w = max_window(w, m);
    }
    assert_eq!(w, MAX_REACH);
    assert_eq!(M2.facts().max_reach, MAX_REACH);
    assert_eq!((MAX_REACH.before, MAX_REACH.after), (0, 4));
}
