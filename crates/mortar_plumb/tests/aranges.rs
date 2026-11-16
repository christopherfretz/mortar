//! `spec/image.md` I-112 (the `.debug_aranges` length after a tombstone)
//! as the verifier accepts it (`spec/plumb.md` §5.8), on i386 and x86-64:
//! lld's `--gc-sections` link of `x86/aranges.s` (assembled twice; the
//! second copy of a COMDAT group is dropped), read with its non-alloc rows
//! as mortar's `nplace` records and with the bytes written as I-47 and
//! I-112 say, is clean; each length written another way is a finding.
//! WP-030d: the acceptance was ELFCLASS64 only, so mortar's i386 output,
//! written as I-112 says, was an S12 finding.

mod common;

use common::verify_with;
use mortar_elf::consts::SHF_ALLOC;
use mortar_elf::{FileHeader, SectionTable};
use mortar_plumb::map::{NonAlloc, SectionRef};
use mortar_plumb::{Format, Map};
use mortar_testutil::{Scratch, run, tool};

const ARANGES_S: &str = include_str!("x86/aranges.s");

/// One machine's link: the output, its inputs, the map read as mortar's,
/// the address width, and the file offsets of the two inputs'
/// `.debug_aranges` in the output.
struct Case {
    output: Vec<u8>,
    files: Vec<(String, Vec<u8>)>,
    map: Map,
    w: usize,
    at: [usize; 2],
}

/// Links `x86/aranges.s` for `triple` with lld; `None` when a tool is
/// missing (a failure under `MORTAR_REQUIRE_TOOLS`).
fn case(triple: &str, w64: bool) -> Option<Case> {
    let tools = (tool(&format!("{triple}-as")), tool("ld.lld-22"));
    let (Some(asm), Some(lld)) = tools else {
        assert!(!mortar_testutil::tools_required(), "{triple}-as and ld.lld-22 are required");
        return None;
    };
    let scratch = Scratch::new(&format!("plumb-aranges-{triple}"));
    let src = scratch.write("aranges.s", ARANGES_S);
    let dir = scratch.path();
    let mut def = Vec::new();
    if w64 {
        def.extend(["--defsym", "W64=1"]);
    }
    for (obj, first) in [("a.o", true), ("b.o", false)] {
        let mut args: Vec<std::ffi::OsString> = def.iter().map(Into::into).collect();
        if first {
            args.extend(["--defsym".into(), "FIRST=1".into()]);
        }
        args.extend([src.clone().into_os_string(), "-o".into(), dir.join(obj).into_os_string()]);
        let o = run(&asm, &args);
        assert!(o.success(), "{triple}-as: {}", o.stderr_text());
    }
    let (out, map) = (dir.join("out"), dir.join("out.map"));
    let args: Vec<std::ffi::OsString> = vec![
        "-static".into(),
        "-e".into(),
        "_start".into(),
        "--gc-sections".into(),
        "-Map".into(),
        map.clone().into_os_string(),
        dir.join("a.o").into_os_string(),
        dir.join("b.o").into_os_string(),
        "-o".into(),
        out.clone().into_os_string(),
    ];
    let o = run(&lld, &args);
    assert!(o.success(), "ld.lld-22: {}", o.stderr_text());
    let output = std::fs::read(&out).unwrap();
    let parsed = mortar_plumb::map::parse(Format::Lld, &std::fs::read_to_string(&map).unwrap())
        .unwrap_or_else(|e| panic!("{triple}: the map: line {}: {}", e.line, e.message));
    let files: Vec<(String, Vec<u8>)> = parsed
        .paths()
        .into_iter()
        .filter_map(|p| std::fs::read(p).ok().map(|b| (p.to_string(), b)))
        .collect();
    // The output's sections: (name, flags, address, file offset).
    let h = FileHeader::parse(&output).unwrap();
    let secs: Vec<(String, u64, u64, u64)> = SectionTable::parse(&output, &h)
        .unwrap()
        .iter()
        .map(|s| {
            let sh = s.header();
            let name = String::from_utf8_lossy(s.name().unwrap_or(b"")).into_owned();
            (name, sh.sh_flags, sh.sh_addr, sh.sh_offset)
        })
        .collect();
    let nonalloc_out = |n: &str| secs.iter().find(|x| x.0 == n && x.1 & SHF_ALLOC == 0);
    // lld's rows of non-alloc sections become mortar's `nplace` records
    // (plumb.md §3.2), as in `mutation.rs` §9.2 (j).
    let mut base = parsed.clone();
    let (nonalloc, placed): (Vec<_>, Vec<_>) = base
        .placed
        .drain(..)
        .partition(|p| matches!(&p.section, SectionRef::Name(n) if nonalloc_out(n).is_some()));
    base.placed = placed;
    let mut at = Vec::new();
    for p in nonalloc {
        let SectionRef::Name(n) = &p.section else { unreachable!() };
        let o = nonalloc_out(n).unwrap();
        let offset = p.addr - o.2;
        if n == ".debug_aranges" {
            at.push((o.3 + offset) as usize);
        }
        base.nonalloc.push(NonAlloc {
            file: p.file,
            section: p.section.clone(),
            out: n.clone(),
            offset: Some(offset),
            size: p.size,
            line: 0,
        });
    }
    at.sort_unstable();
    assert_eq!(at.len(), 2, "{triple}: the two inputs' .debug_aranges");
    let map = mortar_plumb::map::parse(
        Format::Mortar,
        &mortar_plumb::map::mortar::write(&with_line(&base)),
    )
    .unwrap_or_else(|e| panic!("{triple}: mortar's map: line {}: {}", e.line, e.message));
    Some(Case { output, files, map, w: if w64 { 8 } else { 4 }, at: [at[0], at[1]] })
}

impl Case {
    /// The output written as mortar writes it (I-47, I-112): in each
    /// input's tuples (at 16, 2w apart; `x86/aranges.s`), tuple 1's
    /// address (a collected target) all ones and its length 0; b.o's
    /// tuple 2's address (its dropped group) all ones, the length kept;
    /// tuple 3's address all ones, the record after it as relocated.
    fn mortar(&self) -> Vec<u8> {
        let mut m = self.output.clone();
        let w = self.w;
        for (k, &a) in self.at.iter().enumerate() {
            let t = |n: usize| a + 16 + 2 * w * n;
            m[t(0)..t(0) + w].fill(0xff);
            m[t(0) + w..t(0) + 2 * w].fill(0);
            if k == 1 {
                m[t(1)..t(1) + w].fill(0xff);
            }
            m[t(2)..t(2) + w].fill(0xff);
        }
        m
    }

    /// The verifier's content and debug-reference findings (R1, R5, S12)
    /// on `out`, read as mortar's link.
    fn judged(&self, out: &[u8]) -> Vec<String> {
        verify_with(out, &self.files, &self.map)
            .findings
            .into_iter()
            .filter(|f| matches!(f.rule.id().as_str(), "R1" | "R5" | "S12"))
            .map(|f| format!("{} {} {:?} {:?}", f.rule.id(), f.message, f.expected, f.found))
            .collect()
    }

    /// The three rulings on one machine.
    fn check(&self, label: &str) {
        let good = self.mortar();
        let clean = self.judged(&good);
        assert!(clean.is_empty(), "{label}: I-47 and I-112 written are not clean: {clean:#?}");
        let w = self.w;
        for (k, &a) in self.at.iter().enumerate() {
            let t = |n: usize| a + 16 + 2 * w * n;
            // A length kept (the input's 9), and one other value.
            for v in [9u8, 1] {
                let mut m = good.clone();
                m[t(0) + w] = v;
                let r = self.judged(&m);
                assert!(
                    r.iter().any(|f| f.contains("I-112")),
                    "{label}: input {k}: an aranges length {v} after the tombstone is not found: {r:#?}"
                );
            }
            // A target that is not `SHF_ALLOC` (b.o's dropped group): the
            // length is the input's; 0 there is S12.
            if k == 1 {
                let mut m = good.clone();
                m[t(1) + w..t(1) + 2 * w].fill(0);
                let r = self.judged(&m);
                assert!(
                    r.iter().any(|f| f.starts_with("S12")),
                    "{label}: a zero length after a tombstone of a target not SHF_ALLOC is not S12: {r:#?}"
                );
            }
            // A record's field right after the field: 0 there is a
            // finding (the record's own value is `_start`).
            let mut m = good.clone();
            m[t(2) + w..t(2) + 2 * w].fill(0);
            let r = self.judged(&m);
            assert!(
                !r.is_empty(),
                "{label}: input {k}: 0 over the record after an aranges tombstone is not found"
            );
        }
    }
}

// spec: image.md I-112; plumb.md §5.8 (I-112, mortar only)
#[test]
fn i112_aranges_length_on_i386() {
    if let Some(c) = case("i686-linux-gnu", false) {
        c.check("i386");
    }
}

// spec: image.md I-112; plumb.md §5.8 (I-112, mortar only)
#[test]
fn i112_aranges_length_on_x86_64() {
    if let Some(c) = case("x86_64-linux-gnu", true) {
        c.check("x86-64");
    }
}

/// `map` with a line (`spec/plumb.md` §3.2a: every mortar map states its
/// link's words, and mortar's reader refuses one without). A reference's
/// map has none: these links gave none of the options §3.2a's rules read,
/// so the map's input paths stand for the words.
fn with_line(map: &Map) -> Map {
    let mut m = map.clone();
    if m.args.is_empty() {
        m.args = m.paths().iter().map(|p| p.as_bytes().to_vec()).collect();
        if m.args.is_empty() {
            m.args.push(b"a.o".to_vec());
        }
    }
    m
}
