//! `--gdb-index` against lld's and mold's (WP-097, `spec/synth.md` §17):
//! the same objects, compiled by GCC and Clang in the split and non-split
//! forms that carry the GNU name index, linked by the three linkers with
//! `--gdb-index` for each of the three architectures, plain and with
//! `--gc-sections`. The indexes are read here, by a reader of gdb's
//! format that shares nothing with mortar's writer, and their symbolic
//! content must be equal: the CU list's lengths, each unit's address
//! ranges by length, and every name with its CU-vector values. Every
//! difference allowed names its reason:
//!
//! - the number of slots: mortar and lld use at least 1024, mold the
//!   smallest power of two above 4/3 of the names (gdb takes any power of
//!   two, S-G7);
//! - a value lld writes twice in one name's vector (GCC lists a type in
//!   pubtypes once per DIE that names it: a struct and its typedef, or a
//!   declaration and its definition); mortar and mold write each value
//!   once (S-G4: sorted and unique, what `gdb-add-index` writes);
//! - the alignment (lld 1, mold and mortar 4) and the section's place.
//!
//! mortar's index must also be well formed on its own: version 7, the
//! header offsets in order, every name found by gdb's hash and probe at
//! its slot, every vector sorted and unique, every range non-empty, and
//! the slot count S-G7's rule; `readelf --debug-dump=gdb_index` and
//! `llvm-dwarfdump --gdb-index` read it without a complaint, and the
//! verifier finds nothing in the output.

mod support;

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

use mortar_testutil::target::Arch;
use mortar_testutil::{Scratch, run, tool, tools_required};
use support::{Source, build_by, with_output};

const MORTAR: &str = env!("CARGO_BIN_EXE_mortar");

/// Three units: a template kept from the first (its copy in the second
/// is a discarded COMDAT copy), a function `--gc-sections` collects, a
/// namespace, a type only one unit names, a file-local variable.
const SOURCES: &[Source] = &[
    Source {
        name: "gi_main.cpp",
        text: r#"#include <cstdio>
template <class T> __attribute__((noinline)) T twice(T x) { return x + x; }
struct Point { int x; double y; };
int g_total = 3;
int helper(int);
int c_use();
int main(int argc, char **) {
  Point p{argc, 2.5};
  std::printf("%d %d %d\n", twice(argc) + g_total, helper(argc) + (int)p.y, c_use());
  return 0;
}
"#,
    },
    Source {
        name: "gi_b.cpp",
        text: r"template <class T> __attribute__((noinline)) T twice(T x) { return x + x; }
__attribute__((noinline)) int unused_fn(int x) { return x * 9; }
__attribute__((noinline)) int helper(int x) { return twice(x + 1) + 4; }
",
    },
    Source {
        name: "gi_c.cpp",
        text: r"namespace ns { struct OnlyInC { int a; long b; }; __attribute__((noinline)) int c_fn(int x) { return x * 3; } }
ns::OnlyInC g_only_c = {4, 2};
static int s_c_static = 3;
int c_use() { return ns::c_fn(g_only_c.a) + s_c_static; }
",
    },
];

/// The forms: tag, Clang (else GCC), flags.
const FORMS: &[(&str, bool, &[&str])] = &[
    ("gcc-split-d5-O0", false, &["-g", "-gsplit-dwarf", "-gdwarf-5", "-O0"]),
    ("gcc-split-d4-O0", false, &["-g", "-gsplit-dwarf", "-gdwarf-4", "-O0"]),
    ("gcc-split-d5-O2", false, &["-g", "-gsplit-dwarf", "-gdwarf-5", "-O2"]),
    ("gcc-pubnames-d5-O2", false, &["-g", "-ggnu-pubnames", "-gdwarf-5", "-O2"]),
    ("gcc-pubnames-d4-O0", false, &["-g", "-ggnu-pubnames", "-gdwarf-4", "-O0"]),
    ("clang-split-d5-O0", true, &["-g", "-gsplit-dwarf", "-gdwarf-5", "-O0"]),
    ("clang-split-d4-O2", true, &["-g", "-gsplit-dwarf", "-gdwarf-4", "-O2"]),
    ("clang-pubnames-d5-O2", true, &["-g", "-ggnu-pubnames", "-gdwarf-5", "-O2"]),
];

// ---------------------------------------------------------------------
// A reader of gdb's format (version 7), independent of mortar's writer.

/// gdb's `mapped_index_string_hash` for version 5 and later.
fn gdb_hash(name: &[u8]) -> u32 {
    name.iter().fold(0u32, |h, &c| {
        h.wrapping_mul(67).wrapping_add(u32::from(c.to_ascii_lowercase())).wrapping_sub(113)
    })
}

/// An index, symbolically.
#[derive(Debug, PartialEq, Eq)]
struct Index {
    version: u32,
    cu_lengths: Vec<u64>,
    tus: usize,
    /// Per unit: its range lengths, sorted.
    ranges: BTreeMap<u32, Vec<u64>>,
    /// Each name and its CU-vector values, as written.
    names: BTreeMap<Vec<u8>, Vec<u32>>,
    slots: usize,
}

fn u32_at(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(b[at..at + 4].try_into().expect("4 bytes"))
}

fn u64_at(b: &[u8], at: usize) -> u64 {
    u64::from_le_bytes(b[at..at + 8].try_into().expect("8 bytes"))
}

/// Reads `b`; panics naming what is wrong. With `strict`, the checks of
/// mortar's own index (S-G7) too.
fn read_index(label: &str, b: &[u8], strict: bool) -> Index {
    let h: Vec<usize> = (0..6).map(|i| u32_at(b, 4 * i) as usize).collect();
    let (version, cu, tu, addr, sym, pool) = (h[0] as u32, h[1], h[2], h[3], h[4], h[5]);
    assert!(
        cu <= tu && tu <= addr && addr <= sym && sym <= pool && pool <= b.len(),
        "{label}: {h:?}"
    );
    assert!((tu - cu) % 16 == 0 && (addr - tu) % 24 == 0 && (sym - addr) % 20 == 0, "{label}");
    let n_cu = (tu - cu) / 16;
    let mut cu_lengths = Vec::new();
    let mut last_end = 0;
    for i in 0..n_cu {
        let (off, len) = (u64_at(b, cu + 16 * i), u64_at(b, cu + 16 * i + 8));
        if strict {
            assert!(off >= last_end && len > 0, "{label}: CU {i} at {off:#x} length {len:#x}");
        }
        last_end = off + len;
        cu_lengths.push(len);
    }
    let mut ranges: BTreeMap<u32, Vec<u64>> = BTreeMap::new();
    for i in 0..(sym - addr) / 20 {
        let at = addr + 20 * i;
        let (lo, hi, unit) = (u64_at(b, at), u64_at(b, at + 8), u32_at(b, at + 16));
        assert!(hi > lo && (unit as usize) < n_cu, "{label}: range {i}: {lo:#x} {hi:#x} {unit}");
        ranges.entry(unit).or_default().push(hi - lo);
    }
    for v in ranges.values_mut() {
        v.sort_unstable();
    }
    let slots = (pool - sym) / 8;
    assert!(slots.is_power_of_two(), "{label}: {slots} slots");
    let mask = slots - 1;
    let mut names = BTreeMap::new();
    for slot in 0..slots {
        let (name_at, vec_at) =
            (u32_at(b, sym + 8 * slot) as usize, u32_at(b, sym + 8 * slot + 4) as usize);
        if name_at == 0 && vec_at == 0 {
            continue;
        }
        let start = pool + name_at;
        let end = start + b[start..].iter().position(|&c| c == 0).expect("a NUL");
        let name = b[start..end].to_vec();
        let count = u32_at(b, pool + vec_at) as usize;
        let values: Vec<u32> = (0..count).map(|k| u32_at(b, pool + vec_at + 4 + 4 * k)).collect();
        for v in &values {
            assert!(((v & 0x00ff_ffff) as usize) < n_cu, "{label}: {v:#x} names no unit");
        }
        if strict {
            // gdb's probe from the name's hash reaches this slot first.
            let hash = gdb_hash(&name) as usize;
            let step = ((hash.wrapping_mul(17)) & mask) | 1;
            let mut at = hash & mask;
            while at != slot {
                let taken = u32_at(b, sym + 8 * at) != 0 || u32_at(b, sym + 8 * at + 4) != 0;
                assert!(
                    taken,
                    "{label}: {:?} is not where gdb looks",
                    String::from_utf8_lossy(&name)
                );
                at = (at + step) & mask;
            }
            assert!(values.windows(2).all(|w| w[0] < w[1]), "{label}: {values:x?} not sorted");
        }
        assert!(names.insert(name, values).is_none(), "{label}: a name twice");
    }
    if strict {
        let want = (4 * names.len() / 3 + 1).next_power_of_two().max(1024);
        assert_eq!(slots, want, "{label}: S-G7's size for {} names", names.len());
        assert_eq!(version, 7, "{label}");
    }
    Index { version, cu_lengths, tus: (addr - tu) / 24, ranges, names, slots }
}

/// The bytes of section `name` of the ELF file at `path` (either class,
/// little-endian).
fn section(path: &Path, name: &str) -> Option<Vec<u8>> {
    let b = std::fs::read(path).expect("the output");
    let wide = b[4] == 2;
    let rd = |at: usize, n: usize| -> usize {
        let mut v = 0usize;
        for (i, x) in b[at..at + n].iter().enumerate() {
            v |= usize::from(*x) << (8 * i);
        }
        v
    };
    let (shoff, shentsize, shnum, shstrndx) = if wide {
        (rd(0x28, 8), rd(0x3a, 2), rd(0x3c, 2), rd(0x3e, 2))
    } else {
        (rd(0x20, 4), rd(0x2e, 2), rd(0x30, 2), rd(0x32, 2))
    };
    let field = |i: usize| -> (usize, usize, usize) {
        let at = shoff + i * shentsize;
        if wide {
            (rd(at, 4), rd(at + 0x18, 8), rd(at + 0x20, 8))
        } else {
            (rd(at, 4), rd(at + 0x10, 4), rd(at + 0x14, 4))
        }
    };
    let (_, stroff, _) = field(shstrndx);
    (0..shnum).find_map(|i| {
        let (n, off, size) = field(i);
        let s = stroff + n;
        let e = s + b[s..].iter().position(|&c| c == 0)?;
        (&b[s..e] == name.as_bytes()).then(|| b[off..off + size].to_vec())
    })
}

/// lld's index with each vector's repeated values written once.
fn deduped(mut i: Index) -> Index {
    for v in i.names.values_mut() {
        v.dedup();
    }
    i
}

/// The first difference of two indexes, slots aside.
fn differ(a: &Index, b: &Index) -> Option<String> {
    if a.version != b.version || a.cu_lengths != b.cu_lengths || a.tus != b.tus {
        return Some(format!(
            "header: {:?}/{:?} against {:?}/{:?}",
            a.version, a.cu_lengths, b.version, b.cu_lengths
        ));
    }
    if a.ranges != b.ranges {
        return Some(format!("ranges: {:?} against {:?}", a.ranges, b.ranges));
    }
    for (n, v) in &a.names {
        if b.names.get(n) != Some(v) {
            return Some(format!(
                "{:?}: {v:x?} against {:x?}",
                String::from_utf8_lossy(n),
                b.names.get(n)
            ));
        }
    }
    b.names
        .keys()
        .find(|n| !a.names.contains_key(*n))
        .map(|n| format!("{:?} only in the second", String::from_utf8_lossy(n)))
}

// ---------------------------------------------------------------------

/// A linker, linking `args` (the driver's link line) with `--gdb-index`.
fn link(label: &str, exe: &Path, args: &[String], out: &Path, dir: &Path, extra: &[&str]) {
    let mut a = with_output(args, out);
    a.push("--gdb-index".into());
    a.extend(extra.iter().map(OsString::from));
    let o = mortar_testutil::run_with(
        std::process::Command::new(exe).args(&a).current_dir(dir),
        b"",
        std::time::Duration::from_mins(5),
    );
    assert!(o.success(), "{label}: {}: {}", exe.display(), o.stderr_text());
    if exe == Path::new(MORTAR) {
        assert_eq!(o.stderr_text(), "", "{label}: mortar warns");
    }
}

/// The readers on mortar's index: status 0, no warning or error.
fn readers(label: &str, arch: Arch, prog: &Path, dir: &Path) {
    let readelf = arch.tool("readelf").expect("readelf");
    let o = mortar_testutil::run_with(
        std::process::Command::new(&readelf)
            .arg("--debug-dump=gdb_index")
            .arg(prog)
            .current_dir(dir),
        b"",
        std::time::Duration::from_mins(1),
    );
    let all = format!("{}{}", o.stdout_text(), o.stderr_text());
    assert!(o.success() && all.contains("Version 7"), "{label}: readelf: {all}");
    assert!(!all.to_ascii_lowercase().contains("warning"), "{label}: readelf warns: {all}");
    if let Some(dd) = tool("llvm-dwarfdump") {
        let o = run(&dd, [OsString::from("--gdb-index"), prog.into()]);
        let all = format!("{}{}", o.stdout_text(), o.stderr_text());
        assert!(o.success() && all.contains("Version = 7"), "{label}: llvm-dwarfdump: {all}");
        let low = all.to_ascii_lowercase();
        assert!(
            !low.contains("error") && !low.contains("warning"),
            "{label}: llvm-dwarfdump: {all}"
        );
    }
}

/// The three linkers on one form for one architecture.
fn compare(arch: Arch, (tag, clang, flags): (&str, bool, &[&str]), gc: bool) -> bool {
    let label = format!("gdb-index-{}-{tag}{}", arch.name(), if gc { "-gc" } else { "" });
    let (Some(lld), Some(mold)) = (tool("ld.lld"), tool("mold")) else {
        assert!(!tools_required(), "{label}: ld.lld and mold are required");
        return false;
    };
    let scratch = Scratch::new(&label);
    let mut all: Vec<String> = flags.iter().map(|s| (*s).to_owned()).collect();
    all.push("-ffunction-sections".into());
    let cc = if clang {
        let mut a = arch.clang_args();
        a.append(&mut all);
        all = a;
        "clang++".to_owned()
    } else {
        all.push("-fno-use-linker-plugin".into());
        arch.gnu("g++")
    };
    let refs: Vec<&str> = all.iter().map(String::as_str).collect();
    let Some(built) = build_by(&scratch, &cc, &refs, SOURCES) else {
        assert!(!tools_required(), "{label}: {cc} is required");
        return false;
    };
    let extra: &[&str] = if gc { &["--gc-sections"] } else { &[] };
    let dir = &built.dir;
    let ours = dir.join("prog-mortar");
    let map = dir.join("prog-mortar.map");
    let mut ex: Vec<&str> = extra.to_vec();
    let map_arg = format!("--mortar-map={}", map.display());
    ex.push(&map_arg);
    link(&label, Path::new(MORTAR), &built.args, &ours, dir, &ex);
    support::plumb::clean(&ours, &map, dir);
    readers(&label, arch, &ours, dir);
    let ours_i = read_index(
        &format!("{label}: mortar"),
        &section(&ours, ".gdb_index").expect("mortar's .gdb_index"),
        true,
    );
    assert_eq!(ours_i.cu_lengths.len(), 3, "{label}: three units");
    assert!(
        section(&ours, ".debug_gnu_pubnames").is_none(),
        "{label}: the sets are consumed (S-G6)"
    );
    for (name, exe) in [("lld", &lld), ("mold", &mold)] {
        let theirs: PathBuf = dir.join(format!("prog-{name}"));
        link(&label, exe, &built.args, &theirs, dir, extra);
        let b = section(&theirs, ".gdb_index")
            .unwrap_or_else(|| panic!("{label}: {name} wrote no index"));
        let mut t = read_index(&format!("{label}: {name}"), &b, false);
        if name == "lld" {
            t = deduped(t);
        }
        if let Some(d) = differ(&ours_i, &t) {
            panic!("{label}: mortar's index against {name}'s: {d}");
        }
    }
    // The kept copy of twice<int> is the first unit's: both units name
    // it, only the first has its range (S-G4, S-G5 rule 2).
    let twice = &ours_i.names[&b"twice<int>"[..]];
    assert_eq!(
        twice.iter().map(|v| v & 0xff_ffff).collect::<Vec<_>>(),
        [0, 1],
        "{label}: {twice:x?}"
    );
    if gc {
        // unused_fn's name stays (S-G5a).
        assert!(ours_i.names.contains_key(&b"unused_fn"[..]), "{label}: unused_fn's name");
    }
    true
}

fn each(arch: Arch) {
    let mut ran = 0;
    for &(tag, clang, flags) in FORMS {
        for gc in [false, true] {
            if gc && !tag.ends_with("O0") {
                continue;
            }
            ran += usize::from(compare(arch, (tag, clang, flags), gc));
        }
    }
    eprintln!("note: gdb-index {}: {ran} comparisons", arch.name());
}

// spec: synth.md S-G2 S-G4 S-G5 S-G5a S-G6 S-G7
#[test]
fn against_lld_and_mold_aarch64() {
    each(Arch::Aarch64);
}

// spec: synth.md S-G2 S-G4 S-G5 S-G5a S-G6 S-G7
#[test]
fn against_lld_and_mold_x86_64() {
    each(Arch::X86_64);
}

// spec: synth.md S-G2 S-G4 S-G5 S-G5a S-G6 S-G7
#[test]
fn against_lld_and_mold_i386() {
    each(Arch::I386);
}
