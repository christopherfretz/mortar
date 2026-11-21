//! §6.8 P-I8 (WP-210, V-3): the decision records of an incremental-mode
//! map judged against the output (`spec/plumb.md` §3.2; `spec/image.md`
//! I-78a).
//!
//! The link is this build's mortar (`tests/built`), static and
//! `-nostdlib`, of two objects clang makes here with `-fPIC` (AArch64 and
//! x86-64): GOT references to a weak undefined name, to data and to an
//! IFUNC, an IFUNC call (an IPLT entry), an initial-exec TLS reference (a
//! `got-tprel` slot on AArch64) and merged strings (`frag` records). Its
//! own map is ACCEPTED, every slot judged; each mutation of the map (or
//! of a word of the output) is REJECTED by P-I8 alone, with its message.
//! Before WP-210 every mutation passed: the records were read, not
//! judged.

use std::path::{Path, PathBuf};

use mortar_plumb::{Ctx, Files, Format, Input, Map, Report};
use mortar_testutil::target::Arch;
use mortar_testutil::{Scratch, run_with, tool};

mod built;

const P_C: &str = r#"
extern int wk __attribute__((weak));
extern int shared_g;
int ifn(void);
extern __thread int tv __attribute__((tls_model("initial-exec")));
static const char *const names[] = {"alpha", "beta", "gamma"};
volatile int sink;
const char *pick(int i) { return names[i % 3]; }
int use_wk(void) { return &wk ? wk : 0; }
int *addr_g(void) { return &shared_g; }
int tls(void) { return tv; }
int (*fp(void))(void) { return ifn; }
void _start(void) {
  for (;;)
    sink = ifn() + use_wk() + tls() + pick(sink)[0] + *addr_g() + (fp() != 0);
}
"#;

const B_C: &str = r#"
int shared_g = 5;
static int impl1(void) { return 1; }
static int (*resolve(void))(void) { return impl1; }
int ifn(void) __attribute__((ifunc("resolve")));
__thread int tv = 7;
const char *other(void) { return "beta"; }
"#;

const ARCHS: [Arch; 2] = [Arch::Aarch64, Arch::X86_64];

struct Dir<'a>(&'a Path);

impl Files for Dir<'_> {
    fn read(&self, path: &str) -> Option<Vec<u8>> {
        std::fs::read(self.0.join(path)).ok()
    }
}

/// The link of one architecture: its scratch, output and map text.
struct Link {
    s: Scratch,
    out: Vec<u8>,
    map: String,
}

fn link(arch: Arch, mortar: &Path, clang: &Path) -> Link {
    let s = Scratch::new("decide");
    for (n, src) in [("p", P_C), ("b", B_C)] {
        let c = s.write(&format!("{n}.c"), src);
        let mut cmd = std::process::Command::new(clang);
        cmd.args(arch.clang_args())
            .args(["-O1", "-fPIC", "-fno-asynchronous-unwind-tables", "-fno-stack-protector", "-c"])
            .arg(&c)
            .arg("-o")
            .arg(s.join(format!("{n}.o")));
        let r = run_with(&mut cmd, b"", mortar_testutil::DEFAULT_TIMEOUT);
        assert!(r.success(), "{arch:?} clang {n}.c: {}", r.stderr_text());
    }
    let mut cmd = std::process::Command::new(mortar);
    cmd.current_dir(s.path()).args([
        "-static",
        "-o",
        "x",
        "p.o",
        "b.o",
        "--incremental=full",
        "--incremental-state=x.state",
        "--mortar-map=x.map",
    ]);
    let r = run_with(&mut cmd, b"", mortar_testutil::DEFAULT_TIMEOUT);
    assert!(r.success(), "{arch:?} mortar: {}", r.stderr_text());
    let out = std::fs::read(s.join("x")).unwrap();
    let map = std::fs::read_to_string(s.join("x.map")).unwrap();
    Link { s, out, map }
}

fn verify(l: &Link, out: &[u8], text: &str) -> Report {
    let map: Map = mortar_plumb::map::parse(Format::Mortar, text)
        .unwrap_or_else(|e| panic!("map line {}: {}", e.line, e.message));
    let files: Vec<(String, Vec<u8>)> = map
        .paths()
        .into_iter()
        .map(|p| (p.to_string(), std::fs::read(l.s.join(p)).unwrap()))
        .collect();
    let inputs: Vec<Input<'_>> = files.iter().map(|(p, b)| Input { path: p, bytes: b }).collect();
    mortar_plumb::verify_with(out, &inputs, &map, &Ctx { line: None, files: &Dir(l.s.path()) })
}

fn lines(r: &Report) -> Vec<String> {
    r.findings.iter().map(|f| format!("{} {}", f.rule.id(), f.message)).collect()
}

/// The links, or `None` (a skip; a FAIL under `MORTAR_REQUIRE_TOOLS=1`)
/// without clang or this build's mortar.
fn links() -> Option<Vec<(Arch, Link)>> {
    let clang = tool("clang")?;
    let mortar: PathBuf = built::mortar_bin("plumb-decide/mortar")?;
    Some(ARCHS.iter().map(|&a| (a, link(a, &mortar, &clang))).collect())
}

/// The map's record `<word> <rest...>` whose fields after the word start
/// with `head`: its line.
fn record<'a>(text: &'a str, word: &str, head: &str) -> &'a str {
    let p = format!("{word} {head}");
    text.lines()
        .find(|l| l.starts_with(&p))
        .unwrap_or_else(|| panic!("no record '{p}' in the map:\n{text}"))
}

fn hex(s: &str) -> u64 {
    u64::from_str_radix(s, 16).unwrap()
}

/// The last field of a record, hex.
fn addr_of(line: &str) -> u64 {
    hex(line.rsplit(' ').next().unwrap())
}

/// `line` with its last field replaced by `v` (hex).
fn with_addr(line: &str, v: u64) -> String {
    let (head, _) = line.rsplit_once(' ').unwrap();
    format!("{head} {v:x}")
}

fn replace(text: &str, from: &str, to: &str) -> String {
    assert!(text.contains(from), "'{from}' not in the map");
    text.replacen(&format!("{from}\n"), &format!("{to}\n"), 1)
}

/// `text` verified against `out`: only P-I8 findings, one at least
/// containing each needle.
fn rejected(arch: Arch, l: &Link, out: &[u8], text: &str, what: &str, needles: &[&str]) {
    let got = lines(&verify(l, out, text));
    assert!(
        !got.is_empty()
            && got.iter().all(|f| f.starts_with("P-I8 "))
            && needles.iter().all(|n| got.iter().any(|f| f.contains(n))),
        "{arch:?} {what}: want only P-I8 with {needles:?}, got {got:#?}"
    );
}

/// The section `name`'s (address, file offset, size).
fn section(out: &[u8], name: &str) -> (u64, usize, usize) {
    let h = mortar_elf::FileHeader::parse(out).unwrap();
    let t = mortar_elf::SectionTable::parse(out, &h).unwrap();
    let s = t.iter().find(|s| s.name().unwrap_or(b"") == name.as_bytes()).unwrap();
    let x = s.header();
    (x.sh_addr, usize::try_from(x.sh_offset).unwrap(), usize::try_from(x.sh_size).unwrap())
}

/// The offset in `p.o`'s placed `.text` of a branch that reaches `to`
/// (AArch64 `BL`, x86-64 `call rel32`: its rel32 field), and that
/// section's `place` record.
fn branch_to(arch: Arch, l: &Link, to: u64) -> (u64, String) {
    let (ta, to_off, tz) = section(&l.out, ".text");
    // p.o is file 0; its `.text` is the first `place 0` record in .text.
    let place = l
        .map
        .lines()
        .filter(|r| r.starts_with("place 0 "))
        .find(|r| {
            let a = hex(r.split(' ').nth(3).unwrap());
            (ta..ta + tz as u64).contains(&a)
        })
        .unwrap()
        .to_string();
    let f: Vec<&str> = place.split(' ').collect();
    let (pa, pz) = (hex(f[3]), hex(f[4]));
    let b = &l.out[to_off + (pa - ta) as usize..to_off + (pa - ta + pz) as usize];
    let hit = match arch {
        Arch::Aarch64 => (0..b.len() / 4).map(|k| k * 4).find(|&k| {
            let w = u32::from_le_bytes(b[k..k + 4].try_into().unwrap());
            let imm = i64::from(((w & 0x03ff_ffff) << 6) as i32 >> 6) * 4;
            w & 0xfc00_0000 == 0x9400_0000 && (pa + k as u64).wrapping_add_signed(imm) == to
        }),
        _ => (1..b.len().saturating_sub(4)).find(|&k| {
            let v = i32::from_le_bytes(b[k..k + 4].try_into().unwrap());
            b[k - 1] == 0xe8 && (pa + k as u64 + 4).wrapping_add_signed(i64::from(v)) == to
        }),
    };
    (hit.expect("a call of ifn's IPLT entry") as u64, format!("0 {}", f[2]))
}

// The map mortar writes is accepted, every slot judged (by the live
// reference verified through it, by its word, or as an IPLT entry).
#[test]
fn p_i8_mortars_records_are_accepted() {
    let Some(ls) = links() else { return };
    for (arch, l) in &ls {
        let r = verify(l, &l.out, &l.map);
        assert!(r.findings.is_empty(), "{arch:?}: {:#?}", lines(&r));
        let d = r.coverage.decisions.clone().unwrap_or_default();
        let slots = l.map.lines().filter(|x| x.starts_with("slot ")).count();
        let frags = l.map.lines().filter(|x| x.starts_with("frag ")).count();
        assert!(
            d.contains(&format!(
                "slot {slots} ({slots} by a live reference, their word or their entry, 0 by place and key only), redirect 0, frag {frags}"
            )) && slots >= 5
                && frags >= 3,
            "{arch:?}: {d}"
        );
    }
}

// Each slot sentence: a record moved to another key's slot, a slot used
// and not stated, an IPLT record off its entry, two keys at one slot,
// a slot whose word is not its key's.
#[test]
fn p_i8_slots_against_the_output() {
    let Some(ls) = links() else { return };
    for (arch, l) in &ls {
        let m = &l.map;
        let wk = record(m, "slot", "got wk ");
        let ifn = record(m, "slot", "got ifn ");
        // Swapped: the references through either slot use the other's.
        let t = replace(m, wk, &with_addr(wk, addr_of(ifn)));
        let t = replace(&t, ifn, &with_addr(ifn, addr_of(wk)));
        rejected(
            *arch,
            l,
            &l.out,
            &t,
            "wk and ifn swapped",
            &["slot got 'wk' at", "a live reference of"],
        );
        // Gone: wk's slot is used and no record states it.
        let t = replace(m, wk, "");
        rejected(
            *arch,
            l,
            &l.out,
            &t.replacen("\n\n", "\n", 1),
            "wk's record gone",
            &["a live reference uses the got 'wk' slot, and no slot record states it"],
        );
        // An IPLT record off its entry.
        let ip = record(m, "slot", "iplt ifn ");
        let t = replace(m, ip, &with_addr(ip, addr_of(ip) + 4));
        rejected(*arch, l, &l.out, &t, "iplt moved", &["is not an IPLT entry of the resolver"]);
        // Two keys at one slot.
        let sink = record(m, "slot", "got sink ");
        let t = replace(m, sink, &with_addr(sink, addr_of(ifn)));
        rejected(*arch, l, &l.out, &t, "sink at ifn's slot", &["one key per slot"]);
        // A slot outside the GOT.
        let t = replace(m, sink, &with_addr(sink, section(&l.out, ".text").0));
        rejected(
            *arch,
            l,
            &l.out,
            &t,
            "sink in .text",
            &["is not 8 bytes of a GOT section (in .text)"],
        );
        // The word of shared_g's slot: on x86-64 its references are
        // relaxed (no live reference uses it), so its word must be the
        // symbol's value; on AArch64 the reference verified through it.
        let g = record(m, "slot", "got shared_g ");
        let (ga, go, _) = section(&l.out, ".got");
        let at = go + (addr_of(g) - ga) as usize;
        let mut out = l.out.clone();
        out[at] ^= 0x10;
        let want: &[&str] = match arch {
            Arch::X86_64 => &["slot got 'shared_g' at", "its word"],
            _ => &[],
        };
        if want.is_empty() {
            // Not P-I8's: the reference's own rule (§5.5) sees the word.
            let got = lines(&verify(l, &out, m));
            assert!(!got.is_empty(), "{arch:?}: shared_g's word changed: no finding");
        } else {
            rejected(*arch, l, &out, m, "shared_g's word", want);
        }
    }
}

// The `got-tprel` slot (AArch64: the initial-exec reference is verified
// through it) moved onto the next word.
#[test]
fn p_i8_tprel_slot() {
    let Some(ls) = links() else { return };
    for (arch, l) in &ls {
        let Some(tp) = l.map.lines().find(|x| x.starts_with("slot got-tprel tv ")) else {
            assert_eq!(*arch, Arch::X86_64, "{arch:?}: no got-tprel slot");
            continue;
        };
        let t = replace(&l.map, tp, &with_addr(tp, addr_of(tp) + 0x100));
        rejected(*arch, l, &l.out, &t, "tv's slot moved", &["slot got-tprel 'tv' at"]);
    }
}

// A redirect at the call of ifn: to an address no `chunk thunk` or
// `chunk patch` names, and (with one forged at the IPLT entry the call
// reaches) accepted; the site moved off the branch is rejected.
#[test]
fn p_i8_redirects() {
    let Some(ls) = links() else { return };
    for (arch, l) in &ls {
        let ip = addr_of(record(&l.map, "slot", "iplt ifn "));
        let (site, sec) = branch_to(*arch, l, ip);
        let red = format!("redirect {sec} {site:x} {ip:x}");
        // After the last slot record (redirects follow the slots, §3.2).
        let last = l.map.lines().rfind(|x| x.starts_with("slot ")).unwrap();
        let t = replace(&l.map, last, &format!("{last}\n{red}"));
        rejected(
            *arch,
            l,
            &l.out,
            &t,
            "redirect to no patch",
            &["is no chunk thunk or chunk patch record's address"],
        );
        let chunk = format!("chunk patch .text {ip:x} 4");
        let first_chunk = l.map.lines().find(|x| x.starts_with("chunk ")).unwrap();
        let ok = replace(&t, first_chunk, &format!("{chunk}\n{first_chunk}"));
        let got: Vec<String> =
            lines(&verify(l, &l.out, &ok)).into_iter().filter(|f| f.starts_with("P-I8 ")).collect();
        assert!(got.is_empty(), "{arch:?}: a redirect that reaches its patch: {got:#?}");
        let moved = ok.replace(&red, &format!("redirect {sec} {:x} {ip:x}", site + 4));
        let got: Vec<String> = lines(&verify(l, &l.out, &moved))
            .into_iter()
            .filter(|f| f.starts_with("P-I8 "))
            .collect();
        assert!(
            got.len() == 1 && got[0].contains("not the patch at"),
            "{arch:?}: a redirect whose site is no branch to its patch: {got:#?}"
        );
    }
}

// `frag`: a wrong hash, an offset off its string, a section whose
// merged chunk is not one (its capacity record `b0`).
#[test]
fn p_i8_frags() {
    let Some(ls) = links() else { return };
    for (arch, l) in &ls {
        let m = &l.map;
        let f = record(m, "frag", ".rodata ");
        let mut v: Vec<&str> = f.split(' ').collect();
        let hash = v[3].to_string();
        let flipped = format!("{}{}", if hash.starts_with('0') { "1" } else { "0" }, &hash[1..]);
        v[3] = &flipped;
        rejected(*arch, l, &l.out, &replace(m, f, &v.join(" ")), "frag hash", &["L128 of its"]);
        let t = replace(m, f, &with_addr(f, addr_of(f) + 1));
        rejected(*arch, l, &l.out, &t, "frag offset", &["L128 of its"]);
        let cap = record(m, "capacity", ".rodata 0 m ");
        let t = replace(m, cap, &cap.replacen(" 0 m ", " 0 b0 ", 1));
        rejected(
            *arch,
            l,
            &l.out,
            &t,
            "no merged chunk",
            &["it lies in no merged chunk of the section"],
        );
    }
}
