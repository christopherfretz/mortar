//! G-2's detection (WP-229; `spec/incr.md` IN-55, IN-55a) on objects
//! compiled here: `update::split::classify_object` on R10's
//! configurations, by GCC and Clang, for AArch64 and x86-64. Each row is
//! the answer `tests/incr-design/wp229/detect.sh` recorded (WP-229's
//! record, "Detection"); `classify_rows` on stored rows.

use std::path::Path;
use std::process::Command;

use mortar_core::incr::IncrReason;
use mortar_incr::format::SectionRow;
use mortar_incr::update::split::{Handled, classify_object, classify_rows};
use mortar_incr::update::testing::MemoryState;
use mortar_incr::update::view::ChangedFiles;
use mortar_testutil::target::Arch;
use mortar_testutil::{DEFAULT_TIMEOUT, Scratch, run_with, tool};

const SOURCE: &str = "template <class T> struct Box { T v; T get() const { return v; } };\n\
                      struct Pt { int x, y; int sum() const { return x + y; } };\n\
                      int u(int c) { Box<long> b{c}; Pt p{1, c}; return (int)b.get() + p.sum(); }\n";

/// What a configuration must give.
#[derive(Debug)]
enum Want {
    /// Handled, with these counts (compile units, skeletons, DWARF 4
    /// split, GNU index).
    Handled(u32, u32, bool, bool),
    /// Handled; the GNU index present for GCC, absent for Clang
    /// (`-gsplit-dwarf -gno-pubnames`: GCC 14 ignores `-gno-pubnames`).
    HandledGnuIndexGccOnly(u32, u32, bool),
    /// `debug-name-index` naming this section (GCC's, Clang's).
    NameIndex(&'static str, &'static str),
    /// `debug-unit-kind` whose detail contains this.
    UnitKind(&'static str),
}

/// R10's configurations and WP-229's extensions: (flags, answer).
const ROWS: &[(&str, Want)] = &[
    ("-gdwarf-4", Want::Handled(1, 0, false, false)),
    ("-gdwarf-5", Want::Handled(1, 0, false, false)),
    ("-gdwarf-4 -fdebug-types-section", Want::UnitKind(".debug_types")),
    ("-gdwarf-5 -fdebug-types-section", Want::UnitKind("COMDAT")),
    ("-gdwarf-4 -gsplit-dwarf", Want::Handled(1, 0, true, true)),
    ("-gdwarf-5 -gsplit-dwarf", Want::Handled(0, 1, false, true)),
    ("-gdwarf-5 -gpubnames", Want::NameIndex(".debug_pubnames", ".debug_names")),
    ("-gdwarf-5 -ggnu-pubnames", Want::Handled(1, 0, false, true)),
    ("-gdwarf-4 -ggnu-pubnames", Want::Handled(1, 0, false, true)),
    ("-gdwarf-4 -gsplit-dwarf -gno-pubnames", Want::HandledGnuIndexGccOnly(1, 0, true)),
    ("-gdwarf-5 -gsplit-dwarf -gno-pubnames", Want::HandledGnuIndexGccOnly(0, 1, false)),
    ("-gdwarf-5 -gsplit-dwarf -fdebug-types-section", Want::Handled(0, 1, false, true)),
];

fn compile(compiler: &Path, pre: &[String], flags: &str, dir: &Scratch, name: &str) -> Vec<u8> {
    let src = dir.write(&format!("{name}.cc"), SOURCE);
    let obj = dir.join(format!("{name}.o"));
    let mut cmd = Command::new(compiler);
    cmd.current_dir(dir.path()).args(pre).args(["-O0", "-g"]).args(flags.split_whitespace());
    cmd.arg("-c").arg(&src).arg("-o").arg(&obj);
    let out = run_with(&mut cmd, b"", DEFAULT_TIMEOUT);
    assert!(out.success(), "{} {flags}: {}", compiler.display(), out.stderr_text());
    std::fs::read(&obj).expect("the object")
}

fn check(label: &str, gcc: bool, flags: &str, want: &Want, got: &Result<Handled, IncrReason>) {
    let ctx = format!("{label} {flags}: {got:?}");
    match (want, got) {
        (Want::Handled(cu, sk, d4, gi), Ok(h)) => {
            assert_eq!(
                *h,
                Handled { compile_units: *cu, skeletons: *sk, dwarf4_split: *d4, gnu_index: *gi },
                "{ctx}"
            );
        }
        (Want::HandledGnuIndexGccOnly(cu, sk, d4), Ok(h)) => {
            let want =
                Handled { compile_units: *cu, skeletons: *sk, dwarf4_split: *d4, gnu_index: gcc };
            assert_eq!(*h, want, "{ctx}");
        }
        (Want::NameIndex(g, c), Err(IncrReason::DebugNameIndex(d))) => {
            assert!(d.contains(if gcc { g } else { c }), "{ctx}");
        }
        (Want::UnitKind(s), Err(IncrReason::DebugUnitKind(d))) => assert!(d.contains(s), "{ctx}"),
        _ => panic!("{ctx}: want {want:?}"),
    }
}

fn run_compiler(label: &str, gcc: bool, compiler: &Path, pre: &[String]) {
    let dir = Scratch::new(&format!("update-split-{label}"));
    for (i, (flags, want)) in ROWS.iter().enumerate() {
        let bytes = compile(compiler, pre, flags, &dir, &format!("u{i}"));
        let object = mortar_elf::Object::parse(&bytes).expect("an ELF object");
        check(label, gcc, flags, want, &classify_object(&object));
    }
}

// spec: incr.md IN-55 IN-55a (WP-229: R10's configurations, GCC)
#[test]
fn classify_object_gcc() {
    for arch in [Arch::Aarch64, Arch::X86_64] {
        let Some(gxx) = arch.tool("g++") else { return };
        run_compiler(&format!("gcc-{}", arch.name()), true, &gxx, &[]);
    }
}

// spec: incr.md IN-55 IN-55a (WP-229: R10's configurations, Clang)
#[test]
fn classify_object_clang() {
    let Some(clang) = tool("clang++") else { return };
    for arch in [Arch::Aarch64, Arch::X86_64] {
        run_compiler(&format!("clang-{}", arch.name()), false, &clang, &arch.clang_args());
    }
}

/// A stored `SECTIONS` row named `name`.
fn row(m: &mut MemoryState, name: &[u8], flags: u64, size: u64, comdat: bool) -> SectionRow {
    SectionRow {
        name: m.intern(name),
        sh_type: 1,
        sh_flags: flags,
        size,
        anchor_kind: u8::from(comdat),
        ..SectionRow::default()
    }
}

// spec: incr.md IN-55 IN-55a (WP-229 ruling 7: the old version from its
// stored rows, names only)
#[test]
fn classify_rows_by_name() {
    let mut m = MemoryState::default();
    let skeleton = [
        row(&mut m, b".text", 6, 16, false),
        row(&mut m, b".debug_info", 0, 40, false),
        row(&mut m, b".debug_addr", 0, 24, false),
        row(&mut m, b".debug_gnu_pubnames", 0, 30, false),
        row(&mut m, b".debug_gnu_pubtypes", 0, 0, false),
    ];
    let names =
        [row(&mut m, b".debug_info", 0, 40, false), row(&mut m, b".debug_names", 0, 9, false)];
    let types = [row(&mut m, b".debug_info", 0, 40, true)];
    let both =
        [row(&mut m, b".debug_types", 0, 40, false), row(&mut m, b".debug_pubnames", 0, 9, false)];
    let alloc = [row(&mut m, b".debug_gnu_pubnames", 2, 30, false)];
    let view = m.view();
    let changed = ChangedFiles::default();
    let got = classify_rows(&view, &changed, &skeleton);
    assert_eq!(got, Ok(Handled { gnu_index: true, ..Handled::default() }));
    assert!(matches!(classify_rows(&view, &changed, &names), Err(IncrReason::DebugNameIndex(_))));
    assert!(matches!(classify_rows(&view, &changed, &types), Err(IncrReason::DebugUnitKind(_))));
    // Name index before unit kind (ruling 7), whatever the row order.
    assert!(matches!(classify_rows(&view, &changed, &both), Err(IncrReason::DebugNameIndex(_))));
    assert_eq!(classify_rows(&view, &changed, &alloc), Ok(Handled::default()));
}
