//! Loadability of mortar's layouts (WP-024 extra test (ii)).
//!
//! Each link is placed by every stage, checked by `Layout::check`, then
//! written as an ELF skeleton (headers and section headers, contents zero)
//! and checked again by an independent checker of the [G] rules of
//! `spec/layout.md` §2-§6 that reads only the file (`common::elf::check_g`).
//! With `MORTAR_PAGESIM` naming the reviewer's `pagesim.py`, the skeletons
//! also go through its kernel-mapping and RELRO arithmetic at 4, 16 and 64
//! KiB pages (each page size up to the `PT_LOAD` alignment must be OK).
//!
//! The links: synthetic ones by rule, and "reference-shaped" ones whose
//! inputs are the output sections of real static programs linked by lld
//! (one input per output section, sizes and alignments as lld made them).
#![allow(clippy::cast_possible_truncation, clippy::too_many_lines)]

mod common;

use std::path::Path;
use std::time::Duration;

use common::elf::{PT_LOAD, check_g, read_elf, write_skeleton};
use common::{Row, Sec, World, sec};
use mortar_core::layout::Synth;
use mortar_core::par::{Pool, check_schedules};
use mortar_core::resolution::{SectionKind, SectionState};
use mortar_elf::Machine;
use mortar_elf::consts::{
    SHF_ALLOC, SHF_EXECINSTR, SHF_TLS, SHF_WRITE, SHT_INIT_ARRAY, SHT_NOBITS, SHT_NOTE,
    SHT_PROGBITS, SHT_REL, SHT_RELA, SHT_STRTAB, SHT_SYMTAB,
};
use mortar_layout::Header;
use mortar_layout::chunk::SynthChunk;
use mortar_layout::stack::StackNote;
use mortar_testutil::{Scratch, run, run_with, tool};

const A: u64 = SHF_ALLOC;
const AX: u64 = SHF_ALLOC | SHF_EXECINSTR;
const WA: u64 = SHF_ALLOC | SHF_WRITE;
const WAT: u64 = WA | SHF_TLS;
const P: u32 = SHT_PROGBITS;

fn hello_files() -> Vec<Vec<Sec>> {
    vec![
        vec![
            sec(b".text", P, AX, 0x40, 3),
            sec(b".data", P, WA, 0x10, 3),
            sec(b".bss", SHT_NOBITS, WA, 0x20, 4),
            sec(b".note.ABI-tag", SHT_NOTE, A, 0x20, 2),
        ],
        vec![
            sec(b".text.startup.main", P, AX, 0x30, 4),
            sec(b".text", P, AX, 0x1_2340, 4),
            sec(b".rodata.str1.1", P, A, 0x2021, 0),
            sec(b".init_array", SHT_INIT_ARRAY, WA, 8, 3),
            sec(b".data.rel.ro", P, WA, 0x1238, 4),
            sec(b".tdata", P, WAT, 8, 3),
            sec(b".tbss", SHT_NOBITS, WAT, 0x10, 4),
            sec(b".data", P, WA, 0x3456, 5),
            sec(b".bss", SHT_NOBITS, WA, 0x1_0000, 6),
            sec(b".comment", P, 0, 0x2a, 0),
            sec(b".debug_info", P, 0, 0x1234, 0),
        ],
    ]
}

fn got() -> SynthChunk {
    SynthChunk {
        synth: Synth { name: b".got", address_dependent: false },
        sh_type: P,
        flags: WA,
        align: 3,
        size: 0x40,
    }
}

/// The synthetic links: (name, world).
fn synthetic_links() -> Vec<(String, World)> {
    let mut out = Vec::new();
    for (arch, machine) in
        [("aarch64", Machine::AArch64), ("x86-64", Machine::X86_64), ("i386", Machine::I386)]
    {
        let mut w = World::new(machine, &hello_files(), &[]);
        w.synthetic.push(got());
        out.push((format!("hello-{arch}"), w.clone()));
        let mut s = w.clone();
        s.options.separate_code = true;
        out.push((format!("hello-{arch}-separate-code"), s));
        let mut s = w.clone();
        s.options.relro = false;
        out.push((format!("hello-{arch}-norelro"), s));
        let mut s = w.clone();
        s.stack_notes = vec![("a.o".into(), StackNote::Exec)];
        out.push((format!("hello-{arch}-execstack"), s));
        for page in [0x1000u64, 0x4000] {
            let mut s = w.clone();
            s.options.max_page = page;
            out.push((format!("hello-{arch}-maxpage-{page:#x}"), s));
        }
        let files = vec![vec![
            sec(b".text", P, AX, 4, 2),
            sec(b".tbss", SHT_NOBITS, WAT, 0x30, 6),
            sec(b".data", P, WA, 8, 3),
        ]];
        out.push((format!("tbss-only-{arch}"), World::new(machine, &files, &[])));
        let files = vec![vec![
            sec(b".text", P, AX, 4, 2),
            sec(b".rodata", P, A, 0x100, 16),
            sec(b".data.big", P, WA, 0x10, 21),
            sec(b".tdata", P, WAT, 0x10, 12),
            sec(b".bss", SHT_NOBITS, WA, 0x10, 18),
        ]];
        out.push((format!("large-align-{arch}"), World::new(machine, &files, &[])));
        let files =
            vec![vec![sec(b".text", P, AX, 0x10, 2), sec(b".bss", SHT_NOBITS, WA, 0x10_0000, 4)]];
        out.push((format!("bss-only-{arch}"), World::new(machine, &files, &[])));
    }
    out
}

// -- reference-shaped links -------------------------------------------------

const PROGRAMS: [(&str, &str, &str); 2] = [
    ("hello.c", "gcc", "#include <stdio.h>\nint main(void) { puts(\"hi\"); return 0; }\n"),
    (
        "eh.cc",
        "g++",
        "#include <cstdio>\n#include <stdexcept>\nthread_local int tl = 7;\n\
         int main(int c, char **) { try { if (c) throw std::runtime_error(\"x\"); }\n\
         catch (const std::exception &e) { std::printf(\"%s %d\\n\", e.what(), tl); }\n\
         return 0; }\n",
    ),
];

/// The output sections of `exe` as the inputs of one file: the linker's
/// own sections become synthetic chunks.
fn reference_shaped(machine: Machine, exe: &[u8]) -> World {
    let elf = read_elf(exe);
    let mut rows = Vec::new();
    let mut synthetic = Vec::new();
    for s in elf.sections.iter().skip(1) {
        if matches!(s.ty, SHT_SYMTAB | SHT_STRTAB) || s.name == ".relro_padding" {
            continue;
        }
        let name: &'static [u8] = Box::leak(s.name.clone().into_bytes().into_boxed_slice());
        let align = s.align.max(1).trailing_zeros() as u8;
        let linker_made = matches!(s.ty, SHT_RELA | SHT_REL)
            || matches!(
                s.name.as_str(),
                ".got"
                    | ".got.plt"
                    | ".eh_frame_hdr"
                    | ".note.gnu.property"
                    | ".iplt"
                    | ".plt"
                    | ".note.gnu.build-id"
            );
        if linker_made {
            synthetic.push(SynthChunk {
                synth: Synth { name, address_dependent: false },
                sh_type: s.ty,
                flags: s.flags,
                align,
                size: s.size,
            });
            continue;
        }
        let kind = if s.ty == SHT_NOBITS {
            SectionKind::Nobits
        } else if s.ty == SHT_NOTE {
            SectionKind::Note
        } else if s.flags & SHF_ALLOC == 0 {
            SectionKind::Debug
        } else {
            SectionKind::Regular
        };
        rows.push(Row {
            header: Header {
                name,
                sh_type: s.ty,
                flags: s.flags & !0x30, // not SHF_MERGE|SHF_STRINGS: one piece
                size: s.size,
                align,
                link: None,
                entsize: 0,
            },
            link: 0,
            kind,
            state: SectionState::Kept,
        });
    }
    let mut w = World::from_rows(machine, &[rows], &[]);
    w.synthetic = synthetic;
    w
}

fn reference_links(dir: &Path) -> Vec<(String, World)> {
    let mut out = Vec::new();
    if tool("ld.lld").is_none() {
        return out;
    }
    for (arch, machine, prefix) in [
        ("aarch64", Machine::AArch64, ""),
        ("x86-64", Machine::X86_64, "x86_64-linux-gnu-"),
        ("i386", Machine::I386, "i686-linux-gnu-"),
    ] {
        for (file, cc, source) in PROGRAMS {
            let cc = format!("{prefix}{cc}");
            if tool(&cc).is_none() {
                continue;
            }
            let src = dir.join(file);
            std::fs::write(&src, source).unwrap();
            let exe = dir.join(format!("{file}-{arch}"));
            let mut cmd = std::process::Command::new(&cc);
            cmd.args(["-O2", "-static", "-fuse-ld=lld"]).arg(&src).arg("-o").arg(&exe);
            let o = run_with(&mut cmd, &[], Duration::from_mins(5));
            assert!(o.success(), "{cc} {file}: {}", o.stderr_text());
            let bytes = std::fs::read(&exe).unwrap();
            out.push((format!("lld-shaped-{file}-{arch}"), reference_shaped(machine, &bytes)));
        }
    }
    out
}

/// Every link: placed, `Layout::check`ed, written, [G]-checked, and run
/// through pagesim when it is given.
fn check_all(links: &[(String, World)], dir: &Path) -> usize {
    let pool = Pool::sequential();
    let pagesim = std::env::var_os("MORTAR_PAGESIM");
    if pagesim.is_none() {
        eprintln!("note: MORTAR_PAGESIM not set; skipping the page simulation");
    }
    let mut problems = Vec::new();
    let mut simulated = 0;
    for (name, w) in links {
        // (iii) the same result under every schedule of the helpers
        // (thread counts and seeds that permute the processing order).
        let _ = check_schedules(|pool| {
            w.run(pool).map(|(a, p)| {
                let names: Vec<Vec<u8>> = a.outsecs.iter().map(|(_, o)| o.name.to_vec()).collect();
                (names, a.out_of.clone(), p.placement, p.rounds)
            })
        });
        let (a, placed) = match w.run(&pool) {
            Ok(x) => x,
            Err(e) => {
                problems.push(format!(
                    "{name}: refused: {:?}",
                    e.iter().map(mortar_layout::LayoutDiag::message).collect::<Vec<_>>()
                ));
                continue;
            }
        };
        if let Err(v) = w.check(&a, &placed) {
            problems.push(format!("{name}: Layout::check: {v:?}"));
        }
        let class64 = w.target.class == mortar_elf::Class::Elf64;
        let bytes = write_skeleton(&placed.placement, &a, w.target.machine, class64);
        let elf = read_elf(&bytes);
        for v in check_g(&elf) {
            problems.push(format!("{name}: {v}"));
        }
        let Some(sim) = &pagesim else { continue };
        let path = dir.join(format!("{name}.elf"));
        std::fs::write(&path, &bytes).unwrap();
        let o = run("python3", [sim.as_os_str(), path.as_os_str()]);
        assert!(o.success(), "pagesim: {}", o.stderr_text());
        let align = elf.segments.iter().filter(|p| p.ty == PT_LOAD).map(|p| p.align).min().unwrap();
        for line in o.stdout_text().lines() {
            let page = [(0x1000u64, "P=0x1000 "), (0x4000, "P=0x4000 "), (0x1_0000, "P=0x10000")]
                .into_iter()
                .find(|(_, t)| line.contains(t))
                .map(|(p, _)| p);
            let Some(page) = page else { continue };
            eprintln!("{line}");
            if page <= align {
                simulated += 1;
                if !line.contains(" OK") || line.contains("RELRO tail") {
                    problems.push(format!("{name}: pagesim: {line}"));
                }
            }
        }
    }
    for p in &problems {
        eprintln!("PROBLEM {p}");
    }
    assert!(problems.is_empty(), "{} loadability problems", problems.len());
    simulated
}

// spec: layout.md §2-§6 L-1..L-23 [G] (synthetic links, every option set)
#[test]
fn synthetic_links_load() {
    let dir = Scratch::new("layout-load-syn");
    let links = synthetic_links();
    let n = check_all(&links, dir.path());
    eprintln!("{} synthetic links checked; {n} page simulations", links.len());
}

// spec: layout.md §2-§6 L-1..L-23 [G] (links shaped like real lld outputs)
#[test]
fn reference_shaped_links_load() {
    let dir = Scratch::new("layout-load-ref");
    let links = reference_links(dir.path());
    if links.is_empty() {
        return;
    }
    let n = check_all(&links, dir.path());
    eprintln!("{} reference-shaped links checked; {n} page simulations", links.len());
}

// spec: layout.md §2 L-1, L-4, L-8 (the independent checker finds broken layouts)
#[test]
fn checker_finds_mutations() {
    let mut w = World::new(Machine::AArch64, &hello_files(), &[]);
    w.synthetic.push(got());
    let (a, placed) = w.run(&Pool::sequential()).unwrap();
    let good = read_elf(&write_skeleton(&placed.placement, &a, Machine::AArch64, true));
    assert!(check_g(&good).is_empty());
    let loads: Vec<usize> =
        (0..good.segments.len()).filter(|&i| good.segments[i].ty == PT_LOAD).collect();
    // L-1: an offset off by 8.
    let mut e = good.clone();
    e.segments[loads[1]].offset += 8;
    assert!(check_g(&e).iter().any(|v| v.starts_with("L-1")), "{:?}", check_g(&e));
    // L-4: .bss inside p_filesz.
    let mut e = good.clone();
    let last = *loads.last().unwrap();
    e.segments[last].filesz = e.segments[last].memsz;
    assert!(check_g(&e).iter().any(|v| v.starts_with("L-4")), "{:?}", check_g(&e));
    // L-8: RELRO reaching into .data's page at 4 KiB.
    let mut e = good.clone();
    let r = e.segments.iter().position(|p| p.ty == common::elf::PT_GNU_RELRO).unwrap();
    e.segments[r].memsz += 0x2000;
    assert!(
        check_g(&e).iter().any(|v| v.starts_with("L-8") || v.starts_with("L-7")),
        "{:?}",
        check_g(&e)
    );
}
