//! `spec/input.md` §2: what a file is.
mod common;

use mortar_diag::RefusalClass;
use mortar_input::facts::{Bad, Kind, analyse};
use mortar_testutil::Scratch;

const C: &str = "int f(void) { return 1; }\n";

fn refused(bytes: &[u8]) -> Option<(RefusalClass, String)> {
    match analyse(bytes, false).kind {
        Kind::Bad(Bad::Refused { class, name, .. }) => Some((class, name)),
        _ => None,
    }
}

#[test]
fn s2_1_sniff_table() {
    let pad = |head: &[u8]| {
        let mut v = head.to_vec();
        v.resize(32, b' ');
        v
    };
    let ff = |n: &str| Some((RefusalClass::FileFormat, n.to_owned()));
    assert_eq!(refused(&pad(b"BC\xC0\xDE")), ff("LLVM bitcode"));
    assert_eq!(refused(&pad(b"\xDE\xC0\x17\x0B")), ff("LLVM bitcode"));
    for m in [
        &b"\xFE\xED\xFA\xCE"[..],
        b"\xFE\xED\xFA\xCF",
        b"\xCE\xFA\xED\xFE",
        b"\xCF\xFA\xED\xFE",
        b"\xCA\xFE\xBA\xBE",
    ] {
        assert_eq!(refused(&pad(m)), ff("Mach-O"));
    }
    assert_eq!(refused(&pad(b"\0asm")), ff("WebAssembly"));
    for m in [&b"\x4C\x01"[..], b"\x64\x86", b"\x64\xAA"] {
        assert_eq!(refused(&pad(m)), ff("COFF"));
    }
    // A text that starts with `d` and 0x86 is COFF; a short prefix is not.
    assert_eq!(refused(&pad(b"d\x86 INPUT(a.o)")), ff("COFF"));
    assert!(!matches!(analyse(b"\x64\x86 short", false).kind, Kind::Bad(_)));
    assert!(matches!(analyse(b"", false).kind, Kind::Empty));
    assert!(matches!(analyse(b"INPUT(a.o)\n", false).kind, Kind::Script(_)));
    assert!(matches!(analyse(b"\x01\x02\x03 garbage", false).kind, Kind::Unknown));
    // §13: the GNU ld phrase is part of the refusal's text.
    let s = Scratch::new("s2sniff");
    s.write("m.o", pad(b"\xCF\xFA\xED\xFE"));
    let out = common::run(s.path(), &["-maarch64linux", "m.o"]);
    assert_eq!(
        common::report(&out),
        [
            "error: m.o: unsupported file format: Mach-O; file not recognized: file format not recognized"
        ]
    );
    s.write("u.o", b"\x01\x02 garbage");
    let out = common::run(s.path(), &["-maarch64linux", "u.o"]);
    assert_eq!(common::ids(&out), ["unknown-file-type"]);
    assert_eq!(
        common::report(&out),
        ["error: u.o: file not recognized: file format not recognized"]
    );
}

#[test]
fn s2_1_extension_is_ignored() {
    let s = Scratch::new("s2ext");
    let Some(_) = common::cc(s.path(), "native", "x.so", C, &[]) else { return };
    s.write("x.o", "INPUT(x.so)\n");
    let out = common::run(s.path(), &["-maarch64linux", "x.o"]);
    assert!(out.problems.is_empty(), "{:?}", common::report(&out));
    assert_eq!(common::names(&out), ["x.so"]);
    assert_eq!(out.files[0].kind, mortar_input::walk::RowKind::Object);
}

fn patch(bytes: &[u8], at: usize, with: &[u8]) -> Vec<u8> {
    let mut v = bytes.to_vec();
    v[at..at + with.len()].copy_from_slice(with);
    v
}

#[test]
fn s2_2_e_type() {
    let s = Scratch::new("s2etype");
    let Some(o) = common::cc(s.path(), "native", "a.o", C, &[]) else { return };
    let bytes = std::fs::read(o).unwrap();
    let ot = |n: &str| Some((RefusalClass::ObjectType, n.to_owned()));
    assert_eq!(refused(&patch(&bytes, 16, &2u16.to_le_bytes())), ot("ET_EXEC"));
    assert_eq!(refused(&patch(&bytes, 16, &4u16.to_le_bytes())), ot("ET_CORE"));
    assert_eq!(refused(&patch(&bytes, 16, &0u16.to_le_bytes())), ot("ET_NONE"));
    assert_eq!(refused(&patch(&bytes, 16, &0xfe01u16.to_le_bytes())), ot("0xfe01"));
    assert!(matches!(analyse(&bytes, false).kind, Kind::Object(_)));
    // A real executable.
    let Some(exe) =
        common::cc(s.path(), "native", "exe", "int main(void){return 0;}\n", &["-no-pie"])
    else {
        return;
    };
    let _ = exe;
}

#[test]
fn s2_2_gcc_slim_lto_refused() {
    let s = Scratch::new("s2lto");
    let Some(_) = common::cc(s.path(), "native", "slim.o", C, &["-flto", "-fno-fat-lto-objects"])
    else {
        return;
    };
    let out = common::run(s.path(), &["-maarch64linux", "slim.o"]);
    assert_eq!(
        common::report(&out),
        [
            "error: slim.o: unsupported file format: GCC LTO object\n>>> mortar does not perform link-time optimization; compile without -flto, or with -ffat-lto-objects"
        ]
    );
}

#[test]
fn s2_2_gcc_fat_lto_linked() {
    let s = Scratch::new("s2fat");
    let Some(_) = common::cc(s.path(), "native", "fat.o", C, &["-flto", "-ffat-lto-objects"])
    else {
        return;
    };
    let out = common::run(s.path(), &["-maarch64linux", "fat.o"]);
    assert!(out.problems.is_empty(), "{:?}", common::report(&out));
    assert_eq!(common::names(&out), ["fat.o"]);
}

#[test]
fn s2_2_flto_line_refused_naming_lto() {
    let s = Scratch::new("s2fltoline");
    let Some(gcc) = common::need("gcc") else { return };
    let Some(_) = common::cc(s.path(), "native", "m.o", "int main(void){return 0;}\n", &["-flto"])
    else {
        return;
    };
    let args = common::driver_link_args(&gcc, &["-flto", "m.o"], s.path()).unwrap();
    assert!(args.iter().any(|a| a == "-plugin"), "the line has -plugin");
    let out = common::run(s.path(), &args);
    let r = common::report(&out);
    assert!(
        r.iter().any(|l| l.starts_with("error: m.o: unsupported file format: GCC LTO object")),
        "{r:?}"
    );
}

#[test]
fn s2_3_empty_file() {
    let s = Scratch::new("s2empty");
    s.write("e.o", b"");
    let out = common::run(s.path(), &["-maarch64linux", "e.o"]);
    assert!(out.problems.is_empty());
    assert!(out.files.is_empty());
    assert_eq!(common::opened(&out), ["e.o"]);
}

#[test]
fn s2_3_dev_null() {
    let s = Scratch::new("s2null");
    for args in [&["-maarch64linux", "/dev/null"][..], &["-maarch64linux", "-r", "/dev/null"]] {
        let out = common::run(s.path(), args);
        assert!(out.problems.is_empty(), "{:?}", common::report(&out));
        assert_eq!(common::opened(&out), ["/dev/null"]);
        assert_eq!(out.link.opened[0].kind, mortar_input::walk::OpenedKind::Empty);
    }
}

#[test]
fn s2_4_header_errors() {
    let s = Scratch::new("s2hdr");
    let Some(o) = common::cc(s.path(), "native", "a.o", C, &[]) else { return };
    let bytes = std::fs::read(o).unwrap();
    let ff = |n: &str| Some((RefusalClass::FileFormat, n.to_owned()));
    assert_eq!(refused(&patch(&bytes, 5, &[2])), ff("big-endian ELF"));
    assert_eq!(
        refused(&patch(&bytes, 18, &40u16.to_le_bytes())),
        Some((RefusalClass::Machine, "EM_ARM".to_owned()))
    );
    assert_eq!(
        refused(&patch(&bytes, 18, &0x7777u16.to_le_bytes())),
        Some((RefusalClass::Machine, "0x7777".to_owned()))
    );
    if let Some(x32) = common::cc(s.path(), "x86", "x32.o", C, &["-mx32"]) {
        assert_eq!(refused(&std::fs::read(x32).unwrap()), ff("x32"));
    }
    // ELF64 EM_386, ILP32: the class byte against the machine.
    let Some(i) = common::cc(s.path(), "i686", "i.o", C, &[]) else { return };
    let ib = std::fs::read(i).unwrap();
    assert_eq!(refused(&patch(&ib, 4, &[2])).map(|r| r.0), Some(RefusalClass::FileFormat));
    // Truncated and corrupt headers are malformed, with the reader's text.
    for broken in [&bytes[..10], &patch(&bytes, 4, &[9])[..], &patch(&bytes, 6, &[7])[..]] {
        assert!(
            matches!(analyse(broken, false).kind, Kind::Bad(Bad::Malformed(_))),
            "{:?}",
            analyse(broken, false).kind
        );
    }
    s.write("t.o", &bytes[..40]);
    let out = common::run(s.path(), &["-maarch64linux", "t.o"]);
    assert_eq!(common::ids(&out), ["malformed-input"]);
}

#[test]
fn s2_5_directory() {
    let s = Scratch::new("s2dir");
    std::fs::create_dir(s.join("d")).unwrap();
    let out = common::run(s.path(), &["-maarch64linux", "d"]);
    assert_eq!(
        common::report(&out),
        [
            "error: d: cannot open d: Is a directory; file not recognized: file format not recognized"
        ]
    );
}

#[test]
fn s2_5_fifo_not_opened() {
    let s = Scratch::new("s2fifo");
    let Some(mkfifo) = common::need("mkfifo") else { return };
    assert!(common::tool_run(&mkfifo, &["p".into()], s.path()).success());
    // No writer: an open would block; the phase returns.
    let store = mortar_input::store::Store::new(mortar_input::store::StorePolicy::Resident);
    let out = mortar_input::walk::walk(
        &common::plan(&["-maarch64linux", "p"]),
        &mortar_input::walk::LoadEnv { cwd: s.path().to_path_buf() },
        &store,
        &mortar_core::par::Pool::sequential(),
        mortar_input::walk::Options::default(),
    );
    assert_eq!(common::report(&out), ["error: p: is a FIFO"]);
    assert_eq!(store.stats().opens, 0, "the FIFO was never opened");
}

#[test]
fn s2_5_socket() {
    let s = Scratch::new("s2sock");
    let _l = std::os::unix::net::UnixListener::bind(s.join("sk")).unwrap();
    let out = common::run(s.path(), &["-maarch64linux", "sk"]);
    assert_eq!(common::report(&out), ["error: sk: is a socket"]);
}

#[test]
fn s2_5_nonempty_char_device() {
    let s = Scratch::new("s2zero");
    let out = common::run(s.path(), &["-maarch64linux", "/dev/zero"]);
    assert_eq!(common::report(&out), ["error: /dev/zero: not a regular file and not empty"]);
}

#[test]
fn s2_6_formats() {
    let s = Scratch::new("s2fmt");
    let Some(_) = common::cc(s.path(), "native", "a.o", C, &[]) else { return };
    let out = common::run(s.path(), &["-maarch64linux", "-b", "binary", "a.o"]);
    assert_eq!(common::report(&out), ["error: argument 'a.o': unsupported file format: binary"]);
    let out = common::run(s.path(), &["-maarch64linux", "-b", "elf64-littleaarch64", "a.o"]);
    assert!(out.problems.is_empty(), "{:?}", common::report(&out));
    assert_eq!(out.files.len(), 1);
    let out = common::run(s.path(), &["-maarch64linux", "-b", "elf32-i386", "a.o"]);
    assert_eq!(
        common::report(&out),
        ["error: argument 'a.o': unsupported file format: elf32-i386"]
    );
    s.write("t.ld", "INPUT(a.o)\n");
    let out = common::run(s.path(), &["-maarch64linux", "-b", "elf64-littleaarch64", "t.ld"]);
    assert_eq!(common::report(&out), ["error: t.ld: unsupported file format: elf64-littleaarch64"]);
}
