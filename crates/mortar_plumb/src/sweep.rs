//! The byte sweep of a dynamic output's synthetic sections (WP-082,
//! `spec/plumb.md` §12.2): every byte of `.plt`, `.plt.sec`, `.plt.got`,
//! `.iplt`, `.got`, `.got.plt`, `.dynamic`, `.dynsym`, `.dynstr`,
//! `.gnu.hash`, `.gnu.version`, `.gnu.version_r`, `.rela.dyn`,
//! `.rela.plt`, `.interp`, `.eh_frame_hdr`, `.gdb_index` (WP-098) and of
//! the `.eh_frame` records that cover a PLT section is flipped (bit 0, then bit 7), the mutant is
//! judged, and every byte no flip makes a new finding for is a survivor.
//! Survivors are named by field and coalesced into ranges. This measures
//! what the verifier does not judge; it is not a test (a survivor may be
//! a byte no loader reads).

use std::collections::BTreeMap;
use std::fmt::Write as _;

use mortar_elf::consts::SHT_NOBITS;

use crate::dynout::{DT_VERNEEDNUM, DynOut};
use crate::image::Image;
use crate::{Input, Map};

/// The sections the sweep covers, in the order of the record's table.
pub const SECTIONS: &[&str] = &[
    ".interp",
    ".gnu.hash",
    ".dynsym",
    ".dynstr",
    ".gnu.version",
    ".gnu.version_r",
    ".rela.dyn",
    ".rela.plt",
    ".plt",
    ".plt.sec",
    ".plt.got",
    ".iplt",
    ".eh_frame_hdr",
    ".eh_frame",
    ".dynamic",
    ".got",
    ".got.plt",
    ".gdb_index",
    BUILD_ID,
];

/// The build-id note: only its descriptor is swept, never resealed (B1's
/// own case).
pub const BUILD_ID: &str = ".note.gnu.build-id";

/// The bits each byte is flipped by, one mutant each.
pub const FLIPS: [u8; 2] = [0x01, 0x80];

/// One byte's verdict.
#[derive(Debug, Clone)]
pub struct Byte {
    pub section: &'static str,
    /// Offset in the section.
    pub off: u64,
    /// The field's kind (`st_value`, `r_addend`, `entry word 2`, ...).
    pub kind: String,
    /// The field's instance (`entry 3 'puts'`, ...).
    pub instance: String,
    /// Per flip of [`FLIPS`]: whether a new finding caught it.
    pub caught: [bool; 2],
    /// Per flip: the rule ids of the new findings (sorted, once each).
    pub rules: [Vec<String>; 2],
    /// Whether a flip made the verifier panic (a hostile-input bug,
    /// §9.4; counted as caught).
    pub panicked: bool,
}

/// The sweep of one output.
#[derive(Debug, Default)]
pub struct Sweep {
    pub baseline: usize,
    pub bytes: Vec<Byte>,
}

impl Sweep {
    /// Per (section, kind): bytes, bytes surviving some flip, bytes
    /// surviving every flip; then each surviving range.
    #[allow(clippy::many_single_char_names)] // table cells
    pub fn render(&self) -> String {
        let mut s = String::new();
        let mut agg: BTreeMap<(usize, &str, &str), (usize, usize, usize)> = BTreeMap::new();
        for b in &self.bytes {
            let rank = SECTIONS.iter().position(|x| *x == b.section).unwrap_or(99);
            let e = agg.entry((rank, b.section, b.kind.as_str())).or_default();
            e.0 += 1;
            if b.caught.iter().any(|c| !c) {
                e.1 += 1;
            }
            if b.caught.iter().all(|c| !c) {
                e.2 += 1;
            }
        }
        let _ = writeln!(
            s,
            "{:16} {:28} {:>7} {:>9} {:>9}",
            "section", "field", "bytes", "some-flip", "all-flips"
        );
        for ((_, sec, kind), (n, some, all)) in &agg {
            let _ = writeln!(s, "{sec:16} {kind:28} {n:>7} {some:>9} {all:>9}");
        }
        let flips = self.bytes.len() * FLIPS.len();
        let caught = self.bytes.iter().flat_map(|b| b.caught).filter(|c| *c).count();
        let _ = writeln!(s, "flips {flips}, caught {caught}, survived {}", flips - caught);
        for (rule, (any, only)) in self.by_rule() {
            let _ = writeln!(s, "caught by {rule:8} {any:>7} flips ({only} by it alone)");
        }
        for b in self.bytes.iter().filter(|b| b.panicked) {
            let _ = writeln!(s, "panic {} +{:#x} {} {}", b.section, b.off, b.kind, b.instance);
        }
        // Ranges: consecutive surviving bytes of one field instance.
        let mut i = 0;
        while i < self.bytes.len() {
            let b = &self.bytes[i];
            if b.caught.iter().all(|c| *c) {
                i += 1;
                continue;
            }
            let mut j = i + 1;
            while j < self.bytes.len() {
                let n = &self.bytes[j];
                if n.section != b.section
                    || n.off != self.bytes[j - 1].off + 1
                    || n.kind != b.kind
                    || n.instance != b.instance
                    || n.caught.iter().all(|c| *c)
                {
                    break;
                }
                j += 1;
            }
            let flips: Vec<String> = FLIPS
                .iter()
                .enumerate()
                .filter(|&(k, _)| self.bytes[i..j].iter().any(|x| !x.caught[k]))
                .map(|(_, f)| format!("{f:#04x}"))
                .collect();
            let _ = writeln!(
                s,
                "survives {} [{:#x}, {:#x}) {} {} (flip {})",
                b.section,
                b.off,
                self.bytes[j - 1].off + 1,
                b.kind,
                b.instance,
                flips.join(",")
            );
            i = j;
        }
        s
    }
}

impl Sweep {
    /// Per rule id: the flips whose new findings include it, and those
    /// whose only new findings are its.
    pub fn by_rule(&self) -> BTreeMap<&str, (usize, usize)> {
        let mut m: BTreeMap<&str, (usize, usize)> = BTreeMap::new();
        for rules in self.bytes.iter().flat_map(|b| b.rules.iter()) {
            for r in rules {
                let e = m.entry(r.as_str()).or_default();
                e.0 += 1;
                if rules.len() == 1 {
                    e.1 += 1;
                }
            }
        }
        m
    }
}

fn keys(r: &crate::Report) -> std::collections::BTreeSet<(String, Option<u64>, String)> {
    r.findings.iter().map(|f| (f.rule.id(), f.addr, f.message.clone())).collect()
}

/// The names of a `.dynsym`-indexed field.
fn sym_name(out: &DynOut, j: u64) -> String {
    out.syms.get(j as usize).map_or(String::new(), |s| format!(" '{}'", s.name))
}

/// (kind, instance) of the byte at `off` of the section `name` at `addr`.
#[allow(clippy::too_many_lines)] // one arm per section
fn field(
    image: &Image<'_>,
    out: &DynOut,
    name: &str,
    addr: u64,
    size: u64,
    off: u64,
    ran: &std::collections::BTreeSet<u64>,
) -> (String, String) {
    let machine = image.header.e_machine;
    let at = |k: &str| (k.to_string(), String::new());
    match name {
        ".interp" => at("path"),
        ".dynsym" => {
            let (j, f) = (off / 24, off % 24);
            let k = match f {
                0..4 => "st_name",
                4 => "st_info",
                5 => "st_other",
                6..8 => "st_shndx",
                8..16 => "st_value",
                _ => "st_size",
            };
            (k.into(), format!("entry {j}{}", sym_name(out, j)))
        }
        ".dynstr" => {
            // The string holding the byte, by its start.
            let b = image.bytes(addr, size).unwrap_or(&[]);
            let o = off as usize;
            let start = b[..o.min(b.len())].iter().rposition(|&c| c == 0).map_or(0, |p| p + 1);
            let end = b[o.min(b.len())..].iter().position(|&c| c == 0).map_or(b.len(), |p| o + p);
            let text = String::from_utf8_lossy(&b[start..end.max(start)]).into_owned();
            let used = out.syms.iter().any(|s| s.name == text) || out.needed.contains(&text);
            let k = if o == 0 {
                "leading NUL"
            } else if b.get(o) == Some(&0) {
                "NUL"
            } else if used {
                "string (symbol or needed name)"
            } else {
                "string (other)"
            };
            (k.into(), format!("'{text}'"))
        }
        ".gnu.hash" => {
            let Some(h) = &out.gnu_hash else { return at("table") };
            let bloom = 16 + 8 * u64::from(h.bloom_size);
            let buckets = bloom + 4 * u64::from(h.nbuckets);
            if off < 16 {
                at(["nbuckets", "symoffset", "bloom_size", "bloom_shift"][(off / 4) as usize])
            } else if off < bloom {
                ("bloom word".into(), format!("{}", (off - 16) / 8))
            } else if off < buckets {
                ("bucket".into(), format!("{}", (off - bloom) / 4))
            } else {
                let j = (off - buckets) / 4 + u64::from(h.symoffset);
                ("chain word".into(), format!("symbol {j}{}", sym_name(out, j)))
            }
        }
        ".gnu.version" => ("index".into(), format!("entry {}{}", off / 2, sym_name(out, off / 2))),
        ".gnu.version_r" => {
            // Walk the chain: Verneed (16) then its Vernaux (16 each).
            let b = image.bytes(addr, size).unwrap_or(&[]);
            let rd = |o: usize, w: usize| -> u64 {
                b.get(o..o + w).map_or(0, |x| x.iter().rev().fold(0, |a, &c| a << 8 | u64::from(c)))
            };
            let mut vn = 0usize;
            for _ in 0..out.get(DT_VERNEEDNUM).unwrap_or(0).min(256) {
                let o = off as usize;
                if (vn..vn + 16).contains(&o) {
                    let k = ["vn_version", "vn_cnt", "vn_file", "vn_aux", "vn_next"][match o - vn {
                        0..2 => 0,
                        2..4 => 1,
                        4..8 => 2,
                        8..12 => 3,
                        _ => 4,
                    }];
                    return (k.into(), format!("verneed at {vn:#x}"));
                }
                let (cnt, aux, next) =
                    (rd(vn + 2, 2), rd(vn + 8, 4) as usize, rd(vn + 12, 4) as usize);
                let mut a = vn + aux;
                for _ in 0..cnt.min(256) {
                    if (a..a + 16).contains(&o) {
                        let k = ["vna_hash", "vna_flags", "vna_other", "vna_name", "vna_next"]
                            [match o - a {
                                0..4 => 0,
                                4..6 => 1,
                                6..8 => 2,
                                8..12 => 3,
                                _ => 4,
                            }];
                        return (k.into(), format!("vernaux at {a:#x}"));
                    }
                    let n = rd(a + 12, 4) as usize;
                    if n == 0 {
                        break;
                    }
                    a += n;
                }
                if next == 0 {
                    break;
                }
                vn += next;
            }
            at("padding")
        }
        ".rela.dyn" | ".rela.plt" => {
            let (j, f) = (off / 24, off % 24);
            let r = out.rels.iter().find(|r| r.at == addr + 24 * j);
            let ty = r.map_or(String::new(), |r| format!(" {}", rel_name(machine, r.ty)));
            let sym = r.map_or(String::new(), |r| sym_name(out, u64::from(r.sym)));
            let k = match f {
                0..8 => "r_offset",
                8..12 => "r_info type",
                12..16 => "r_info sym",
                _ => "r_addend",
            };
            (format!("{k}{ty}"), format!("record {j}{sym}"))
        }
        ".dynamic" => {
            let (j, f) = (off / 16, off % 16);
            let tag = out
                .entries
                .get(j as usize)
                .map_or("DT_NULL (tail)".to_string(), |e| crate::dynout::tag_name(e.0));
            (
                (if f < 8 { format!("d_tag {tag}") } else { format!("d_val {tag}") }),
                format!("entry {j}"),
            )
        }
        ".got" | ".got.plt" => {
            let j = off / 8;
            let a = addr + 8 * j;
            let rec = out.rels.iter().find(|r| r.offset == a);
            let what = match rec {
                Some(r) => format!("word with {}", rel_name(machine, r.ty)),
                None if name == ".got.plt"
                    && Some(addr) == out.get(crate::dynout::DT_PLTGOT)
                    && j < 3 =>
                {
                    format!("reserved word {j}")
                }
                None if Some(a) == out.get(crate::dynout::DT_PLTGOT) => "word at DT_PLTGOT".into(),
                None => "word without a record".into(),
            };
            (what, format!("word {j}"))
        }
        ".plt" | ".plt.sec" | ".plt.got" | ".iplt" => {
            let unit = if machine == mortar_elf::consts::EM_AARCH64 { 4 } else { 1 };
            let k = if ran.contains(&(addr + off)) { "code" } else { "code no path executes" };
            (k.into(), format!("+{:#x}", off - off % unit))
        }
        ".eh_frame_hdr" => {
            if off < 4 {
                at("header bytes")
            } else if off < 8 {
                at("eh_frame_ptr")
            } else if off < 12 {
                at("fde_count")
            } else {
                let (j, f) = ((off - 12) / 8, (off - 12) % 8);
                (
                    (if f < 4 { "initial location" } else { "FDE address" }).into(),
                    format!("row {j}"),
                )
            }
        }
        ".eh_frame" => at("PLT record"),
        BUILD_ID => at("build id"),
        _ => at("?"),
    }
}

fn rel_name(machine: u16, ty: u32) -> String {
    format!("{:?}", crate::loader::kind(machine, ty))
}

/// The `[start, end)` file ranges of `.eh_frame` records (CIE or FDE)
/// whose FDE covers a PLT section, as section offsets.
fn plt_eh_ranges(image: &Image<'_>, addr: u64, b: &[u8]) -> Vec<(u64, u64)> {
    let plts: Vec<(u64, u64)> = image
        .sections
        .iter()
        .filter(|s| matches!(s.name.as_str(), ".plt" | ".plt.sec" | ".plt.got" | ".iplt"))
        .map(|s| (s.shdr.sh_addr, s.shdr.sh_addr + s.shdr.sh_size))
        .collect();
    let mut v = Vec::new();
    let mut at = 0usize;
    while at + 8 <= b.len() {
        let len = u32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]]) as usize;
        if len == 0 || len == 0xffff_ffff || at + 4 + len > b.len() {
            break;
        }
        let id = u32::from_le_bytes([b[at + 4], b[at + 5], b[at + 6], b[at + 7]]);
        // An FDE with a pc-relative sdata4 initial location (the encoding
        // every linker's PLT FDE uses).
        if id != 0 && at + 12 <= b.len() {
            let rel = i64::from(i32::from_le_bytes([b[at + 8], b[at + 9], b[at + 10], b[at + 11]]));
            let pc = (addr + at as u64 + 8).wrapping_add_signed(rel);
            if plts.iter().any(|&(lo, hi)| (lo..hi).contains(&pc)) {
                v.push((at as u64, (at + 4 + len) as u64));
            }
        }
        at += 4 + len;
    }
    v
}

/// How a sweep runs.
#[derive(Debug, Clone, Copy)]
pub struct Options<'s> {
    pub threads: usize,
    /// In a section larger than twice this many bytes only the first and
    /// last `window` bytes are flipped; 0: every byte.
    pub window: u64,
    /// The sections swept (each one of [`SECTIONS`]).
    pub sections: &'s [&'s str],
    /// Whether mortar's build id is resealed after each flip
    /// ([`crate::buildid::reseal`]), so that the mutant is the output of
    /// a linker that wrote the flipped byte (§12.3). A flip inside the id
    /// itself is never resealed: the id is then the mutant. `false` only
    /// to measure what B1 alone catches.
    pub reseal: bool,
}

impl Default for Options<'_> {
    fn default() -> Self {
        Options { threads: 1, window: 0, sections: SECTIONS, reseal: true }
    }
}

/// Sweeps `output` (see the module's documentation).
///
/// # Errors
///
/// When the output cannot be read or is not dynamic.
pub fn run(
    output: &[u8],
    inputs: &[Input<'_>],
    map: &Map,
    threads: usize,
    window: u64,
    ctx: &crate::Ctx<'_>,
) -> Result<Sweep, String> {
    run_with(output, inputs, map, ctx, &Options { threads, window, ..Options::default() })
}

/// [`run`] over the sections of `sections` only (each one of
/// [`SECTIONS`]).
///
/// # Errors
///
/// When the output cannot be read or is not dynamic.
pub fn run_sections(
    output: &[u8],
    inputs: &[Input<'_>],
    map: &Map,
    threads: usize,
    window: u64,
    ctx: &crate::Ctx<'_>,
    sections: &[&str],
) -> Result<Sweep, String> {
    run_with(output, inputs, map, ctx, &Options { threads, window, sections, reseal: true })
}

/// The sweep as `opts` says.
///
/// # Errors
///
/// When the output cannot be read or is not dynamic.
#[allow(clippy::too_many_lines)] // the sites, then the judging
pub fn run_with(
    output: &[u8],
    inputs: &[Input<'_>],
    map: &Map,
    ctx: &crate::Ctx<'_>,
    opts: &Options<'_>,
) -> Result<Sweep, String> {
    let image = Image::read(output)?;
    let out = DynOut::read(&image).ok_or("not a dynamic output")?;
    let verify = |b: &[u8]| crate::verify_with(b, inputs, map, ctx);
    let base = verify(output);
    let base_keys = keys(&base);
    let ids = crate::buildid::id_ranges(output);
    let ran = crate::plt::executed(&image);
    let mut bytes = Vec::new();
    // (file offset, Byte)
    let mut sites: Vec<(usize, Byte)> = Vec::new();
    for &name in SECTIONS.iter().filter(|n| opts.sections.contains(n)) {
        for (si, s) in image.sections.iter().enumerate().filter(|(_, s)| s.name == name) {
            if s.shdr.sh_type == SHT_NOBITS || s.shdr.sh_size == 0 {
                continue;
            }
            let (addr, size) = (s.shdr.sh_addr, s.shdr.sh_size);
            let alloc = s.shdr.sh_flags & mortar_elf::consts::SHF_ALLOC != 0;
            let data = if alloc { image.bytes(addr, size) } else { image.section_data(si) };
            let Some(data) = data else { continue };
            let file = s.shdr.sh_offset as usize;
            let ranges: Vec<(u64, u64)> = if name == ".eh_frame" {
                plt_eh_ranges(&image, addr, data)
            } else if name == BUILD_ID {
                // The descriptors only (the id's own case, §12.3).
                ids.iter()
                    .filter(|r| r.start >= file && r.end <= file + data.len())
                    .map(|r| ((r.start - file) as u64, (r.end - file) as u64))
                    .collect()
            } else if opts.window > 0 && size > 2 * opts.window {
                vec![(0, opts.window), (size - opts.window, size)]
            } else {
                vec![(0, size)]
            };
            for (lo, hi) in ranges {
                for off in lo..hi {
                    let (kind, instance) = if name == crate::gdbindex::NAME {
                        crate::gdbindex::field(data, off)
                    } else {
                        field(&image, &out, name, addr, size, off, &ran)
                    };
                    sites.push((
                        file + off as usize,
                        Byte {
                            section: name,
                            off,
                            kind,
                            instance,
                            caught: [false; 2],
                            rules: [Vec::new(), Vec::new()],
                            panicked: false,
                        },
                    ));
                }
            }
        }
    }
    type Verdict = ([bool; 2], [Vec<String>; 2], bool);
    let judge = |&(o, _): &(usize, Byte)| -> Verdict {
        let mut r = [false; 2];
        let mut rules: [Vec<String>; 2] = [Vec::new(), Vec::new()];
        let mut panicked = false;
        let in_id = ids.iter().any(|x| x.contains(&o));
        for (k, f) in FLIPS.iter().enumerate() {
            let mut m = output.to_vec();
            m[o] ^= f;
            if opts.reseal && !in_id {
                crate::buildid::reseal(&mut m, map);
            }
            // A panic is caught (the mutant is reported) and named.
            if let Ok(rep) = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| verify(&m))) {
                let new: std::collections::BTreeSet<String> = rep
                    .findings
                    .iter()
                    .filter(|x| !base_keys.contains(&(x.rule.id(), x.addr, x.message.clone())))
                    .map(|x| x.rule.id())
                    .collect();
                r[k] = !new.is_empty();
                rules[k] = new.into_iter().collect();
            } else {
                r[k] = true;
                panicked = true;
            }
        }
        (r, rules, panicked)
    };
    let threads = opts.threads.max(1).min(sites.len().max(1));
    let chunk = sites.len().div_ceil(threads).max(1);
    let verdicts: Vec<Verdict> = std::thread::scope(|sc| {
        let hs: Vec<_> = sites
            .chunks(chunk)
            .map(|c| sc.spawn(move || c.iter().map(judge).collect::<Vec<_>>()))
            .collect();
        hs.into_iter().flat_map(|h| h.join().expect("a sweep thread")).collect()
    });
    for ((_, mut b), (c, rules, p)) in sites.into_iter().zip(verdicts) {
        b.caught = c;
        b.rules = rules;
        b.panicked = p;
        bytes.push(b);
    }
    Ok(Sweep { baseline: base.findings.len(), bytes })
}
