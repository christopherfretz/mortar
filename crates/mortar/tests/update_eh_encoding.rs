//! WP-225 M2b-5 item 3 (IN-34a; ruling "After M2b-4's hand-back" 3): the
//! refill's `.eh_frame` walk (`mortar_image::update::walk_eh`) and move
//! (`patch_eh`) on the CIE encodings real compilers state, a kiln/hod
//! object (hod's AArch64 CIEs: `zPLR`, `P` 0x9c indirect pcrel sdata8, `L`
//! 0x1c pcrel sdata8, `R` 0x1b; its x86-64 ones `P` 0x9b, `L` and `R`
//! 0x1b) and a GCC one (`P` 0x9b, `L` and `R` 0x1b), on AArch64 and
//! x86-64.
//!
//! The fields are found by this test's own reading of the records (the
//! CIE's augmentation string and data, each FDE's `pc_begin` and LSDA
//! pointer), each given a value naming a target in a "code" or "data"
//! range; the section and both ranges move by different amounts, and
//! every field after `patch_eh`'s patches must name its target where it
//! moved. An encoding IN-34a does not move is refused, by its byte
//! (`walk_eh_why`).

use std::process::Command;
use std::time::Duration;

use mortar_core::incr::{IncrReason, Unimplemented};
use mortar_image::update::{Counts, RefillError, Shift, patch_eh, walk_eh, walk_eh_why};
use mortar_testutil::target::Arch;
use mortar_testutil::{Scratch, run_with, tools_required};

const ARCHES: [Arch; 2] = [Arch::Aarch64, Arch::X86_64];

/// `gc_eh_owner_dies`' `b.cc`: a function with a try/catch (a personality,
/// an LSDA).
const SRC: &str = "int thrower(int x) {\n  try {\n    if (x > 100) throw x;\n    return x;\n  } \
                   catch (int e) {\n    return -e;\n  }\n}\n";

/// The object's `.eh_frame` bytes, compiled by kiln/hod (`kiln`) or g++.
fn eh_of(arch: Arch, dir: &Scratch, kiln: bool) -> Option<Vec<u8>> {
    let mut cmd = if kiln {
        let k = std::env::var_os("MASON_KILN")?;
        let h = std::env::var_os("MASON_HOD")?;
        let mut c = Command::new(k);
        c.env("KILN_HOD", h).arg("-fkiln-backend=hod").arg(format!("--target={}", arch.triple()));
        c
    } else {
        let mut c = Command::new(arch.tool("g++")?);
        c.args(["-O1", "-fexceptions"]);
        c
    };
    let name = if kiln { "k" } else { "g" };
    let src = dir.write(&format!("{name}.cc"), SRC);
    let obj = dir.join(format!("{name}.o"));
    cmd.args(["-c".as_ref(), src.as_os_str(), "-o".as_ref(), obj.as_os_str()]);
    let o = run_with(&mut cmd, b"", Duration::from_mins(2));
    assert!(o.success(), "{arch:?} {name}: {}", o.stderr_text());
    let bytes = std::fs::read(&obj).unwrap();
    let elf = mortar_elf::Object::parse(&bytes).unwrap();
    let eh = elf.sections().find(|s| s.name().ok() == Some(b".eh_frame".as_slice()))?;
    let r = eh.file_range().unwrap().unwrap();
    Some(bytes[r].to_vec())
}

fn le(b: &[u8], at: usize, n: usize) -> u64 {
    b[at..at + n].iter().rev().fold(0, |a, &x| a << 8 | u64::from(x))
}

fn uleb(b: &[u8], at: &mut usize) -> u64 {
    let (mut v, mut s) = (0u64, 0);
    loop {
        let x = b[*at];
        *at += 1;
        v |= u64::from(x & 0x7f) << s;
        if x & 0x80 == 0 {
            return v;
        }
        s += 7;
    }
}

/// A `zPLR`-style CIE as this test reads it: the offset of each encoding
/// byte by its letter, and the encodings.
#[derive(Debug, Clone)]
struct Cie {
    at: usize,
    enc: Vec<(u8, usize, u8)>,
    personality: Option<(usize, usize)>,
}

fn width(enc: u8) -> usize {
    match enc & 0x0f {
        0x0b => 4,
        0x0c => 8,
        e => panic!("an encoding this test does not read: {e:#x}"),
    }
}

/// Every PC-relative field of the records: `(offset, width)`, and the CIEs.
fn fields(b: &[u8]) -> (Vec<(usize, usize)>, Vec<Cie>) {
    let mut cies: Vec<Cie> = Vec::new();
    let mut out = Vec::new();
    let mut at = 0;
    while at + 4 <= b.len() {
        let len = le(b, at, 4) as usize;
        if len == 0 {
            break;
        }
        let end = at + 4 + len;
        if le(b, at + 4, 4) == 0 {
            let mut p = at + 9;
            let z = p + b[p..].iter().position(|&x| x == 0).unwrap();
            let aug = b[p..z].to_vec();
            p = z + 1;
            uleb(b, &mut p);
            uleb(b, &mut p);
            if b[at + 8] == 1 {
                p += 1;
            } else {
                uleb(b, &mut p);
            }
            let mut c = Cie { at, enc: Vec::new(), personality: None };
            if aug.first() == Some(&b'z') {
                uleb(b, &mut p);
                for &l in &aug[1..] {
                    if l == b'S' {
                        continue;
                    }
                    let e = b[p];
                    c.enc.push((l, p, e));
                    p += 1;
                    if l == b'P' {
                        c.personality = Some((p, width(e)));
                        out.push((p, width(e)));
                        p += width(e);
                    }
                }
            }
            cies.push(c);
        } else {
            let cie = at + 4 - le(b, at + 4, 4) as usize;
            let c = cies.iter().find(|c| c.at == cie).unwrap();
            let of = |l: u8| c.enc.iter().find(|e| e.0 == l).map(|e| width(e.2));
            let w = of(b'R').expect("an FDE of a CIE with R");
            out.push((at + 8, w));
            if let Some(lw) = of(b'L') {
                let mut p = at + 8 + 2 * w;
                if uleb(b, &mut p) as usize >= lw {
                    out.push((p, lw));
                }
            }
        }
        at = end;
    }
    (out, cies)
}

fn put(b: &mut [u8], at: usize, w: usize, v: i64) {
    b[at..at + w].copy_from_slice(&v.to_le_bytes()[..w]);
}

fn get(b: &[u8], at: usize, w: usize) -> i64 {
    let v = le(b, at, w);
    if w == 4 { i64::from(v as u32 as i32) } else { v as i64 }
}

/// The walk, the move and the refusal on one object's `.eh_frame`.
fn judge(arch: Arch, label: &str, eh: &[u8], wide: bool) {
    let (fl, cies) = fields(eh);
    let z = cies.iter().find(|c| c.personality.is_some()).expect("a CIE with P");
    let enc = |l: u8| z.enc.iter().find(|e| e.0 == l).map(|e| e.2);
    if wide {
        assert_eq!(
            (enc(b'P'), enc(b'L'), enc(b'R')),
            (Some(0x9c), Some(0x1c), Some(0x1b)),
            "{arch:?} {label}: hod's AArch64 CIE"
        );
    } else {
        assert_eq!(
            (enc(b'P'), enc(b'L'), enc(b'R')),
            (Some(0x9b), Some(0x1b), Some(0x1b)),
            "{arch:?} {label}: an sdata4 CIE"
        );
    }
    // Each field names a target: a "code" target for an FDE's pc_begin,
    // a "data" one (the personality's pointer, the LSDA) otherwise.
    let (old, new) = (0x40_0000u64, 0x40_2000u64);
    let code = 0x1_0000u64..0x2_0000;
    let data = 0x3_0000u64..0x3_1000;
    let mut b = eh.to_vec();
    let mut want = Vec::new();
    for (k, &(at, w)) in fl.iter().enumerate() {
        let t = if k % 2 == 0 { code.start } else { data.start } + 16 * k as u64;
        put(&mut b, at, w, t.wrapping_sub(old + at as u64) as i64);
        want.push(t);
    }
    let mut shift = Shift::default();
    shift.addrs = vec![
        (code.clone(), 0x1000),
        (data.clone(), 0x3000),
        (old..old + eh.len() as u64, (new - old) as i64),
    ];
    let mut counts = Counts::default();
    let records = walk_eh(&b, &mut counts).unwrap_or_else(|e| {
        panic!("{arch:?} {label}: walk_eh: {e:?}: {:?}", walk_eh_why(&b));
    });
    assert_eq!(walk_eh_why(&b), None);
    let patches = patch_eh(&b, &records, old, new, &shift, &mut counts).unwrap();
    let mut moved = b.clone();
    for (at, p) in &patches {
        moved[*at as usize..*at as usize + 4].copy_from_slice(p);
    }
    for (&(at, w), &t) in fl.iter().zip(&want) {
        let named = (new + at as u64).wrapping_add(get(&moved, at, w) as u64);
        assert_eq!(named, shift.map(t), "{arch:?} {label}: the {w}-byte field at {at:#x}");
    }
    assert_eq!(counts.eh_patched as usize, fl.len(), "{arch:?} {label}: every field patched");
    assert_eq!(fl.iter().any(|f| f.1 == 8), wide, "{arch:?} {label}: an 8-byte field");
    // Not moved: an absolute sdata8 personality, an udata4 FDE encoding.
    for (l, bad) in [(b'P', 0x0cu8), (b'R', 0x03)] {
        let at = z.enc.iter().find(|e| e.0 == l).unwrap().1;
        let mut x = b.clone();
        x[at] = bad;
        assert!(
            matches!(
                walk_eh(&x, &mut counts),
                Err(RefillError::Fallback(IncrReason::UpdateNotImplemented(Some(
                    Unimplemented::Eh
                ))))
            ),
            "{arch:?} {label}: {} {bad:#x}",
            char::from(l)
        );
        let why = walk_eh_why(&x).unwrap();
        let named = format!("CIE at {:#x} {} encoding {bad:#04x}", z.at, char::from(l));
        assert_eq!(why, named, "{arch:?} {label}");
    }
}

// spec: incr.md IN-34a (the pcrel encodings a CIE states move with the
// code); ruling "After M2b-4's hand-back" 3.
#[test]
fn walk_eh_moves_hod_and_gcc_pcrel_encodings() {
    let mut ran = 0;
    for arch in ARCHES {
        let dir = Scratch::new(&format!("eh-enc-{arch:?}"));
        if let Some(eh) = eh_of(arch, &dir, false) {
            judge(arch, "g++", &eh, false);
            ran += 1;
        }
        match eh_of(arch, &dir, true) {
            Some(eh) => {
                judge(arch, "kiln/hod", &eh, arch == Arch::Aarch64);
                ran += 1;
            }
            None => assert!(
                !tools_required() || std::env::var_os("MASON_KILN").is_none(),
                "{arch:?}: kiln/hod named but no object"
            ),
        }
    }
    eprintln!("walk_eh_moves_hod_and_gcc_pcrel_encodings: {ran} objects");
}
