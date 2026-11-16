//! `.gdb_index` (`spec/plumb.md` §6.10, P-G1 to P-G9; `spec/synth.md`
//! §17), on aarch64, x86-64 and i386.
//!
//! Three hand-written objects (`a`, `b`, `c`): one function each, a DWARF 4
//! unit with a child DIE (S-G3a: debug-bearing) whose `low_pc`/`high_pc`
//! state the function, and GNU name sets whose names, attributes and order
//! the test knows. lld links them with `--gdb-index`; its output, judged as
//! lld's, must give no P-G finding (the format rules accept a correct
//! reference). Its index replaced by the bytes S-G7 gives for the sets
//! (written here from the spec, not from the verifier) and its map read
//! as mortar's with the `.gdb_index` chunk, it is modelled as mortar's
//! output: clean of P-G findings; each mutation of it is rejected by the
//! rule it names. The silent `gdb-index-incomplete`: `c` without its sets
//! and an index built from `a` and `b` alone, everything else consistent,
//! is P-G7.

#![allow(clippy::many_single_char_names)] // byte models: b, h, m, t, w

mod common;

use std::ffi::OsString;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use common::verify_with;
use mortar_elf::consts::SHF_ALLOC;
use mortar_elf::{FileHeader, SectionTable};
use mortar_plumb::map::{Chunk, ChunkKind, NonAlloc, SectionRef};
use mortar_plumb::{Format, Map};
use mortar_testutil::{Scratch, run, tool};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Arch {
    Aarch64,
    X86_64,
    I386,
}

impl Arch {
    fn triple(self) -> &'static str {
        match self {
            Arch::Aarch64 => "aarch64-linux-gnu",
            Arch::X86_64 => "x86_64-linux-gnu",
            Arch::I386 => "i686-linux-gnu",
        }
    }
    fn w(self) -> u8 {
        if self == Arch::I386 { 4 } else { 8 }
    }
}

/// A unit's sets: (attribute byte, name), pubnames then pubtypes.
type Sets = (&'static [(u8, &'static str)], &'static [(u8, &'static str)]);

/// The units' sets. `main_a` and `Main_a` share a hash (gdb's hash folds
/// case), `shared` and `T_a` occur in more than one unit.
const SETS: [(&str, Sets); 3] = [
    ("a", (&[(0x30, "main_a"), (0x30, "shared"), (0x20, "ns::Var")], &[(0x10, "T_a")])),
    ("b", (&[(0x30, "fn_b"), (0x30, "shared"), (0x30, "Main_a")], &[(0x10, "T_a")])),
    ("c", (&[(0xb0, "fn_c"), (0x30, "shared")], &[])),
];

/// One unit's object source: `fn_<u>` (and `_start` for `a`), its unit,
/// and, when `sets`, its name sets.
fn source(arch: Arch, u: &str, sets: Option<Sets>) -> String {
    let (pb, ty, ret) = match arch {
        Arch::Aarch64 => ("%progbits", "%function", "\tret\n\tnop\n"),
        Arch::X86_64 | Arch::I386 => ("@progbits", "@function", "\tret\n\tnop\n\tnop\n\tnop\n"),
    };
    let word = if arch.w() == 8 { ".8byte" } else { ".4byte" };
    let w = arch.w();
    let start = if u == "a" { "\t.globl _start\n_start:\n" } else { "" };
    let mut s = format!(
        "\t.section .text.{u},\"ax\",{pb}\n\t.globl fn_{u}\n\t.type fn_{u},{ty}\n{start}fn_{u}:\n{ret}.Lend:\n\
         \t.section .debug_abbrev,\"\",{pb}\n.Labbrev:\n\
         \t.uleb128 1\n\t.uleb128 0x11\n\t.byte 1\n\t.uleb128 0x03\n\t.uleb128 0x08\n\
         \t.uleb128 0x11\n\t.uleb128 0x01\n\t.uleb128 0x12\n\t.uleb128 0x06\n\t.byte 0,0\n\
         \t.uleb128 2\n\t.uleb128 0x2e\n\t.byte 0\n\t.uleb128 0x03\n\t.uleb128 0x08\n\t.byte 0,0\n\t.byte 0\n\
         \t.section .debug_info,\"\",{pb}\n.Linfo:\n\t.4byte .Linfo_end - .Linfo_body\n.Linfo_body:\n\
         \t.2byte 4\n\t.4byte .Labbrev\n\t.byte {w}\n\t.uleb128 1\n\t.asciz \"{u}.c\"\n\
         \t{word} fn_{u}\n\t.4byte .Lend - fn_{u}\n.Ldie:\n\t.uleb128 2\n\t.asciz \"fn_{u}\"\n\t.byte 0\n.Linfo_end:\n"
    );
    if let Some((names, types)) = sets {
        for (sec, set, l) in [("pubnames", names, "pn"), ("pubtypes", types, "pt")] {
            let _ = write!(
                s,
                "\t.section .debug_gnu_{sec},\"\",{pb}\n\t.4byte .L{l}_end - .L{l}_body\n.L{l}_body:\n\
                 \t.2byte 2\n\t.4byte .Linfo\n\t.4byte .Linfo_end - .Linfo\n"
            );
            for (attr, name) in set {
                let _ =
                    write!(s, "\t.4byte .Ldie - .Linfo\n\t.byte {attr:#x}\n\t.asciz \"{name}\"\n");
            }
            let _ = write!(s, "\t.4byte 0\n.L{l}_end:\n");
        }
    }
    s
}

/// gdb's hash (gdb manual, appendix J): `r = r * 67 + tolower(c) - 113`.
fn hash(s: &[u8]) -> u32 {
    s.iter().fold(0u32, |r, &c| {
        r.wrapping_mul(67).wrapping_add(u32::from(c.to_ascii_lowercase())).wrapping_sub(113)
    })
}

/// The bytes S-G7 gives: CU list, address area, names (each with its
/// sorted, unique values) in name order.
fn model(cus: &[(u64, u64)], area: &[(u64, u64, u32)], names: &[(String, Vec<u32>)]) -> Vec<u8> {
    let n = names.len() as u64;
    let t = (4 * n / 3 + 1).next_power_of_two().max(1024);
    let area_at = 24 + 16 * cus.len() as u64;
    let sym = area_at + 20 * area.len() as u64;
    let pool_at = sym + 8 * t;
    let mut pool = Vec::new();
    let mut vecs = Vec::new();
    for (_, v) in names {
        vecs.push(pool.len() as u32);
        pool.extend_from_slice(&(v.len() as u32).to_le_bytes());
        for x in v {
            pool.extend_from_slice(&x.to_le_bytes());
        }
    }
    let mut slots = vec![(0u32, 0u32); t as usize];
    let m = (t - 1) as u32;
    for (i, (name, _)) in names.iter().enumerate() {
        let at = pool.len() as u32;
        pool.extend_from_slice(name.as_bytes());
        pool.push(0);
        let h = hash(name.as_bytes());
        let (mut k, step) = (h & m, (h.wrapping_mul(17) & m) | 1);
        while slots[k as usize] != (0, 0) {
            k = (k + step) & m;
        }
        slots[k as usize] = (at, vecs[i]);
    }
    let mut b = Vec::new();
    for x in [7, 24, area_at, area_at, sym, pool_at] {
        b.extend_from_slice(&(x as u32).to_le_bytes());
    }
    for (o, l) in cus {
        b.extend_from_slice(&o.to_le_bytes());
        b.extend_from_slice(&l.to_le_bytes());
    }
    for (lo, hi, cu) in area {
        b.extend_from_slice(&lo.to_le_bytes());
        b.extend_from_slice(&hi.to_le_bytes());
        b.extend_from_slice(&cu.to_le_bytes());
    }
    for (a, v) in slots {
        b.extend_from_slice(&a.to_le_bytes());
        b.extend_from_slice(&v.to_le_bytes());
    }
    b.extend_from_slice(&pool);
    b
}

/// The names S-G4 gives for the units with sets, in name order.
fn names(with_sets: [bool; 3]) -> Vec<(String, Vec<u32>)> {
    let mut v: Vec<(String, Vec<u32>)> = Vec::new();
    for (cu, (_, (pn, pt))) in SETS.iter().enumerate() {
        if !with_sets[cu] {
            continue;
        }
        for &(attr, name) in pn.iter().chain(pt.iter()) {
            let val = cu as u32 | u32::from(attr) << 24;
            match v.iter_mut().find(|x| x.0 == name) {
                Some(x) => x.1.push(val),
                None => v.push((name.to_string(), vec![val])),
            }
        }
    }
    for x in &mut v {
        x.1.sort_unstable();
        x.1.dedup();
    }
    v
}

/// An output section: name, flags, address, file offset, size, index.
#[derive(Clone, Debug)]
struct Sec {
    name: String,
    flags: u64,
    addr: u64,
    off: u64,
    size: u64,
}

fn sections(b: &[u8]) -> Vec<Sec> {
    let h = FileHeader::parse(b).unwrap();
    SectionTable::parse(b, &h)
        .unwrap()
        .iter()
        .map(|s| {
            let sh = s.header();
            Sec {
                name: String::from_utf8_lossy(s.name().unwrap_or(b"")).into_owned(),
                flags: sh.sh_flags,
                addr: sh.sh_addr,
                off: sh.sh_offset,
                size: sh.sh_size,
            }
        })
        .collect()
}

/// The file offset of section `i`'s header field at `field64`/`field32`.
fn shdr_field(b: &[u8], i: usize, field64: usize, field32: usize) -> usize {
    let w64 = b[4] == 2;
    let rd = |at: usize, n: usize| {
        let mut x = 0u64;
        for k in (0..n).rev() {
            x = x << 8 | u64::from(b[at + k]);
        }
        x
    };
    let (shoff, entsize) =
        if w64 { (rd(0x28, 8), rd(0x3a, 2)) } else { (rd(0x20, 4), rd(0x2e, 2)) };
    (shoff + entsize * i as u64) as usize + if w64 { field64 } else { field32 }
}

/// One machine's link and its mortar model.
struct Case {
    arch: Arch,
    /// lld's output, its inputs, its map (lld's format).
    lld: Vec<u8>,
    files: Vec<(String, Vec<u8>)>,
    lld_map: Map,
    /// The model: the output with S-G7's index, the map as mortar's.
    out: Vec<u8>,
    map: Map,
    /// The index's file offset and bytes in `out`.
    at: usize,
    index: Vec<u8>,
    _scratch: Scratch,
}

struct Tools {
    asm: PathBuf,
    lld: PathBuf,
    objcopy: PathBuf,
}

fn tools(arch: Arch) -> Option<Tools> {
    let t = arch.triple();
    if let (Some(asm), Some(lld), Some(objcopy)) =
        (tool(&format!("{t}-as")), tool("ld.lld-22"), tool(&format!("{t}-objcopy")))
    {
        Some(Tools { asm, lld, objcopy })
    } else {
        {
            assert!(
                !mortar_testutil::tools_required(),
                "{t}-as, ld.lld-22, {t}-objcopy are required"
            );
            None
        }
    }
}

fn must(program: &Path, args: &[OsString]) {
    let o = run(program, args);
    assert!(o.success(), "{}: {}", program.display(), o.stderr_text());
}

impl Case {
    /// Links the three objects (`with_sets` says which have their name
    /// sets; the model's index holds those units' names).
    #[allow(clippy::too_many_lines)] // the link, the model, the map
    fn new(arch: Arch, with_sets: [bool; 3], label: &str) -> Option<Case> {
        let t = tools(arch)?;
        let scratch = Scratch::new(&format!("plumb-gdbindex-{label}-{arch:?}"));
        let dir = scratch.path().to_path_buf();
        let mut objs = Vec::new();
        for (k, (u, sets)) in SETS.iter().enumerate() {
            let src =
                scratch.write(&format!("{u}.s"), source(arch, u, with_sets[k].then_some(*sets)));
            let obj = dir.join(format!("{u}.o"));
            let mut args: Vec<OsString> = Vec::new();
            if arch == Arch::I386 {
                args.push("--32".into());
            }
            args.extend([src.into_os_string(), "-o".into(), obj.clone().into_os_string()]);
            must(&t.asm, &args);
            objs.push(obj);
        }
        let (out, map) = (dir.join("out"), dir.join("out.map"));
        let mut args: Vec<OsString> = vec![
            "-static".into(),
            "-e".into(),
            "_start".into(),
            "--gdb-index".into(),
            "-Map".into(),
            map.clone().into_os_string(),
        ];
        args.extend(objs.iter().map(|o| o.clone().into_os_string()));
        args.extend(["-o".into(), out.clone().into_os_string()]);
        must(&t.lld, &args);
        let lld = std::fs::read(&out).unwrap();
        let lld_map =
            mortar_plumb::map::parse(Format::Lld, &std::fs::read_to_string(&map).unwrap())
                .unwrap_or_else(|e| panic!("{arch:?}: the map: line {}: {}", e.line, e.message));
        let files: Vec<(String, Vec<u8>)> = lld_map
            .paths()
            .into_iter()
            .filter_map(|p| std::fs::read(p).ok().map(|b| (p.to_string(), b)))
            .collect();

        // The index S-G7 gives, from lld's `.debug_info` layout (the CU
        // list) and its `.text.*` placements (the area).
        let secs = sections(&lld);
        let info = secs.iter().find(|s| s.name == ".debug_info").unwrap();
        let mut cus = Vec::new();
        let mut p = 0u64;
        while p < info.size {
            let at = (info.off + p) as usize;
            let len = u64::from(u32::from_le_bytes(lld[at..at + 4].try_into().unwrap()));
            cus.push((p, len + 4));
            p += len + 4;
        }
        assert_eq!(cus.len(), 3, "{arch:?}: three units");
        let text = |u: &str| {
            let sym = format!("fn_{u}");
            let p = lld_map
                .placed
                .iter()
                .find(|p| matches!(&p.section, SectionRef::Name(n) if *n == format!(".text.{u}")))
                .unwrap_or_else(|| panic!("{arch:?}: {sym}'s section placed"));
            (p.addr, p.addr + p.size)
        };
        let area: Vec<(u64, u64, u32)> = ["a", "b", "c"]
            .iter()
            .enumerate()
            .map(|(k, u)| (text(u).0, text(u).1, k as u32))
            .collect();
        let index = model(&cus, &area, &names(with_sets));
        let bin = scratch.write("index.bin", &index);
        let mout = dir.join("mortar.out");
        let mut update = OsString::from(".gdb_index=");
        update.push(bin.as_os_str());
        must(
            &t.objcopy,
            &[
                "--update-section".into(),
                update,
                "--set-section-alignment".into(),
                ".gdb_index=4".into(),
                out.into_os_string(),
                mout.clone().into_os_string(),
            ],
        );
        let mbytes = std::fs::read(&mout).unwrap();
        let msecs = sections(&mbytes);
        let g = msecs.iter().find(|s| s.name == ".gdb_index").unwrap();
        assert_eq!(g.size as usize, index.len());
        let at = g.off as usize;
        assert_eq!(&mbytes[at..at + index.len()], index.as_slice());

        // lld's map as mortar's: non-alloc rows as `nplace` (as in
        // `aranges.rs`), and the index's chunk.
        let nonalloc_out = |n: &str| msecs.iter().find(|x| x.name == n && x.flags & SHF_ALLOC == 0);
        let mut base = lld_map.clone();
        let (nonalloc, placed): (Vec<_>, Vec<_>) = base
            .placed
            .drain(..)
            .partition(|p| matches!(&p.section, SectionRef::Name(n) if nonalloc_out(n).is_some()));
        base.placed = placed;
        for p in nonalloc {
            let SectionRef::Name(n) = &p.section else { unreachable!() };
            let o = nonalloc_out(n).unwrap();
            base.nonalloc.push(NonAlloc {
                file: p.file,
                section: p.section.clone(),
                out: n.clone(),
                offset: Some(p.addr - o.addr),
                size: p.size,
                line: 0,
            });
        }
        base.chunks.retain(|c| c.name != ".gdb_index");
        base.chunks.push(Chunk {
            kind: ChunkKind::Synthetic,
            name: ".gdb_index".into(),
            addr: None,
            size: index.len() as u64,
            target: None,
            line: 0,
        });
        let map =
            mortar_plumb::map::parse(Format::Mortar, &mortar_plumb::map::mortar::write(&base))
                .unwrap_or_else(|e| {
                    panic!("{arch:?}: mortar's map: line {}: {}", e.line, e.message)
                });
        Some(Case { arch, lld, files, lld_map, out: mbytes, map, at, index, _scratch: scratch })
    }

    /// The P-G findings on `out` with `map`.
    fn pg(&self, out: &[u8], map: &Map) -> Vec<String> {
        verify_with(out, &self.files, map)
            .findings
            .into_iter()
            .filter(|f| f.rule.id().starts_with("P-G"))
            .map(|f| format!("{} {}", f.rule.id(), f.message))
            .collect()
    }

    /// The model with the index's bytes at `off` replaced.
    fn patched(&self, off: usize, bytes: &[u8]) -> Vec<u8> {
        let mut m = self.out.clone();
        m[self.at + off..self.at + off + bytes.len()].copy_from_slice(bytes);
        m
    }

    fn u32_at(&self, off: usize) -> u32 {
        u32::from_le_bytes(self.index[off..off + 4].try_into().unwrap())
    }

    /// The slot holding `name` and its vector's pool offset.
    fn slot_of(&self, name: &str) -> (usize, usize) {
        let (sym, pool) = (self.u32_at(16) as usize, self.u32_at(20) as usize);
        for k in 0..(pool - sym) / 8 {
            let (n, v) = (self.u32_at(sym + 8 * k), self.u32_at(sym + 8 * k + 4));
            if (n, v) == (0, 0) {
                continue;
            }
            let s = pool + n as usize;
            if self.index[s..].starts_with(name.as_bytes()) && self.index[s + name.len()] == 0 {
                return (sym + 8 * k, pool + v as usize);
            }
        }
        panic!("{name} not in the index");
    }

    /// Each mutation of the model, rejected by the rule it names.
    #[allow(clippy::too_many_lines)] // one line per mutant
    fn mutants(&self) {
        let w = |x: u32| x.to_le_bytes().to_vec();
        let q = |x: u64| x.to_le_bytes().to_vec();
        let (area, sym, pool) =
            (self.u32_at(12) as usize, self.u32_at(16) as usize, self.u32_at(20) as usize);
        let (shared_slot, shared_vec) = self.slot_of("shared");
        let (main_slot, _) = self.slot_of("main_a");
        let (_, fn_c_vec) = self.slot_of("fn_c");
        let first_cu_lo = u64::from_le_bytes(self.index[area..area + 8].try_into().unwrap());
        let first_cu_hi = u64::from_le_bytes(self.index[area + 8..area + 16].try_into().unwrap());
        let empty = (0..(pool - sym) / 8)
            .map(|k| sym + 8 * k)
            .find(|&s| self.u32_at(s) == 0 && self.u32_at(s + 4) == 0)
            .unwrap();
        let last_nul = self.index.len() - 1;
        let cases: Vec<(&str, usize, Vec<u8>, &str)> = vec![
            ("version 6", 0, w(6), "P-G1"),
            ("version 8 (a format gdb reads; not S-G7's)", 0, w(8), "P-G9"),
            ("the CU list not at the header's end", 4, w(28), "P-G1"),
            ("the symbol table after the pool", 16, w(pool as u32 + 8), "P-G1"),
            ("the pool offset past the end", 20, w(self.index.len() as u32 + 1), "P-G1"),
            ("a CU entry's offset", 24 + 16, q(1), "P-G2"),
            ("a CU entry's length", 24 + 8, q(3), "P-G2"),
            ("an address entry's cu out of range", area + 16, w(3), "P-G3"),
            ("an address entry's cu another unit", area + 16, w(1), "P-G3"),
            ("an address entry low >= high", area, q(first_cu_hi), "P-G3"),
            ("an address entry past its unit's range", area + 8, q(first_cu_hi + 4), "P-G3"),
            ("an address entry moved before its range", area, q(first_cu_lo - 4), "P-G3"),
            ("a name's slot emptied", main_slot, q(0), "P-G8"),
            (
                "a name moved to an empty slot",
                empty,
                self.index[main_slot..main_slot + 8].to_vec(),
                "P-G4",
            ),
            ("a name offset past the pool", main_slot, w(u32::MAX - 8), "P-G4"),
            ("a vector offset past the pool", main_slot + 4, w(u32::MAX - 8), "P-G4"),
            ("the last name without its NUL", last_nul, vec![b'x'], "P-G4"),
            ("a vector of count 0", shared_vec, w(0), "P-G5"),
            ("a vector's cu past the units", shared_vec + 4, w(0x3000_0009), "P-G5"),
            ("a value's reserved bits", shared_vec + 4, w(0x3100_0000), "P-G5"),
            ("a value of kind 5", shared_vec + 4, w(0x5000_0000), "P-G5"),
            ("a value of another kind", shared_vec + 4, w(0x2000_0000), "P-G8"),
            ("a value's static bit", fn_c_vec + 4, w(0x3000_0002), "P-G8"),
            (
                "a vector out of order",
                shared_vec + 4,
                [w(0x3000_0001), w(0x3000_0000)].concat(),
                "P-G9",
            ),
            ("a name's bytes", pool + self.u32_at(shared_slot) as usize, vec![b'S'], "P-G8"),
            ("the TU list non-empty", 12, w(area as u32 + 24), "P-G1"),
        ];
        for (what, off, bytes, rule) in cases {
            let m = self.patched(off, &bytes);
            let r = self.pg(&m, &self.map);
            assert!(
                r.iter().any(|f| f.starts_with(&format!("{rule} "))),
                "{:?}: {what}: not {rule}: {r:#?}",
                self.arch
            );
            // A format rule's mutant is found in any producer's output
            // (judged with lld's map), but P-G3's bound by the unit's own
            // ranges, which is mortar's.
            let format_rule = matches!(rule, "P-G1" | "P-G2" | "P-G4" | "P-G5")
                || (rule == "P-G3" && !what.contains("range") && !what.contains("another unit"));
            if format_rule {
                let r = self.pg(&m, &self.lld_map);
                assert!(
                    r.iter().any(|f| f.starts_with(&format!("{rule} "))),
                    "{:?}: {what}, judged as lld's: not {rule}: {r:#?}",
                    self.arch
                );
            }
        }
        // P-G6: the section's alignment, the map's chunk.
        let gi = sections(&self.out).iter().position(|s| s.name == ".gdb_index").unwrap();
        let mut m = self.out.clone();
        let f = shdr_field(&m, gi, 48, 32);
        m[f] = 1;
        let r = self.pg(&m, &self.map);
        assert!(
            r.iter().any(|f| f.starts_with("P-G6 ") && f.contains("alignment")),
            "{:?}: alignment: {r:#?}",
            self.arch
        );
        let mut map = self.map.clone();
        map.chunks.iter_mut().filter(|c| c.name == ".gdb_index").for_each(|c| c.size += 4);
        let r = self.pg(&self.out, &map);
        assert!(
            r.iter().any(|f| f.starts_with("P-G6 ")),
            "{:?}: the chunk's size: {r:#?}",
            self.arch
        );
        let mut map = self.map.clone();
        map.chunks.retain(|c| c.name != ".gdb_index");
        let r = self.pg(&self.out, &map);
        assert!(r.iter().any(|f| f.starts_with("P-G6 ")), "{:?}: no chunk: {r:#?}", self.arch);
    }
}

fn arches() -> [Arch; 3] {
    [Arch::Aarch64, Arch::X86_64, Arch::I386]
}

// spec: plumb.md §6.10 P-G1 to P-G5 (format rules), §10.2 (references)
#[test]
fn p_g_lld_index_is_clean() {
    for arch in arches() {
        let Some(c) = Case::new(arch, [true; 3], "lld") else { return };
        let r = c.pg(&c.lld, &c.lld_map);
        assert!(r.is_empty(), "{arch:?}: lld's index: {r:#?}");
    }
}

// spec: plumb.md §6.10 P-G1 to P-G9; synth.md S-G7
#[test]
fn p_g_model_is_clean_and_mutants_rejected() {
    for arch in arches() {
        let Some(c) = Case::new(arch, [true; 3], "model") else { return };
        let r = c.pg(&c.out, &c.map);
        assert!(r.is_empty(), "{arch:?}: S-G7's index: {r:#?}");
        c.mutants();
    }
}

// spec: plumb.md §6.10 P-G7; synth.md S-G3 (the silent gdb-index-incomplete)
#[test]
fn p_g7_incomplete_index() {
    for arch in arches() {
        let Some(c) = Case::new(arch, [true, true, false], "incomplete") else { return };
        let r = c.pg(&c.out, &c.map);
        assert!(!r.is_empty(), "{arch:?}: an index without c's names is clean");
        assert!(
            r.iter().all(|f| f.starts_with("P-G7 ")
                && f.contains("unit 2")
                && f.contains("incomplete")),
            "{arch:?}: {r:#?}"
        );
        // Judged as lld's (the format rules only), it is a valid index.
        let r = c.pg(&c.out, &c.lld_map);
        assert!(r.is_empty(), "{arch:?}: format rules: {r:#?}");
    }
}

/// A small deterministic generator (xorshift64*), as `hostile.rs`.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }
}

// spec: plumb.md §9.4 (hostile): absurd fields of the index give
// findings, never a panic, in bounded time.
#[test]
fn p_g_hostile_index() {
    let Some(c) = Case::new(Arch::X86_64, [true; 3], "hostile") else { return };
    let n = c.index.len();
    let fields: Vec<usize> = (0..24).step_by(4).collect();
    let mut rng = Rng(0x6764_625f_696e_6465);
    for &f in &fields {
        for v in
            [0u32, 1, 3, 23, 24, 25, n as u32 - 1, n as u32, n as u32 + 1, u32::MAX, 0x8000_0000]
        {
            let m = c.patched(f, &v.to_le_bytes());
            let r = c.pg(&m, &c.map);
            assert!(!r.is_empty() || v == c.u32_at(f), "header field {f} = {v} is clean");
        }
    }
    // A table of 1000 slots (not a power of two), a full table.
    let (sym, pool) = (c.u32_at(16) as usize, c.u32_at(20) as usize);
    let m = c.patched(16, &((pool - 8000) as u32).to_le_bytes());
    assert!(c.pg(&m, &c.map).iter().any(|f| f.starts_with("P-G")));
    let mut full = c.index.clone();
    for k in 0..(pool - sym) / 8 {
        let s = sym + 8 * k;
        if full[s..s + 8] == [0; 8] {
            full[s..s + 8].copy_from_slice(&[0xff, 0xff, 0xff, 0x7f, 0xff, 0xff, 0xff, 0x7f]);
        }
    }
    let m = c.patched(0, &full);
    let r = c.pg(&m, &c.map);
    assert!(r.iter().any(|f| f.starts_with("P-G4 ") && f.contains("no empty slot")), "{r:#?}");
    // Every byte of the header, CU list and area, and a sample of the rest.
    let mut at: Vec<usize> = (0..sym.min(n)).collect();
    at.extend((0..400).map(|_| rng.below(n)));
    for a in at {
        for v in [0u8, 0xff, c.index[a] ^ 0x80] {
            let mut m = c.out.clone();
            m[c.at + a] = v;
            let _ = c.pg(&m, &c.map);
        }
    }
}
