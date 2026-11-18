//! P-D6's D-19a order for a shared object mentioned again inside a
//! linker script (`spec/plumb.md` §12.1a, `spec/dyn.md` D-19a; WP-193):
//! GNU ld's and mortar's AArch64 `-pie` outputs of the lane's
//! `needed-order-script-group.test` (`g-anx-obj`, `nested-group`) and
//! `needed-order-script.test` (`s-twice-off`; mortar's `input-then-group`,
//! which GNU ld refuses), checked in under `tests/scriptorder/`
//! (`gen.sh`). Each is clean; its `DT_NEEDED` list permuted to lld's
//! order (and, for `input-then-group`, to the list mortar's base wrote)
//! is P-D6; a mortar map whose `arg` records or file names are changed,
//! the image unchanged, is judged as before: the order comes from the
//! line and the scripts the verifier reads, never from what the judged
//! linker writes of them.
//!
//! The lines are GCC 14's driver lines of an AArch64 host: they name the
//! host's crt objects, `-lgcc`, `-lgcc_s`, `-lc` and `-L` directories by
//! absolute paths. Those are read from `tests/scriptorder/sys/`, never
//! from the host running the test (WP-241: on an x86-64 host the host's
//! own files are absent and every verdict was I1/P-D1/P-D2): the crt
//! objects, the two GNU ld scripts and `libc_nonshared.a` are the AArch64
//! host's own bytes, and `libc.so.6`, `ld-linux-aarch64.so.1`,
//! `libgcc_s.so.1` and `libgcc.a` stand-ins built from `stub/` (`gen.sh`)
//! with the sonames, `DT_NEEDED` edges, versions and symbols the outputs
//! bind. `s12_1a_the_sysroot_stands_for_the_aarch64_host` checks on an
//! AArch64 host with those files that every verdict is the same with the
//! host's real files; elsewhere it is a named skip.

use std::path::{Component, Path, PathBuf};

use mortar_plumb::{Ctx, Files, Format, Input, Report};

/// The fixture directory, and where an absolute path of the line or the
/// map is read: `root` joined with the path (normalized without leaving
/// `root`). An absolute path into the fixture directory itself is read
/// there (`s12_1a_an_input_the_line_does_not_name_stops_the_order`).
struct Dir<'a> {
    dir: &'a Path,
    root: &'a Path,
}

impl Dir<'_> {
    fn resolve(&self, path: &str) -> PathBuf {
        let p = Path::new(path);
        if !p.is_absolute() || p.starts_with(self.dir) {
            return self.dir.join(p);
        }
        let mut rel = PathBuf::new();
        for c in p.components() {
            match c {
                Component::Normal(n) => rel.push(n),
                Component::ParentDir => {
                    rel.pop();
                }
                Component::RootDir | Component::CurDir | Component::Prefix(_) => {}
            }
        }
        self.root.join(rel)
    }
    fn read_file(&self, path: &str) -> Option<Vec<u8>> {
        std::fs::read(self.resolve(path)).ok()
    }
}

impl Files for Dir<'_> {
    fn read(&self, path: &str) -> Option<Vec<u8>> {
        self.read_file(path)
    }
}

fn dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/scriptorder")
}

/// The checked-in stand-in for the AArch64 host's system files.
fn sys() -> PathBuf {
    dir().join("sys")
}

/// One fixture: the output `{cell}.{linker}`, its map with text replaced
/// (`edits`), judged with the line the lane recorded (`line`) or none,
/// its absolute paths read under `root`.
struct Fx {
    name: String,
    format: Format,
    edits: Vec<(String, String)>,
    line: bool,
    root: PathBuf,
}

impl Fx {
    fn new(cell: &str, linker: &str) -> Fx {
        let format = if linker == "mortar" { Format::Mortar } else { Format::Gnu };
        Fx { name: format!("{cell}.{linker}"), format, edits: Vec::new(), line: true, root: sys() }
    }
    fn bytes(&self) -> Vec<u8> {
        std::fs::read(dir().join(&self.name)).unwrap()
    }
    fn verify(&self, bytes: &[u8]) -> Report {
        let d = dir();
        let mut text = std::fs::read_to_string(d.join(format!("{}.map", self.name))).unwrap();
        for (a, b) in &self.edits {
            assert!(text.contains(a.as_str()), "{}: no '{a}' in the map", self.name);
            text = text.replace(a.as_str(), b);
        }
        let map = mortar_plumb::map::parse(self.format, &text).unwrap();
        let files = Dir { dir: &d, root: &self.root };
        let inputs: Vec<(String, Vec<u8>)> = map
            .paths()
            .into_iter()
            .filter_map(|p| files.read_file(p).map(|b| (p.to_string(), b)))
            .collect();
        let inputs: Vec<Input<'_>> =
            inputs.iter().map(|(p, b)| Input { path: p, bytes: b }).collect();
        let line = self.line.then(|| {
            std::fs::read_to_string(d.join(format!("{}.line", self.name)))
                .unwrap()
                .lines()
                .map(str::to_string)
                .collect()
        });
        mortar_plumb::verify_with(bytes, &inputs, &map, &Ctx { line, files: &files })
    }
    /// Every finding, anomalies included.
    fn all(&self, bytes: &[u8]) -> Vec<String> {
        self.verify(bytes)
            .findings
            .iter()
            .map(|f| format!("{} {}", f.rule.id(), f.message))
            .collect()
    }
    /// The findings no anomaly of the format names.
    fn plain(&self, bytes: &[u8]) -> Vec<String> {
        self.verify(bytes)
            .findings
            .iter()
            .filter(|f| mortar_plumb::anomaly::classify(self.format, bytes, f).is_none())
            .map(|f| format!("{} {}", f.rule.id(), f.message))
            .collect()
    }
}

// ---- ELF64 little-endian reads ----

fn le16(b: &[u8], o: usize) -> usize {
    u16::from_le_bytes([b[o], b[o + 1]]) as usize
}
fn le32(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes(b[o..o + 4].try_into().unwrap())
}
fn le64(b: &[u8], o: usize) -> u64 {
    u64::from_le_bytes(b[o..o + 8].try_into().unwrap())
}
fn cstr(b: &[u8], o: usize) -> String {
    let e = b[o..].iter().position(|&c| c == 0).unwrap();
    String::from_utf8_lossy(&b[o..o + e]).into_owned()
}

/// The `DT_NEEDED` entries of an output: the file offset of each `d_val`
/// and its name.
fn needed(b: &[u8]) -> Vec<(usize, String)> {
    let shoff = le64(b, 0x28) as usize;
    let n = le16(b, 0x3c);
    let sh = |i: usize| shoff + i * 64;
    let dynamic = (0..n).find(|&i| le32(b, sh(i) + 4) == 6).expect("SHT_DYNAMIC");
    let (off, size) = (le64(b, sh(dynamic) + 0x18) as usize, le64(b, sh(dynamic) + 0x20) as usize);
    let strs = le64(b, sh(le32(b, sh(dynamic) + 0x28) as usize) + 0x18) as usize;
    (off..off + size)
        .step_by(16)
        .take_while(|&e| le64(b, e) != 0)
        .filter(|&e| le64(b, e) == 1)
        .map(|e| (e + 8, cstr(b, strs + le64(b, e + 8) as usize)))
        .collect()
}

/// The output with its `DT_NEEDED` entries rewritten to `order` (short
/// names: `na` is `libna.so`, `c` is `libc.so.6`): each entry's `d_val`
/// set to the string offset of the name wanted there.
fn reorder(b: &[u8], order: &[&str]) -> Vec<u8> {
    let full = |s: &str| if s == "c" { "libc.so.6".to_string() } else { format!("lib{s}.so") };
    let have = needed(b);
    assert_eq!(have.len(), order.len());
    let vals: Vec<(String, u64)> = have.iter().map(|(o, n)| (n.clone(), le64(b, *o))).collect();
    let mut out = b.to_vec();
    for ((o, _), want) in have.iter().zip(order) {
        let v = vals.iter().find(|(n, _)| *n == full(want)).expect("a needed name").1;
        out[*o..*o + 8].copy_from_slice(&v.to_le_bytes());
    }
    out
}

fn short(b: &[u8]) -> Vec<String> {
    needed(b)
        .into_iter()
        .map(|(_, n)| {
            if n == "libc.so.6" {
                "c".into()
            } else {
                n.trim_start_matches("lib").trim_end_matches(".so").into()
            }
        })
        .collect()
}

fn p_d6(plain: &[String]) -> bool {
    plain.iter().any(|p| p.starts_with("P-D6 DT_NEEDED order is"))
}

/// The cells, what each linker wrote (GNU ld's and mortar's keeping
/// mention), and lld's first-mention list (`spec/landed/dyn/WP-172.md`
/// §2).
const CELLS: [(&str, &[&str], &[&str]); 3] = [
    ("g-anx-obj", &["na", "nb", "nx", "nc", "c"], &["na", "nx", "nb", "nc", "c"]),
    ("nested-group", &["na", "nb", "nx", "nc", "c"], &["na", "nx", "nb", "nc", "c"]),
    ("s-twice-off", &["na", "nb", "nx", "ny", "nc", "c"], &["na", "nx", "nb", "ny", "nc", "c"]),
];

#[test]
fn s12_1a_outputs_are_clean() {
    for (cell, wrote, _) in CELLS {
        for linker in ["bfd", "mortar"] {
            let fx = Fx::new(cell, linker);
            let b = fx.bytes();
            assert_eq!(short(&b), wrote, "{cell}.{linker}");
            let plain = fx.plain(&b);
            assert!(plain.is_empty(), "{cell}.{linker}: {plain:#?}");
        }
    }
    let fx = Fx::new("input-then-group", "mortar");
    let b = fx.bytes();
    // GNU ld refuses the line; the model's list for mortar's output is
    // the fallback (first mention), derived: the INPUT entry is in no
    // group and use_x.o comes after both mentions of libnx.so.
    assert_eq!(short(&b), ["na", "nx", "nb", "nc", "c"]);
    assert!(fx.plain(&b).is_empty(), "{:#?}", fx.plain(&b));
}

#[test]
fn s12_1a_lld_order_is_p_d6() {
    for (cell, _, lld) in CELLS {
        for linker in ["bfd", "mortar"] {
            let fx = Fx::new(cell, linker);
            let plain = fx.plain(&reorder(&fx.bytes(), lld));
            assert!(p_d6(&plain), "{cell}.{linker}: {plain:#?}");
        }
    }
}

#[test]
fn s12_1a_the_base_list_of_input_then_group_is_p_d6() {
    // mortar's base took the group of the script's other entries for the
    // INPUT entry and kept libnx.so at the script (WP-172 record §2).
    let fx = Fx::new("input-then-group", "mortar");
    let plain = fx.plain(&reorder(&fx.bytes(), &["na", "nb", "nx", "nc", "c"]));
    assert!(p_d6(&plain), "{plain:#?}");
}

/// mortar map edits that change a mention's state or group, or an
/// object's spelling, in what mortar writes of its own line and inputs.
fn map_edits(cell: &str) -> Vec<Vec<(String, String)>> {
    let e = |a: &str, b: &str| (a.to_string(), b.to_string());
    let mut out = vec![
        // libnx.so's first mention made plain (its state).
        vec![e("arg --as-needed\narg libnx.so", "arg --no-as-needed\narg libnx.so")],
        // The script's entry moved into a command-line group with use_x.o
        // (its group) and the as-needed state around it.
        vec![e(
            "arg libnb.so\n",
            "arg libnb.so\narg --start-group\narg use_x.o\narg --end-group\n",
        )],
        // mortar's arg records gone: the caller's line decides.
        vec![e("arg -pie\n", "")],
    ];
    if cell != "s-twice-off" {
        // use_x.o spelled as GNU ld's map spells it.
        out.push(vec![e(" use_x.o\n", " ./use_x.o\n")]);
    }
    out
}

#[test]
fn s12_1a_a_mutated_mortar_map_does_not_change_the_verdict() {
    for (cell, _, lld) in CELLS.iter().copied().chain([("input-then-group", &[][..], &[][..])]) {
        let base = Fx::new(cell, "mortar");
        let b = base.bytes();
        let wrong = (!lld.is_empty()).then(|| reorder(&b, lld));
        for edits in map_edits(cell) {
            let fx = Fx { edits: edits.clone(), ..Fx::new(cell, "mortar") };
            let plain = fx.plain(&b);
            assert!(plain.is_empty(), "{cell} {edits:?}: {plain:#?}");
            if let Some(w) = &wrong {
                assert!(p_d6(&fx.plain(w)), "{cell} {edits:?}");
            }
        }
        // The edit is one the model would follow if it believed the map:
        // with no caller's line the `arg` records are the line (§12.1a),
        // and libnx.so's first mention made plain keeps it there.
        if !lld.is_empty() {
            let fx =
                Fx { edits: map_edits(cell).remove(0), line: false, ..Fx::new(cell, "mortar") };
            assert!(p_d6(&fx.plain(&b)), "{cell}: the edit is followed without a line");
        }
    }
}

#[test]
fn s12_1a_an_input_the_line_does_not_name_stops_the_order() {
    // The map names use_x.o by a path no line item is (an absolute
    // spelling of the same bytes): D-19a's order is not computed (P-D2),
    // never computed with the object before every mention.
    let abs = format!("{}/use_x.o", dir().display());
    let fx = Fx { edits: vec![("./use_x.o".into(), abs.clone())], ..Fx::new("g-anx-obj", "bfd") };
    let plain = fx.plain(&fx.bytes());
    assert!(
        plain.iter().any(|p| p.starts_with("P-D2") && p.contains(&abs) && p.contains("D-19a")),
        "{plain:#?}"
    );
    assert!(!p_d6(&plain), "{plain:#?}");
}

#[test]
fn s12_1a_gnu_ld_map_spelling_is_the_same_file() {
    // GNU ld names the object a script found `./use_x.o`; spelled as the
    // line spells it, the verdicts stay.
    for cell in ["g-anx-obj", "nested-group"] {
        let fx = Fx { edits: vec![("./use_x.o".into(), "use_x.o".into())], ..Fx::new(cell, "bfd") };
        let b = fx.bytes();
        assert!(fx.plain(&b).is_empty(), "{cell}: {:#?}", fx.plain(&b));
        let lld = CELLS.iter().find(|c| c.0 == cell).unwrap().2;
        assert!(p_d6(&fx.plain(&reorder(&b, lld))), "{cell}");
    }
}

/// The system files of the lines under `sys/`: the host's own bytes
/// (`true`) or a stand-in built from `stub/` (`false`).
const SYSTEM: [(&str, bool); 12] = [
    ("/usr/lib/aarch64-linux-gnu/Scrt1.o", true),
    ("/usr/lib/aarch64-linux-gnu/crti.o", true),
    ("/usr/lib/aarch64-linux-gnu/crtn.o", true),
    ("/usr/lib/gcc/aarch64-linux-gnu/14/crtbeginS.o", true),
    ("/usr/lib/gcc/aarch64-linux-gnu/14/crtendS.o", true),
    ("/usr/lib/aarch64-linux-gnu/libc.so", true),
    ("/usr/lib/aarch64-linux-gnu/libc_nonshared.a", true),
    ("/usr/lib/gcc/aarch64-linux-gnu/14/libgcc_s.so", true),
    ("/usr/lib/gcc/aarch64-linux-gnu/14/libgcc.a", false),
    ("/usr/lib/aarch64-linux-gnu/libgcc_s.so.1", false),
    ("/lib/aarch64-linux-gnu/libc.so.6", false),
    ("/lib/ld-linux-aarch64.so.1", false),
];

#[test]
fn s12_1a_the_sysroot_stands_for_the_aarch64_host() {
    let sys = sys();
    for (p, _) in SYSTEM {
        assert!(sys.join(&p[1..]).is_file(), "sys{p} is missing");
    }
    // The host's files at the lines' paths: present, and the kept ones
    // the same bytes as `sys/`'s (else the host is not the one the lines
    // were recorded on).
    let host = |p: &str| std::fs::read(p).ok();
    let differ: Vec<&str> = SYSTEM
        .iter()
        .filter(|(p, kept)| match host(p) {
            None => true,
            Some(b) => *kept && std::fs::read(sys.join(&p[1..])).ok() != Some(b),
        })
        .map(|(p, _)| *p)
        .collect();
    if !differ.is_empty() {
        mortar_testutil::named_skip(
            "scriptorder::s12_1a_the_sysroot_stands_for_the_aarch64_host/aarch64-gcc14",
            &format!(
                "the host's GCC 14 / glibc AArch64 files are absent or other bytes: {}",
                differ.join(", ")
            ),
        );
        return;
    }
    let both = |fx: Fx, b: &[u8], what: &str| {
        let mine = fx.all(b);
        let host = Fx { root: PathBuf::from("/"), ..fx };
        assert_eq!(mine, host.all(b), "{}: {what}", host.name);
    };
    // input-then-group: mortar's base list (WP-172 record §2).
    let base: &[&str] = &["na", "nb", "nx", "nc", "c"];
    for (cell, _, lld) in CELLS.iter().copied().chain([("input-then-group", &[][..], base)]) {
        for linker in ["bfd", "mortar"] {
            if cell == "input-then-group" && linker == "bfd" {
                continue; // GNU ld refuses the line
            }
            let b = Fx::new(cell, linker).bytes();
            both(Fx::new(cell, linker), &b, "as linked");
            both(Fx::new(cell, linker), &reorder(&b, lld), "permuted");
        }
        for edits in map_edits(cell) {
            let b = Fx::new(cell, "mortar").bytes();
            both(Fx { edits, ..Fx::new(cell, "mortar") }, &b, "a mutated map");
        }
    }
}
