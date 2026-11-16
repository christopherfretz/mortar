//! `spec/input.md` §8 with `spec/dyn.md` D-43a: the program-header and
//! `.dynamic` facts of a shared input (`SharedFacts::{flags, flags_1,
//! symbolic, loads, relro, mason_note}`, WP-116), on libraries GNU ld
//! builds here, and the reader's behaviour on a truncated program header
//! table (the existing malformed-file error, never a panic).
mod common;

use mortar_elf::consts::{DF_1_NOW, DF_BIND_NOW, DF_SYMBOLIC};
use mortar_input::facts::{Bad, Kind, LoadSeg, SharedFacts, analyse};
use mortar_testutil::Scratch;

const C: &str = "int g = 1;\nint f(void) { return g; }\n";

/// A `.note.mason` table note (owner `MasonMeta`, `NT_MASON_ODR`) with
/// the 4-byte descriptor `0x11223344`, in an allocated `SHT_NOTE`
/// section, which GNU ld places in a `PT_NOTE`.
const NOTED: &str = "int f(void) { return 1; }\n\
__asm__(\".section .note.mason,\\\"a\\\",%note\\n\"\n\
        \".balign 4\\n\"\n\
        \".long 10\\n.long 4\\n.long 0x4d4f4401\\n\"\n\
        \".asciz \\\"MasonMeta\\\"\\n.balign 4\\n\"\n\
        \".long 0x11223344\\n\"\n\
        \".previous\\n\");\n";

/// Builds `dir/name` with the native GNU toolchain; `false` when it is
/// missing (a failure under `MORTAR_REQUIRE_TOOLS`).
fn so(dir: &std::path::Path, name: &str, src: &str, extra: &[&str]) -> bool {
    let mut flags = vec!["-shared", "-fPIC", "-nostdlib", "-fuse-ld=bfd"];
    flags.extend_from_slice(extra);
    common::cc(dir, "native", name, src, &flags).is_some()
}

fn facts(bytes: &[u8]) -> Box<SharedFacts<'_>> {
    match analyse(bytes, false).kind {
        Kind::Shared(s) => s,
        other => panic!("not a shared object: {other:?}"),
    }
}

/// `readelf -lW`'s `PT_LOAD` rows and its `GNU_RELRO` row, as the facts
/// should hold them.
fn readelf_segments(dir: &std::path::Path, file: &str) -> (Vec<LoadSeg>, Option<(u64, u64)>) {
    let readelf = common::need("readelf").unwrap();
    let o = common::tool_run(&readelf, &["-lW".into(), file.into()], dir);
    assert!(o.success(), "{}", o.stderr_text());
    let hex = |w: &str| u64::from_str_radix(w.trim_start_matches("0x"), 16).unwrap();
    let mut loads = Vec::new();
    let mut relro = None;
    for line in o.stdout_text().lines() {
        let w: Vec<&str> = line.split_whitespace().collect();
        // Type Offset VirtAddr PhysAddr FileSiz MemSiz Flg... Align
        match w.first() {
            Some(&"LOAD") => loads.push(LoadSeg {
                vaddr: hex(w[2]),
                memsz: hex(w[5]),
                writable: w[6..w.len() - 1].concat().contains('W'),
            }),
            Some(&"GNU_RELRO") => relro = Some((hex(w[2]), hex(w[5]))),
            _ => {}
        }
    }
    (loads, relro)
}

#[test]
fn d43a_plain_library() {
    let s = Scratch::new("sf_plain");
    if !so(s.path(), "libplain.so", C, &["-Wl,-z,norelro", "-Wl,-z,lazy"]) {
        return;
    }
    let bytes = std::fs::read(s.path().join("libplain.so")).unwrap();
    let f = facts(&bytes);
    assert!(!f.symbolic);
    assert_eq!(f.flags & (DF_SYMBOLIC | DF_BIND_NOW), 0);
    assert_eq!(f.flags_1 & DF_1_NOW, 0);
    assert_eq!(f.relro, None);
    assert_eq!(f.mason_note, None);
    let (loads, relro) = readelf_segments(s.path(), "libplain.so");
    assert_eq!(relro, None);
    assert_eq!(&*f.loads, &loads[..]);
    assert!(f.loads.iter().any(|l| l.writable) && f.loads.iter().any(|l| !l.writable));
}

#[test]
fn d43a_bsymbolic() {
    let s = Scratch::new("sf_bsym");
    if !so(s.path(), "libsym.so", C, &["-Wl,-Bsymbolic"]) {
        return;
    }
    let bytes = std::fs::read(s.path().join("libsym.so")).unwrap();
    let f = facts(&bytes);
    // GNU ld writes both DT_SYMBOLIC and DF_SYMBOLIC for -Bsymbolic.
    assert!(f.symbolic);
    assert_ne!(f.flags & DF_SYMBOLIC, 0);
}

#[test]
fn d43a_z_now() {
    let s = Scratch::new("sf_now");
    if !so(s.path(), "libnow.so", C, &["-Wl,-z,now"]) {
        return;
    }
    let bytes = std::fs::read(s.path().join("libnow.so")).unwrap();
    let f = facts(&bytes);
    assert!(!f.symbolic);
    assert_ne!(f.flags & DF_BIND_NOW, 0);
    assert_ne!(f.flags_1 & DF_1_NOW, 0);
}

#[test]
fn d43a_z_relro() {
    let s = Scratch::new("sf_relro");
    if !so(s.path(), "librelro.so", C, &["-Wl,-z,relro"]) {
        return;
    }
    let bytes = std::fs::read(s.path().join("librelro.so")).unwrap();
    let f = facts(&bytes);
    let (loads, relro) = readelf_segments(s.path(), "librelro.so");
    assert!(relro.is_some(), "GNU ld wrote no GNU_RELRO");
    assert_eq!(f.relro, relro);
    assert_eq!(&*f.loads, &loads[..]);
}

#[test]
fn d43a_mason_note_in_pt_note() {
    let s = Scratch::new("sf_note");
    if !so(s.path(), "libnote.so", NOTED, &["-Wl,--build-id=sha1"]) {
        return;
    }
    let bytes = std::fs::read(s.path().join("libnote.so")).unwrap();
    let f = facts(&bytes);
    // The build-id note shares the PT_NOTE or has its own: either way the
    // table note is found and its descriptor returned unparsed.
    assert_eq!(f.mason_note, Some(&0x1122_3344_u32.to_le_bytes()[..]));
}

/// A program header table that runs past the end of the file is the
/// existing malformed-file error of `SharedObject::parse`, not a panic.
#[test]
fn d43a_truncated_program_headers_are_malformed() {
    let s = Scratch::new("sf_trunc");
    if !so(s.path(), "libt.so", C, &[]) {
        return;
    }
    let mut bytes = std::fs::read(s.path().join("libt.so")).unwrap();
    // e_phoff (ELF64: offset 32) at 16 bytes before the end: the table
    // (e_phnum entries of 56 bytes) does not fit.
    let at = (bytes.len() - 16) as u64;
    bytes[32..40].copy_from_slice(&at.to_le_bytes());
    match analyse(&bytes, false).kind {
        Kind::Bad(Bad::Malformed(text)) => assert!(!text.is_empty()),
        other => panic!("not malformed: {other:?}"),
    }
    // And a file cut inside the table.
    let whole = std::fs::read(s.path().join("libt.so")).unwrap();
    let cut = &whole[..64 + 20];
    assert!(matches!(analyse(cut, false).kind, Kind::Bad(_)));
}
