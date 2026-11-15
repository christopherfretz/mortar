//! `--gc-sections` through the whole pipeline (WP-031, `spec/image.md`
//! I-6 to I-10, I-109): each root and edge class of the mark on assembled
//! objects, `--print-gc-sections`' text, and its consequences downstream
//! (`.symtab` S-76, `.eh_frame`/`.eh_frame_hdr` I-3, merge pieces S-16,
//! debug tombstones I-47, archive members, the map's `gc` records), then
//! a compact differential of real programs against GNU ld (the full one,
//! with lld and mold, is the record's: `spec/landed/image/WP-031.md`).
//! Every output passes the verifier with mortar's map.

mod support;

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use mortar_testutil::{Outcome, Scratch, run_with, tool, tools_required};
use support::{AARCH64, Built, Driver, build, with_output};

const MORTAR: &str = env!("CARGO_BIN_EXE_mortar");

/// Whether the AArch64 tools are there (on any host: the programs run
/// natively on AArch64 and under `qemu-aarch64` elsewhere).
fn host() -> bool {
    if AARCH64.tool("gcc").is_none() || AARCH64.tool("ld.bfd").is_none() {
        assert!(!tools_required(), "gcc and ld.bfd are required");
        return false;
    }
    true
}

fn run_in(exe: &Path, args: &[OsString], dir: &Path, env: &[(&str, &str)]) -> Outcome {
    let mut c = Command::new(exe);
    c.args(args).current_dir(dir);
    for (k, v) in env {
        c.env(k, v);
    }
    run_with(&mut c, b"", Duration::from_mins(5))
}

fn map_of(out: &Path) -> PathBuf {
    out.with_extension("map")
}

/// mortar's link of `args` (its `-o` replaced by `out`), with the map. An
/// output it writes is judged by the verifier, which must find nothing
/// (`docs/VALIDATION.md` L3, WP-035).
fn mortar(
    dir: &Path,
    args: &[String],
    out: &Path,
    extra: &[&str],
    env: &[(&str, &str)],
) -> Outcome {
    let o = mortar_unverified(dir, args, out, extra, env);
    if o.success() && out.exists() {
        support::plumb::clean(out, &map_of(out), dir);
    }
    o
}

/// [`mortar`] without the verifier: for a link whose output is compared
/// byte for byte with one [`mortar`] verified.
fn mortar_unverified(
    dir: &Path,
    args: &[String],
    out: &Path,
    extra: &[&str],
    env: &[(&str, &str)],
) -> Outcome {
    let mut a = with_output(args, out);
    a.push(format!("--mortar-map={}", map_of(out).display()).into());
    a.extend(extra.iter().map(OsString::from));
    run_in(Path::new(MORTAR), &a, dir, env)
}

fn bfd(dir: &Path, args: &[String], out: &Path, extra: &[&str]) -> Outcome {
    let mut a = with_output(args, out);
    a.extend(extra.iter().map(OsString::from));
    run_in(&AARCH64.tool("ld.bfd").expect("ld.bfd"), &a, dir, &[])
}

/// Runs a program as `prog`: (standard output, exit status).
fn execute(exe: &Path) -> (String, Option<i32>) {
    let o = run_with(&mut AARCH64.command(exe, Some("prog")), b"", Duration::from_mins(1));
    (o.stdout_text(), o.status.and_then(|s| s.code()))
}

/// The `(section, file)` pairs of `--print-gc-sections`' lines (GNU ld's
/// text, `<prog>: removing unused section '<s>' in file '<f>'`), the file
/// by its last path component; and the lines that are not such a line.
fn removed(stderr: &str) -> (Vec<(String, String)>, Vec<String>) {
    let mut set = Vec::new();
    let mut other = Vec::new();
    for line in stderr.lines() {
        let parsed = line.split_once(": removing unused section '").and_then(|(_, rest)| {
            let (sec, rest) = rest.split_once("' in file '")?;
            let file = rest.strip_suffix('\'')?;
            let base = file.rsplit('/').next().unwrap_or(file);
            Some((sec.to_owned(), base.to_owned()))
        });
        match parsed {
            Some(p) => set.push(p),
            None => other.push(line.to_owned()),
        }
    }
    (set, other)
}

/// Assembles each `(name, text)` with gcc in `dir`: the objects' names.
fn assemble(dir: &Path, files: &[(&str, &str)]) -> Vec<String> {
    let mut objs = Vec::new();
    for (name, text) in files {
        std::fs::write(dir.join(name), text).unwrap();
        let obj = format!("{name}.o");
        let o = run_with(
            Command::new(AARCH64.tool("gcc").expect("gcc"))
                .args(["-c", name, "-o", &obj])
                .current_dir(dir),
            b"",
            Duration::from_mins(1),
        );
        assert!(o.success(), "gcc -c {name}: {}", o.stderr_text());
        objs.push(obj);
    }
    objs
}

/// A static link line over `inputs` with entry `_start`.
fn line(inputs: &[String]) -> Vec<String> {
    let mut args: Vec<String> =
        ["-maarch64linux", "-static", "-e", "_start", "-o", "out"].map(str::to_owned).to_vec();
    args.extend(inputs.iter().cloned());
    args
}

/// The exit of `_start` (status `n`).
fn exit_with(n: u32) -> String {
    format!("    mov x0, #{n}\n    mov x8, #93\n    svc #0\n")
}

/// Links `inputs` with mortar and `--gc-sections --print-gc-sections`
/// (plus `extra`): the outcome, the removed set, the output. The link must
/// succeed, print nothing else, and pass the verifier.
fn gc_link(
    dir: &Path,
    inputs: &[String],
    extra: &[&str],
    tag: &str,
) -> (BTreeSet<(String, String)>, PathBuf) {
    let out = dir.join(format!("{tag}.out"));
    let mut flags = vec!["--gc-sections", "--print-gc-sections"];
    flags.extend_from_slice(extra);
    let m = mortar(dir, &line(inputs), &out, &flags, &[]);
    assert!(m.success(), "{tag}: {}", m.stderr_text());
    let (set, other) = removed(&m.stderr_text());
    assert!(other.is_empty(), "{tag}: {other:?}");
    (set.into_iter().collect(), out)
}

fn pairs(v: &[(&str, &str)]) -> BTreeSet<(String, String)> {
    v.iter().map(|(s, f)| ((*s).to_owned(), (*f).to_owned())).collect()
}

/// The names of `out`'s symbols (readelf), when readelf is there.
fn symbols(out: &Path) -> Option<String> {
    let readelf = tool("readelf")?;
    let o = run_with(Command::new(readelf).arg("-sW").arg(out), b"", Duration::from_mins(1));
    assert!(o.success());
    Some(o.stdout_text())
}

fn has_symbol(table: &str, name: &str) -> bool {
    table.lines().any(|l| l.split_whitespace().last() == Some(name))
}

// ---- roots and edges, one class each (assembled objects) -------------------

/// I-6, I-7: the entry is a root, a relocation is an edge whatever its
/// type (a call, an address, `R_AARCH64_NONE` from `.reloc`); a chain
/// reachable only from a collected section is collected; `-e` names the
/// root (`-u` and `--require-defined` are refused today, cli.md). `.symtab` loses the
/// symbols of collected sections (S-76); the program runs.
#[test]
fn i6_i7_roots_and_relocation_edges() {
    if !host() {
        return;
    }
    let s = Scratch::new("gc-edges");
    let a = format!(
        "    .text\n    .globl _start\n_start:\n    bl f1\n    adrp x1, d1\n    add x1, x1, :lo12:d1\n\
         \n    .reloc ., R_AARCH64_NONE, f5\n{}\
         \n    .section .text.f1,\"ax\",@progbits\n    .globl f1\nf1:\n    ret\n\
         \n    .section .text.f2,\"ax\",@progbits\n    .globl f2\nf2:\n    bl f3\n    adrp x0, d2\n    ret\n\
         \n    .section .text.f3,\"ax\",@progbits\n    .globl f3\nf3:\n    ret\n\
         \n    .section .text.f4,\"ax\",@progbits\n    .globl f4\nf4:\n    ret\n\
         \n    .section .text.f5,\"ax\",@progbits\n    .globl f5\nf5:\n    ret\n\
         \n    .section .data.d1,\"aw\",@progbits\n    .globl d1\nd1:\n    .xword 1\n\
         \n    .section .data.d2,\"aw\",@progbits\n    .globl d2\nd2:\n    .xword 2\n\
         \n    .section .bss.b,\"aw\",@nobits\n    .globl b\nb:\n    .zero 8\n",
        exit_with(7)
    );
    let objs = assemble(s.path(), &[("a.s", &a)]);
    let (got, out) = gc_link(s.path(), &objs, &[], "plain");
    let want = pairs(&[
        (".text.f2", "a.s.o"),
        (".text.f3", "a.s.o"),
        (".text.f4", "a.s.o"),
        (".data.d2", "a.s.o"),
        (".bss.b", "a.s.o"),
    ]);
    assert_eq!(got, want);
    assert_eq!(execute(&out).1, Some(7));
    if let Some(t) = symbols(&out) {
        for live in ["_start", "f1", "f5", "d1"] {
            assert!(has_symbol(&t, live), "{live} in .symtab:\n{t}");
        }
        for dead in ["f2", "f3", "f4", "d2", "b"] {
            assert!(!has_symbol(&t, dead), "{dead} left .symtab (S-76):\n{t}");
        }
    }
    // `-e` names the root: f2 (and with it f3, d2) instead of _start.
    let (got, _) = gc_link(s.path(), &objs, &["-e", "f2"], "entry");
    let want_e = pairs(&[
        (".text", "a.s.o"),
        (".text.f1", "a.s.o"),
        (".text.f4", "a.s.o"),
        (".text.f5", "a.s.o"),
        (".data.d1", "a.s.o"),
        (".bss.b", "a.s.o"),
    ]);
    assert_eq!(got, want_e);
    // GNU ld removes the same set.
    let out = s.join("bfd.out");
    let b = bfd(s.path(), &line(&objs), &out, &["--gc-sections", "--print-gc-sections"]);
    assert!(b.success(), "{}", b.stderr_text());
    let theirs: BTreeSet<_> = removed(&b.stderr_text()).0.into_iter().collect();
    assert_eq!(theirs, want, "GNU ld's removed set");
}

/// I-6's section rules: notes, non-allocated sections, `SHF_GNU_RETAIN`,
/// the start-up tables (`.init_array`, `.preinit_array`, `.fini_array`,
/// `.ctors`) and what they point to are roots; a section of a COMDAT group
/// is live with its group (I-7, E14 Q3); a `SHF_LINK_ORDER` section
/// follows its target (I-109).
#[test]
fn i6_section_rules_groups_and_link_order() {
    if !host() {
        return;
    }
    let s = Scratch::new("gc-rules");
    let a = format!(
        "    .text\n    .globl _start\n_start:\n    bl g_used\n{}\
         \n    .section .note.mine,\"a\",@note\n    .balign 4\n    .word 4, 4, 1\n    .ascii \"abc\\0\"\n    .word 0\n\
         \n    .section .mine.nonalloc,\"\",@progbits\n    .xword 5\n\
         \n    .section .text.retained,\"axR\",@progbits\nretained:\n    ret\n\
         \n    .section .init_array,\"aw\",@init_array\n    .xword ctor\n\
         \n    .section .preinit_array,\"aw\",@preinit_array\n    .xword pre\n\
         \n    .section .fini_array,\"aw\",@fini_array\n    .xword fini\n\
         \n    .section .text.ctor,\"ax\",@progbits\nctor:\n    ret\n\
         \n    .section .text.pre,\"ax\",@progbits\npre:\n    ret\n\
         \n    .section .text.fini,\"ax\",@progbits\nfini:\n    ret\n\
         \n    .section .text.g_used,\"axG\",@progbits,g_used,comdat\n    .globl g_used\ng_used:\n    ret\n\
         \n    .section .data.g_used,\"awG\",@progbits,g_used,comdat\n    .xword 1\n\
         \n    .section .text.g_dead,\"axG\",@progbits,g_dead,comdat\n    .globl g_dead\ng_dead:\n    ret\n\
         \n    .section .data.g_dead,\"awG\",@progbits,g_dead,comdat\n    .xword 2\n\
         \n    .section .text.lo_live,\"ax\",@progbits\nlo_live:\n    ret\n\
         \n    .section .text.lo_dead,\"ax\",@progbits\nlo_dead:\n    ret\n\
         \n    .section .meta,\"ao\",@progbits,.text.g_used\n    .xword 3\n\
         \n    .section .meta,\"ao\",@progbits,.text.lo_dead\n    .xword 4\n",
        exit_with(0)
    );
    let objs = assemble(s.path(), &[("a.s", &a)]);
    let (got, out) = gc_link(s.path(), &objs, &[], "rules");
    let want = pairs(&[
        (".text.g_dead[g_dead]", "a.s.o"),
        (".data.g_dead[g_dead]", "a.s.o"),
        (".text.lo_live", "a.s.o"),
        (".text.lo_dead", "a.s.o"),
        (".meta", "a.s.o"),
    ]);
    assert_eq!(got, want);
    assert_eq!(execute(&out).1, Some(0));
    let out = s.join("bfd.out");
    let b = bfd(s.path(), &line(&objs), &out, &["--gc-sections", "--print-gc-sections"]);
    assert!(b.success(), "{}", b.stderr_text());
    let theirs: BTreeSet<_> = removed(&b.stderr_text()).0.into_iter().collect();
    // GNU ld also lists the dead group's `.group` section.
    let theirs_wo_group: BTreeSet<_> =
        theirs.iter().filter(|(s, _)| !s.starts_with(".group")).cloned().collect();
    assert_eq!(theirs_wo_group, want, "GNU ld's removed set (without `.group`): {theirs:?}");
}

/// `__start_`/`__stop_` (L-96, GNU's `-z nostart-stop-gc`): a reference
/// to either keeps every section of that name (and what they refer to);
/// an unreferenced section with a C-identifier name is collected.
#[test]
fn l96_start_stop_references_keep_their_sections() {
    if !host() {
        return;
    }
    let s = Scratch::new("gc-startstop");
    let a = format!(
        "    .text\n    .globl _start\n_start:\n    adrp x0, __start_myset\n    adrp x1, __stop_myset\n{}\
         \n    .section myset,\"aw\",@progbits\n    .xword in_set\n\
         \n    .section .text.in_set,\"ax\",@progbits\nin_set:\n    ret\n\
         \n    .section otherset,\"aw\",@progbits\n    .xword 9\n",
        exit_with(0)
    );
    let b = "    .section myset,\"aw\",@progbits\n    .xword 2\n";
    let objs = assemble(s.path(), &[("a.s", &a), ("b.s", b)]);
    let (got, out) = gc_link(s.path(), &objs, &[], "startstop");
    assert_eq!(got, pairs(&[("otherset", "a.s.o")]));
    assert_eq!(execute(&out).1, Some(0));
}

/// S-16 under `--gc-sections`: only the referenced pieces of a split
/// string section reach the output.
#[test]
fn s16_only_referenced_merge_pieces_are_output() {
    if !host() {
        return;
    }
    let s = Scratch::new("gc-merge");
    let a = format!(
        "    .text\n    .globl _start\n_start:\n    adrp x0, live_str\n    add x0, x0, :lo12:live_str\n{}\
         \n    .section .text.dead,\"ax\",@progbits\n    adrp x0, dead_str\n\
         \n    .section .rodata.str1.1,\"aMS\",@progbits,1\nlive_str:\n    .asciz \"wp031-live-piece\"\n\
         dead_str:\n    .asciz \"wp031-dead-piece\"\n",
        exit_with(0)
    );
    let objs = assemble(s.path(), &[("a.s", &a)]);
    let (got, out) = gc_link(s.path(), &objs, &[], "merge");
    assert_eq!(got, pairs(&[(".text.dead", "a.s.o")]));
    let bytes = std::fs::read(&out).unwrap();
    let has = |n: &[u8]| bytes.windows(n.len()).any(|w| w == n);
    assert!(has(b"wp031-live-piece\0"));
    assert!(!has(b"wp031-dead-piece"), "a dead piece in the output");
    // Without --gc-sections every piece is output (I-2).
    let plain = s.join("plain.out");
    let m = mortar(s.path(), &line(&objs), &plain, &[], &[]);
    assert!(m.success(), "{}", m.stderr_text());
    let bytes = std::fs::read(&plain).unwrap();
    assert!(bytes.windows(16).any(|w| w == b"wp031-dead-piece"));
}

/// I-3 under `--gc-sections`: the FDE of a collected function leaves
/// `.eh_frame` and `.eh_frame_hdr`'s table; an FDE's LSDA and its CIE's
/// personality are edges of the code it describes (I-7), so an LSDA of a
/// collected function is collected with it.
#[test]
fn i3_fdes_and_their_lsda_follow_their_code() {
    if !host() {
        return;
    }
    let s = Scratch::new("gc-eh");
    let fun = |name: &str| {
        format!(
            "\n    .section .text.{name},\"ax\",@progbits\n    .globl {name}\n{name}:\n    .cfi_startproc\n\
             .cfi_personality 0x9b, DW.ref.pers\n    .cfi_lsda 0x1b, .Llsda_{name}\n    ret\n    .cfi_endproc\n\
             \n    .section .gcc_except_table.{name},\"a\",@progbits\n.Llsda_{name}:\n    .byte 0xff, 0xff, 1, 0\n"
        )
    };
    let a = format!(
        "    .text\n    .globl _start\n_start:\n    bl live_f\n{}{}{}\
         \n    .section .text.pers,\"ax\",@progbits\n    .globl pers\npers:\n    ret\n\
         \n    .section .data.rel.local.DW.ref.pers,\"awG\",@progbits,DW.ref.pers,comdat\n    .hidden DW.ref.pers\n    .weak DW.ref.pers\n\
         DW.ref.pers:\n    .xword pers\n",
        exit_with(0),
        fun("live_f"),
        fun("dead_f")
    );
    let objs = assemble(s.path(), &[("a.s", &a)]);
    let (got, out) = gc_link(s.path(), &objs, &["--eh-frame-hdr"], "eh");
    assert_eq!(got, pairs(&[(".text.dead_f", "a.s.o"), (".gcc_except_table.dead_f", "a.s.o")]));
    if let Some(readelf) = tool("readelf") {
        let o = run_with(
            Command::new(readelf).arg("--debug-dump=frames").arg(&out),
            b"",
            Duration::from_mins(1),
        );
        let fdes = o.stdout_text().lines().filter(|l| l.contains(" FDE ")).count();
        assert_eq!(fdes, 1, "only live_f's FDE:\n{}", o.stdout_text());
        let o = run_with(
            Command::new(tool("readelf").unwrap()).args(["-x", ".eh_frame_hdr"]).arg(&out),
            b"",
            Duration::from_mins(1),
        );
        // version 1, eh_frame_ptr sdata4, fde_count udata4, table sdata4.
        let hex: String = o
            .stdout_text()
            .lines()
            .filter(|l| l.starts_with("  0x"))
            .flat_map(|l| {
                l.split_whitespace().skip(1).take(4).map(str::to_owned).collect::<Vec<_>>()
            })
            .collect();
        assert!(hex.starts_with("011b033b"), "{hex}");
        assert_eq!(&hex[16..24], "01000000", "fde_count 1: {hex}");
    }
}

/// `--gc-keep-exported` (GNU ld): every global of default or protected
/// visibility is a root; a hidden one is not.
#[test]
fn gc_keep_exported_roots_default_visibility_globals() {
    if !host() {
        return;
    }
    let s = Scratch::new("gc-exported");
    let a = format!(
        "    .text\n    .globl _start\n_start:\n{}\
         \n    .section .text.exp,\"ax\",@progbits\n    .globl exp\nexp:\n    ret\n\
         \n    .section .text.prot,\"ax\",@progbits\n    .globl prot\n    .protected prot\nprot:\n    ret\n\
         \n    .section .text.hid,\"ax\",@progbits\n    .globl hid\n    .hidden hid\nhid:\n    ret\n\
         \n    .section .text.loc,\"ax\",@progbits\nloc:\n    ret\n",
        exit_with(0)
    );
    let objs = assemble(s.path(), &[("a.s", &a)]);
    let (got, _) = gc_link(s.path(), &objs, &[], "plain");
    assert_eq!(
        got,
        pairs(&[
            (".text.exp", "a.s.o"),
            (".text.prot", "a.s.o"),
            (".text.hid", "a.s.o"),
            (".text.loc", "a.s.o")
        ])
    );
    let (got, _) = gc_link(s.path(), &objs, &["--gc-keep-exported"], "kept");
    assert_eq!(got, pairs(&[(".text.hid", "a.s.o"), (".text.loc", "a.s.o")]));
    let out = s.join("bfd.out");
    let b = bfd(
        s.path(),
        &line(&objs),
        &out,
        &["--gc-sections", "--print-gc-sections", "--gc-keep-exported"],
    );
    assert!(b.success(), "{}", b.stderr_text());
    let theirs: BTreeSet<_> = removed(&b.stderr_text()).0.into_iter().collect();
    assert_eq!(theirs, got, "GNU ld's removed set");
}

/// Archive members (E14 Q10): an extracted member whose every section is
/// collected still counts as extracted (symbol resolution is not redone):
/// its definitions still collide; its sections are listed under its
/// `archive(member)` name.
#[test]
fn archive_members_stay_extracted_when_collected() {
    if !host() {
        return;
    }
    let Some(ar) = AARCH64.tool("ar") else { return };
    let dir = Scratch::new("gc-archive");
    let a = format!(
        "    .text\n    .globl _start\n_start:\n{}\
         \n    .section .text.dead,\"ax\",@progbits\n    bl helper\n",
        exit_with(0)
    );
    let member =
        "    .section .text.helper,\"ax\",@progbits\n    .globl helper\nhelper:\n    ret\n";
    let objs = assemble(dir.path(), &[("a.s", &a), ("m.s", member)]);
    let o = run_with(
        Command::new(&ar).args(["rcs", "libm.a", "m.s.o"]).current_dir(dir.path()),
        b"",
        Duration::from_mins(1),
    );
    assert!(o.success(), "{}", o.stderr_text());
    let inputs = vec![objs[0].clone(), "libm.a".to_owned()];
    let (got, _) = gc_link(dir.path(), &inputs, &[], "member");
    assert_eq!(got, pairs(&[(".text.dead", "a.s.o"), (".text.helper", "libm.a(m.s.o)")]));
    // A second definition of `helper` still collides with the member's.
    let c = "    .section .text.again,\"ax\",@progbits\n    .globl helper\nhelper:\n    ret\n";
    let more = assemble(dir.path(), &[("c.s", c)]);
    let inputs = vec![objs[0].clone(), "libm.a".to_owned(), more[0].clone()];
    let out = dir.join("dup.out");
    let m = mortar(dir.path(), &line(&inputs), &out, &["--gc-sections"], &[]);
    assert!(!m.success(), "a duplicate definition is still one");
    assert!(
        m.stderr_text().contains("multiple definition") || m.stderr_text().contains("duplicate"),
        "{}",
        m.stderr_text()
    );
}

/// I-47 under `--gc-sections`: a debug reference to a collected function
/// is the tombstone (all ones in ELFCLASS64), not 0 overlapping live code;
/// a reference from live code to it cannot exist (the mark follows it).
#[test]
fn i47_debug_references_to_collected_code_are_tombstones() {
    if !host() {
        return;
    }
    let s = Scratch::new("gc-tomb");
    let a = format!(
        "    .text\n    .globl _start\n_start:\n{}\
         \n    .section .text.dead,\"ax\",@progbits\n    .globl dead\ndead:\n    ret\n\
         \n    .section .debug_info,\"\",@progbits\n    .xword dead\n    .xword _start\n",
        exit_with(0)
    );
    let objs = assemble(s.path(), &[("a.s", &a)]);
    let (got, out) = gc_link(s.path(), &objs, &[], "tomb");
    assert_eq!(got, pairs(&[(".text.dead", "a.s.o")]));
    let Some(readelf) = tool("readelf") else { return };
    let o = run_with(
        Command::new(readelf).args(["-x", ".debug_info"]).arg(&out),
        b"",
        Duration::from_mins(1),
    );
    let words: Vec<String> = o
        .stdout_text()
        .lines()
        .filter(|l| l.starts_with("  0x"))
        .flat_map(|l| l.split_whitespace().skip(1).take(4).map(str::to_owned).collect::<Vec<_>>())
        .collect();
    assert_eq!(words.first().map(String::as_str), Some("ffffffff"), "{words:?}");
    assert_eq!(words.get(1).map(String::as_str), Some("ffffffff"), "{words:?}");
    assert_ne!(words.get(2).map(String::as_str), Some("00000000"), "_start's address: {words:?}");
}

/// I-47 with S-16: a debug reference to a merge piece no live section
/// references (a local, so the assembler writes section symbol + addend)
/// is the tombstone; one to a live piece is its address. Found by the
/// differential (`clang++ -O2 -g`): before it, fill's "a live merge piece
/// has no address".
#[test]
fn i47_debug_references_to_dead_merge_pieces_are_tombstones() {
    if !host() {
        return;
    }
    let s = Scratch::new("gc-tomb-piece");
    let a = format!(
        "    .text\n    .globl _start\n_start:\n    adrp x0, live_str\n    add x0, x0, :lo12:live_str\n{}\
         \n    .section .rodata.str1.1,\"aMS\",@progbits,1\nlive_str:\n    .asciz \"wp031-live-piece\"\n\
         dead_str:\n    .asciz \"wp031-dead-piece\"\n\
         \n    .section .debug_info,\"\",@progbits\n    .xword dead_str\n    .xword live_str\n",
        exit_with(0)
    );
    let objs = assemble(s.path(), &[("a.s", &a)]);
    let (got, out) = gc_link(s.path(), &objs, &[], "tomb-piece");
    assert!(got.is_empty(), "{got:?}");
    let Some(readelf) = tool("readelf") else { return };
    let o = run_with(
        Command::new(readelf).args(["-x", ".debug_info"]).arg(&out),
        b"",
        Duration::from_mins(1),
    );
    let words: Vec<String> = o
        .stdout_text()
        .lines()
        .filter(|l| l.starts_with("  0x"))
        .flat_map(|l| l.split_whitespace().skip(1).take(4).map(str::to_owned).collect::<Vec<_>>())
        .collect();
    assert_eq!(words.first().map(String::as_str), Some("ffffffff"), "{words:?}");
    assert_eq!(words.get(1).map(String::as_str), Some("ffffffff"), "{words:?}");
    assert_ne!(words.get(2).map(String::as_str), Some("ffffffff"), "the live piece: {words:?}");
    assert_ne!(words.get(2).map(String::as_str), Some("00000000"), "the live piece: {words:?}");
}

// ---- --print-gc-sections, the option pair, determinism ----------------------

/// I-10: the lines are GNU ld's text on standard error, one per removed
/// section with bytes, in `(file, section index)` order; a section of size
/// 0 is not listed; `--no-print-gc-sections` and a link without
/// `--gc-sections` print nothing; of `--gc-sections` and
/// `--no-gc-sections` the last wins.
#[test]
fn i10_print_gc_sections_text_and_order() {
    if !host() {
        return;
    }
    let s = Scratch::new("gc-print");
    let a = format!(
        "    .text\n    .globl _start\n_start:\n{}\
         \n    .section .text.z,\"ax\",@progbits\n    ret\n\
         \n    .section .text.empty,\"ax\",@progbits\n\
         \n    .section .text.a,\"ax\",@progbits\n    ret\n",
        exit_with(0)
    );
    let b = "    .section .data.b,\"aw\",@progbits\n    .xword 1\n";
    let objs = assemble(s.path(), &[("a.s", &a), ("b.s", b)]);
    let out = s.join("p.out");
    let m = mortar(s.path(), &line(&objs), &out, &["--gc-sections", "--print-gc-sections"], &[]);
    assert!(m.success(), "{}", m.stderr_text());
    assert_eq!(
        m.stderr_text(),
        "mortar: removing unused section '.text.z' in file 'a.s.o'\n\
         mortar: removing unused section '.text.a' in file 'a.s.o'\n\
         mortar: removing unused section '.data.b' in file 'b.s.o'\n"
    );
    assert_eq!(m.stdout_text(), "");
    let b =
        bfd(s.path(), &line(&objs), &s.join("b.out"), &["--gc-sections", "--print-gc-sections"]);
    let theirs: Vec<String> =
        b.stderr_text().lines().map(|l| l.split_once(": ").unwrap().1.to_owned()).collect();
    let ours: Vec<String> =
        m.stderr_text().lines().map(|l| l.split_once(": ").unwrap().1.to_owned()).collect();
    assert_eq!(ours, theirs, "GNU ld's lines and order");
    for (flags, printed) in [
        (&["--gc-sections", "--print-gc-sections", "--no-print-gc-sections"][..], false),
        (&["--print-gc-sections"][..], false),
        (&["--gc-sections", "--no-gc-sections", "--print-gc-sections"][..], false),
        (&["--no-gc-sections", "--gc-sections", "--print-gc-sections"][..], true),
    ] {
        let out = s.join("q.out");
        let m = mortar(s.path(), &line(&objs), &out, flags, &[]);
        assert!(m.success(), "{flags:?}: {}", m.stderr_text());
        assert_eq!(!m.stderr_text().is_empty(), printed, "{flags:?}: {}", m.stderr_text());
        std::fs::remove_file(&out).unwrap();
    }
}

/// The mark is a function of the inputs (I-8, `spec/model.md` §6): the
/// output, the map and the printed lines of a C++ link with debug
/// information under `--gc-sections` are the same at 1, 2, 4 and 8
/// threads, under three scheduler seeds and both stores.
#[test]
fn i8_the_mark_is_schedule_independent() {
    if !host() {
        return;
    }
    let scratch = Scratch::new("gc-determinism");
    let flags = ["-O2", "-g", "-ffunction-sections", "-fdata-sections"];
    let Some(built) = build(&scratch, Driver::Gxx, &flags, CXX_GC) else {
        assert!(!tools_required(), "g++ is required");
        return;
    };
    let gc = ["--gc-sections", "--print-gc-sections"];
    let first = built.dir.join("first.out");
    let m = mortar(&built.dir, &built.args, &first, &[&gc[..], &["--threads=1"]].concat(), &[]);
    assert!(m.success(), "{}", m.stderr_text());
    let want =
        (std::fs::read(&first).unwrap(), std::fs::read(map_of(&first)).unwrap(), m.stderr_text());
    assert!(!want.2.is_empty());
    let mut runs = 0;
    for store in ["staged", "resident"] {
        for threads in ["1", "2", "4", "8"] {
            for seed in ["", "1", "977"] {
                let out = built.dir.join(format!("o-{store}-{threads}-{seed}.out"));
                let mut env = vec![("MORTAR_STORE", store)];
                if !seed.is_empty() {
                    env.push(("MORTAR_SCHED_SEED", seed));
                }
                let t = format!("--threads={threads}");
                let m = mortar_unverified(
                    &built.dir,
                    &built.args,
                    &out,
                    &[&gc[..], &[t.as_str()]].concat(),
                    &env,
                );
                assert!(m.success(), "{store} {threads} {seed}: {}", m.stderr_text());
                let got = (
                    std::fs::read(&out).unwrap(),
                    std::fs::read(map_of(&out)).unwrap(),
                    m.stderr_text(),
                );
                assert!(
                    got.0 == want.0,
                    "{store}, {threads} threads, seed {seed:?}: the output differs"
                );
                assert!(
                    got.1 == want.1,
                    "{store}, {threads} threads, seed {seed:?}: the map differs"
                );
                assert_eq!(
                    got.2, want.2,
                    "{store}, {threads} threads, seed {seed:?}: the lines differ"
                );
                std::fs::remove_file(&out).unwrap();
                std::fs::remove_file(map_of(&out)).unwrap();
                runs += 1;
            }
        }
    }
    assert_eq!(runs, 24);
}

// ---- the compact differential ----------------------------------------------

/// A C++ program for collection: exceptions thrown across a collected
/// neighbour, RTTI, `thread_local`, static constructors, iostream,
/// templates instantiated in two units with dead instantiations.
const CXX_GC: &[support::Source] = &[
    support::Source {
        name: "g1.cpp",
        text: r#"#include <iostream>
#include <stdexcept>
#include <string>
#include <vector>
template <class T> T twice(T x) { return x + x; }
template <class T> T thrice(T x) { return x + x + x; }
int other(int);
int dead_neighbour_before(int x) { if (x > 100) throw std::logic_error("never"); return x * 5; }
int thrower(int n) { if (n > 2) throw std::runtime_error("deep"); return n; }
int dead_neighbour_after(int x) { try { return thrower(x); } catch (...) { return -1; } }
thread_local std::string where = "tls";
struct Global { Global() { std::cout << "ctor\n"; } ~Global() { std::cout << "dtor\n"; } } global;
struct Base { virtual ~Base() {} virtual int v() const { return 1; } };
struct Derived : Base { int v() const override { return 2; } };
int main(int argc, char **) {
    std::vector<std::string> v{"a", "bb"};
    Base *b = argc > 0 ? new Derived : new Base;
    try { thrower(argc + 3); } catch (const std::runtime_error &e) { std::cout << "caught " << e.what() << "\n"; }
    std::cout << twice(argc) << " " << other(argc) << " " << where << " " << dynamic_cast<Derived *>(b)->v()
              << " " << v[1] << "\n";
    delete b;
    return 3;
}
"#,
    },
    support::Source {
        name: "g2.cpp",
        text: r"#include <string>
template <class T> T twice(T x) { return x + x; }
template <class T> T thrice(T x) { return x + x + x; }
int unused_user(int x) { return thrice(x) + static_cast<int>(std::to_string(x).size()); }
int other(int x) { return twice(x) + 1; }
",
    },
];

/// Links `built` with mortar and GNU ld under `--gc-sections`: both run
/// as mortar's link without it does; mortar removes no section GNU ld
/// keeps but a COMDAT loser or `.group`; every section GNU ld removes and
/// mortar keeps is one of the numbered classes; mortar's output is
/// smaller than without gc, and passes the verifier and the readers.
fn differential(label: &str, built: &Built) {
    let dir = &built.dir;
    let nogc = dir.join("nogc.out");
    let m = mortar(dir, &built.args, &nogc, &[], &[]);
    assert!(m.success(), "{label}: {}", m.stderr_text());
    let ours = dir.join("gc.out");
    let m = mortar(dir, &built.args, &ours, &["--gc-sections", "--print-gc-sections"], &[]);
    assert!(m.success(), "{label}: {}", m.stderr_text());
    let (mine, other) = removed(&m.stderr_text());
    assert!(other.is_empty(), "{label}: {other:?}");
    let theirs = dir.join("bfd.out");
    let gnu_map = dir.join("bfd.map");
    let gnu_map_arg = gnu_map.to_str().expect("a UTF-8 scratch path");
    let b = bfd(
        dir,
        &built.args,
        &theirs,
        &["--gc-sections", "--print-gc-sections", "-Map", gnu_map_arg],
    );
    assert!(b.success(), "{label}: {}", b.stderr_text());
    let (gnu, _) = removed(&b.stderr_text());
    let want = execute(&nogc);
    assert_eq!(execute(&ours), want, "{label}: with gc against without");
    assert_eq!(execute(&theirs), want, "{label}: GNU ld's against mortar's");
    let mine: BTreeSet<_> = mine.into_iter().collect();
    let gnu: BTreeSet<_> = gnu.into_iter().collect();
    let only_ours: Vec<_> = mine.difference(&gnu).collect();
    let map = std::fs::read_to_string(map_of(&ours)).unwrap();
    // COMDAT keepers (spec/resolve.md §6, ruling C): mortar keeps a
    // group's copy of the least live FileId, GNU ld the first loaded, so
    // the two may keep the copies of different files, and what only the
    // copy GNU keeps refers to is GNU's alone to keep. Tolerated: a
    // section of file F that GNU keeps and mortar removes when (i) GNU's
    // map has F's copy of a COMDAT group kept and (ii) mortar dropped F's
    // copy of that group (`discard ... comdat`: it kept another file's).
    // That the section is reachable in GNU's link only through F's copy
    // (iii) is not shown: it belongs to F, which is what is checked.
    let (keepers, our_keepers) =
        keeper_differences(dir, &map, &std::fs::read_to_string(&gnu_map).unwrap());
    let (tolerated, only_ours): (Vec<_>, Vec<_>) =
        only_ours.into_iter().partition(|(_, file)| keepers.contains_key(file.as_str()));
    eprintln!(
        "{label}: {} sections tolerated (COMDAT keepers differ): {tolerated:?}",
        tolerated.len()
    );
    // The keepers differ only with Debian's cross libstdc++ (the AArch64
    // library of an x86-64 host); with the native one they agree, and
    // nothing is tolerated.
    for (sec, file) in &tolerated {
        let path = &keepers[file.as_str()];
        assert!(
            path.contains("/gcc-cross/"),
            "{label}: {sec} in {file} ({path}): COMDAT keepers differ outside the cross libraries"
        );
    }
    assert!(only_ours.is_empty(), "{label}: mortar removes what GNU ld keeps: {only_ours:?}");
    let comdat =
        map.lines().filter(|l| l.starts_with("discard ") && l.ends_with(" comdat")).count();
    // The other way round: a section of file F that mortar keeps and GNU
    // ld removes, where mortar keeps F's copy of a group whose copy GNU
    // keeps from another file (and discards F's), under the same
    // conditions.
    let mut tolerated_gnu = Vec::new();
    for (sec, file) in gnu.difference(&mine) {
        // `.group` sections (mortar outputs none, so lists none), and
        // COMDAT copies mortar dropped in resolution (GNU ld lists its
        // losers of archive members as collected).
        let allowed = sec == ".group" || (comdat > 0 && sec.contains('['));
        if !allowed && let Some(path) = our_keepers.get(file.as_str()) {
            assert!(
                path.contains("/gcc-cross/"),
                "{label}: {sec} in {file} ({path}): COMDAT keepers differ outside the cross \
                 libraries"
            );
            tolerated_gnu.push((sec, file));
            continue;
        }
        assert!(allowed, "{label}: GNU ld removes {sec} in {file}, mortar keeps it");
    }
    eprintln!(
        "{label}: {} sections GNU ld removes tolerated (COMDAT keepers differ): {tolerated_gnu:?}",
        tolerated_gnu.len()
    );
    let size = |p: &Path| std::fs::metadata(p).unwrap().len();
    assert!(size(&ours) < size(&nogc), "{label}: gc does not shrink the output");
    support::plumb::readers(&ours);
}

/// The files whose COMDAT copies the two links keep differently, by the
/// name `--print-gc-sections` gives them (`libstdc++.a(random.o)`), each
/// with the path of the file or archive: first the files whose copy of
/// some group GNU ld keeps and mortar drops (`discard <file> <section>
/// comdat` in mortar's `map`, the same section name kept from the file in
/// GNU ld's `gnu_map`), then the files whose copy of such a group mortar
/// keeps and GNU ld discards (its "Discarded input sections", not dropped
/// by mortar).
fn keeper_differences(dir: &Path, map: &str, gnu_map: &str) -> (Keepers, Keepers) {
    // mortar's files: id -> (path, member).
    let mut files: BTreeMap<&str, (&str, Option<&str>)> = BTreeMap::new();
    let mut dropped: BTreeMap<&str, Vec<usize>> = BTreeMap::new();
    for line in map.lines() {
        let w: Vec<&str> = line.split(' ').collect();
        match w.as_slice() {
            ["file", id, path] => {
                files.insert(id, (path, None));
            }
            ["file", id, path, member, _offset] => {
                files.insert(id, (path, Some(member)));
            }
            ["discard", id, index, "comdat"] => {
                dropped.entry(id).or_default().push(index.parse().unwrap());
            }
            _ => {}
        }
    }
    let (discarded, body) = gnu_map.split_once("Linker script and memory map").unwrap_or(("", ""));
    let discarded = discarded.split_once("Discarded input sections").map_or("", |(_, d)| d);
    // GNU's kept and discarded input sections: (name, file name).
    let kept = gnu_map_sections(body);
    let gnu_dropped = gnu_map_sections(discarded);
    // mortar's dropped copies: (name, file name).
    let mut ours_dropped: BTreeSet<(String, String)> = BTreeSet::new();
    let mut gnu_side = Keepers::new();
    let mut groups: BTreeSet<String> = BTreeSet::new();
    for (id, indices) in &dropped {
        let Some(&(path, member)) = files.get(id) else { continue };
        let base = path.rsplit('/').next().unwrap_or(path);
        let shown = member.map_or_else(|| base.to_owned(), |m| format!("{base}({m})"));
        let names = section_names(dir, path, member);
        for name in indices.iter().filter_map(|&i| names.get(i)) {
            ours_dropped.insert((name.clone(), shown.clone()));
            if kept.contains_key(&(name.clone(), shown.clone())) {
                groups.insert(name.clone());
                gnu_side.insert(shown.clone(), path.to_owned());
            }
        }
    }
    let mut mortar_side = Keepers::new();
    for ((name, shown), path) in &gnu_dropped {
        if groups.contains(name) && !ours_dropped.contains(&(name.clone(), shown.clone())) {
            mortar_side.insert(shown.clone(), path.clone());
        }
    }
    (gnu_side, mortar_side)
}

/// File name (`lib.a(member)` or the file's) -> path of the file or
/// archive.
type Keepers = BTreeMap<String, String>;

/// The input sections a part of a GNU ld map lists: (section name, file
/// name as `--print-gc-sections` gives it) -> the file's or archive's
/// path. A section line is ` .name addr size file`, or ` .name` alone
/// with the rest on the next line.
fn gnu_map_sections(text: &str) -> BTreeMap<(String, String), String> {
    let mut out = BTreeMap::new();
    let mut pending: Option<&str> = None;
    for line in text.lines() {
        let words: Vec<&str> = line.split_whitespace().collect();
        let (name, rest) = match (pending.take(), words.as_slice()) {
            (Some(name), [addr, _, ..]) if addr.starts_with("0x") => (name, &words[..]),
            (_, [name]) if line.starts_with(" .") => {
                pending = Some(name);
                continue;
            }
            (_, [name, addr, ..]) if line.starts_with(" .") && addr.starts_with("0x") => {
                (*name, &words[1..])
            }
            _ => continue,
        };
        if let [_, _, file, ..] = rest {
            let base = file.rsplit('/').next().unwrap_or(file);
            let path = file.split_once('(').map_or(*file, |(p, _)| p);
            out.insert((name.to_owned(), base.to_owned()), path.to_owned());
        }
    }
    out
}

/// The section names of an object, or of an archive's member, by index.
fn section_names(dir: &Path, path: &str, member: Option<&str>) -> Vec<String> {
    let object = match member {
        Some(member) => {
            let o = run_in(
                &AARCH64.tool("ar").expect("ar"),
                &["p".into(), path.into(), member.into()],
                dir,
                &[],
            );
            assert!(o.success(), "ar p {path} {member}: {}", o.stderr_text());
            let file = dir.join(format!("member-{member}"));
            std::fs::write(&file, &o.stdout).unwrap();
            file
        }
        None => dir.join(path),
    };
    let o = run_in(
        &AARCH64.tool("readelf").expect("readelf"),
        &["-SW".into(), object.clone().into_os_string()],
        dir,
        &[],
    );
    assert!(o.success(), "readelf -SW {}: {}", object.display(), o.stderr_text());
    let mut names = Vec::new();
    for line in o.stdout_text().lines() {
        let Some((index, rest)) =
            line.trim_start().strip_prefix('[').and_then(|l| l.split_once(']'))
        else {
            continue;
        };
        let Ok(index) = index.trim().parse::<usize>() else { continue };
        if names.len() <= index {
            names.resize(index + 1, String::new());
        }
        // Section 0 has no name: its type follows the bracket.
        if index > 0 {
            rest.split_whitespace().next().unwrap_or("").clone_into(&mut names[index]);
        }
    }
    names
}

#[test]
fn differential_c_and_cxx_against_gnu_ld() {
    if !host() {
        return;
    }
    let fs = ["-ffunction-sections", "-fdata-sections"];
    let cases: &[(&str, Driver, &[&str], &[support::Source])] = &[
        ("c-gcc-fs", Driver::Gcc, &[&["-O2"][..], &fs].concat(), support::C_PROGRAM),
        ("c-clang-O0", Driver::Clang, &["-O0"], support::C_PROGRAM),
        ("cxx-gxx-fs-g", Driver::Gxx, &[&["-O2", "-g"][..], &fs].concat(), CXX_GC),
        ("cxx-clangxx-fs", Driver::Clangxx, &[&["-O2"][..], &fs].concat(), CXX_GC),
        ("cxx-gxx-O0", Driver::Gxx, &["-O0"], CXX_GC),
        (
            "f-gfortran-fs",
            Driver::Gfortran,
            &[&["-O2"][..], &fs].concat(),
            support::FORTRAN_PROGRAM,
        ),
    ];
    for (label, driver, flags, program) in cases {
        let scratch = Scratch::new(&format!("gc-diff-{label}"));
        let Some(built) = build(&scratch, *driver, flags, program) else {
            assert!(!tools_required(), "{} is required", driver.tool());
            continue;
        };
        differential(label, &built);
    }
}

/// gdb and `llvm-dwarfdump --verify` on a collected C++ program with
/// debug information: the verifier finds no more errors than in GNU ld's
/// output of the same link, and gdb places a breakpoint on a live
/// function, runs to it, and has no line for a collected one.
#[test]
fn debuggers_read_a_collected_program() {
    if !host() {
        return;
    }
    let scratch = Scratch::new("gc-debug");
    let flags = ["-O0", "-g", "-ffunction-sections", "-fdata-sections"];
    let Some(built) = build(&scratch, Driver::Gxx, &flags, CXX_GC) else {
        assert!(!tools_required(), "g++ is required");
        return;
    };
    let ours = built.dir.join("gc.out");
    let m = mortar(&built.dir, &built.args, &ours, &["--gc-sections"], &[]);
    assert!(m.success(), "{}", m.stderr_text());
    let theirs = built.dir.join("bfd.out");
    let b = bfd(&built.dir, &built.args, &theirs, &["--gc-sections"]);
    assert!(b.success(), "{}", b.stderr_text());
    if let Some(dd) = tool("llvm-dwarfdump") {
        let verify = |p: &Path| {
            let o = run_with(Command::new(&dd).arg("--verify").arg(p), b"", Duration::from_mins(2));
            let errors = o.stdout_text().lines().filter(|l| l.starts_with("error:")).count();
            (o.success(), errors, o.stdout_text())
        };
        let (ok_m, err_m, text_m) = verify(&ours);
        let (_, err_b, _) = verify(&theirs);
        assert!(
            ok_m || err_m <= err_b,
            "llvm-dwarfdump --verify: {err_m} errors (GNU ld's: {err_b}):\n{text_m}"
        );
    }
    // A debugger session runs the program: natively only (gdb would
    // need qemu's remote stub off an AArch64 host).
    if !AARCH64.is_host() {
        mortar_testutil::named_skip(
            "gc::debuggers_read_a_collected_program (gdb session)",
            "gdb runs the AArch64 program: on an AArch64 host only",
        );
        return;
    }
    let Some(gdb) = tool("gdb") else { return };
    let o = run_with(
        Command::new(gdb)
            .args([
                "-nx",
                "-batch",
                "-ex",
                "break thrower",
                "-ex",
                "run",
                "-ex",
                "bt 1",
                "-ex",
                "info line dead_neighbour_before",
                "-ex",
                "kill",
            ])
            .arg(&ours)
            .current_dir(&built.dir),
        b"",
        Duration::from_mins(2),
    );
    let text = o.stdout_text() + &o.stderr_text();
    assert!(text.contains("Breakpoint 1, thrower"), "gdb stops in thrower:\n{text}");
    assert!(!text.contains("is at address 0x0 "), "a collected function at address 0:\n{text}");
}
