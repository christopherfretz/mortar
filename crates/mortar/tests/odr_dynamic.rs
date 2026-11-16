//! The `.note.mason` table and O-24 end to end through a compiler driver
//! (`spec/odr.md` O-24, O-50 to O-52; WP-075): the table's bytes in a
//! dynamic executable, read back, deterministic, absent without
//! `--odr-table`, and the program runs; and the read counter of a link
//! against a library without a table. The shapes of O-24 with tables are
//! `tests/link/dyn/odr/o24-*.test`.

use std::ffi::OsString;
use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;

use mortar_meta::table::{DESC_OFFSET, FullTable, NT_MASON_ODR_FULL};
use mortar_meta::write::{Definition, Summary, Type, section};
use mortar_meta::{NO_DETAILS, UnitBuilder, class, flags, fnv1a64, producer};
use mortar_testutil::{Outcome, Scratch, run_exec_with, run_with, tool, tools_required};

const MORTAR: &str = env!("CARGO_BIN_EXE_mortar");

/// The container is in the file that defines f: a row is anchored to a
/// symbol of its own file.
const MAIN: &str = "#include <stdio.h>\nint f(void) { return 2; }\n\
    __attribute__((visibility(\"hidden\"))) int h(void) { return 0; }\n\
    int main(void) { printf(\"%d\\n\", f() + h()); return 0; }\n\
    __asm__(\".section .mason.meta,\\\"e\\\",%progbits\\n.balign 8\\n\
    .incbin \\\"m.bin\\\"\\n.previous\");\n";
const LIB: &str = "int f(void) { return 1; }\nint g(void) { return f(); }\n";

struct Cx {
    scratch: Scratch,
    _bin: Scratch,
    b: String,
    gcc: PathBuf,
}

impl Cx {
    fn new(label: &str) -> Option<Self> {
        if std::env::consts::ARCH != "aarch64" {
            return None;
        }
        let Some(gcc) = tool("gcc") else {
            assert!(!tools_required(), "gcc is required");
            return None;
        };
        let bin = Scratch::new(&format!("{label}-bin"));
        std::os::unix::fs::symlink(MORTAR, bin.join("ld")).unwrap();
        let b = format!("-B{}/", bin.path().display());
        let cx = Self { scratch: Scratch::new(label), _bin: bin, b, gcc };
        // main.o: f, exported under -rdynamic, with its kind 2 row (VAGUE).
        let mut u = UnitBuilder::new(producer::TEST);
        for (n, v) in [("lang", "c++"), ("producer", "test"), ("source", "main.cc")] {
            u.summary.push(Summary { name: n.into(), value: v.into(), flags: 0 });
        }
        u.definitions.push(Definition {
            key: fnv1a64(b"f"),
            hash: [2; 16],
            name: None,
            class: class::FUNCTION,
            flags: flags::VAGUE,
            module: Vec::new(),
            file: b"h.h".to_vec(),
            line: 3,
        });
        // h: hidden, a vague definition only `=full` writes (WP-076).
        u.definitions.push(Definition {
            key: fnv1a64(b"h"),
            hash: [3; 16],
            name: None,
            class: class::FUNCTION,
            flags: flags::VAGUE,
            module: Vec::new(),
            file: b"h.h".to_vec(),
            line: 4,
        });
        u.types.push(Type {
            key: fnv1a64(b"1S"),
            hash: [4; 16],
            layout: 0x55,
            name: b"1S".to_vec(),
            size: 8,
            align: 4,
            flags: 0,
            file: b"h.h".to_vec(),
            line: 5,
            details: NO_DETAILS,
        });
        std::fs::write(cx.scratch.join("m.bin"), section(&[u.finish().unwrap()], 0)).unwrap();
        cx.scratch.write("main.c", MAIN);
        cx.scratch.write("lib.c", LIB);
        for a in [
            &["-O1", "-fPIE", "-c", "main.c", "-o", "main.o"][..],
            &["-O1", "-fPIC", "-shared", "lib.c", "-o", "liba.so"],
        ] {
            let o = cx.gcc(a);
            assert!(o.success(), "gcc {a:?}: {}", o.stderr_text());
        }
        Some(cx)
    }

    fn gcc(&self, args: &[&str]) -> Outcome {
        let mut c = Command::new(&self.gcc);
        c.args(args).current_dir(self.scratch.path()).env_remove("MORTAR_SCHED_SEED");
        run_with(&mut c, b"", Duration::from_mins(2))
    }

    /// gcc with mortar as its `ld`: `-pie -rdynamic main.o`, `extra`,
    /// to `out`; it must succeed.
    fn link(&self, extra: &[&str], out: &str) -> Outcome {
        let mut a = vec![self.b.as_str(), "-pie", "-rdynamic", "main.o"];
        a.extend_from_slice(extra);
        a.extend(["-o", out]);
        let o = self.gcc(&a);
        assert!(o.success(), "link {a:?}: {}", o.stderr_text());
        o
    }

    fn bytes(&self, out: &str) -> Vec<u8> {
        std::fs::read(self.scratch.join(out)).unwrap()
    }

    fn run(&self, out: &str) -> Outcome {
        let prog = self.scratch.join(out);
        let dir = self.scratch.path().to_path_buf();
        run_exec_with(
            || {
                let mut c = Command::new("sh");
                c.args([
                    OsString::from("-c"),
                    "ulimit -c 0; exec \"$0\"".into(),
                    prog.clone().into(),
                ])
                .current_dir(&dir)
                .env("LD_LIBRARY_PATH", &dir);
                c
            },
            b"",
            Duration::from_mins(1),
        )
    }
}

/// The note in `image`, found by its owner: (`n_descsz`, the descriptor).
fn note(image: &[u8]) -> Option<(u32, &[u8])> {
    let owner = b"MasonMeta\0";
    let at = image.windows(owner.len()).position(|w| w == owner)?;
    let head = at.checked_sub(12)?;
    let word = |o: usize| u32::from_le_bytes(image[o..o + 4].try_into().unwrap());
    assert_eq!(word(head), 10, "n_namesz");
    assert_eq!(word(head + 8), 0x4d4f_4401, "n_type");
    let size = word(head + 4);
    Some((size, &image[head + 24..head + 24 + size as usize]))
}

fn stats(o: &Outcome) -> String {
    let e = o.stderr_text();
    e.lines()
        .find(|l| l.contains("odr-stats:"))
        .unwrap_or_else(|| panic!("no stats: {e}"))
        .to_owned()
}

/// The number after `what` in a stats line.
fn number(line: &str, what: &str) -> u64 {
    let at = line.find(what).unwrap_or_else(|| panic!("{what}: {line}")) + what.len();
    line[at..].split(|c: char| !c.is_ascii_digit()).next().unwrap().parse().unwrap()
}

// spec: odr.md O-50, O-51, O-52 (WP-075): `--odr-table=exported` writes
// one row for the exported vague definition f: key FNV-1a 64 of "f", the
// keeper's hash's first 8 bytes, a `.dynsym` index, VAGUE and
// NONPREEMPTIBLE (an executable binds its own definitions). Two links
// write the same bytes; without the option no table is written; the
// program runs.
#[test]
fn table_in_a_dynamic_executable() {
    let Some(cx) = Cx::new("odr-dyn-table") else { return };
    cx.link(&["-Wl,--odr-table=exported"], "t1");
    cx.link(&["-Wl,--odr-table=exported"], "t2");
    cx.link(&[], "plain");
    let t1 = cx.bytes("t1");
    assert_eq!(t1, cx.bytes("t2"), "the table's links differ");
    let (size, desc) = note(&t1).expect("no MasonMeta note");
    assert_eq!(size, 16 + 24);
    let word = |o: usize| u32::from_le_bytes(desc[o..o + 4].try_into().unwrap());
    assert_eq!((word(0), word(4), word(8), word(12)), (1, 1, 24, 0), "header");
    assert_eq!(u64::from_le_bytes(desc[16..24].try_into().unwrap()), fnv1a64(b"f"));
    assert_eq!(&desc[24..32], &[2; 8]);
    assert_ne!(word(32), 0, "dynsym index");
    let row_flags = u16::from_le_bytes([desc[36], desc[37]]);
    assert_eq!(row_flags, flags::VAGUE | 0x8000);
    assert_eq!(&desc[38..40], &[0, 0]);
    assert!(note(&cx.bytes("plain")).is_none(), "a table without --odr-table");
    // `=off` is the default's bytes.
    cx.link(&["-Wl,--odr-table=off"], "off");
    assert_eq!(cx.bytes("off"), cx.bytes("plain"));
    for out in ["t1", "plain"] {
        let o = cx.run(out);
        assert!(o.success(), "{out}: {}", o.stderr_text());
        assert_eq!(o.stdout_text(), "2\n", "{out}");
    }
}

/// Every `MasonMeta` note of `image` after the first: (`n_type`,
/// descriptor), found by their owner.
fn later_notes(image: &[u8]) -> Vec<(u32, &[u8])> {
    let owner = b"MasonMeta\0";
    let word = |o: usize| u32::from_le_bytes(image[o..o + 4].try_into().unwrap());
    let mut out = Vec::new();
    let mut from = 0;
    while let Some(p) = image[from..].windows(owner.len()).position(|w| w == owner) {
        let head = from + p - 12;
        let size = word(head + 4) as usize;
        out.push((word(head + 8), &image[head + DESC_OFFSET..head + DESC_OFFSET + size]));
        from = head + DESC_OFFSET + size;
    }
    out.split_off(1.min(out.len()))
}

// spec: odr.md O-50, O-51, O-52 (WP-076): `--odr-table=full` writes the
// `=exported` note unchanged and, after it in the same section, a second
// note (`NT_MASON_ODR_FULL`): the hidden vague definition h (its key, its
// hash's first 8 bytes, VAGUE without NONPREEMPTIBLE; f is exported and is
// only in the first note) and the type 1S. Deterministic; under --odr=off
// no table at all; the program runs.
#[test]
fn full_table_in_a_dynamic_executable() {
    let Some(cx) = Cx::new("odr-dyn-full") else { return };
    cx.link(&["-Wl,--odr-table=full"], "f1");
    cx.link(&["-Wl,--odr-table=full"], "f2");
    cx.link(&["-Wl,--odr-table=exported"], "e");
    let f1 = cx.bytes("f1");
    assert_eq!(f1, cx.bytes("f2"), "=full's links differ");
    assert_eq!(note(&f1), note(&cx.bytes("e")), "the first note");
    let later = later_notes(&f1);
    assert_eq!(later.len(), 1, "one second note");
    let (ty, desc) = later[0];
    assert_eq!(ty, NT_MASON_ODR_FULL);
    let t = FullTable::parse(desc).unwrap();
    let defs: Vec<_> = t.definitions().collect();
    assert_eq!(defs.len(), 1, "{defs:?}");
    assert_eq!((defs[0].key, defs[0].hash, defs[0].flags), (fnv1a64(b"h"), [3; 8], flags::VAGUE));
    assert_eq!((defs[0].layout, defs[0].size, defs[0].align), (0, 0, 0));
    let types: Vec<_> = t.types().collect();
    assert_eq!(types.len(), 1, "{types:?}");
    let ty = types[0];
    assert_eq!(
        (ty.key, ty.hash, ty.layout, ty.size, ty.align),
        (fnv1a64(b"1S"), [4; 8], 0x55, 8, 4)
    );
    assert!(later_notes(&cx.bytes("e")).is_empty(), "=exported writes one note");
    cx.link(&["-Wl,--odr-table=full,--odr=off"], "off");
    cx.link(&[], "plain");
    assert_eq!(cx.bytes("off"), cx.bytes("plain"), "--odr=off writes no table");
    let o = cx.run("f1");
    assert!(o.success(), "{}", o.stderr_text());
    assert_eq!(o.stdout_text(), "2\n");
}

// spec: odr.md O-24 (WP-075): a library without a table (the system
// libraries, liba.so here) is silent under --odr=strict, no table is
// read, and the container bytes read are those of the link without the
// library: no byte beyond today's. With the table off and the default
// mode the counters are those of today's links.
#[test]
fn a_library_without_a_table_reads_nothing_more() {
    let Some(cx) = Cx::new("odr-dyn-reads") else { return };
    let strict = ["-Wl,--odr=strict,--odr-stats"];
    let without = stats(&cx.link(&strict, "s0"));
    let with = stats(&cx.link(&[strict[0], "-L.", "-Wl,--no-as-needed", "-la"], "s1"));
    for line in [&without, &with] {
        assert!(line.contains("shared-object tables 0 (0 rows)"), "{line}");
        assert!(number(line, "container bytes read ") > 0, "{line}");
    }
    assert_eq!(number(&without, "container bytes read "), number(&with, "container bytes read "));
    let o = cx.link(&["-Wl,--odr-stats", "-L.", "-Wl,--no-as-needed", "-la"], "w1");
    let line = stats(&o);
    assert!(line.contains("shared-object tables 0 (0 rows)"), "{line}");
    assert!(!o.stderr_text().contains("one-definition"), "{}", o.stderr_text());
    // liba's f runs the executable's copy: the program prints 2 either way.
    let o = cx.run("s1");
    assert_eq!(o.stdout_text(), "2\n", "{}", o.stderr_text());
}
