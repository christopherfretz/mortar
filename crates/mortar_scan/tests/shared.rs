//! The scan of a shared object (WP-117, AArch64): `spec/dyn.md` D-113's
//! target kinds, D-117's table and its address-level rule with the
//! boundary rule (the before-the-start refusal), D-118, D-122, and TLS
//! and IFUNC refused by name until WP-124. Objects are compiled by the
//! test (GCC, Clang 19, GNU as) and taken through the phases before the
//! scan on a `-shared` line (`support::link`: both stores). The command
//! line refuses `-shared` itself until WP-123 joins the output (a
//! refusal of the parse, which the phases here do not read).

mod support;

use std::ffi::OsString;
use std::path::Path;

use mortar_core::par::Pool;
use mortar_testutil::Scratch;
use support::{Linked, assemble, compile, link, need, run_in};

/// `-shared` with `extra` and the objects, through the scan.
fn shared(dir: &Path, objects: &[&str], extra: &[&str]) -> Linked {
    let mut args: Vec<OsString> = vec!["-shared".into()];
    args.extend(extra.iter().map(OsString::from));
    args.extend(objects.iter().map(OsString::from));
    let l = link(dir, &args, &Pool::new(4)).expect("load");
    let before: Vec<&str> = l.before.iter().map(|(_, d)| d.id()).collect();
    assert!(before.is_empty(), "{before:?}");
    l
}

/// The rendered `non-pic-in-shared-object` diagnostics of a scan.
fn non_pic(l: &Linked) -> Vec<String> {
    l.scanned
        .diagnostics
        .iter()
        .filter(|(_, d)| d.id() == "non-pic-in-shared-object")
        .map(|(k, d)| support::render(&[(*k, d.clone())], mortar_diag::Policy::default()))
        .collect()
}

/// Clang 19 for AArch64, compiling `src` into `dir/name`.
fn clang(dir: &Path, name: &str, src: &str, flags: &[&str]) -> Option<()> {
    let cc = need("clang-19")?;
    let s = dir.join(format!("{name}.c"));
    std::fs::write(&s, src).unwrap();
    let mut a: Vec<OsString> = vec!["--target=aarch64-linux-gnu".into(), "-c".into()];
    a.extend(flags.iter().map(OsString::from));
    a.extend(["-o".into(), dir.join(name).into(), s.into()]);
    let r = run_in(&cc, &a, dir);
    assert!(r.success(), "clang {name}: {}", r.stderr_text());
    Some(())
}

/// `tests/shared-design/sup/c.c` (WP-115): `static double tab[8]` between
/// the exported `gvar` and `gcount`, and `tab - 1`, the 1-based idiom.
const BEFORE_START: &str = "int gvar[4] = {1,2,3,4};
static double tab[8] = {1,2,3,4,5,6,7,8};
int gcount = 7;
static int w[5] = {1,2,3,4,5};
double f(int n){ double r=0; double *p = tab - 1; for(int i=1;i<=n;i++){ p[i] = p[i]*2+gvar[0]; r+=p[i]; } return r; }
int g(int i){ return w[i-1] + w[i-2] + gcount; }
int h(int i,int j){ return w[i-j-3]; }
";

// spec: dyn.md D-117 ("Before the start of a local object", test
// `shared-pic-before-start`): GCC `-O0 -fPIC` forms `.data + 8`, inside
// `gvar`, refused with both notes; the `-O2` object links. The Clang
// `-O0` case is found by the analysis only: WP-119's (marked below).
#[test]
fn shared_pic_before_start() {
    let dir = Scratch::new("scan-shared-before-start");
    let Some(_) = compile(dir.path(), "gcc", "c0.o", "c", BEFORE_START, &["-O0", "-fPIC"]) else {
        return;
    };
    let l = shared(dir.path(), &["c0.o"], &[]);
    let refused = non_pic(&l);
    assert_eq!(refused.len(), 2, "{}", l.rendered());
    for text in &refused {
        assert!(text.contains("against symbol `gvar' which may bind externally"), "{text}");
        assert!(text.contains("recompile with -fPIC"), "{text}");
        assert!(text.contains(".data+0x8"), "{text}");
        assert!(text.contains("-fdata-sections"), "{text}");
        assert!(text.contains("do not form an address before an object"), "{text}");
    }
    assert!(refused.iter().any(|t| t.contains("R_AARCH64_ADR_PREL_PG_HI21")));
    assert!(refused.iter().any(|t| t.contains("R_AARCH64_ADD_ABS_LO12_NC")));

    compile(dir.path(), "gcc", "c2.o", "c", BEFORE_START, &["-O2", "-fPIC", "-g"]);
    let l = shared(dir.path(), &["c2.o"], &[]);
    assert!(l.scanned.diagnostics.is_empty(), "{}", l.rendered());

    // Clang's `-O0` object reaches `gcount` through `.data + 0x18` and a
    // `sub #8` in a register: no relocation names the address, so the
    // rule alone would link it; WP-119's analysis, joined by WP-123,
    // refuses it (D-117's table, the Clang row) with both notes.
    if clang(dir.path(), "k0.o", BEFORE_START, &["-O0", "-fPIC"]).is_some() {
        let l = shared(dir.path(), &["k0.o"], &[]);
        let refused = non_pic(&l);
        assert!(!refused.is_empty(), "the analysis: {}", l.rendered());
        for text in &refused {
            assert!(text.contains("against symbol `gcount' which may bind externally"), "{text}");
            assert!(text.contains("-fdata-sections"), "{text}");
            assert!(text.contains("do not form an address before an object"), "{text}");
        }
    }
}

// spec: dyn.md D-117 (the address-level rule; test `shared-nonpic-anchor`):
// GCC AArch64 `-fPIE` reads its own exported `g` through the section
// anchor `.data + 0`; refused, naming `g`, also under
// `-Bsymbolic-functions` (it binds functions only: `g` stays
// preemptible, D-112). The `-fPIC` object links.
#[test]
fn shared_nonpic_anchor() {
    let src = "int g = 7;\nint b[4] = {1, 2, 3, 4};\nint get(void) { return g + b[2]; }\nvoid set(int v) { g = v; }\n";
    let dir = Scratch::new("scan-shared-anchor");
    let Some(_) = compile(dir.path(), "gcc", "pie.o", "c", src, &["-O2", "-fPIE"]) else { return };
    compile(dir.path(), "gcc", "pic.o", "c", src, &["-O2", "-fPIC"]);
    for extra in [&[][..], &["-Bsymbolic-functions"][..]] {
        let l = shared(dir.path(), &["pie.o"], extra);
        let refused = non_pic(&l);
        assert!(!refused.is_empty(), "{extra:?}");
        assert!(refused.iter().any(|t| t.contains("against symbol `g'")), "{refused:?}");
        assert!(refused.iter().all(|t| !t.contains("do not form an address")), "no self-GOT");
    }
    let l = shared(dir.path(), &["pic.o"], &[]);
    assert!(l.scanned.diagnostics.is_empty(), "{}", l.rendered());
}

/// A `-fPIC`-shaped object (it loads `after` from the GOT: self-GOT) with
/// a load at the boundary `arr + 16` (the start of the exported `after`,
/// one past the end of the static `arr`), and an address formed there.
const BOUNDARY_SELF_GOT: &str = "
        .data
        .p2align 2
        .type arr, %object
        .size arr, 16
arr:    .zero 16
        .globl after
        .type after, %object
        .size after, 4
after:  .word 5
        .text
        .globl f
        .type f, %function
f:      adrp x0, :got:after
        ldr x0, [x0, :got_lo12:after]
        adrp x1, arr+16
        ldr w1, [x1, :lo12:arr+16]
        adrp x2, arr+16
        add x2, x2, :lo12:arr+16
        ret
        .size f, .-f
";

/// The same object without the GOT load (no self-GOT).
const BOUNDARY_NO_GOT: &str = "
        .data
        .p2align 2
        .type arr, %object
        .size arr, 16
arr:    .zero 16
        .globl after
        .type after, %object
        .size after, 4
after:  .word 5
        .text
        .globl f
        .type f, %function
f:      adrp x2, arr+16
        add x2, x2, :lo12:arr+16
        ret
        .size f, .-f
";

/// Clang's `c2.c` (record WP-110 §G.3): a loop to `arr + 10`.
const C2: &str = "static int arr[10];\nint after;\nint sum(void){int s=0; for(int *p=arr; p<arr+10; p++) s+=*p; return s + after;}\n";

// spec: dyn.md D-117 ("The boundary"; test `shared-nonpic-boundary`): a
// load at the boundary in a self-GOT object is a use, an address formed
// there is not; without a self-GOT the address is a use. Both refusals
// carry the `-fdata-sections` note. Clang `c2.c` `-O2 -fPIC` links.
#[test]
fn shared_nonpic_boundary() {
    let dir = Scratch::new("scan-shared-boundary");
    let Some(_) = assemble(dir.path(), "got.o", BOUNDARY_SELF_GOT) else { return };
    let l = shared(dir.path(), &["got.o"], &[]);
    let refused = non_pic(&l);
    assert_eq!(refused.len(), 1, "{}", l.rendered());
    assert!(refused[0].contains("R_AARCH64_LDST32_ABS_LO12_NC against symbol `after'"));
    assert!(refused[0].contains("-fdata-sections"), "{}", refused[0]);
    assert!(!refused[0].contains("do not form an address before"), "{}", refused[0]);

    assemble(dir.path(), "nogot.o", BOUNDARY_NO_GOT);
    let l = shared(dir.path(), &["nogot.o"], &[]);
    let refused = non_pic(&l);
    assert_eq!(refused.len(), 2, "{}", l.rendered());
    assert!(refused.iter().all(|t| t.contains("`after'") && t.contains("-fdata-sections")));

    if clang(dir.path(), "c2.o", C2, &["-O2", "-fPIC"]).is_some() {
        let l = shared(dir.path(), &["c2.o"], &[]);
        assert!(l.scanned.diagnostics.is_empty(), "{}", l.rendered());
    }
}

// spec: dyn.md D-117 ("Not uses"; test `shared-nonpic-not-uses`): debug
// sections, `.eh_frame` and the uses of an exported function through its
// `.localalias` (`-fno-semantic-interposition`) are not uses
#[test]
fn shared_nonpic_not_uses() {
    let src = "int counter = 1;\nint step(int x) { return x + counter; }\nint twice(int x) { return step(step(x)); }\nint (*fp)(int) = step;\n";
    let dir = Scratch::new("scan-shared-not-uses");
    let flags =
        ["-O2", "-fPIC", "-g", "-fno-semantic-interposition", "-fasynchronous-unwind-tables"];
    let Some(_) = compile(dir.path(), "gcc", "n.o", "c", src, &flags) else { return };
    let l = shared(dir.path(), &["n.o"], &[]);
    assert!(l.scanned.diagnostics.is_empty(), "{}", l.rendered());
}

/// Hand-written forms against a preemptible datum `pd`, an import `ext`,
/// a weak undefined `wk` and a hidden datum `hd` (D-117's table).
const TABLE: &str = "
        .data
        .globl pd
        .type pd, %object
        .size pd, 8
pd:     .xword 1
        .globl hd
        .hidden hd
        .type hd, %object
        .size hd, 8
hd:     .xword 2
        .weak wk
words:  .xword pd
        .xword ext
        .xword wk
        .xword hd
        .text
        .globl f
        .type f, %function
f:      adrp x0, :got:pd
        ldr x0, [x0, :got_lo12:pd]
        adrp x1, :got:ext
        ldr x1, [x1, :got_lo12:ext]
        adrp x2, :got:wk
        ldr x2, [x2, :got_lo12:wk]
        adrp x3, hd
        add x3, x3, :lo12:hd
        bl ext
        bl f
        ret
        .size f, .-f
";

// spec: dyn.md D-117 (the table: GOT, Branch, AbsWord in a writable
// place against every kind link; Pc against preemptible, import and weak
// undefined is `non-pic-in-shared-object`, against a hidden one resolved;
// AbsWord in a read-only place `text-relocation`; AbsNarrow refused for
// every kind), D-118 (an undefined `ext` is accepted; `-z defs` refuses
// it), D-122 (no copy, no canonical PLT)
#[test]
fn shared_reloc_table() {
    use mortar_core::demands::DemandFlags as F;
    let dir = Scratch::new("scan-shared-table");
    let Some(_) = assemble(dir.path(), "t.o", TABLE) else { return };
    let l = shared(dir.path(), &["t.o"], &[]);
    assert!(l.scanned.diagnostics.is_empty(), "{}", l.rendered());
    let d = &l.scanned.demands;
    assert_eq!(d.keys_with(F::COPY).count() + d.keys_with(F::CANON_PLT).count(), 0, "D-122");
    let l = shared(dir.path(), &["t.o"], &["-z", "defs"]);
    assert_eq!(l.ids(), ["undefined-symbol"], "{}", l.rendered());

    let pc = |target: &str| {
        format!(
            "        .text\n        .globl f\n        .type f, %function\nf:      adrp x0, {target}\n        ret\n        .size f, .-f\n"
        )
    };
    let decls = "        .data\n        .globl pd\n        .type pd, %object\n        .size pd, 8\npd:     .xword 1\n        .globl hd\n        .hidden hd\n        .type hd, %object\n        .size hd, 8\nhd:     .xword 2\n        .weak wk\n";
    for (target, refused) in [("pd", true), ("ext", true), ("wk", true), ("hd", false)] {
        let name = format!("pc-{target}.o");
        assemble(dir.path(), &name, &format!("{decls}{}", pc(target)));
        let l = shared(dir.path(), &[&name], &[]);
        let ids = l.ids();
        if refused {
            assert_eq!(ids, ["non-pic-in-shared-object"], "{target}: {}", l.rendered());
            assert!(l.rendered().contains(&format!(
                "relocation R_AARCH64_ADR_PREL_PG_HI21 against symbol `{target}' which may bind \
                 externally can not be used when making a shared object; recompile with -fPIC"
            )));
        } else {
            assert!(ids.is_empty(), "{target}: {}", l.rendered());
        }
    }
    for (target, kind) in [("pd", "text-relocation"), ("hd", "text-relocation")] {
        let name = format!("ro-{target}.o");
        let src = format!("{decls}        .section .rodata\n        .xword {target}\n");
        assemble(dir.path(), &name, &src);
        let l = shared(dir.path(), &[&name], &[]);
        assert_eq!(l.ids(), [kind], "{target}: {}", l.rendered());
    }
    for target in ["pd", "hd", "ext"] {
        let name = format!("narrow-{target}.o");
        let src = format!("{decls}        .section .data.n, \"aw\"\n        .word {target}\n");
        assemble(dir.path(), &name, &src);
        let l = shared(dir.path(), &[&name], &[]);
        assert_eq!(l.ids(), ["non-pic-in-shared-object"], "{target}: {}", l.rendered());
    }
}

// spec: dyn.md D-119, D-121 (WP-124): TLS in every model but local-exec
// and the library's own IFUNC scan in a shared object without a
// diagnostic; a local-exec reference is refused by name (`class_outcome`).
#[test]
fn shared_tls_and_ifunc_scan() {
    let dir = Scratch::new("scan-shared-feature");
    let tls = "__thread int t = 1;\nint get(void) { return t; }\n";
    let ifunc = "static int one(void) { return 1; }\nstatic int (*pick(void))(void) { return one; }\nint sel(void) __attribute__((ifunc(\"pick\")));\nint call(void) { return sel(); }\n";
    let Some(_) = compile(dir.path(), "gcc", "tls.o", "c", tls, &["-O2", "-fPIC"]) else { return };
    compile(dir.path(), "gcc", "ie.o", "c", tls, &["-O2", "-fPIC", "-ftls-model=initial-exec"]);
    compile(dir.path(), "gcc", "le.o", "c", tls, &["-O2", "-fPIC", "-ftls-model=local-exec"]);
    compile(dir.path(), "gcc", "ifunc.o", "c", ifunc, &["-O2", "-fPIC"]);
    for o in ["tls.o", "ie.o", "ifunc.o"] {
        let l = shared(dir.path(), &[o], &[]);
        assert!(l.scanned.diagnostics.is_empty(), "{o}: {}", l.rendered());
    }
    let l = shared(dir.path(), &["le.o"], &[]);
    let text = l.rendered();
    assert!(text.contains("local-exec TLS in a shared object"), "{text}");
}
