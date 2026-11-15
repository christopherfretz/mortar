//! Erratum 843419 against GNU ld and lld (a64.md §11.1; WP-023 stage A
//! test (ii)).
//!
//! Generated sequences, one per 4 KiB page, are assembled into one input
//! section and linked by each reference with and without
//! `--fix-cortex-a53-843419`. The sites a reference fixed are the words
//! that differ: a final word turned into `b` (a patch) or an ADRP turned
//! into ADR. mortar's predicate runs over the unfixed image's `.text`
//! with the proof the image's mapping symbols give. It must find a
//! superset of every reference's sites, and no site with a word in a `$d`
//! range.

use std::collections::BTreeSet;
use std::fmt::Write as _;

use mortar_a64::A64;
use mortar_a64::mapping::{MapSym, mapping_kind, proof_runs};
use mortar_arch::{Arch, Code, Erratum, ErratumSite};
use mortar_testutil::{Scratch, run, tool};

mod elf;
use elf::{Image, u32_at, u64_at};

/// Loads and stores of every group (condition 2), and two words outside
/// the group.
const W2: &[u32] = &[
    0xF940_0041, // ldr x1, [x2]
    0xB900_0BE3, // str w3, [sp, #8]
    0xA940_0861, // ldp x1, x2, [x3]
    0xA900_0861, // stp x1, x2, [x3]
    0xF863_6841, // ldr x1, [x2, x3]
    0xC85F_7C41, // ldxr x1, [x2]
    0xC8DF_FC41, // ldar x1, [x2]
    0x4C40_7020, // ld1 {v0.16b}, [x1]
    0xF980_0020, // prfm pldl1keep, [x1]
    0x5800_0041, // ldr x1, .+8
    0xF820_0041, // ldadd x0, x1, [x2]
    0xF840_0441, // ldr x1, [x2], #0
    0x8B03_0041, // add x1, x2, x3 (not a load/store)
    0x1400_0002, // b .+8 (not a load/store)
];

/// The third word of the 4-word form.
const W3: &[u32] = &[0xD503_201F, 0x8B03_0041, 0xF940_0041, 0x1400_0001, 0xD65F_03C0];

/// The final word for base `n`: loads and stores (unsigned immediate) of
/// each size with base `Xn`, `Rt = n`, and three that are not a site.
fn finals(n: u32) -> [u32; 9] {
    [
        0xF940_0401 | n << 5,              // ldr x1, [xn, #8]
        0xB900_0002 | n << 5,              // str w2, [xn]
        0x3940_0003 | n << 5,              // ldrb w3, [xn]
        0x3DC0_0400 | n << 5,              // ldr q0, [xn, #16]
        0xF980_0000 | n << 5,              // prfm pldl1keep, [xn]
        0xF940_0000 | n << 5 | n,          // ldr xn, [xn]
        0xF840_0001 | n << 5,              // ldur x1, [xn] (not unsigned-immediate)
        0xF940_0401 | ((n + 1) % 31) << 5, // another base
        0xD503_201F,                       // nop
    ]
}

struct Gen(u64);

impl Gen {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn pick<T: Copy>(&mut self, xs: &[T]) -> T {
        xs[(self.next() % xs.len() as u64) as usize]
    }
}

const BLOCKS: usize = 640;

/// The assembly, and the page offsets of the sequences written as data.
fn generate() -> (String, usize) {
    let mut g = Gen(0x843_419);
    let mut s = String::from("\t.text\n\t.globl _start\n_start:\n");
    let mut data_blocks = 0;
    for _ in 0..BLOCKS {
        let at = match g.next() % 10 {
            0 => 0xFF4,
            1 => 0x000,
            n if n < 6 => 0xFF8,
            _ => 0xFFC,
        };
        let n = (g.next() % 31) as u32;
        let words = if g.next().is_multiple_of(2) { 3 } else { 4 };
        let w2 = g.pick(W2);
        let w3 = g.pick(W3);
        let f = g.pick(&finals(n));
        // Which words are written as data: none mostly.
        let data_mask = match g.next() % 8 {
            0 => 0b1111,
            1 => 1 << (g.next() % words),
            _ => 0,
        };
        if data_mask != 0 {
            data_blocks += 1;
        }
        writeln!(s, "\t.p2align 12\n\t.skip {at:#x}").unwrap();
        let seq: Vec<u32> = if words == 3 { vec![0, w2, f] } else { vec![0, w2, w3, f] };
        for (i, w) in seq.iter().enumerate() {
            let data = data_mask >> i & 1 == 1;
            match (i, data) {
                (0, false) => writeln!(s, "\tadrp x{n}, tgt").unwrap(),
                (0, true) => writeln!(s, "\t.word {:#x}", 0x9000_0000 | n).unwrap(),
                (_, false) => writeln!(s, "\t.inst {w:#x}").unwrap(),
                (_, true) => writeln!(s, "\t.word {w:#x}").unwrap(),
            }
        }
        writeln!(s, "\tnop").unwrap();
    }
    s.push_str("\t.data\n\t.globl tgt\ntgt:\t.xword 0\n");
    (s, data_blocks)
}

struct Text {
    addr: u64,
    bytes: Vec<u8>,
    mapping: Vec<MapSym>,
}

fn text(img: &Image) -> Text {
    let (idx, sec) = img.sections.iter().enumerate().find(|(_, s)| s.name == ".text").unwrap();
    let mut mapping = Vec::new();
    if let Some(st) = img.sections.iter().find(|s| s.kind == elf::SHT_SYMTAB) {
        let names = img.data(&img.sections[st.link as usize]);
        for (i, e) in img.data(st).chunks_exact(24).enumerate() {
            if usize::from(elf::u16_at(e, 6)) != idx {
                continue;
            }
            let n = &names[u32_at(e, 0) as usize..];
            let name = &n[..n.iter().position(|&b| b == 0).unwrap()];
            if let Some(kind) = mapping_kind(name, e[4]) {
                mapping.push(MapSym { index: i as u32, value: u64_at(e, 8) - sec.addr, kind });
            }
        }
    }
    Text { addr: sec.addr, bytes: img.data(sec).to_vec(), mapping }
}

/// The fixed sites: final-word offsets patched, and ADRP offsets turned
/// into ADR.
fn fixed(plain: &Text, fixed: &Text) -> (BTreeSet<u64>, BTreeSet<u64>) {
    assert_eq!(plain.addr, fixed.addr);
    let mut patched = BTreeSet::new();
    let mut adr = BTreeSet::new();
    let n = plain.bytes.len().min(fixed.bytes.len());
    for off in (0..n).step_by(4) {
        let a = u32_at(&plain.bytes, off);
        let b = u32_at(&fixed.bytes, off);
        // An ADRP to `tgt` moves with the data when the fix grows `.text`.
        if a == b || (a & 0x9F00_001F == b & 0x9F00_001F && a & 0x9F00_0000 == 0x9000_0000) {
            continue;
        }
        if b & 0xFC00_0000 == 0x1400_0000 {
            patched.insert(off as u64);
        } else if a & 0x9F00_0000 == 0x9000_0000 && b & 0x9F00_0000 == 0x1000_0000 {
            adr.insert(off as u64);
        } else {
            panic!("unexpected change at {off:#x}: {a:#010x} -> {b:#010x}");
        }
    }
    (patched, adr)
}

#[test]
fn s11_1_superset_of_reference_linkers() {
    let (Some(gcc), Some(bfd), Some(lld)) = (tool("gcc"), tool("ld.bfd"), tool("ld.lld")) else {
        return;
    };
    let dir = Scratch::new("a64-843419");
    let (src, data_blocks) = generate();
    let s = dir.write("seq.s", src);
    let obj = dir.join("seq.o");
    let o = run(&gcc, [std::ffi::OsStr::new("-c"), s.as_os_str(), "-o".as_ref(), obj.as_os_str()]);
    assert!(o.success(), "{}", o.stderr_text());

    let link = |linker: &std::path::Path, name: &str, fix: bool| {
        let out = dir.join(format!("{name}-{fix}"));
        // Off is both linkers' default when they are run directly.
        let mut args =
            vec!["-e".to_string(), "_start".into(), "-o".into(), out.display().to_string()];
        if fix {
            args.push("--fix-cortex-a53-843419".into());
        }
        args.push(obj.display().to_string());
        let o = run(linker, &args);
        assert!(o.success(), "{name}: {}", o.stderr_text());
        text(&Image::read(&out))
    };

    let mut lines = Vec::new();
    for (name, path) in [("bfd", &bfd), ("lld", &lld)] {
        let plain = link(path, name, false);
        let fix = link(path, name, true);
        let (patched, adr) = fixed(&plain, &fix);

        let mut syms = plain.mapping.clone();
        let mut proof = Vec::new();
        proof_runs(&mut syms, plain.bytes.len() as u64, true, 0, 0, &mut proof);
        let code = Code { bytes: &plain.bytes, addr: plain.addr, proof: &proof };
        let mut sites: Vec<ErratumSite> = Vec::new();
        A64::new(mortar_arch::Status::M1, true)
            .erratum_sites(Erratum::E843419, &code, &mut sites)
            .expect("every word has a mapping symbol");
        let qs: BTreeSet<u64> = sites.iter().map(|s| s.q).collect();
        let fs: BTreeSet<u64> = sites.iter().map(|s| s.f).collect();

        // Superset: every fix the reference made is at a mortar site.
        let missed_f: Vec<_> = patched.difference(&fs).collect();
        let missed_q: Vec<_> = adr.difference(&qs).collect();
        assert!(missed_f.is_empty(), "{name} patched final words mortar does not: {missed_f:x?}");
        assert!(missed_q.is_empty(), "{name} fixed ADRPs mortar does not: {missed_q:x?}");

        // Data is never a site: no word of a site is in a `$d` range.
        for s in &sites {
            for at in (s.q..=s.f).step_by(4) {
                assert_eq!(code.word_proof(at).0, mortar_arch::Proof::Insn, "{name}: site {s:x?}");
            }
        }
        let reference: BTreeSet<u64> = sites
            .iter()
            .filter(|s| patched.contains(&s.f) || adr.contains(&s.q))
            .map(|s| s.q)
            .collect();
        lines.push(format!(
            "{name}: {} fixed by the reference ({} patches, {} ADR), {} mortar sites, {} mortar-only",
            patched.len() + adr.len(),
            patched.len(),
            adr.len(),
            sites.len(),
            sites.len() - reference.len(),
        ));
    }
    eprintln!("843419: {BLOCKS} generated sequences, {data_blocks} with data words");
    for l in &lines {
        eprintln!("843419: {l}");
    }
}
