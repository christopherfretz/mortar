//! WP-278: a `-g` update of an object larger than the staged store's
//! resident threshold (`spec/input.md` §11.1 (b): a section above 64 KiB
//! of an object above 64 KiB stays on disk, `mortar_input::staged::SMALL`)
//! reads its `.debug_info` unit headers from the file (`spec/incr.md`
//! IN-55a, IN-70's debug condition), static C++ by host-aware GNU drivers,
//! AArch64 and x86-64.
//!
//! - `classify_input` on the front's object (the staged store's parts view)
//!   answers what `classify_object` answers on the file's whole bytes, for
//!   a plain object and for an archive member; `classify_object` over the
//!   parts view is the base's refusal (`section N: unreadable`).
//! - The update of a same-size body edit of such an object, under the
//!   DEFAULT store, is taken, and its output is byte-identical to the full
//!   link's (`--incremental=full`); also with `-ffunction-sections
//!   -fdata-sections --gc-sections` (the operator's configuration). At
//!   1bfce906 both were refused `debug-unit-kind: section 4: unreadable`.

mod support;

use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;

use mortar_core::par::Pool;
use mortar_elf::consts::SHF_ALLOC;
use mortar_incr::update::split::{Handled, classify_input, classify_object};
use mortar_input::store::{Store, StorePolicy};
use mortar_testutil::target::Arch;
use mortar_testutil::{Scratch, run_with, tools_required};
use support::{Source, build_by, front_of, with_output};

const MORTAR: &str = env!("CARGO_BIN_EXE_mortar");

const MAIN: &str = "#include <cstdio>\nint c_value(int);\n\
int main(int argc, char **){ std::printf(\"c=%d\\n\", c_value(argc + 1)); return 0; }\n";

/// The changed file: `c_value`'s immediate is a macro (an edit keeps every
/// line, string and size of the debug sections), after 900 types and
/// functions that give the object a `.debug_info` of about 120 KB.
fn c_cc(k: u32) -> String {
    let mut s = format!("#define C_K {k}\n");
    s.push_str(
        "#define C_DEF(n) struct S##n { int a; long b; double c; unsigned e; }; \\\n\
         int f##n(struct S##n *s, int x) { int loc = s->a + x; return (int)(s->b * loc + s->e); }\n\
         #define C_D10(n) C_DEF(n##0) C_DEF(n##1) C_DEF(n##2) C_DEF(n##3) C_DEF(n##4) \\\n\
         C_DEF(n##5) C_DEF(n##6) C_DEF(n##7) C_DEF(n##8) C_DEF(n##9)\n\
         #define C_D100(n) C_D10(n##0) C_D10(n##1) C_D10(n##2) C_D10(n##3) C_D10(n##4) \\\n\
         C_D10(n##5) C_D10(n##6) C_D10(n##7) C_D10(n##8) C_D10(n##9)\n\
         C_D100(1) C_D100(2) C_D100(3) C_D100(4) C_D100(5) C_D100(6) C_D100(7) C_D100(8) C_D100(9)\n",
    );
    s.push_str("int c_value(int x) {\n  int scaled = x * C_K;\n  return scaled + 1;\n}\n");
    s
}

fn targets() -> [Arch; 2] {
    [Arch::Aarch64, Arch::X86_64]
}

struct Prog {
    cxx: String,
    flags: Vec<&'static str>,
    _scratch: Scratch,
    dir: PathBuf,
    args: Vec<String>,
}

impl Prog {
    fn new(arch: Arch, case: &str, extra: &[&'static str], gc: bool) -> Option<Self> {
        let cxx = arch.gnu("g++");
        let scratch = Scratch::new(&format!("wp278-{case}-{}", arch.name()));
        let c: &'static str = Box::leak(c_cc(7).into_boxed_str());
        let mut flags = vec!["-O0", "-g"];
        flags.extend_from_slice(extra);
        let sources = [Source { name: "main.cc", text: MAIN }, Source { name: "c.cc", text: c }];
        let Some(b) = build_by(&scratch, &cxx, &flags, &sources) else {
            assert!(!tools_required(), "{cxx} is required");
            return None;
        };
        let mut args = b.args;
        if gc {
            args.push("--gc-sections".into());
        }
        Some(Self { cxx, flags, _scratch: scratch, dir: b.dir, args })
    }

    /// `c.cc.o` rebuilt with `C_K` = `k`, as a new inode, from the same path
    /// (one `DW_AT_comp_dir`, as a rebuild has).
    fn rebuild_c(&self, k: u32) {
        let src = self.dir.join("c.cc");
        std::fs::write(&src, c_cc(k)).expect("c.cc");
        let tmp = self.dir.join("c.cc.o.new");
        let mut c = Command::new(&self.cxx);
        c.args(&self.flags).arg("-c").arg(&src).arg("-o").arg(&tmp);
        let o = run_with(&mut c, b"", Duration::from_mins(2));
        assert!(o.success(), "{}: {}", self.cxx, o.stderr_text());
        std::fs::rename(&tmp, self.dir.join("c.cc.o")).expect("rename c.cc.o");
    }

    /// The link with `extra`, under the DEFAULT store (`MORTAR_STORE`
    /// removed: the tests' switch must not make the objects resident).
    fn link(&self, out: &str, extra: &[&str]) -> (bool, String) {
        let mut c = Command::new(MORTAR);
        c.env_remove("MORTAR_STORE")
            .args(with_output(&self.args, &self.dir.join(out)))
            .args(extra)
            .current_dir(&self.dir)
            .env("MORTAR_UPDATE_FINDINGS", self.dir.join("findings.txt"));
        let o = run_with(&mut c, b"", Duration::from_mins(5));
        assert!(!o.timed_out(), "mortar: TIMEOUT");
        (o.success(), o.stderr_text())
    }

    fn bytes(&self, name: &str) -> Vec<u8> {
        std::fs::read(self.dir.join(name)).unwrap_or_else(|e| panic!("{name}: {e}"))
    }
}

/// `.debug_info`'s size in the whole bytes of an object.
fn debug_info_size(bytes: &[u8]) -> u64 {
    let o = mortar_elf::Object::parse(bytes).expect("an object");
    o.sections()
        .find(|s| s.flags() & SHF_ALLOC == 0 && s.name().ok() == Some(b".debug_info".as_slice()))
        .map_or(0, |s| s.size())
}

// spec: input.md §11.1 (b) (the staged store's residency); incr.md IN-55a
// (unit headers, read from the file's bytes wherever the store keeps them;
// WP-278). `classify_input` over the front's parts view answers what
// `classify_object` answers over the whole bytes; `classify_object` over the
// parts view is the base's `unreadable`. A plain object and an archive
// member (offsets in the member).
#[test]
fn classify_input_reads_debug_info_on_disk() {
    let mut ran = 0;
    for arch in targets() {
        let Some(prog) = Prog::new(arch, "classify", &[], false) else { continue };
        ran += 1;
        let label = arch.name();
        let whole = prog.bytes("c.cc.o");
        assert!(whole.len() as u64 > 64 << 10, "{label}: the object must exceed 64 KiB");
        assert!(debug_info_size(&whole) > 64 << 10, "{label}: .debug_info must exceed 64 KiB");
        let want = classify_object(&mortar_elf::Object::parse(&whole).expect("c.cc.o"));
        assert_eq!(
            want,
            Ok(Handled { compile_units: 1, ..Handled::default() }),
            "{label}: one DWARF 5 compile unit"
        );
        // The archive: `c.cc.o` as a member of `libc278.a`, after main.
        let ar = arch.gnu("ar");
        let mut cmd = Command::new(&ar);
        cmd.arg("rcs").arg(prog.dir.join("libc278.a")).arg(prog.dir.join("c.cc.o"));
        let out = run_with(&mut cmd, b"", Duration::from_mins(1));
        assert!(out.success(), "{ar}: {}", out.stderr_text());
        let plain = prog.args.clone();
        let member: Vec<String> = prog
            .args
            .iter()
            .map(|a| if a.ends_with("c.cc.o") { "libc278.a".into() } else { a.clone() })
            .collect();
        for (case, args) in [("object", plain), ("member", member)] {
            let store = Store::new(StorePolicy::Staged);
            let args = with_output(&args, &prog.dir.join("o"));
            let front = front_of(&args, &prog.dir, &store, Pool::new(2))
                .unwrap_or_else(|r| panic!("{label} {case}: the front: {r:?}"));
            let file = front
                .inputs
                .files
                .iter()
                .find(|(_, x)| {
                    let name = x.member.as_ref().map_or(x.path, |m| m.name);
                    name.ends_with(b"c.cc.o")
                })
                .map_or_else(|| panic!("{label} {case}: no c.cc.o in the front"), |(id, _)| id);
            let Some(mortar_core::inputs::Reader::Object(obj)) =
                front.inputs.files.get(file).map(|x| &x.reader)
            else {
                panic!("{label} {case}: c.cc.o is not an object")
            };
            let info = obj
                .sections()
                .find(|s| s.name().ok() == Some(b".debug_info".as_slice()))
                .expect(".debug_info");
            // The case forces non-residency: the precondition of the test.
            assert!(!info.is_resident(), "{label} {case}: .debug_info is resident");
            let base = classify_object(obj);
            assert!(
                matches!(&base, Err(mortar_core::incr::IncrReason::DebugUnitKind(d)) if d.contains("unreadable")),
                "{label} {case}: classify_object over the parts view: {base:?}"
            );
            assert_eq!(classify_input(&front.inputs, file), want, "{label} {case}");
        }
    }
    assert!(ran == 2 || !tools_required(), "{ran} of the 2 targets");
}

/// The update of `c.cc.o` to `C_K` 9, then back to 7, each taken under the
/// default store and byte-identical to the full link.
fn updates_are_taken(p: &Prog, label: &str) {
    assert!(debug_info_size(&p.bytes("c.cc.o")) > 64 << 10, "{label}: .debug_info too small");
    let (ok, err) = p.link("o", &["--incremental"]);
    assert!(ok, "{label}: the first link: {err}");
    for k in [9, 7] {
        p.rebuild_c(k);
        let (ok, err) = p.link("o", &["--incremental=update", "--incremental-report"]);
        let findings = std::fs::read_to_string(p.dir.join("findings.txt")).unwrap_or_default();
        assert!(ok, "{label} k={k}: not an update: {err}\nfindings:\n{findings}");
        assert!(err.contains("incremental: update ("), "{label} k={k}: {err}");
        let (ok, err) = p.link("full", &["--incremental=full"]);
        assert!(ok, "{label} k={k}: the full link: {err}");
        let (got, want) = (p.bytes("o"), p.bytes("full"));
        if let Some(i) = got.iter().zip(&want).position(|(a, b)| a != b) {
            panic!("{label} k={k}: the output differs from the full link at {i:#x}");
        }
        assert_eq!(got.len(), want.len(), "{label} k={k}: sizes");
    }
}

// spec: incr.md IN-70 (the debug condition), IN-55a, IN-50 step 1 (WP-278):
// a same-size body edit of a -g object whose .debug_info the staged store
// keeps on disk is an update under the default store, byte-equal to the
// full link. At 1bfce906: `debug-unit-kind: section 4: unreadable`.
#[test]
fn large_debug_object_update_is_taken_under_the_staged_store() {
    let mut ran = 0;
    for arch in targets() {
        let Some(p) = Prog::new(arch, "upd", &[], false) else { continue };
        ran += 1;
        updates_are_taken(&p, &format!("plain {}", arch.name()));
    }
    assert!(ran == 2 || !tools_required(), "{ran} of the 2 targets");
}

// spec: as above, on the operator's configuration (static, -g,
// -ffunction-sections -fdata-sections, --gc-sections).
#[test]
fn gc_large_debug_object_update_is_taken_under_the_staged_store() {
    let mut ran = 0;
    for arch in targets() {
        let fs = ["-ffunction-sections", "-fdata-sections"];
        let Some(p) = Prog::new(arch, "gcupd", &fs, true) else { continue };
        ran += 1;
        updates_are_taken(&p, &format!("gc {}", arch.name()));
    }
    assert!(ran == 2 || !tools_required(), "{ran} of the 2 targets");
}
