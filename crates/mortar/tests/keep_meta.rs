//! `--keep-mason-meta` end to end through a compiler driver (`spec/meta.md`
//! MT-30, MT-32, MT-33; WP-076): the non-allocated `.mason.meta` of a
//! static, static-pie, PIE and non-PIE dynamic executable holds the live
//! inputs' units in `FileId` order, a malformed unit dropped and an
//! unextracted member's units left out; deterministic; absent without the
//! option, present under `--odr=off`; the map names it; the program runs;
//! and what the strip tools do to it (MT-32's measurement, re-run here on
//! the harness's tool versions).

use std::ffi::OsString;
use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;

use mortar_meta::write::{Definition, Summary, section};
use mortar_meta::{UnitBuilder, class, flags, fnv1a64, producer};
use mortar_testutil::{Outcome, Scratch, run_exec_with, run_with, tool, tools_required};

const MORTAR: &str = env!("CARGO_BIN_EXE_mortar");

/// A C file whose object carries the container `bin`.
fn source(body: &str, bin: &str) -> String {
    format!(
        "{body}\n__asm__(\".section .mason.meta,\\\"e\\\",%progbits\\n.balign 8\\n\
         .incbin \\\"{bin}\\\"\\n.previous\");\n"
    )
}

fn unit(source: &str, name: &str, hash: u8) -> Vec<u8> {
    let mut u = UnitBuilder::new(producer::TEST);
    for (n, v) in [("lang", "c++"), ("producer", "test"), ("source", source)] {
        u.summary.push(Summary { name: n.into(), value: v.into(), flags: 0 });
    }
    u.definitions.push(Definition {
        key: fnv1a64(name.as_bytes()),
        hash: [hash; 16],
        name: None,
        class: class::FUNCTION,
        flags: flags::VAGUE,
        module: Vec::new(),
        file: b"h.h".to_vec(),
        line: 1,
    });
    u.finish().unwrap()
}

struct Cx {
    scratch: Scratch,
    _bin: Scratch,
    b: String,
    gcc: PathBuf,
    /// The kept section a link of `main.o g.o libu.a` must hold.
    expected: Vec<u8>,
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
        let scratch = Scratch::new(label);
        // main.o: two units; g.o: one unit, then eight bytes that are
        // neither a unit nor padding (MT-10: malformed, dropped); u.o, an
        // archive member nothing extracts: one unit, not kept.
        let (m1, m2) = (unit("main.cc", "main", 1), unit("main2.cc", "m2", 2));
        let g = unit("g.cc", "g", 3);
        let mut g_bin = section(std::slice::from_ref(&g), 0);
        g_bin.extend_from_slice(b"garbage!");
        let main_bin = section(&[m1.clone(), m2.clone()], 0);
        std::fs::write(scratch.join("main.bin"), &main_bin).unwrap();
        std::fs::write(scratch.join("g.bin"), &g_bin).unwrap();
        std::fs::write(scratch.join("u.bin"), section(&[unit("u.cc", "u", 4)], 0)).unwrap();
        scratch.write(
            "main.c",
            source(
                "#include <stdio.h>\nint g(void);\n\
                 int main(void) { printf(\"%d\\n\", g()); return 0; }",
                "main.bin",
            ),
        );
        scratch.write("g.c", source("int g(void) { return 7; }", "g.bin"));
        scratch.write("u.c", source("int u(void) { return 4; }", "u.bin"));
        let mut expected = Vec::new();
        for u in [&m1, &m2, &g] {
            expected.extend_from_slice(u);
            expected.resize(expected.len().next_multiple_of(8), 0);
        }
        let cx = Self { scratch, _bin: bin, b, gcc, expected };
        for f in ["main", "g", "u"] {
            let o = cx.gcc(&["-O1", "-fPIE", "-c", &format!("{f}.c"), "-o", &format!("{f}.o")]);
            assert!(o.success(), "gcc {f}: {}", o.stderr_text());
        }
        let ar = tool("ar").expect("ar is required");
        let mut c = Command::new(ar);
        c.args(["rcs", "libu.a", "u.o"]).current_dir(cx.scratch.path());
        assert!(run_with(&mut c, b"", Duration::from_mins(1)).success());
        Some(cx)
    }

    fn gcc(&self, args: &[&str]) -> Outcome {
        let mut c = Command::new(&self.gcc);
        c.args(args).current_dir(self.scratch.path()).env_remove("MORTAR_SCHED_SEED");
        run_with(&mut c, b"", Duration::from_mins(2))
    }

    /// gcc with mortar as its `ld`: `kind main.o g.o libu.a extra -o out`;
    /// it must succeed.
    fn link(&self, kind: &str, extra: &[&str], out: &str) -> Outcome {
        let mut a = vec![self.b.as_str(), kind, "main.o", "g.o", "libu.a"];
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
                .current_dir(&dir);
                c
            },
            b"",
            Duration::from_mins(1),
        )
    }

    /// `tool args... file` on a copy of `from` named `to`; it must succeed.
    fn edit(&self, tool_name: &str, args: &[&str], from: &str, to: &str) {
        std::fs::copy(self.scratch.join(from), self.scratch.join(to)).unwrap();
        let t = tool(tool_name).unwrap_or_else(|| panic!("{tool_name} is required"));
        let mut c = Command::new(t);
        c.args(args).arg(to).current_dir(self.scratch.path());
        let o = run_with(&mut c, b"", Duration::from_mins(1));
        assert!(o.success(), "{tool_name} {args:?}: {}", o.stderr_text());
    }
}

/// A section of an ELF64 little-endian image: (type, flags, addralign,
/// contents).
type Section = (u32, u64, u64, Vec<u8>);

/// The section named `name` in `image`, if any.
fn section_of(image: &[u8], name: &[u8]) -> Option<Section> {
    let u16_at = |o: usize| u16::from_le_bytes(image[o..o + 2].try_into().unwrap()) as usize;
    let u32_at = |o: usize| u32::from_le_bytes(image[o..o + 4].try_into().unwrap());
    let u64_at = |o: usize| u64::from_le_bytes(image[o..o + 8].try_into().unwrap());
    let shoff = u64_at(0x28) as usize;
    let (entsize, count, strndx) = (u16_at(0x3a), u16_at(0x3c), u16_at(0x3e));
    let header = |i: usize| shoff + i * entsize;
    let strtab = u64_at(header(strndx) + 24) as usize;
    for i in 0..count {
        let h = header(i);
        let at = strtab + u32_at(h) as usize;
        let end = at + image[at..].iter().position(|&b| b == 0)?;
        if &image[at..end] != name {
            continue;
        }
        let (ty, flags, off, size, align) =
            (u32_at(h + 4), u64_at(h + 8), u64_at(h + 24) as usize, u64_at(h + 32), u64_at(h + 48));
        let contents = if ty == 8 { Vec::new() } else { image[off..off + size as usize].to_vec() };
        return Some((ty, flags, align, contents));
    }
    None
}

// spec: meta.md MT-30, MT-32, MT-33 (WP-076): each output kind gets the
// non-allocated `.mason.meta` (SHT_PROGBITS, no flags, alignment 8) with
// the live inputs' units in FileId order: main.o's two, g.o's one (its
// malformed tail dropped), not the unextracted member's; deterministic;
// none without the option; the same under --odr=off and --odr=error (an
// output request, not a check); under --odr=strict g.o's malformed tail is
// meta-malformed, an error (O-41); the map names it; the program runs.
#[test]
fn kept_in_every_output_kind() {
    let Some(cx) = Cx::new("keep-meta") else { return };
    for kind in ["-static", "-static-pie", "-pie", "-no-pie"] {
        let map = format!("-Wl,-Map,{kind}.map");
        cx.link(kind, &["-Wl,--keep-mason-meta", &map], "k1");
        cx.link(kind, &["-Wl,--keep-mason-meta"], "k2");
        cx.link(kind, &[], "plain");
        cx.link(kind, &["-Wl,--keep-mason-meta,--odr=off"], "off");
        cx.link(kind, &["-Wl,--keep-mason-meta,--odr=error"], "error");
        let o = cx.gcc(&[&cx.b, kind, "main.o", "g.o", "-Wl,--keep-mason-meta,--odr=strict"]);
        assert!(!o.success(), "{kind}: strict");
        assert!(o.stderr_text().contains("neither a unit nor zero padding"), "{kind}");
        let k1 = cx.bytes("k1");
        assert_eq!(k1, cx.bytes("k2"), "{kind}: two links differ");
        let (ty, flags, align, contents) = section_of(&k1, b".mason.meta").expect(kind);
        assert_eq!((ty, flags, align), (1, 0, 8), "{kind}: SHT_PROGBITS, no flags, align 8");
        assert_eq!(contents, cx.expected, "{kind}: the units");
        assert!(section_of(&cx.bytes("plain"), b".mason.meta").is_none(), "{kind}");
        for other in ["off", "error"] {
            let s = section_of(&cx.bytes(other), b".mason.meta").expect(other);
            assert_eq!(s.3, cx.expected, "{kind} {other}");
        }
        let map = std::fs::read_to_string(cx.scratch.join(format!("{kind}.map"))).unwrap();
        assert!(map.lines().any(|l| l.starts_with(".mason.meta")), "{kind}: the map: {map}");
        for out in ["k1", "plain"] {
            let o = cx.run(out);
            assert!(o.success(), "{kind} {out}: {}", o.stderr_text());
            assert_eq!(o.stdout_text(), "7\n", "{kind} {out}");
        }
    }
}

// spec: meta.md MT-32 (WP-076): what the strip tools do to the kept
// section (MT-32's `[unverified]`, measured): GNU strip in each mode and
// `objcopy --strip-all` keep it; `llvm-strip` (its default `--strip-all`)
// and `eu-strip` remove it, and `eu-strip -f` moves it to the debug file;
// `llvm-strip --strip-debug` keeps it; `objcopy --only-keep-debug` and
// `llvm-objcopy --only-keep-debug` keep its contents.
#[test]
fn what_the_strip_tools_do() {
    let Some(cx) = Cx::new("keep-meta-strip") else { return };
    cx.link("-static", &["-Wl,--keep-mason-meta"], "k");
    let cases: &[(&str, &[&str], bool)] = &[
        ("strip", &[], true),
        ("strip", &["--strip-debug"], true),
        ("strip", &["--strip-all"], true),
        ("objcopy", &["--strip-all"], true),
        ("objcopy", &["--only-keep-debug"], true),
        ("llvm-objcopy", &["--only-keep-debug"], true),
        ("llvm-strip", &[], false),
        ("llvm-strip", &["--strip-debug"], true),
        ("eu-strip", &[], false),
        ("eu-strip", &["-f", "k.debug"], false),
    ];
    for &(t, args, kept) in cases {
        cx.edit(t, args, "k", "s");
        let s = section_of(&cx.bytes("s"), b".mason.meta");
        assert_eq!(s.is_some(), kept, "{t} {args:?}");
        if let Some(s) = s {
            assert_eq!(s.3, cx.expected, "{t} {args:?}: the contents");
        }
    }
    let debug = section_of(&cx.bytes("k.debug"), b".mason.meta").expect("eu-strip -f");
    assert_eq!(debug.3, cx.expected, "eu-strip -f: the debug file's contents");
}
