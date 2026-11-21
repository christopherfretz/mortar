//! WP-263: a full link's capture writes `SYMTAB` (`spec/incr.md` IN-18e:
//! per file its `.symtab` block and `.strtab` region, file 0's range the
//! global area) and `META.symtab` (IN-49a: `sh_info`, the local tail
//! reserve, the tables' sizes and file slack), and `META`'s count of live
//! `.gnu.warning.<sym>` sections whose `<sym>` is no key (255d5's F-7).
//!
//! `stored_blocks_are_the_output_blocks`: on a static C++ link (the
//! `support` program: libstdc++, libm, libc), `-g`, `--gc-sections`, in
//! incremental mode, on AArch64 and x86-64: the OUTPUT's `.symtab` and
//! `.strtab`, read with `mortar_elf`, against every stored row.
//! - Each stored block `[first, first + count)` holds no filler and only
//!   names of its file's own locals and hidden/internal definitions (the
//!   input object's `.symtab`, read from its `FILES` path and range), its
//!   first entry the file's `STT_FILE` when the input has one, no other
//!   file's; `[first + count, first + capacity)` is fillers, `capacity`
//!   IN-49a's `count + max(2, count/8)`; every `STT_FILE` entry of the
//!   output starts a stored block.
//! - Its `.strtab` region holds exactly its entries' names (fixed names
//!   apart), zeros after them to its capacity.
//! - The blocks tile `[1, tail_first)` after the internal file's block;
//!   `[tail_first, sh_info)` is fillers; `META.symtab`'s `sh_info` and
//!   sizes are the section headers', its slacks lie before the next
//!   section; file 0's first row is the global area, its second the
//!   internal file's block (WP-264: entry 1 on, its entries then its
//!   fillers up to the first file's block, its names in its region).
//!
//! `orphan_gnu_warning_is_counted`: the same link with and without a unit
//! holding `.gnu.warning.nosuchkey` (no file mentions `nosuchkey`):
//! `META.gnu_warning_orphans` 1 and 0.
//!
//! `version_10_state_is_refused`: the header with version 10 (WP-264's
//! format: no `DIAGS` row for a link that warned; WP-265 made it 11) is
//! refused
//! `state-version`, by the reader and end to end.

mod support;

use std::collections::HashMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

use mortar::incremental::State5;
use mortar_core::FileId;
use mortar_core::incr::SymtabBlock;
use mortar_elf::raw::Sym;
use mortar_incr::update::testing::MemoryState;
use mortar_testutil::target::Arch;
use mortar_testutil::{Scratch, tools_required};
use support::{Source, build_by, with_output};

const STT_SECTION: u8 = 3;
const STT_FILE: u8 = 4;

/// The unit that holds a `.gnu.warning.<sym>` section for a `<sym>` no
/// file mentions.
const ORPHAN: Source = Source {
    name: "x4.cpp",
    text: r#"__asm__(".pushsection .gnu.warning.nosuchkey,\"\",%progbits\n"
        ".string \"wp263: nosuchkey is used\"\n"
        ".popsection");
extern "C" int wp263_x4() { return 4; }
"#,
};

struct Line {
    _scratch: Scratch,
    label: &'static str,
    dir: PathBuf,
    args: Vec<String>,
}

fn lines(orphan: bool) -> Vec<Line> {
    let mut v = Vec::new();
    let mut srcs = support::CXX_PROGRAM.to_vec();
    if orphan {
        srcs.push(ORPHAN);
    }
    for (label, arch) in [("aarch64", Arch::Aarch64), ("x86-64", Arch::X86_64)] {
        let cc = arch.gnu("g++");
        let s = Scratch::new(&format!("wp263-{label}-{}", u8::from(orphan)));
        let Some(b) = build_by(&s, &cc, &["-O1", "-g"], &srcs) else {
            assert!(!tools_required(), "{cc} is required");
            continue;
        };
        let mut args = b.args;
        args.push("--gc-sections".into());
        v.push(Line { _scratch: s, label, dir: b.dir, args });
    }
    v
}

fn incremental_args(line: &Line, mode: &str) -> Vec<OsString> {
    let mut args = with_output(&line.args, &line.dir.join("out"));
    args.extend([mode, "--incremental-state", "out.mortar-state"].map(OsString::from));
    args
}

/// The link's format-5 state, decoded whole (a test's read), and its bytes.
fn state(line: &Line) -> (MemoryState, Vec<u8>) {
    let path = line.dir.join("out.state5");
    let args = incremental_args(line, "--incremental=full");
    let inv = mortar::invoke_state5(
        "mortar",
        &args,
        &line.dir,
        State5 { path: path.clone(), collide: false, rows: None },
    );
    let ok = matches!(inv.outcome, mortar::Outcome::Linked { .. });
    assert!(ok, "{}: the link failed: {:?}\n{}", line.label, inv.outcome, inv.incremental);
    let bytes = std::fs::read(&path)
        .unwrap_or_else(|_| panic!("{}: no state: {}", line.label, inv.incremental));
    (mortar_incr::state5::read::decode(&bytes).expect("the state decodes"), bytes)
}

fn text(m: &MemoryState, s: mortar_incr::format::Str) -> &[u8] {
    m.strings.get(s.off as usize..(s.off + s.len) as usize).unwrap_or_default()
}

/// S-86's fixed `.strtab` names (in no block's region).
fn fixed(name: &[u8]) -> bool {
    matches!(name, b"" | b"$x" | b"$d" | b"__erratum_843419_veneer")
}

/// The output's symbol tables as read.
struct Out {
    syms: Vec<Sym>,
    names: Vec<Vec<u8>>,
    sh_info: u32,
    strtab: Vec<u8>,
    /// `(sh_offset, sh_size, the next section's sh_offset)`.
    symtab_at: (u64, u64, Option<u64>),
    strtab_at: (u64, u64, Option<u64>),
    /// Section 1's `sh_addr` (the filler's `st_value`, IN-49a).
    sec1_addr: u64,
}

impl Out {
    fn read(out: &Path) -> Self {
        let bytes = std::fs::read(out).expect("the output");
        let h = mortar_elf::FileHeader::parse(&bytes).expect("an ELF header");
        let t = mortar_elf::section::SectionTable::parse(&bytes, &h).expect("section headers");
        let syms = t
            .find_symbol_table(mortar_elf::consts::SHT_SYMTAB)
            .expect("a symbol table")
            .expect("a .symtab");
        let mut secs: Vec<(Vec<u8>, u64, u64, u32)> = t
            .iter()
            .map(|s| {
                let n = s.name().expect("a section name").to_vec();
                (n, s.header().sh_offset, s.size(), s.sh_type())
            })
            .collect();
        let sec1_addr = t.iter().nth(1).expect("section 1").header().sh_addr;
        secs.sort_by_key(|s| s.1);
        let at = |name: &[u8]| {
            let i = secs.iter().position(|s| s.0 == name).expect("the section");
            let next = secs[i + 1..].iter().find(|s| s.3 != mortar_elf::consts::SHT_NOBITS);
            (secs[i].1, secs[i].2, next.map(|s| s.1))
        };
        let (symtab_at, strtab_at) = (at(b".symtab"), at(b".strtab"));
        let st = strtab_at;
        let strtab = bytes[st.0 as usize..(st.0 + st.1) as usize].to_vec();
        let (mut all, mut names) = (Vec::new(), Vec::new());
        for (_, s) in syms.iter() {
            names.push(syms.name(&s).expect("a name").to_vec());
            all.push(s);
        }
        Self {
            syms: all,
            names,
            sh_info: syms.first_global().0,
            strtab,
            symtab_at,
            strtab_at,
            sec1_addr,
        }
    }

    /// IN-49a's one filler form.
    fn filler(&self, i: u32) -> bool {
        let s = &self.syms[i as usize];
        s.st_name == 0
            && s.st_info == STT_SECTION
            && s.st_other == 0
            && s.st_shndx == 1
            && s.st_value == self.sec1_addr
            && s.st_size == 0
    }
}

/// The input object's own names: its `STT_FILE` names in order and the
/// multiset of the names a block of it may hold: `(name, false)` for its
/// locals other than sections, `(name, true)` for its global definitions
/// (a block holds one only demoted, S-74: hidden or internal by the merged
/// visibility, which another file's mention can make so: `_start` of
/// AArch64's `crt1.o` is hidden by libc's mention).
type Own = HashMap<(Vec<u8>, bool), u32>;

fn input_names(m: &MemoryState, file: usize, dir: &Path) -> (Vec<Vec<u8>>, Own) {
    let row = &m.files[file];
    let path = dir.join(Path::new(std::str::from_utf8(text(m, row.path)).expect("a path")));
    let all = std::fs::read(&path).unwrap_or_else(|_| panic!("{}", path.display()));
    let bytes = if row.len == 0 {
        &all[..]
    } else {
        &all[row.base as usize..(row.base + row.len) as usize]
    };
    let h = mortar_elf::FileHeader::parse(bytes).expect("an input's ELF header");
    let t = mortar_elf::section::SectionTable::parse(bytes, &h).expect("an input's sections");
    let (mut files, mut names) = (Vec::new(), HashMap::new());
    let Some(syms) = t.find_symbol_table(mortar_elf::consts::SHT_SYMTAB).expect("its .symtab")
    else {
        return (files, names);
    };
    let first = syms.first_global().0;
    for (i, s) in syms.iter() {
        let name = syms.name(&s).expect("an input name").to_vec();
        let local = i.0 < first && i.0 > 0;
        let defined = i.0 >= first && s.st_shndx != 0;
        if local && s.sym_type() == STT_FILE {
            files.push(name.clone());
        }
        if (local && s.sym_type() != STT_SECTION) || defined {
            *names.entry((name, defined)).or_insert(0) += 1;
        }
    }
    (files, names)
}

/// What [`judge`] counted.
#[derive(Default)]
struct Judged {
    blocks: usize,
    entries: u32,
    fillers: u32,
    internal: u32,
    tail: u32,
}

/// The oracle: `m`'s `SYMTAB` rows and `META.symtab` against `out`.
#[allow(clippy::too_many_lines, clippy::many_single_char_names)] // one walk, each property in its place
fn judge(label: &str, m: &MemoryState, dir: &Path) -> Judged {
    let o = Out::read(&dir.join("out"));
    let t = m.meta.symtab;
    let entries = o.syms.len() as u32;
    let mut j = Judged::default();
    // META.symtab against the section headers.
    assert_eq!(t.sh_info, o.sh_info, "{label}: META.symtab.sh_info");
    assert_eq!(t.symtab_size, o.symtab_at.1, "{label}: META.symtab.symtab_size");
    assert_eq!(t.strtab_size, o.strtab_at.1, "{label}: META.symtab.strtab_size");
    for (what, (off, size, next), slack) in
        [(".symtab", o.symtab_at, t.symtab_slack), (".strtab", o.strtab_at, t.strtab_slack)]
    {
        assert!(slack >= 4096, "{label}: {what}'s slack {slack} (IN-49a: at least 4 KiB)");
        if let Some(n) = next {
            assert!(off + size + slack <= n, "{label}: {what}'s slack runs into the next section");
        }
    }
    // The tail reserve.
    assert_eq!(t.tail_first + t.tail_capacity, t.sh_info, "{label}: the tail ends at sh_info");
    for i in t.tail_first..t.sh_info {
        assert!(o.filler(i), "{label}: tail entry {i} is not a filler");
    }
    j.tail = t.tail_capacity;
    // File 0's rows: the global area, then the internal file's block
    // (WP-264; none when it has no entry).
    let rows: Vec<(FileId, SymtabBlock)> = m.symtab.clone();
    let zero: Vec<SymtabBlock> = rows.iter().filter(|(f, _)| f.0 == 0).map(|r| r.1).collect();
    assert!(matches!(zero.len(), 1 | 2), "{label}: file 0 has {} rows", zero.len());
    let (area, internal) = (zero[0], zero.get(1).copied());
    assert_eq!(
        (area.first, area.count, area.capacity),
        (o.sh_info, entries - o.sh_info, entries - o.sh_info),
        "{label}: the global area"
    );
    let global_names = (o.sh_info..entries)
        .map(|i| (o.syms[i as usize].st_name, &o.names[i as usize]))
        .filter(|(_, n)| !fixed(n));
    let lowest = global_names.map(|(s, _)| u64::from(s)).min().expect("a global name");
    assert_eq!(area.strtab_offset, lowest, "{label}: the global area's names start");
    assert_eq!(
        area.strtab_offset + area.strtab_len,
        t.strtab_size,
        "{label}: the global names end .strtab"
    );
    // The file blocks, in FILES order.
    let mut blocks: Vec<(FileId, SymtabBlock)> =
        rows.iter().copied().filter(|(f, _)| f.0 != 0).collect();
    blocks.sort_by_key(|(f, _)| *f);
    let mut starts: Vec<u32> = Vec::new();
    let mut at = blocks.first().map_or(t.tail_first, |(_, b)| b.first);
    for i in 1..at {
        assert!(
            o.syms[i as usize].sym_type() != STT_FILE,
            "{label}: an STT_FILE in the internal block, {i}"
        );
    }
    // The internal block (WP-264): entry 1 on, its entries then its
    // fillers up to the first file's block; its names in its region.
    match internal {
        None => assert_eq!(at, 1, "{label}: entries before the first file's block, no row"),
        Some(b) => {
            let who = format!("{label}: the internal block {b:?}");
            assert_eq!(b.first, 1, "{who}: its first entry");
            assert_eq!(b.first + b.capacity, at, "{who}: it ends where the first file's starts");
            assert_eq!(b.capacity, b.count + (b.count / 8).max(2), "{who}: capacity (IN-49a)");
            let mut names_len = 0u64;
            for i in b.first..b.first + b.count {
                assert!(!o.filler(i), "{who}: entry {i} is a filler");
                let name = &o.names[i as usize];
                if !fixed(name) {
                    let off = u64::from(o.syms[i as usize].st_name);
                    assert!(
                        off >= b.strtab_offset
                            && off + (name.len() as u64) < b.strtab_offset + b.strtab_len,
                        "{who}: entry {i}'s name at {off}, outside its region"
                    );
                    names_len += name.len() as u64 + 1;
                }
            }
            for i in b.first + b.count..at {
                assert!(o.filler(i), "{who}: entry {i} of its slack is not a filler");
            }
            assert_eq!(names_len, b.strtab_len, "{who}: its region against its entries' names");
            let slack = &o.strtab[(b.strtab_offset + b.strtab_len) as usize
                ..(b.strtab_offset + b.strtab_capacity) as usize];
            assert!(slack.iter().all(|&c| c == 0), "{who}: its region's slack is not zeros");
            j.internal = b.count;
        }
    }
    for &(file, b) in &blocks {
        let who = format!(
            "{label}: file {} {}",
            file.0,
            String::from_utf8_lossy(text(m, m.files[file.0 as usize].path))
        );
        assert_eq!(b.first, at, "{who}: its block does not start where the one before ends");
        assert_eq!(b.capacity, b.count + (b.count / 8).max(2), "{who}: capacity (IN-49a)");
        let (files, mut own) = input_names(m, file.0 as usize, dir);
        let mut names_len = 0u64;
        for i in b.first..b.first + b.count {
            assert!(!o.filler(i), "{who}: entry {i} is a filler");
            let s = &o.syms[i as usize];
            let name = &o.names[i as usize];
            let shown = String::from_utf8_lossy(name);
            if s.sym_type() == STT_FILE {
                assert!(
                    files.contains(name),
                    "{who}: entry {i} STT_FILE {shown} is another file's"
                );
            }
            let demoted = matches!(s.visibility(), 1 | 2);
            let mut take = |global: bool| {
                own.get_mut(&(name.clone(), global)).filter(|n| **n > 0).map(|n| *n -= 1)
            };
            let found = take(false).or_else(|| if demoted { take(true) } else { None });
            assert!(
                found.is_some(),
                "{who}: entry {i} {shown} (bind {} vis {} shndx {}) is no local of the file; \
                 block {b:?}, sh_info {}",
                s.binding(),
                s.visibility(),
                s.st_shndx,
                o.sh_info
            );
            if !fixed(name) {
                let off = u64::from(s.st_name);
                assert!(
                    off >= b.strtab_offset
                        && off + (name.len() as u64) < b.strtab_offset + b.strtab_len,
                    "{who}: entry {i} {shown}'s name at {off}, outside its region"
                );
                names_len += name.len() as u64 + 1;
            }
        }
        if let Some(f) = files.first() {
            let s = &o.syms[b.first as usize];
            assert!(
                s.sym_type() == STT_FILE && &o.names[b.first as usize] == f,
                "{who}: its first entry is not its STT_FILE"
            );
        }
        assert_eq!(names_len, b.strtab_len, "{who}: its region's bytes against its entries' names");
        assert_eq!(
            b.strtab_capacity,
            b.strtab_len + (b.strtab_len / 8).max(16),
            "{who}: its region's capacity (IN-49a)"
        );
        let slack = &o.strtab[(b.strtab_offset + b.strtab_len) as usize
            ..(b.strtab_offset + b.strtab_capacity) as usize];
        assert!(slack.iter().all(|&c| c == 0), "{who}: its region's slack is not zeros");
        for i in b.first + b.count..b.first + b.capacity {
            assert!(o.filler(i), "{who}: entry {i} of its slack is not a filler");
            j.fillers += 1;
        }
        starts.push(b.first);
        at = b.first + b.capacity;
        j.blocks += 1;
        j.entries += b.count;
    }
    assert_eq!(at, t.tail_first, "{label}: the blocks end where the tail starts");
    for i in 1..t.tail_first {
        if o.syms[i as usize].sym_type() == STT_FILE {
            assert!(
                starts.binary_search(&i).is_ok(),
                "{label}: STT_FILE entry {i} {} starts no stored block",
                String::from_utf8_lossy(&o.names[i as usize])
            );
        }
    }
    j
}

#[test]
fn stored_blocks_are_the_output_blocks() {
    let mut ran = 0;
    for line in lines(false) {
        let label = line.label;
        let (m, _) = state(&line);
        let j = judge(label, &m, &line.dir);
        let t = m.meta.symtab;
        eprintln!(
            "[wp263] {label}: {} blocks, {} entries and {} fillers; internal block {} entries; \
             tail [{}, +{}), sh_info {}; .symtab {} (+{}), .strtab {} (+{}); global area {:?}",
            j.blocks,
            j.entries,
            j.fillers,
            j.internal,
            t.tail_first,
            j.tail,
            t.sh_info,
            t.symtab_size,
            t.symtab_slack,
            t.strtab_size,
            t.strtab_slack,
            m.symtab.iter().find(|(f, _)| f.0 == 0).map(|r| r.1)
        );
        assert!(j.internal > 0, "{label}: the internal block has a row");
        ran += 1;
    }
    assert!(ran == 2 || !tools_required(), "{ran} of 2 links");
}

#[test]
fn orphan_gnu_warning_is_counted() {
    let mut ran = 0;
    for (orphan, want) in [(false, 0), (true, 1)] {
        for line in lines(orphan) {
            let (m, _) = state(&line);
            assert_eq!(
                m.meta.gnu_warning_orphans, want,
                "{}: META.gnu_warning_orphans",
                line.label
            );
            if orphan {
                // The unit is live and the section is no key's.
                let named = m.symbols.iter().any(|s| text(&m, s.name) == b"nosuchkey");
                assert!(!named, "{}: nosuchkey has a SYMBOLS row", line.label);
                judge(line.label, &m, &line.dir);
            }
            eprintln!(
                "[wp263] {} with{} x4.cpp: gnu_warning_orphans {want}",
                line.label,
                if orphan { "" } else { "out" }
            );
            ran += 1;
        }
    }
    assert!(ran == 4 || !tools_required(), "{ran} of 4 links");
}

#[test]
fn version_10_state_is_refused() {
    let mut ran = 0;
    for line in lines(false) {
        let label = line.label;
        let (_, mut bytes) = state(&line);
        assert_eq!(&bytes[..8], b"MORTARSR", "{label}: format 5's magic");
        assert_eq!(
            bytes[8..12],
            mortar_incr::state5::VERSION.to_le_bytes(),
            "{label}: the state's version"
        );
        bytes[8..12].copy_from_slice(&10u32.to_le_bytes());
        let e = mortar_incr::state5::read::decode(&bytes).expect_err("a version-10 state decodes");
        let shown = format!("{e:?}");
        assert!(
            shown.contains("Version") && shown.contains("format version 10"),
            "{label}: {shown}"
        );
        let at = line.dir.join("out.mortar-state");
        let mut disk = std::fs::read(&at).expect("the state");
        assert_eq!(&disk[..8], b"MORTARSR", "{label}: the written state's magic");
        disk[8..12].copy_from_slice(&10u32.to_le_bytes());
        std::fs::write(&at, disk).expect("the state");
        let args = incremental_args(&line, "--incremental=update");
        let second = mortar::invoke_in("mortar", &args, &line.dir);
        let report = format!("{:?}\n{}", second.outcome, second.incremental);
        assert!(report.contains("state-version"), "{label}: {report}");
        eprintln!("[wp263] {label}: a version-10 state: state-version");
        ran += 1;
    }
    assert!(ran == 2 || !tools_required(), "{ran} of 2 links");
}
