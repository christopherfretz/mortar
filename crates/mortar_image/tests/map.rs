//! The map file (§9.2) round-trips through `spec/plumb.md` §3.2's grammar,
//! read by a parser written here from the grammar; `--time-phases` (§11.2);
//! the structural encoders (I-40).

mod support;

use mortar_core::par::{self, Pool};
use mortar_image::map::{
    ChunkKind, DiscardWhy, Fold, Map, MapChunk, MapFile, Placed, render, token,
};
use support::Gen;

fn untoken(t: &str) -> Vec<u8> {
    if t == "%" {
        return Vec::new();
    }
    let b = t.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' {
            out.push(u8::from_str_radix(&t[i + 1..i + 3], 16).unwrap());
            assert!(t[i + 1..i + 3].bytes().all(|c| !c.is_ascii_lowercase()), "upper-case hex");
            i += 3;
        } else {
            assert!((b'!'..=b'~').contains(&b[i]));
            out.push(b[i]);
            i += 1;
        }
    }
    out
}

fn hex(t: &str) -> u64 {
    assert!(
        t.len() <= 16 && t.bytes().all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c)),
        "{t}"
    );
    u64::from_str_radix(t, 16).unwrap()
}

/// A reader of `mortar-map 1`, from the grammar alone.
fn parse(text: &str) -> Map {
    let mut lines = text.lines();
    assert_eq!(lines.next(), Some("mortar-map 1"));
    let mut m = Map::default();
    let mut ended = false;
    for line in lines {
        if ended {
            assert!(line.is_empty());
            continue;
        }
        let t: Vec<&str> = line.split(' ').collect();
        let file = |s: &str| -> usize {
            let id: usize = s.parse().unwrap();
            assert!(id < m.files.len(), "a record refers only to an earlier file");
            id
        };
        match t[0] {
            "file" => {
                assert_eq!(t[1].parse::<usize>().unwrap(), m.files.len());
                let member = (t.len() == 5).then(|| (untoken(t[3]), hex(t[4])));
                m.files.push(MapFile { path: untoken(t[2]), member });
                m.placed.push(Vec::new());
                m.discards.push(Vec::new());
            }
            "place" => {
                let f = file(t[1]);
                m.placed[f].push(Placed::Place {
                    section: t[2].parse().unwrap(),
                    addr: hex(t[3]),
                    size: hex(t[4]),
                });
            }
            "piece" => {
                let f = file(t[1]);
                m.placed[f].push(Placed::Piece {
                    section: t[2].parse().unwrap(),
                    input_offset: hex(t[3]),
                    addr: hex(t[4]),
                    size: hex(t[5]),
                });
            }
            "discard" => {
                let f = file(t[1]);
                let why = match t[3] {
                    "gc" => DiscardWhy::Gc,
                    "comdat" => DiscardWhy::Comdat,
                    "script" => DiscardWhy::Script,
                    "unspecified" => DiscardWhy::Unspecified,
                    w => panic!("{w}"),
                };
                m.discards[f].push((t[2].parse().unwrap(), why));
            }
            "fold" => m.folds.push(Fold {
                file: file(t[1]) as u32,
                section: t[2].parse().unwrap(),
                into_file: file(t[3]) as u32,
                into_section: t[4].parse().unwrap(),
            }),
            "chunk" => {
                let kind = match t[1] {
                    "thunk" => ChunkKind::Thunk,
                    "patch" => ChunkKind::Patch,
                    "synthetic" => ChunkKind::Synthetic,
                    "common" => ChunkKind::Common,
                    k => panic!("{k}"),
                };
                let addr = (t[3] != "-").then(|| hex(t[3]));
                let target = (t.len() == 7).then(|| {
                    let a = t[6];
                    let v = match a.strip_prefix('-') {
                        Some(n) => (hex(n) as i64).wrapping_neg(),
                        None => hex(a) as i64,
                    };
                    (untoken(t[5]), v)
                });
                m.chunks.push(MapChunk {
                    kind,
                    name: untoken(t[2]),
                    addr,
                    size: hex(t[4]),
                    target,
                });
            }
            "define" => m.defines.push((untoken(t[1]), hex(t[2]))),
            "end" => ended = true,
            r => panic!("unknown record {r}"),
        }
    }
    assert!(ended, "truncated");
    m
}

fn random_map(g: &mut Gen) -> Map {
    let name = |g: &mut Gen| -> Vec<u8> { (0..g.below(12)).map(|_| g.next() as u8).collect() };
    let files = 1 + g.below(20) as usize;
    let mut m = Map::default();
    for k in 0..files {
        let member = (k % 3 == 1).then(|| (name(g), g.next()));
        m.files.push(MapFile { path: name(g), member });
        let mut placed = Vec::new();
        for _ in 0..g.below(30) {
            placed.push(if g.below(4) == 0 {
                Placed::Piece {
                    section: g.below(100) as u32,
                    input_offset: g.next(),
                    addr: g.next(),
                    size: g.next(),
                }
            } else {
                Placed::Place {
                    section: g.below(100) as u32,
                    addr: g.next() << 20,
                    size: g.below(1 << 20),
                }
            });
        }
        m.placed.push(placed);
        let whys =
            [DiscardWhy::Gc, DiscardWhy::Comdat, DiscardWhy::Script, DiscardWhy::Unspecified];
        m.discards.push(
            (0..g.below(4)).map(|_| (g.below(50) as u32, whys[g.below(4) as usize])).collect(),
        );
    }
    for _ in 0..g.below(5) {
        let f = g.below(files as u64) as u32;
        m.folds.push(Fold { file: f, section: 3, into_file: f, into_section: 4 });
    }
    let kinds = [ChunkKind::Thunk, ChunkKind::Patch, ChunkKind::Synthetic, ChunkKind::Common];
    for _ in 0..g.below(6) {
        let target = (g.below(2) == 0).then(|| (name(g), g.next() as i64));
        let addr = (g.below(3) != 0).then(|| g.next());
        m.chunks.push(MapChunk {
            kind: kinds[g.below(4) as usize],
            name: name(g),
            addr,
            size: g.next(),
            target,
        });
    }
    m.chunks.push(MapChunk {
        kind: ChunkKind::Thunk,
        name: Vec::new(),
        addr: None,
        size: 0,
        target: Some((vec![b'x'], i64::MIN)),
    });
    for _ in 0..g.below(6) {
        m.defines.push((name(g), g.next()));
    }
    m
}

// spec: image.md I-77, I-78, I-79; plumb.md §3.2: the map round-trips, and
// its bytes do not depend on the schedule.
#[test]
fn i79_map_round_trips_through_the_grammar() {
    let mut g = Gen(42);
    for _ in 0..50 {
        let m = random_map(&mut g);
        let text = par::check_schedules(|pool| render(&m, pool));
        let s = String::from_utf8(text).unwrap();
        assert_eq!(parse(&s), m);
        // Group order: files, places/pieces, discards, folds, chunks, defines.
        let order = ["file", "place", "discard", "fold", "chunk", "define", "end"];
        let ranks: Vec<usize> = s
            .lines()
            .skip(1)
            .map(|l| {
                let w = l.split(' ').next().unwrap();
                let w = if w == "piece" { "place" } else { w };
                order.iter().position(|o| *o == w).unwrap()
            })
            .collect();
        assert!(ranks.windows(2).all(|p| p[0] <= p[1]), "{s}");
    }
}

// spec: plumb.md §3.2 "Strings": every byte encodes and decodes.
#[test]
fn i78_tokens_encode_every_byte() {
    let all: Vec<u8> = (0..=255).collect();
    let mut s = String::new();
    token(&all, &mut s);
    assert!(s.bytes().all(|b| (b'!'..=b'~').contains(&b)));
    assert_eq!(untoken(&s), all);
    let mut e = String::new();
    token(b"", &mut e);
    assert_eq!(e, "%");
    let mut p = String::new();
    token(b"a b%", &mut p);
    assert_eq!(p, "a%20b%25");
}

// spec: image.md I-88: the line format; `-` for a value not read; CPU
// fields 14 and 15 after the command's last parenthesis.
#[test]
fn i88_time_phases_lines() {
    use mortar_image::phases::{Sample, phase_line, stat_cpu_ticks, status_kib, total_line};
    let stat = "1234 (a (b) c) S 1 2 3 4 5 6 7 8 9 10 150 25 0 0 20 0 1 0";
    assert_eq!(stat_cpu_ticks(stat), Some(175));
    assert_eq!(
        status_kib("Name:\tx\nVmHWM:\t  208896 kB\nVmRSS:\t 207974 kB\n", "VmRSS"),
        Some(207_974)
    );
    let t0 = std::time::Instant::now();
    let a = Sample { at: t0, cpu_ticks: Some(100), rss_kib: Some(1024), peak_kib: Some(2048) };
    let b = Sample {
        at: t0 + std::time::Duration::from_micros(112_449),
        cpu_ticks: Some(164),
        rss_kib: Some(207_974),
        peak_kib: Some(208_896),
    };
    assert_eq!(
        phase_line(20, "input", &a, &b),
        "mortar: time-phases: 20 input: wall 112.4 ms, cpu 640 ms, rss 203.1 MiB, peak 204.0 MiB"
    );
    let c = Sample { cpu_ticks: None, rss_kib: None, peak_kib: None, ..b };
    assert_eq!(
        phase_line(100, "image", &a, &c),
        "mortar: time-phases: 100 image: wall 112.4 ms, cpu -, rss -, peak -"
    );
    assert_eq!(
        total_line(&a, &b),
        "mortar: time-phases: total: wall 112.4 ms, cpu 640 ms, peak 204.0 MiB"
    );
    // The live reading works on Linux.
    let s = Sample::now();
    assert!(s.cpu_ticks.is_some() && s.rss_kib.is_some() && s.peak_kib.is_some());
}

// spec: image.md I-92: the formula's terms.
#[test]
fn i92_memory_formula() {
    use mortar_image::phases::{Budget, image_tables};
    let b = Budget {
        resident: 100,
        tables: 10,
        threads: 8,
        window: 1 << 20,
        reloc_buffer: 3 << 19,
        ..Budget::default()
    };
    assert_eq!(b.peak(), 110 + 8 * ((1 << 20) + (3 << 19)));
    assert_eq!(image_tables(10, 2, 3, 4097, 5, 16), 80 + 48 + 12 + 16 + 80);
}

// spec: image.md I-40, synth.md S-90: header encoders' sizes and fields;
// `.shstrtab` deduplicates in index order.
#[test]
fn i40_structural_encoders() {
    use mortar_elf::Class;
    use mortar_image::headers::{Ehdr, Phdr, Shdr, ehdr, phdrs, shdrs, shstrtab};
    let h = Ehdr {
        e_type: 2,
        e_machine: 183,
        e_entry: 0x40_0000,
        e_phoff: 64,
        e_shoff: 0x1000,
        e_phnum: 3,
        e_shnum: 5,
        e_shstrndx: 4,
        ..Ehdr::default()
    };
    let b = ehdr(Class::Elf64, &h);
    assert_eq!(b.len(), 64);
    assert_eq!(&b[..7], &[0x7f, b'E', b'L', b'F', 2, 1, 1]);
    assert_eq!(u64::from_le_bytes(b[24..32].try_into().unwrap()), 0x40_0000);
    assert_eq!(u16::from_le_bytes(b[54..56].try_into().unwrap()), 56);
    assert_eq!(u16::from_le_bytes(b[62..64].try_into().unwrap()), 4);
    assert_eq!(ehdr(Class::Elf32, &h).len(), 52);
    let p = Phdr {
        p_type: 1,
        p_flags: 5,
        p_offset: 0,
        p_vaddr: 0x40_0000,
        p_paddr: 0x40_0000,
        p_filesz: 10,
        p_memsz: 10,
        p_align: 0x1_0000,
    };
    let pb = phdrs(Class::Elf64, &[p, p]);
    assert_eq!(pb.len(), 112);
    assert_eq!(u32::from_le_bytes(pb[4..8].try_into().unwrap()), 5, "p_flags second in ELF64");
    let pb32 = phdrs(Class::Elf32, &[p]);
    assert_eq!(pb32.len(), 32);
    assert_eq!(u32::from_le_bytes(pb32[24..28].try_into().unwrap()), 5, "p_flags seventh in ELF32");
    assert_eq!(shdrs(Class::Elf64, &[Shdr::default(); 3]).len(), 192);
    assert_eq!(shdrs(Class::Elf32, &[Shdr::default(); 3]).len(), 120);
    let (tab, offs) = shstrtab(&[b"", b".text", b".data", b".text", b".shstrtab"]);
    assert_eq!(tab, b"\0.text\0.data\0.shstrtab\0");
    assert_eq!(offs, vec![0, 1, 7, 1, 13]);
    let _ = Pool::sequential();
}
