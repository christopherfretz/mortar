//! Stage 4 with the load-time ODR runtime (`spec/plumb.md` §13.1, §13.2;
//! WP-197, `spec/odr.md` O-59): mortar's PIE with `--odr-runtime=report`
//! and its shared object with `--odr-runtime=fatal`, AArch64 and x86-64
//! (`tests/odrrt/gen.sh`), accepted as linked; one mutant per field and
//! per fact the rules read, each caught by its rule and sub-rule name.
//! The runtime's bytes are the verifier's own copy: the fixtures' maps
//! name `<mortar:odr-runtime>` and no file of that name is given.

#![allow(clippy::many_single_char_names)] // offsets, words and images: o, a, v, p, e

use std::path::PathBuf;

use mortar_plumb::{Ctx, Files, Format, Input, Report};

const ARCHES: [&str; 2] = ["aarch64", "x86_64"];
const RT: &str = "<mortar:odr-runtime>";

struct Dir(PathBuf);

impl Files for Dir {
    fn read(&self, path: &str) -> Option<Vec<u8>> {
        std::fs::read(self.0.join(path)).ok()
    }
}

fn dir(arch: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/odrrt").join(arch)
}

struct Fx {
    arch: &'static str,
    dir: PathBuf,
    out: Vec<u8>,
    map: String,
}

fn fixture(arch: &'static str, out: &str, map: &str) -> Fx {
    let d = dir(arch);
    Fx {
        arch,
        out: std::fs::read(d.join(out)).unwrap(),
        map: std::fs::read_to_string(d.join(map)).unwrap(),
        dir: d,
    }
}

fn judge(fx: &Fx, out: &[u8], map_text: &str) -> Report {
    let map = mortar_plumb::map::parse(Format::Mortar, map_text).unwrap();
    let files: Vec<(String, Vec<u8>)> = map
        .paths()
        .into_iter()
        .filter_map(|p| std::fs::read(fx.dir.join(p)).ok().map(|b| (p.to_string(), b)))
        .collect();
    let inputs: Vec<Input<'_>> = files.iter().map(|(p, b)| Input { path: p, bytes: b }).collect();
    let files = Dir(fx.dir.clone());
    mortar_plumb::verify_with(out, &inputs, &map, &Ctx { line: None, files: &files })
}

fn keys(r: &Report) -> Vec<String> {
    r.findings.iter().map(|f| format!("{} {}", f.rule.id(), f.message)).collect()
}

/// The mutant is caught by `rule` with a message starting `sub`; returns
/// the number of findings.
fn caught(fx: &Fx, out: &[u8], map: &str, rule: &str, sub: &str, what: &str) -> usize {
    let r = judge(fx, out, map);
    assert!(
        r.findings.iter().any(|f| f.rule.id() == rule && f.message.starts_with(sub)),
        "{}: {what}: not caught as {rule} '{sub}': {:#?}",
        fx.arch,
        keys(&r)
    );
    r.findings.len()
}

// ---- ELF64 little-endian helpers (the fixtures are ELF64) ----

fn rd(b: &[u8], o: usize, n: usize) -> u64 {
    let mut v = 0u64;
    for i in (0..n).rev() {
        v = (v << 8) | u64::from(b[o + i]);
    }
    v
}

fn wr(b: &mut [u8], o: usize, n: usize, v: u64) {
    for i in 0..n {
        b[o + i] = (v >> (8 * i)) as u8;
    }
}

/// (section header offset, name, file offset, size, address).
fn sections(b: &[u8]) -> Vec<(usize, String, usize, usize, u64)> {
    let shoff = rd(b, 0x28, 8) as usize;
    let n = rd(b, 0x3c, 2) as usize;
    let strndx = rd(b, 0x3e, 2) as usize;
    let str_off = rd(b, shoff + strndx * 64 + 0x18, 8) as usize;
    (0..n)
        .map(|i| {
            let h = shoff + i * 64;
            let name_at = str_off + rd(b, h, 4) as usize;
            let end = b[name_at..].iter().position(|&c| c == 0).unwrap();
            let name = String::from_utf8_lossy(&b[name_at..name_at + end]).into_owned();
            (h, name, rd(b, h + 0x18, 8) as usize, rd(b, h + 0x20, 8) as usize, rd(b, h + 0x10, 8))
        })
        .collect()
}

fn section(b: &[u8], name: &str) -> (usize, usize, usize, u64) {
    let s = sections(b).into_iter().find(|s| s.1 == name).unwrap();
    (s.0, s.2, s.3, s.4)
}

/// The file offset of address `a` (through the `PT_LOAD` headers).
fn off_of(b: &[u8], a: u64) -> usize {
    let phoff = rd(b, 0x20, 8) as usize;
    let n = rd(b, 0x38, 2) as usize;
    for i in 0..n {
        let h = phoff + i * 56;
        let (vaddr, offset, filesz) = (rd(b, h + 0x10, 8), rd(b, h + 0x08, 8), rd(b, h + 0x20, 8));
        if rd(b, h, 4) == 1 && vaddr <= a && a < vaddr + filesz {
            return (a - vaddr + offset) as usize;
        }
    }
    panic!("{a:#x} is in no PT_LOAD's file bytes");
}

/// `.note.mason`'s notes: (header offset, type, descriptor offset).
fn notes(b: &[u8]) -> Vec<(usize, u32, usize)> {
    let (_, off, size, _) = section(b, ".note.mason");
    let mut at = off;
    let mut v = Vec::new();
    while at < off + size {
        let d = at + 24;
        v.push((at, rd(b, at + 8, 4) as u32, d));
        at = d + (rd(b, at + 4, 4) as usize).div_ceil(8) * 8;
    }
    v
}

/// The runtime note's descriptor offset.
fn rt_note(b: &[u8]) -> usize {
    notes(b).iter().find(|n| n.1 == 0x4d4f_4403).unwrap().2
}

/// The map's file id of `path`.
fn file_id(map: &str, path: &str) -> String {
    map.lines()
        .find_map(|l| {
            l.strip_prefix("file ").and_then(|r| r.split_once(' ')).filter(|(_, p)| *p == path)
        })
        .map(|(id, _)| id.to_string())
        .unwrap()
}

/// The map's `place` records of file `id`: (line, section, address).
fn places(map: &str, id: &str) -> Vec<(String, u32, u64)> {
    map.lines()
        .filter_map(|l| {
            let w: Vec<&str> = l.split(' ').collect();
            (w.len() == 5 && w[0] == "place" && w[1] == id).then(|| {
                (l.to_string(), w[2].parse().unwrap(), u64::from_str_radix(w[3], 16).unwrap())
            })
        })
        .collect()
}

/// The runtime object's section `name` index (the verifier's copy, read
/// here from the checked-in file).
fn rt_object(arch: &str, kind: &str) -> Vec<u8> {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join(format!("../../runtime/odr/obj/mason_odr_rt-{arch}-{kind}.o"));
    std::fs::read(p).unwrap()
}

/// The address of the runtime's input section `name` in the output.
fn rt_place(fx: &Fx, kind: &str, name: &str) -> u64 {
    let o = rt_object(fx.arch, kind);
    let idx = sections(&o).iter().position(|s| s.1 == name).unwrap() as u32;
    let id = file_id(&fx.map, RT);
    places(&fx.map, &id).into_iter().find(|p| p.1 == idx).unwrap().2
}

/// `.rela.dyn`'s record whose `r_offset` is `a`: its file offset.
fn rela_at(b: &[u8], a: u64) -> usize {
    let (_, off, size, _) = section(b, ".rela.dyn");
    (off..off + size).step_by(24).find(|&r| rd(b, r, 8) == a).unwrap()
}

/// Writes `v` into the word at `a` and into its RELATIVE record's addend.
fn set_word(b: &mut [u8], a: u64, v: u64) {
    let o = off_of(b, a);
    wr(b, o, 8, v);
    let r = rela_at(b, a);
    wr(b, r + 16, 8, v);
}

fn word(b: &[u8], a: u64) -> u64 {
    rd(b, rela_at(b, a) + 16, 8)
}

/// The address `.symtab` gives `name`.
fn sym(b: &[u8], name: &str) -> u64 {
    let (_, off, size, _) = section(b, ".symtab");
    let (_, stroff, _, _) = section(b, ".strtab");
    (off..off + size)
        .step_by(24)
        .find(|&s| {
            let at = stroff + rd(b, s, 4) as usize;
            b[at..].starts_with(name.as_bytes()) && b[at + name.len()] == 0
        })
        .map(|s| rd(b, s + 8, 8))
        .unwrap()
}

#[test]
fn the_fixtures_are_accepted() {
    for arch in ARCHES {
        for (out, map, entries) in
            [("prog", "prog.map", 1), ("lib.so", "lib.map", 1), ("plain", "plain.map", 0)]
        {
            let fx = fixture(arch, out, map);
            let r = judge(&fx, &fx.out, &fx.map);
            assert!(r.clean(), "{arch} {out}: {:#?}", keys(&r));
            let m = &r.coverage.meta;
            assert_eq!(
                (m.outputs, m.tables, m.runtime_outputs, m.runtime_entries, m.runtime_imports),
                (1, 1, entries, entries, 8 * entries),
                "{arch} {out}: {m:?}"
            );
        }
    }
}

/// One mutation of an image.
type Mutant = Box<dyn Fn(&mut Vec<u8>)>;

/// P-M2's runtime note, field by field, its presence and its place.
#[test]
fn p_m2_runtime_note_mutants() {
    for arch in ARCHES {
        let (other_target, base) = if arch == "aarch64" { (3, 0) } else { (1, 2) };
        for (out, map, kind) in [("prog", "prog.map", 1u64), ("lib.so", "lib.map", 0)] {
            let fx = fixture(arch, out, map);
            let d = rt_note(&fx.out);
            let mode = rd(&fx.out, d + 4, 4);
            let cases: Vec<(&str, &str, Mutant)> = vec![
                ("runtime: version", "version", Box::new(move |p| wr(p, d, 4, 2))),
                ("runtime: mode", "mode swapped", Box::new(move |p| wr(p, d + 4, 4, 3 - mode))),
                ("runtime: flags", "flags", Box::new(move |p| wr(p, d + 8, 4, 1))),
                (
                    "runtime: runtime id",
                    "the id of the other kind",
                    Box::new(move |p| wr(p, d + 12, 4, base + (1 - kind))),
                ),
                (
                    "runtime: runtime id",
                    "the id of another target",
                    Box::new(move |p| wr(p, d + 12, 4, other_target - kind)),
                ),
                (
                    "presence",
                    "the note removed",
                    Box::new(|p| {
                        let (h, _, size, _) = section(p, ".note.mason");
                        wr(p, h + 0x20, 8, (size - 40) as u64);
                    }),
                ),
            ];
            for (sub, what, f) in cases {
                let mut p = fx.out.clone();
                f(&mut p);
                caught(&fx, &p, &fx.map, "P-M2", sub, &format!("{out}: {what}"));
            }
        }
        // A second runtime note: prog's =full note made one.
        let fx = fixture(arch, "prog", "prog.map");
        let n = notes(&fx.out);
        assert_eq!(
            n.iter().map(|x| x.1).collect::<Vec<_>>(),
            [0x4d4f_4401, 0x4d4f_4402, 0x4d4f_4403]
        );
        let mut p = fx.out.clone();
        let rt = n[2].2;
        let desc = p[rt..rt + 16].to_vec();
        wr(&mut p, n[1].0 + 8, 4, 0x4d4f_4403);
        p[n[1].2..n[1].2 + 16].copy_from_slice(&desc);
        caught(&fx, &p, &fx.map, "P-M2", "place", "a second runtime note");
        // The option off on the line (the image as linked).
        let m = fx.map.replace("arg --odr-runtime=report\n", "");
        caught(&fx, &fx.out, &m, "P-M2", "presence", "the line without the option");
        // The note added to an output linked without the option: plain's
        // =full note (16 bytes of descriptor, no row) made a runtime note.
        let px = fixture(arch, "plain", "plain.map");
        let n = notes(&px.out);
        let mut p = px.out.clone();
        wr(&mut p, n[1].0 + 8, 4, 0x4d4f_4403);
        p[n[1].2..n[1].2 + 16].copy_from_slice(&desc);
        caught(&px, &p, &px.map, "P-M2", "presence", "a runtime note without the option");
        // The lines mortar refuses.
        let m = fx.map.replace("arg --odr-table=full", "arg --odr-table=off");
        caught(&fx, &fx.out, &m, "P-M2", "presence", "an explicit --odr-table=off beside it");
        let m = fx.map.replace("arg --odr-runtime=report", "arg --odr-runtime=loud");
        caught(&fx, &fx.out, &m, "P-M2", "presence", "a value mortar refuses");
    }
}

/// P-M3: exactly the runtime's code, its entry, its file; P-D6: `-z now`.
#[test]
#[allow(clippy::too_many_lines)] // one mutant per fact, both kinds
fn p_m3_runtime_mutants() {
    for arch in ARCHES {
        let fx = fixture(arch, "prog", "prog.map");
        let lx = fixture(arch, "lib.so", "lib.map");
        // One byte of the runtime's code.
        let text = rt_place(&fx, "exe", ".text");
        let mut p = fx.out.clone();
        let o = off_of(&p, text + 0x40);
        p[o] ^= 0x01;
        caught(
            &fx,
            &p,
            &fx.map,
            "S12",
            "the output differs from the input",
            "a byte of the runtime's code",
        );
        // One of its relocated words: the field of its first relocation
        // record of .text.
        let obj = rt_object(arch, "exe");
        let (_, roff, _, _) = section(&obj, ".rela.text");
        let r_offset = rd(&obj, roff, 8);
        let mut p = fx.out.clone();
        let o = off_of(&p, text + r_offset);
        p[o + 1] ^= 0x01;
        caught(
            &fx,
            &p,
            &fx.map,
            "R1",
            "the relocated field holds the wrong value",
            "a relocated field of the runtime's code",
        );

        // The executable's entry: .preinit_array [user, runtime].
        let user = places(&fx.map, &file_id(&fx.map, "main.o"));
        let rt_pre = rt_place(&fx, "exe", ".preinit_array");
        let (_, _, _, pre_addr) = section(&fx.out, ".preinit_array");
        assert_eq!((pre_addr, rt_pre), (pre_addr, pre_addr + 8), "{arch}: the order as linked");
        let user_pre = user.iter().find(|u| u.2 == pre_addr).unwrap().clone();
        // Moved before the user's entry, the map and the image agreeing.
        let (uw, rw) = (word(&fx.out, pre_addr), word(&fx.out, rt_pre));
        let mut p = fx.out.clone();
        set_word(&mut p, pre_addr, rw);
        set_word(&mut p, rt_pre, uw);
        let rt_line =
            places(&fx.map, &file_id(&fx.map, RT)).into_iter().find(|x| x.2 == rt_pre).unwrap().0;
        let m = fx
            .map
            .replace(&rt_line, &rt_line.replace(&format!("{rt_pre:x}"), &format!("{pre_addr:x}")))
            .replace(
                &user_pre.0,
                &user_pre.0.replace(&format!("{pre_addr:x}"), &format!("{rt_pre:x}")),
            );
        caught(
            &fx,
            &p,
            &m,
            "P-M3",
            "entry",
            "exe: the runtime's entry moved before a user's preinit entry",
        );
        // The image alone swapped: the words are §5's.
        caught(
            &fx,
            &p,
            &fx.map,
            "R1",
            "the relocated field holds the wrong value",
            "exe: the words swapped in the image only",
        );
        // A user function's address in its place.
        let mut p = fx.out.clone();
        set_word(&mut p, rt_pre, sym(&fx.out, "user_fn"));
        caught(
            &fx,
            &p,
            &fx.map,
            "R1",
            "the relocated field holds the wrong value",
            "exe: user_fn in the entry",
        );
        caught(&fx, &p, &fx.map, "P-D9", "a RELATIVE record", "exe: user_fn in the entry");
        // Its entry removed: the map no longer places it.
        let m = fx.map.replace(&format!("{rt_line}\n"), "");
        caught(&fx, &fx.out, &m, "P-M3", "entry", "exe: the entry removed");
        caught(&fx, &fx.out, &m, "P-M3", "kept", "exe: the entry removed");

        // The shared object's entry: .init_array [user 0, runtime, user
        // 101, user].
        let rt_init = rt_place(&lx, "so", ".init_array.00000");
        let (_, _, _, init_addr) = section(&lx.out, ".init_array");
        assert_eq!(rt_init, init_addr + 8, "{arch}: the order as linked");
        let luser = places(&lx.map, &file_id(&lx.map, "lib.o"));
        let p101 = luser.iter().find(|u| u.2 == init_addr + 16).unwrap().clone();
        let (aw, bw) = (word(&lx.out, rt_init), word(&lx.out, init_addr + 16));
        let mut p = lx.out.clone();
        set_word(&mut p, rt_init, bw);
        set_word(&mut p, init_addr + 16, aw);
        let lrt =
            places(&lx.map, &file_id(&lx.map, RT)).into_iter().find(|x| x.2 == rt_init).unwrap().0;
        let a = format!("{rt_init:x}");
        let b = format!("{:x}", init_addr + 16);
        let m =
            lx.map.replace(&lrt, &lrt.replace(&a, &b)).replace(&p101.0, &p101.0.replace(&b, &a));
        caught(
            &lx,
            &p,
            &m,
            "P-M3",
            "entry",
            "so: the runtime's entry moved after a priority-101 one",
        );
        // Its entry removed: the word and the map's record.
        let m = lx.map.replace(&format!("{lrt}\n"), "");
        caught(&lx, &lx.out, &m, "P-M3", "entry", "so: the entry removed");
        // A user function's address in its place.
        let mut p = lx.out.clone();
        set_word(&mut p, rt_init, sym(&lx.out, "user_fn"));
        caught(
            &lx,
            &p,
            &lx.map,
            "R1",
            "the relocated field holds the wrong value",
            "so: user_fn in the entry",
        );

        // -z now: DF_BIND_NOW cleared; DF_1_NOW cleared.
        let (_, doff, dsize, _) = section(&fx.out, ".dynamic");
        let tag = |b: &[u8], t: u64| {
            (doff..doff + dsize).step_by(16).find(|&e| rd(b, e, 8) == t).unwrap()
        };
        let mut p = fx.out.clone();
        let e = tag(&p, 30);
        let v = rd(&p, e + 8, 8) & !8;
        wr(&mut p, e + 8, 8, v);
        caught(
            &fx,
            &p,
            &fx.map,
            "P-D6",
            "-z now and DT_FLAGS has no DF_BIND_NOW",
            "DF_BIND_NOW cleared",
        );
        let mut p = fx.out.clone();
        let e = tag(&p, 0x6fff_fffb);
        let v = rd(&p, e + 8, 8) & !1;
        wr(&mut p, e + 8, 8, v);
        caught(
            &fx,
            &p,
            &fx.map,
            "P-D6",
            "-z now and DT_FLAGS_1 has no DF_1_NOW",
            "DF_1_NOW cleared",
        );
        // -z lazy on the line: the output's BIND_NOW is then a finding.
        let m = fx
            .map
            .replace("arg --odr-runtime=report\n", "arg --odr-runtime=report\narg -z\narg lazy\n");
        caught(
            &fx,
            &fx.out,
            &m,
            "P-D6",
            "DF_BIND_NOW or DF_1_NOW without -z now",
            "-z lazy on the line",
        );

        // The map's runtime file renamed to a path on the line.
        let m = fx.map.replace(&format!(" {RT}\n"), " main.o\n");
        caught(&fx, &fx.out, &m, "P-M3", "input", "the runtime's file renamed to main.o");
        // The option off: today's rule, the name a path like any other.
        let m = fx.map.replace("arg --odr-runtime=report\n", "");
        caught(
            &fx,
            &fx.out,
            &m,
            "I1",
            "the map names '<mortar:odr-runtime>'",
            "the option dropped",
        );
    }
}

/// A reference linker's output carries no runtime note and names no file
/// the line does not give: GNU ld's PIE is accepted as linked; with a
/// type-3 note added (the runtime's, from prog) it is a finding.
/// GNU ld's words for `ref` (`tests/odrrt/gen.sh`).
const REF_LINE: [&str; 11] = [
    "-pie",
    "--hash-style=gnu",
    "-dynamic-linker",
    "/lib/ld.so",
    "-z",
    "max-page-size=4096",
    "-o",
    "ref",
    "main.o",
    "libc.so.6",
    "-Map=ref.map",
];

#[test]
fn a_reference_output_with_a_runtime_note() {
    for arch in ARCHES {
        let d = dir(arch);
        let text = std::fs::read_to_string(d.join("ref.map")).unwrap();
        let map = mortar_plumb::map::parse(Format::Gnu, &text).unwrap();
        let files: Vec<(String, Vec<u8>)> = map
            .paths()
            .into_iter()
            .filter_map(|p| std::fs::read(d.join(p)).ok().map(|b| (p.to_string(), b)))
            .collect();
        let inputs: Vec<Input<'_>> =
            files.iter().map(|(p, b)| Input { path: p, bytes: b }).collect();
        let fs = Dir(d.clone());
        let ctx =
            Ctx { line: Some(REF_LINE.iter().map(|w| (*w).to_string()).collect()), files: &fs };
        let r =
            mortar_plumb::verify_with(&std::fs::read(d.join("ref")).unwrap(), &inputs, &map, &ctx);
        assert!(r.clean(), "{arch}: GNU ld's PIE: {:#?}", keys(&r));
        let r = mortar_plumb::verify_with(
            &std::fs::read(d.join("ref-note")).unwrap(),
            &inputs,
            &map,
            &ctx,
        );
        let sub = "presence: the runtime note is present and the line's --odr-runtime is off";
        assert!(
            r.findings.iter().any(|f| f.rule.id() == "P-M2" && f.message.starts_with(sub)),
            "{arch}: a reference output with a runtime note: {:#?}",
            keys(&r)
        );
    }
}
