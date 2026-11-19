//! The update tests' shared programs and helpers (WP-255d2d, ruling item 6):
//! the C++ program's link line on AArch64 and x86-64 with `lib2.a`'s
//! members, the format-5 state of a link, the whole front of a line, and
//! files matched by path and member name. Included by
//! `file_front_update.rs` and `changed_facts.rs` only (`#[path]`), so no
//! other test compiles it.

use std::ffi::OsString;
use std::path::PathBuf;

use mortar::front::{Env, Front, front};
use mortar::incremental::State5;
use mortar::timing::Timer;
use mortar_core::id::FileId;
use mortar_core::par::Pool;
use mortar_diag::{Collector, Policy};
use mortar_incr::update::view::StateView;
use mortar_testutil::{Scratch, run, tool, tools_required};

use crate::support::{self, Driver, build, build_by, with_output};

pub const M1: &str =
    "int wp255d_m1_data = 1;\nint wp255d_m1(int x) { return x + wp255d_m1_data; }\n";
pub const M2: &str = "static volatile int wp255d_m1_data = 3;\n\
                  int wp255d_m2(int x) { return x * wp255d_m1_data + 7; }\n";
/// `m1.o` edited (C++): `wp255d_m1` grown, a renamed function, an added
/// section, an added COMDAT group (`inline`, `noinline` so -O1 keeps it, used).
pub const M1_EDITED: &str = "extern \"C\" { int wp255d_m1_data = 1; }\n\
    inline __attribute__((noinline)) int wp255d_inl(int x) { return x * 3 + wp255d_m1_data; }\n\
    extern \"C\" int wp255d_m1_renamed(int x) { return x ^ 5; }\n\
    extern \"C\" __attribute__((section(\".text.wp255d2_added\"))) int wp255d_added(int x) \
    { return x + 11; }\n\
    extern \"C\" int wp255d_m1(int x) { int s = 0; for (int i = 0; i < x; i++) s += i * x; \
    return s + wp255d_inl(x) + wp255d_m1_renamed(x) + wp255d_added(x); }\n";
/// A member with versioned definitions and the three keys without a row.
pub const M4: &str = "__asm__(\".symver wp255d_v1, wp255d_ver@V1\");\n\
    __asm__(\".symver wp255d_v2, wp255d_ver@@V2\");\n\
    int wp255d_v1(void) { return 1; }\n\
    int wp255d_v2(void) { return 2; }\n\
    extern char _end[];\n\
    extern int _DYNAMIC[] __attribute__((weak));\n\
    extern int wp255d_nowhere __attribute__((weak));\n\
    long wp255d_m4(void) { return (long)_end + (_DYNAMIC != 0) + (&wp255d_nowhere != 0); }\n";
/// A member whose string section's last string is not terminated (S-8: the
/// full link refuses it; the split meets it as a problem).
pub const M6: &str = "__asm__(\".section .rodata.wp255d_str,\\\"aMS\\\",%progbits,1\\n\"\n\
    \"\\t.ascii \\\"ab\\\"\\n\\t.text\\n\");\n\
    int wp255d_m6(int x) { return x + 6; }\n";
/// An `SHF_LINK_ORDER` section outside the groups, linked to a COMDAT member.
pub const M3: &str = "__asm__(\".section .text.wp255d_g,\\\"axG\\\",%progbits,wp255d_g,comdat\\n\"\n\
    \"\\t.globl wp255d_g\\nwp255d_g:\\n\\tret\\n\"\n\
    \"\\t.section .wp255d_lo,\\\"ao\\\",%progbits,.text.wp255d_g\\n\\t.byte 1\\n\\t.text\\n\");\n\
    int wp255d_m3(int x) { return x + 2; }\n";

pub struct Line {
    pub _scratch: Scratch,
    pub label: String,
    pub dir: PathBuf,
    /// The C++ program's link line, without `lib2.a` and its `-u`s.
    pub base: Vec<String>,
    pub cc: PathBuf,
}

impl Line {
    pub fn compile(&self, name: &str, lang: &str, text: &str) {
        let src = self.dir.join(format!("{name}.src"));
        std::fs::write(&src, text).expect("source");
        let obj = self.dir.join(format!("{name}.o"));
        let args: Vec<OsString> = vec![
            "-x".into(),
            lang.into(),
            "-O1".into(),
            "-c".into(),
            src.into_os_string(),
            "-o".into(),
            obj.into_os_string(),
        ];
        let o = run(&self.cc, &args);
        assert!(o.success(), "{}: {name}: {}", self.label, o.stderr_text());
    }

    pub fn archive(&self, members: &[&str]) {
        let ar = tool("ar").expect("ar");
        let lib = self.dir.join("lib2.a");
        let _ = std::fs::remove_file(&lib);
        let mut args: Vec<OsString> = vec!["rcs".into(), lib.into_os_string()];
        args.extend(members.iter().map(|m| self.dir.join(m).into_os_string()));
        let o = run(&ar, &args);
        assert!(o.success(), "ar: {}", o.stderr_text());
    }

    /// The line with `-u` for each of `roots`, then `lib2.a`.
    pub fn args(&self, roots: &[&str]) -> Vec<String> {
        let mut a = self.base.clone();
        for r in roots {
            a.push("-u".into());
            a.push((*r).into());
        }
        a.push(self.dir.join("lib2.a").to_string_lossy().into_owned());
        a
    }
}

/// The lines of both targets, `m1`, `m2`, `m1e`, `m3`, `m4`, `m6` compiled,
/// then `extra` (name, language, source).
pub fn lines(extra: &[(&str, &str, &str)]) -> Vec<Line> {
    let mut v = Vec::new();
    for (label, cc, a64) in [
        ("aarch64 g++", Driver::Gxx.tool(), true),
        ("x86-64 g++", "x86_64-linux-gnu-g++".into(), false),
    ] {
        let s = Scratch::new(if a64 { "wp255d2-a64" } else { "wp255d2-x64" });
        let built = if a64 {
            build(&s, Driver::Gxx, &["-O1"], support::CXX_PROGRAM)
        } else {
            build_by(&s, &cc, &["-O1"], support::CXX_PROGRAM)
        };
        let Some(b) = built else {
            assert!(!tools_required(), "{cc} is required");
            continue;
        };
        let cc = tool(&cc).expect("the compiler");
        let line = Line { _scratch: s, label: label.into(), dir: b.dir, base: b.args, cc };
        line.compile("m1", "c", M1);
        line.compile("m2", "c", M2);
        line.compile("m1e", "c++", M1_EDITED);
        line.compile("m3", "c", M3);
        line.compile("m4", "c", M4);
        line.compile("m6", "c", M6);
        for (name, lang, text) in extra {
            line.compile(name, lang, text);
        }
        v.push(line);
    }
    v
}

pub fn pool() -> Pool {
    Pool::try_new(Pool::available()).expect("a pool")
}

/// The link of `args` with the format-5 switch; its state as the update
/// reads it.
pub fn state(line: &Line, args: &[String], out: &str) -> StateView<'static> {
    let mut a = with_output(args, &line.dir.join(out));
    a.extend(["--incremental=full", "--incremental-state"].iter().map(OsString::from));
    a.push(format!("{out}.mortar-state").into());
    let path = line.dir.join(format!("{out}.state5"));
    let inv = mortar::invoke_state5(
        "mortar",
        &a,
        &line.dir,
        State5 { path: path.clone(), collide: false, rows: None },
    );
    let ok = matches!(inv.outcome, mortar::Outcome::Linked { .. });
    assert!(ok, "{}: the link failed: {:?}", line.label, inv.outcome);
    let bytes = std::fs::read(&path).expect("the state");
    mortar_incr::state5::read::open_bytes(bytes).expect("the state opens")
}

/// The whole front of `args` (incremental mode, the switch).
pub fn whole<'s>(line: &Line, args: &[String], store: &'s mortar_input::store::Store) -> Front<'s> {
    let parsed = mortar_cli::parse(&with_output(args, &line.dir.join("front")));
    assert!(!parsed.failed(), "{:?}", parsed.diagnostics);
    let diag = Collector::new(Policy::default());
    let env = Env { cwd: &line.dir, pool: pool(), replay: None, slack: true, state5: true };
    let f = front(parsed.plan, env, store, &diag, &mut Timer::new(false));
    let report = diag.finish();
    assert!(!report.failed(), "{}: {report:?}", line.label);
    f.expect("the front")
}

/// The member of `lib2.a` whose name ends in `name`, in `view`.
pub fn member(view: &StateView<'_>, name: &str) -> FileId {
    let files = u32::try_from(view.count(mortar_incr::format::kind::FILES)).expect("FILES");
    (1..files)
        .map(FileId)
        .find(|&id| {
            let r = view.file(id).expect("FILES");
            r.member != 0
                && view.string(r.path).expect("path").ends_with(b"lib2.a")
                && view.string(r.member_name).expect("name").ends_with(name.as_bytes())
        })
        .unwrap_or_else(|| panic!("no member {name}"))
}

/// The front's file of the same path and member name as `file` of `view`.
pub fn same_file(f: &Front<'_>, view: &StateView<'_>, file: FileId) -> FileId {
    let r = view.file(file).expect("FILES");
    let (path, name) = (view.string(r.path).expect("path"), view.string(r.member_name).expect("m"));
    (1..f.inputs.files.len())
        .map(FileId)
        .find(|&id| {
            let x = &f.inputs.files[id];
            x.path == &path[..] && x.member.map_or(&b""[..], |m| m.name) == &name[..]
        })
        .expect("the file in the new front")
}

pub fn first_free(view: &StateView<'_>) -> u32 {
    u32::try_from(view.count(mortar_incr::format::kind::SECTIONS)).expect("SECTIONS")
}
